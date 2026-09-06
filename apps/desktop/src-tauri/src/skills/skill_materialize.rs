// ============================================================================
// Skills Module - skill_materialize
// Per-harness disable for shared-folder skills (spec: "Materialize"). A
// harness that symlinks its whole skills dir at the shared root (Case B in
// docs/spec-event-store.md) can't unlink one skill - `explode_shared_dir`
// converts that whole-dir link into a real directory of per-skill symlinks
// once, after which `unlink_harness`/`relink_harness` (Case A) toggle
// individual skills. `materialized_roots`/`materialized_disabled` in
// event_store.rs are the durable record of which roots were converted and
// which skills are deliberately unlinked; `reconcile_materialized_root` keeps
// the filesystem a faithful projection of that record plus the shared root's
// current contents. Every function here takes `&EventStore` and plain paths
// so they're testable without Tauri - see event_commands.rs for the IPC
// wrappers.
//
// Symlink targets inside an exploded directory are written relative to the
// directory holding them, so `~/.claude/skills/<name>` reads
// `../../.agents/skills/<name>` - byte-identical to what `npx skills` v1.5.23
// creates in symlink mode, and verified against a real install. Relative
// targets survive the whole home directory being moved or restored under a
// different user name, which absolute ones do not. `relative_link_target`
// falls back to the absolute path whenever no relative route exists (separate
// volumes, or a non-absolute input). The exploded directory is staged at a
// temp path in `root`'s own parent, so it sits at the same depth as `root`
// and the targets are already correct before the rename.
// ============================================================================

use std::collections::HashSet;
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::event_store::{
    allocate_id, fingerprint_path, EventDraft, EventRow, EventStatus, EventStore, InverseOp,
};

struct StagingDirectory(PathBuf);

impl StagingDirectory {
    fn persist(self) {
        std::mem::forget(self);
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Validates that `root` is safe for `materialize_harness_root` to explode:
/// a symlink whose canonical target's last two path components are
/// `.agents/skills`. Shared by the command-level validation and its tests -
/// `explode_shared_dir` itself only checks "is a symlink" since its other
/// caller (`set_shared_harness_skill_enabled`, before Part 2) already knows
/// the root is the shared one.
pub fn validate_materialize_root(root: &Path) -> Result<PathBuf, String> {
    let meta = fs::symlink_metadata(root)
        .map_err(|e| format!("Failed to stat {}: {e}", root.display()))?;
    if !meta.file_type().is_symlink() {
        return Err(format!("{} is not a symlink", root.display()));
    }
    let canonical =
        fs::canonicalize(root).map_err(|e| format!("Failed to resolve {}: {e}", root.display()))?;
    let components: Vec<_> = canonical
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    let ends_in_shared_skills = components.len() >= 2
        && components[components.len() - 2] == ".agents"
        && components[components.len() - 1] == "skills";
    if !ends_in_shared_skills {
        return Err(format!(
            "{} does not resolve into a .agents/skills folder",
            root.display()
        ));
    }
    Ok(canonical)
}

/// Converts `root` (a symlink whose canonical target is the shared skills
/// dir) into a real directory containing one absolute per-skill symlink for
/// every skill directory in the shared root. Registers `root` as a
/// materialized root on success. See the module header for the staging
/// order and the choice of absolute targets.
pub fn explode_shared_dir(
    store: &EventStore,
    root: &Path,
    harness: &str,
) -> Result<String, String> {
    explode_shared_dir_with_event_id_and_hook(store, root, harness, &allocate_id(), &|_| Ok(()))
}

/// Converts a shared root with the child event id reserved by a parent Make
/// operation. The hook exists for crash-boundary tests.
pub(crate) fn explode_shared_dir_with_event_id_and_hook(
    store: &EventStore,
    root: &Path,
    harness: &str,
    id: &str,
    after_phase: &dyn Fn(&str) -> Result<(), String>,
) -> Result<String, String> {
    let meta = fs::symlink_metadata(root)
        .map_err(|e| format!("Failed to stat {}: {e}", root.display()))?;
    if !meta.file_type().is_symlink() {
        return Err(format!("{} is not a symlink", root.display()));
    }
    let literal_target =
        fs::read_link(root).map_err(|e| format!("Failed to read link {}: {e}", root.display()))?;
    let shared_root =
        fs::canonicalize(root).map_err(|e| format!("Failed to resolve {}: {e}", root.display()))?;
    let parent = root
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", root.display()))?;
    let pre_fingerprint = fingerprint_path(root);

    // Phase 1: build the replacement directory complete, at a temp path in
    // the same parent, before anything at `root` is touched.
    let tmp = parent.join(format!(".skill-studio-materialize-{id}"));
    let staging = StagingDirectory(tmp.clone());
    build_exploded_dir(&tmp, &shared_root)?;
    let staged_fingerprint = fingerprint_path(&tmp);

    let inverse = InverseOp::RecreateSymlink {
        link: root.to_path_buf(),
        target: literal_target.clone(),
        pre_fingerprint,
        post_fingerprint: None,
    };
    store.record(
        id,
        EventDraft {
            kind: "explode_shared_dir".to_string(),
            skill: String::new(),
            harness: Some(harness.to_string()),
            scope: None,
            project_path: None,
            payload: serde_json::json!({
                "root": root,
                "shared_root": shared_root,
                "staging": tmp,
                "literal_root_target": literal_target,
                "staged_fingerprint": staged_fingerprint,
            }),
            inverse: Some(
                serde_json::to_value(&inverse)
                    .map_err(|e| format!("Failed to serialize inverse: {e}"))?,
            ),
            backup_dir: None,
            restorable: true,
        },
    )?;
    if let Err(error) = after_phase("materialize_staged") {
        staging.persist();
        return Err(error);
    }

    let mutate: Result<(), String> = (|| {
        // Re-resolve the symlink right before replacing it, so a change
        // between the caller's validation (and this function's own initial
        // read above) and this mutation can't slip a different target past
        // the check that ran a moment ago.
        let current = fs::canonicalize(root)
            .map_err(|e| format!("Failed to resolve {}: {e}", root.display()))?;
        if current != shared_root {
            return Err(format!(
                "{} changed since it was validated; aborting",
                root.display()
            ));
        }
        fs::remove_file(root).map_err(|e| format!("Failed to remove {}: {e}", root.display()))?;
        after_phase("materialize_root_removed")?;
        rename_directory_without_replace(&tmp, root)?;
        after_phase("materialize_published")
    })();

    match mutate {
        Ok(()) => {
            let post_fp = fingerprint_path(root);
            let finalize = (|| {
                store.patch_inverse_post_fingerprint(id, &post_fp)?;
                store.register_materialized_root(root, harness, &shared_root, id)?;
                store.finish(id, EventStatus::Done)?;
                Ok(id.to_string())
            })();
            match finalize {
                Ok(event_id) => Ok(event_id),
                Err(error) => {
                    let _ = fs::remove_dir_all(root);
                    let _ = create_symlink(&literal_target, root);
                    let _ = store.unregister_materialized_root(root);
                    let _ = store.finish(id, EventStatus::Failed);
                    Err(error)
                }
            }
        }
        Err(e) => {
            if e.starts_with("injected independent-copy crash") {
                staging.persist();
                return Err(e);
            }
            // Rollback: if the temp dir is still there and `root` is gone,
            // the crash/error landed between removing the link and renaming
            // the temp dir into place - put the original link back so the
            // harness isn't left with nothing.
            if tmp.exists() && fs::symlink_metadata(root).is_err() {
                let _ = create_symlink(&literal_target, root);
            }
            let _ = store.finish(id, EventStatus::Failed);
            Err(e)
        }
    }
}

fn path_c_string(path: &Path) -> Result<CString, String> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| format!("Path contains a null byte: {}", path.display()))
}

