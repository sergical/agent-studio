// ============================================================================
// Skills Module - skill_independent_copy
// Replaces one healthy Universal-backed deployment link with an exact,
// independently owned Copy deployment at the same lexical path.
// ============================================================================

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::event_store::{
    allocate_id, fingerprint_path, EventDraft, EventRow, EventStatus, EventStore, InverseOp,
};
use super::skill_deployment::{deployment_id, SkillDestination};
use super::skill_dto::InstallScope;
use super::skill_fork_registry::{
    read_fork_registry, write_fork_registry, CopyDeploymentRecord, ForkRegistry,
    CURRENT_REGISTRY_VERSION,
};
use super::skill_fs::copy_dir_preserving_symlinks;

const INJECTED_CRASH_PREFIX: &str = "injected independent-copy crash";
const RESTORE_LINK_PREFIX: &str = ".skill-studio-restore-";

struct StagingDirectory {
    path: PathBuf,
}

impl StagingDirectory {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn persist(self) {
        std::mem::forget(self);
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug)]
struct IndependentCopyEventData {
    event_id: String,
    deployment_path: PathBuf,
    copy_deployment_id: String,
    expected_root: PathBuf,
    expected_real_root: Option<PathBuf>,
    root_device: Option<u64>,
    root_inode: Option<u64>,
    staged_fingerprint: Option<String>,
    staging: PathBuf,
    saved_link: PathBuf,
    original_link_target: Option<PathBuf>,
    copy_record: Option<CopyDeploymentRecord>,
    materialize_event_id: Option<String>,
}

/// Exact identity needed to detach one linked deployment without affecting
/// another deployment with the same skill name.
pub struct IndependentCopyRequest<'a> {
    pub home: &'a Path,
    pub skill: &'a str,
    pub link: &'a Path,
    pub expected_source: &'a Path,
    pub harness: &'a str,
    pub scope: InstallScope,
    pub project_path: Option<&'a str>,
    pub slot: &'a str,
    /// Convert a whole-root Universal link to per-skill links before copying.
    pub convert_whole_root: bool,
}

/// Replaces one verified per-skill link with a staged directory and records
/// exact Copy ownership. The original literal link target is retained for undo.
pub fn make_skill_independent_copy(
    store: &EventStore,
    request: IndependentCopyRequest<'_>,
) -> Result<String, String> {
    make_skill_independent_copy_with(
        store,
        request,
        &|from, to| fs::rename(from, to),
        &|home, registry| write_fork_registry(home, registry),
        &|_| Ok(()),
    )
}

fn make_skill_independent_copy_with(
    store: &EventStore,
    request: IndependentCopyRequest<'_>,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
    write_registry: &dyn Fn(&Path, &ForkRegistry) -> Result<(), String>,
    after_phase: &dyn Fn(&str) -> Result<(), String>,
) -> Result<String, String> {
    let parent = request
        .link
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", request.link.display()))?;
    if request.convert_whole_root {
        super::skill_materialize::validate_materialize_root(parent)?;
    } else {
        refuse_unless_current_link(&request)?;
        refuse_unless_real_per_skill_root(parent)?;
    }

    let scope_name = match request.scope {
        InstallScope::Global => "global",
        InstallScope::Project => "project",
    };
    let copy_id = deployment_id(
        request.skill,
        scope_name,
        SkillDestination::PerHarness,
        request.slot,
        request.project_path,
        request.link,
    );
    let id = allocate_id();
    let staging = parent.join(format!(".skill-studio-independent-{id}"));
    let saved_link = parent.join(format!(".skill-studio-linked-{id}"));
    let materialize_event_id = request.convert_whole_root.then(allocate_id);
    let mut payload = json!({
        "deployment_path": request.link,
        "copy_deployment_id": copy_id,
        "source": request.expected_source,
        "expected_root": parent,
        "staging": staging,
        "saved_link": saved_link,
        "convert_whole_root": request.convert_whole_root,
        "materialize_event_id": materialize_event_id,
    });
    store.record(
        &id,
        EventDraft {
            kind: "make_independent_copy".to_string(),
            skill: request.skill.to_string(),
            harness: Some(request.harness.to_string()),
            scope: Some(scope_name.to_string()),
            project_path: request.project_path.map(str::to_string),
            payload: payload.clone(),
            inverse: None,
            backup_dir: None,
            restorable: true,
        },
    )?;

    match run_independent_copy(
        store,
        &request,
        &id,
        &copy_id,
        parent,
        &staging,
        &saved_link,
        &mut payload,
        rename,
        write_registry,
        after_phase,
    ) {
        Ok(()) => Ok(id),
        Err(error) if is_injected_crash(&error) => Err(error),
        Err(error) => {
            let rollback = rollback_unstarted_whole_root(store, parent, &payload);
            let _ = store.finish(&id, EventStatus::Failed);
            match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!(
                    "Make independent copy failed ({error}) and whole-root conversion rollback failed: {rollback_error}"
                )),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_independent_copy(
    store: &EventStore,
    request: &IndependentCopyRequest<'_>,
    id: &str,
    copy_id: &str,
    parent: &Path,
    staging: &Path,
    saved_link: &Path,
    payload: &mut Value,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
    write_registry: &dyn Fn(&Path, &ForkRegistry) -> Result<(), String>,
    after_phase: &dyn Fn(&str) -> Result<(), String>,
) -> Result<(), String> {
    after_phase("intent")?;
    if request.convert_whole_root {
        let materialize_event_id = payload
            .get("materialize_event_id")
            .and_then(Value::as_str)
            .ok_or("Independent copy event has no reserved materialize event id")?;
        super::skill_materialize::explode_shared_dir_with_event_id_and_hook(
            store,
            parent,
            request.slot,
            materialize_event_id,
            after_phase,
        )?;
        after_phase("converted")?;
    }

    refuse_unless_current_link(request)?;
    let parent_metadata = refuse_unless_real_per_skill_root(parent)?;
    let resolved_parent = fs::canonicalize(parent).map_err(|error| {
        format!(
            "Independent copy refused: failed to resolve {}: {error}",
            parent.display()
        )
    })?;
    let source = fs::canonicalize(request.link).map_err(|error| {
        format!("Independent copy refused: link is broken or unreadable: {error}")
    })?;
    if resolved_parent.starts_with(&source) {
        return Err("Independent copy refused: the destination is inside its source".to_string());
    }
    let literal_target = fs::read_link(request.link).map_err(|error| {
        format!(
            "Independent copy refused: failed to read {}: {error}",
            request.link.display()
        )
    })?;
    let source_fingerprint = fingerprint_path(&source);
    payload["source"] = json!(source);
    payload["expected_real_root"] = json!(resolved_parent);
    payload["root_device"] = json!(parent_metadata.dev());
    payload["root_inode"] = json!(parent_metadata.ino());
    payload["original_link_target"] = json!(literal_target);
    store.patch_event_payload(id, payload)?;

    let staging_guard = StagingDirectory::new(staging.to_path_buf());
    copy_dir_preserving_symlinks(&source, staging)?;
    let staged_fingerprint = fingerprint_path(staging);
    if staged_fingerprint != source_fingerprint {
        return Err(
            "Independent copy refused: the staged copy does not match its source".to_string(),
        );
    }
    let content_hash = super::skill_discovery::live_skill_content_hash(staging)?;
    let record = CopyDeploymentRecord {
        deployment_id: copy_id.to_string(),
        name: request.skill.to_string(),
        path: request.link.to_path_buf(),
        scope: request.scope.clone(),
        destination: SkillDestination::PerHarness,
        slot: request.slot.to_string(),
        project_path: request.project_path.map(str::to_string),
        content_hash,
        disabled: false,
    };
    payload["staged_fingerprint"] = json!(staged_fingerprint);
    payload["copy_record"] = serde_json::to_value(&record)
        .map_err(|error| format!("Failed to serialize independent copy record: {error}"))?;
    store.patch_event_payload(id, payload)?;
    let inverse = InverseOp::RecreateSymlink {
        link: request.link.to_path_buf(),
        target: literal_target.clone(),
        pre_fingerprint: fingerprint_path(request.link),
        post_fingerprint: Some(staged_fingerprint.clone()),
    };
    store.patch_event_inverse(
        id,
        &serde_json::to_value(&inverse)
            .map_err(|error| format!("Failed to serialize independent copy inverse: {error}"))?,
    )?;
    if let Err(error) = after_phase("staged") {
        staging_guard.persist();
        return Err(error);
    }

    let current_literal = fs::read_link(request.link).map_err(|error| {
        format!("Independent copy refused: link changed while copying: {error}")
    })?;
    let current_source = fs::canonicalize(request.link).map_err(|error| {
        format!("Independent copy refused: link changed while copying: {error}")
    })?;
    if current_literal != literal_target
        || current_source != source
        || fingerprint_path(&source) != source_fingerprint
    {
        return Err(
            "Independent copy refused: the link or its source changed while copying".to_string(),
        );
    }

    let previous_registry = read_fork_registry(request.home)?;
    let mut registry = previous_registry.clone();
    registry.copies.insert(copy_id.to_string(), record);
    registry.version = CURRENT_REGISTRY_VERSION;

    let mutation = (|| -> Result<(), String> {
        rename(request.link, saved_link).map_err(|error| {
            format!(
                "Failed to preserve linked deployment {}: {error}",
                request.link.display()
            )
        })?;
        if let Err(error) = rename(staging, request.link) {
            let rollback = rename(saved_link, request.link);
            return match rollback {
                Ok(()) => Err(format!("Failed to move independent copy into place: {error}")),
                Err(rollback_error) => Err(format!(
                    "Failed to move independent copy into place ({error}) and failed to restore the link: {rollback_error}"
                )),
            };
        }
        after_phase("replaced")?;
        if let Err(error) = write_registry(request.home, &registry) {
            let moved_copy = rename(request.link, staging);
            let restored_link = rename(saved_link, request.link);
            return match (moved_copy, restored_link) {
                (Ok(()), Ok(())) => Err(format!(
                    "Failed to record Copy ownership; restored the original link: {error}"
                )),
                (copy_result, link_result) => Err(format!(
                    "Failed to record Copy ownership ({error}); rollback failed (copy: {:?}, link: {:?})",
                    copy_result.err(),
                    link_result.err()
                )),
            };
        }
        after_phase("registry")?;
        Ok(())
    })();

    match mutation {
        Ok(()) => {
            let finalize = store.finish(id, EventStatus::Done);
            if let Err(error) = finalize {
                let filesystem_rollback =
                    rename(request.link, staging).and_then(|_| rename(saved_link, request.link));
                let registry_rollback = write_registry(request.home, &previous_registry);
                let _ = store.finish(id, EventStatus::Failed);
                return match (filesystem_rollback, registry_rollback) {
                    (Ok(()), Ok(())) => Err(format!(
                        "Failed to finalize independent copy; restored the original link: {error}"
                    )),
                    (filesystem_result, registry_result) => Err(format!(
                        "Failed to finalize independent copy ({error}); rollback failed (filesystem: {:?}, registry: {:?})",
                        filesystem_result.err(),
                        registry_result.err()
                    )),
                };
            }
            let _ = fs::remove_file(saved_link);
            drop(staging_guard);
            Ok(())
        }
        Err(error) if is_injected_crash(&error) => {
            staging_guard.persist();
            Err(error)
        }
        Err(error) => Err(error),
    }
}

fn refuse_unless_current_link(request: &IndependentCopyRequest<'_>) -> Result<(), String> {
    let metadata = fs::symlink_metadata(request.link).map_err(|error| {
        format!(
            "Independent copy refused: failed to stat {}: {error}",
            request.link.display()
        )
    })?;
    if !metadata.file_type().is_symlink() {
        return Err(format!(
            "Independent copy refused: {} is not a symlink",
            request.link.display()
        ));
    }
    let source = fs::canonicalize(request.link).map_err(|error| {
        format!("Independent copy refused: link is broken or unreadable: {error}")
    })?;
    let expected_source = fs::canonicalize(request.expected_source).map_err(|error| {
        format!(
            "Independent copy refused: selected Universal source {} cannot be resolved: {error}",
            request.expected_source.display()
        )
    })?;
    if source != expected_source {
        return Err("Independent copy refused: the link was repointed".to_string());
    }
    if !source.is_dir() {
        return Err("Independent copy refused: the link target is not a directory".to_string());
    }
    Ok(())
}

fn refuse_unless_real_per_skill_root(parent: &Path) -> Result<std::fs::Metadata, String> {
    let parent_metadata = fs::symlink_metadata(parent).map_err(|error| {
        format!(
            "Independent copy refused: failed to identify {}: {error}",
            parent.display()
        )
    })?;
    if !parent_metadata.is_dir() || parent_metadata.file_type().is_symlink() {
        return Err(
            "Independent copy refused: the per-skill root is not a real directory".to_string(),
        );
    }
    Ok(parent_metadata)
}

fn is_injected_crash(error: &str) -> bool {
    error.starts_with(INJECTED_CRASH_PREFIX)
}

fn rollback_unstarted_whole_root(
    store: &EventStore,
    parent: &Path,
    payload: &Value,
) -> Result<(), String> {
    let Some(event_id) = payload.get("materialize_event_id").and_then(Value::as_str) else {
        return Ok(());
    };
    if fs::symlink_metadata(parent).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Ok(());
    }
    let materialize_event = store
        .get(event_id)?
        .ok_or_else(|| format!("Connected materialize event {event_id} was not recorded"))?;
    if materialize_event.status != "done" {
        return Err(
            "Connected materialize did not complete; preserved the current root unchanged"
                .to_string(),
        );
    }
    store.restore(event_id, false)?;
    store.unregister_materialized_root(parent)
}

fn event_path(event: &EventRow, key: &str) -> Result<PathBuf, String> {
    event
        .payload
        .get(key)
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| format!("Independent copy event has no {key}"))
}

