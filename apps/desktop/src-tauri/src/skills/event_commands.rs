// ============================================================================
// Skills Module - event_commands
// Tauri IPC surface for the event store (docs/spec-event-store.md): listing
// History rows, restoring an event, and the Locations card's per-harness
// disable entry point for shared-folder skills (`skill_materialize`).
// `EventStoreState` wraps an `Option` rather than the bare `EventStore`
// because opening the database can fail (e.g. a locked or corrupt file) and
// the app should still start - every command surfaces that as an ordinary
// `Err` instead of panicking at startup.
// ============================================================================

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::agents::AgentId;
use super::event_store::{EventRow, EventStore};
use super::skill_agent_runner::validate_skill_dir_name;
use super::skill_dto::{Deployment, LifecycleTarget, SkillEventDto};
use super::skill_fork::ForkMutationLock;
use super::skill_materialize;
use super::skill_refresh::{self, SkillRefreshState, SkillSnapshot};

pub struct EventStoreState(pub Mutex<Option<EventStore>>);

fn locked_store(
    state: &EventStoreState,
) -> Result<std::sync::MutexGuard<'_, Option<EventStore>>, String> {
    state
        .0
        .lock()
        .map_err(|e| format!("event store lock poisoned: {e}"))
}

fn dto_from_row(store: &EventStore, row: EventRow) -> SkillEventDto {
    let restorable = row.inverse.is_some()
        && row.reverted_by.is_none()
        && matches!(row.status.as_str(), "done" | "failed" | "interrupted");
    let backup_path = row
        .backup_dir
        .as_ref()
        .map(|dir| store.app_data.join(dir).to_string_lossy().into_owned());
    SkillEventDto {
        id: row.id,
        ts: row.ts,
        kind: row.kind,
        skill: row.skill,
        harness: row.harness,
        scope: row.scope,
        project_path: row.project_path,
        status: row.status,
        restorable,
        reverted_by: row.reverted_by,
        backup_path,
    }
}

/// Lists events newest-first, for the Activity view's History section.
#[tauri::command]
pub fn list_skill_events(
    limit: Option<usize>,
    skill: Option<String>,
    event_store: tauri::State<EventStoreState>,
) -> Result<Vec<SkillEventDto>, String> {
    let guard = locked_store(&event_store)?;
    let store = guard.as_ref().ok_or("Event store is unavailable")?;
    let rows = store.list(limit.unwrap_or(200), skill.as_deref())?;
    Ok(rows
        .into_iter()
        .map(|row| dto_from_row(store, row))
        .collect())
}

/// Undoes one event. Refuses an `explode_shared_dir` restore while any of
/// its skills are individually disabled (`restore_guard_for_explode`), and
/// unregisters the materialized root once such a restore succeeds.
#[tauri::command]
pub fn restore_skill_event(
    event_id: String,
    force: bool,
    app: tauri::AppHandle,
    fork_lock: tauri::State<ForkMutationLock>,
    event_store: tauri::State<EventStoreState>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let guard = locked_store(&event_store)?;
    let store = guard.as_ref().ok_or("Event store is unavailable")?;

    let target = store
        .get(&event_id)?
        .ok_or_else(|| format!("Event {event_id} not found"))?;
    skill_materialize::restore_guard_for_explode(store, &target)?;
    store.restore(&event_id, force)?;
    if target.kind == "explode_shared_dir" {
        if let Some(root) = target.payload.get("root").and_then(|v| v.as_str()) {
            store.unregister_materialized_root(Path::new(root))?;
        }
    }
    drop(guard);

    skill_refresh::request_snapshot_rebuild(&app);
    Ok(())
}

