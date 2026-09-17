// ============================================================================
// Skills Module - deterministic malformed frontmatter repair
// Previews and applies the one safe first-version repair: an unquoted `: ` in
// a top-level name or description scalar.
// ============================================================================

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::Manager;

use super::event_commands::EventStoreState;
use super::event_store::{
    allocate_id, fingerprint_path, EventDraft, EventRow, EventStatus, EventStore, InverseOp,
};
use super::frontmatter::{parse_frontmatter, FrontmatterParseResult};
use super::skill_deployment::{BackingRelationship, DeploymentMutability, SkillDestination};
use super::skill_dto::{Deployment, LifecycleTarget};
use super::skill_fork::ForkMutationLock;
use super::skill_md_write::{begin_skill_md_write_transaction, SkillMdWriteTransaction};
use super::skill_ownership::LifecycleOwnerKind;
use super::skill_refresh::{self, SkillRefreshState};

use super::skill_document_operation::{check_document_cancellation, DocumentOperation};
use skill_studio_core::skill_frontmatter_repair::BoundFrontmatterRepairRequest;
pub use skill_studio_core::skill_frontmatter_repair::{
    FrontmatterRepairApplyMode, FrontmatterRepairPreview,
};
use skill_studio_core::skill_service::{CancellationToken, ScopedSkillService};

#[cfg(all(target_os = "macos", feature = "worker-repair"))]
fn prepare_scoped_fork_repair<'a>(
    service: &'a mut ScopedSkillService,
    request: &BoundFrontmatterRepairRequest,
    cancellation: CancellationToken,
) -> Result<skill_studio_core::skill_service::PreparedRepairSelection<'a>, String> {
    service
        .prepare_repair_selection(
            request,
            &[],
            Some(std::time::Duration::from_secs(30)),
            cancellation,
        )
        .map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyFrontmatterRepairRequest {
    pub target: LifecycleTarget,
    pub proposal_id: String,
    pub expected_content_fingerprint: String,
    pub mode: FrontmatterRepairApplyMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FrontmatterRepairIntent {
    deployment_id: String,
    name: String,
    path: PathBuf,
    expected_content_fingerprint: String,
    proposed_content: String,
    proposed_content_fingerprint: String,
    mode: FrontmatterRepairApplyMode,
    managed_update_warning: bool,
    fork_registry_before: Option<super::skill_fork_registry::ForkRegistry>,
}

fn content_fingerprint(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

fn proposal_id(deployment: &Deployment, fingerprint: &str, proposed: &str) -> String {
    let identity = format!(
        "{}\0{}\0{}\0{:?}\0{}\0{}",
        deployment.id,
        deployment.path,
        deployment.owner_id.as_deref().unwrap_or(""),
        deployment.owner_kind,
        fingerprint,
        proposed
    );
    content_fingerprint(identity.as_bytes())
}

fn frontmatter_end(lines: &[&str]) -> Option<usize> {
    if lines.first().map(|line| line.trim_end_matches('\r').trim()) != Some("---") {
        return None;
    }
    lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.trim_end_matches('\r').trim() == "---")
        .map(|(index, _)| index)
}

/// Produces an exact-byte proposal only when one top-level plain scalar is the
/// unique likely source of the YAML parser error.
pub fn propose_colon_scalar_repair(content: &str) -> Result<(String, String), String> {
    let parse_error = match parse_frontmatter(content) {
        FrontmatterParseResult::Invalid(error) => error,
        FrontmatterParseResult::Absent | FrontmatterParseResult::Valid(_) => {
            return Err("SKILL.md does not have a malformed YAML frontmatter block".to_string())
        }
    };
    let separator = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    if separator == "\r\n" && content.replace("\r\n", "").contains('\n') {
        return Err("Mixed line endings make the scalar boundary ambiguous".to_string());
    }
    let had_final_newline = content.ends_with(separator);
    let lines: Vec<&str> = content.split(separator).collect();
    let end = frontmatter_end(&lines).ok_or("Frontmatter is missing or unterminated")?;
    let mut candidates = Vec::new();
    for (index, line) in lines.iter().enumerate().take(end).skip(1) {
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some((key, value)) = line.split_once(": ") else {
            continue;
        };
        if !matches!(key, "name" | "description") || !value.contains(": ") {
            continue;
        }
        if value.starts_with(['\'', '"', '|', '>', '[', '{'])
            || value.ends_with(':')
            || value.contains(" #")
        {
            continue;
        }
        candidates.push((index, key, value));
    }
    let [(index, key, value)] = candidates.as_slice() else {
        return Err("No unique top-level name or description scalar can be repaired safely".into());
    };
    if parse_error.line != index + 1 || !parse_error.message.contains("mapping values") {
        return Err("The YAML error is not caused by the candidate scalar".to_string());
    }

    let replacement = if *key == "description" {
        format!("description: |-{}  {}", separator, value)
    } else {
        let quoted = serde_yaml::to_string(value)
            .map_err(|error| format!("Could not quote name: {error}"))?
            .trim_end()
            .to_string();
        if quoted.contains('\n') {
            return Err("Name repair would not remain single-line".to_string());
        }
        format!("name: {quoted}")
    };
    let mut proposed_lines: Vec<String> = lines.iter().map(|line| (*line).to_string()).collect();
    proposed_lines[*index] = replacement;
    let mut proposed = proposed_lines.join(separator);
    if had_final_newline && !proposed.ends_with(separator) {
        proposed.push_str(separator);
    }

    let parsed = match parse_frontmatter(&proposed) {
        FrontmatterParseResult::Valid(parsed) => parsed,
        _ => return Err("The proposed repair does not parse successfully".to_string()),
    };
    let repaired_value = if *key == "description" {
        parsed.description.as_deref()
    } else {
        parsed.name.as_deref()
    };
    if repaired_value != Some(*value) {
        return Err("The proposed repair changes the scalar value".to_string());
    }
    Ok((
        proposed,
        format!("Encode the top-level {key} value so its `: ` is text, not YAML syntax."),
    ))
}

fn apply_modes(deployment: &Deployment) -> Vec<FrontmatterRepairApplyMode> {
    if deployment.plugin.is_some()
        || deployment.is_symlink
        || deployment.shared_via_whole_dir_link
        || deployment.mutability == DeploymentMutability::ReadOnly
            && deployment.owner_kind != LifecycleOwnerKind::Manual
    {
        return vec![];
    }
    match deployment.owner_kind {
        LifecycleOwnerKind::SkillsSh | LifecycleOwnerKind::Dotagents => {
            if deployment.scope == "global"
                && deployment.destination == SkillDestination::Universal
                && matches!(deployment.backing, BackingRelationship::Canonical)
            {
                vec![
                    FrontmatterRepairApplyMode::ForkAndFix,
                    FrontmatterRepairApplyMode::FixInstalledCopy,
                ]
            } else {
                vec![]
            }
        }
        LifecycleOwnerKind::Copy | LifecycleOwnerKind::Fork | LifecycleOwnerKind::Manual => {
            vec![FrontmatterRepairApplyMode::ApplyFix]
        }
        _ => vec![],
    }
}

fn preview_from_deployment(deployment: &Deployment) -> Result<FrontmatterRepairPreview, String> {
    let path = Path::new(&deployment.path).join("SKILL.md");
    let bytes =
        fs::read(&path).map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
    let original =
        String::from_utf8(bytes.clone()).map_err(|_| "SKILL.md is not UTF-8".to_string())?;
    let (proposed, reason) = propose_colon_scalar_repair(&original)?;
    let fingerprint = content_fingerprint(&bytes);
    Ok(FrontmatterRepairPreview {
        deployment_id: deployment.id.clone(),
        path: deployment.path.clone(),
        scope: deployment.scope.clone(),
        reason,
        expected_content_fingerprint: fingerprint.clone(),
        proposal_id: proposal_id(deployment, &fingerprint, &proposed),
        original_content: original,
        proposed_content: proposed,
        allowed_apply_modes: apply_modes(deployment),
    })
}

fn validate_bound_preview(
    deployment: &Deployment,
    expected_content_fingerprint: &str,
    expected_proposal_id: &str,
) -> Result<FrontmatterRepairPreview, String> {
    let preview = preview_from_deployment(deployment)?;
    if preview.expected_content_fingerprint != expected_content_fingerprint
        || preview.proposal_id != expected_proposal_id
    {
        return Err(
            "YAML repair refused: the deployment, ownership, or content changed".to_string(),
        );
    }
    Ok(preview)
}

fn begin_bound_frontmatter_repair_transaction(
    deployment: &Deployment,
    expected_content_fingerprint: &str,
    expected_proposal_id: &str,
) -> Result<(SkillMdWriteTransaction, FrontmatterRepairPreview), String> {
    let transaction = begin_skill_md_write_transaction()?;
    let preview = validate_bound_preview(
        deployment,
        expected_content_fingerprint,
        expected_proposal_id,
    )?;
    Ok((transaction, preview))
}

fn exact_target<'a>(
    snapshot: &'a skill_refresh::SkillSnapshot,
    target: &LifecycleTarget,
) -> Result<&'a Deployment, String> {
    let id = target
        .deployment_id
        .as_deref()
        .ok_or("YAML repair needs one exact deployment_id")?;
    if target.owner_id.is_some() {
        return Err("YAML repair does not accept an owner target".to_string());
    }
    let (_, deployment) = super::skill_lifecycle::find_deployment(snapshot, id)?;
    super::skill_lifecycle::revalidate_deployment(deployment, id)?;
    Ok(deployment)
}

