// ============================================================================
// Skills Module - skill_trial
// "Try for 24 hours" installs (see `skill_add`'s `trial` flag). Records a
// `TrialRecord` in `skill_fork_registry`'s `trials` map on a successful
// `add_skill`; a background loop removes anything past its `expires_at`,
// trashing the folder first so an expiry can never lose data even if the
// owning tool's removal fails. `keep_skill_trial` just drops the record;
// `restore_trashed_skill` copies a trashed folder back as a plain
// (untracked) skill.
// ============================================================================

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

use super::agents::AgentId;
use super::event_store::fingerprint_path;
use super::skill_add::{maybe_claude_code_symlink, CommandRunner, RealCommandRunner};
use super::skill_dto::LifecycleTarget;
use super::skill_fork::ForkMutationLock;
use super::skill_fork_registry::{
    deployment_trial_key, name_from_trial_key, read_fork_registry, trial_key, write_fork_registry,
    AddMethod, ForkRegistry, TrialRecord, TrialScope, TrialStatus,
};
use super::skill_lifecycle::{
    find_deployment, revalidate_deployment, skills_sh_remove_args_for_scope,
};
use super::skill_refresh::{self, SkillRefreshState};

/// One trial that just expired, for the `skills://trial-expired` event.
pub struct ExpiredTrial {
    pub name: String,
    pub trash_path: String,
}

/// Records a 24 h trial for `name`, started at `now`, keyed by `scope` so
/// the same name can be on trial globally and in a project at once.
/// `skill_dir`/`claude_link` are the exact paths `add_skill` just created,
/// so expiry removes precisely those instead of recomputing them (which was
/// wrong for `skills-sh` trials, since that method never writes the shared
/// `.agents/skills` folder).
#[allow(clippy::too_many_arguments)]
pub fn record_trial(
    home: &Path,
    deployment_id: &str,
    scope: TrialScope,
    project_path: Option<&str>,
    method: AddMethod,
    skill_dir: PathBuf,
    claude_link: Option<PathBuf>,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let mut registry = read_fork_registry(home)?;
    let claude_link_target = claude_link
        .as_deref()
        .map(fs::read_link)
        .transpose()
        .map_err(|error| format!("Failed to read trial Claude link: {error}"))?;
    let deployment_fingerprint = fingerprint_path(&skill_dir);
    registry.trials.insert(
        deployment_trial_key(deployment_id),
        TrialRecord {
            deployment_id: deployment_id.to_string(),
            started_at: now.to_rfc3339(),
            expires_at: (now + chrono::Duration::hours(24)).to_rfc3339(),
            status: TrialStatus::Active,
            method,
            scope,
            project_path: project_path.map(String::from),
            skill_dir,
            deployment_fingerprint,
            claude_link,
            claude_link_target,
        },
    );
    write_fork_registry(home, &registry)
}

/// Drops `name`'s trial record for `scope`, if any - called by
/// `remove_skill`, `unfork_skill`, and `fork_skill` so a removed, un-forked,
/// or forked skill never leaves a stale trial behind it.
pub fn drop_trial_record(
    home: &Path,
    deployment_id: &str,
    name: &str,
    scope: TrialScope,
    skill_dir: &Path,
) -> Result<(), String> {
    let mut registry = read_fork_registry(home)?;
    let new_key = deployment_trial_key(deployment_id);
    let removed_new = registry.trials.remove(&new_key).is_some();
    let legacy_key = trial_key(scope, name);
    let removed_legacy = registry
        .trials
        .get(&legacy_key)
        .is_some_and(|trial| trial.skill_dir == skill_dir)
        && registry.trials.remove(&legacy_key).is_some();
    if !removed_new && !removed_legacy {
        return Ok(());
    }
    write_fork_registry(home, &registry)
}

fn drop_recovery_trial_without_deployment(
    home: &Path,
    deployment_id: &str,
) -> Result<bool, String> {
    let mut registry = read_fork_registry(home)?;
    let key = deployment_trial_key(deployment_id);
    if !registry
        .trials
        .get(&key)
        .is_some_and(|trial| trial.status == TrialStatus::RecoveryRequired)
    {
        return Ok(false);
    }
    registry.trials.remove(&key);
    write_fork_registry(home, &registry)?;
    Ok(true)
}