fn optional_event_path(event: &EventRow, key: &str) -> Option<PathBuf> {
    event
        .payload
        .get(key)
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

fn optional_event_u64(event: &EventRow, key: &str) -> Option<u64> {
    event.payload.get(key).and_then(Value::as_u64)
}

fn independent_copy_event_data(event: &EventRow) -> Result<IndependentCopyEventData, String> {
    let copy_record = event
        .payload
        .get("copy_record")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("Independent copy event has an invalid copy_record: {error}"))?;
    Ok(IndependentCopyEventData {
        event_id: event.id.clone(),
        deployment_path: event_path(event, "deployment_path")?,
        copy_deployment_id: event
            .payload
            .get("copy_deployment_id")
            .and_then(Value::as_str)
            .ok_or("Independent copy event has no Copy deployment identity")?
            .to_string(),
        expected_root: event_path(event, "expected_root")?,
        expected_real_root: optional_event_path(event, "expected_real_root"),
        root_device: optional_event_u64(event, "root_device"),
        root_inode: optional_event_u64(event, "root_inode"),
        staged_fingerprint: event
            .payload
            .get("staged_fingerprint")
            .and_then(Value::as_str)
            .map(str::to_string),
        staging: optional_event_path(event, "staging").unwrap_or_else(|| {
            event_path(event, "expected_root")
                .unwrap_or_default()
                .join(format!(".skill-studio-independent-{}", event.id))
        }),
        saved_link: optional_event_path(event, "saved_link").unwrap_or_else(|| {
            event_path(event, "expected_root")
                .unwrap_or_default()
                .join(format!(".skill-studio-linked-{}", event.id))
        }),
        original_link_target: optional_event_path(event, "original_link_target"),
        copy_record,
        materialize_event_id: event
            .payload
            .get("materialize_event_id")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// Verifies that an independent-copy inverse still addresses the exact real
/// per-skill root in which the copy was created. This must run before any
/// registry or filesystem mutation during undo.
pub fn validate_independent_copy_restore(event: &EventRow) -> Result<(), String> {
    let data = independent_copy_event_data(event)?;
    let expected_real_root = data
        .expected_real_root
        .ok_or("Independent copy restore refused: the per-skill root was never recorded")?;
    let root_device = data
        .root_device
        .ok_or("Independent copy restore refused: the per-skill root was never recorded")?;
    let root_inode = data
        .root_inode
        .ok_or("Independent copy restore refused: the per-skill root was never recorded")?;
    if data.deployment_path.parent() != Some(data.expected_root.as_path()) {
        return Err("Independent copy restore refused: the deployment is outside its recorded per-skill root".to_string());
    }
    let metadata = fs::symlink_metadata(&data.expected_root).map_err(|error| {
        format!(
            "Independent copy restore refused: cannot identify the recorded per-skill root {}: {error}",
            data.expected_root.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "Independent copy restore refused: {} is no longer the real per-skill root; undo independent copies before restoring the whole-folder link",
            data.expected_root.display()
        ));
    }
    let real_root = fs::canonicalize(&data.expected_root).map_err(|error| {
        format!(
            "Independent copy restore refused: cannot resolve {}: {error}",
            data.expected_root.display()
        )
    })?;
    if real_root != expected_real_root
        || metadata.dev() != root_device
        || metadata.ino() != root_inode
    {
        return Err(
            "Independent copy restore refused: the per-skill root identity changed".to_string(),
        );
    }
    Ok(())
}

fn remove_matching_copy_ownership(
    home: &Path,
    data: &IndependentCopyEventData,
) -> Result<Option<ForkRegistry>, String> {
    let mut registry = read_fork_registry(home)?;
    match registry.copies.get(&data.copy_deployment_id) {
        None => Ok(None),
        Some(existing) if ownership_matches(existing, data) => {
            let previous = registry.clone();
            registry.copies.remove(&data.copy_deployment_id);
            write_fork_registry(home, &registry)?;
            Ok(Some(previous))
        }
        Some(_) => Err(
            "Interrupted independent copy has conflicting Copy ownership; preserved the filesystem unchanged"
                .to_string(),
        ),
    }
}

fn ownership_matches(existing: &CopyDeploymentRecord, data: &IndependentCopyEventData) -> bool {
    match &data.copy_record {
        Some(record) => existing == record,
        None => {
            existing.deployment_id == data.copy_deployment_id
                && existing.path == data.deployment_path
        }
    }
}

fn remove_recorded_staging(data: &IndependentCopyEventData) -> Result<(), String> {
    remove_recorded_staging_with(data, &|path| fs::remove_dir_all(path))
}

fn remove_recorded_staging_with(
    data: &IndependentCopyEventData,
    remove_dir_all: &dyn Fn(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(&data.staging) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Interrupted independent copy could not identify its staging path {}: {error}",
                data.staging.display()
            ));
        }
    };
    let expected_staging = data
        .expected_root
        .join(format!(".skill-studio-independent-{}", data.event_id));
    if data.staging != expected_staging || data.staging == data.deployment_path {
        return Err(
            "Interrupted independent copy staging path is outside its recorded root; preserved it unchanged"
                .to_string(),
        );
    }
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(
            "Interrupted independent copy found an unexpected staging path type; preserved it unchanged"
                .to_string(),
        );
    }
    let expected = data.staged_fingerprint.as_ref().ok_or(
        "Interrupted independent copy found staging content without a recorded fingerprint; preserved it unchanged",
    )?;
    if fingerprint_path(&data.staging) != *expected {
        return Err(
            "Interrupted independent copy staging fingerprint changed; preserved it unchanged"
                .to_string(),
        );
    }
    remove_dir_all(&data.staging).map_err(|error| {
        format!(
            "Interrupted independent copy failed to remove verified staging directory {}: {error}",
            data.staging.display()
        )
    })
}

