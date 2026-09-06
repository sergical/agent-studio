// ============================================================================
// Skills Module - deterministic malformed frontmatter repair
// Previews and applies the one safe first-version repair: an unquoted `: ` in
// a top-level name or description scalar.
// ============================================================================

use std::fs;
use std::io::Write;
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
use super::skill_ownership::LifecycleOwnerKind;
use super::skill_refresh::{self, SkillRefreshState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FrontmatterRepairApplyMode {
    ApplyFix,
    FixInstalledCopy,
    ForkAndFix,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontmatterRepairPreview {
    pub deployment_id: String,
    pub path: String,
    pub scope: String,
    pub reason: String,
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
pub fn preview_skill_frontmatter_repair(
    target: LifecycleTarget,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
) -> Result<FrontmatterRepairPreview, String> {
    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    preview_from_deployment(exact_target(&snapshot, &target)?)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("SKILL.md has no parent directory")?;
    let permissions = fs::metadata(path)
        .map_err(|error| format!("Failed to stat {}: {error}", path.display()))?
        .permissions();
    let temp = parent.join(format!(".SKILL.md.repair-{}", allocate_id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| format!("Failed to create repair file: {error}"))?;
        file.write_all(bytes)
            .map_err(|error| format!("Failed to write repair file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Failed to sync repair file: {error}"))?;
        fs::set_permissions(&temp, permissions)
            .map_err(|error| format!("Failed to preserve SKILL.md permissions: {error}"))?;
        fs::rename(&temp, path).map_err(|error| format!("Failed to replace SKILL.md: {error}"))?;
        fs::File::open(parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|error| format!("Failed to sync skill directory: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
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
    let intent: FrontmatterRepairIntent = serde_json::from_value(row.payload.clone())
        .map_err(|error| format!("Malformed frontmatter repair intent: {error}"))?;
    let skill_md = intent.path.join("SKILL.md");
    let current = fs::read(&skill_md)
        .map_err(|error| format!("Failed to read {}: {error}", skill_md.display()))?;
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
        atomic_write,
    )
}

#[tauri::command]
pub fn apply_skill_frontmatter_repair(
    request: ApplyFrontmatterRepairRequest,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
    event_store: tauri::State<EventStoreState>,
) -> Result<(), String> {
    let ApplyFrontmatterRepairRequest {
        target,
        proposal_id,
        expected_content_fingerprint,
        mode,
    } = request;
    let _guard = fork_lock.try_acquire()?;
    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    let deployment = exact_target(&snapshot, &target)?.clone();
    let preview = validate_bound_preview(&deployment, &expected_content_fingerprint, &proposal_id)?;
    if !preview.allowed_apply_modes.contains(&mode) {
        return Err("This repair mode is not allowed for the selected deployment".to_string());
    }

    let skill_md = PathBuf::from(&deployment.path).join("SKILL.md");
    let name = super::skill_deployment::parse_deployment_id(&deployment.id)
        .map(|id| id.name)
        .unwrap_or_default();
    let mut guard = event_store
        .0
        .lock()
        .map_err(|error| format!("event store lock poisoned: {error}"))?;
    let store = guard.as_mut().ok_or("Event store is unavailable")?;
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
        if let Err(error) = super::skill_fork::fork_resolved_deployment_with_real_services(
            &home,
            &app_data,
            &name,
            Path::new(&deployment.path),
        ) {
            store.finish(&event_id, EventStatus::Failed)?;
            return Err(error);
        }
        let live = fs::read(Path::new(&deployment.path).join("SKILL.md"))
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
        atomic_write(&skill_md, preview.proposed_content.as_bytes()).and_then(|()| {
            store.patch_inverse_post_fingerprint(&event_id, &fingerprint_path(&skill_md))?;
            store.finish(&event_id, EventStatus::Done)
        })
    } else {
        finish_repair_write(
            store,
            &event_id,
            &skill_md,
            preview.proposed_content.as_bytes(),
            atomic_write,
        )
    };
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

#[cfg(test)]
mod tests {
    use super::super::skill_fork_registry::{
        write_fork_registry, ForkRecord, ForkRegistry, OriginTool,
    };
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
        atomic_write(
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
            atomic_write,
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
        reconcile_interrupted_frontmatter_repair(&store, &home, &rows[0]).unwrap();
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