/// Like `copy_dir_all`, but recreates symlinks (`read_link` + `symlink`)
/// instead of skipping them. A skill folder that itself contains symlinked
/// files or subdirectories must not lose them on the way into the trash -
/// the "trash copy always happens first" guarantee is only real if the
/// trash copy is complete.
pub(crate) fn copy_dir_preserving_symlinks(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("Failed to create {}: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("Failed to read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("Failed to read a directory entry: {e}"))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("Failed to stat {}: {e}", entry.path().display()))?;
        let dest_path = dst.join(entry.file_name());
        if file_type.is_symlink() {
            let target = fs::read_link(entry.path())
                .map_err(|e| format!("Failed to read symlink {}: {e}", entry.path().display()))?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &dest_path)
                .map_err(|e| format!("Failed to symlink {}: {e}", dest_path.display()))?;
            #[cfg(not(unix))]
            return Err("Symlinking is only supported on Unix".to_string());
        } else if file_type.is_dir() {
            copy_dir_preserving_symlinks(&entry.path(), &dest_path)?;
        } else {
            fs::copy(entry.path(), &dest_path)
                .map_err(|e| format!("Failed to copy {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Total number of filesystem entries (files, dirs, symlinks) under `dir`,
/// recursively - lets `expire_one` verify a trash copy is complete before
/// removing the original, without diffing file contents.
fn count_entries(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut count = 0;
    for entry in entries.filter_map(|e| e.ok()) {
        count += 1;
        let is_real_dir = entry
            .file_type()
            .map(|t| t.is_dir() && !t.is_symlink())
            .unwrap_or(false);
        if is_real_dir {
            count += count_entries(&entry.path());
        }
    }
    count
}

fn validate_trial_deployment<'a>(
    snapshot: &'a super::skill_refresh::SkillSnapshot,
    trial: &TrialRecord,
) -> Result<
    (
        &'a super::skill_dto::InstalledSkill,
        &'a super::skill_dto::Deployment,
    ),
    String,
> {
    if trial.deployment_id.is_empty() || trial.deployment_fingerprint.is_empty() {
        return Err(
            "Legacy trial has no exact deployment identity; keeping it for manual review"
                .to_string(),
        );
    }
    let (skill, deployment) = find_deployment(snapshot, &trial.deployment_id)?;
    revalidate_deployment(deployment, &trial.deployment_id)?;
    if Path::new(&deployment.path) != trial.skill_dir {
        return Err("Trial deployment path changed; no files were removed".to_string());
    }
    let expected_scope = match trial.scope {
        TrialScope::Global => "global",
        TrialScope::Project => "project",
    };
    if deployment.scope != expected_scope || deployment.project_path != trial.project_path {
        return Err(
            "Trial deployment scope or project owner changed; no files were removed".to_string(),
        );
    }
    let owner_matches = matches!(
        (trial.method, deployment.owner_kind),
        (
            AddMethod::Dotagents,
            super::skill_ownership::LifecycleOwnerKind::Dotagents
        ) | (
            AddMethod::SkillsSh,
            super::skill_ownership::LifecycleOwnerKind::SkillsSh
        ) | (
            AddMethod::Copy,
            super::skill_ownership::LifecycleOwnerKind::Copy
        )
    );
    if !owner_matches {
        return Err("Trial deployment owner changed; no files were removed".to_string());
    }
    if fingerprint_path(&trial.skill_dir) != trial.deployment_fingerprint {
        return Err("Trial deployment content changed; no files were removed".to_string());
    }

    if let Some(link) = &trial.claude_link {
        let expected_raw_target = trial
            .claude_link_target
            .as_deref()
            .ok_or("Legacy trial has no Claude link identity; keeping it for manual review")?;
        let metadata = fs::symlink_metadata(link)
            .map_err(|error| format!("Trial Claude link cannot be verified: {error}"))?;
        let raw_target_matches = fs::read_link(link)
            .map(|target| target == expected_raw_target)
            .unwrap_or(false);
        if !metadata.file_type().is_symlink() || !raw_target_matches {
            return Err(
                "Trial Claude link was replaced or repointed; no files were removed".to_string(),
            );
        }
        let actual = fs::canonicalize(link)
            .map_err(|error| format!("Trial Claude link cannot be resolved: {error}"))?;
        let expected = fs::canonicalize(&trial.skill_dir)
            .map_err(|error| format!("Trial deployment cannot be resolved: {error}"))?;
        if actual != expected {
            return Err(
                "Trial Claude link was replaced or repointed; no files were removed".to_string(),
            );
        }
        let linked_in_snapshot = skill.deployments.iter().any(|candidate| {
            Path::new(&candidate.path) == link
                && candidate.scope == deployment.scope
                && candidate.project_path == deployment.project_path
                && matches!(
                    &candidate.backing,
                    super::skill_deployment::BackingRelationship::LinkedTo { deployment_id }
                        if deployment_id == &deployment.id
                )
        });
        if !linked_in_snapshot {
            return Err("Trial Claude link is not backed by the selected deployment in the current snapshot"
                .to_string());
        }
    }
    Ok((skill, deployment))
}

/// Expires one trial: trash-copies the folder, then removes it via the
/// owning tool (or deletes it directly for "Copy"). The trash copy always
/// happens first, so a failing removal never loses the skill - the caller
/// keeps the trial record and retries next tick.
fn expire_one(
    home: &Path,
    name: &str,
    trial: &TrialRecord,
    now: DateTime<Utc>,
    runner: &dyn CommandRunner,
) -> Result<String, String> {
    let is_project = trial.scope == TrialScope::Project;
    let project_path = trial.project_path.as_deref();
    if is_project && project_path.is_none() {
        return Err(format!(
            "Trial for {name} is project-scoped but has no project_path"
        ));
    }

    let skill_dir = &trial.skill_dir;
    if !skill_dir.exists() {
        return Err(format!("{} not found", skill_dir.display()));
    }

    let trash_root = home.join(".agents").join("skills-trash");
    fs::create_dir_all(&trash_root)
        .map_err(|e| format!("Failed to create {}: {e}", trash_root.display()))?;
    let stamp = now.format("%Y%m%d-%H%M%S");
    let deployment_digest = Sha256::digest(trial.deployment_id.as_bytes());
    let deployment_key: String = deployment_digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let invocation_root = loop {
        let candidate = trash_root.join(format!(
            "{name}-{stamp}--v2-{deployment_key}-{}",
            ulid::Ulid::new()
        ));
        match fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Failed to create {}: {error}", candidate.display())),
        }
    };
    let staging_path = invocation_root.join(".staging");
    fs::create_dir(&staging_path)
        .map_err(|error| format!("Failed to create {}: {error}", staging_path.display()))?;
    let trash_path = invocation_root.join("backup");
    // Trash copy first - removal must never run without one already in place.
    // Symlinks are recreated, never followed, so a symlinked file or
    // subdirectory inside the skill isn't silently dropped from the backup.
    let source_fingerprint_before = fingerprint_path(skill_dir);
    if let Err(error) = copy_dir_preserving_symlinks(skill_dir, &staging_path) {
        let _ = fs::remove_dir_all(&invocation_root);
        return Err(error);
    }
    let source_fingerprint_after = fingerprint_path(skill_dir);
    let staging_fingerprint = fingerprint_path(&staging_path);
    if source_fingerprint_before != source_fingerprint_after
        || staging_fingerprint != source_fingerprint_after
    {
        let _ = fs::remove_dir_all(&invocation_root);
        return Err(format!(
            "Trash copy of {} does not match the original; not removing the original",
            skill_dir.display()
        ));
    }
    fs::rename(&staging_path, &trash_path).map_err(|error| {
        format!(
            "Failed to publish trial backup {}: {error}",
            trash_path.display()
        )
    })?;

    match trial.method {
        AddMethod::Dotagents => {
            let scope = if is_project {
                super::skill_dto::InstallScope::Project
            } else {
                super::skill_dto::InstallScope::Global
            };
            let args = super::commands::dotagents_remove_args(name, scope);
            let cwd = if is_project {
                project_path.map(PathBuf::from)
            } else {
                None
            };
            runner.run_npx(&args, cwd.as_deref())?;
        }
        AddMethod::SkillsSh => {
            let scope = if is_project {
                super::skill_dto::InstallScope::Project
            } else {
                super::skill_dto::InstallScope::Global
            };
            let args = skills_sh_remove_args_for_scope(name, scope);
            let cwd = if is_project {
                project_path.map(PathBuf::from)
            } else {
                None
            };
            runner.run_npx(&args, cwd.as_deref())?;
        }
        AddMethod::Copy => {
            fs::remove_dir_all(skill_dir)
                .map_err(|e| format!("Failed to remove {}: {e}", skill_dir.display()))?;
        }
    }

    remove_original_claude_link_if_present(trial)?;

    Ok(trash_path.to_string_lossy().to_string())
}

fn registry_without_expired_trial(
    registry: &ForkRegistry,
    key: &str,
    trial: &TrialRecord,
) -> ForkRegistry {
    let mut next = registry.clone();
    next.trials.remove(key);
    if trial.method == AddMethod::Copy {
        next.copies.retain(|_, record| {
            record.path != trial.skill_dir && trial.claude_link.as_ref() != Some(&record.path)
        });
    }
    next
}

fn restore_copy_trial(trial: &TrialRecord, trash_path: &Path) -> Result<(), String> {
    if fs::symlink_metadata(&trial.skill_dir).is_ok() {
        return Err(format!(
            "Trial rollback refused because {} was replaced",
            trial.skill_dir.display()
        ));
    }
    copy_dir_preserving_symlinks(trash_path, &trial.skill_dir)?;
    if fingerprint_path(&trial.skill_dir) != trial.deployment_fingerprint {
        return Err(format!(
            "Trial rollback did not restore {} exactly",
            trial.skill_dir.display()
        ));
    }

    if let Some(link) = &trial.claude_link {
        let target = trial
            .claude_link_target
            .as_deref()
            .ok_or("Trial rollback has no recorded Claude link target")?;
        if fs::symlink_metadata(link).is_ok() {
            if fs::read_link(link).ok().as_deref() == Some(target) {
                return Ok(());
            }
            return Err(format!(
                "Trial rollback refused because {} was replaced",
                link.display()
            ));
        }
        if let Some(parent) = link.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link)
            .map_err(|error| format!("Failed to restore {}: {error}", link.display()))?;
        #[cfg(not(unix))]
        return Err("Symlinking is only supported on Unix".to_string());
    }
    Ok(())
}

fn trial_deployment_still_matches(trial: &TrialRecord) -> bool {
    if fingerprint_path(&trial.skill_dir) != trial.deployment_fingerprint {
        return false;
    }
    match (&trial.claude_link, &trial.claude_link_target) {
        (Some(link), Some(target)) => {
            fs::symlink_metadata(link)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
                && fs::read_link(link).ok().as_deref() == Some(target)
                && fs::canonicalize(link).ok() == fs::canonicalize(&trial.skill_dir).ok()
        }
        (None, _) => true,
        (Some(_), None) => false,
    }
}

enum ExpiringRecovery {
    Resume,
    FinalizeAbsent,
    RecoveryRequired(String),
}