fn original_link_still_present(data: &IndependentCopyEventData) -> bool {
    fs::symlink_metadata(&data.deployment_path)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
        && data.original_link_target.as_ref().is_some_and(|target| {
            fs::read_link(&data.deployment_path).ok().as_ref() == Some(target)
        })
}

fn path_is_absent(path: &Path) -> bool {
    fs::symlink_metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

fn recorded_restore_link(
    event: &EventRow,
    data: &IndependentCopyEventData,
) -> Result<PathBuf, String> {
    let staged_link = event_path(event, "staged_restore_link")?;
    let expected_staged_link = data
        .expected_root
        .join(format!("{RESTORE_LINK_PREFIX}{}", event.id));
    if staged_link != expected_staged_link
        || optional_event_path(event, "deployment_path").as_ref() != Some(&data.deployment_path)
        || optional_event_path(event, "expected_root").as_ref() != Some(&data.expected_root)
        || optional_event_path(event, "expected_real_root") != data.expected_real_root
        || optional_event_u64(event, "root_device") != data.root_device
        || optional_event_u64(event, "root_inode") != data.root_inode
        || optional_event_path(event, "original_link_target") != data.original_link_target
    {
        return Err(
            "Interrupted independent copy restore has inconsistent recorded link data; preserved the filesystem unchanged"
                .to_string(),
        );
    }
    Ok(staged_link)
}

fn staged_link_matches(staged_link: &Path, target: &Path) -> bool {
    fs::symlink_metadata(staged_link).is_ok_and(|metadata| metadata.file_type().is_symlink())
        && fs::read_link(staged_link).ok().as_deref() == Some(target)
}

fn prepare_recorded_restore_link(staged_link: &Path, target: &Path) -> Result<(), String> {
    match fs::symlink_metadata(staged_link) {
        Ok(_) if staged_link_matches(staged_link, target) => Ok(()),
        Ok(_) => Err(
            "Independent copy restore found unknown content at its recorded staging path; preserved it unchanged"
                .to_string(),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::os::unix::fs::symlink(target, staged_link).map_err(|error| {
                format!(
                    "Failed to stage the independent-copy restore link {}: {error}",
                    staged_link.display()
                )
            })
        }
        Err(error) => Err(format!(
            "Failed to identify the independent-copy restore staging path {}: {error}",
            staged_link.display()
        )),
    }
}

fn publish_recorded_restore_link(staged_link: &Path, destination: &Path) -> Result<(), String> {
    fs::hard_link(staged_link, destination).map_err(|error| {
        format!(
            "Failed to publish the independent-copy restore link at {} without overwriting existing content: {error}",
            destination.display()
        )
    })?;
    let _ = fs::remove_file(staged_link);
    Ok(())
}

fn restore_copy_directory_with_recorded_link(
    data: &IndependentCopyEventData,
    staged_link: &Path,
    after_phase: &dyn Fn(&str) -> Result<(), String>,
) -> Result<(), String> {
    let target = data.original_link_target.as_deref().ok_or(
        "Independent copy restore refused: the original literal link target was never recorded",
    )?;
    prepare_recorded_restore_link(staged_link, target)?;
    fs::remove_dir_all(&data.deployment_path).map_err(|error| {
        format!(
            "Failed to remove the independent copy {}: {error}",
            data.deployment_path.display()
        )
    })?;
    after_phase("copy_removed")?;
    publish_recorded_restore_link(staged_link, &data.deployment_path)
}

fn restore_absent_copy_path(
    data: &IndependentCopyEventData,
    staged_link: &Path,
) -> Result<(), String> {
    let target = data.original_link_target.as_deref().ok_or(
        "Independent copy restore refused: the original literal link target was never recorded",
    )?;
    match fs::symlink_metadata(staged_link) {
        Ok(_) if staged_link_matches(staged_link, target) => {
            publish_recorded_restore_link(staged_link, &data.deployment_path)
        }
        Ok(_) => Err(
            "Interrupted independent copy restore found unknown staging content; preserved it unchanged"
                .to_string(),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::os::unix::fs::symlink(target, &data.deployment_path).map_err(|error| {
                format!(
                    "Failed to recreate the independent-copy link at {} without overwriting existing content: {error}",
                    data.deployment_path.display()
                )
            })
        }
        Err(error) => Err(format!(
            "Failed to identify the independent-copy restore staging path {}: {error}",
            staged_link.display()
        )),
    }
}

/// Records non-restorable undo intent before Copy ownership is removed, then
/// puts the original link back. Startup can resume this if the process exits
/// mid-undo.
pub fn restore_independent_copy(
    store: &EventStore,
    home: &Path,
    target: &EventRow,
) -> Result<String, String> {
    restore_independent_copy_with(store, home, target, &|_| Ok(()))
}

fn restore_independent_copy_with(
    store: &EventStore,
    home: &Path,
    target: &EventRow,
    after_phase: &dyn Fn(&str) -> Result<(), String>,
) -> Result<String, String> {
    if target.kind != "make_independent_copy" {
        return Err(
            "Independent copy restore refused: event is not a make independent copy".to_string(),
        );
    }
    if target.reverted_by.is_some() {
        return Err(format!("Event {} was already restored", target.id));
    }
    validate_independent_copy_restore(target)?;
    let data = independent_copy_event_data(target)?;
    let inverse_value = target
        .inverse
        .clone()
        .ok_or_else(|| format!("Event {} has no inverse and cannot be restored", target.id))?;
    let inverse: InverseOp = serde_json::from_value(inverse_value)
        .map_err(|error| format!("Failed to parse inverse for {}: {error}", target.id))?;
    let dest = data.deployment_path.clone();
    let current_fp = fingerprint_path(&dest);
    if let InverseOp::RecreateSymlink {
        post_fingerprint: Some(expected),
        ..
    } = &inverse
    {
        if current_fp != *expected {
            return Err(format!(
                "{} has changed since the event that would be undone; use force to restore anyway (the current content will be backed up first)",
                dest.display()
            ));
        }
    }

    let restore_id = allocate_id();
    let staged_restore_link = data
        .expected_root
        .join(format!("{RESTORE_LINK_PREFIX}{restore_id}"));
    store.backup_paths(&restore_id, std::slice::from_ref(&dest))?;
    let restore_inverse = InverseOp::RestoreBackup {
        path: dest.clone(),
        pre_fingerprint: current_fp,
        post_fingerprint: None,
    };
    store.record(
        &restore_id,
        EventDraft {
            kind: "restore".to_string(),
            skill: target.skill.clone(),
            harness: target.harness.clone(),
            scope: target.scope.clone(),
            project_path: target.project_path.clone(),
            payload: json!({
                "target_event": target.id,
                "copy_deployment_id": data.copy_deployment_id,
                "copy_record": data.copy_record,
                "deployment_path": data.deployment_path,
                "expected_root": data.expected_root,
                "expected_real_root": data.expected_real_root,
                "root_device": data.root_device,
                "root_inode": data.root_inode,
                "original_link_target": data.original_link_target,
                "staged_restore_link": staged_restore_link,
            }),
            inverse: Some(serde_json::to_value(&restore_inverse).map_err(|error| {
                format!("Failed to serialize independent copy restore inverse: {error}")
            })?),
            backup_dir: Some(format!("backups/{restore_id}")),
            restorable: false,
        },
    )?;
    store.claim_event_restore(&target.id, &restore_id)?;
    if let Err(error) = after_phase("recorded") {
        return Err(crash_or_fail_restore(
            store,
            home,
            target,
            &restore_id,
            None,
            error,
        ));
    }

    let previous_registry = match remove_matching_copy_ownership(home, &data) {
        Ok(previous) => previous,
        Err(error) => {
            let _ = store.unclaim_event_restore(&target.id, &restore_id);
            let _ = store.finish(&restore_id, EventStatus::Failed);
            return Err(error);
        }
    };
    if let Err(error) = after_phase("ownership_removed") {
        return Err(crash_or_fail_restore(
            store,
            home,
            target,
            &restore_id,
            previous_registry.as_ref(),
            error,
        ));
    }

    match restore_copy_directory_with_recorded_link(&data, &staged_restore_link, after_phase) {
        Ok(()) => {
            let post_fp = fingerprint_path(&dest);
            store.patch_inverse_post_fingerprint(&restore_id, &post_fp)?;
            store.finish(&restore_id, EventStatus::Done)?;
            Ok(restore_id)
        }
        Err(error) => Err(error),
    }
}

fn crash_or_fail_restore(
    store: &EventStore,
    home: &Path,
    target: &EventRow,
    restore_id: &str,
    previous_registry: Option<&ForkRegistry>,
    error: String,
) -> String {
    if is_injected_crash(&error) {
        return error;
    }
    if let Some(registry) = previous_registry {
        let _ = write_fork_registry(home, registry);
    }
    let _ = store.unclaim_event_restore(&target.id, restore_id);
    let _ = store.finish(restore_id, EventStatus::Failed);
    error
}

/// Reconciles `make_independent_copy` rows interrupted by a process exit.
/// A completed replacement becomes owned; an untouched original link is kept.
/// Any edited or ambiguous path is preserved and left interrupted.
pub fn reconcile_interrupted_independent_copy(
    store: &EventStore,
    home: &Path,
    event: &EventRow,
) -> Result<(), String> {
    if event.kind != "make_independent_copy" || event.status != "interrupted" {
        return Ok(());
    }
    let data = independent_copy_event_data(event)?;
    let copy_started = data.copy_record.is_some() && data.original_link_target.is_some();
    if !copy_started {
        remove_recorded_staging(&data)?;
    }
    if let Some(materialize_event_id) = event
        .payload
        .get("materialize_event_id")
        .and_then(Value::as_str)
    {
        if let Some(materialize_event) = store.get(materialize_event_id)? {
            super::skill_materialize::reconcile_connected_materialize_event(
                store,
                &materialize_event,
            )?;
        }
    }
    if !copy_started {
        return fail_unstarted_copy(store, home, event, &data);
    }
    validate_independent_copy_restore(event)?;

    if original_link_still_present(&data) {
        return fail_unstarted_copy(store, home, event, &data);
    }
    if fs::symlink_metadata(&data.deployment_path)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(
            "Interrupted independent copy found a different link; preserved it unchanged"
                .to_string(),
        );
    }
    let saved_original_link = fs::symlink_metadata(&data.saved_link)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
        && data
            .original_link_target
            .as_ref()
            .is_some_and(|target| fs::read_link(&data.saved_link).ok().as_ref() == Some(target));
    if fs::symlink_metadata(&data.deployment_path).is_ok_and(|metadata| metadata.is_dir())
        && saved_original_link
        && path_is_absent(&data.staging)
    {
        return claim_replaced_copy(store, home, event, &data);
    }
    if fs::symlink_metadata(&data.deployment_path).is_err() && saved_original_link {
        remove_recorded_staging(&data)?;
        fs::rename(&data.saved_link, &data.deployment_path).map_err(|error| {
            format!("Failed to restore the interrupted independent-copy link: {error}")
        })?;
        return fail_unstarted_copy(store, home, event, &data);
    }
    Err("Interrupted independent copy is ambiguous; preserved the filesystem unchanged".to_string())
}