/// Moves a staged directory into an absent destination in one operation. It
/// never replaces content that appeared after the caller's last check.
fn rename_directory_without_replace(from: &Path, to: &Path) -> Result<(), String> {
    let from_c = path_c_string(from)?;
    let to_c = path_c_string(to)?;
    #[cfg(target_vendor = "apple")]
    let result = {
        // Both pointers remain valid for this call and point to null-terminated path bytes.
        unsafe { libc::renamex_np(from_c.as_ptr(), to_c.as_ptr(), libc::RENAME_EXCL) }
    };
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let result = {
        // Both pointers remain valid for this call and AT_FDCWD makes each path absolute or cwd-relative.
        unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from_c.as_ptr(),
                libc::AT_FDCWD,
                to_c.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        }
    };
    #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
    return Err(
        "Publishing a materialized root without replacement is unsupported on this platform"
            .to_string(),
    );

    #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
    if result == 0 {
        Ok(())
    } else {
        Err(format!(
            "Failed to publish staged materialized root at {} without replacing existing content: {}",
            to.display(),
            std::io::Error::last_os_error()
        ))
    }
}

fn materialize_event_path(event: &EventRow, key: &str) -> Result<PathBuf, String> {
    event
        .payload
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| format!("Interrupted materialize event has no {key}"))
}

fn finish_recovered_materialized_root(
    store: &EventStore,
    event: &EventRow,
    root: &Path,
    shared_root: &Path,
) -> Result<(), String> {
    let post_fingerprint = fingerprint_path(root);
    store.patch_inverse_post_fingerprint(&event.id, &post_fingerprint)?;
    store.register_materialized_root(
        root,
        event.harness.as_deref().unwrap_or_default(),
        shared_root,
        &event.id,
    )?;
    store.finish(&event.id, EventStatus::Done)
}

/// Recovers the exact materialization child reserved by a Make operation.
/// Unknown root or staging content remains untouched and interrupted.
pub(crate) fn reconcile_connected_materialize_event(
    store: &EventStore,
    event: &EventRow,
) -> Result<(), String> {
    reconcile_connected_materialize_event_with_hook(store, event, &|| Ok(()))
}

pub(crate) fn reconcile_connected_materialize_event_with_hook(
    store: &EventStore,
    event: &EventRow,
    before_publish: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    if event.kind != "explode_shared_dir" || event.status != "interrupted" {
        return Ok(());
    }
    let root = materialize_event_path(event, "root")?;
    let shared_root = materialize_event_path(event, "shared_root")?;
    let staging = materialize_event_path(event, "staging")?;
    let literal_target = materialize_event_path(event, "literal_root_target")?;
    let staged_fingerprint = event
        .payload
        .get("staged_fingerprint")
        .and_then(serde_json::Value::as_str)
        .ok_or("Interrupted materialize event has no staged_fingerprint")?;
    let expected_staging = root
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", root.display()))?
        .join(format!(".skill-studio-materialize-{}", event.id));
    if staging != expected_staging {
        return Err(
            "Interrupted materialize event has an inconsistent staging path; preserved the filesystem unchanged"
                .to_string(),
        );
    }
    let staging_is_exact = fs::symlink_metadata(&staging)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        && fingerprint_path(&staging) == staged_fingerprint;
    let staging_is_absent = fs::symlink_metadata(&staging)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);

    match fs::symlink_metadata(&root) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                && fs::read_link(&root).ok().as_ref() == Some(&literal_target) =>
        {
            if staging_is_exact {
                fs::remove_dir_all(&staging).map_err(|error| {
                    format!(
                        "Failed to remove recovered materialize staging {}: {error}",
                        staging.display()
                    )
                })?;
            } else if !staging_is_absent {
                return Err("Interrupted materialize event found changed staging content; preserved it unchanged".to_string());
            }
            store.finish(&event.id, EventStatus::Failed)
        }
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            if !staging_is_absent || fingerprint_path(&root) != staged_fingerprint {
                return Err("Interrupted materialize event found unknown root content; preserved it unchanged".to_string());
            }
            finish_recovered_materialized_root(store, event, &root, &shared_root)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if staging_is_exact {
                before_publish()?;
                rename_directory_without_replace(&staging, &root)?;
                finish_recovered_materialized_root(store, event, &root, &shared_root)
            } else if staging_is_absent {
                create_symlink(&literal_target, &root).map_err(|error| {
                    format!(
                        "Failed to recreate materialized root link at {} without replacing existing content: {error}",
                        root.display()
                    )
                })?;
                store.finish(&event.id, EventStatus::Failed)
            } else {
                Err("Interrupted materialize event found changed staging content; preserved it unchanged".to_string())
            }
        }
        Ok(_) => Err(
            "Interrupted materialize event found unknown root content; preserved it unchanged"
                .to_string(),
        ),
        Err(error) => Err(format!(
            "Failed to identify interrupted materialize root {}: {error}",
            root.display()
        )),
    }
}