fn reconcile_expiring_trial(
    snapshot: &super::skill_refresh::SkillSnapshot,
    trial: &TrialRecord,
) -> ExpiringRecovery {
    match fs::symlink_metadata(&trial.skill_dir) {
        Ok(_) => match validate_trial_deployment(snapshot, trial) {
            Ok(_) => ExpiringRecovery::Resume,
            Err(error) => ExpiringRecovery::RecoveryRequired(error),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let link_is_absent_or_original = match (&trial.claude_link, &trial.claude_link_target) {
                (None, _) => true,
                (Some(link), Some(target)) => match fs::symlink_metadata(link) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                    Ok(metadata) => {
                        metadata.file_type().is_symlink()
                            && fs::read_link(link).ok().as_deref() == Some(target)
                    }
                    Err(_) => false,
                },
                (Some(_), None) => false,
            };
            if link_is_absent_or_original {
                ExpiringRecovery::FinalizeAbsent
            } else {
                ExpiringRecovery::RecoveryRequired(
                    "Trial deployment was removed, but its Claude link was replaced or repointed"
                        .to_string(),
                )
            }
        }
        Err(error) => ExpiringRecovery::RecoveryRequired(format!(
            "Trial deployment cannot be inspected: {error}"
        )),
    }
}

fn remove_original_claude_link_if_present(trial: &TrialRecord) -> Result<(), String> {
    let Some(link) = &trial.claude_link else {
        return Ok(());
    };
    let expected_target = trial
        .claude_link_target
        .as_deref()
        .ok_or("Legacy trial has no Claude link identity; keeping it for manual review")?;
    let metadata = match fs::symlink_metadata(link) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Trial Claude link cannot be inspected: {error}")),
    };
    if !metadata.file_type().is_symlink()
        || fs::read_link(link).ok().as_deref() != Some(expected_target)
    {
        return Err(
            "Trial Claude link was replaced or repointed during expiry; the replacement was kept"
                .to_string(),
        );
    }
    fs::remove_file(link).map_err(|error| {
        format!(
            "Failed to remove trial Claude link {}: {error}",
            link.display()
        )
    })
}

fn run_trial_expiry_pass_with_writer(
    home: &Path,
    now: DateTime<Utc>,
    runner: &dyn CommandRunner,
    snapshot: &super::skill_refresh::SkillSnapshot,
    write_registry: &mut dyn FnMut(&Path, &ForkRegistry) -> Result<(), String>,
) -> Vec<ExpiredTrial> {
    let mut registry = match read_fork_registry(home) {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("[skill_trial] {error}");
            return vec![];
        }
    };

    let due: Vec<String> = registry
        .trials
        .iter()
        .filter(|(_, trial)| {
            trial.status == TrialStatus::Expiring
                || (trial.status == TrialStatus::Active
                    && DateTime::parse_from_rfc3339(&trial.expires_at)
                        .map(|expires_at| expires_at.with_timezone(&Utc) <= now)
                        .unwrap_or(false))
        })
        .map(|(key, _)| key.clone())
        .collect();

    let mut expired = Vec::new();
    for key in due {
        let original_registry = registry.clone();
        let trial = registry.trials[&key].clone();
        let name = super::skill_deployment::parse_deployment_id(&trial.deployment_id)
            .map(|parsed| parsed.name)
            .unwrap_or_else(|| name_from_trial_key(&key).to_string());
        if trial.status == TrialStatus::Expiring {
            match reconcile_expiring_trial(snapshot, &trial) {
                ExpiringRecovery::Resume => {}
                ExpiringRecovery::FinalizeAbsent => {
                    if let Err(error) = remove_original_claude_link_if_present(&trial) {
                        eprintln!("[skill_trial] failed to finalize {name}: {error}");
                        continue;
                    }
                    let completed_registry =
                        registry_without_expired_trial(&registry, &key, &trial);
                    match write_registry(home, &completed_registry) {
                        Ok(()) => registry = completed_registry,
                        Err(error) => eprintln!(
                            "[skill_trial] failed to finalize interrupted expiry for {name}: {error}"
                        ),
                    }
                    continue;
                }
                ExpiringRecovery::RecoveryRequired(error) => {
                    registry
                        .trials
                        .get_mut(&key)
                        .expect("expiring trial exists")
                        .status = TrialStatus::RecoveryRequired;
                    if let Err(write_error) = write_registry(home, &registry) {
                        registry = original_registry;
                        eprintln!(
                            "[skill_trial] failed to record recovery state for {name}: {write_error}"
                        );
                    }
                    eprintln!("[skill_trial] {name} needs expiry recovery: {error}");
                    continue;
                }
            }
        }
        if let Err(error) = validate_trial_deployment(snapshot, &trial) {
            eprintln!("[skill_trial] failed to expire {name}: {error}");
            continue;
        }

        if trial.method != AddMethod::Copy {
            registry
                .trials
                .get_mut(&key)
                .expect("due trial exists")
                .status = TrialStatus::Expiring;
            if let Err(error) = write_registry(home, &registry) {
                registry = original_registry;
                eprintln!("[skill_trial] failed to persist expiry tombstone for {name}: {error}");
                continue;
            }
        }

        let trash_path = match expire_one(home, &name, &trial, now, runner) {
            Ok(trash_path) => trash_path,
            Err(error) => {
                if trial.method != AddMethod::Copy {
                    if trial_deployment_still_matches(&trial) {
                        match write_registry(home, &original_registry) {
                            Ok(()) => registry = original_registry,
                            Err(restore_error) => eprintln!(
                                "[skill_trial] failed to restore active trial after {name} removal failed: {restore_error}"
                            ),
                        }
                    } else {
                        eprintln!(
                            "[skill_trial] {name} removal failed after changing the deployment; its durable tombstone remains"
                        );
                    }
                }
                eprintln!("[skill_trial] failed to expire {name}: {error}");
                continue;
            }
        };

        let completed_registry = registry_without_expired_trial(&registry, &key, &trial);
        if let Err(write_error) = write_registry(home, &completed_registry) {
            if trial.method == AddMethod::Copy {
                let restore_error = restore_copy_trial(&trial, Path::new(&trash_path)).err();
                registry = original_registry.clone();
                let registry_restore_error = write_fork_registry(home, &original_registry).err();
                eprintln!(
                    "[skill_trial] failed to persist expiry for {name}: {write_error}; filesystem rollback: {}; registry rollback: {}",
                    restore_error.as_deref().unwrap_or("ok"),
                    registry_restore_error.as_deref().unwrap_or("ok")
                );
            } else {
                eprintln!(
                    "[skill_trial] {name} was removed, but expiry finalization failed; its durable tombstone remains: {write_error}"
                );
            }
            continue;
        }

        registry = completed_registry;
        expired.push(ExpiredTrial { name, trash_path });
    }
    expired
}

/// Removes every trial past `now`'s `expires_at`, one at a time. A skill
/// whose removal fails keeps its trial record (and whatever trash copy was
/// made) so the next tick retries it; only skills that fully expired are
/// returned and dropped from the registry.
pub fn run_trial_expiry_pass(
    home: &Path,
    now: DateTime<Utc>,
    runner: &dyn CommandRunner,
    snapshot: &super::skill_refresh::SkillSnapshot,
) -> Vec<ExpiredTrial> {
    run_trial_expiry_pass_with_writer(home, now, runner, snapshot, &mut write_fork_registry)
}

/// Runs one expiry pass against the real filesystem/CLIs and emits
/// `skills://trial-expired` for anything that expired.
fn run_and_emit(app: &AppHandle) {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let runner = RealCommandRunner;
    let expired = {
        let lock = app.state::<ForkMutationLock>();
        let Ok(_guard) = lock.try_acquire() else {
            return;
        };
        let refresh_state = app.state::<SkillRefreshState>();
        let Ok(snapshot) = skill_refresh::rebuild_snapshot_now(app, &refresh_state) else {
            return;
        };
        run_trial_expiry_pass(&home, Utc::now(), &runner, &snapshot)
    };
    if expired.is_empty() {
        return;
    }
    skill_refresh::request_snapshot_rebuild(app);
    for trial in expired {
        let _ = app.emit(
            "skills://trial-expired",
            serde_json::json!({ "name": trial.name, "trash_path": trial.trash_path }),
        );
    }
}