fn fail_unstarted_copy(
    store: &EventStore,
    home: &Path,
    event: &EventRow,
    data: &IndependentCopyEventData,
) -> Result<(), String> {
    remove_recorded_staging(data)?;
    remove_matching_copy_ownership(home, data)?;
    if let Some(materialize_event_id) = &data.materialize_event_id {
        if fs::symlink_metadata(&data.expected_root)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        {
            store.restore(materialize_event_id, false)?;
            store.unregister_materialized_root(&data.expected_root)?;
        }
    }
    store.finish(&event.id, EventStatus::Failed)?;
    Ok(())
}

fn claim_replaced_copy(
    store: &EventStore,
    home: &Path,
    event: &EventRow,
    data: &IndependentCopyEventData,
) -> Result<(), String> {
    remove_recorded_staging(data)?;
    let Some(copy_record) = data.copy_record.clone() else {
        return Err(
            "Interrupted independent copy is ambiguous; preserved the filesystem unchanged"
                .to_string(),
        );
    };
    let mut record = copy_record.clone();
    record.content_hash = super::skill_discovery::live_skill_content_hash(&data.deployment_path)?;
    let mut registry = read_fork_registry(home)?;
    if registry
        .copies
        .get(&data.copy_deployment_id)
        .is_some_and(|existing| existing != &record && existing != &copy_record)
    {
        return Err("Interrupted independent copy has conflicting Copy ownership; preserved the filesystem unchanged".to_string());
    }
    registry
        .copies
        .insert(data.copy_deployment_id.clone(), record);
    write_fork_registry(home, &registry)?;
    if fs::symlink_metadata(&data.saved_link)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
        && data
            .original_link_target
            .as_ref()
            .is_some_and(|target| fs::read_link(&data.saved_link).ok().as_ref() == Some(target))
    {
        let _ = fs::remove_file(&data.saved_link);
    }
    store.finish(&event.id, EventStatus::Done)?;
    Ok(())
}