/// Exact identity for converting one whole harness root and then disabling one deployment.
pub struct ConvertThenDisableRequest<'a> {
    pub root: &'a Path,
    pub shared_root: &'a Path,
    pub skill: &'a str,
    pub harness: &'a str,
    pub deployment_id: &'a str,
    pub deployment_path: &'a Path,
    pub scope: &'a str,
    pub project_path: Option<&'a str>,
}

/// Persists the combined intent before converting the root, then disables only the selected link.
pub fn convert_root_then_disable(
    store: &EventStore,
    request: ConvertThenDisableRequest<'_>,
) -> Result<(), String> {
    convert_root_then_disable_with_hook(store, request, &|_| Ok(()))
}

pub(crate) fn convert_root_then_disable_with_hook(
    store: &EventStore,
    request: ConvertThenDisableRequest<'_>,
    after_phase: &dyn Fn(&str) -> Result<(), String>,
) -> Result<(), String> {
    let intent_id = allocate_id();
    let materialize_event_id = allocate_id();
    store.record(
        &intent_id,
        EventDraft {
            kind: "materialize_then_disable".to_string(),
            skill: request.skill.to_string(),
            harness: Some(request.harness.to_string()),
            scope: Some(request.scope.to_string()),
            project_path: request.project_path.map(str::to_string),
            payload: serde_json::json!({
                "root": request.root,
                "shared_root": request.shared_root,
                "skill": request.skill,
                "harness": request.harness,
                "deployment_id": request.deployment_id,
                "deployment_path": request.deployment_path,
                "materialize_event_id": materialize_event_id,
            }),
            inverse: None,
            backup_dir: None,
            restorable: false,
        },
    )?;
    after_phase("intent_recorded")?;

    if let Err(error) = explode_shared_dir_with_event_id_and_hook(
        store,
        request.root,
        request.harness,
        &materialize_event_id,
        after_phase,
    ) {
        if !error.starts_with("injected independent-copy crash") {
            let _ = store.finish(&intent_id, EventStatus::Failed);
        }
        return Err(error);
    }
    after_phase("conversion_completed")?;

    let disable_result = match after_phase("before_disable") {
        Ok(()) => unlink_harness(store, request.root, request.skill, request.harness),
        Err(error) if error.starts_with("injected independent-copy crash") => return Err(error),
        Err(error) => Err(error),
    };
    match disable_result {
        Ok(()) => {
            after_phase("disable_completed")?;
            store.finish(&intent_id, EventStatus::Done)
        }
        Err(disable_error) => {
            match rollback_unchanged_materialization(store, request.root, &materialize_event_id) {
                Ok(true) => {
                    store.finish(&intent_id, EventStatus::Failed)?;
                    Err(format!(
                        "Could not turn off {} after conversion; the conversion was rolled back: {disable_error}",
                        request.skill
                    ))
                }
                Ok(false) | Err(_) => Err(format!(
                    "Conversion completed, but turning off {} did not complete. Restart Skill Studio to recover: {disable_error}",
                    request.skill
                )),
            }
        }
    }
}

fn rollback_unchanged_materialization(
    store: &EventStore,
    root: &Path,
    materialize_event_id: &str,
) -> Result<bool, String> {
    let event = store
        .get(materialize_event_id)?
        .ok_or_else(|| format!("Materialize event {materialize_event_id} was not found"))?;
    if event.status != "done" {
        return Ok(false);
    }
    let expected = event
        .inverse
        .as_ref()
        .and_then(|inverse| inverse.get("post_fingerprint"))
        .and_then(serde_json::Value::as_str)
        .ok_or("Materialize event has no completed fingerprint")?;
    if fingerprint_path(root) != expected {
        return Ok(false);
    }
    store.restore(materialize_event_id, false)?;
    store.unregister_materialized_root(root)?;
    Ok(true)
}

