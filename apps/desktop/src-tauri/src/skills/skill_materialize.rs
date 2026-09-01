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
// Symlink targets inside an exploded directory are absolute: the directory
// is built at a temp path in `root`'s parent, and a robust *relative* target
// from there to `<shared_root>/<skill>` would depend on how far apart `root`
// and `shared_root` are in the filesystem (unlike the one-level shift
// `skill_harness_disable`'s move-aside handles, there's no fixed relationship
// here). An absolute target is always correct and the shared root's path
// itself never gets exploded, so this doesn't create the "moved a relative
// symlink without" hazard.
// ============================================================================

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::agents::AgentId;
use super::event_store::{
    allocate_id, copy_recursive, fingerprint_path, EventDraft, EventRow, EventStatus, EventStore,
    InverseOp,
};

/// Converts `root` (a symlink whose canonical target is the shared skills
/// dir) into a real directory containing one absolute per-skill symlink for
/// every skill directory in the shared root. Registers `root` as a
/// materialized root on success. See the module header for the staging
/// order and the choice of absolute targets.
pub fn explode_shared_dir(store: &EventStore, root: &Path, harness: &str) -> Result<(), String> {
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
    let id = allocate_id();
    let tmp = parent.join(format!(".skill-studio-materialize-{id}"));
    if let Err(e) = build_exploded_dir(&tmp, &shared_root) {
        let _ = fs::remove_dir_all(&tmp);
        return Err(e);
    }

    let inverse = InverseOp::RecreateSymlink {
        link: root.to_path_buf(),
        target: literal_target.clone(),
        pre_fingerprint,
        post_fingerprint: None,
    };
    store.record(
        &id,
        EventDraft {
            kind: "explode_shared_dir".to_string(),
            skill: String::new(),
            harness: Some(harness.to_string()),
            scope: None,
            project_path: None,
            payload: serde_json::json!({ "root": root, "shared_root": shared_root }),
            inverse: Some(
                serde_json::to_value(&inverse)
                    .map_err(|e| format!("Failed to serialize inverse: {e}"))?,
            ),
            backup_dir: None,
        },
    )?;

    let mutate: Result<(), String> = (|| {
        fs::remove_file(root).map_err(|e| format!("Failed to remove {}: {e}", root.display()))?;
        fs::rename(&tmp, root)
            .map_err(|e| format!("Failed to move {} into place: {e}", root.display()))
    })();

    match mutate {
        Ok(()) => {
            let post_fp = fingerprint_path(root);
            store.patch_inverse_post_fingerprint(&id, &post_fp)?;
            store.finish(&id, EventStatus::Done)?;
            store.register_materialized_root(root, harness, &shared_root, &id)
        }
        Err(e) => {
            // Rollback: if the temp dir is still there and `root` is gone,
            // the crash/error landed between removing the link and renaming
            // the temp dir into place - put the original link back so the
            // harness isn't left with nothing.
            if tmp.exists() && fs::symlink_metadata(root).is_err() {
                let _ = create_symlink(&literal_target, root);
            }
            let _ = fs::remove_dir_all(&tmp);
            let _ = store.finish(&id, EventStatus::Failed);
            Err(e)
        }
    }
}

/// Populates `tmp` with one absolute symlink per top-level directory in
/// `shared_root`, named after it.
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
        create_symlink(&shared_root.join(&name), &tmp.join(&name))?;
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
        },
    )?;

    finish_link_event(store, &id, &link, create_symlink(&target, &link))?;
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
        },
    )?;
    let removed =
        fs::remove_file(link).map_err(|e| format!("Failed to remove {}: {e}", link.display()));
    finish_link_event(store, &id, link, removed)
}

/// Refuses restoring an `explode_shared_dir` event while its root has any
/// `materialized_disabled` entries - the whole-dir link can't represent
/// per-skill disables, so un-materializing would silently re-enable them.
/// A no-op for any other event kind.
pub fn restore_guard_for_explode(store: &EventStore, event: &EventRow) -> Result<(), String> {
    if event.kind != "explode_shared_dir" {
        return Ok(());
    }
    let root = event
        .payload
        .get("root")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "explode_shared_dir event has no root in its payload".to_string())?;
    let disabled = store.materialized_disabled(Path::new(root))?;
    if disabled.is_empty() {
        return Ok(());
    }
    Err(format!(
        "Cannot undo the whole-folder link while these skills are individually disabled here: {} - re-enable them first",
        disabled.join(", ")
    ))
}