/// Checks for expired trials 15 s after startup, then every 5 minutes -
/// mirrors `skill_update_check::spawn_update_check_loop`'s shape.
pub fn spawn_trial_expiry_loop(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(15));
        loop {
            run_and_emit(&app);
            std::thread::sleep(std::time::Duration::from_secs(5 * 60));
        }
    });
}

/// Drops `name`'s trial record for `scope` without touching the skill
/// itself. `scope`/`project_path` come from the deployment the Keep button
/// was clicked on, so a same-named global and project trial are kept
/// independently.
#[tauri::command]
pub fn keep_skill_trial(
    target: LifecycleTarget,
    app: AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let deployment_id = target
        .deployment_id
        .as_deref()
        .ok_or("Keep trial needs one deployment_id")?;
    if target.owner_id.is_some() {
        return Err("Keep trial targets one deployment, not an owner group".to_string());
    }
    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let (skill, deployment) = match find_deployment(&snapshot, deployment_id) {
        Ok(found) => found,
        Err(error) => {
            if !drop_recovery_trial_without_deployment(&home, deployment_id)? {
                return Err(error);
            }
            skill_refresh::request_snapshot_rebuild(&app);
            return Ok(());
        }
    };
    revalidate_deployment(deployment, deployment_id)?;
    let scope = if deployment.scope == "global" {
        TrialScope::Global
    } else if deployment.scope == "project" {
        TrialScope::Project
    } else {
        return Err(format!(
            "Keep trial is not available for {} scope",
            deployment.scope
        ));
    };
    drop_trial_record(
        &home,
        deployment_id,
        &skill.name,
        scope,
        Path::new(&deployment.path),
    )?;
    skill_refresh::request_snapshot_rebuild(&app);
    Ok(())
}

/// `<name>-YYYYMMDD-HHMMSS` -> `name`. The suffix is always exactly 16 chars
/// (`-` + 8 digits + `-` + 6 digits), so it can be stripped without a regex
/// crate.
fn strip_trash_suffix(dir_name: &str) -> Option<String> {
    if let Some((legacy_prefix, unique_suffix)) = dir_name.split_once("--v2-") {
        let valid_unique_suffix =
            unique_suffix
                .split_once('-')
                .is_some_and(|(deployment, invocation)| {
                    deployment.len() == 16
                        && deployment
                            .chars()
                            .all(|character| character.is_ascii_hexdigit())
                        && invocation.len() == 26
                        && invocation
                            .chars()
                            .all(|character| character.is_ascii_alphanumeric())
                });
        return valid_unique_suffix
            .then(|| strip_trash_suffix(legacy_prefix))
            .flatten();
    }
    if dir_name.len() < 18 {
        return None;
    }
    let split_at = dir_name.len() - 16;
    let (name, suffix) = dir_name.split_at(split_at);
    let bytes: Vec<char> = suffix.chars().collect();
    let looks_right = bytes.len() == 16
        && bytes[0] == '-'
        && bytes[1..9].iter().all(|c| c.is_ascii_digit())
        && bytes[9] == '-'
        && bytes[10..16].iter().all(|c| c.is_ascii_digit());
    if looks_right {
        Some(name.to_string())
    } else {
        None
    }
}

/// `restore_trashed_skill`'s logic, taking `home` directly so it's testable
/// without a Tauri `AppHandle`. `trash_path` is untrusted IPC input, so it's
/// validated before anything is read from or written to it: it must
/// canonicalize to a path under `~/.agents/skills-trash`, and the name
/// recovered from it must itself pass `validate_skill_dir_name`. Returns the
/// restored skill's name.
pub fn restore_trashed_skill_with(home: &Path, trash_path: &str) -> Result<String, String> {
    let trash_root = home.join(".agents").join("skills-trash");
    let canonical_root = fs::canonicalize(&trash_root)
        .map_err(|e| format!("Failed to resolve {}: {e}", trash_root.display()))?;
    let trash = PathBuf::from(trash_path);
    let canonical_trash =
        fs::canonicalize(&trash).map_err(|e| format!("Invalid trash path: {e}"))?;
    if !canonical_trash.starts_with(&canonical_root) {
        return Err("Trash path is not under ~/.agents/skills-trash".to_string());
    }

    let trash_name_path = if trash.file_name().and_then(|name| name.to_str()) == Some("backup") {
        trash.parent().ok_or("Invalid trash path")?
    } else {
        trash.as_path()
    };
    let dir_name = trash_name_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Invalid trash path")?;
    let name = strip_trash_suffix(dir_name)
        .ok_or("Could not determine the skill's name from its trash path")?;
    super::skill_agent_runner::validate_skill_dir_name(&name)?;

    let shared_dir = home.join(".agents").join("skills");
    let target = shared_dir.join(&name);
    if target.parent() != Some(shared_dir.as_path()) {
        return Err("Refusing to write outside the skills folder".to_string());
    }
    if target.exists() {
        return Err(format!("`{name}` already exists in `~/.agents/skills`"));
    }
    copy_dir_preserving_symlinks(&canonical_trash, &target)?;
    if count_entries(&target) != count_entries(&canonical_trash) {
        return Err(format!(
            "Restore of {name} is incomplete; the trashed copy may be corrupt"
        ));
    }

    let claude_dir = home.join(".claude").join("skills");
    maybe_claude_code_symlink(&claude_dir, &shared_dir, &name, &[AgentId::ClaudeCode])?;
    Ok(name)
}