/// Completes or safely rolls back one interrupted convert-and-disable intent.
pub fn reconcile_interrupted_convert_then_disable(
    store: &EventStore,
    event: &EventRow,
) -> Result<(), String> {
    if event.kind != "materialize_then_disable" || event.status != "interrupted" {
        return Ok(());
    }
    let root = materialize_event_path(event, "root")?;
    let shared_root = materialize_event_path(event, "shared_root")?;
    let deployment_path = materialize_event_path(event, "deployment_path")?;
    let skill = event
        .payload
        .get("skill")
        .and_then(serde_json::Value::as_str)
        .ok_or("Interrupted convert-and-disable intent has no skill")?;
    let harness = event
        .payload
        .get("harness")
        .and_then(serde_json::Value::as_str)
        .ok_or("Interrupted convert-and-disable intent has no harness")?;
    let materialize_event_id = event
        .payload
        .get("materialize_event_id")
        .and_then(serde_json::Value::as_str)
        .ok_or("Interrupted convert-and-disable intent has no materialize_event_id")?;
    if deployment_path != root.join(skill) {
        return Err(
            "Interrupted convert-and-disable deployment identity is inconsistent".to_string(),
        );
    }

    let Some(mut materialize_event) = store.get(materialize_event_id)? else {
        if fs::symlink_metadata(&root).is_ok_and(|metadata| metadata.file_type().is_symlink())
            && fs::canonicalize(&root).ok().as_deref()
                == fs::canonicalize(&shared_root).ok().as_deref()
        {
            store.finish(&event.id, EventStatus::Failed)?;
            return Ok(());
        }
        return Err(format!(
            "Materialize event {materialize_event_id} was not found and the original root is not intact"
        ));
    };
    if materialize_event.status == "interrupted" {
        reconcile_connected_materialize_event(store, &materialize_event)?;
        materialize_event = store
            .get(materialize_event_id)?
            .ok_or_else(|| format!("Materialize event {materialize_event_id} was not found"))?;
    }

    if fs::symlink_metadata(&root).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        if fs::canonicalize(&root).ok().as_deref() == fs::canonicalize(&shared_root).ok().as_deref()
        {
            store.finish(&event.id, EventStatus::Failed)?;
            return Ok(());
        }
        return Err("Interrupted convert-and-disable found a different root link".to_string());
    }
    if materialize_event.status != "done" {
        return Err(
            "Interrupted convert-and-disable has no completed conversion to recover".to_string(),
        );
    }

    match fs::symlink_metadata(&deployment_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            store.set_materialized_disabled(&root, skill, true)?;
            store.finish(&event.id, EventStatus::Done)
        }
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let expected_target = shared_root.join(skill);
            if fs::canonicalize(&deployment_path).ok() != fs::canonicalize(&expected_target).ok() {
                return Err(
                    "Interrupted convert-and-disable found a changed selected deployment link"
                        .to_string(),
                );
            }
            match unlink_harness(store, &root, skill, harness) {
                Ok(()) => store.finish(&event.id, EventStatus::Done),
                Err(error) => {
                    if rollback_unchanged_materialization(store, &root, materialize_event_id)? {
                        store.finish(&event.id, EventStatus::Failed)?;
                        Ok(())
                    } else {
                        Err(format!(
                            "Interrupted convert-and-disable could not turn off {skill}: {error}"
                        ))
                    }
                }
            }
        }
        Ok(_) => Err(
            "Interrupted convert-and-disable found changed selected deployment content".to_string(),
        ),
        Err(error) => Err(format!(
            "Interrupted convert-and-disable could not inspect {}: {error}",
            deployment_path.display()
        )),
    }
}

/// Populates `tmp` with one symlink per top-level directory in `shared_root`,
/// named after it, each target written relative to `tmp` itself.
fn build_exploded_dir(tmp: &Path, shared_root: &Path) -> Result<(), String> {
    fs::create_dir_all(tmp).map_err(|e| format!("Failed to create {}: {e}", tmp.display()))?;
    for entry in fs::read_dir(shared_root)
        .map_err(|e| format!("Failed to read {}: {e}", shared_root.display()))?
    {
        let entry = entry.map_err(|e| format!("Failed to read dir entry: {e}"))?;
        let is_dir = entry
            .metadata()
            .map(|m| m.is_dir())
            .map_err(|e| format!("Failed to stat {}: {e}", entry.path().display()))?;
        if !is_dir {
            continue;
        }
        let name = entry.file_name();
        let link = tmp.join(&name);
        create_symlink(&relative_link_target(tmp, &shared_root.join(&name)), &link)?;
    }
    Ok(())
}

/// Removes `<root>/<skill>`, which must be a symlink - the per-skill
/// materialize-disable (Case A). Marks the skill disabled in
/// `materialized_disabled` when `root` is a registered materialized root.
pub fn unlink_harness(
    store: &EventStore,
    root: &Path,
    skill: &str,
    harness: &str,
) -> Result<(), String> {
    let link = root.join(skill);
    let meta = fs::symlink_metadata(&link)
        .map_err(|e| format!("Failed to stat {}: {e}", link.display()))?;
    if !meta.file_type().is_symlink() {
        return Err(format!("{} is not a symlink", link.display()));
    }
    let literal_target =
        fs::read_link(&link).map_err(|e| format!("Failed to read link {}: {e}", link.display()))?;
    let pre_fingerprint = fingerprint_path(&link);

    let id = allocate_id();
    let inverse = InverseOp::RecreateSymlink {
        link: link.clone(),
        target: literal_target,
        pre_fingerprint,
        post_fingerprint: None,
    };
    store.record(
        &id,
        EventDraft {
            kind: "unlink_harness".to_string(),
            skill: skill.to_string(),
            harness: Some(harness.to_string()),
            scope: None,
            project_path: None,
            payload: serde_json::json!({ "root": root }),
            inverse: Some(
                serde_json::to_value(&inverse)
                    .map_err(|e| format!("Failed to serialize inverse: {e}"))?,
            ),
            backup_dir: None,
            restorable: true,
        },
    )?;

    let removed =
        fs::remove_file(&link).map_err(|e| format!("Failed to remove {}: {e}", link.display()));
    finish_link_event(store, &id, &link, removed)?;
    if store.materialized_root(root)?.is_some() {
        store.set_materialized_disabled(root, skill, true)?;
    }
    Ok(())
}

/// Recreates `<root>/<skill>` as a symlink into `root`'s registered
/// materialized shared root - the per-skill materialize-enable (Case A).
/// `root` must already be a registered materialized root; use
/// `explode_shared_dir` first for a whole-dir link.
pub fn relink_harness(
    store: &EventStore,
    root: &Path,
    skill: &str,
    harness: &str,
) -> Result<(), String> {
    let materialized = store
        .materialized_root(root)?
        .ok_or_else(|| format!("{} is not a materialized root", root.display()))?;
    let target = PathBuf::from(&materialized.shared_root).join(skill);
    let link = root.join(skill);
    let pre_fingerprint = fingerprint_path(&link);

    let id = allocate_id();
    let inverse = InverseOp::RemoveSymlink {
        link: link.clone(),
        pre_fingerprint,
        post_fingerprint: None,
    };
    store.record(
        &id,
        EventDraft {
            kind: "relink_harness".to_string(),
            skill: skill.to_string(),
            harness: Some(harness.to_string()),
            scope: None,
            project_path: None,
            payload: serde_json::json!({ "root": root, "target": target }),
            inverse: Some(
                serde_json::to_value(&inverse)
                    .map_err(|e| format!("Failed to serialize inverse: {e}"))?,
            ),
            backup_dir: None,
            restorable: true,
        },
    )?;

    // The event payload records where the link points; the link on disk
    // spells that same destination relative to `root`.
    let created = create_symlink(&relative_link_target(root, &target), &link);
    finish_link_event(store, &id, &link, created)?;
    store.set_materialized_disabled(root, skill, false)
}