/// Resumes an interrupted undo of `make_independent_copy`. Ownership is
/// never dropped unless a restore row already recorded that intent.
pub fn reconcile_interrupted_independent_copy_restore(
    store: &EventStore,
    home: &Path,
    event: &EventRow,
) -> Result<(), String> {
    if event.kind != "restore" || event.status != "interrupted" {
        return Ok(());
    }
    let Some(target_id) = event.payload.get("target_event").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(target) = store.get(target_id)? else {
        return Ok(());
    };
    if target.kind != "make_independent_copy" {
        return Ok(());
    }
    if target.reverted_by.as_deref() != Some(&event.id) {
        store.claim_event_restore(&target.id, &event.id)?;
    }
    validate_independent_copy_restore(&target)?;
    let data = independent_copy_event_data(&target)?;
    let staged_restore_link = recorded_restore_link(event, &data)?;
    if original_link_still_present(&data) {
        remove_matching_copy_ownership(home, &data)?;
        if data
            .original_link_target
            .as_deref()
            .is_some_and(|target| staged_link_matches(&staged_restore_link, target))
        {
            let _ = fs::remove_file(&staged_restore_link);
        }
        store.finish(&event.id, EventStatus::Done)?;
        return Ok(());
    }
    if fs::symlink_metadata(&data.deployment_path).is_ok_and(|metadata| metadata.is_dir()) {
        let current_fp = fingerprint_path(&data.deployment_path);
        let expected = target
            .inverse
            .as_ref()
            .and_then(|value| value.get("post_fingerprint"))
            .and_then(Value::as_str);
        if expected.is_some_and(|expected| expected != current_fp) {
            return Err(format!(
                "{} has changed since the event that would be undone; preserved the filesystem unchanged",
                data.deployment_path.display()
            ));
        }
        remove_matching_copy_ownership(home, &data)?;
        restore_copy_directory_with_recorded_link(&data, &staged_restore_link, &|_| Ok(()))?;
        let post_fp = fingerprint_path(&data.deployment_path);
        store.patch_inverse_post_fingerprint(&event.id, &post_fp)?;
        store.finish(&event.id, EventStatus::Done)?;
        return Ok(());
    }
    match fs::symlink_metadata(&data.deployment_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            remove_matching_copy_ownership(home, &data)?;
            restore_absent_copy_path(&data, &staged_restore_link)?;
            let post_fp = fingerprint_path(&data.deployment_path);
            store.patch_inverse_post_fingerprint(&event.id, &post_fp)?;
            store.finish(&event.id, EventStatus::Done)?;
            return Ok(());
        }
        _ => {}
    }
    Err(
        "Interrupted independent copy restore is ambiguous; preserved the filesystem unchanged"
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::os::unix::fs::symlink;

    fn write_skill(path: &Path, body: &str) {
        fs::create_dir_all(path.join("assets")).unwrap();
        fs::write(
            path.join("SKILL.md"),
            format!("---\nname: find-bugs\ndescription: test\n---\n{body}"),
        )
        .unwrap();
        fs::write(path.join("assets/data.txt"), "same bytes").unwrap();
        symlink("assets/data.txt", path.join("data-link.txt")).unwrap();
    }

    fn request<'a>(home: &'a Path, link: &'a Path, source: &'a Path) -> IndependentCopyRequest<'a> {
        IndependentCopyRequest {
            home,
            skill: "find-bugs",
            link,
            expected_source: source,
            harness: "Claude Code",
            scope: InstallScope::Global,
            project_path: None,
            slot: "claude-code",
            convert_whole_root: false,
        }
    }

    fn setup() -> (tempfile::TempDir, EventStore, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let source = home.join(".agents/skills/find-bugs");
        let link = home.join(".claude/skills/find-bugs");
        write_skill(&source, "Body");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink("../../.agents/skills/find-bugs", &link).unwrap();
        let store = EventStore::open(&temp.path().join("app-data")).unwrap();
        (temp, store, source, link)
    }

    fn setup_whole_root() -> (tempfile::TempDir, EventStore, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let shared = home.join(".agents/skills");
        write_skill(&shared.join("find-bugs"), "Body");
        write_skill(&shared.join("write-docs"), "Sibling");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let root = home.join(".claude/skills");
        symlink("../.agents/skills", &root).unwrap();
        let store = EventStore::open(&temp.path().join("app-data")).unwrap();
        (temp, store, home, shared, root)
    }

    fn mark_pending(store: &EventStore, event_id: &str) {
        store
            .conn
            .execute(
                "UPDATE events SET status = 'pending' WHERE id = ?1",
                rusqlite::params![event_id],
            )
            .unwrap();
    }

    fn crash_at(phase: &'static str) -> impl Fn(&str) -> Result<(), String> {
        move |current| {
            if current == phase {
                Err(format!("{INJECTED_CRASH_PREFIX}: {phase}"))
            } else {
                Ok(())
            }
        }
    }

    fn recover_make(store: &EventStore, home: &Path, event_id: &str) -> Result<(), String> {
        let status = store.get(event_id).unwrap().unwrap().status;
        if status == "pending" {
            store.reconcile_at_startup().unwrap();
        } else if status != "interrupted" {
            return Ok(());
        }
        let interrupted = store.interrupted_independent_copy_events().unwrap();
        let event = interrupted
            .into_iter()
            .find(|row| row.id == event_id)
            .unwrap();
        reconcile_interrupted_independent_copy(store, home, &event)
    }

    fn recover_restore(store: &EventStore, home: &Path, event_id: &str) -> Result<(), String> {
        let status = store.get(event_id).unwrap().unwrap().status;
        if status == "pending" {
            store.reconcile_at_startup().unwrap();
        } else if status != "interrupted" {
            return Ok(());
        }
        let interrupted = store.interrupted_independent_copy_events().unwrap();
        let event = interrupted
            .into_iter()
            .find(|row| row.id == event_id)
            .unwrap();
        reconcile_interrupted_independent_copy_restore(store, home, &event)
    }

    fn no_unknown_staging(root: &Path) {
        assert!(fs::read_dir(root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".skill-studio-independent-")));
    }

    #[test]
    fn linked_skill_becomes_an_identical_owned_copy_and_other_scope_is_untouched() {
        let (temp, store, source, link) = setup();
        let project_copy = temp.path().join("project/.claude/skills/find-bugs");
        write_skill(&project_copy, "Project body");

        make_skill_independent_copy(&store, request(&temp.path().join("home"), &link, &source))
            .unwrap();

        assert!(link.is_dir());
        assert!(!fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fingerprint_path(&link), fingerprint_path(&source));
        assert_eq!(
            fs::read_link(link.join("data-link.txt")).unwrap(),
            PathBuf::from("assets/data.txt")
        );
        assert!(fs::read_to_string(project_copy.join("SKILL.md"))
            .unwrap()
            .contains("Project body"));
        let registry = read_fork_registry(&temp.path().join("home")).unwrap();
        let record = registry.copies.values().next().unwrap();
        assert_eq!(record.path, link);
        assert_eq!(record.destination, SkillDestination::PerHarness);
        assert_eq!(record.slot, "claude-code");
        assert!(!record.disabled);
    }

    #[test]
    fn whole_root_can_be_exploded_then_only_one_skill_becomes_a_copy() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let shared = home.join(".agents/skills");
        write_skill(&shared.join("find-bugs"), "Body");
        write_skill(&shared.join("write-docs"), "Sibling");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let root = home.join(".claude/skills");
        symlink("../.agents/skills", &root).unwrap();
        let store = EventStore::open(&temp.path().join("app-data")).unwrap();

        super::super::skill_materialize::explode_shared_dir(&store, &root, "claude-code").unwrap();
        make_skill_independent_copy(
            &store,
            request(&home, &root.join("find-bugs"), &shared.join("find-bugs")),
        )
        .unwrap();

        assert!(!fs::symlink_metadata(root.join("find-bugs"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(fs::symlink_metadata(root.join("write-docs"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn failed_copy_after_whole_root_conversion_restores_the_exact_root_link() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let shared = home.join(".agents/skills");
        write_skill(&shared.join("find-bugs"), "Body");
        write_skill(&shared.join("write-docs"), "Sibling");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let root = home.join(".claude/skills");
        let root_target = PathBuf::from("../.agents/skills");
        symlink(&root_target, &root).unwrap();
        let store = EventStore::open(&temp.path().join("app-data")).unwrap();
        let link = root.join("find-bugs");
        let source = shared.join("find-bugs");
        let mut copy_request = request(&home, &link, &source);
        copy_request.convert_whole_root = true;

        let error = make_skill_independent_copy_with(
            &store,
            copy_request,
            &|from, to| fs::rename(from, to),
            &|_, _| Err("injected registry failure".to_string()),
            &|_| Ok(()),
        )
        .unwrap_err();
        assert!(error.contains("restored the original link"));

        assert_eq!(fs::read_link(&root).unwrap(), root_target);
        assert!(store.materialized_root(&root).unwrap().is_none());
        assert!(read_fork_registry(&home).unwrap().copies.is_empty());
        assert!(fs::read_dir(&shared).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".skill-studio-")));
    }

    #[test]
    fn broken_repointed_and_non_link_targets_are_refused() {
        let (temp, store, source, link) = setup();
        fs::remove_file(&link).unwrap();
        symlink("missing", &link).unwrap();
        assert!(make_skill_independent_copy(
            &store,
            request(&temp.path().join("home"), &link, &source)
        )
        .unwrap_err()
        .contains("broken"));

        fs::remove_file(&link).unwrap();
        let other = temp.path().join("other");
        write_skill(&other, "Other");
        symlink(&other, &link).unwrap();
        assert!(make_skill_independent_copy(
            &store,
            request(&temp.path().join("home"), &link, &source)
        )
        .unwrap_err()
        .contains("repointed"));

        fs::remove_file(&link).unwrap();
        write_skill(&link, "Local");
        assert!(make_skill_independent_copy(
            &store,
            request(&temp.path().join("home"), &link, &source)
        )
        .unwrap_err()
        .contains("not a symlink"));
    }

    #[test]
    fn registry_write_failure_restores_the_link() {
        let (temp, store, source, link) = setup();
        let error = make_skill_independent_copy_with(
            &store,
            request(&temp.path().join("home"), &link, &source),
            &|from, to| fs::rename(from, to),
            &|_, _| Err("injected registry failure".to_string()),
            &|_| Ok(()),
        )
        .unwrap_err();

        assert!(error.contains("restored the original link"));
        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );
        assert!(read_fork_registry(&temp.path().join("home"))
            .unwrap()
            .copies
            .is_empty());
    }

    #[test]
    fn replacement_rename_failure_restores_the_link() {
        let (temp, store, source, link) = setup();
        let calls = Cell::new(0);
        let error = make_skill_independent_copy_with(
            &store,
            request(&temp.path().join("home"), &link, &source),
            &|from, to| {
                let call = calls.get();
                calls.set(call + 1);
                if call == 1 {
                    Err(std::io::Error::other("injected rename failure"))
                } else {
                    fs::rename(from, to)
                }
            },
            &|home, registry| write_fork_registry(home, registry),
            &|_| Ok(()),
        )
        .unwrap_err();

        assert!(error.contains("move independent copy"));
        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );
    }

    #[test]
    fn undo_restores_exact_relative_link_and_refuses_after_edits() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let event_id = make_skill_independent_copy(&store, request(&home, &link, &source)).unwrap();
        let event = store.get(&event_id).unwrap().unwrap();
        restore_independent_copy(&store, &home, &event).unwrap();
        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );
        assert!(read_fork_registry(&home).unwrap().copies.is_empty());

        let second_id =
            make_skill_independent_copy(&store, request(&home, &link, &source)).unwrap();
        fs::write(link.join("local-edit.txt"), "keep me").unwrap();
        let second = store.get(&second_id).unwrap().unwrap();
        let error = restore_independent_copy(&store, &home, &second).unwrap_err();
        assert!(error.contains("changed since"));
        assert_eq!(
            fs::read_to_string(link.join("local-edit.txt")).unwrap(),
            "keep me"
        );
        assert!(!read_fork_registry(&home).unwrap().copies.is_empty());
    }

    #[test]
    fn inverse_is_seeded_before_the_link_is_replaced() {
        let (temp, store, source, link) = setup();
        let id =
            make_skill_independent_copy(&store, request(&temp.path().join("home"), &link, &source))
                .unwrap();
        let event = store.get(&id).unwrap().unwrap();
        let inverse: InverseOp = serde_json::from_value(event.inverse.unwrap()).unwrap();
        let InverseOp::RecreateSymlink {
            post_fingerprint, ..
        } = inverse
        else {
            panic!("expected symlink inverse");
        };
        assert_eq!(post_fingerprint, Some(fingerprint_path(&link)));
    }

    #[test]
    fn restore_refuses_after_the_materialized_parent_becomes_a_whole_root_link() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let shared = home.join(".agents/skills");
        write_skill(&shared.join("find-bugs"), "Canonical body");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let root = home.join(".claude/skills");
        symlink("../.agents/skills", &root).unwrap();
        let store = EventStore::open(&temp.path().join("app-data")).unwrap();
        let explode_id =
            super::super::skill_materialize::explode_shared_dir(&store, &root, "claude-code")
                .unwrap();
        let copy_id = make_skill_independent_copy(
            &store,
            request(&home, &root.join("find-bugs"), &shared.join("find-bugs")),
        )
        .unwrap();

        let explode = store.get(&explode_id).unwrap().unwrap();
        let ordering_error =
            super::super::skill_materialize::restore_guard_for_explode(&store, &explode, &home)
                .unwrap_err();
        assert!(ordering_error.contains("undo independent copies first"));

        store.restore(&explode_id, true).unwrap();
        let copy = store.get(&copy_id).unwrap().unwrap();
        let error = validate_independent_copy_restore(&copy).unwrap_err();
        assert!(error.contains("no longer the real per-skill root"));
        assert!(fs::read_to_string(shared.join("find-bugs/SKILL.md"))
            .unwrap()
            .contains("Canonical body"));
    }

    #[test]
    fn restore_refuses_a_symlinked_recorded_parent_even_when_the_child_resolves() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let id = make_skill_independent_copy(&store, request(&home, &link, &source)).unwrap();
        let event = store.get(&id).unwrap().unwrap();
        let root = link.parent().unwrap();
        let moved = home.join("real-skills");
        fs::rename(root, &moved).unwrap();
        symlink(&moved, root).unwrap();

        assert!(validate_independent_copy_restore(&event)
            .unwrap_err()
            .contains("no longer the real per-skill root"));
        assert!(link.join("SKILL.md").is_file());
    }

    #[test]
    fn startup_recovery_claims_a_replaced_copy_without_deleting_edits() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let id = make_skill_independent_copy(&store, request(&home, &link, &source)).unwrap();
        let event = store.get(&id).unwrap().unwrap();
        let data = independent_copy_event_data(&event).unwrap();
        symlink(data.original_link_target.unwrap(), &data.saved_link).unwrap();
        fs::write(link.join("local-edit.txt"), "preserve me").unwrap();

        let mut registry = read_fork_registry(&home).unwrap();
        registry.copies.remove(&data.copy_deployment_id);
        write_fork_registry(&home, &registry).unwrap();
        mark_pending(&store, &id);
        recover_make(&store, &home, &id).unwrap();

        assert_eq!(
            fs::read_to_string(link.join("local-edit.txt")).unwrap(),
            "preserve me"
        );
        assert!(read_fork_registry(&home)
            .unwrap()
            .copies
            .contains_key(&data.copy_deployment_id));
        assert_eq!(store.get(&id).unwrap().unwrap().status, "done");
        recover_make(&store, &home, &id).unwrap();
        assert_eq!(store.get(&id).unwrap().unwrap().status, "done");
    }

    #[test]
    fn startup_recovery_after_registry_write_is_idempotent() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let id = make_skill_independent_copy(&store, request(&home, &link, &source)).unwrap();
        let event = store.get(&id).unwrap().unwrap();
        let data = independent_copy_event_data(&event).unwrap();
        symlink(data.original_link_target.unwrap(), &data.saved_link).unwrap();
        mark_pending(&store, &id);
        recover_make(&store, &home, &id).unwrap();
        assert!(link.is_dir());
        assert!(fs::symlink_metadata(&data.saved_link).is_err());
        assert_eq!(store.get(&id).unwrap().unwrap().status, "done");
        recover_make(&store, &home, &id).unwrap();
        assert_eq!(store.get(&id).unwrap().unwrap().status, "done");
    }

    #[test]
    fn every_return_after_staging_cleans_the_staging_directory() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let root = link.parent().unwrap().to_path_buf();
        let error = make_skill_independent_copy_with(
            &store,
            request(&home, &link, &source),
            &|from, to| fs::rename(from, to),
            &|home, registry| write_fork_registry(home, registry),
            &|phase| {
                if phase == "staged" {
                    fs::remove_file(&link).unwrap();
                }
                Ok(())
            },
        )
        .unwrap_err();

        assert!(error.contains("link changed while copying"));
        no_unknown_staging(&root);
    }

    #[test]
    fn crash_before_staging_leaves_the_original_link() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let root = link.parent().unwrap().to_path_buf();
        let error = make_skill_independent_copy_with(
            &store,
            request(&home, &link, &source),
            &|from, to| fs::rename(from, to),
            &|home, registry| write_fork_registry(home, registry),
            &crash_at("intent"),
        )
        .unwrap_err();
        assert!(error.starts_with(INJECTED_CRASH_PREFIX));
        let event_id = store.list(1, None).unwrap()[0].id.clone();
        recover_make(&store, &home, &event_id).unwrap();
        recover_make(&store, &home, &event_id).unwrap();

        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );
        assert!(read_fork_registry(&home).unwrap().copies.is_empty());
        no_unknown_staging(&root);
        assert_eq!(store.get(&event_id).unwrap().unwrap().status, "failed");
    }

    #[test]
    fn crash_after_staging_cleans_only_the_recorded_staging_path() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let root = link.parent().unwrap().to_path_buf();
        let unknown = root.join(".skill-studio-independent-unknown");
        fs::create_dir_all(&unknown).unwrap();
        fs::write(unknown.join("keep.txt"), "not ours").unwrap();
        let error = make_skill_independent_copy_with(
            &store,
            request(&home, &link, &source),
            &|from, to| fs::rename(from, to),
            &|home, registry| write_fork_registry(home, registry),
            &crash_at("staged"),
        )
        .unwrap_err();
        assert!(error.starts_with(INJECTED_CRASH_PREFIX));
        let event_id = store.list(1, None).unwrap()[0].id.clone();
        let staging = independent_copy_event_data(&store.get(&event_id).unwrap().unwrap())
            .unwrap()
            .staging;
        assert!(staging.is_dir());
        recover_make(&store, &home, &event_id).unwrap();
        recover_make(&store, &home, &event_id).unwrap();

        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );
        assert!(fs::symlink_metadata(&staging).is_err());
        assert_eq!(
            fs::read_to_string(unknown.join("keep.txt")).unwrap(),
            "not ours"
        );
        assert_eq!(store.get(&event_id).unwrap().unwrap().status, "failed");
    }

    #[test]
    fn recovery_without_a_staging_fingerprint_preserves_state_on_every_start() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        make_skill_independent_copy_with(
            &store,
            request(&home, &link, &source),
            &|from, to| fs::rename(from, to),
            &|home, registry| write_fork_registry(home, registry),
            &crash_at("staged"),
        )
        .unwrap_err();
        let event_id = store.list(1, None).unwrap()[0].id.clone();
        mark_pending(&store, &event_id);
        store.reconcile_at_startup().unwrap();
        let mut event = store.get(&event_id).unwrap().unwrap();
        let data = independent_copy_event_data(&event).unwrap();
        let mut registry = read_fork_registry(&home).unwrap();
        registry.copies.insert(
            data.copy_deployment_id.clone(),
            data.copy_record.clone().unwrap(),
        );
        write_fork_registry(&home, &registry).unwrap();
        event
            .payload
            .as_object_mut()
            .unwrap()
            .remove("staged_fingerprint");
        store
            .patch_event_payload(&event_id, &event.payload)
            .unwrap();

        for _ in 0..2 {
            let interrupted = store.get(&event_id).unwrap().unwrap();
            let error =
                reconcile_interrupted_independent_copy(&store, &home, &interrupted).unwrap_err();
            assert!(error.contains("without a recorded fingerprint"));
            assert!(data.staging.join("SKILL.md").is_file());
            assert!(read_fork_registry(&home)
                .unwrap()
                .copies
                .contains_key(&data.copy_deployment_id));
            assert_eq!(store.get(&event_id).unwrap().unwrap().status, "interrupted");
            assert!(fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink());
        }
    }

    #[test]
    fn recovery_with_a_changed_staging_fingerprint_preserves_state() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        make_skill_independent_copy_with(
            &store,
            request(&home, &link, &source),
            &|from, to| fs::rename(from, to),
            &|home, registry| write_fork_registry(home, registry),
            &crash_at("staged"),
        )
        .unwrap_err();
        let event_id = store.list(1, None).unwrap()[0].id.clone();
        mark_pending(&store, &event_id);
        store.reconcile_at_startup().unwrap();
        let event = store.get(&event_id).unwrap().unwrap();
        let data = independent_copy_event_data(&event).unwrap();
        fs::rename(&link, &data.saved_link).unwrap();
        fs::write(data.staging.join("unknown.txt"), "preserve me").unwrap();
        let mut registry = read_fork_registry(&home).unwrap();
        registry.copies.insert(
            data.copy_deployment_id.clone(),
            data.copy_record.clone().unwrap(),
        );
        write_fork_registry(&home, &registry).unwrap();

        let error = reconcile_interrupted_independent_copy(&store, &home, &event).unwrap_err();

        assert!(error.contains("staging fingerprint changed"));
        assert_eq!(
            fs::read_to_string(data.staging.join("unknown.txt")).unwrap(),
            "preserve me"
        );
        assert!(read_fork_registry(&home)
            .unwrap()
            .copies
            .contains_key(&data.copy_deployment_id));
        assert_eq!(store.get(&event_id).unwrap().unwrap().status, "interrupted");
        assert!(path_is_absent(&link));
        assert!(fs::symlink_metadata(&data.saved_link)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn verified_staging_removal_failure_is_reported_without_deleting_content() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        make_skill_independent_copy_with(
            &store,
            request(&home, &link, &source),
            &|from, to| fs::rename(from, to),
            &|home, registry| write_fork_registry(home, registry),
            &crash_at("staged"),
        )
        .unwrap_err();
        let event = store.list(1, None).unwrap().remove(0);
        let data = independent_copy_event_data(&event).unwrap();

        let error = remove_recorded_staging_with(&data, &|_| {
            Err(std::io::Error::other("injected removal failure"))
        })
        .unwrap_err();

        assert!(error.contains("failed to remove verified staging directory"));
        assert!(error.contains("injected removal failure"));
        assert!(data.staging.join("SKILL.md").is_file());
    }

    #[test]
    fn recovery_refuses_an_unknown_destination_directory_before_replacement() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let error = make_skill_independent_copy_with(
            &store,
            request(&home, &link, &source),
            &|from, to| fs::rename(from, to),
            &|home, registry| write_fork_registry(home, registry),
            &crash_at("staged"),
        )
        .unwrap_err();
        assert!(error.starts_with(INJECTED_CRASH_PREFIX));
        fs::remove_file(&link).unwrap();
        write_skill(&link, "Unknown replacement");
        let event_id = store.list(1, None).unwrap()[0].id.clone();

        let recovery_error = recover_make(&store, &home, &event_id).unwrap_err();

        assert!(recovery_error.contains("ambiguous"));
        assert!(fs::read_to_string(link.join("SKILL.md"))
            .unwrap()
            .contains("Unknown replacement"));
        assert!(read_fork_registry(&home).unwrap().copies.is_empty());
        assert_eq!(store.get(&event_id).unwrap().unwrap().status, "interrupted");
    }

    #[test]
    fn crash_after_whole_root_conversion_rolls_back_the_connected_intent() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let shared = home.join(".agents/skills");
        write_skill(&shared.join("find-bugs"), "Body");
        write_skill(&shared.join("write-docs"), "Sibling");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let root = home.join(".claude/skills");
        let root_target = PathBuf::from("../.agents/skills");
        symlink(&root_target, &root).unwrap();
        let store = EventStore::open(&temp.path().join("app-data")).unwrap();
        let link = root.join("find-bugs");
        let source = shared.join("find-bugs");
        let mut copy_request = request(&home, &link, &source);
        copy_request.convert_whole_root = true;
        let error = make_skill_independent_copy_with(
            &store,
            copy_request,
            &|from, to| fs::rename(from, to),
            &|home, registry| write_fork_registry(home, registry),
            &crash_at("converted"),
        )
        .unwrap_err();
        assert!(error.starts_with(INJECTED_CRASH_PREFIX));
        let event_id = store
            .list(8, None)
            .unwrap()
            .into_iter()
            .find(|row| row.kind == "make_independent_copy")
            .unwrap()
            .id;
        let parent = store.get(&event_id).unwrap().unwrap();
        let materialize_event_id = parent
            .payload
            .get("materialize_event_id")
            .and_then(Value::as_str)
            .unwrap();
        assert_eq!(
            store
                .materialized_root(&root)
                .unwrap()
                .unwrap()
                .created_by
                .as_deref(),
            Some(materialize_event_id)
        );
        assert_eq!(
            store.get(materialize_event_id).unwrap().unwrap().status,
            "done"
        );
        recover_make(&store, &home, &event_id).unwrap();
        recover_make(&store, &home, &event_id).unwrap();

        assert_eq!(fs::read_link(&root).unwrap(), root_target);
        assert!(store.materialized_root(&root).unwrap().is_none());
        assert_eq!(store.get(&event_id).unwrap().unwrap().status, "failed");
    }

    #[test]
    fn absent_whole_root_publishes_recorded_staging_before_parent_rollback() {
        let (_temp, store, home, shared, root) = setup_whole_root();
        let link = root.join("find-bugs");
        let source = shared.join("find-bugs");
        let mut copy_request = request(&home, &link, &source);
        copy_request.convert_whole_root = true;
        let error = make_skill_independent_copy_with(
            &store,
            copy_request,
            &|from, to| fs::rename(from, to),
            &|home, registry| write_fork_registry(home, registry),
            &crash_at("materialize_root_removed"),
        )
        .unwrap_err();
        assert!(error.starts_with(INJECTED_CRASH_PREFIX));
        assert!(fs::symlink_metadata(&root).is_err());
        let parent = store
            .list(8, None)
            .unwrap()
            .into_iter()
            .find(|row| row.kind == "make_independent_copy")
            .unwrap();
        let child_id = parent.payload["materialize_event_id"]
            .as_str()
            .unwrap()
            .to_string();
        let staging = store.get(&child_id).unwrap().unwrap().payload["staging"]
            .as_str()
            .map(PathBuf::from)
            .unwrap();

        recover_make(&store, &home, &parent.id).unwrap();
        recover_make(&store, &home, &parent.id).unwrap();

        assert_eq!(
            fs::read_link(&root).unwrap(),
            PathBuf::from("../.agents/skills")
        );
        assert!(fs::symlink_metadata(staging).is_err());
        assert_eq!(store.get(&parent.id).unwrap().unwrap().status, "failed");
        let child = store.get(&child_id).unwrap().unwrap();
        assert_eq!(child.status, "done");
        assert!(child.reverted_by.is_some());
    }

    #[test]
    fn absent_whole_root_without_staging_recreates_only_the_recorded_link() {
        let (_temp, store, home, shared, root) = setup_whole_root();
        let link = root.join("find-bugs");
        let source = shared.join("find-bugs");
        let mut copy_request = request(&home, &link, &source);
        copy_request.convert_whole_root = true;
        make_skill_independent_copy_with(
            &store,
            copy_request,
            &|from, to| fs::rename(from, to),
            &|home, registry| write_fork_registry(home, registry),
            &crash_at("materialize_root_removed"),
        )
        .unwrap_err();
        let parent = store
            .list(8, None)
            .unwrap()
            .into_iter()
            .find(|row| row.kind == "make_independent_copy")
            .unwrap();
        let child_id = parent.payload["materialize_event_id"].as_str().unwrap();
        let staging = PathBuf::from(
            store.get(child_id).unwrap().unwrap().payload["staging"]
                .as_str()
                .unwrap(),
        );
        fs::remove_dir_all(staging).unwrap();

        recover_make(&store, &home, &parent.id).unwrap();
        recover_make(&store, &home, &parent.id).unwrap();

        assert_eq!(
            fs::read_link(&root).unwrap(),
            PathBuf::from("../.agents/skills")
        );
        assert_eq!(store.get(child_id).unwrap().unwrap().status, "failed");
        assert_eq!(store.get(&parent.id).unwrap().unwrap().status, "failed");
    }

    #[test]
    fn materialize_recovery_does_not_replace_a_concurrent_destination() {
        let (_temp, store, home, shared, root) = setup_whole_root();
        let link = root.join("find-bugs");
        let source = shared.join("find-bugs");
        let mut copy_request = request(&home, &link, &source);
        copy_request.convert_whole_root = true;
        make_skill_independent_copy_with(
            &store,
            copy_request,
            &|from, to| fs::rename(from, to),
            &|home, registry| write_fork_registry(home, registry),
            &crash_at("materialize_root_removed"),
        )
        .unwrap_err();
        store.reconcile_at_startup().unwrap();
        let parent = store
            .interrupted_independent_copy_events()
            .unwrap()
            .into_iter()
            .find(|row| row.kind == "make_independent_copy")
            .unwrap();
        let child_id = parent.payload["materialize_event_id"].as_str().unwrap();
        let child = store.get(child_id).unwrap().unwrap();

        let error =
            super::super::skill_materialize::reconcile_connected_materialize_event_with_hook(
                &store,
                &child,
                &|| {
                    write_skill(&root, "Concurrent content");
                    Ok(())
                },
            )
            .unwrap_err();

        assert!(error.contains("without replacing existing content"));
        assert!(fs::read_to_string(root.join("SKILL.md"))
            .unwrap()
            .contains("Concurrent content"));
        assert_eq!(store.get(child_id).unwrap().unwrap().status, "interrupted");
        assert_eq!(
            store.get(&parent.id).unwrap().unwrap().status,
            "interrupted"
        );
    }

    #[test]
    fn crash_after_link_replacement_claims_copy_ownership() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let error = make_skill_independent_copy_with(
            &store,
            request(&home, &link, &source),
            &|from, to| fs::rename(from, to),
            &|home, registry| write_fork_registry(home, registry),
            &crash_at("replaced"),
        )
        .unwrap_err();
        assert!(error.starts_with(INJECTED_CRASH_PREFIX));
        let event_id = store.list(1, None).unwrap()[0].id.clone();
        recover_make(&store, &home, &event_id).unwrap();
        recover_make(&store, &home, &event_id).unwrap();

        assert!(link.is_dir());
        assert!(read_fork_registry(&home)
            .unwrap()
            .copies
            .values()
            .any(|record| record.path == link));
        assert_eq!(store.get(&event_id).unwrap().unwrap().status, "done");
    }

    #[test]
    fn crash_after_registry_write_finishes_without_deleting_the_copy() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let error = make_skill_independent_copy_with(
            &store,
            request(&home, &link, &source),
            &|from, to| fs::rename(from, to),
            &|home, registry| write_fork_registry(home, registry),
            &crash_at("registry"),
        )
        .unwrap_err();
        assert!(error.starts_with(INJECTED_CRASH_PREFIX));
        let event_id = store.list(1, None).unwrap()[0].id.clone();
        recover_make(&store, &home, &event_id).unwrap();
        recover_make(&store, &home, &event_id).unwrap();
        assert!(link.is_dir());
        assert!(!read_fork_registry(&home).unwrap().copies.is_empty());
        assert_eq!(store.get(&event_id).unwrap().unwrap().status, "done");
    }

    #[test]
    fn undo_crash_before_ownership_removal_still_restores_the_link() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let event_id = make_skill_independent_copy(&store, request(&home, &link, &source)).unwrap();
        let event = store.get(&event_id).unwrap().unwrap();
        let error = restore_independent_copy_with(&store, &home, &event, &crash_at("recorded"))
            .unwrap_err();
        assert!(error.starts_with(INJECTED_CRASH_PREFIX));
        let restore_id = store
            .list(8, None)
            .unwrap()
            .into_iter()
            .find(|row| row.kind == "restore")
            .unwrap()
            .id;
        let restore_event = store.get(&restore_id).unwrap().unwrap();
        assert!(!restore_event.restorable);
        assert!(store
            .restore(&restore_id, false)
            .unwrap_err()
            .contains("not restorable"));
        recover_restore(&store, &home, &restore_id).unwrap();
        recover_restore(&store, &home, &restore_id).unwrap();
        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );
        assert!(read_fork_registry(&home).unwrap().copies.is_empty());
        assert_eq!(store.get(&restore_id).unwrap().unwrap().status, "done");
    }

    #[test]
    fn undo_crash_after_ownership_removal_does_not_orphan_the_copy() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let event_id = make_skill_independent_copy(&store, request(&home, &link, &source)).unwrap();
        let event = store.get(&event_id).unwrap().unwrap();
        let error =
            restore_independent_copy_with(&store, &home, &event, &crash_at("ownership_removed"))
                .unwrap_err();
        assert!(error.starts_with(INJECTED_CRASH_PREFIX));
        assert!(read_fork_registry(&home).unwrap().copies.is_empty());
        assert!(link.is_dir());
        let restore_id = store
            .list(8, None)
            .unwrap()
            .into_iter()
            .find(|row| row.kind == "restore")
            .unwrap()
            .id;
        recover_restore(&store, &home, &restore_id).unwrap();
        recover_restore(&store, &home, &restore_id).unwrap();
        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );
        assert!(read_fork_registry(&home).unwrap().copies.is_empty());
        assert_eq!(store.get(&restore_id).unwrap().unwrap().status, "done");
    }

    #[test]
    fn undo_crash_after_copy_removal_publishes_the_recorded_staged_link() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let event_id = make_skill_independent_copy(&store, request(&home, &link, &source)).unwrap();
        let event = store.get(&event_id).unwrap().unwrap();

        let error = restore_independent_copy_with(&store, &home, &event, &crash_at("copy_removed"))
            .unwrap_err();
        assert!(error.starts_with(INJECTED_CRASH_PREFIX));
        let restore = store
            .list(8, None)
            .unwrap()
            .into_iter()
            .find(|row| row.kind == "restore")
            .unwrap();
        let staged_link = event_path(&restore, "staged_restore_link").unwrap();
        assert!(fs::symlink_metadata(&link).is_err());
        assert_eq!(
            fs::read_link(&staged_link).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );

        recover_restore(&store, &home, &restore.id).unwrap();
        recover_restore(&store, &home, &restore.id).unwrap();

        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );
        assert!(fs::symlink_metadata(&staged_link).is_err());
        assert!(read_fork_registry(&home).unwrap().copies.is_empty());
        assert_eq!(store.get(&restore.id).unwrap().unwrap().status, "done");
    }

    #[test]
    fn undo_crash_after_staged_link_loss_recreates_only_the_recorded_link() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let event_id = make_skill_independent_copy(&store, request(&home, &link, &source)).unwrap();
        let event = store.get(&event_id).unwrap().unwrap();

        let error = restore_independent_copy_with(&store, &home, &event, &|phase| {
            if phase == "copy_removed" {
                let restore = store
                    .list(8, None)?
                    .into_iter()
                    .find(|row| row.kind == "restore")
                    .ok_or("Restore event was not recorded")?;
                fs::remove_file(event_path(&restore, "staged_restore_link")?)
                    .map_err(|error| format!("Failed to remove staged link in test: {error}"))?;
                return Err(format!("{INJECTED_CRASH_PREFIX}: {phase}"));
            }
            Ok(())
        })
        .unwrap_err();
        assert!(error.starts_with(INJECTED_CRASH_PREFIX));
        let restore = store
            .list(8, None)
            .unwrap()
            .into_iter()
            .find(|row| row.kind == "restore")
            .unwrap();
        let staged_link = event_path(&restore, "staged_restore_link").unwrap();
        assert!(fs::symlink_metadata(&link).is_err());
        assert!(fs::symlink_metadata(&staged_link).is_err());

        recover_restore(&store, &home, &restore.id).unwrap();
        recover_restore(&store, &home, &restore.id).unwrap();

        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );
        assert!(read_fork_registry(&home).unwrap().copies.is_empty());
        assert_eq!(store.get(&restore.id).unwrap().unwrap().status, "done");
    }

    #[test]
    fn conflicting_ownership_recovery_stays_interrupted() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let id = make_skill_independent_copy(&store, request(&home, &link, &source)).unwrap();
        fs::remove_dir_all(&link).unwrap();
        symlink("../../.agents/skills/find-bugs", &link).unwrap();
        let mut registry = read_fork_registry(&home).unwrap();
        let deployment_id = registry.copies.keys().next().unwrap().clone();
        let mut conflicting = registry.copies.get(&deployment_id).unwrap().clone();
        conflicting.content_hash = "not-the-recorded-copy".to_string();
        registry.copies.insert(deployment_id, conflicting);
        write_fork_registry(&home, &registry).unwrap();

        mark_pending(&store, &id);
        let error = recover_make(&store, &home, &id).unwrap_err();
        assert!(error.contains("conflicting Copy ownership"));
        assert_eq!(store.get(&id).unwrap().unwrap().status, "interrupted");
        assert!(!read_fork_registry(&home).unwrap().copies.is_empty());
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn second_start_retries_an_existing_interrupted_make_event() {
        let (temp, store, source, link) = setup();
        let home = temp.path().join("home");
        let id = make_skill_independent_copy(&store, request(&home, &link, &source)).unwrap();
        fs::remove_dir_all(&link).unwrap();
        symlink("../../.agents/skills/find-bugs", &link).unwrap();
        let mut registry = read_fork_registry(&home).unwrap();
        let deployment_id = registry.copies.keys().next().unwrap().clone();
        registry
            .copies
            .get_mut(&deployment_id)
            .unwrap()
            .content_hash = "conflict".to_string();
        write_fork_registry(&home, &registry).unwrap();

        mark_pending(&store, &id);
        assert!(recover_make(&store, &home, &id).is_err());
        assert_eq!(store.get(&id).unwrap().unwrap().status, "interrupted");

        let mut registry = read_fork_registry(&home).unwrap();
        registry.copies.remove(&deployment_id);
        write_fork_registry(&home, &registry).unwrap();
        recover_make(&store, &home, &id).unwrap();

        assert_eq!(store.get(&id).unwrap().unwrap().status, "failed");
        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );
    }
}