/// Copies a trashed skill back into `~/.agents/skills/<name>`, untracked
/// (the trial record is gone; this isn't re-registered as a new trial), and
/// re-applies the Claude Code symlink rule.
#[tauri::command]
pub fn restore_trashed_skill(
    trash_path: String,
    app: AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let _ = &refresh_state;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    restore_trashed_skill_with(&home, &trash_path)?;
    skill_refresh::request_snapshot_rebuild(&app);
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRunner {
        calls: Mutex<Vec<(Vec<String>, Option<PathBuf>)>>,
        fail: bool,
    }

    impl CommandRunner for FakeRunner {
        fn run_npx(&self, args: &[String], cwd: Option<&Path>) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push((args.to_vec(), cwd.map(PathBuf::from)));
            if self.fail {
                Err("npx failed".to_string())
            } else {
                Ok(())
            }
        }
    }

    struct ReinstallingRunner {
        skill_dir: PathBuf,
        fail_after_reinstall: bool,
    }

    impl CommandRunner for ReinstallingRunner {
        fn run_npx(&self, _args: &[String], _cwd: Option<&Path>) -> Result<(), String> {
            fs::remove_dir_all(&self.skill_dir).unwrap();
            fs::create_dir_all(&self.skill_dir).unwrap();
            fs::write(self.skill_dir.join("SKILL.md"), "replacement").unwrap();
            if self.fail_after_reinstall {
                Err("CLI failed after replacing deployment".to_string())
            } else {
                Ok(())
            }
        }
    }

    struct RemovingRunner {
        skill_dir: PathBuf,
    }

    impl CommandRunner for RemovingRunner {
        fn run_npx(&self, _args: &[String], _cwd: Option<&Path>) -> Result<(), String> {
            fs::remove_dir_all(&self.skill_dir).map_err(|error| error.to_string())
        }
    }

    struct RepointingSkillsShRunner {
        skill_dir: PathBuf,
        claude_link: PathBuf,
        replacement: PathBuf,
    }

    impl CommandRunner for RepointingSkillsShRunner {
        fn run_npx(&self, _args: &[String], _cwd: Option<&Path>) -> Result<(), String> {
            fs::remove_dir_all(&self.skill_dir).unwrap();
            fs::remove_file(&self.claude_link).unwrap();
            std::os::unix::fs::symlink(&self.replacement, &self.claude_link).unwrap();
            Ok(())
        }
    }

    fn seed_trial(home: &Path, name: &str, method: AddMethod, expires_at: DateTime<Utc>) {
        seed_trial_with_link(home, name, method, expires_at, None);
    }

    fn seed_trial_with_link(
        home: &Path,
        name: &str,
        method: AddMethod,
        expires_at: DateTime<Utc>,
        claude_link: Option<PathBuf>,
    ) {
        let skill_dir = home.join(".agents/skills").join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "body").unwrap();
        let deployment_id = super::super::skill_deployment::deployment_id(
            name,
            "global",
            super::super::skill_deployment::SkillDestination::Universal,
            "universal",
            None,
            &skill_dir,
        );
        let claude_link_target = claude_link
            .as_deref()
            .and_then(|link| fs::read_link(link).ok());
        let mut registry = read_fork_registry(home).unwrap();
        registry.trials.insert(
            deployment_trial_key(&deployment_id),
            TrialRecord {
                deployment_id,
                started_at: (expires_at - chrono::Duration::hours(24)).to_rfc3339(),
                expires_at: expires_at.to_rfc3339(),
                status: TrialStatus::Active,
                method,
                scope: TrialScope::Global,
                project_path: None,
                deployment_fingerprint: fingerprint_path(&skill_dir),
                skill_dir,
                claude_link,
                claude_link_target,
            },
        );
        write_fork_registry(home, &registry).unwrap();
    }

    fn seed_project_trial(
        home: &Path,
        project: &Path,
        name: &str,
        method: AddMethod,
        expires_at: DateTime<Utc>,
    ) -> PathBuf {
        let skill_dir = project.join(".agents/skills").join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "body").unwrap();
        let deployment_id = super::super::skill_deployment::deployment_id(
            name,
            "project",
            super::super::skill_deployment::SkillDestination::Universal,
            "universal",
            project.to_str(),
            &skill_dir,
        );
        let mut registry = read_fork_registry(home).unwrap();
        registry.trials.insert(
            deployment_trial_key(&deployment_id),
            TrialRecord {
                deployment_id,
                started_at: (expires_at - chrono::Duration::hours(24)).to_rfc3339(),
                expires_at: expires_at.to_rfc3339(),
                status: TrialStatus::Active,
                method,
                scope: TrialScope::Project,
                project_path: Some(project.to_string_lossy().to_string()),
                deployment_fingerprint: fingerprint_path(&skill_dir),
                skill_dir: skill_dir.clone(),
                claude_link: None,
                claude_link_target: None,
            },
        );
        write_fork_registry(home, &registry).unwrap();
        skill_dir
    }

    fn trial_snapshot(home: &Path) -> super::super::skill_refresh::SkillSnapshot {
        use super::super::frontmatter::InvocationPolicy;
        use super::super::provenance::SourceKind;
        use super::super::skill_deployment::{BackingRelationship, DeploymentMutability};
        use super::super::skill_dto::{Deployment, InstalledSkill};
        use super::super::skill_invocations::InvocationHeatmap;
        use super::super::skill_ownership::LifecycleOwnerKind;
        use std::collections::BTreeMap;

        let registry = read_fork_registry(home).unwrap();
        let skills = registry
            .trials
            .values()
            .filter_map(|trial| {
                let parsed =
                    super::super::skill_deployment::parse_deployment_id(&trial.deployment_id)?;
                let owner_kind = match trial.method {
                    AddMethod::Dotagents => LifecycleOwnerKind::Dotagents,
                    AddMethod::SkillsSh => LifecycleOwnerKind::SkillsSh,
                    AddMethod::Copy => LifecycleOwnerKind::Copy,
                };
                let canonical = Deployment {
                    id: trial.deployment_id.clone(),
                    destination: parsed.destination,
                    owner_kind,
                    owner_id: None,
                    mutability: DeploymentMutability::Mutable,
                    backing: BackingRelationship::Canonical,
                    agent: "shared".to_string(),
                    scope: parsed.scope.clone(),
                    path: trial.skill_dir.to_string_lossy().to_string(),
                    project_path: trial.project_path.clone(),
                    invocation: InvocationPolicy::Both,
                    ..Default::default()
                };
                let mut deployments = vec![canonical];
                if let Some(link) = &trial.claude_link {
                    deployments.push(Deployment {
                        id: super::super::skill_deployment::deployment_id(
                            &parsed.name,
                            &parsed.scope,
                            parsed.destination,
                            "claude-code",
                            parsed.project_path.as_deref(),
                            link,
                        ),
                        destination: parsed.destination,
                        owner_kind,
                        owner_id: None,
                        mutability: DeploymentMutability::Mutable,
                        backing: BackingRelationship::LinkedTo {
                            deployment_id: trial.deployment_id.clone(),
                        },
                        agent: "Claude Code".to_string(),
                        scope: parsed.scope.clone(),
                        path: link.to_string_lossy().to_string(),
                        project_path: trial.project_path.clone(),
                        is_symlink: true,
                        invocation: InvocationPolicy::Both,
                        ..Default::default()
                    });
                }
                Some(InstalledSkill {
                    name: parsed.name,
                    source: "test".to_string(),
                    source_type: "test".to_string(),
                    source_url: None,
                    skill_path: None,
                    installed_at: Utc::now().to_rfc3339(),
                    updated_at: None,
                    has_update: false,
                    update_owner_ids: Vec::new(),
                    update_owners: Vec::new(),
                    update_commit: None,
                    update_commit_at: None,
                    source_kind: SourceKind::Manual,
                    deployments,
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
                    frontmatter_fields: BTreeMap::new(),
                    folder_truncated: false,
                    fork: None,
                    trial: None,
                    trials: Vec::new(),
                    parked: false,
                    parked_at: None,
                    invocation: InvocationPolicy::Both,
                })
            })
            .collect();
        super::super::skill_refresh::SkillSnapshot {
            skills,
            projects: Vec::new(),
            invocations: Vec::new(),
            heatmap: InvocationHeatmap::default(),
            scanned_at: Utc::now().to_rfc3339(),
            last_test_by_skill: Default::default(),
            update_check: Default::default(),
            opencode_config_kind: None,
        }
    }

    #[test]
    fn expiry_selects_only_trials_past_now() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        seed_trial(
            home,
            "old-one",
            AddMethod::Copy,
            now - chrono::Duration::hours(1),
        );
        seed_trial(
            home,
            "fresh-one",
            AddMethod::Copy,
            now + chrono::Duration::hours(23),
        );

        let runner = FakeRunner::default();
        let snapshot = trial_snapshot(home);
        let expired = run_trial_expiry_pass(home, now, &runner, &snapshot);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].name, "old-one");
        assert!(!home.join(".agents/skills/old-one").exists());
        assert!(home.join(".agents/skills/fresh-one").exists());
        let registry = read_fork_registry(home).unwrap();
        assert!(!registry
            .trials
            .values()
            .any(|trial| trial.deployment_id.contains("old-one")));
        assert!(registry
            .trials
            .values()
            .any(|trial| trial.deployment_id.contains("fresh-one")));
    }

    #[test]
    fn copy_expiry_registry_failure_restores_files_link_and_old_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        let link = home.join(".claude/skills/find-bugs");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../../.agents/skills/find-bugs", &link).unwrap();
        seed_trial_with_link(
            home,
            "find-bugs",
            AddMethod::Copy,
            now - chrono::Duration::hours(1),
            Some(link.clone()),
        );
        let old_registry = read_fork_registry(home).unwrap();
        let snapshot = trial_snapshot(home);

        let expired = run_trial_expiry_pass_with_writer(
            home,
            now,
            &FakeRunner::default(),
            &snapshot,
            &mut |_, _| Err("injected registry write failure".to_string()),
        );

        assert!(expired.is_empty());
        assert_eq!(
            read_fork_registry(home).unwrap().trials,
            old_registry.trials
        );
        assert_eq!(
            fs::read_to_string(home.join(".agents/skills/find-bugs/SKILL.md")).unwrap(),
            "body"
        );
        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );
        assert!(fs::read_dir(home.join(".agents/skills-trash"))
            .unwrap()
            .next()
            .is_some());
    }

    #[test]
    fn simultaneous_same_name_global_and_project_expiry_keeps_distinct_recoverable_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let now = Utc::now();
        seed_trial(
            &home,
            "find-bugs",
            AddMethod::Copy,
            now - chrono::Duration::hours(1),
        );
        let project_skill = seed_project_trial(
            &home,
            &project,
            "find-bugs",
            AddMethod::Copy,
            now - chrono::Duration::hours(1),
        );
        let global_skill = home.join(".agents/skills/find-bugs");
        fs::write(global_skill.join("SKILL.md"), "global body").unwrap();
        fs::write(project_skill.join("SKILL.md"), "project body").unwrap();
        let mut registry = read_fork_registry(&home).unwrap();
        for trial in registry.trials.values_mut() {
            trial.deployment_fingerprint = fingerprint_path(&trial.skill_dir);
        }
        write_fork_registry(&home, &registry).unwrap();
        let snapshot = trial_snapshot(&home);
        let mut writes = 0;

        let mut expired = run_trial_expiry_pass_with_writer(
            &home,
            now,
            &FakeRunner::default(),
            &snapshot,
            &mut |home, registry| {
                writes += 1;
                if writes == 1 {
                    Err("injected registry write failure".to_string())
                } else {
                    write_fork_registry(home, registry)
                }
            },
        );
        assert_eq!(expired.len(), 1);
        assert_eq!(read_fork_registry(&home).unwrap().trials.len(), 1);
        assert_eq!(
            global_skill.exists() as usize + project_skill.exists() as usize,
            1
        );

        let recovery_snapshot = trial_snapshot(&home);
        expired.extend(run_trial_expiry_pass(
            &home,
            now + chrono::Duration::minutes(1),
            &FakeRunner::default(),
            &recovery_snapshot,
        ));
        assert_eq!(expired.len(), 2);
        assert!(read_fork_registry(&home).unwrap().trials.is_empty());
        assert!(!global_skill.exists());
        assert!(!project_skill.exists());
        assert_ne!(expired[0].trash_path, expired[1].trash_path);
        let mut backed_up_bodies: Vec<String> = expired
            .iter()
            .map(|trial| fs::read_to_string(Path::new(&trial.trash_path).join("SKILL.md")).unwrap())
            .collect();
        backed_up_bodies.sort();
        assert_eq!(backed_up_bodies, vec!["global body", "project body"]);
    }

    #[test]
    fn cli_expiry_finalization_failure_tombstones_reinstalled_replacement() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        seed_trial(
            home,
            "find-bugs",
            AddMethod::Dotagents,
            now - chrono::Duration::hours(1),
        );
        let snapshot = trial_snapshot(home);
        let skill_dir = home.join(".agents/skills/find-bugs");
        let mut writes = 0;

        let expired = run_trial_expiry_pass_with_writer(
            home,
            now,
            &ReinstallingRunner {
                skill_dir: skill_dir.clone(),
                fail_after_reinstall: false,
            },
            &snapshot,
            &mut |home, registry| {
                writes += 1;
                if writes == 2 {
                    Err("injected final registry write failure".to_string())
                } else {
                    write_fork_registry(home, registry)
                }
            },
        );

        assert!(expired.is_empty());
        assert_eq!(
            read_fork_registry(home)
                .unwrap()
                .trials
                .values()
                .next()
                .unwrap()
                .status,
            TrialStatus::Expiring
        );
        assert_eq!(
            fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
            "replacement"
        );

        let replacement_snapshot = trial_snapshot(home);
        let retried = run_trial_expiry_pass(
            home,
            now + chrono::Duration::hours(1),
            &FakeRunner::default(),
            &replacement_snapshot,
        );
        assert!(retried.is_empty());
        assert_eq!(
            fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
            "replacement"
        );
        assert_eq!(
            read_fork_registry(home)
                .unwrap()
                .trials
                .values()
                .next()
                .unwrap()
                .status,
            TrialStatus::RecoveryRequired
        );
    }

    #[test]
    fn cli_failure_after_replacement_keeps_durable_expiry_tombstone() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        seed_trial(
            home,
            "find-bugs",
            AddMethod::SkillsSh,
            now - chrono::Duration::hours(1),
        );
        let snapshot = trial_snapshot(home);
        let skill_dir = home.join(".agents/skills/find-bugs");

        let expired = run_trial_expiry_pass(
            home,
            now,
            &ReinstallingRunner {
                skill_dir: skill_dir.clone(),
                fail_after_reinstall: true,
            },
            &snapshot,
        );

        assert!(expired.is_empty());
        assert_eq!(
            read_fork_registry(home)
                .unwrap()
                .trials
                .values()
                .next()
                .unwrap()
                .status,
            TrialStatus::Expiring
        );
        assert_eq!(
            fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
            "replacement"
        );

        let replacement_snapshot = trial_snapshot(home);
        let retried = run_trial_expiry_pass(
            home,
            now + chrono::Duration::minutes(1),
            &FakeRunner::default(),
            &replacement_snapshot,
        );
        assert!(retried.is_empty());
        assert_eq!(
            read_fork_registry(home)
                .unwrap()
                .trials
                .values()
                .next()
                .unwrap()
                .status,
            TrialStatus::RecoveryRequired
        );
        assert_eq!(
            fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
            "replacement"
        );
    }

    #[test]
    fn restart_resumes_an_expiring_exact_deployment() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        seed_trial(
            home,
            "find-bugs",
            AddMethod::SkillsSh,
            now - chrono::Duration::hours(1),
        );
        let mut registry = read_fork_registry(home).unwrap();
        registry.trials.values_mut().next().unwrap().status = TrialStatus::Expiring;
        write_fork_registry(home, &registry).unwrap();
        let snapshot = trial_snapshot(home);
        let skill_dir = home.join(".agents/skills/find-bugs");

        let expired = run_trial_expiry_pass(
            home,
            now,
            &RemovingRunner {
                skill_dir: skill_dir.clone(),
            },
            &snapshot,
        );

        assert_eq!(expired.len(), 1);
        assert!(!skill_dir.exists());
        assert!(read_fork_registry(home).unwrap().trials.is_empty());
    }

    #[test]
    fn restart_finalizes_an_expiring_deployment_already_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        seed_trial(
            home,
            "find-bugs",
            AddMethod::SkillsSh,
            now - chrono::Duration::hours(1),
        );
        let mut registry = read_fork_registry(home).unwrap();
        registry.trials.values_mut().next().unwrap().status = TrialStatus::Expiring;
        let skill_dir = registry.trials.values().next().unwrap().skill_dir.clone();
        write_fork_registry(home, &registry).unwrap();
        fs::remove_dir_all(skill_dir).unwrap();
        let snapshot = trial_snapshot(home);
        let runner = FakeRunner::default();

        let expired = run_trial_expiry_pass(home, now, &runner, &snapshot);

        assert!(expired.is_empty());
        assert!(runner.calls.lock().unwrap().is_empty());
        assert!(read_fork_registry(home).unwrap().trials.is_empty());
    }

    #[test]
    fn recovery_record_can_be_cleared_after_its_deployment_is_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        seed_trial(
            home,
            "find-bugs",
            AddMethod::SkillsSh,
            now - chrono::Duration::hours(1),
        );
        let mut registry = read_fork_registry(home).unwrap();
        let trial = registry.trials.values_mut().next().unwrap();
        trial.status = TrialStatus::RecoveryRequired;
        let deployment_id = trial.deployment_id.clone();
        let skill_dir = trial.skill_dir.clone();
        write_fork_registry(home, &registry).unwrap();
        fs::remove_dir_all(skill_dir).unwrap();

        assert!(drop_recovery_trial_without_deployment(home, &deployment_id).unwrap());
        assert!(read_fork_registry(home).unwrap().trials.is_empty());
    }

    #[test]
    fn skills_sh_expiry_keeps_a_claude_link_replaced_by_the_cli() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        let link = home.join(".claude/skills/find-bugs");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../../.agents/skills/find-bugs", &link).unwrap();
        seed_trial_with_link(
            home,
            "find-bugs",
            AddMethod::SkillsSh,
            now - chrono::Duration::hours(1),
            Some(link.clone()),
        );
        let replacement = home.join("replacement/find-bugs");
        fs::create_dir_all(&replacement).unwrap();
        fs::write(replacement.join("SKILL.md"), "replacement").unwrap();
        let skill_dir = home.join(".agents/skills/find-bugs");
        let snapshot = trial_snapshot(home);

        let expired = run_trial_expiry_pass(
            home,
            now,
            &RepointingSkillsShRunner {
                skill_dir,
                claude_link: link.clone(),
                replacement: replacement.clone(),
            },
            &snapshot,
        );

        assert!(expired.is_empty());
        assert_eq!(
            fs::canonicalize(&link).unwrap(),
            fs::canonicalize(&replacement).unwrap()
        );
        assert_eq!(
            read_fork_registry(home)
                .unwrap()
                .trials
                .values()
                .next()
                .unwrap()
                .status,
            TrialStatus::Expiring
        );

        let restart_snapshot = trial_snapshot(home);
        run_trial_expiry_pass(
            home,
            now + chrono::Duration::minutes(1),
            &FakeRunner::default(),
            &restart_snapshot,
        );
        assert_eq!(
            fs::canonicalize(&link).unwrap(),
            fs::canonicalize(&replacement).unwrap()
        );
        assert_eq!(
            read_fork_registry(home)
                .unwrap()
                .trials
                .values()
                .next()
                .unwrap()
                .status,
            TrialStatus::RecoveryRequired
        );
    }

    #[test]
    fn expiry_after_park_and_unpark_removes_the_restored_exact_claude_link() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        let link_path = home.join(".claude/skills/find-bugs");
        fs::create_dir_all(link_path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../../.agents/skills/find-bugs", &link_path).unwrap();
        seed_trial_with_link(
            home,
            "find-bugs",
            AddMethod::Copy,
            now + chrono::Duration::hours(1),
            Some(link_path.clone()),
        );

        super::super::skill_park::park_skill_with(
            home,
            "find-bugs",
            super::super::provenance::SourceKind::Manual,
            now,
        )
        .unwrap();
        let parked = read_fork_registry(home).unwrap();
        let parked_trial = parked.trials.values().next().unwrap();
        assert_eq!(parked_trial.claude_link, None);
        assert_eq!(
            parked_trial.claude_link_target,
            Some(PathBuf::from("../../.agents/skills/find-bugs"))
        );

        super::super::skill_park::unpark_skill_with(home, "find-bugs", now).unwrap();
        let mut registry = read_fork_registry(home).unwrap();
        let trial = registry.trials.values_mut().next().unwrap();
        assert_eq!(trial.claude_link, Some(link_path.clone()));
        trial.expires_at = (now - chrono::Duration::seconds(1)).to_rfc3339();
        write_fork_registry(home, &registry).unwrap();
        let snapshot = trial_snapshot(home);

        let expired = run_trial_expiry_pass(home, now, &FakeRunner::default(), &snapshot);
        assert_eq!(expired.len(), 1);
        assert!(fs::symlink_metadata(&link_path).is_err());
        assert!(!home.join(".agents/skills/find-bugs").exists());
    }

    #[test]
    fn trash_copy_precedes_removal_and_survives_a_failing_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        seed_trial(
            home,
            "find-bugs",
            AddMethod::Dotagents,
            now - chrono::Duration::hours(1),
        );

        let runner = FakeRunner {
            fail: true,
            ..Default::default()
        };
        let snapshot = trial_snapshot(home);
        let expired = run_trial_expiry_pass(home, now, &runner, &snapshot);
        assert!(expired.is_empty());
        // The folder is untouched (the CLI would have removed it, but our
        // fake failed before that), and the trial record is kept for retry.
        assert!(home.join(".agents/skills/find-bugs").exists());
        let registry = read_fork_registry(home).unwrap();
        assert!(registry
            .trials
            .values()
            .any(|trial| trial.deployment_id.contains("find-bugs")));
        // The trash copy was still made before the failing removal attempt.
        let trash_root = home.join(".agents/skills-trash");
        let has_trash = fs::read_dir(&trash_root)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
        assert!(has_trash);
    }

    #[test]
    fn trash_copy_preserves_a_symlinked_file_inside_the_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        seed_trial(
            home,
            "find-bugs",
            AddMethod::Copy,
            now - chrono::Duration::hours(1),
        );
        fs::write(home.join(".agents/skills/find-bugs/target.txt"), "t").unwrap();
        std::os::unix::fs::symlink("target.txt", home.join(".agents/skills/find-bugs/link.txt"))
            .unwrap();
        let mut registry = read_fork_registry(home).unwrap();
        registry.trials.values_mut().for_each(|trial| {
            trial.deployment_fingerprint = fingerprint_path(&trial.skill_dir);
        });
        write_fork_registry(home, &registry).unwrap();

        let snapshot = trial_snapshot(home);
        let expired = run_trial_expiry_pass(home, now, &FakeRunner::default(), &snapshot);
        let trashed = PathBuf::from(&expired[0].trash_path);
        assert!(fs::symlink_metadata(trashed.join("link.txt"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn dotagents_removal_uses_project_flag_for_project_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let now = Utc::now();
        seed_project_trial(
            &home,
            &project,
            "find-bugs",
            AddMethod::Dotagents,
            now - chrono::Duration::hours(1),
        );

        let runner = FakeRunner::default();
        let snapshot = trial_snapshot(&home);
        let expired = run_trial_expiry_pass(&home, now, &runner, &snapshot);
        assert_eq!(expired.len(), 1);
        let calls = runner.calls.lock().unwrap();
        assert!(calls[0].0.contains(&"--project".to_string()));
        assert_eq!(calls[0].1, Some(project));
    }

    #[test]
    fn symlink_cleanup_only_removes_a_link_the_app_would_have_created() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        let link = home.join(".claude/skills/find-bugs");
        seed_trial_with_link(
            home,
            "find-bugs",
            AddMethod::Copy,
            now - chrono::Duration::hours(1),
            None,
        );
        std::os::unix::fs::symlink(home.join(".agents/skills/find-bugs"), &link).unwrap();
        let mut registry = read_fork_registry(home).unwrap();
        let trial = registry.trials.values_mut().next().unwrap();
        trial.claude_link = Some(link.clone());
        trial.claude_link_target = Some(fs::read_link(&link).unwrap());
        write_fork_registry(home, &registry).unwrap();
        // A plain directory for a different, unrelated skill - must survive.
        fs::create_dir_all(home.join(".claude/skills/unrelated")).unwrap();

        let snapshot = trial_snapshot(home);
        run_trial_expiry_pass(home, now, &FakeRunner::default(), &snapshot);
        assert!(fs::symlink_metadata(&link).is_err());
        assert!(home.join(".claude/skills/unrelated").is_dir());
    }

    #[test]
    fn expiry_refuses_a_repointed_claude_link_and_keeps_both_deployments() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        seed_trial(
            home,
            "find-bugs",
            AddMethod::Copy,
            now - chrono::Duration::hours(1),
        );
        let trial_dir = home.join(".agents/skills/find-bugs");
        let independent = home.join("independent/find-bugs");
        fs::create_dir_all(&independent).unwrap();
        fs::write(independent.join("SKILL.md"), "other").unwrap();
        let link = home.join(".claude/skills/find-bugs");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&trial_dir, &link).unwrap();
        let mut registry = read_fork_registry(home).unwrap();
        let trial = registry.trials.values_mut().next().unwrap();
        trial.claude_link = Some(link.clone());
        trial.claude_link_target = Some(fs::read_link(&link).unwrap());
        write_fork_registry(home, &registry).unwrap();
        let snapshot = trial_snapshot(home);
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&independent, &link).unwrap();

        let expired = run_trial_expiry_pass(home, now, &FakeRunner::default(), &snapshot);

        assert!(expired.is_empty());
        assert!(trial_dir.join("SKILL.md").is_file());
        assert!(independent.join("SKILL.md").is_file());
        assert_eq!(
            fs::canonicalize(link).unwrap(),
            fs::canonicalize(independent).unwrap()
        );
        assert_eq!(read_fork_registry(home).unwrap().trials.len(), 1);
    }

    #[test]
    fn expiry_refuses_content_fingerprint_drift() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        seed_trial(
            home,
            "find-bugs",
            AddMethod::Copy,
            now - chrono::Duration::hours(1),
        );
        let snapshot = trial_snapshot(home);
        fs::write(home.join(".agents/skills/find-bugs/SKILL.md"), "edited").unwrap();

        let expired = run_trial_expiry_pass(home, now, &FakeRunner::default(), &snapshot);

        assert!(expired.is_empty());
        assert!(home.join(".agents/skills/find-bugs/SKILL.md").is_file());
        assert_eq!(read_fork_registry(home).unwrap().trials.len(), 1);
    }

    #[test]
    fn expiry_keeps_ambiguous_legacy_record() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        let skill_dir = home.join(".agents/skills/find-bugs");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "body").unwrap();
        let mut registry = read_fork_registry(home).unwrap();
        registry.trials.insert(
            trial_key(TrialScope::Global, "find-bugs"),
            TrialRecord {
                deployment_id: String::new(),
                started_at: (now - chrono::Duration::hours(25)).to_rfc3339(),
                expires_at: (now - chrono::Duration::hours(1)).to_rfc3339(),
                status: TrialStatus::Active,
                method: AddMethod::Copy,
                scope: TrialScope::Global,
                project_path: None,
                skill_dir: skill_dir.clone(),
                deployment_fingerprint: String::new(),
                claude_link: None,
                claude_link_target: None,
            },
        );
        write_fork_registry(home, &registry).unwrap();
        let snapshot = trial_snapshot(home);

        let expired = run_trial_expiry_pass(home, now, &FakeRunner::default(), &snapshot);

        assert!(expired.is_empty());
        assert!(skill_dir.join("SKILL.md").is_file());
        assert!(read_fork_registry(home)
            .unwrap()
            .trials
            .contains_key(&trial_key(TrialScope::Global, "find-bugs")));
    }

    #[test]
    fn keep_removes_the_trial_record_without_touching_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        seed_trial(
            home,
            "find-bugs",
            AddMethod::Copy,
            Utc::now() + chrono::Duration::hours(5),
        );

        let mut registry = read_fork_registry(home).unwrap();
        registry.trials.clear();
        write_fork_registry(home, &registry).unwrap();

        let registry = read_fork_registry(home).unwrap();
        assert!(registry.trials.is_empty());
        assert!(home.join(".agents/skills/find-bugs").exists());
    }

    #[test]
    fn same_name_global_and_project_trials_expire_independently() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let now = Utc::now();
        seed_trial(
            &home,
            "find-bugs",
            AddMethod::Copy,
            now - chrono::Duration::hours(1),
        );
        let project_skill_dir = seed_project_trial(
            &home,
            &project,
            "find-bugs",
            AddMethod::Copy,
            now + chrono::Duration::hours(23),
        );

        let snapshot = trial_snapshot(&home);
        let expired = run_trial_expiry_pass(&home, now, &FakeRunner::default(), &snapshot);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].name, "find-bugs");
        // The global trial expired; the project-scoped one (not due yet) is untouched.
        assert!(!home.join(".agents/skills/find-bugs").exists());
        assert!(project_skill_dir.exists());
        let registry = read_fork_registry(&home).unwrap();
        assert_eq!(registry.trials.len(), 1);
        assert_eq!(
            registry.trials.values().next().unwrap().scope,
            TrialScope::Project
        );
    }

    #[test]
    fn legacy_project_trial_is_not_kept_from_another_same_name_project() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project_a = tmp.path().join("project-a/.agents/skills/find-bugs");
        let project_b = tmp.path().join("project-b/.agents/skills/find-bugs");
        fs::create_dir_all(&project_a).unwrap();
        fs::create_dir_all(&project_b).unwrap();
        let mut registry = read_fork_registry(&home).unwrap();
        registry.trials.insert(
            trial_key(TrialScope::Project, "find-bugs"),
            TrialRecord {
                deployment_id: String::new(),
                started_at: Utc::now().to_rfc3339(),
                expires_at: (Utc::now() + chrono::Duration::hours(24)).to_rfc3339(),
                status: TrialStatus::Active,
                method: AddMethod::Copy,
                scope: TrialScope::Project,
                project_path: Some(
                    project_a
                        .ancestors()
                        .nth(3)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                ),
                skill_dir: project_a.clone(),
                deployment_fingerprint: String::new(),
                claude_link: None,
                claude_link_target: None,
            },
        );
        write_fork_registry(&home, &registry).unwrap();
        let project_b_id = super::super::skill_deployment::deployment_id(
            "find-bugs",
            "project",
            super::super::skill_deployment::SkillDestination::Universal,
            "universal",
            project_b.ancestors().nth(3).and_then(Path::to_str),
            &project_b,
        );

        drop_trial_record(
            &home,
            &project_b_id,
            "find-bugs",
            TrialScope::Project,
            &project_b,
        )
        .unwrap();

        assert!(read_fork_registry(&home)
            .unwrap()
            .trials
            .contains_key(&trial_key(TrialScope::Project, "find-bugs")));
    }

    #[test]
    fn restore_round_trips_a_trashed_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let trash_dir = home.join(".agents/skills-trash/find-bugs-20260101-120000");
        fs::create_dir_all(&trash_dir).unwrap();
        fs::write(trash_dir.join("SKILL.md"), "body").unwrap();
        fs::create_dir_all(home.join(".claude/skills")).unwrap();

        let name = restore_trashed_skill_with(home, &trash_dir.to_string_lossy()).unwrap();
        assert_eq!(name, "find-bugs");

        let target = home.join(".agents/skills/find-bugs");
        assert!(target.join("SKILL.md").exists());
        assert!(fs::symlink_metadata(home.join(".claude/skills/find-bugs"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    /// Finding 4: a symlinked file and a symlinked subdirectory inside the
    /// trashed skill must both survive the restore round-trip, not be
    /// silently dropped by a plain (symlink-skipping) copy.
    #[test]
    fn restore_preserves_symlinked_files_and_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let trash_dir = home.join(".agents/skills-trash/find-bugs-20260101-120000");
        fs::create_dir_all(&trash_dir).unwrap();
        fs::write(trash_dir.join("SKILL.md"), "body").unwrap();
        fs::create_dir_all(trash_dir.join("real-dir")).unwrap();
        fs::write(trash_dir.join("real-dir/inner.txt"), "inner").unwrap();
        std::os::unix::fs::symlink("real-dir", trash_dir.join("linked-dir")).unwrap();
        std::os::unix::fs::symlink("SKILL.md", trash_dir.join("linked-file")).unwrap();

        let name = restore_trashed_skill_with(home, &trash_dir.to_string_lossy()).unwrap();
        let target = home.join(".agents/skills").join(name);

        assert!(fs::symlink_metadata(target.join("linked-dir"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(fs::symlink_metadata(target.join("linked-file"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_link(target.join("linked-dir")).unwrap(),
            PathBuf::from("real-dir")
        );
    }

    #[test]
    fn restore_rejects_a_trash_path_outside_skills_trash() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join(".agents/skills-trash")).unwrap();
        let outside = home.join("not-trash/find-bugs-20260101-120000");
        fs::create_dir_all(&outside).unwrap();

        let err = restore_trashed_skill_with(home, &outside.to_string_lossy()).unwrap_err();
        assert!(err.contains("not under"));
    }

    #[test]
    fn strip_trash_suffix_rejects_names_without_the_timestamp_shape() {
        assert_eq!(strip_trash_suffix("find-bugs"), None);
        assert_eq!(
            strip_trash_suffix("find-bugs-20260101-120000"),
            Some("find-bugs".to_string())
        );
    }
}