#[tauri::command]
pub async fn preview_skill_frontmatter_repair(
    target: LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<FrontmatterRepairPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        preview_skill_frontmatter_repair_blocking(
            target,
            app.clone(),
            app.state::<SkillRefreshState>(),
        )
    })
    .await
    .map_err(|error| format!("Repair preview task failed: {error}"))?
}

fn preview_skill_frontmatter_repair_blocking(
    target: LifecycleTarget,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
) -> Result<FrontmatterRepairPreview, String> {
    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    let deployment = exact_target(&snapshot, &target)?;
    if deployment.owner_kind == LifecycleOwnerKind::Copy
        || cfg!(all(target_os = "macos", feature = "worker-repair"))
    {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let projects = snapshot
            .projects
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let scope = super::skill_scope_config::desktop_skill_scope(&home, &projects)?;
        let mut service = ScopedSkillService::bind(scope).map_err(|error| error.to_string())?;
        return service
            .preview_frontmatter_repair(
                &deployment.id,
                Some(std::time::Duration::from_secs(30)),
                CancellationToken::default(),
            )
            .map_err(|error| error.to_string());
    }
    preview_from_deployment(deployment)
}

fn finish_repair_write(
    store: &EventStore,
    event_id: &str,
    skill_md: &Path,
    proposed: &[u8],
    write: impl FnOnce(&Path, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let result = write(skill_md, proposed);
    match &result {
        Ok(()) => {
            store.patch_inverse_post_fingerprint(event_id, &fingerprint_path(skill_md))?;
            store.finish(event_id, EventStatus::Done)?;
        }
        Err(_) => {
            store.finish(event_id, EventStatus::Failed)?;
        }
    }
    result
}

fn fork_record_matches(home: &Path, intent: &FrontmatterRepairIntent) -> Result<bool, String> {
    let registry = super::skill_fork_registry::read_fork_registry(home)?;
    Ok(registry.forks.get(&intent.name).is_some_and(|record| {
        record.deployment_id == intent.deployment_id && record.skill_dir == intent.path
    }))
}

fn managed_ledger_still_owns(home: &Path, name: &str) -> Result<bool, String> {
    let agents_dir = home.join(".agents");
    let skills_sh = super::lock_file::read_lock_file_at(&agents_dir.join(".skill-lock.json"))?
        .skills
        .contains_key(name);
    let dotagents = super::dotagents_ledger::read_dotagents_ledger(&agents_dir)?
        .iter()
        .any(|skill| skill.name == name);
    Ok(skills_sh || dotagents)
}

fn roll_back_incomplete_fork(
    store: &EventStore,
    home: &Path,
    row: &EventRow,
    intent: &FrontmatterRepairIntent,
) -> Result<(), String> {
    let registry = intent
        .fork_registry_before
        .as_ref()
        .ok_or("Fork repair intent has no ownership rollback snapshot")?;
    super::skill_fork_registry::write_fork_registry(home, registry)?;
    let app_data = store.app_data.clone();
    let _ = fs::remove_dir_all(super::skill_fork_registry::fork_snapshot_dir(
        &app_data,
        &intent.name,
    ));
    let _ = fs::remove_dir_all(
        app_data
            .join("skill-studio/forks")
            .join(&intent.name)
            .join("live-recovery"),
    );
    store.finish(&row.id, EventStatus::Failed)
}

/// Completes an atomic repair interrupted after durable intent was recorded.
/// A fork repair is resumed only when the exact fork record and unchanged
/// malformed bytes still match the intent.
pub fn reconcile_interrupted_frontmatter_repair(
    store: &EventStore,
    home: &Path,
    row: &EventRow,
) -> Result<(), String> {
    reconcile_interrupted_frontmatter_repair_with(store, home, row, |_| {})
}

fn reconcile_interrupted_frontmatter_repair_with(
    store: &EventStore,
    home: &Path,
    row: &EventRow,
    after_read: impl FnOnce(&SkillMdWriteTransaction),
) -> Result<(), String> {
    let transaction = begin_skill_md_write_transaction()?;
    let intent: FrontmatterRepairIntent = serde_json::from_value(row.payload.clone())
        .map_err(|error| format!("Malformed frontmatter repair intent: {error}"))?;
    let skill_md = intent.path.join("SKILL.md");
    let current = transaction.read(&skill_md)?;
    after_read(&transaction);
    let current_fingerprint = content_fingerprint(&current);
    if current_fingerprint == intent.proposed_content_fingerprint {
        store.patch_inverse_post_fingerprint(&row.id, &fingerprint_path(&skill_md))?;
        return store.finish(&row.id, EventStatus::Done);
    }
    if current_fingerprint != intent.expected_content_fingerprint {
        store.finish(&row.id, EventStatus::Failed)?;
        return Err("SKILL.md drifted from both sides of the repair intent".to_string());
    }
    if intent.mode != FrontmatterRepairApplyMode::ForkAndFix {
        store.finish(&row.id, EventStatus::Failed)?;
        return Ok(());
    }
    if managed_ledger_still_owns(home, &intent.name)? {
        roll_back_incomplete_fork(store, home, row, &intent)?;
        return Ok(());
    }
    if !fork_record_matches(home, &intent)? {
        store.finish(&row.id, EventStatus::Failed)?;
        return Err("The exact fork ownership record is absent or changed".to_string());
    }
    finish_repair_write(
        store,
        &row.id,
        &skill_md,
        intent.proposed_content.as_bytes(),
        |path, bytes| transaction.replace_bytes(path, bytes),
    )
}

#[tauri::command]
pub async fn apply_skill_frontmatter_repair(
    request: ApplyFrontmatterRepairRequest,
    app: tauri::AppHandle,
    operation_id: Option<String>,
) -> Result<(), String> {
    let operation = DocumentOperation::start(&app, operation_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        apply_skill_frontmatter_repair_blocking(
            request,
            app.clone(),
            app.state::<SkillRefreshState>(),
            app.state::<ForkMutationLock>(),
            app.state::<EventStoreState>(),
            _operation.cancellation.clone(),
        )
    })
    .await
    .map_err(|error| format!("Repair task failed: {error}"))?
}