/// The Locations card's entry point for disabling/enabling one skill under
/// one harness that reads from the shared root. Converts a whole-dir link to
/// per-skill links on first disable (`explode_shared_dir`), then delegates
/// to `unlink_harness`/`relink_harness`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn set_shared_harness_skill_enabled(
    root_path: String,
    target: LifecycleTarget,
    harness: String,
    enabled: bool,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
    event_store: tauri::State<EventStoreState>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let deployment_id = target
        .deployment_id
        .as_deref()
        .ok_or("Shared harness disable needs one deployment_id")?;
    if target.owner_id.is_some() {
        return Err(
            "Shared harness disable targets one deployment, not an owner group".to_string(),
        );
    }
    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    let (installed_skill, deployment) =
        super::skill_lifecycle::find_deployment(&snapshot, deployment_id)?;
    super::skill_lifecycle::revalidate_deployment(deployment, deployment_id)?;
    let display = AgentId::all()
        .into_iter()
        .find(|agent| {
            agent.cli_name() == harness || (*agent == AgentId::OpenCode && harness == "open-code")
        })
        .map(|agent| agent.display_name())
        .ok_or_else(|| format!("Unknown harness: {harness}"))?;
    if deployment.agent != display
        || Path::new(&deployment.path).parent() != Some(Path::new(&root_path))
        || !matches!(
            deployment.backing,
            super::skill_deployment::BackingRelationship::LinkedTo { .. }
        )
    {
        return Err(format!(
            "Deployment {deployment_id} is not the selected {harness} deployment under {root_path}"
        ));
    }
    let skill = installed_skill.name.clone();
    validate_skill_dir_name(&skill)?;
    let root = PathBuf::from(&root_path);

    let guard = locked_store(&event_store)?;
    let store = guard.as_ref().ok_or("Event store is unavailable")?;

    if enabled {
        skill_materialize::relink_harness(store, &root, &skill, &harness)?;
    } else {
        // No longer converts a whole-dir link implicitly (that used to run
        // silently on first disable) - the frontend routes that case through
        // the explicit `materialize_harness_root` dialog first (Locations
        // card / Home repair card) and only calls this once the root is
        // already per-skill links.
        let is_whole_dir_link = std::fs::symlink_metadata(&root)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_whole_dir_link {
            return Err(format!(
                "{} is a link to the shared folder; convert it to per-skill links first",
                root.display()
            ));
        }
        skill_materialize::unlink_harness(store, &root, &skill, &harness)?;
    }
    drop(guard);

    skill_refresh::request_snapshot_rebuild(&app);
    Ok(())
}

/// `materialize_harness_root`'s guard against a renderer-supplied
/// `(harness, root)` pair that doesn't match a real whole-directory link:
/// the snapshot must record a global deployment for `harness` at exactly
/// `root` with `shared_via_whole_dir_link` set.
fn validate_materialize_request(
    snapshot: &SkillSnapshot,
    target: &LifecycleTarget,
    harness: &str,
    root: &str,
) -> Result<PathBuf, String> {
    let deployment_id = target
        .deployment_id
        .as_deref()
        .ok_or("Materialization needs one deployment_id")?;
    if target.owner_id.is_some() {
        return Err("Materialization targets one deployment, not an owner group".to_string());
    }
    let (_, deployment) = super::skill_lifecycle::find_deployment(snapshot, deployment_id)?;
    super::skill_lifecycle::revalidate_deployment(deployment, deployment_id)?;
    let display = AgentId::all()
        .into_iter()
        .find(|agent| {
            agent.cli_name() == harness || (*agent == AgentId::OpenCode && harness == "open-code")
        })
        .map(|agent| agent.display_name().to_string())
        .unwrap_or_else(|| harness.to_string());
    if deployment.agent != display || !deployment.shared_via_whole_dir_link {
        return Err(format!(
            "Deployment {deployment_id} is not a recorded whole-directory link for {harness}"
        ));
    }
    let deployment_root = Path::new(&deployment.path)
        .parent()
        .ok_or_else(|| format!("{} has no skills root", deployment.path))?;
    if deployment_root != Path::new(root) {
        return Err(format!(
            "{root} is not the harness root of deployment {deployment_id}"
        ));
    }
    let universal_id = match &deployment.backing {
        super::skill_deployment::BackingRelationship::LinkedTo { deployment_id } => deployment_id,
        _ => return Err("Materialization requires a deployment linked to Universal".to_string()),
    };
    let (_, universal) = super::skill_lifecycle::find_deployment(snapshot, universal_id)?;
    if universal.scope != deployment.scope
        || universal.project_path != deployment.project_path
        || !matches!(
            universal.backing,
            super::skill_deployment::BackingRelationship::Canonical
        )
    {
        return Err(
            "The harness deployment does not match its exact scoped Universal deployment"
                .to_string(),
        );
    }
    Path::new(&universal.path)
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{} has no Universal root", universal.path))
}

