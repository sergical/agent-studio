// ============================================================================
// Skills Module - skill_park
// "Park" is Skill Studio's global disable for the harnesses that discover
// skills purely by directory presence and have no native per-skill switch
// (Claude Code, pi): the shared-folder copy at `~/.agents/skills/<name>`
// moves to `~/.agents/skills-parked/<name>`, and a per-skill Claude Code
// symlink pointing at it (if any) is removed. `unpark_skill` reverses both.
// Known limitation: `dotagents install` / `npx skills add` can recreate the
// shared folder while a skill is parked - the snapshot detects that (see
// `skill_refresh::build_snapshot`) and reports it as the frontend's
// `parked-but-reinstalled` health issue; `unpark_skill` reconciles it by
// retaining the old tree under `~/.agents/skills-trash` while preserving the
// reinstalled tree. Identical content is reported as reconciled.
// ============================================================================

#[cfg(test)]
use std::fs;
#[cfg(test)]
use std::path::{Path, PathBuf};

#[cfg(test)]
use chrono::DateTime;
use chrono::Utc;
use tauri::Manager;

use super::event_commands::EventStoreState;
use super::skill_dto::LifecycleTarget;
use super::skill_fork::ForkMutationLock;
use super::skill_lifecycle::{
    find_deployment, require_global_universal_park_target, revalidate_deployment,
};
use super::skill_refresh::{self, SkillRefreshState};
use skill_studio_core::skill_fork_registry::ParkedRecord;
pub use skill_studio_core::skill_park_operation::UnparkOutcome;
use skill_studio_core::skill_provenance::SourceKind;

/// `~/.agents/skills-parked`.
#[cfg(test)]
fn skills_parked_root(home: &Path) -> PathBuf {
    home.join(".agents").join("skills-parked")
}

#[cfg(test)]
fn shared_skill_dir(home: &Path, name: &str) -> PathBuf {
    home.join(".agents").join("skills").join(name)
}

/// Test adapter used by trial integration tests. Production commands build
/// the same scoped operation from desktop state below.
#[cfg(test)]
pub fn park_skill_with(
    home: &Path,
    name: &str,
    source_kind: SourceKind,
    now: DateTime<Utc>,
) -> Result<ParkedRecord, String> {
    let scope = skill_studio_core::skill_service::SkillScope {
        home: home.to_path_buf(),
        projects: vec![],
        backing_roots: vec![],
        plugin_ownership_roots: vec![],
    };
    let mut service = skill_studio_core::skill_service::ScopedSkillService::bind(scope)
        .map_err(|error| error.to_string())?;
    let active = shared_skill_dir(home, name);
    let deployment = service
        .scan(None, Some(std::time::Duration::from_secs(5)))
        .map_err(|error| error.to_string())?
        .skills
        .into_iter()
        .flat_map(|skill| skill.deployments)
        .find(|deployment| Path::new(&deployment.path) == active)
        .ok_or_else(|| format!("\"{name}\" is not deployed in the Universal folder"))?;
    let store =
        super::event_store::EventStore::open(&home.join(".agents/.skill-studio-park-test-events"))?;
    skill_studio_core::skill_park_operation::park_skill(
        &mut service,
        &store,
        &skill_studio_core::skill_park_operation::ParkSkillRequest {
            deployment_id: deployment.id,
            source_kind,
            parked_at: now.to_rfc3339(),
        },
        super::skill_harness_disable::copy_visibility_limits(),
        Some(std::time::Duration::from_secs(5)),
        skill_studio_core::skill_service::CancellationToken::default(),
    )
}