/// Makes the filesystem under a materialized `root` match
/// `materialized_roots` + `materialized_disabled` + the shared root's
/// current contents: creates a link for every shared-root skill not listed
/// in `materialized_disabled` and missing at `root`, and removes symlinks at
/// `root` whose target no longer exists. Never touches a non-symlink entry.
/// No-op when `root` isn't a registered materialized root.
pub fn reconcile_materialized_root(store: &EventStore, root: &Path) -> Result<(), String> {
    let Some(materialized) = store.materialized_root(root)? else {
        return Ok(());
    };
    let shared_root = PathBuf::from(&materialized.shared_root);
    let disabled: HashSet<String> = store.materialized_disabled(root)?.into_iter().collect();

    let shared_skills: Vec<String> = fs::read_dir(&shared_root)
        .map_err(|e| format!("Failed to read {}: {e}", shared_root.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.metadata().map(|m| m.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    for skill in &shared_skills {
        if disabled.contains(skill) {
            continue;
        }
        if fs::symlink_metadata(root.join(skill)).is_ok() {
            continue; // already linked, or a real dir the user dropped in - leave it
        }
        relink_harness(store, root, skill, &materialized.harness)?;
    }

    for entry in
        fs::read_dir(root).map_err(|e| format!("Failed to read {}: {e}", root.display()))?
    {
        let entry = entry.map_err(|e| format!("Failed to read dir entry: {e}"))?;
        let path = entry.path();
        let smeta = fs::symlink_metadata(&path)
            .map_err(|e| format!("Failed to stat {}: {e}", path.display()))?;
        if !smeta.file_type().is_symlink() {
            continue; // a real folder the user dropped in stays theirs
        }
        if fs::metadata(&path).is_ok() {
            continue; // target still resolves
        }
        let skill = entry.file_name().to_string_lossy().into_owned();
        remove_stale_link(store, &path, &skill, &materialized.harness)?;
    }

    Ok(())
}

/// Removes a symlink whose shared-root target vanished out from under a
/// materialized root - not a user disable, so it doesn't touch
/// `materialized_disabled`: if the skill reappears in the shared root, the
/// next reconcile relinks it. Recorded as its own event kind
/// (`reconcile_remove_stale_link`, payload: `{root}`) so it's restorable
/// like any other mutation.
fn remove_stale_link(
    store: &EventStore,
    link: &Path,
    skill: &str,
    harness: &str,
) -> Result<(), String> {
    let literal_target =
        fs::read_link(link).map_err(|e| format!("Failed to read link {}: {e}", link.display()))?;
    let pre_fingerprint = fingerprint_path(link);
    let id = allocate_id();
    let inverse = InverseOp::RecreateSymlink {
        link: link.to_path_buf(),
        target: literal_target,
        pre_fingerprint,
        post_fingerprint: None,
    };
    store.record(
        &id,
        EventDraft {
            kind: "reconcile_remove_stale_link".to_string(),
            skill: skill.to_string(),
            harness: Some(harness.to_string()),
            scope: None,
            project_path: None,
            payload: serde_json::json!({ "root": link.parent() }),
            inverse: Some(
                serde_json::to_value(&inverse)
                    .map_err(|e| format!("Failed to serialize inverse: {e}"))?,
            ),
            backup_dir: None,
            restorable: true,
        },
    )?;
    let removed =
        fs::remove_file(link).map_err(|e| format!("Failed to remove {}: {e}", link.display()));
    finish_link_event(store, &id, link, removed)
}

/// Refuses restoring an `explode_shared_dir` event while per-skill state
/// depends on its real directory. Force restore must not bypass this guard.
pub fn restore_guard_for_explode(
    store: &EventStore,
    event: &EventRow,
    home: &Path,
) -> Result<(), String> {
    if event.kind != "explode_shared_dir" {
        return Ok(());
    }
    let root = event
        .payload
        .get("root")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "explode_shared_dir event has no root in its payload".to_string())?;
    let disabled = store.materialized_disabled(Path::new(root))?;
    if !disabled.is_empty() {
        return Err(format!(
            "Cannot undo the whole-folder link while these skills are individually disabled here: {} - re-enable them first",
            disabled.join(", ")
        ));
    }
    let dependent = store
        .active_events_of_kind("make_independent_copy")?
        .into_iter()
        .find(|candidate| {
            candidate
                .payload
                .get("materialize_event_id")
                .and_then(|value| value.as_str())
                == Some(event.id.as_str())
                || candidate
                    .payload
                    .get("expected_root")
                    .and_then(|value| value.as_str())
                    == Some(root)
        });
    if let Some(dependent) = dependent {
        return Err(format!(
            "Cannot undo the whole-folder link while {} has an independent Copy in this root; undo independent copies first",
            dependent.skill
        ));
    }
    let registry = super::skill_fork_registry::read_fork_registry(home)?;
    if let Some(record) = registry
        .copies
        .values()
        .find(|record| !record.disabled && record.path.parent() == Some(Path::new(root)))
    {
        return Err(format!(
            "Cannot undo the whole-folder link while {} has an independent Copy in this root; undo independent copies first",
            record.name
        ));
    }
    Ok(())
}

/// Finishes a recorded event after its filesystem mutation runs: on success,
/// patches the inverse's post-fingerprint and marks the event `done`; on
/// failure, marks it `failed` and propagates the error. Shared by
/// `unlink_harness`, `relink_harness`, and `remove_stale_link`, which differ
/// only in the mutation and what (if anything) they do after it succeeds.
fn finish_link_event(
    store: &EventStore,
    id: &str,
    link: &Path,
    result: Result<(), String>,
) -> Result<(), String> {
    match result {
        Ok(()) => {
            let post_fp = fingerprint_path(link);
            store.patch_inverse_post_fingerprint(id, &post_fp)?;
            store.finish(id, EventStatus::Done)
        }
        Err(e) => {
            let _ = store.finish(id, EventStatus::Failed);
            Err(e)
        }
    }
}

/// Removes a single broken deployment symlink, for the SkillPage "Repair
/// this location" flow (see `event_commands::repair_skill_link`). Unlike
/// `unlink_harness`, `link` is the deployment's own path directly (not
/// `root.join(skill)`), and its target is expected to already be broken -
/// this doesn't require it to resolve.
pub fn repair_remove_link(
    store: &EventStore,
    link: &Path,
    skill: &str,
    harness: &str,
) -> Result<(), String> {
    let meta = fs::symlink_metadata(link)
        .map_err(|e| format!("Failed to stat {}: {e}", link.display()))?;
    if !meta.file_type().is_symlink() {
        return Err(format!("{} is not a symlink", link.display()));
    }
    let literal_target =
        fs::read_link(link).map_err(|e| format!("Failed to read link {}: {e}", link.display()))?;
    let pre_fingerprint = fingerprint_path(link);

    let id = allocate_id();
    let inverse = InverseOp::RecreateSymlink {
        link: link.to_path_buf(),
        target: literal_target,
        pre_fingerprint,
        post_fingerprint: None,
    };
    store.record(
        &id,
        EventDraft {
            kind: "repair_remove_link".to_string(),
            skill: skill.to_string(),
            harness: Some(harness.to_string()),
            scope: None,
            project_path: None,
            payload: serde_json::json!({ "link": link }),
            inverse: Some(
                serde_json::to_value(&inverse)
                    .map_err(|e| format!("Failed to serialize inverse: {e}"))?,
            ),
            backup_dir: None,
            restorable: true,
        },
    )?;

    let removed =
        fs::remove_file(link).map_err(|e| format!("Failed to remove {}: {e}", link.display()));
    finish_link_event(store, &id, link, removed)
}

/// Repoints a broken deployment symlink at a healthy copy's path, for the
/// SkillPage "Repair this location" flow. The link's *old* (broken) target is
/// recorded as the inverse, so undo restores the exact prior link rather than
/// removing the new one and leaving nothing - the same shape `unlink_harness`
/// undoes to.
pub fn repair_relink_link(
    store: &EventStore,
    link: &Path,
    target: &Path,
    skill: &str,
    harness: &str,
) -> Result<(), String> {
    let meta = fs::symlink_metadata(link)
        .map_err(|e| format!("Failed to stat {}: {e}", link.display()))?;
    if !meta.file_type().is_symlink() {
        return Err(format!("{} is not a symlink", link.display()));
    }
    let literal_target =
        fs::read_link(link).map_err(|e| format!("Failed to read link {}: {e}", link.display()))?;
    let pre_fingerprint = fingerprint_path(link);

    let id = allocate_id();
    let inverse = InverseOp::RecreateSymlink {
        link: link.to_path_buf(),
        target: literal_target,
        pre_fingerprint,
        post_fingerprint: None,
    };
    store.record(
        &id,
        EventDraft {
            kind: "repair_relink_link".to_string(),
            skill: skill.to_string(),
            harness: Some(harness.to_string()),
            scope: None,
            project_path: None,
            payload: serde_json::json!({ "link": link, "target": target }),
            inverse: Some(
                serde_json::to_value(&inverse)
                    .map_err(|e| format!("Failed to serialize inverse: {e}"))?,
            ),
            backup_dir: None,
            restorable: true,
        },
    )?;

    let relinked: Result<(), String> = (|| {
        fs::remove_file(link).map_err(|e| format!("Failed to remove {}: {e}", link.display()))?;
        create_symlink(target, link)
    })();
    finish_link_event(store, &id, link, relinked)
}

/// `target` expressed relative to `link_dir`, the directory the symlink will
/// live in. Returns `target` unchanged when the two share no common ancestor
/// or either is relative, since there is then no relative route to write.
fn relative_link_target(link_dir: &Path, target: &Path) -> PathBuf {
    let (Some(from_dir), Some(to_path)) = (resolved_dir(link_dir), resolved_file(target)) else {
        return target.to_path_buf();
    };
    let from: Vec<_> = from_dir.components().collect();
    let to: Vec<_> = to_path.components().collect();
    let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    // One shared component is the filesystem root alone, which is no
    // relationship at all - a link across two volumes stays absolute.
    if shared < 2 {
        return target.to_path_buf();
    }
    let mut relative = PathBuf::new();
    for _ in shared..from.len() {
        relative.push("..");
    }
    for component in &to[shared..] {
        relative.push(component);
    }
    if relative.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        relative
    }
}

/// `dir` with every symlink in it resolved, so two paths naming one directory
/// through different routes (`/var` and `/private/var` on macOS) compare equal.
fn resolved_dir(dir: &Path) -> Option<PathBuf> {
    if !dir.is_absolute() {
        return None;
    }
    Some(fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()))
}