/// Converts a harness's whole-dir link to the shared skills root into a real
/// directory of per-skill links, as an explicit, named action - the
/// Locations card's Convert dialog and Home's linked-root repair card, both
/// of which must ask before doing this (see the module doc). Refuses when
/// `root` isn't a symlink whose canonical target ends in `.agents/skills`, or
/// when the snapshot has no matching whole-dir-link deployment for `harness`.
#[tauri::command]
pub fn materialize_harness_root(
    app: tauri::AppHandle,
    target: LifecycleTarget,
    harness: String,
    root: String,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
    event_store: tauri::State<EventStoreState>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let root_path = PathBuf::from(&root);
    skill_materialize::validate_materialize_root(&root_path)?;

    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    let universal_root = validate_materialize_request(&snapshot, &target, &harness, &root)?;
    let resolved_harness_root = std::fs::canonicalize(&root_path)
        .map_err(|error| format!("Failed to resolve {root}: {error}"))?;
    let resolved_universal_root = std::fs::canonicalize(&universal_root).map_err(|error| {
        format!(
            "Failed to resolve selected Universal root {}: {error}",
            universal_root.display()
        )
    })?;
    if resolved_harness_root != resolved_universal_root {
        return Err(format!(
            "{root} does not point to the selected deployment's exact scoped Universal root {}",
            universal_root.display()
        ));
    }

    let guard = locked_store(&event_store)?;
    let store = guard.as_ref().ok_or("Event store is unavailable")?;
    skill_materialize::explode_shared_dir(store, &root_path, &harness)?;
    drop(guard);

    skill_refresh::request_snapshot_rebuild(&app);
    Ok(())
}

/// Normalizes a deployment path for comparison against the snapshot without
/// requiring it to resolve: `fs::canonicalize` fails on a broken symlink's
/// final component, so this canonicalizes the *parent* directory instead and
/// rejoins the file name. Works whether or not `path` itself resolves.
fn normalize_link_path(path: &Path) -> Option<PathBuf> {
    let file_name = path.file_name()?;
    let parent = path.parent()?;
    let canonical_parent = std::fs::canonicalize(parent).ok()?;
    Some(canonical_parent.join(file_name))
}

/// Finds the `(skill name, deployment)` in `snapshot` whose path is `path`,
/// matched via `normalize_link_path` so a broken symlink still resolves to
/// its snapshot entry. Used to keep `repair_skill_link` from becoming an
/// arbitrary rm/ln - it can only touch a path the snapshot already knows as
/// a deployment.
fn find_deployment_at<'a>(
    snapshot: &'a SkillSnapshot,
    path: &Path,
) -> Option<(&'a str, &'a Deployment)> {
    let normalized = normalize_link_path(path)?;
    snapshot.skills.iter().find_map(|skill| {
        skill
            .deployments
            .iter()
            .find(|d| {
                normalize_link_path(Path::new(&d.path)).as_deref() == Some(normalized.as_path())
            })
            .map(|d| (skill.name.as_str(), d))
    })
}

fn is_unresolved(deployment: &Deployment) -> bool {
    deployment.symlink_is_broken || deployment.symlink_error.is_some()
}