/// `unpark_skill`'s logic. See `UnparkOutcome` for the three ways this can
/// resolve; the "parked-but-reinstalled" case (the shared folder was
/// recreated while parked) is detected here from `shared_dir.exists()`.
#[cfg(test)]
pub fn unpark_skill_with(
    home: &Path,
    name: &str,
    now: DateTime<Utc>,
) -> Result<UnparkOutcome, String> {
    let _ = now;
    let scope = skill_studio_core::skill_service::SkillScope {
        home: home.to_path_buf(),
        projects: vec![],
        backing_roots: vec![],
        plugin_ownership_roots: vec![],
    };
    let mut service = skill_studio_core::skill_service::ScopedSkillService::bind(scope)
        .map_err(|error| error.to_string())?;
    let parked_path = skills_parked_root(home).join(name);
    let deployment = service
        .scan(None, Some(std::time::Duration::from_secs(5)))
        .map_err(|error| error.to_string())?
        .skills
        .into_iter()
        .flat_map(|skill| skill.deployments)
        .find(|deployment| Path::new(&deployment.path) == parked_path)
        .ok_or_else(|| format!("\"{name}\" is not parked"))?;
    let store =
        super::event_store::EventStore::open(&home.join(".agents/.skill-studio-park-test-events"))?;
    skill_studio_core::skill_park_operation::unpark_skill(
        &mut service,
        &store,
        &skill_studio_core::skill_park_operation::UnparkSkillRequest {
            deployment_id: deployment.id,
        },
        super::skill_harness_disable::copy_visibility_limits(),
        Some(std::time::Duration::from_secs(5)),
        skill_studio_core::skill_service::CancellationToken::default(),
    )
}

fn park_target_skill(
    snapshot: &skill_refresh::SkillSnapshot,
    target: &LifecycleTarget,
    action: &str,
) -> Result<(String, SourceKind), String> {
    let deployment_id = target
        .deployment_id
        .as_deref()
        .ok_or("Park needs a deployment_id")?;
    if target.owner_id.is_some() {
        return Err("Park targets one Global Universal deployment, not an owner group".to_string());
    }
    let (skill, deployment) = find_deployment(snapshot, deployment_id)?;
    revalidate_deployment(deployment, deployment_id)?;
    require_global_universal_park_target(deployment)?;
    match action {
        "Park" if deployment.scope != "global" => {
            return Err("Park is only available for the Global Universal folder.".to_string())
        }
        "Unpark" if deployment.scope != "parked" => {
            return Err(
                "Unpark is only available for a parked Global Universal folder.".to_string(),
            )
        }
        _ => {}
    }
    Ok((skill.name.clone(), skill.source_kind))
}

