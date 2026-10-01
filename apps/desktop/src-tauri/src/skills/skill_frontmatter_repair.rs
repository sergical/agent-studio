// ============================================================================
// Skills Module - deterministic malformed frontmatter repair
// Previews and applies deterministic SKILL.md repairs: an unquoted `: ` in a
// top-level name or description scalar, a name that differs from its folder,
// and conflicting invocation keys.
// ============================================================================

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::Manager;

use super::event_commands::EventStoreState;
use super::event_store::{
    allocate_id, fingerprint_path, EventDraft, EventRow, EventStatus, EventStore, InverseOp,
};
use super::skill_deployment::{BackingRelationship, DeploymentMutability, SkillDestination};
use super::skill_dto::{Deployment, LifecycleTarget};
use super::skill_md_write::{begin_skill_md_write_transaction, SkillMdWriteTransaction};
use super::skill_ownership::LifecycleOwnerKind;
use super::skill_refresh::{self, SkillRefreshState};
use skill_studio_core::frontmatter_repair::propose_repair;
pub use skill_studio_core::frontmatter_repair::{FrontmatterRepairKind, InvocationConflictChoice};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FrontmatterRepairApplyMode {
    ApplyFix,
    FixInstalledCopy,
    ForkAndFix,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FrontmatterRepairPreview {
    pub deployment_id: String,
    pub path: String,
    pub scope: String,
    pub reason: String,
    pub kind: FrontmatterRepairKind,
    /// Set once the user picked a side of an invocation conflict.
    pub choice: Option<InvocationConflictChoice>,
    pub expected_content_fingerprint: String,
    pub proposal_id: String,
    pub original_content: String,
    pub proposed_content: String,
    pub allowed_apply_modes: Vec<FrontmatterRepairApplyMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyFrontmatterRepairRequest {
    pub target: LifecycleTarget,
    pub proposal_id: String,
    pub expected_content_fingerprint: String,
    pub mode: FrontmatterRepairApplyMode,
    #[serde(default)]
    pub kind: FrontmatterRepairKind,
    #[serde(default)]
    pub choice: Option<InvocationConflictChoice>,
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
    // One `write!` per byte into a pre-sized `String`, rather than collecting
    // a `Vec<String>` of two-char fragments.
    let hex = digest
        .iter()
        .fold(String::with_capacity(digest.len() * 2), |mut acc, byte| {
            let _ = write!(acc, "{byte:02x}");
            acc
        });
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

// `propose_colon_scalar_repair` now lives in `skill_studio_core::
// frontmatter_repair`, ported byte-for-byte from what was here; this module
// is a thin adapter that re-imports it below rather than keeping its own
// copy. The apply-mode/fork/journaling machinery below it (`FixInstalledCopy`
// and `ForkAndFix`) stays desktop-only: core's own `preview_frontmatter_repair`
// doc comment says it supports only `ApplyFix` today.
pub use skill_studio_core::frontmatter_repair::propose_colon_scalar_repair;

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

/// The real folder name a name fix writes into `SKILL.md`. The scanner checks
/// each deployment against its own folder name, so a link under another name
/// must not write its own name into the shared file. Other deployments of the
/// same file are guarded by `refuse_name_fix_with_differently_named_peer`.
fn repair_folder_name(
    deployment_path: &Path,
    kind: FrontmatterRepairKind,
) -> Result<String, String> {
    let resolved = fs::canonicalize(deployment_path)
        .map_err(|error| format!("Failed to resolve {}: {error}", deployment_path.display()))?;
    let resolved_name = resolved
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Skill folder name is not UTF-8")?;
    let is_symlink = fs::symlink_metadata(deployment_path)
        .map_err(|error| format!("Failed to inspect {}: {error}", deployment_path.display()))?
        .file_type()
        .is_symlink();
    let link_name = deployment_path.file_name().and_then(|name| name.to_str());
    let is_name_fix = matches!(
        kind,
        FrontmatterRepairKind::NameMismatch | FrontmatterRepairKind::NameFormat
    );
    if is_name_fix && is_symlink && link_name != Some(resolved_name) {
        return Err(format!(
            "This skill is linked under a different folder name than its real folder \"{resolved_name}\". Fix the name from the real folder instead."
        ));
    }
    Ok(resolved_name.to_string())
}

/// A name fix on a real folder moves the mismatch to any other deployment
/// that reaches the same `SKILL.md` under a different folder name, so it is
/// refused while one exists.
fn refuse_name_fix_with_differently_named_peer(
    snapshot: &skill_refresh::SkillSnapshot,
    deployment: &Deployment,
    kind: FrontmatterRepairKind,
) -> Result<(), String> {
    if !matches!(
        kind,
        FrontmatterRepairKind::NameMismatch | FrontmatterRepairKind::NameFormat
    ) {
        return Ok(());
    }
    let Ok(real) = fs::canonicalize(&deployment.path) else {
        return Ok(());
    };
    let real_name = real.file_name();
    for other in snapshot.skills.iter().flat_map(|skill| &skill.deployments) {
        if other.id == deployment.id {
            continue;
        }
        let same_real = fs::canonicalize(&other.path).is_ok_and(|resolved| resolved == real);
        if same_real && Path::new(&other.path).file_name() != real_name {
            return Err(format!(
                "Another deployment at {} reaches this SKILL.md under a different folder name, so fixing the name here would break it. Rename or remove that link first.",
                other.path
            ));
        }
    }
    Ok(())
}

fn preview_from_deployment(
    deployment: &Deployment,
    kind: FrontmatterRepairKind,
    choice: Option<InvocationConflictChoice>,
) -> Result<FrontmatterRepairPreview, String> {
    let path = Path::new(&deployment.path).join("SKILL.md");
    let bytes =
        fs::read(&path).map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
    let original =
        String::from_utf8(bytes.clone()).map_err(|_| "SKILL.md is not UTF-8".to_string())?;
    let dir_name = repair_folder_name(Path::new(&deployment.path), kind)?;
    let (proposed, reason) = propose_repair(kind, &original, &dir_name, choice)?;
    let fingerprint = content_fingerprint(&bytes);
    Ok(FrontmatterRepairPreview {
        deployment_id: deployment.id.clone(),
        path: deployment.path.clone(),
        scope: deployment.scope.clone(),
        reason,
        kind,
        choice,
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
    kind: FrontmatterRepairKind,
    choice: Option<InvocationConflictChoice>,
) -> Result<FrontmatterRepairPreview, String> {
    let preview = preview_from_deployment(deployment, kind, choice)?;
    if preview.expected_content_fingerprint != expected_content_fingerprint
        || preview.proposal_id != expected_proposal_id
    {
        return Err(
            "YAML repair refused: the deployment, ownership, or content changed".to_string(),
        );
    }
    if preview.proposed_content == preview.original_content {
        return Err("Choose an option before applying this fix".to_string());
    }
    Ok(preview)
}

fn begin_bound_frontmatter_repair_transaction(
    deployment: &Deployment,
    expected_content_fingerprint: &str,
    expected_proposal_id: &str,
    kind: FrontmatterRepairKind,
    choice: Option<InvocationConflictChoice>,
) -> Result<(SkillMdWriteTransaction, FrontmatterRepairPreview), String> {
    let transaction = begin_skill_md_write_transaction()?;
    let preview = validate_bound_preview(
        deployment,
        expected_content_fingerprint,
        expected_proposal_id,
        kind,
        choice,
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

/// Previews from the published snapshot, never a rescan: the apply step
/// re-checks the file's fingerprint, so a stale snapshot cannot write a wrong
/// file, and a full rebuild per call took minutes when the page re-asked.
fn preview_from_cached_snapshot(
    refresh_state: &SkillRefreshState,
    target: &LifecycleTarget,
    kind: FrontmatterRepairKind,
    choice: Option<InvocationConflictChoice>,
) -> Result<FrontmatterRepairPreview, String> {
    let deployment = {
        let guard = refresh_state
            .snapshot
            .read()
            .map_err(|_| "Skill snapshot is unavailable".to_string())?;
        let snapshot = guard
            .as_ref()
            .ok_or("Skills are still loading; no snapshot to preview from")?;
        let deployment = exact_target(snapshot, target)?;
        refuse_name_fix_with_differently_named_peer(snapshot, deployment, kind)?;
        deployment.clone()
    };
    preview_from_deployment(&deployment, kind, choice)
}

#[tauri::command]
pub async fn preview_skill_frontmatter_repair(
    target: LifecycleTarget,
    kind: Option<FrontmatterRepairKind>,
    choice: Option<InvocationConflictChoice>,
    app: tauri::AppHandle,
) -> Result<FrontmatterRepairPreview, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(
        &timing_app,
        "preview_skill_frontmatter_repair",
        move || {
            preview_from_cached_snapshot(
                &app.state::<SkillRefreshState>(),
                &target,
                kind.unwrap_or_default(),
                choice,
            )
        },
    )
    .await
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
    let fs = skill_studio_host::RealFs::new();
    let skills_sh = skill_studio_core::lock_file::read_lock_file(
        &fs,
        &skill_studio_core::lock_file::lock_file_path_in(&agents_dir),
    )
    .map_err(|e| e.to_string())?
    .skills
    .contains_key(name);
    let dotagents = skill_studio_core::dotagents_ledger::read_dotagents_ledger(&fs, &agents_dir)
        .map_err(|e| e.to_string())?
        .iter()
        .any(|skill| skill.name == name);
    Ok(skills_sh || dotagents)
}

fn roll_back_incomplete_fork(
    store: &EventStore,
    home: &Path,
    row: &EventRow,
    intent: &FrontmatterRepairIntent,
    guard: Option<&super::write_lease::WriteLeaseGuard>,
) -> Result<(), String> {
    let registry = intent
        .fork_registry_before
        .as_ref()
        .ok_or("Fork repair intent has no ownership rollback snapshot")?;
    super::skill_fork_registry::write_fork_registry_maybe_locked(guard, home, registry)?;
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
    guard: Option<&super::write_lease::WriteLeaseGuard>,
) -> Result<(), String> {
    reconcile_interrupted_frontmatter_repair_with(store, home, row, guard, |_| {})
}

fn reconcile_interrupted_frontmatter_repair_with(
    store: &EventStore,
    home: &Path,
    row: &EventRow,
    guard: Option<&super::write_lease::WriteLeaseGuard>,
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
        roll_back_incomplete_fork(store, home, row, &intent, guard)?;
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
) -> Result<(), String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(
        &timing_app,
        "apply_skill_frontmatter_repair",
        move || {
            let refresh_state = app.state::<SkillRefreshState>();
            let event_store = app.state::<EventStoreState>();
            let ApplyFrontmatterRepairRequest {
                target,
                proposal_id,
                expected_content_fingerprint,
                mode,
                kind,
                choice,
            } = request;
            let home = dirs::home_dir().ok_or("Could not find home directory")?;
            let write_lease = super::write_lease::WriteLease::default();
            let write_guard = write_lease.try_acquire(&home)?;
            let snapshot =
                super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
            let deployment = exact_target(&snapshot, &target)?.clone();
            refuse_name_fix_with_differently_named_peer(&snapshot, &deployment, kind)?;
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
                kind,
                choice,
            )?;
            if !preview.allowed_apply_modes.contains(&mode) {
                return Err(
                    "This repair mode is not allowed for the selected deployment".to_string(),
                );
            }
            let event_id = allocate_id();
            let pre_fingerprint = fingerprint_path(&skill_md);
            let intent = FrontmatterRepairIntent {
                deployment_id: deployment.id.clone(),
                name: name.clone(),
                path: PathBuf::from(&deployment.path),
                expected_content_fingerprint: expected_content_fingerprint.clone(),
                proposed_content_fingerprint: content_fingerprint(
                    preview.proposed_content.as_bytes(),
                ),
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
            store.backup_paths(&event_id, std::slice::from_ref(&skill_md))?;
            store.record(
                &event_id,
                &EventDraft {
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
                if let Err(error) = super::skill_fork::fork_resolved_deployment_with_real_services(
                    &write_guard,
                    &home,
                    &app_data,
                    &name,
                    Path::new(&deployment.path),
                ) {
                    store.finish(&event_id, EventStatus::Failed)?;
                    return Err(error);
                }
                let live = transaction.read(&skill_md).map_err(|error| {
                    format!("Fork completed, but the repair needs recovery: {error}")
                })?;
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
                        store.patch_inverse_post_fingerprint(
                            &event_id,
                            &fingerprint_path(&skill_md),
                        )?;
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
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::super::frontmatter::{parse_frontmatter, FrontmatterParseResult};
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

    fn snapshot_with(deployment: Deployment) -> skill_refresh::SkillSnapshot {
        use super::super::frontmatter::InvocationPolicy;
        use super::super::skill_dto::InstalledSkill;
        use super::super::SourceKind;
        skill_refresh::SkillSnapshot {
            revision: 1,
            skills: vec![InstalledSkill {
                name: "sample".to_string(),
                source: "manual".to_string(),
                source_type: "manual".to_string(),
                source_url: None,
                skill_path: None,
                installed_at: "2024-01-01T00:00:00Z".to_string(),
                updated_at: None,
                has_update: false,
                update_owner_ids: Vec::new(),
                update_owners: Vec::new(),
                update_commit: None,
                update_commit_at: None,
                source_kind: SourceKind::Manual,
                deployments: vec![deployment],
                has_spec: false,
                description: None,
                spec_violations: Vec::new(),
                skill_md_tokens: 0,
                description_tokens: 0,
                folder_bytes: 0,
                file_count: 0,
                content_hash: String::new(),
                content_hashes: Vec::new(),
                modified_at: None,
                frontmatter_fields: Default::default(),
                folder_truncated: false,
                fork: None,
                parked: false,
                parked_at: None,
                invocation: InvocationPolicy::Both,
            }],
            projects: Vec::new(),
            invocations: Vec::new(),
            heatmap: skill_studio_core::skill_uses::InvocationHeatmap::default(),
            scanned_at: "2024-01-01T00:00:00Z".to_string(),
            last_test_by_skill: Default::default(),
            update_check: Default::default(),
            opencode_config_kind: None,
            scan_partial: false,
            scan_observations: Vec::new(),
            unread_roots: Vec::new(),
        }
    }

    fn target_for(deployment: &Deployment) -> LifecycleTarget {
        LifecycleTarget {
            deployment_id: Some(deployment.id.clone()),
            owner_id: None,
        }
    }

    /// Flow: the page asks for a preview while the refresh state holds a snapshot
    /// whose content hash is stale. Expect: the preview is built from the bytes
    /// now on disk, with no app handle and no rebuild. Failure: the preview
    /// needs a full rescan again (minutes per call) or shows stale content.
    #[test]
    fn preview_reads_disk_bytes_through_the_cached_snapshot_without_a_rescan() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("sample");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), malformed()).unwrap();
        let dep = deployment(&dir, LifecycleOwnerKind::Manual);
        let state = SkillRefreshState::fixture(snapshot_with(dep.clone()));

        let preview = preview_from_cached_snapshot(
            &state,
            &target_for(&dep),
            FrontmatterRepairKind::ColonScalar,
            None,
        )
        .unwrap();

        assert_eq!(preview.original_content, malformed());
        assert_eq!(
            preview.expected_content_fingerprint,
            content_fingerprint(malformed().as_bytes())
        );
        assert_eq!(
            preview.allowed_apply_modes,
            vec![FrontmatterRepairApplyMode::ApplyFix]
        );
    }

    /// Flow: a link named `sample` points at the real folder `sample-real`.
    /// Expect: the folder name used is the resolved `sample-real`. Failure: the
    /// code takes the link's own basename and writes the wrong name.
    #[cfg(unix)]
    #[test]
    fn repair_folder_name_uses_the_resolved_folder_not_the_link_path() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("sample-real");
        fs::create_dir_all(&real).unwrap();
        let link = temp.path().join("sample");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let name = repair_folder_name(&link, FrontmatterRepairKind::ColonScalar).unwrap();

        assert_eq!(name, "sample-real");
    }

    fn real_folder_with_link(link_name: &str) -> (tempfile::TempDir, Deployment, Deployment) {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("foo");
        fs::create_dir_all(&real).unwrap();
        fs::write(
            real.join("SKILL.md"),
            "---\nname: bar\ndescription: d\n---\n",
        )
        .unwrap();
        let links = temp.path().join("links");
        fs::create_dir_all(&links).unwrap();
        let link = links.join(link_name);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let real_dep = deployment(&real, LifecycleOwnerKind::Manual);
        let link_dep = deployment(&link, LifecycleOwnerKind::Manual);
        (temp, real_dep, link_dep)
    }

    /// Flow: real folder `foo` is also linked as `bar`, and the user fixes the
    /// name on `foo`. Expect: refused, naming the `bar` link. Failure: `foo`
    /// is written and the mismatch moves to the `bar` link.
    #[cfg(unix)]
    #[test]
    fn name_fix_is_refused_when_another_deployment_links_the_folder_under_a_different_name() {
        let (_temp, real_dep, link_dep) = real_folder_with_link("bar");
        let mut snapshot = snapshot_with(real_dep.clone());
        snapshot.skills[0].deployments.push(link_dep.clone());

        let error = refuse_name_fix_with_differently_named_peer(
            &snapshot,
            &real_dep,
            FrontmatterRepairKind::NameMismatch,
        )
        .unwrap_err();

        assert!(error.contains(&link_dep.path), "{error}");
    }

    /// Flow: real folder `foo` is also linked under the same name `foo`.
    /// Expect: the name fix is allowed. Failure: harmless same-named links
    /// block the fix.
    #[cfg(unix)]
    #[test]
    fn name_fix_is_allowed_when_another_deployment_links_the_folder_under_the_same_name() {
        let (_temp, real_dep, link_dep) = real_folder_with_link("foo");
        let mut snapshot = snapshot_with(real_dep.clone());
        snapshot.skills[0].deployments.push(link_dep);

        refuse_name_fix_with_differently_named_peer(
            &snapshot,
            &real_dep,
            FrontmatterRepairKind::NameMismatch,
        )
        .unwrap();
    }

    /// Flow: a link whose basename differs from its real folder gets a name
    /// fix. Expect: refused with a message about the linked folder name.
    /// Failure: the alias name is written into the shared SKILL.md.
    #[cfg(unix)]
    #[test]
    fn name_fix_through_a_differently_named_symlink_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real/sample");
        fs::create_dir_all(&real).unwrap();
        fs::write(
            real.join("SKILL.md"),
            "---\nname: Other Name\ndescription: d\n---\n",
        )
        .unwrap();
        let link = temp.path().join("alias");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let dep = deployment(&link, LifecycleOwnerKind::Manual);

        for kind in [
            FrontmatterRepairKind::NameMismatch,
            FrontmatterRepairKind::NameFormat,
        ] {
            let error = preview_from_deployment(&dep, kind, None).unwrap_err();
            assert!(
                error.contains("linked under a different folder name"),
                "{error}"
            );
        }
    }

    /// Flow: the snapshot is not published yet (app just started). Expect: an
    /// error the page treats as "no repair". Failure: the call blocks on a
    /// rebuild or panics on the empty slot.
    #[test]
    fn preview_errors_when_no_snapshot_is_published() {
        let temp = tempfile::tempdir().unwrap();
        let dep = deployment(temp.path(), LifecycleOwnerKind::Manual);
        let state = SkillRefreshState::fixture(snapshot_with(dep.clone()));
        *state.snapshot.write().unwrap() = None;

        let error = preview_from_cached_snapshot(
            &state,
            &target_for(&dep),
            FrontmatterRepairKind::ColonScalar,
            None,
        )
        .unwrap_err();

        assert!(error.contains("no snapshot"), "{error}");
    }

    /// Flow: the target id is not in the cached snapshot. Expect: an error, not
    /// a fallback rescan. Failure: an unknown deployment triggers a rebuild.
    #[test]
    fn preview_errors_for_a_deployment_missing_from_the_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let dep = deployment(temp.path(), LifecycleOwnerKind::Manual);
        let other = deployment(&temp.path().join("other"), LifecycleOwnerKind::Manual);
        let state = SkillRefreshState::fixture(snapshot_with(dep));

        let error = preview_from_cached_snapshot(
            &state,
            &target_for(&other),
            FrontmatterRepairKind::ColonScalar,
            None,
        )
        .unwrap_err();

        assert!(error.contains("not in the current snapshot"), "{error}");
    }

    fn record_repair_intent(
        store: &EventStore,
        event_id: &str,
        deployment: &Deployment,
        mode: FrontmatterRepairApplyMode,
    ) -> FrontmatterRepairIntent {
        record_repair_intent_for(
            store,
            event_id,
            deployment,
            mode,
            FrontmatterRepairKind::ColonScalar,
            None,
        )
    }

    fn record_repair_intent_for(
        store: &EventStore,
        event_id: &str,
        deployment: &Deployment,
        mode: FrontmatterRepairApplyMode,
        kind: FrontmatterRepairKind,
        choice: Option<InvocationConflictChoice>,
    ) -> FrontmatterRepairIntent {
        let preview = preview_from_deployment(deployment, kind, choice).unwrap();
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
                &EventDraft {
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

    const NAME_MISMATCH: &str = "---\r\nname: other\r\ndescription: d\r\n---\r\n# Body\r\n";
    const NAME_FORMAT: &str = "---\nname: Sample Skill\ndescription: d\n---\n# Body\n";
    const CONFLICT: &str = "---\nname: sample\ndescription: d\ndisable-model-invocation: true\nuser-invocable: false\n---\n# Body\n";

    fn sample_kinds() -> [(
        FrontmatterRepairKind,
        Option<InvocationConflictChoice>,
        &'static str,
        &'static str,
    ); 4] {
        [
            (
                FrontmatterRepairKind::NameMismatch,
                None,
                NAME_MISMATCH,
                "---\r\nname: sample\r\ndescription: d\r\n---\r\n# Body\r\n",
            ),
            (
                FrontmatterRepairKind::NameFormat,
                None,
                NAME_FORMAT,
                "---\nname: sample\ndescription: d\n---\n# Body\n",
            ),
            (
                FrontmatterRepairKind::InvocationConflict,
                Some(InvocationConflictChoice::UserOnly),
                CONFLICT,
                "---\nname: sample\ndescription: d\ndisable-model-invocation: true\n---\n# Body\n",
            ),
            (
                FrontmatterRepairKind::InvocationConflict,
                Some(InvocationConflictChoice::ModelOnly),
                CONFLICT,
                "---\nname: sample\ndescription: d\nuser-invocable: false\n---\n# Body\n",
            ),
        ]
    }

    /// Flow: apply each new repair kind through the bound transaction. Expect:
    /// the file holds exactly the proposed bytes. Failure: a kind or choice is
    /// dropped between preview and apply, so the wrong bytes are written.
    #[test]
    fn apply_writes_the_exact_proposal_for_each_new_kind() {
        for (kind, choice, original, expected) in sample_kinds() {
            let temp = tempfile::tempdir().unwrap();
            let skill = temp.path().join("sample");
            fs::create_dir_all(&skill).unwrap();
            fs::write(skill.join("SKILL.md"), original).unwrap();
            let deployment = deployment(&skill, LifecycleOwnerKind::Manual);
            let preview = preview_from_deployment(&deployment, kind, choice).unwrap();

            let (transaction, validated) = begin_bound_frontmatter_repair_transaction(
                &deployment,
                &preview.expected_content_fingerprint,
                &preview.proposal_id,
                kind,
                choice,
            )
            .unwrap();
            transaction
                .replace_text(&skill.join("SKILL.md"), &validated.proposed_content)
                .unwrap();
            drop(transaction);

            assert_eq!(
                fs::read_to_string(skill.join("SKILL.md")).unwrap(),
                expected,
                "{kind:?} {choice:?}"
            );
        }
    }

    /// Flow: the user previews one side of the conflict, then applies with the
    /// other. Expect: refused. Failure: the apply writes content the user never
    /// previewed.
    #[test]
    fn apply_refuses_a_proposal_bound_to_a_different_choice() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("sample");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), CONFLICT).unwrap();
        let deployment = deployment(&skill, LifecycleOwnerKind::Manual);
        let kind = FrontmatterRepairKind::InvocationConflict;
        let preview =
            preview_from_deployment(&deployment, kind, Some(InvocationConflictChoice::UserOnly))
                .unwrap();

        let error = validate_bound_preview(
            &deployment,
            &preview.expected_content_fingerprint,
            &preview.proposal_id,
            kind,
            Some(InvocationConflictChoice::ModelOnly),
        )
        .unwrap_err();

        assert!(error.contains("refused"), "{error}");
    }

    /// Flow: apply is requested for a conflict before the user picked a side.
    /// Expect: refused. Failure: the unchanged file is written and a "fixed"
    /// event is journaled.
    #[test]
    fn apply_refuses_a_conflict_with_no_choice() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("sample");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), CONFLICT).unwrap();
        let deployment = deployment(&skill, LifecycleOwnerKind::Manual);
        let kind = FrontmatterRepairKind::InvocationConflict;
        let preview = preview_from_deployment(&deployment, kind, None).unwrap();
        assert_eq!(preview.proposed_content, CONFLICT);

        let error = validate_bound_preview(
            &deployment,
            &preview.expected_content_fingerprint,
            &preview.proposal_id,
            kind,
            None,
        )
        .unwrap_err();

        assert!(error.contains("Choose an option"), "{error}");
    }

    /// Flow: each new repair is applied and journaled, then undone. Expect: the
    /// inverse restores the original bytes exactly (including CRLF). Failure:
    /// Activity undo leaves the repaired file or normalises line endings.
    #[test]
    fn undo_restores_the_original_bytes_for_each_new_kind() {
        for (kind, choice, original, expected) in sample_kinds() {
            let temp = tempfile::tempdir().unwrap();
            let skill = temp.path().join("sample");
            fs::create_dir_all(&skill).unwrap();
            fs::write(skill.join("SKILL.md"), original).unwrap();
            let deployment = deployment(&skill, LifecycleOwnerKind::Manual);
            let store = EventStore::open(&temp.path().join("app-data")).unwrap();
            let intent = record_repair_intent_for(
                &store,
                "repair",
                &deployment,
                FrontmatterRepairApplyMode::ApplyFix,
                kind,
                choice,
            );
            finish_repair_write(
                &store,
                "repair",
                &skill.join("SKILL.md"),
                intent.proposed_content.as_bytes(),
                write_skill_md_bytes,
            )
            .unwrap();
            assert_eq!(
                fs::read_to_string(skill.join("SKILL.md")).unwrap(),
                expected
            );

            store.restore("repair", false).unwrap();

            assert_eq!(
                fs::read(skill.join("SKILL.md")).unwrap(),
                original.as_bytes(),
                "{kind:?} {choice:?}"
            );
        }
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
        let preview =
            preview_from_deployment(&deployment, FrontmatterRepairKind::ColonScalar, None).unwrap();

        fs::write(first.join("SKILL.md"), format!("{}drift", malformed())).unwrap();
        assert!(validate_bound_preview(
            &deployment,
            &preview.expected_content_fingerprint,
            &preview.proposal_id,
            FrontmatterRepairKind::ColonScalar,
            None
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
            &preview.proposal_id,
            FrontmatterRepairKind::ColonScalar,
            None
        )
        .is_err());

        let mut changed_owner = deployment.clone();
        changed_owner.owner_id = Some("owner:v1/global/universal/dotagents/sample/-".to_string());
        changed_owner.owner_kind = LifecycleOwnerKind::Dotagents;
        assert!(validate_bound_preview(
            &changed_owner,
            &preview.expected_content_fingerprint,
            &preview.proposal_id,
            FrontmatterRepairKind::ColonScalar,
            None
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
        let preview =
            preview_from_deployment(&deployment, FrontmatterRepairKind::ColonScalar, None).unwrap();

        let (transaction, validated) = begin_bound_frontmatter_repair_transaction(
            &deployment,
            &preview.expected_content_fingerprint,
            &preview.proposal_id,
            FrontmatterRepairKind::ColonScalar,
            None,
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
        let preview =
            preview_from_deployment(&deployment, FrontmatterRepairKind::ColonScalar, None).unwrap();
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
        reconcile_interrupted_frontmatter_repair_with(&store, &home, &rows[0], None, |_| {
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
        assert!(reconcile_interrupted_frontmatter_repair(&store, &home, &rows[0], None).is_err());
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
        reconcile_interrupted_frontmatter_repair(&store, &home, &rows[0], None).unwrap();
        assert!(super::super::skill_fork_registry::read_fork_registry(&home)
            .unwrap()
            .forks
            .is_empty());
        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            malformed()
        );
    }

    /// Regression for the second fix-round BLOCKER: `lib.rs`'s startup pass
    /// holds `home`'s `WriteLease` across this whole recovery loop, then
    /// calls this function - which used to call the unlocked
    /// `write_fork_registry` from inside `roll_back_incomplete_fork`, taking
    /// a second, conflicting lease on the same root. Passing the held guard
    /// through (`Some(&guard)`) is the fix; this test holds the same lease
    /// `reconcile_event_store_at_startup` would, so it is red without it.
    #[test]
    fn rolling_back_a_fork_while_the_startup_lease_is_held_does_not_self_deadlock() {
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

        // The exact shape `reconcile_event_store_at_startup` takes: one
        // lease over `home` held for the whole recovery pass. Must be the
        // real `WriteLease::default()`, not a test-scoped lease root -
        // `write_fork_registry`'s own internal lock (what the fix routes
        // around via `write_fork_registry_locked`) always uses the real
        // `core_runtime::data_root()/leases`, so only the real lease root
        // can reproduce the two locks conflicting.
        let write_lease = super::super::write_lease::WriteLease::default();
        let guard = write_lease.try_acquire(&home).unwrap();
        reconcile_interrupted_frontmatter_repair(&store, &home, &rows[0], Some(&guard)).unwrap();

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