/// SkillPage's "Repair this location" entry point for a broken deployment
/// symlink that `unlink_harness`/`relink_harness` don't cover (those only
/// handle the shared-root materialize pattern). Validates `path` against the
/// current snapshot as an unresolved deployment, and - for `"relink"` -
/// `target` as a healthy deployment of the *same* skill, so this can't be
/// used to rm/ln an arbitrary path.
#[tauri::command]
pub fn repair_skill_link(
    path: String,
    action: String,
    target: Option<String>,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
    event_store: tauri::State<EventStoreState>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let link = PathBuf::from(&path);

    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;

    let (skill_name, deployment) = find_deployment_at(&snapshot, &link)
        .ok_or_else(|| format!("Path is not an installed skill: {path}"))?;
    if !is_unresolved(deployment) {
        return Err(format!("{path} is not a broken link"));
    }
    let skill_name = skill_name.to_string();
    let harness = deployment.agent.clone();

    let guard = locked_store(&event_store)?;
    let store = guard.as_ref().ok_or("Event store is unavailable")?;

    let mut relinked_target: Option<String> = None;
    match action.as_str() {
        "remove" => skill_materialize::repair_remove_link(store, &link, &skill_name, &harness)?,
        "relink" => {
            let target = target.ok_or("relink requires a target")?;
            let target_path = PathBuf::from(&target);
            let (target_skill, target_deployment) = find_deployment_at(&snapshot, &target_path)
                .ok_or_else(|| format!("Target is not an installed skill: {target}"))?;
            if target_skill != skill_name {
                return Err("Target must be a deployment of the same skill".to_string());
            }
            if is_unresolved(target_deployment) {
                return Err("Target location is not healthy".to_string());
            }
            let resolved_target = std::fs::canonicalize(&target_path)
                .map_err(|e| format!("Failed to resolve {target}: {e}"))?;
            skill_materialize::repair_relink_link(
                store,
                &link,
                &resolved_target,
                &skill_name,
                &harness,
            )?;
            relinked_target = Some(resolved_target.to_string_lossy().into_owned());
        }
        other => return Err(format!("Unknown repair action: {other}")),
    }
    drop(guard);

    let normalized_link = normalize_link_path(&link);
    match action.as_str() {
        "remove" => {
            if let Err(e) =
                skill_refresh::patch_snapshot_and_emit(&app, &refresh_state, |snapshot| {
                    let Some(skill) = snapshot.skills.iter_mut().find(|s| s.name == skill_name)
                    else {
                        return;
                    };
                    skill.deployments.retain(|d| {
                        normalize_link_path(Path::new(&d.path)).as_deref()
                            != normalized_link.as_deref()
                    });
                    if skill.deployments.is_empty() {
                        snapshot.skills.retain(|s| s.name != skill_name);
                    }
                })
            {
                eprintln!("[repair_skill_link] snapshot patch failed: {e}");
            }
        }
        "relink" => {
            let new_target = relinked_target;
            if let Err(e) =
                skill_refresh::patch_snapshot_and_emit(&app, &refresh_state, |snapshot| {
                    let Some(skill) = snapshot.skills.iter_mut().find(|s| s.name == skill_name)
                    else {
                        return;
                    };
                    let Some(deployment) = skill.deployments.iter_mut().find(|d| {
                        normalize_link_path(Path::new(&d.path)).as_deref()
                            == normalized_link.as_deref()
                    }) else {
                        return;
                    };
                    deployment.symlink_target = new_target;
                    deployment.symlink_is_broken = false;
                    deployment.symlink_error = None;
                })
            {
                eprintln!("[repair_skill_link] snapshot patch failed: {e}");
            }
        }
        _ => unreachable!("validated above"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::symlink;

    /// Builds a two-deployment snapshot for one skill: `broken_path` as an
    /// unresolved (broken symlink) deployment, `healthy_path` as a resolved
    /// one - for `find_deployment_at`/`repair_skill_link` validation tests,
    /// without needing a running Tauri app.
    fn fixture_snapshot(broken_path: &Path, healthy_path: &Path) -> SkillSnapshot {
        use super::super::provenance::SourceKind;
        use super::super::skill_dto::InstalledSkill;

        fn deployment(path: &Path, broken: bool) -> Deployment {
            Deployment {
                agent: "Claude Code".to_string(),
                scope: "project".to_string(),
                path: path.to_string_lossy().to_string(),
                is_symlink: true,
                plugin: None,
                symlink_is_broken: broken,
                ..Default::default()
            }
        }

        SkillSnapshot {
            skills: vec![InstalledSkill {
                name: "find-bugs".to_string(),
                source: "manual".to_string(),
                source_type: "manual".to_string(),
                source_url: None,
                skill_path: None,
                installed_at: chrono::Utc::now().to_rfc3339(),
                updated_at: None,
                has_update: false,
                update_owner_ids: Vec::new(),
                update_owners: Vec::new(),
                update_commit: None,
                update_commit_at: None,
                source_kind: SourceKind::Manual,
                deployments: vec![
                    deployment(broken_path, true),
                    deployment(healthy_path, false),
                ],
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
                invocation: super::super::frontmatter::InvocationPolicy::Both,
            }],
            projects: Vec::new(),
            invocations: Vec::new(),
            heatmap: super::super::skill_invocations::InvocationHeatmap::default(),
            scanned_at: chrono::Utc::now().to_rfc3339(),
            last_test_by_skill: Default::default(),
            update_check: Default::default(),
            opencode_config_kind: None,
        }
    }

    /// A snapshot with one global, whole-dir-linked Claude Code deployment,
    /// for `validate_materialize_request` tests.
    fn fixture_materialize_snapshot(root: &Path) -> SkillSnapshot {
        use super::super::skill_deployment::{
            deployment_id, BackingRelationship, SkillDestination,
        };

        let canonical_path = PathBuf::from("/home/.agents/skills/find-bugs");
        let canonical_id = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            &canonical_path,
        );
        let linked_path = root.join("find-bugs");
        let linked_id = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::Universal,
            "claude-code",
            None,
            &linked_path,
        );
        let mut snapshot = fixture_snapshot(&linked_path, &canonical_path);
        let linked = &mut snapshot.skills[0].deployments[0];
        linked.id = linked_id;
        linked.destination = SkillDestination::Universal;
        linked.scope = "global".to_string();
        linked.symlink_is_broken = false;
        linked.shared_via_whole_dir_link = true;
        linked.backing = BackingRelationship::LinkedTo {
            deployment_id: canonical_id.clone(),
        };
        let canonical = &mut snapshot.skills[0].deployments[1];
        canonical.id = canonical_id;
        canonical.agent = "shared".to_string();
        canonical.destination = SkillDestination::Universal;
        canonical.scope = "global".to_string();
        canonical.is_symlink = false;
        canonical.backing = BackingRelationship::Canonical;
        snapshot
    }

    fn materialize_target(snapshot: &SkillSnapshot) -> LifecycleTarget {
        LifecycleTarget {
            deployment_id: Some(snapshot.skills[0].deployments[0].id.clone()),
            owner_id: None,
        }
    }

    #[test]
    fn validate_materialize_request_accepts_a_recorded_whole_dir_link() {
        let root = PathBuf::from("/home/.claude/skills");
        let snapshot = fixture_materialize_snapshot(&root);
        let target = materialize_target(&snapshot);
        assert!(validate_materialize_request(
            &snapshot,
            &target,
            "claude-code",
            "/home/.claude/skills"
        )
        .is_ok());
    }

    #[test]
    fn materialize_refuses_when_fresh_snapshot_no_longer_has_cached_target() {
        let root = PathBuf::from("/home/.claude/skills");
        let cached = fixture_materialize_snapshot(&root);
        let target = materialize_target(&cached);
        assert!(validate_materialize_request(
            &cached,
            &target,
            "claude-code",
            "/home/.claude/skills"
        )
        .is_ok());

        let mut fresh = cached;
        fresh.skills[0]
            .deployments
            .retain(|deployment| target.deployment_id.as_deref() != Some(&deployment.id));
        assert!(validate_materialize_request(
            &fresh,
            &target,
            "claude-code",
            "/home/.claude/skills"
        )
        .is_err());
    }

    #[test]
    fn materialize_resolves_whole_root_children_to_the_exact_universal_deployment() {
        use super::super::skill_deployment::{id_for_candidate, DeploymentCandidate};

        for (label, harness, root) in [
            ("Claude Code", "claude-code", "/home/.claude/skills"),
            ("OpenCode", "open-code", "/home/.config/opencode/skills"),
        ] {
            let root = PathBuf::from(root);
            let linked_path = root.join("find-bugs");
            let resolved_path = PathBuf::from("/home/.agents/skills/find-bugs");
            let (linked_id, _, backing) = id_for_candidate(DeploymentCandidate {
                name: "find-bugs",
                root_label: label,
                scope: "global",
                path: &linked_path,
                project_path: None,
                is_symlink: false,
                symlink_target: None,
                resolved_path: Some(&resolved_path),
                shared_via_whole_dir_link: true,
            });
            let mut snapshot = fixture_materialize_snapshot(&root);
            let linked = &mut snapshot.skills[0].deployments[0];
            linked.id = linked_id.clone();
            linked.agent = label.to_string();
            linked.path = linked_path.to_string_lossy().into_owned();
            linked.resolved_path = Some(resolved_path.to_string_lossy().into_owned());
            linked.backing = backing;
            let target = LifecycleTarget {
                deployment_id: Some(linked_id),
                owner_id: None,
            };

            let selected =
                validate_materialize_request(&snapshot, &target, harness, &root.to_string_lossy())
                    .unwrap();
            assert_eq!(selected, PathBuf::from("/home/.agents/skills"), "{label}");
        }
    }

    #[test]
    fn validate_materialize_request_rejects_a_root_not_in_the_snapshot() {
        let root = PathBuf::from("/home/.claude/skills");
        let snapshot = fixture_materialize_snapshot(&root);
        let target = materialize_target(&snapshot);
        let err =
            validate_materialize_request(&snapshot, &target, "claude-code", "/home/.codex/skills")
                .unwrap_err();
        assert!(err.contains("/home/.codex/skills"), "{err}");
    }

    #[test]
    fn validate_materialize_request_rejects_a_harness_root_mismatch() {
        let root = PathBuf::from("/home/.claude/skills");
        let snapshot = fixture_materialize_snapshot(&root);
        let target = materialize_target(&snapshot);
        // The root is recorded for Claude Code, not Codex - the two must
        // agree, not just each independently point at something real.
        let err = validate_materialize_request(&snapshot, &target, "codex", "/home/.claude/skills")
            .unwrap_err();
        assert!(err.contains("codex"), "{err}");
    }

    #[test]
    fn materialize_same_name_project_target_resolves_only_project_universal_root() {
        use super::super::skill_deployment::{
            deployment_id, BackingRelationship, SkillDestination,
        };

        let mut snapshot = fixture_materialize_snapshot(Path::new("/home/.claude/skills"));
        let project_root = PathBuf::from("/work/app/.agents/skills");
        let project_skill = project_root.join("find-bugs");
        let project_id = deployment_id(
            "find-bugs",
            "project",
            SkillDestination::Universal,
            "universal",
            Some("/work/app"),
            &project_skill,
        );
        let linked_path = PathBuf::from("/work/app/.claude/skills/find-bugs");
        let linked_id = deployment_id(
            "find-bugs",
            "project",
            SkillDestination::Universal,
            "claude-code",
            Some("/work/app"),
            &linked_path,
        );
        let mut project_link = snapshot.skills[0].deployments[0].clone();
        project_link.id = linked_id.clone();
        project_link.scope = "project".to_string();
        project_link.project_path = Some("/work/app".to_string());
        project_link.path = linked_path.to_string_lossy().into_owned();
        project_link.backing = BackingRelationship::LinkedTo {
            deployment_id: project_id.clone(),
        };
        let mut project_universal = snapshot.skills[0].deployments[1].clone();
        project_universal.id = project_id;
        project_universal.scope = "project".to_string();
        project_universal.project_path = Some("/work/app".to_string());
        project_universal.path = project_skill.to_string_lossy().into_owned();
        snapshot.skills[0].deployments.push(project_link);
        snapshot.skills[0].deployments.push(project_universal);
        let target = LifecycleTarget {
            deployment_id: Some(linked_id),
            owner_id: None,
        };

        let selected = validate_materialize_request(
            &snapshot,
            &target,
            "claude-code",
            "/work/app/.claude/skills",
        )
        .unwrap();
        assert_eq!(selected, project_root);
    }

    #[test]
    fn find_deployment_at_matches_a_broken_symlink_by_its_own_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let broken = root.join("find-bugs-claude");
        symlink("/does/not/exist", &broken).unwrap();
        let healthy = root.join("find-bugs-codex");
        fs::create_dir_all(&healthy).unwrap();

        let snapshot = fixture_snapshot(&broken, &healthy);
        let (skill, deployment) = find_deployment_at(&snapshot, &broken).unwrap();
        assert_eq!(skill, "find-bugs");
        assert!(is_unresolved(deployment));
    }

    #[test]
    fn find_deployment_at_rejects_a_path_outside_the_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let broken = root.join("find-bugs-claude");
        symlink("/does/not/exist", &broken).unwrap();
        let healthy = root.join("find-bugs-codex");
        fs::create_dir_all(&healthy).unwrap();
        let outside = root.join("some-other-skill");
        fs::create_dir_all(&outside).unwrap();

        let snapshot = fixture_snapshot(&broken, &healthy);
        assert!(find_deployment_at(&snapshot, &outside).is_none());
    }

    #[test]
    fn repair_refuses_when_fresh_snapshot_no_longer_has_cached_link() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let broken = root.join("find-bugs-claude");
        symlink("/does/not/exist", &broken).unwrap();
        let healthy = root.join("find-bugs-codex");
        fs::create_dir_all(&healthy).unwrap();
        let cached = fixture_snapshot(&broken, &healthy);
        assert!(find_deployment_at(&cached, &broken).is_some());

        let mut fresh = cached;
        fresh.skills[0]
            .deployments
            .retain(|deployment| Path::new(&deployment.path) != broken);
        assert!(find_deployment_at(&fresh, &broken).is_none());
    }

    #[test]
    fn find_deployment_at_finds_the_healthy_deployment_as_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let broken = root.join("find-bugs-claude");
        symlink("/does/not/exist", &broken).unwrap();
        let healthy = root.join("find-bugs-codex");
        fs::create_dir_all(&healthy).unwrap();

        let snapshot = fixture_snapshot(&broken, &healthy);
        let (_, deployment) = find_deployment_at(&snapshot, &healthy).unwrap();
        assert!(!is_unresolved(deployment));
    }
}