/// `file` with its parent directory resolved, leaving the final component
/// alone - it names the link's destination and may not exist yet.
fn resolved_file(file: &Path) -> Option<PathBuf> {
    let (parent, name) = (file.parent()?, file.file_name()?);
    Some(resolved_dir(parent)?.join(name))
}

fn create_symlink(target: &Path, link: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
            .map_err(|e| format!("Failed to symlink {}: {e}", link.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        Err("Symlinking is only supported on Unix".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn store(dir: &Path) -> EventStore {
        EventStore::open(&dir.join("app_data")).expect("open store")
    }

    fn write_skill(dir: &Path, name: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test\n---\nBody."),
        )
        .unwrap();
    }

    #[test]
    fn unlink_then_restore_recreates_the_symlink_with_the_same_target() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let home = tmp.path().join("home");
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        let root = home.join(".claude/skills");
        fs::create_dir_all(&root).unwrap();
        symlink("../../.agents/skills/find-bugs", root.join("find-bugs")).unwrap();

        unlink_harness(&store, &root, "find-bugs", "claude-code").unwrap();
        assert!(fs::symlink_metadata(root.join("find-bugs")).is_err());

        let events = store.list(10, Some("find-bugs")).unwrap();
        assert_eq!(events[0].kind, "unlink_harness");
        store.restore(&events[0].id, false).unwrap();

        let restored = root.join("find-bugs");
        assert!(fs::symlink_metadata(&restored)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_link(&restored).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );
    }

    #[test]
    fn validate_materialize_root_refuses_a_real_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".claude/skills");
        fs::create_dir_all(&root).unwrap();

        let err = validate_materialize_root(&root).unwrap_err();
        assert!(err.contains("is not a symlink"), "{err}");
    }

    #[test]
    fn validate_materialize_root_refuses_a_symlink_outside_agents_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("elsewhere");
        fs::create_dir_all(&target).unwrap();
        let root = tmp.path().join(".claude/skills");
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        symlink(&target, &root).unwrap();

        let err = validate_materialize_root(&root).unwrap_err();
        assert!(err.contains(".agents/skills"), "{err}");
    }

    #[test]
    fn validate_materialize_root_accepts_a_symlink_into_agents_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join(".agents/skills");
        fs::create_dir_all(&shared).unwrap();
        let root = tmp.path().join(".claude/skills");
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        symlink(&shared, &root).unwrap();

        let canonical = validate_materialize_root(&root).unwrap();
        assert_eq!(canonical, fs::canonicalize(&shared).unwrap());
    }

    /// `npx skills` v1.5.23 in symlink mode writes each per-skill link as
    /// `../../.agents/skills/<name>`, verified against a real install into a
    /// throwaway HOME. An exploded root has to produce the same shape, or a
    /// moved home directory leaves every link dangling.
    #[test]
    fn exploded_links_are_relative_the_way_the_skills_cli_writes_them() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let home = tmp.path().join("home");
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let root = home.join(".claude/skills");
        symlink(home.join(".agents/skills"), &root).unwrap();

        explode_shared_dir(&store, &root, "claude-code").unwrap();

        assert_eq!(
            fs::read_link(root.join("find-bugs")).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );
    }

    #[test]
    fn convert_then_disable_rolls_back_when_disable_fails_before_mutation() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let home = tmp.path().join("home");
        let shared = home.join(".agents/skills");
        write_skill(&shared.join("find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let root = home.join(".claude/skills");
        symlink(&shared, &root).unwrap();
        let deployment_path = root.join("find-bugs");

        let error = convert_root_then_disable_with_hook(
            &store,
            ConvertThenDisableRequest {
                root: &root,
                shared_root: &shared,
                skill: "find-bugs",
                harness: "claude-code",
                deployment_id: "dep:v1/global/claude-code/universal/find-bugs/-/path",
                deployment_path: &deployment_path,
                scope: "global",
                project_path: None,
            },
            &|phase| {
                if phase == "before_disable" {
                    Err("injected disable failure".to_string())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();

        assert!(error.contains("conversion was rolled back"), "{error}");
        assert!(fs::symlink_metadata(&root)
            .unwrap()
            .file_type()
            .is_symlink());
        let intent = store
            .list(20, Some("find-bugs"))
            .unwrap()
            .into_iter()
            .find(|event| event.kind == "materialize_then_disable")
            .unwrap();
        assert_eq!(intent.status, "failed");
    }

    #[test]
    fn interrupted_convert_then_disable_completes_exact_link_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let home = tmp.path().join("home");
        let shared = home.join(".agents/skills");
        write_skill(&shared.join("find-bugs"), "find-bugs");
        write_skill(&shared.join("write-docs"), "write-docs");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let root = home.join(".claude/skills");
        symlink(&shared, &root).unwrap();
        let same_name_elsewhere = home.join(".codex/skills/find-bugs");
        write_skill(&same_name_elsewhere, "find-bugs");
        let deployment_path = root.join("find-bugs");

        let crash = convert_root_then_disable_with_hook(
            &store,
            ConvertThenDisableRequest {
                root: &root,
                shared_root: &shared,
                skill: "find-bugs",
                harness: "claude-code",
                deployment_id: "dep:v1/global/claude-code/universal/find-bugs/-/path",
                deployment_path: &deployment_path,
                scope: "global",
                project_path: None,
            },
            &|phase| {
                if phase == "conversion_completed" {
                    Err("injected independent-copy crash after conversion".to_string())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert!(crash.starts_with("injected independent-copy crash"));

        store.reconcile_at_startup().unwrap();
        let event = store
            .interrupted_convert_then_disable_events()
            .unwrap()
            .pop()
            .unwrap();
        reconcile_interrupted_convert_then_disable(&store, &event).unwrap();
        reconcile_interrupted_convert_then_disable(&store, &event).unwrap();

        assert!(fs::symlink_metadata(&deployment_path).is_err());
        assert!(fs::symlink_metadata(root.join("write-docs")).is_ok());
        assert!(same_name_elsewhere.join("SKILL.md").is_file());
        assert_eq!(
            store.materialized_disabled(&root).unwrap(),
            vec!["find-bugs".to_string()]
        );
        assert_eq!(store.get(&event.id).unwrap().unwrap().status, "done");
    }

    #[test]
    fn interrupted_intent_before_conversion_is_safely_closed_as_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let home = tmp.path().join("home");
        let shared = home.join(".agents/skills");
        write_skill(&shared.join("find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let root = home.join(".claude/skills");
        symlink(&shared, &root).unwrap();
        let deployment_path = root.join("find-bugs");

        convert_root_then_disable_with_hook(
            &store,
            ConvertThenDisableRequest {
                root: &root,
                shared_root: &shared,
                skill: "find-bugs",
                harness: "claude-code",
                deployment_id: "dep:v1/global/claude-code/universal/find-bugs/-/path",
                deployment_path: &deployment_path,
                scope: "global",
                project_path: None,
            },
            &|phase| {
                if phase == "intent_recorded" {
                    Err("injected independent-copy crash before conversion".to_string())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        store.reconcile_at_startup().unwrap();
        let event = store
            .interrupted_convert_then_disable_events()
            .unwrap()
            .pop()
            .unwrap();

        reconcile_interrupted_convert_then_disable(&store, &event).unwrap();

        assert!(fs::symlink_metadata(&root)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(store.get(&event.id).unwrap().unwrap().status, "failed");
    }

    #[test]
    fn a_relinked_skill_is_relative_too() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let home = tmp.path().join("home");
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let root = home.join(".claude/skills");
        symlink(home.join(".agents/skills"), &root).unwrap();

        explode_shared_dir(&store, &root, "claude-code").unwrap();
        unlink_harness(&store, &root, "find-bugs", "claude-code").unwrap();
        relink_harness(&store, &root, "find-bugs", "claude-code").unwrap();

        assert_eq!(
            fs::read_link(root.join("find-bugs")).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );
    }

    #[test]
    fn a_target_on_another_root_keeps_its_absolute_path() {
        let elsewhere = PathBuf::from("/Volumes/other/skills/find-bugs");
        assert_eq!(
            relative_link_target(Path::new("/Users/x/.claude/skills"), &elsewhere),
            elsewhere
        );
    }

    #[test]
    fn explode_converts_whole_dir_link_and_restore_refuses_while_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let home = tmp.path().join("home");
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        write_skill(&home.join(".agents/skills/write-docs"), "write-docs");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let root = home.join(".claude/skills");
        symlink(home.join(".agents/skills"), &root).unwrap();

        explode_shared_dir(&store, &root, "claude-code").unwrap();
        assert!(!fs::symlink_metadata(&root)
            .unwrap()
            .file_type()
            .is_symlink());
        for name in ["find-bugs", "write-docs"] {
            let link = root.join(name);
            assert!(fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(
                fs::canonicalize(&link).unwrap(),
                fs::canonicalize(home.join(".agents/skills").join(name)).unwrap()
            );
        }

        let events = store.list(10, None).unwrap();
        let explode_event = events
            .iter()
            .find(|e| e.kind == "explode_shared_dir")
            .unwrap();

        // Disable one skill through the materialized root, then refuse to
        // un-materialize while it's disabled.
        unlink_harness(&store, &root, "find-bugs", "claude-code").unwrap();
        let err = restore_guard_for_explode(&store, explode_event, &home).unwrap_err();
        assert!(err.contains("find-bugs"), "{err}");

        // Re-enable, then the restore is allowed and puts the dir-level
        // symlink back.
        relink_harness(&store, &root, "find-bugs", "claude-code").unwrap();
        restore_guard_for_explode(&store, explode_event, &home).unwrap();
        store.restore(&explode_event.id, false).unwrap();
        assert!(fs::symlink_metadata(&root)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::canonicalize(&root).unwrap(),
            fs::canonicalize(home.join(".agents/skills")).unwrap()
        );
    }

    #[test]
    fn reconcile_creates_missing_links_respects_disabled_and_leaves_real_dirs_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let home = tmp.path().join("home");
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let root = home.join(".claude/skills");
        symlink(home.join(".agents/skills"), &root).unwrap();
        explode_shared_dir(&store, &root, "claude-code").unwrap();

        // A skill installed after materialization.
        write_skill(&home.join(".agents/skills/write-docs"), "write-docs");
        // A skill deliberately disabled.
        unlink_harness(&store, &root, "find-bugs", "claude-code").unwrap();
        // A real (non-symlink) folder a user dropped straight into the root.
        write_skill(&root.join("user-made"), "user-made");

        reconcile_materialized_root(&store, &root).unwrap();

        assert!(fs::symlink_metadata(root.join("write-docs"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(fs::symlink_metadata(root.join("find-bugs")).is_err());
        assert!(!fs::symlink_metadata(root.join("user-made"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(root.join("user-made/SKILL.md").is_file());
    }

    #[test]
    fn repair_remove_link_deletes_only_the_symlink_and_records_an_event() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        let link = home.join(".claude/skills/find-bugs");
        // A dangling target on purpose - `repair_remove_link` must not
        // require the target to resolve.
        symlink("/does/not/exist/find-bugs", &link).unwrap();

        repair_remove_link(&store, &link, "find-bugs", "claude-code").unwrap();

        assert!(fs::symlink_metadata(&link).is_err());
        let events = store.list(10, Some("find-bugs")).unwrap();
        assert_eq!(events[0].kind, "repair_remove_link");
        assert_eq!(events[0].status, "done");
    }

    #[test]
    fn repair_relink_link_points_the_path_at_the_target() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let home = tmp.path().join("home");
        write_skill(&home.join(".claude/skills/find-bugs"), "find-bugs");
        let healthy = home.join(".claude/skills/find-bugs");
        fs::create_dir_all(home.join(".codex/skills")).unwrap();
        let broken = home.join(".codex/skills/find-bugs");
        symlink("/does/not/exist/find-bugs", &broken).unwrap();

        repair_relink_link(&store, &broken, &healthy, "find-bugs", "codex").unwrap();

        assert!(fs::symlink_metadata(&broken)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::canonicalize(&broken).unwrap(),
            fs::canonicalize(&healthy).unwrap()
        );

        let events = store.list(10, Some("find-bugs")).unwrap();
        assert_eq!(events[0].kind, "repair_relink_link");

        // Undo restores the original (broken) link, same shape as
        // `unlink_harness`'s undo.
        store.restore(&events[0].id, false).unwrap();
        assert_eq!(
            fs::read_link(&broken).unwrap(),
            PathBuf::from("/does/not/exist/find-bugs")
        );
    }
}