#[tauri::command]
pub async fn park_skill(
    target: LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<ParkedRecord, String> {
    tauri::async_runtime::spawn_blocking(move || park_skill_blocking(target, app))
        .await
        .map_err(|error| format!("Park task failed: {error}"))?
}

fn park_skill_blocking(
    target: LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<ParkedRecord, String> {
    let fork_lock = app.state::<ForkMutationLock>();
    let _guard = fork_lock.try_acquire()?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let refresh_state = app.state::<SkillRefreshState>();
    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    let (name, source_kind) = park_target_skill(&snapshot, &target, "Park")?;
    let deployment_id = target
        .deployment_id
        .clone()
        .ok_or("Park needs a deployment_id")?;
    let projects = snapshot
        .projects
        .iter()
        .map(std::path::PathBuf::from)
        .collect::<Vec<_>>();
    let scope = super::skill_scope_config::desktop_skill_scope(&home, &projects)?;
    let mut service = skill_studio_core::skill_service::ScopedSkillService::bind(scope)
        .map_err(|error| error.to_string())?;
    let event_store = app.state::<EventStoreState>();
    let event_guard = event_store
        .0
        .lock()
        .map_err(|error| format!("event store lock poisoned: {error}"))?;
    let store = event_guard
        .as_ref()
        .ok_or("Event store is unavailable; Park made no changes")?;
    let _transaction = super::skill_md_write::begin_skill_md_write_transaction()?;
    let result = skill_studio_core::skill_park_operation::park_skill(
        &mut service,
        store,
        &skill_studio_core::skill_park_operation::ParkSkillRequest {
            deployment_id,
            source_kind,
            parked_at: Utc::now().to_rfc3339(),
        },
        super::skill_harness_disable::copy_visibility_limits(),
        Some(std::time::Duration::from_secs(30)),
        skill_studio_core::skill_service::CancellationToken::default(),
    );
    if let Ok(record) = &result {
        let parked_at = record.parked_at.clone();
        if let Err(e) = skill_refresh::patch_snapshot_and_emit(&app, &refresh_state, |snapshot| {
            let Some(skill) = snapshot.skills.iter_mut().find(|s| s.name == name) else {
                return;
            };
            skill.parked = true;
            skill.parked_at = Some(parked_at);
        }) {
            eprintln!("[park_skill] snapshot patch failed: {e}");
        }
    }
    result
}

#[tauri::command]
pub async fn unpark_skill(
    target: LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<UnparkOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || unpark_skill_blocking(target, app))
        .await
        .map_err(|error| format!("Unpark task failed: {error}"))?
}

fn unpark_skill_blocking(
    target: LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<UnparkOutcome, String> {
    let fork_lock = app.state::<ForkMutationLock>();
    let _guard = fork_lock.try_acquire()?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let refresh_state = app.state::<SkillRefreshState>();
    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    let (name, _) = park_target_skill(&snapshot, &target, "Unpark")?;
    let deployment_id = target
        .deployment_id
        .clone()
        .ok_or("Unpark needs a deployment_id")?;
    let projects = snapshot
        .projects
        .iter()
        .map(std::path::PathBuf::from)
        .collect::<Vec<_>>();
    let scope = super::skill_scope_config::desktop_skill_scope(&home, &projects)?;
    let mut service = skill_studio_core::skill_service::ScopedSkillService::bind(scope)
        .map_err(|error| error.to_string())?;
    let event_store = app.state::<EventStoreState>();
    let event_guard = event_store
        .0
        .lock()
        .map_err(|error| format!("event store lock poisoned: {error}"))?;
    let store = event_guard
        .as_ref()
        .ok_or("Event store is unavailable; Unpark made no changes")?;
    let _transaction = super::skill_md_write::begin_skill_md_write_transaction()?;
    let result = skill_studio_core::skill_park_operation::unpark_skill(
        &mut service,
        store,
        &skill_studio_core::skill_park_operation::UnparkSkillRequest { deployment_id },
        super::skill_harness_disable::copy_visibility_limits(),
        Some(std::time::Duration::from_secs(30)),
        skill_studio_core::skill_service::CancellationToken::default(),
    );
    if result.is_ok() {
        if let Err(e) = skill_refresh::patch_snapshot_and_emit(&app, &refresh_state, |snapshot| {
            let Some(skill) = snapshot.skills.iter_mut().find(|s| s.name == name) else {
                return;
            };
            skill.parked = false;
            skill.parked_at = None;
        }) {
            eprintln!("[unpark_skill] snapshot patch failed: {e}");
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(dir: &Path, name: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test\n---\nBody."),
        )
        .unwrap();
    }

    #[test]
    fn recovery_refuses_name_only_intent_without_content_or_owner_evidence() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        let store = super::super::event_store::EventStore::open(&home.join("state")).unwrap();
        store
            .record(
                "unbound-park-recovery",
                super::super::event_store::EventDraft {
                    kind: "unpark_global_universal".into(),
                    skill: "find-bugs".into(),
                    harness: None,
                    scope: Some("global".into()),
                    project_path: None,
                    payload: serde_json::json!({"version": 1, "name": "find-bugs"}),
                    inverse: None,
                    backup_dir: None,
                    restorable: false,
                },
            )
            .unwrap();
        let row = store.get("unbound-park-recovery").unwrap().unwrap();
        let before = fs::read(home.join(".agents/skills/find-bugs/SKILL.md")).unwrap();
        let scope = skill_studio_core::skill_service::SkillScope {
            home: home.clone(),
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        };
        let mut service =
            skill_studio_core::skill_service::ScopedSkillService::bind(scope).unwrap();
        assert!(
            skill_studio_core::skill_park_operation::recover_park_operation(
                &mut service,
                &store,
                &row,
                super::super::skill_harness_disable::copy_visibility_limits(),
                Some(std::time::Duration::from_secs(5)),
            )
            .is_err(),
            "Folder presence alone must not settle an operation without ownership/content evidence"
        );
        assert_eq!(store.get(&row.id).unwrap().unwrap().status, "pending");
        assert_eq!(
            fs::read(home.join(".agents/skills/find-bugs/SKILL.md")).unwrap(),
            before
        );
    }
}