fn apply_skill_frontmatter_repair_blocking(
    request: ApplyFrontmatterRepairRequest,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
    event_store: tauri::State<EventStoreState>,
    cancellation: CancellationToken,
) -> Result<(), String> {
    check_document_cancellation(&cancellation)?;
    let ApplyFrontmatterRepairRequest {
        target,
        proposal_id,
        expected_content_fingerprint,
        mode,
    } = request;
    let _guard = fork_lock.try_acquire()?;
    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    let deployment = exact_target(&snapshot, &target)?.clone();
    #[cfg(all(target_os = "macos", feature = "native-fork-repair"))]
    if mode == FrontmatterRepairApplyMode::ForkAndFix {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        if super::skill_native_fork::supports(&deployment, &home) {
            let projects = snapshot
                .projects
                .iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>();
            let scope = super::skill_scope_config::desktop_skill_scope(&home, &projects)?;
            let mut service = skill_studio_core::skill_service::ScopedSkillService::bind(scope)
                .map_err(|error| error.to_string())?;
            let gh = super::skill_update_check::resolve_gh_binary().ok_or("Run Check now first")?;
            let guard = event_store.0.lock().map_err(|error| error.to_string())?;
            let store = guard.as_ref().ok_or("Event store is unavailable")?;
            let transaction = begin_skill_md_write_transaction()?;
            let request = BoundFrontmatterRepairRequest {
                deployment_id: deployment.id.clone(),
                proposal_id,
                expected_content_fingerprint,
                mode,
            };
            let result = super::skill_native_fork::apply(
                &mut service,
                store,
                &request,
                &gh,
                &allocate_id(),
                cancellation,
            );
            drop(transaction);
            drop(guard);
            skill_refresh::request_snapshot_rebuild(&app);
            return result;
        }
    }
    #[cfg(all(target_os = "macos", feature = "worker-repair"))]
    if mode != FrontmatterRepairApplyMode::ForkAndFix
        && deployment.owner_kind != skill_studio_core::skill_ownership::LifecycleOwnerKind::Copy
    {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let projects = snapshot
            .projects
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let scope = super::skill_scope_config::desktop_skill_scope(&home, &projects)?;
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let name = skill_studio_core::skill_deployment::parse_deployment_id(&deployment.id)
            .ok_or("Invalid deployment ID")?
            .name;
        let guard = event_store.0.lock().map_err(|error| error.to_string())?;
        let store = guard.as_ref().ok_or("Event store is unavailable")?;
        let transaction = begin_skill_md_write_transaction()?;
        let request = BoundFrontmatterRepairRequest {
            deployment_id: deployment.id.clone(),
            proposal_id: proposal_id.clone(),
            expected_content_fingerprint,
            mode,
        };
        let event_id = allocate_id();
        let command = || {
            let mut command = std::process::Command::new(&executable);
            command.arg("__event-worker");
            command
        };
        let result = execute_scoped_desktop_repair(
            scope.clone(),
            &store.app_data,
            &request,
            &event_id,
            cancellation,
            command,
        );
        drop(transaction);
        let result =
            settle_desktop_document_operation(scope, store, &event_id, result, (), command);
        drop(guard);
        let projects = deployment
            .project_path
            .as_ref()
            .map(PathBuf::from)
            .into_iter()
            .collect::<Vec<_>>();
        skill_refresh::reconcile_skill_names_and_emit(&app, &refresh_state, [name], &projects)?;
        skill_refresh::request_snapshot_rebuild(&app);
        return result;
    }
    if deployment.owner_kind == LifecycleOwnerKind::Copy {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let projects = snapshot
            .projects
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let scope = super::skill_scope_config::desktop_skill_scope(&home, &projects)?;
        let mut service = ScopedSkillService::bind(scope).map_err(|error| error.to_string())?;
        let guard = event_store.0.lock().map_err(|error| error.to_string())?;
        let store = guard.as_ref().ok_or("Event store is unavailable")?;
        let transaction = begin_skill_md_write_transaction()?;
        let request = BoundFrontmatterRepairRequest {
            deployment_id: deployment.id.clone(),
            proposal_id: proposal_id.clone(),
            expected_content_fingerprint,
            mode,
        };
        let result = super::skill_copy_repair::apply(
            &mut service,
            store,
            &request,
            &allocate_id(),
            cancellation,
        );
        drop(transaction);
        drop(guard);
        skill_refresh::request_snapshot_rebuild(&app);
        return result;
    }
    check_document_cancellation(&cancellation)?;

    #[cfg(all(target_os = "macos", feature = "worker-repair"))]
    let mut fork_service = if mode == FrontmatterRepairApplyMode::ForkAndFix {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let projects = snapshot
            .projects
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let scope = super::skill_scope_config::desktop_skill_scope(&home, &projects)?;
        Some(ScopedSkillService::bind(scope).map_err(|error| error.to_string())?)
    } else {
        None
    };
    #[cfg(all(target_os = "macos", feature = "worker-repair"))]
    let mut fork_selection = if let Some(service) = fork_service.as_mut() {
        let request = BoundFrontmatterRepairRequest {
            deployment_id: deployment.id.clone(),
            proposal_id: proposal_id.clone(),
            expected_content_fingerprint: expected_content_fingerprint.clone(),
            mode,
        };
        Some(prepare_scoped_fork_repair(
            service,
            &request,
            cancellation.clone(),
        )?)
    } else {
        None
    };
    #[cfg(all(target_os = "macos", feature = "worker-repair"))]
    let proposal_id = fork_selection.as_ref().map_or(proposal_id, |selection| {
        self::proposal_id(
            &deployment,
            &expected_content_fingerprint,
            &selection.preview().proposed_content,
        )
    });
    let skill_md = PathBuf::from(&deployment.path).join("SKILL.md");
    let name = super::skill_deployment::parse_deployment_id(&deployment.id)
        .map(|id| id.name)
        .unwrap_or_default();
    let mut guard = event_store
        .0
        .lock()
        .map_err(|error| format!("event store lock poisoned: {error}"))?;
    let store = guard.as_mut().ok_or("Event store is unavailable")?;
    let (transaction, preview) = begin_bound_frontmatter_repair_transaction(
        &deployment,
        &expected_content_fingerprint,
        &proposal_id,
    )?;
    if !preview.allowed_apply_modes.contains(&mode) {
        return Err("This repair mode is not allowed for the selected deployment".to_string());
    }
    let event_id = allocate_id();
    let pre_fingerprint = fingerprint_path(&skill_md);
    let intent = FrontmatterRepairIntent {
        deployment_id: deployment.id.clone(),
        name: name.clone(),
        path: PathBuf::from(&deployment.path),
        expected_content_fingerprint: expected_content_fingerprint.clone(),
        proposed_content_fingerprint: content_fingerprint(preview.proposed_content.as_bytes()),
        proposed_content: preview.proposed_content.clone(),
        mode,
        managed_update_warning: mode == FrontmatterRepairApplyMode::FixInstalledCopy,
        fork_registry_before: if mode == FrontmatterRepairApplyMode::ForkAndFix {
            Some(super::skill_fork_registry::read_fork_registry(
                &dirs::home_dir().ok_or("Could not find home directory")?,
            )?)
        } else {
            None
        },
    };
    #[cfg(all(target_os = "macos", feature = "worker-repair"))]
    if let Some(selection) = fork_selection.as_ref() {
        selection.revalidate().map_err(|_| {
            "YAML repair refused: the deployment, ownership, or content changed".to_string()
        })?;
    }
    store.backup_paths(&event_id, std::slice::from_ref(&skill_md))?;
    store.record(
        &event_id,
        EventDraft {
            kind: "repair_skill_frontmatter".to_string(),
            skill: name.clone(),
            harness: None,
            scope: Some(deployment.scope.clone()),
            project_path: deployment.project_path.clone(),
            payload: serde_json::to_value(&intent)
                .map_err(|error| format!("Failed to serialize repair intent: {error}"))?,
            inverse: Some(
                serde_json::to_value(InverseOp::RestoreBackup {
                    path: skill_md.clone(),
                    pre_fingerprint,
                    post_fingerprint: None,
                })
                .map_err(|error| format!("Failed to serialize repair undo: {error}"))?,
            ),
            backup_dir: Some(format!("backups/{event_id}")),
            restorable: true,
        },
    )?;

    if mode == FrontmatterRepairApplyMode::ForkAndFix {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let app_data = app
            .path()
            .app_data_dir()
            .map_err(|error| format!("Could not resolve app data dir: {error}"))?;
        #[cfg(all(target_os = "macos", feature = "worker-repair"))]
        let fork_result = super::skill_fork::fork_resolved_deployment_with_real_services(
            &home,
            &app_data,
            &name,
            Path::new(&deployment.path),
            fork_selection
                .take()
                .ok_or("Fork repair lost its approved ownership selection")?,
        );
        #[cfg(not(all(target_os = "macos", feature = "worker-repair")))]
        let fork_result = super::skill_fork::fork_resolved_deployment_with_real_services(
            &home,
            &app_data,
            &name,
            Path::new(&deployment.path),
        );
        if let Err(error) = fork_result {
            store.finish(&event_id, EventStatus::Failed)?;
            return Err(error);
        }
        let live = transaction
            .read(&skill_md)
            .map_err(|error| format!("Fork completed, but the repair needs recovery: {error}"))?;
        if content_fingerprint(&live) != expected_content_fingerprint {
            return Err(
                "Fork completed, but SKILL.md changed before repair; Activity recovery is required"
                    .to_string(),
            );
        }
    }
    let result = if mode == FrontmatterRepairApplyMode::ForkAndFix {
        // Keep durable intent pending if the write fails. Startup can safely
        // finish it because the exact fork record and source fingerprint bind it.
        transaction
            .replace_bytes(&skill_md, preview.proposed_content.as_bytes())
            .and_then(|()| {
                store.patch_inverse_post_fingerprint(&event_id, &fingerprint_path(&skill_md))?;
                store.finish(&event_id, EventStatus::Done)
            })
    } else {
        finish_repair_write(
            store,
            &event_id,
            &skill_md,
            preview.proposed_content.as_bytes(),
            |path, bytes| transaction.replace_bytes(path, bytes),
        )
    };
    drop(transaction);
    drop(guard);
    let affected_projects: Vec<PathBuf> = deployment
        .project_path
        .as_deref()
        .map(PathBuf::from)
        .into_iter()
        .collect();
    skill_refresh::reconcile_skill_names_and_emit(
        &app,
        &refresh_state,
        [name],
        &affected_projects,
    )?;
    skill_refresh::request_snapshot_rebuild(&app);
    result
}