/// The five harnesses that read the shared `.agents/skills` root directly -
/// Codex and OpenCode natively per-skill, pi/Cursor/Grok Build unconditionally
/// (see skill_harness_disable.rs's module doc). Every one of them is always a
/// `distribute_from_shared` target, whether or not it happens to have its own
/// symlink to the skill yet.
const SHARED_ROOT_READERS: &[AgentId] = &[
    AgentId::Codex,
    AgentId::OpenCode,
    AgentId::Pi,
    AgentId::Cursor,
    AgentId::GrokBuild,
];

/// Whether `entry` is a symlink whose canonical target is `shared_dir` -
/// `distribute_from_shared`'s test for "this harness reaches the skill only
/// through a link into the shared root".
fn is_symlink_into(entry: &Path, shared_dir: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(entry) else {
        return false;
    };
    if !meta.file_type().is_symlink() {
        return false;
    }
    match (fs::canonicalize(entry), fs::canonicalize(shared_dir)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// One thing `distribute_from_shared` does for a target harness: give it a
/// fresh copy where there was nothing, or replace a symlink into the shared
/// dir with one.
enum DistributePlanItem {
    Copy(PathBuf),
    ReplaceSymlink(PathBuf, PathBuf),
}

impl DistributePlanItem {
    fn entry(&self) -> &Path {
        match self {
            DistributePlanItem::Copy(p) => p,
            DistributePlanItem::ReplaceSymlink(p, _) => p,
        }
    }
}

/// Moves `skill` out of the shared root at `root` (e.g. `.agents/skills`) and
/// into a real copy under every harness that reads it, then deletes
/// `root/<skill>`. This is the only way to give pi, Cursor, and Grok Build
/// (which have no per-skill off switch of their own) something they can be
/// individually disabled for - see the module's spec doc,
/// docs/spec-event-store.md.
///
/// Targets are the five `SHARED_ROOT_READERS`, always, plus any other
/// first-class harness (in practice, Claude Code) whose skills dir already
/// symlinks this one skill into `root`. A harness whose whole skills dir is
/// itself a symlink into the shared root is exploded first (its own,
/// separately restorable event), so its entry for `skill` becomes a per-skill
/// symlink before the plan below classifies it. A target that already has a
/// real `skill` dir of its own is left untouched.
pub fn distribute_from_shared(
    store: &EventStore,
    home: &Path,
    root: &Path,
    skill: &str,
) -> Result<(), String> {
    let shared_dir = root.join(skill);
    let meta = fs::symlink_metadata(&shared_dir)
        .map_err(|_| format!("\"{}\" does not exist", shared_dir.display()))?;
    if !meta.is_dir() {
        return Err(format!(
            "\"{}\" is not a real directory",
            shared_dir.display()
        ));
    }

    let agents_dir = root
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", root.display()))?;
    let base = agents_dir
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", agents_dir.display()))?
        .to_path_buf();
    let is_global = base == home;
    let harness_dir = |id: AgentId| -> PathBuf {
        if is_global {
            id.global_skills_dir(home)
        } else {
            id.project_skills_dir(&base)
        }
    };

    let mut targets: Vec<(AgentId, PathBuf)> = SHARED_ROOT_READERS
        .iter()
        .map(|&id| (id, harness_dir(id)))
        .collect();
    let claude_dir = harness_dir(AgentId::ClaudeCode);
    if is_symlink_into(&claude_dir.join(skill), &shared_dir) {
        targets.push((AgentId::ClaudeCode, claude_dir));
    }

    // Whole-dir links convert to per-skill links first, exactly like
    // `set_shared_harness_skill_enabled` - each conversion is its own
    // restorable event, not part of the one below.
    for (id, dir) in &targets {
        let is_whole_dir_link = fs::symlink_metadata(dir)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_whole_dir_link {
            explode_shared_dir(store, dir, id.cli_name())?;
        }
    }

    // Plan the mutation now that every whole-dir link has been converted, so
    // each target's `skill` entry is either absent, a real dir/file to leave
    // alone, or a per-skill symlink into the shared dir to replace.
    let mut plan = Vec::new();
    for (_, dir) in &targets {
        let entry = dir.join(skill);
        match fs::symlink_metadata(&entry) {
            Ok(m) if m.file_type().is_symlink() && is_symlink_into(&entry, &shared_dir) => {
                let former_target = fs::read_link(&entry)
                    .map_err(|e| format!("Failed to read link {}: {e}", entry.display()))?;
                plan.push(DistributePlanItem::ReplaceSymlink(entry, former_target));
            }
            Ok(_) => {} // already has its own thing there - leave it
            Err(_) => plan.push(DistributePlanItem::Copy(entry)),
        }
    }
    let copies: Vec<PathBuf> = plan.iter().map(|item| item.entry().to_path_buf()).collect();
    let symlinks: Vec<(PathBuf, PathBuf)> = plan
        .iter()
        .filter_map(|item| match item {
            DistributePlanItem::ReplaceSymlink(p, t) => Some((p.clone(), t.clone())),
            DistributePlanItem::Copy(_) => None,
        })
        .collect();

    let pre_fingerprint = fingerprint_path(&shared_dir);
    let id = allocate_id();
    store.backup_paths(&id, std::slice::from_ref(&shared_dir))?;

    let inverse = InverseOp::UndistributeFromShared {
        shared_dir: shared_dir.clone(),
        copies,
        copy_fingerprints: Vec::new(),
        symlinks,
        pre_fingerprint,
        post_fingerprint: None,
    };
    store.record(
        &id,
        EventDraft {
            kind: "distribute_from_shared".to_string(),
            skill: skill.to_string(),
            harness: None,
            scope: None,
            project_path: None,
            payload: serde_json::json!({ "root": root, "shared_dir": shared_dir }),
            inverse: Some(
                serde_json::to_value(&inverse)
                    .map_err(|e| format!("Failed to serialize inverse: {e}"))?,
            ),
            backup_dir: Some(format!("backups/{id}")),
        },
    )?;

    let mutate: Result<Vec<String>, String> = (|| {
        let mut fingerprints = Vec::new();
        for item in &plan {
            match item {
                DistributePlanItem::Copy(entry) => {
                    if let Some(parent) = entry.parent() {
                        fs::create_dir_all(parent)
                            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
                    }
                    copy_recursive(&shared_dir, entry)?;
                    fingerprints.push(fingerprint_path(entry));
                }
                DistributePlanItem::ReplaceSymlink(link, _) => {
                    fs::remove_file(link)
                        .map_err(|e| format!("Failed to remove {}: {e}", link.display()))?;
                    copy_recursive(&shared_dir, link)?;
                    fingerprints.push(fingerprint_path(link));
                }
            }
        }
        fs::remove_dir_all(&shared_dir)
            .map_err(|e| format!("Failed to remove {}: {e}", shared_dir.display()))?;
        Ok(fingerprints)
    })();

    match mutate {
        Ok(fingerprints) => {
            store.patch_inverse_copy_fingerprints(&id, &fingerprints)?;
            store.patch_inverse_post_fingerprint(&id, "absent")?;
            store.finish(&id, EventStatus::Done)
        }
        Err(e) => {
            let _ = store.finish(&id, EventStatus::Failed);
            Err(e)
        }
    }
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
        },
    )?;

    let relinked: Result<(), String> = (|| {
        fs::remove_file(link).map_err(|e| format!("Failed to remove {}: {e}", link.display()))?;
        create_symlink(target, link)
    })();
    finish_link_event(store, &id, link, relinked)
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
        let err = restore_guard_for_explode(&store, explode_event).unwrap_err();
        assert!(err.contains("find-bugs"), "{err}");

        // Re-enable, then the restore is allowed and puts the dir-level
        // symlink back.
        relink_harness(&store, &root, "find-bugs", "claude-code").unwrap();
        restore_guard_for_explode(&store, explode_event).unwrap();
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

    /// A fake home with `find-bugs` shared globally, a pre-existing real
    /// Cursor copy, and Claude Code linked to it per-skill. Returns
    /// `(home, root)`.
    fn home_with_shared_skill(tmp: &Path) -> (PathBuf, PathBuf) {
        let home = tmp.join("home");
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::write(home.join(".agents/skills/find-bugs/extra.txt"), "extra").unwrap();

        // Pre-existing real copy - distribute must leave this one alone.
        write_skill(
            &home.join(".cursor/skills/find-bugs"),
            "find-bugs-cursor-copy",
        );

        // Claude Code links this one skill in, per-skill (not a whole-dir link).
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        symlink(
            "../../.agents/skills/find-bugs",
            home.join(".claude/skills/find-bugs"),
        )
        .unwrap();

        let root = home.join(".agents/skills");
        (home, root)
    }

    #[test]
    fn distribute_gives_every_reader_a_copy_and_removes_the_shared_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let (home, root) = home_with_shared_skill(tmp.path());

        distribute_from_shared(&store, &home, &root, "find-bugs").unwrap();

        assert!(fs::symlink_metadata(root.join("find-bugs")).is_err());

        for copy in [
            home.join(".codex/skills/find-bugs"),
            home.join(".config/opencode/skills/find-bugs"),
            home.join(".pi/agent/skills/find-bugs"),
            home.join(".grok/skills/find-bugs"),
            home.join(".claude/skills/find-bugs"),
        ] {
            assert!(
                !fs::symlink_metadata(&copy)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "{} should be a real copy",
                copy.display()
            );
            assert!(copy.join("extra.txt").is_file(), "{}", copy.display());
        }
        // Claude Code's symlink was replaced, not left as a broken link.
        assert!(fs::read_to_string(home.join(".claude/skills/find-bugs/extra.txt")).is_ok());

        // Pre-existing Cursor copy is untouched - still has its own content,
        // not the shared dir's.
        assert!(!home.join(".cursor/skills/find-bugs/extra.txt").exists());
        assert_eq!(
            fs::read_to_string(home.join(".cursor/skills/find-bugs/SKILL.md")).unwrap(),
            "---\nname: find-bugs-cursor-copy\ndescription: test\n---\nBody."
        );

        let events = store.list(10, Some("find-bugs")).unwrap();
        let event = events
            .iter()
            .find(|e| e.kind == "distribute_from_shared")
            .unwrap();
        assert_eq!(event.status, "done");
    }

    #[test]
    fn restore_after_distribute_brings_back_the_shared_dir_and_the_removed_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let (home, root) = home_with_shared_skill(tmp.path());

        distribute_from_shared(&store, &home, &root, "find-bugs").unwrap();
        let events = store.list(10, Some("find-bugs")).unwrap();
        let event = events
            .iter()
            .find(|e| e.kind == "distribute_from_shared")
            .unwrap();

        store.restore(&event.id, false).unwrap();

        assert!(root.join("find-bugs/extra.txt").is_file());
        for copy in [
            home.join(".codex/skills/find-bugs"),
            home.join(".config/opencode/skills/find-bugs"),
            home.join(".pi/agent/skills/find-bugs"),
            home.join(".grok/skills/find-bugs"),
        ] {
            assert!(fs::symlink_metadata(&copy).is_err(), "{}", copy.display());
        }
        let claude_link = home.join(".claude/skills/find-bugs");
        assert!(fs::symlink_metadata(&claude_link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_link(&claude_link).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );

        // The pre-existing Cursor copy was never touched by distribute or restore.
        assert_eq!(
            fs::read_to_string(home.join(".cursor/skills/find-bugs/SKILL.md")).unwrap(),
            "---\nname: find-bugs-cursor-copy\ndescription: test\n---\nBody."
        );
    }

    #[test]
    fn restore_refuses_when_a_copy_drifted_unless_forced() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let (home, root) = home_with_shared_skill(tmp.path());

        distribute_from_shared(&store, &home, &root, "find-bugs").unwrap();
        let events = store.list(10, Some("find-bugs")).unwrap();
        let event = events
            .iter()
            .find(|e| e.kind == "distribute_from_shared")
            .unwrap()
            .clone();

        fs::write(
            home.join(".codex/skills/find-bugs/extra.txt"),
            "edited by someone",
        )
        .unwrap();

        let err = store.restore(&event.id, false).unwrap_err();
        assert!(err.contains("changed since"), "{err}");
        assert!(root.join("find-bugs").symlink_metadata().is_err());

        store.restore(&event.id, true).unwrap();
        assert!(root.join("find-bugs/extra.txt").is_file());
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