#[cfg(all(target_os = "macos", feature = "worker-repair"))]
fn execute_scoped_desktop_repair(
    scope: skill_studio_core::skill_service::SkillScope,
    state_root: &Path,
    request: &BoundFrontmatterRepairRequest,
    event_id: &str,
    cancellation: CancellationToken,
    command: impl Fn() -> std::process::Command,
) -> Result<(), String> {
    use skill_studio_core::skill_service::ScopedSkillService;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut service = ScopedSkillService::bind(scope).map_err(|error| error.to_string())?;
    let selection = service
        .prepare_repair_selection(
            request,
            &[state_root.to_path_buf()],
            Some(deadline.saturating_duration_since(std::time::Instant::now())),
            cancellation.clone(),
        )
        .map_err(|error| error.to_string())?;
    skill_studio_core::skill_repair_worker::RepairEventWorker {
        state_root,
        command: &command,
        cancellation: &cancellation,
        deadline,
    }
    .execute(selection, event_id)
    .map(|_| ())
    .map_err(|error| error.to_string())
}

#[cfg(all(target_os = "macos", feature = "worker-repair"))]
pub(crate) fn restore_scoped_desktop_repair(
    scope: skill_studio_core::skill_service::SkillScope,
    state_root: &Path,
    source: &EventRow,
    force: bool,
    event_id: &str,
    cancellation: CancellationToken,
    command: impl Fn() -> std::process::Command,
) -> Result<bool, String> {
    use skill_studio_core::skill_service::ScopedSkillService;
    let payload = match source.kind.as_str() {
        "repair_skill_frontmatter" => &source.payload,
        "restore" => match source.payload.get("repair") {
            Some(payload) => payload,
            None => return Ok(false),
        },
        _ => return Ok(false),
    };
    let intent: skill_studio_core::skill_repair_intent::FrontmatterRepairIntent =
        serde_json::from_value(payload.clone()).map_err(|error| error.to_string())?;
    intent.validate_record()?;
    if intent.mode == FrontmatterRepairApplyMode::ForkAndFix {
        return Ok(false);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let remaining = || Some(deadline.saturating_duration_since(std::time::Instant::now()));
    let mut service = ScopedSkillService::bind(scope).map_err(|error| error.to_string())?;
    let names = std::collections::BTreeSet::from([intent.name.clone()]);
    let inventory = service
        .scan_cancellable(Some(&names), remaining(), cancellation.clone())
        .map_err(|error| error.to_string())?;
    let deployment = inventory
        .skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .find(|deployment| deployment.id == intent.deployment_id)
        .ok_or("Restore deployment is no longer available")?;
    if deployment.owner_kind == skill_studio_core::skill_ownership::LifecycleOwnerKind::Copy {
        return Ok(false);
    }
    let prepared = service
        .prepare_direct_restore(source, state_root, force, remaining(), cancellation.clone())
        .map_err(|error| error.to_string())?;
    skill_studio_core::skill_repair_worker::RepairEventWorker {
        state_root,
        command: &command,
        cancellation: &cancellation,
        deadline,
    }
    .restore(prepared, event_id)
    .map(|_| true)
    .map_err(|error| error.to_string())
}

#[cfg(all(target_os = "macos", feature = "worker-repair"))]
fn recover_scoped_desktop_event(
    scope: skill_studio_core::skill_service::SkillScope,
    store: &EventStore,
    row: &EventRow,
    command: &dyn Fn() -> std::process::Command,
) -> Result<(), String> {
    use skill_studio_core::skill_service::ScopedSkillService;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let remaining = || Some(deadline.saturating_duration_since(std::time::Instant::now()));
    let mut service = ScopedSkillService::bind(scope).map_err(|error| error.to_string())?;
    let _transaction = begin_skill_md_write_transaction()?;
    let cancellation = CancellationToken::default();
    let worker = skill_studio_core::skill_repair_worker::RepairEventWorker {
        state_root: &store.app_data,
        command,
        cancellation: &cancellation,
        deadline,
    };
    if row.kind == "restore" {
        let source_id = row
            .payload
            .get("target_event")
            .and_then(serde_json::Value::as_str)
            .ok_or("Restore recovery is missing its source ID")?;
        let source = store
            .get(source_id)?
            .ok_or("Restore recovery source is unavailable")?;
        let prepared = service
            .prepare_direct_restore_recovery(
                &source,
                row,
                &store.app_data,
                remaining(),
                cancellation.clone(),
            )
            .map_err(|error| error.to_string())?;
        worker
            .recover_restore(prepared)
            .map_err(|error| error.to_string())?;
    } else {
        let prepared = service
            .prepare_repair_event_recovery(
                row,
                std::slice::from_ref(&store.app_data),
                remaining(),
                cancellation.clone(),
            )
            .map_err(|error| error.to_string())?;
        worker
            .recover(prepared)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(all(target_os = "macos", feature = "worker-repair"))]
pub(crate) fn settle_desktop_document_operation<T>(
    scope: skill_studio_core::skill_service::SkillScope,
    store: &EventStore,
    event_id: &str,
    result: Result<T, String>,
    completed: T,
    command: impl Fn() -> std::process::Command,
) -> Result<T, String> {
    let error = match result {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    let Some(row) = store.get(event_id)? else {
        return Err(error);
    };
    if matches!(row.status.as_str(), "pending" | "interrupted") {
        recover_scoped_desktop_event(scope, store, &row, &command)
            .map_err(|recovery| format!("{error}; recovery remains unresolved: {recovery}"))?;
    }
    if store.get(event_id)?.is_some_and(|row| row.status == "done") {
        Ok(completed)
    } else {
        Err(error)
    }
}

#[cfg(all(target_os = "macos", feature = "worker-repair"))]
pub(crate) fn recover_desktop_repair(
    scope: skill_studio_core::skill_service::SkillScope,
    store: &EventStore,
    row: &EventRow,
    command: &dyn Fn() -> std::process::Command,
) -> Result<(), String> {
    use skill_studio_core::skill_service::ScopedSkillService;
    let payload = if row.kind == "restore" {
        row.payload
            .get("repair")
            .ok_or("Missing restore repair intent")?
    } else {
        &row.payload
    };
    let intent: skill_studio_core::skill_repair_intent::FrontmatterRepairIntent =
        serde_json::from_value(payload.clone()).map_err(|error| error.to_string())?;
    intent.validate_record()?;
    if intent.mode == FrontmatterRepairApplyMode::ForkAndFix && row.kind != "restore" {
        return reconcile_interrupted_frontmatter_repair(store, &scope.home, row);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let remaining = || Some(deadline.saturating_duration_since(std::time::Instant::now()));
    let mut service = ScopedSkillService::bind(scope.clone()).map_err(|error| error.to_string())?;
    let inventory = service
        .scan(None, remaining())
        .map_err(|error| error.to_string())?;
    let deployment = inventory
        .skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .find(|deployment| deployment.id == intent.deployment_id)
        .ok_or("Recovery deployment is no longer available")?;
    if deployment.owner_kind == skill_studio_core::skill_ownership::LifecycleOwnerKind::Copy
        && row.kind != "restore"
    {
        return reconcile_interrupted_frontmatter_repair(store, &scope.home, row);
    }
    recover_scoped_desktop_event(scope.clone(), store, row, command)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(all(target_os = "macos", feature = "worker-repair"))]
    #[test]
    #[ignore = "private desktop event worker fixture"]
    fn desktop_event_worker_child() -> std::process::ExitCode {
        unsafe { skill_studio_core::skill_event_worker_entry::run_event_worker_stdio() }
    }

    #[cfg(all(target_os = "macos", feature = "worker-repair"))]
    #[test]
    fn desktop_worker_bridge_preserves_preview_ownership_and_history() {
        verify_desktop_worker_bridge(|| {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args([
                "--exact",
                "skills::skill_frontmatter_repair::tests::desktop_event_worker_child",
                "--ignored",
                "--nocapture",
            ]);
            command
        });
    }

    #[cfg(all(target_os = "macos", feature = "worker-repair"))]
    #[test]
    fn desktop_worker_bridge_preflights_repair_and_restore_sizes_before_backup() {
        use skill_studio_core::skill_event_worker_protocol::PreparedEventExchange;
        use skill_studio_core::skill_repair_intent::FrontmatterRepairIntent as CoreRepairIntent;
        use skill_studio_core::skill_service::{ScopedSkillService, SkillScope};

        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let skill = home.join(".agents/skills/boundary");
        let path = skill.join("SKILL.md");
        fs::create_dir_all(&skill).unwrap();
        let scope = SkillScope {
            home,
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        };
        let state = temp.path().join("state");
        fs::create_dir(&state).unwrap();
        let content = |body: usize| {
            format!(
                "---\nname: boundary\ndescription: Use when: testing\n---\n{}",
                "x".repeat(body)
            )
        };
        fs::write(&path, content(0)).unwrap();
        let mut service = ScopedSkillService::bind(scope.clone()).unwrap();
        let inventory = service
            .scan(None, Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let deployment = inventory.skills[0].deployments[0].clone();
        let timestamp = "2026-09-16T12:34:56.123456789Z";
        let drafts = |body: usize| {
            let original = content(body);
            let preview = skill_studio_core::skill_frontmatter_repair::preview_frontmatter_repair(
                &deployment,
                original.as_bytes(),
            )
            .unwrap();
            let intent = CoreRepairIntent::from_preview(
                &preview,
                FrontmatterRepairApplyMode::ApplyFix,
                None,
            )
            .unwrap();
            let before = "b".repeat(64);
            let after = "a".repeat(64);
            let inverse = serde_json::json!({
                "op": "restore_backup",
                "path": path,
                "pre_fingerprint": before,
                "post_fingerprint": after,
            });
            let repair = skill_studio_core::skill_event::EventDraft {
                kind: "repair_skill_frontmatter".into(),
                skill: "boundary".into(),
                harness: None,
                scope: Some("global".into()),
                project_path: None,
                payload: serde_json::to_value(&intent).unwrap(),
                inverse: Some(inverse.clone()),
                backup_dir: Some("backups/boundary-repair".into()),
                restorable: true,
            };
            let source = skill_studio_core::skill_event::EventRow {
                id: "boundary-repair".into(),
                ts: timestamp.into(),
                kind: repair.kind.clone(),
                skill: repair.skill.clone(),
                harness: None,
                scope: repair.scope.clone(),
                project_path: None,
                payload: repair.payload.clone(),
                inverse: repair.inverse.clone(),
                backup_dir: repair.backup_dir.clone(),
                status: "done".into(),
                reverted_by: None,
                restorable: true,
            };
            let restore = skill_studio_core::skill_event::EventDraft {
                kind: "restore".into(),
                skill: "boundary".into(),
                harness: None,
                scope: Some("global".into()),
                project_path: None,
                payload: serde_json::json!({
                    "target_event": "boundary-repair",
                    "repair": intent,
                    "before": after,
                    "after": before,
                }),
                inverse: Some(inverse),
                backup_dir: Some("backups/boundary-restore".into()),
                restorable: true,
            };
            (repair, source, restore)
        };
        let fits = |body: usize| {
            let (repair, source, restore) = drafts(body);
            let repair_fits = PreparedEventExchange::record_pending(
                "preflight".into(),
                "repair".into(),
                "boundary-repair".into(),
                timestamp.into(),
                repair,
            )
            .is_ok();
            let restore_fits = PreparedEventExchange::record_restore(
                "preflight".into(),
                "restore".into(),
                "boundary-restore".into(),
                source,
                timestamp.into(),
                restore,
            )
            .is_ok();
            (repair_fits, restore_fits)
        };
        let largest = |index: usize| {
            let (mut low, mut high) = (
                0,
                skill_studio_core::skill_history::MAX_HISTORY_RECORD_BYTES,
            );
            while low < high {
                let middle = low + (high - low).div_ceil(2);
                let fit = fits(middle);
                if [fit.0, fit.1][index] {
                    low = middle;
                } else {
                    high = middle - 1;
                }
            }
            low
        };
        let repair_max = largest(0);
        let restore_max = largest(1);
        assert!(restore_max < repair_max);
        let boundary_body = restore_max + (repair_max - restore_max).div_ceil(2);
        assert_eq!(fits(boundary_body), (true, false));

        fs::write(&path, content(boundary_body)).unwrap();
        let preview = service
            .preview_frontmatter_repair(
                &deployment.id,
                Some(std::time::Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        let request = BoundFrontmatterRepairRequest {
            deployment_id: deployment.id.clone(),
            proposal_id: preview.proposal_id,
            expected_content_fingerprint: preview.expected_content_fingerprint,
            mode: FrontmatterRepairApplyMode::ApplyFix,
        };
        let command = || {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args([
                "--exact",
                "skills::skill_frontmatter_repair::tests::desktop_event_worker_child",
                "--ignored",
                "--nocapture",
            ]);
            command
        };
        execute_scoped_desktop_repair(
            scope.clone(),
            &state,
            &request,
            "boundary-repair",
            CancellationToken::default(),
            command,
        )
        .unwrap();
        let store = EventStore::open(&state).unwrap();
        let source = store.get("boundary-repair").unwrap().unwrap();
        assert!(restore_scoped_desktop_repair(
            scope.clone(),
            &state,
            &source,
            false,
            "boundary-restore",
            CancellationToken::default(),
            || panic!("oversized restore must not launch a worker")
        )
        .is_err());
        assert!(store.get("boundary-restore").unwrap().is_none());
        assert!(!state.join("backups/boundary-restore").exists());

        fs::write(
            &path,
            content(skill_studio_core::skill_history::MAX_HISTORY_RECORD_BYTES),
        )
        .unwrap();
        let preview = service
            .preview_frontmatter_repair(
                &deployment.id,
                Some(std::time::Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        let oversized = BoundFrontmatterRepairRequest {
            deployment_id: deployment.id,
            proposal_id: preview.proposal_id,
            expected_content_fingerprint: preview.expected_content_fingerprint,
            mode: FrontmatterRepairApplyMode::ApplyFix,
        };
        assert!(execute_scoped_desktop_repair(
            scope,
            &state,
            &oversized,
            "oversized-repair",
            CancellationToken::default(),
            || panic!("oversized repair must not launch a worker")
        )
        .is_err());
        assert!(store.get("oversized-repair").unwrap().is_none());
        assert!(!state.join("backups/oversized-repair").exists());
    }

    #[cfg(all(target_os = "macos", feature = "worker-repair"))]
    #[test]
    #[ignore = "requires SKILL_STUDIO_RELEASE_WORKER pointing to a built desktop executable"]
    fn desktop_release_worker_bridge_preserves_preview_ownership_and_history() {
        let executable = std::path::PathBuf::from(
            std::env::var_os("SKILL_STUDIO_RELEASE_WORKER").expect("release worker path"),
        );
        assert!(executable.is_absolute() && executable.is_file());
        let environment = tempfile::tempdir().unwrap();
        verify_desktop_worker_bridge(|| {
            let mut command = std::process::Command::new(&executable);
            command
                .arg("__event-worker")
                .env_clear()
                .env("HOME", environment.path())
                .env("CFFIXED_USER_HOME", environment.path())
                .env("TMPDIR", environment.path())
                .env("PATH", "/usr/bin:/bin");
            command
        });
    }

    #[cfg(all(target_os = "macos", feature = "worker-repair"))]
    fn verify_desktop_worker_bridge(command: impl Fn() -> std::process::Command + Copy) {
        use skill_studio_core::skill_service::{CancellationToken, ScopedSkillService, SkillScope};
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let path = home.join(".agents/skills/sample/SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "---\nname: sample\ndescription: Use when: testing\n---\nbody\n",
        )
        .unwrap();
        let lock = home.join(".agents/.skill-lock.json");
        fs::write(&lock, r#"{"version":3,"skills":{"sample":{"source":"fixture/repo","sourceType":"github","sourceUrl":"https://example.invalid/repo","skillFolderHash":"hash","installedAt":"before","updatedAt":"before"}}}"#).unwrap();
        let saved_lock = fs::read(&lock).unwrap();
        let state = temp.path().join("state");
        fs::create_dir(&state).unwrap();
        let scope = SkillScope {
            home,
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        };
        let mut service = ScopedSkillService::bind(scope.clone()).unwrap();
        let inventory = service
            .scan(None, Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let id = inventory.skills[0].deployments[0].id.clone();
        let preview = service
            .preview_frontmatter_repair(
                &id,
                Some(std::time::Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        let fork_request = BoundFrontmatterRepairRequest {
            deployment_id: id.clone(),
            proposal_id: preview.proposal_id.clone(),
            expected_content_fingerprint: preview.expected_content_fingerprint.clone(),
            mode: FrontmatterRepairApplyMode::ForkAndFix,
        };
        let original = fs::read(&path).unwrap();
        fs::write(&path, [original.as_slice(), b"external edit"].concat()).unwrap();
        assert!(prepare_scoped_fork_repair(
            &mut service,
            &fork_request,
            CancellationToken::default()
        )
        .is_err());
        assert!(!state.join("events.sqlite3").exists());
        fs::write(&path, &original).unwrap();
        fs::write(&lock, r#"{"version":3,"skills":{"sample":{"source":"changed/repo","sourceType":"github","sourceUrl":"https://example.invalid/changed","skillFolderHash":"hash","installedAt":"before","updatedAt":"after"}}}"#).unwrap();
        assert!(prepare_scoped_fork_repair(
            &mut service,
            &fork_request,
            CancellationToken::default()
        )
        .is_err());
        assert!(!state.join("events.sqlite3").exists());
        fs::write(&lock, &saved_lock).unwrap();
        drop(service);
        let request = BoundFrontmatterRepairRequest {
            deployment_id: id,
            proposal_id: preview.proposal_id,
            expected_content_fingerprint: preview.expected_content_fingerprint,
            mode: FrontmatterRepairApplyMode::FixInstalledCopy,
        };
        let cancelled = CancellationToken::default();
        cancelled.cancel();
        assert!(execute_scoped_desktop_repair(
            scope.clone(),
            &state,
            &request,
            "cancelled-repair",
            cancelled,
            || panic!("cancelled repair must not launch a worker")
        )
        .is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(!state.join("events.sqlite3").exists());

        execute_scoped_desktop_repair(
            scope.clone(),
            &state,
            &request,
            "desktop-repair",
            CancellationToken::default(),
            command,
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), preview.proposed_content);
        let store = EventStore::open(&state).unwrap();
        let repair = store.get("desktop-repair").unwrap().unwrap();
        assert_eq!(repair.status, "done");
        store.conn.execute("UPDATE events SET status = 'pending', inverse = json_set(inverse, '$.post_fingerprint', NULL) WHERE id = 'desktop-repair'", []).unwrap();
        settle_desktop_document_operation(
            scope.clone(),
            &store,
            "desktop-repair",
            Err("cancelled".to_string()),
            (),
            command,
        )
        .unwrap();
        assert!(settle_desktop_document_operation(
            scope.clone(),
            &store,
            "absent",
            Err("cancelled".to_string()),
            (),
            || panic!("absent intent must not launch recovery")
        )
        .is_err());
        assert_eq!(store.get("desktop-repair").unwrap().unwrap().status, "done");
        assert_eq!(fs::read_to_string(&path).unwrap(), preview.proposed_content);
        assert!(restore_scoped_desktop_repair(
            scope.clone(),
            &state,
            &repair,
            false,
            "desktop-restore",
            CancellationToken::default(),
            command
        )
        .unwrap());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(
            store
                .get("desktop-repair")
                .unwrap()
                .unwrap()
                .reverted_by
                .as_deref(),
            Some("desktop-restore")
        );
        store.conn.execute("UPDATE events SET status = 'interrupted', inverse = json_set(inverse, '$.post_fingerprint', NULL) WHERE id = 'desktop-restore'", []).unwrap();
        super::super::skill_startup_recovery::recover_all_with_worker(
            scope.clone(),
            &store,
            &command,
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(
            store.get("desktop-restore").unwrap().unwrap().status,
            "done"
        );
        let restore = store.get("desktop-restore").unwrap().unwrap();
        let edited = b"---\nname: sample\ndescription: user edit\n---\nlocal changes\n";
        fs::write(&path, edited).unwrap();
        assert!(restore_scoped_desktop_repair(
            scope.clone(),
            &state,
            &restore,
            false,
            "refused",
            CancellationToken::default(),
            command
        )
        .is_err());
        assert!(store.get("refused").unwrap().is_none());
        assert_eq!(fs::read(&path).unwrap(), edited);
        assert!(restore_scoped_desktop_repair(
            scope.clone(),
            &state,
            &restore,
            true,
            "forced",
            CancellationToken::default(),
            command
        )
        .unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), preview.proposed_content);
        let forced = store.get("forced").unwrap().unwrap();
        assert!(restore_scoped_desktop_repair(
            scope.clone(),
            &state,
            &forced,
            false,
            "preserved",
            CancellationToken::default(),
            command
        )
        .unwrap());
        assert_eq!(fs::read(&path).unwrap(), edited);
        store.conn.execute("UPDATE events SET status = 'interrupted', inverse = json_set(inverse, '$.post_fingerprint', NULL) WHERE id = 'preserved'", []).unwrap();
        fs::write(&path, "unrelated edit during recovery").unwrap();
        assert!(
            super::super::skill_startup_recovery::recover_all_with_worker(
                scope.clone(),
                &store,
                &command
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "unrelated edit during recovery"
        );
        assert_eq!(
            store.get("preserved").unwrap().unwrap().status,
            "interrupted"
        );
        fs::write(&path, &preview.proposed_content).unwrap();
        store
            .conn
            .execute(
                "UPDATE events SET status = 'pending' WHERE id = 'preserved'",
                [],
            )
            .unwrap();
        assert!(settle_desktop_document_operation(
            scope,
            &store,
            "preserved",
            Err("cancelled".to_string()),
            (),
            command
        )
        .is_err());
        assert_eq!(store.get("preserved").unwrap().unwrap().status, "failed");
        assert!(store.get("forced").unwrap().unwrap().reverted_by.is_none());
        assert_eq!(fs::read_to_string(&path).unwrap(), preview.proposed_content);
        assert_eq!(fs::read(lock).unwrap(), saved_lock);
    }

    use super::super::skill_fork_registry::{
        write_fork_registry, ForkRecord, ForkRegistry, OriginTool,
    };
    use super::super::skill_md_write::{skill_md_write_transaction_is_held, write_skill_md_bytes};
    use super::*;

    fn malformed() -> &'static str {
        "---\nname: sample\ndescription: Use when: testing\n---\n# Body\n"
    }

    fn deployment(path: &Path, owner_kind: LifecycleOwnerKind) -> Deployment {
        Deployment {
            id: super::super::skill_deployment::deployment_id(
                "sample",
                "global",
                SkillDestination::Universal,
                "universal",
                None,
                path,
            ),
            path: path.to_string_lossy().into_owned(),
            scope: "global".to_string(),
            destination: SkillDestination::Universal,
            backing: BackingRelationship::Canonical,
            owner_kind,
            owner_id: Some("owner:v1/global/universal/skills-sh/sample/-".to_string()),
            mutability: DeploymentMutability::Mutable,
            ..Deployment::default()
        }
    }

    fn record_repair_intent(
        store: &EventStore,
        event_id: &str,
        deployment: &Deployment,
        mode: FrontmatterRepairApplyMode,
    ) -> FrontmatterRepairIntent {
        let preview = preview_from_deployment(deployment).unwrap();
        let skill_md = Path::new(&deployment.path).join("SKILL.md");
        let intent = FrontmatterRepairIntent {
            deployment_id: deployment.id.clone(),
            name: "sample".to_string(),
            path: PathBuf::from(&deployment.path),
            expected_content_fingerprint: preview.expected_content_fingerprint,
            proposed_content_fingerprint: content_fingerprint(preview.proposed_content.as_bytes()),
            proposed_content: preview.proposed_content,
            mode,
            managed_update_warning: false,
            fork_registry_before: (mode == FrontmatterRepairApplyMode::ForkAndFix)
                .then(ForkRegistry::default),
        };
        let pre_fingerprint = fingerprint_path(&skill_md);
        store
            .backup_paths(event_id, std::slice::from_ref(&skill_md))
            .unwrap();
        store
            .record(
                event_id,
                EventDraft {
                    kind: "repair_skill_frontmatter".to_string(),
                    skill: "sample".to_string(),
                    harness: None,
                    scope: Some("global".to_string()),
                    project_path: None,
                    payload: serde_json::to_value(&intent).unwrap(),
                    inverse: Some(
                        serde_json::to_value(InverseOp::RestoreBackup {
                            path: skill_md,
                            pre_fingerprint,
                            post_fingerprint: None,
                        })
                        .unwrap(),
                    ),
                    backup_dir: Some(format!("backups/{event_id}")),
                    restorable: true,
                },
            )
            .unwrap();
        intent
    }

    #[test]
    fn repairs_description_without_touching_body_or_crlf() {
        let input = "---\r\nname: sample\r\ndescription: Use when: testing \"quotes\"\r\nlicense: MIT\r\n---\r\n# Body\r\nbytes: stay\r\n";
        let (actual, _) = propose_colon_scalar_repair(input).unwrap();
        assert_eq!(actual, "---\r\nname: sample\r\ndescription: |-\r\n  Use when: testing \"quotes\"\r\nlicense: MIT\r\n---\r\n# Body\r\nbytes: stay\r\n");
        let FrontmatterParseResult::Valid(parsed) = parse_frontmatter(&actual) else {
            panic!("proposal did not parse")
        };
        assert_eq!(
            parsed.description.as_deref(),
            Some("Use when: testing \"quotes\"")
        );
    }

    #[test]
    fn refuses_ambiguous_and_other_yaml_failures() {
        for input in [
            "---\nname: one: two\ndescription: three: four\n---\n",
            "---\nname: [broken\ndescription: ok\n---\n",
            "---\nname: 'broken\ndescription: ok: here\n---\n",
            "---\nname: ok\ndescription: nested:\n  child: value\n---\n",
        ] {
            assert!(propose_colon_scalar_repair(input).is_err(), "{input}");
        }
    }

    #[test]
    fn repairs_name_as_one_line_with_exact_value() {
        let input = "---\nname: alpha: beta\ndescription: safe\n---\nbody\n";
        let (actual, _) = propose_colon_scalar_repair(input).unwrap();
        assert!(!actual.contains("name: |"));
        let FrontmatterParseResult::Valid(parsed) = parse_frontmatter(&actual) else {
            panic!("proposal did not parse")
        };
        assert_eq!(parsed.name.as_deref(), Some("alpha: beta"));
        assert!(actual.ends_with("---\nbody\n"));
    }

    #[test]
    fn action_policy_is_exact_for_each_owner_class() {
        let managed = Deployment {
            owner_kind: LifecycleOwnerKind::SkillsSh,
            mutability: DeploymentMutability::Mutable,
            destination: SkillDestination::Universal,
            backing: BackingRelationship::Canonical,
            scope: "global".to_string(),
            ..Deployment::default()
        };
        assert_eq!(
            apply_modes(&managed),
            vec![
                FrontmatterRepairApplyMode::ForkAndFix,
                FrontmatterRepairApplyMode::FixInstalledCopy
            ]
        );
        for owner_kind in [
            LifecycleOwnerKind::Copy,
            LifecycleOwnerKind::Fork,
            LifecycleOwnerKind::Manual,
        ] {
            assert_eq!(
                apply_modes(&Deployment {
                    owner_kind,
                    mutability: DeploymentMutability::Mutable,
                    ..Deployment::default()
                }),
                vec![FrontmatterRepairApplyMode::ApplyFix]
            );
        }
        for owner_kind in [
            LifecycleOwnerKind::Plugin,
            LifecycleOwnerKind::WildcardDotagents,
            LifecycleOwnerKind::Ambiguous,
        ] {
            assert!(apply_modes(&Deployment {
                owner_kind,
                ..Deployment::default()
            })
            .is_empty());
        }
    }

    #[test]
    fn bound_preview_refuses_stale_bytes_repoint_and_owner_change() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("sample");
        fs::create_dir_all(&first).unwrap();
        fs::write(first.join("SKILL.md"), malformed()).unwrap();
        let deployment = deployment(&first, LifecycleOwnerKind::SkillsSh);
        let preview = preview_from_deployment(&deployment).unwrap();

        fs::write(first.join("SKILL.md"), format!("{}drift", malformed())).unwrap();
        assert!(validate_bound_preview(
            &deployment,
            &preview.expected_content_fingerprint,
            &preview.proposal_id
        )
        .is_err());
        fs::write(first.join("SKILL.md"), malformed()).unwrap();

        let second = temp.path().join("other-scope/sample");
        fs::create_dir_all(&second).unwrap();
        fs::write(second.join("SKILL.md"), malformed()).unwrap();
        let mut repointed = deployment.clone();
        repointed.path = second.to_string_lossy().into_owned();
        assert!(validate_bound_preview(
            &repointed,
            &preview.expected_content_fingerprint,
            &preview.proposal_id
        )
        .is_err());

        let mut changed_owner = deployment.clone();
        changed_owner.owner_id = Some("owner:v1/global/universal/dotagents/sample/-".to_string());
        changed_owner.owner_kind = LifecycleOwnerKind::Dotagents;
        assert!(validate_bound_preview(
            &changed_owner,
            &preview.expected_content_fingerprint,
            &preview.proposal_id
        )
        .is_err());
    }

    #[test]
    fn repair_apply_validation_and_replace_hold_the_skill_md_transaction() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("sample");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), malformed()).unwrap();
        let deployment = deployment(&skill, LifecycleOwnerKind::Manual);
        let preview = preview_from_deployment(&deployment).unwrap();

        let (transaction, validated) = begin_bound_frontmatter_repair_transaction(
            &deployment,
            &preview.expected_content_fingerprint,
            &preview.proposal_id,
        )
        .unwrap();
        assert!(skill_md_write_transaction_is_held());
        transaction
            .replace_text(&skill.join("SKILL.md"), &validated.proposed_content)
            .unwrap();
        drop(transaction);

        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            preview.proposed_content
        );
    }

    #[test]
    fn direct_write_changes_only_the_exact_scope_and_keeps_managed_registry_ownership() {
        let temp = tempfile::tempdir().unwrap();
        let selected = temp.path().join("global/sample");
        let other = temp.path().join("project/sample");
        fs::create_dir_all(&selected).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(selected.join("SKILL.md"), malformed()).unwrap();
        fs::write(other.join("SKILL.md"), malformed()).unwrap();
        let deployment = deployment(&selected, LifecycleOwnerKind::SkillsSh);
        let owner_before = deployment.owner_id.clone();
        let preview = preview_from_deployment(&deployment).unwrap();
        write_skill_md_bytes(
            &selected.join("SKILL.md"),
            preview.proposed_content.as_bytes(),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(other.join("SKILL.md")).unwrap(),
            malformed()
        );
        assert_eq!(deployment.owner_id, owner_before);
    }

    #[test]
    fn atomic_failure_leaves_original_and_failed_event_is_undoable_with_drift_guard() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("sample");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), malformed()).unwrap();
        let deployment = deployment(&skill, LifecycleOwnerKind::Manual);
        let store = EventStore::open(&temp.path().join("app-data")).unwrap();
        let intent = record_repair_intent(
            &store,
            "repair",
            &deployment,
            FrontmatterRepairApplyMode::ApplyFix,
        );
        let error = finish_repair_write(
            &store,
            "repair",
            &skill.join("SKILL.md"),
            intent.proposed_content.as_bytes(),
            |_, _| Err("injected atomic failure".to_string()),
        )
        .unwrap_err();
        assert_eq!(error, "injected atomic failure");
        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            malformed()
        );

        // Complete a second event, then prove exact-byte undo and drift refusal.
        let intent = record_repair_intent(
            &store,
            "repair-2",
            &deployment,
            FrontmatterRepairApplyMode::ApplyFix,
        );
        finish_repair_write(
            &store,
            "repair-2",
            &skill.join("SKILL.md"),
            intent.proposed_content.as_bytes(),
            write_skill_md_bytes,
        )
        .unwrap();
        fs::write(skill.join("SKILL.md"), "external drift").unwrap();
        assert!(store
            .restore("repair-2", false)
            .unwrap_err()
            .contains("has changed"));
        fs::write(skill.join("SKILL.md"), intent.proposed_content).unwrap();
        store.restore("repair-2", false).unwrap();
        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            malformed()
        );
    }

    #[test]
    fn startup_finishes_exact_unchanged_fork_after_crash() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let skill = home.join(".agents/skills/sample");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), malformed()).unwrap();
        let deployment = deployment(&skill, LifecycleOwnerKind::SkillsSh);
        let store = EventStore::open(&temp.path().join("app-data")).unwrap();
        let intent = record_repair_intent(
            &store,
            "crashed",
            &deployment,
            FrontmatterRepairApplyMode::ForkAndFix,
        );
        let mut registry = ForkRegistry::default();
        registry.forks.insert(
            "sample".to_string(),
            ForkRecord {
                deployment_id: deployment.id.clone(),
                skill_dir: skill.clone(),
                forked_at: "now".to_string(),
                origin_tool: OriginTool::SkillsSh,
                origin_source: "owner/repo".to_string(),
                repo: "owner/repo".to_string(),
                path: "skills/sample".to_string(),
                declared_ref: None,
                base_commit: "abc".to_string(),
            },
        );
        write_fork_registry(&home, &registry).unwrap();
        let rows = store.reconcile_at_startup().unwrap();
        reconcile_interrupted_frontmatter_repair_with(&store, &home, &rows[0], |_| {
            assert!(skill_md_write_transaction_is_held());
        })
        .unwrap();
        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            intent.proposed_content
        );
        assert_eq!(store.list(10, Some("sample")).unwrap()[0].status, "done");
    }

    #[test]
    fn startup_refuses_repointed_fork_after_crash() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let skill = home.join(".agents/skills/sample");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), malformed()).unwrap();
        let deployment = deployment(&skill, LifecycleOwnerKind::SkillsSh);
        let store = EventStore::open(&temp.path().join("app-data")).unwrap();
        record_repair_intent(
            &store,
            "crashed",
            &deployment,
            FrontmatterRepairApplyMode::ForkAndFix,
        );
        write_fork_registry(&home, &ForkRegistry::default()).unwrap();
        let rows = store.reconcile_at_startup().unwrap();
        assert!(reconcile_interrupted_frontmatter_repair(&store, &home, &rows[0]).is_err());
        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            malformed()
        );
        assert_eq!(store.list(10, Some("sample")).unwrap()[0].status, "failed");
    }

    #[test]
    fn startup_rolls_back_fork_record_when_ledger_detach_never_finished() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let agents = home.join(".agents");
        let skill = agents.join("skills/sample");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), malformed()).unwrap();
        fs::write(
            agents.join("agents.lock"),
            "[skills.sample]\nsource = \"owner/repo\"\nresolved_path = \"skills/sample\"\n",
        )
        .unwrap();
        let deployment = deployment(&skill, LifecycleOwnerKind::Dotagents);
        let store = EventStore::open(&temp.path().join("app-data")).unwrap();
        record_repair_intent(
            &store,
            "crashed",
            &deployment,
            FrontmatterRepairApplyMode::ForkAndFix,
        );
        let mut registry = ForkRegistry::default();
        registry.forks.insert(
            "sample".to_string(),
            ForkRecord {
                deployment_id: deployment.id.clone(),
                skill_dir: skill.clone(),
                forked_at: "now".to_string(),
                origin_tool: OriginTool::Dotagents,
                origin_source: "owner/repo".to_string(),
                repo: "owner/repo".to_string(),
                path: "skills/sample".to_string(),
                declared_ref: None,
                base_commit: "abc".to_string(),
            },
        );
        write_fork_registry(&home, &registry).unwrap();
        let rows = store.reconcile_at_startup().unwrap();
        reconcile_interrupted_frontmatter_repair(&store, &home, &rows[0]).unwrap();
        assert!(super::super::skill_fork_registry::read_fork_registry(&home)
            .unwrap()
            .forks
            .is_empty());
        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            malformed()
        );
    }
}
