// ============================================================================
// Skills Module - Tauri Commands
// IPC commands for skill discovery, installation, and management
// ============================================================================

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::api;
use super::skill_dto::{
    InstallScope, InstalledSkill, LifecycleTarget, PaginatedSkillsResponse, SkillDetails,
};
use super::skill_editor;
use super::skill_lifecycle::{
    ledger_matching_deployment, rebuild_fresh_lifecycle_snapshot, resolve_lifecycle_target,
};
use super::skill_md_write::write_skill_md_compare_and_swap;
use super::skill_process::RealCommandRunner;
use super::skill_refresh::{self, SkillRefreshState};
use super::skill_update_check;
use skill_studio_core::dto::{RemoveOutcome, RemoveRequest};
use skill_studio_core::identity::{CorrelationId, DeploymentId};
use skill_studio_core::ops::{self, Operation, ResultEnvelope};
use skill_studio_core::ports::OpContext;
use tauri::Manager;

/// The `npx -y @sentry/dotagents add <source> --name <name> [--ref <ref>]`
/// argv - the plain (non-re-pinning) shape of what `dotagents_update_args`
/// builds for a named entry, reused by `skill_fork::unfork_skill` to
/// reinstall a fork from its recorded origin.
pub(crate) fn dotagents_add_args(source: &str, name: &str, r#ref: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "-y".to_string(),
        "@sentry/dotagents".to_string(),
        "add".to_string(),
        source.to_string(),
        "--name".to_string(),
        name.to_string(),
    ];
    if let Some(r#ref) = r#ref {
        args.push("--ref".to_string());
        args.push(r#ref.to_string());
    }
    args
}

/// The `npx -y @sentry/dotagents remove <name>` argv, reused by
/// `skill_fork::fork_skill` to detach a dotagents-managed skill.
pub(crate) fn dotagents_remove_args(name: &str, scope: InstallScope) -> Vec<String> {
    let mut args = vec!["-y".to_string(), "@sentry/dotagents".to_string()];
    if scope == InstallScope::Project {
        args.push("--project".to_string());
    }
    args.extend(["remove".to_string(), name.to_string()]);
    args
}

#[cfg(test)]
fn with_authorized_lifecycle_command_target<T>(
    snapshot: &skill_refresh::SkillSnapshot,
    target: &LifecycleTarget,
    action: &str,
    operation: impl FnOnce(InstalledSkill, super::skill_dto::Deployment) -> Result<T, String>,
) -> Result<T, String> {
    let (skill, deployment) = resolve_lifecycle_target(snapshot, target, action)?;
    operation(skill, deployment)
}

/// Search for skills on skills.sh
#[tauri::command]
pub async fn search_skills(
    query: String,
    limit: Option<u32>,
    app: tauri::AppHandle,
) -> Result<PaginatedSkillsResponse, String> {
    crate::timing_log::time_command_async(&app, "search_skills", async move {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let access = api::resolve_skills_sh_access(&home)?;
        api::search_skills(&access, &query, limit).await
    })
    .await
}

/// Get popular skills (sorted by install count)
#[tauri::command]
pub async fn get_popular_skills(
    page: Option<u32>,
    per_page: Option<u32>,
    app: tauri::AppHandle,
) -> Result<PaginatedSkillsResponse, String> {
    crate::timing_log::time_command_async(&app, "get_popular_skills", async move {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let access = api::resolve_skills_sh_access(&home)?;
        api::get_popular_skills(&access, page, per_page).await
    })
    .await
}

/// Get skill details from skills.sh
#[tauri::command]
pub async fn get_skill_details(
    skill_id: String,
    app: tauri::AppHandle,
) -> Result<SkillDetails, String> {
    crate::timing_log::time_command_async(&app, "get_skill_details", async move {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let access = api::resolve_skills_sh_access(&home)?;
        api::get_skill_details(&access, &skill_id).await
    })
    .await
}

/// Get all installed skills. Returns the background-refreshed snapshot's
/// skills (see `skill_refresh`) when one exists and no mutation is pending
/// (`skills_dirty`); otherwise rebuilds the snapshot synchronously (so a
/// read right after a write, or the very first read before the background
/// thread's initial build has landed, still sees fresh data). The project
/// list comes from `~/.agents/skill-studio.json` (see
/// `skill_refresh::effective_project_paths`), not from the caller.
#[tauri::command]
pub async fn get_installed_skills(app: tauri::AppHandle) -> Result<Vec<InstalledSkill>, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "get_installed_skills", move || {
        let refresh_state = app.state::<SkillRefreshState>();
        let snapshot = refresh_state.snapshot.read().ok().and_then(|g| g.clone());

        if let Some(snapshot) = &snapshot {
            if !refresh_state.is_skills_dirty() {
                return Ok(snapshot.skills.clone());
            }
        }

        let rebuilt = skill_refresh::rebuild_snapshot_now(&app, &refresh_state)?;
        Ok(rebuilt.skills)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::super::skill_md_write::write_skill_md;
    use super::super::skill_process::CommandRunner;
    use super::*;
    use std::sync::atomic::Ordering;

    struct CountingLifecycleRunner(std::sync::atomic::AtomicUsize);

    impl CommandRunner for CountingLifecycleRunner {
        fn run(&self, _program: &str, _args: &[String], _cwd: Option<&Path>) -> Result<(), String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// A minimal `SkillSnapshot` with one skill deployed at `dep_dir`, with
    /// or without a plugin deployment, for `check_skill_md_write_allowed` tests.
    fn fixture_snapshot(
        dep_dir: &std::path::Path,
        plugin: Option<super::super::skill_dto::PluginInfo>,
    ) -> skill_refresh::SkillSnapshot {
        use super::super::skill_dto::{Deployment, InstalledSkill};
        use super::super::SourceKind;
        use chrono::Utc;
        use skill_studio_core::skill_uses::InvocationHeatmap;
        use std::collections::BTreeMap;

        skill_refresh::SkillSnapshot {
            revision: 0,
            skills: vec![InstalledSkill {
                name: "foo".to_string(),
                source: "manual".to_string(),
                source_type: "manual".to_string(),
                source_url: None,
                skill_path: None,
                installed_at: Utc::now().to_rfc3339(),
                updated_at: None,
                has_update: false,
                update_owner_ids: Vec::new(),
                update_owners: Vec::new(),
                update_commit: None,
                update_commit_at: None,
                source_kind: if plugin.is_some() {
                    SourceKind::Plugin
                } else {
                    SourceKind::Manual
                },
                deployments: vec![Deployment {
                    agent: "Claude Code".to_string(),
                    scope: "project".to_string(),
                    path: dep_dir.to_string_lossy().to_string(),
                    is_symlink: false,
                    plugin,
                    ..Default::default()
                }],
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
                parked: false,
                parked_at: None,
                invocation: super::super::frontmatter::InvocationPolicy::Both,
            }],
            projects: Vec::new(),
            invocations: Vec::new(),
            heatmap: InvocationHeatmap::default(),
            scanned_at: Utc::now().to_rfc3339(),
            last_test_by_skill: Default::default(),
            update_check: Default::default(),
            opencode_config_kind: None,
            scan_partial: false,
            scan_observations: Vec::new(),
            unread_roots: Vec::new(),
        }
    }

    fn installed_skill_fixture(name: &str) -> super::super::skill_dto::InstalledSkill {
        use super::super::SourceKind;
        use chrono::Utc;

        super::super::skill_dto::InstalledSkill {
            name: name.to_string(),
            source: "manual".to_string(),
            source_type: "manual".to_string(),
            source_url: None,
            skill_path: None,
            installed_at: Utc::now().to_rfc3339(),
            updated_at: None,
            has_update: true,
            update_owner_ids: vec![
                "skills-sh/global".to_string(),
                "dotagents/global".to_string(),
            ],
            update_owners: vec![
                super::super::skill_dto::OwnerUpdateInfo {
                    owner_id: "skills-sh/global".to_string(),
                    latest_commit: Some("aaa1111".to_string()),
                    latest_commit_at: None,
                },
                super::super::skill_dto::OwnerUpdateInfo {
                    owner_id: "dotagents/global".to_string(),
                    latest_commit: Some("bbb2222".to_string()),
                    latest_commit_at: None,
                },
            ],
            update_commit: Some("aaa1111".to_string()),
            update_commit_at: None,
            source_kind: SourceKind::Manual,
            deployments: Vec::new(),
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
            invocation: super::super::frontmatter::InvocationPolicy::Both,
        }
    }

    fn discovered_dotagents_snapshot(
        home: &Path,
        projects: &[PathBuf],
    ) -> skill_refresh::SkillSnapshot {
        let update_check_path = home.join("core-data/update-check.json");
        let core_skills = super::super::skill_refresh::core_scan_installed_skills(
            home,
            projects,
            &update_check_path,
            &[],
        );
        let lock = skill_studio_core::lock_file::SkillLockFile {
            version: 3,
            skills: Default::default(),
        };
        let skills =
            super::super::skill_assembly::assemble_installed_skills(&core_skills.skills, &lock);
        let mut snapshot = fixture_snapshot(home, None);
        snapshot.skills = skills;
        snapshot
    }

    fn propagated_link_target_fixture(
        root: &Path,
    ) -> (skill_refresh::SkillSnapshot, LifecycleTarget) {
        use super::super::skill_deployment::{
            deployment_id, BackingRelationship, DeploymentMutability, SkillDestination,
        };
        use super::super::skill_ownership::LifecycleOwnerKind;

        let linked_path = root.join(".codex/skills/foo");
        std::fs::create_dir_all(&linked_path).unwrap();
        let linked_id = deployment_id(
            "foo",
            "global",
            SkillDestination::PerHarness,
            "codex",
            None,
            &linked_path,
        );
        let mut snapshot = fixture_snapshot(&linked_path, None);
        snapshot.skills[0].deployments[0] = super::super::skill_dto::Deployment {
            id: linked_id.clone(),
            destination: SkillDestination::PerHarness,
            owner_kind: LifecycleOwnerKind::SkillsSh,
            owner_id: Some("owner:v1/global/foo".to_string()),
            mutability: DeploymentMutability::ReadOnly,
            backing: BackingRelationship::LinkedTo {
                deployment_id: "dep:v1/global/universal/universal/foo/-".to_string(),
            },
            agent: "Codex".to_string(),
            scope: "global".to_string(),
            path: linked_path.to_string_lossy().to_string(),
            ..Default::default()
        };
        (
            snapshot,
            LifecycleTarget {
                deployment_id: Some(linked_id),
                owner_id: None,
            },
        )
    }

    /// A snapshot with one skill owned by a single mutable owner (no
    /// direct-deployment target), matching what `lifecycleTargetForSkill`
    /// sends for Fork, `SkillsSh` and Dotagents owners: `{ owner_id }` with
    /// `deployment_id` absent.
    fn single_owner_target_fixture(root: &Path) -> (skill_refresh::SkillSnapshot, LifecycleTarget) {
        use super::super::skill_deployment::{
            deployment_id, BackingRelationship, DeploymentMutability, SkillDestination,
        };
        use super::super::skill_ownership::LifecycleOwnerKind;

        let dep_dir = root.join(".agents/skills/foo");
        std::fs::create_dir_all(&dep_dir).unwrap();
        std::fs::write(dep_dir.join("SKILL.md"), "---\nname: foo\n---\n").unwrap();
        let content_hash =
            crate::skills::core_content_hash::live_skill_content_hash(&dep_dir).unwrap();
        let id = deployment_id(
            "foo",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            &dep_dir,
        );
        let owner_id = "owner:v1/global/foo".to_string();

        let mut snapshot = fixture_snapshot(&dep_dir, None);
        snapshot.skills[0].deployments[0] = super::super::skill_dto::Deployment {
            id: id.clone(),
            destination: SkillDestination::Universal,
            owner_kind: LifecycleOwnerKind::SkillsSh,
            owner_id: Some(owner_id.clone()),
            mutability: DeploymentMutability::Mutable,
            backing: BackingRelationship::Canonical,
            agent: "shared".to_string(),
            scope: "global".to_string(),
            path: dep_dir.to_string_lossy().to_string(),
            content_hash,
            ..Default::default()
        };
        (
            snapshot,
            LifecycleTarget {
                deployment_id: None,
                owner_id: Some(owner_id),
            },
        )
    }

    fn run_counted_lifecycle_command(
        snapshot: &skill_refresh::SkillSnapshot,
        target: &LifecycleTarget,
        action: &str,
        runner: &dyn CommandRunner,
    ) -> Result<(), String> {
        with_authorized_lifecycle_command_target(snapshot, target, action, |_, _| {
            runner.run_npx(&["skills".to_string(), action.to_lowercase()], None)
        })
    }

    #[test]
    fn remove_command_rejects_propagated_link_without_invoking_runner() {
        let tmp = tempfile::tempdir().unwrap();
        let (snapshot, target) = propagated_link_target_fixture(tmp.path());
        let runner = CountingLifecycleRunner(std::sync::atomic::AtomicUsize::new(0));

        let error =
            run_counted_lifecycle_command(&snapshot, &target, "Remove", &runner).unwrap_err();

        assert!(error.contains("read-only"));
        assert_eq!(runner.0.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn update_command_rejects_propagated_link_without_invoking_runner() {
        let tmp = tempfile::tempdir().unwrap();
        let (snapshot, target) = propagated_link_target_fixture(tmp.path());
        let runner = CountingLifecycleRunner(std::sync::atomic::AtomicUsize::new(0));

        let error =
            run_counted_lifecycle_command(&snapshot, &target, "Update", &runner).unwrap_err();

        assert!(error.contains("read-only"));
        assert_eq!(runner.0.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn write_refused_for_plugin_deployment() {
        let tmp = tempfile::tempdir().unwrap();
        let dep_dir = tmp.path().join("foo");
        std::fs::create_dir_all(&dep_dir).unwrap();
        let skill_md = dep_dir.join("SKILL.md");
        std::fs::write(&skill_md, "original").unwrap();

        let plugin = super::super::skill_dto::PluginInfo {
            name: "openai-templates".to_string(),
            version: Some("1.0.0".to_string()),
            harness: "Codex".to_string(),
            marketplace: "some-marketplace".to_string(),
            id: "openai-templates@some-marketplace".to_string(),
        };
        let snapshot = fixture_snapshot(&dep_dir, Some(plugin));

        let err = check_skill_md_write_allowed(Some(&snapshot), &skill_md).unwrap_err();
        assert!(err.contains("managed by a plugin"));
    }

    #[test]
    fn write_refused_for_non_owned_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dep_dir = tmp.path().join("foo");
        std::fs::create_dir_all(&dep_dir).unwrap();
        std::fs::write(dep_dir.join("SKILL.md"), "original").unwrap();
        let outside = tmp.path().join("outside").join("SKILL.md");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        std::fs::write(&outside, "original").unwrap();

        let snapshot = fixture_snapshot(&dep_dir, None);

        let err = check_skill_md_write_allowed(Some(&snapshot), &outside).unwrap_err();
        assert!(err.contains("not an installed skill"));
    }

    #[test]
    fn write_succeeds_and_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let dep_dir = tmp.path().join("foo");
        std::fs::create_dir_all(&dep_dir).unwrap();
        let skill_md = dep_dir.join("SKILL.md");
        std::fs::write(&skill_md, "original").unwrap();

        let snapshot = fixture_snapshot(&dep_dir, None);
        assert!(check_skill_md_write_allowed(Some(&snapshot), &skill_md).is_ok());

        write_skill_md(&skill_md, "---\nname: foo\n---\nupdated body").unwrap();

        let round_tripped = std::fs::read_to_string(&skill_md).unwrap();
        assert_eq!(round_tripped, "---\nname: foo\n---\nupdated body");
    }

    #[test]
    fn atomic_write_round_trips_twice_in_a_row() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_md = tmp.path().join("SKILL.md");
        std::fs::write(&skill_md, "original").unwrap();

        write_skill_md(&skill_md, "first save").unwrap();
        assert_eq!(std::fs::read_to_string(&skill_md).unwrap(), "first save");

        write_skill_md(&skill_md, "second save").unwrap();
        assert_eq!(std::fs::read_to_string(&skill_md).unwrap(), "second save");
    }

    #[test]
    fn atomic_write_leaves_no_temp_file_on_failed_rename() {
        let tmp = tempfile::tempdir().unwrap();
        // `canonical` names a directory, not a file: the rename onto it fails,
        // and the temp file created alongside it must not survive.
        let canonical = tmp.path().join("SKILL.md");
        std::fs::create_dir_all(&canonical).unwrap();

        let err = write_skill_md(&canonical, "content");
        assert!(err.is_err());

        let leftover_temp_files = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".SKILL.md.tmp-")
            })
            .count();
        assert_eq!(leftover_temp_files, 0);
    }

    #[test]
    fn compare_and_swap_refuses_mismatch_and_leaves_file_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_md = tmp.path().join("SKILL.md");
        std::fs::write(&skill_md, "on disk now").unwrap();

        let err =
            write_skill_md_compare_and_swap(&skill_md, "stale copy", "new content").unwrap_err();
        assert!(err.contains("changed on disk since it was loaded"));
        assert_eq!(std::fs::read_to_string(&skill_md).unwrap(), "on disk now");
    }

    #[test]
    fn compare_and_swap_writes_on_match() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_md = tmp.path().join("SKILL.md");
        std::fs::write(&skill_md, "on disk now").unwrap();

        write_skill_md_compare_and_swap(&skill_md, "on disk now", "new content").unwrap();
        assert_eq!(std::fs::read_to_string(&skill_md).unwrap(), "new content");
    }

    #[test]
    fn concurrent_compare_and_swap_allows_only_one_matching_write() {
        use std::sync::{mpsc, Arc};

        let tmp = tempfile::tempdir().unwrap();
        let skill_md = Arc::new(tmp.path().join("SKILL.md"));
        std::fs::write(skill_md.as_ref(), "shared baseline").unwrap();

        let first_path = Arc::clone(&skill_md);
        let (first_compared_tx, first_compared_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let first = std::thread::spawn(move || {
            super::super::skill_md_write::write_skill_md_compare_and_swap_with(
                &first_path,
                "shared baseline",
                "first write",
                || {
                    let _ = first_compared_tx.send(());
                    let _ = release_first_rx.recv();
                },
            )
        });

        first_compared_rx.recv().unwrap();
        let lock_was_held_across_compare =
            super::super::skill_md_write::skill_md_write_transaction_is_held();
        let second_path = Arc::clone(&skill_md);
        let second = std::thread::spawn(move || {
            write_skill_md_compare_and_swap(&second_path, "shared baseline", "second write")
        });
        release_first_tx.send(()).unwrap();

        let results = [first.join().unwrap(), second.join().unwrap()];
        assert!(lock_was_held_across_compare);
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        assert_eq!(
            std::fs::read_to_string(skill_md.as_ref()).unwrap(),
            "first write"
        );
    }

    #[test]
    fn update_from_the_desktop_clears_the_outdated_flag_without_a_full_rescan_or_names_the_stale_row(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let app_data = tmp.path().join("app-data");
        std::fs::create_dir_all(&app_data).unwrap();

        let mut skill = installed_skill_fixture("alpha");
        skill.deployments = vec![
            super::super::skill_dto::Deployment {
                owner_id: Some("skills-sh/global".to_string()),
                agent: "shared".to_string(),
                scope: "global".to_string(),
                path: tmp.path().join("alpha").to_string_lossy().to_string(),
                ..Default::default()
            },
            super::super::skill_dto::Deployment {
                owner_id: Some("dotagents/global".to_string()),
                agent: "shared".to_string(),
                scope: "global".to_string(),
                path: tmp.path().join("alpha").to_string_lossy().to_string(),
                ..Default::default()
            },
        ];
        assert!(skill.has_update);
        let snapshot = skill_refresh::SkillSnapshot {
            revision: 0,
            skills: vec![skill],
            projects: Vec::new(),
            invocations: Vec::new(),
            heatmap: Default::default(),
            scanned_at: chrono::Utc::now().to_rfc3339(),
            last_test_by_skill: Default::default(),
            update_check: Default::default(),
            opencode_config_kind: None,
            scan_partial: false,
            scan_observations: Vec::new(),
            unread_roots: Vec::new(),
        };
        let refresh_state = skill_refresh::SkillRefreshState::fixture(snapshot);
        // Both owners, the way `update_skill`/`update_all_skills` pass the
        // fresh snapshot's own owner ids rather than re-reading `refresh_state`.
        let current_owner_ids = vec![
            "skills-sh/global".to_string(),
            "dotagents/global".to_string(),
        ];

        // Seed the on-disk store the way the background loop's last check
        // would have left it before `update_skill` ran: both owners outdated.
        let store_state = |installed: &str, latest: &str, error: Option<&str>| {
            skill_update_check::SkillUpdateState {
                repo: "obra/write-tests".to_string(),
                path: "skills/alpha".to_string(),
                installed_commit: Some(installed.to_string()),
                latest_commit: Some(latest.to_string()),
                latest_commit_at: None,
                checked_at: chrono::Utc::now().to_rfc3339(),
                error: error.map(str::to_string),
            }
        };
        let store = skill_update_check::UpdateCheckStore {
            version: 2,
            checked_at: Some(chrono::Utc::now().to_rfc3339()),
            gh_status: skill_update_check::GhStatus::Ok,
            owners: [
                (
                    "skills-sh/global".to_string(),
                    // A stale error from a prior failed check, still on
                    // this owner's record when the update that just
                    // succeeded runs - N2 (review round 3) clears it.
                    store_state("old-a", "new-a", Some("gh: rate limited")),
                ),
                (
                    "dotagents/global".to_string(),
                    store_state("old-b", "new-b", None),
                ),
            ]
            .into_iter()
            .collect(),
            legacy_skills: Default::default(),
        };
        let update_check_path = skill_update_check::update_check_path(&app_data);
        std::fs::create_dir_all(update_check_path.parent().unwrap()).unwrap();
        std::fs::write(&update_check_path, serde_json::to_string(&store).unwrap()).unwrap();

        let built = clear_outdated_state(
            &app_data,
            &refresh_state,
            "alpha",
            Some("skills-sh/global"),
            &current_owner_ids,
        )
        .unwrap()
        .expect("a snapshot existed to patch");
        assert_eq!(
            built.skills[0].update_owner_ids,
            vec!["dotagents/global".to_string()]
        );
        let refreshed = skill_update_check::read_update_check_store(&app_data);
        assert_eq!(
            refreshed
                .owners
                .get("skills-sh/global")
                .and_then(|s| s.error.as_deref()),
            None,
            "a successful update must clear the stale error on the rewritten entry"
        );
        assert!(
            built.skills[0].has_update,
            "a second still-outdated owner must keep the badge on"
        );

        let built = clear_outdated_state(
            &app_data,
            &refresh_state,
            "alpha",
            Some("dotagents/global"),
            &current_owner_ids,
        )
        .unwrap()
        .expect("a snapshot existed to patch");
        assert!(built.skills[0].update_owner_ids.is_empty());
        assert!(
            !built.skills[0].has_update,
            "clearing the last outdated owner must drop the badge"
        );

        // Rebuild overlays straight from the store, the way the next full
        // `get_installed_skills` would - this is the B1 assertion.
        let refreshed_store = skill_update_check::read_update_check_store(&app_data);
        let mut skills = built.skills.clone();
        skill_refresh::apply_skill_snapshot_overlays(
            tmp.path(),
            &mut skills,
            &super::super::skill_fork_registry::ForkRegistry::default(),
            &refreshed_store,
            &[],
        );
        assert!(
            !skills[0].has_update,
            "the badge must not reappear on the next full rebuild after B1's store patch"
        );
    }

    /// `update_on_a_migrated_v1_store_keeps_the_badge_off_or_names_the_resurrected_owner`
    /// (B2, review round 2): round 1's `clear_owner_after_update` removed the
    /// `owners` entry, which did nothing when a migrated v1 store's
    /// background loop hadn't run against this owner yet - the only record
    /// was `legacy_skills["alpha"]`, still holding the pre-update pair, and
    /// `state_for_owner`'s sole-Global-owner fallback kept serving it. Seeds
    /// exactly that: an empty `owners` map and a `legacy_skills` entry for
    /// the sole Global owner. Without B2's fix (falling back to the legacy
    /// record and writing it into `owners[owner_id]` with
    /// `installed_commit = latest_commit`), the rebuilt overlay below
    /// recomputes `has_update = true` from the untouched legacy pair and
    /// this is the assertion that fails.
    #[test]
    fn update_on_a_migrated_v1_store_keeps_the_badge_off_or_names_the_resurrected_owner() {
        let tmp = tempfile::tempdir().unwrap();
        let app_data = tmp.path().join("app-data");
        std::fs::create_dir_all(&app_data).unwrap();

        // A real `owner:v1/...` id, not the shorthand the sibling test
        // above uses - `state_for_owner`'s legacy fallback only fires for
        // an id `parse_owner_id` can actually parse.
        let owner_id = "owner:v1/global/alpha";
        let mut skill = installed_skill_fixture("alpha");
        skill.deployments = vec![super::super::skill_dto::Deployment {
            owner_id: Some(owner_id.to_string()),
            agent: "shared".to_string(),
            scope: "global".to_string(),
            path: tmp.path().join("alpha").to_string_lossy().to_string(),
            ..Default::default()
        }];
        // Fixture's default is two owners - trim to the sole owner this
        // test's single deployment actually has.
        skill.update_owner_ids = vec![owner_id.to_string()];
        skill
            .update_owners
            .retain(|update| update.owner_id == owner_id);
        let snapshot = skill_refresh::SkillSnapshot {
            revision: 0,
            skills: vec![skill],
            projects: Vec::new(),
            invocations: Vec::new(),
            heatmap: Default::default(),
            scanned_at: chrono::Utc::now().to_rfc3339(),
            last_test_by_skill: Default::default(),
            update_check: Default::default(),
            opencode_config_kind: None,
            scan_partial: false,
            scan_observations: Vec::new(),
            unread_roots: Vec::new(),
        };
        let refresh_state = skill_refresh::SkillRefreshState::fixture(snapshot);
        // The sole owner, the way `update_skill`/`update_all_skills` pass the
        // fresh snapshot's own owner ids rather than re-reading `refresh_state`.
        let current_owner_ids = vec![owner_id.to_string()];

        // A genuine migrated v1 store on disk: no top-level `"owners"` key,
        // only the legacy name-keyed pair - the same shape
        // `read_update_check_store_at` detects and migrates into
        // `legacy_skills`. Building this via `UpdateCheckStore` and
        // `serde_json::to_string` would not do, since `legacy_skills` is
        // `#[serde(skip)]` and would round-trip empty.
        let update_check_path = skill_update_check::update_check_path(&app_data);
        std::fs::create_dir_all(update_check_path.parent().unwrap()).unwrap();
        std::fs::write(
            &update_check_path,
            serde_json::json!({
                "checked_at": "2026-01-01T00:00:00Z",
                "gh_status": {"kind": "ok"},
                "skills": {
                    "alpha": {
                        "repo": "obra/write-tests",
                        "path": "skills/alpha",
                        "installed_commit": "old-a",
                        "latest_commit": "new-a",
                        "latest_commit_at": null,
                        "checked_at": "2026-01-01T00:00:00Z",
                        "error": null
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let built = clear_outdated_state(
            &app_data,
            &refresh_state,
            "alpha",
            Some(owner_id),
            &current_owner_ids,
        )
        .unwrap()
        .expect("a snapshot existed to patch");
        assert!(!built.skills[0].has_update);

        // Rebuild overlays straight from the store, the way the next full
        // `get_installed_skills` would - the sole-Global-owner fallback
        // `state_for_owner` runs needs the owner id present in
        // `current_owner_ids` to count it as the only matching owner.
        let refreshed_store = skill_update_check::read_update_check_store(&app_data);
        let mut skills = built.skills.clone();
        skill_refresh::apply_skill_snapshot_overlays(
            tmp.path(),
            &mut skills,
            &super::super::skill_fork_registry::ForkRegistry::default(),
            &refreshed_store,
            &[owner_id.to_string()],
        );
        assert!(
            !skills[0].has_update,
            "the pre-update legacy pair must not resurrect the badge on the next full rebuild"
        );
    }

    /// `update_for_a_project_owner_does_not_write_a_global_legacy_record_or_names_the_borrowed_state`
    /// (N2, review round 3): the round-2 `clear_owner_after_update` fell
    /// back to `legacy_skills[skill_name]` for *any* owner with no `owners`
    /// entry, wider than `state_for_owner`'s read-side fallback (sole
    /// matching Global owner only). Seeds a Project owner with no `owners`
    /// entry and a `legacy_skills["alpha"]` record a pre-3.6b Global-only
    /// checker wrote - a real shape, since the legacy store never
    /// distinguished scope. Without N2's fix this owner would get that
    /// Global-scoped commit pair written into `owners[owner_id]`, in effect
    /// reading a Global check result for a Project deployment. Asserts the
    /// call is a no-op instead: `owners` gains no entry for the Project id.
    #[test]
    fn update_for_a_project_owner_does_not_write_a_global_legacy_record_or_names_the_borrowed_state(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let app_data = tmp.path().join("app-data");
        std::fs::create_dir_all(&app_data).unwrap();

        let owner_id = "owner:v1/project/-/alpha";
        let update_check_path = skill_update_check::update_check_path(&app_data);
        std::fs::create_dir_all(update_check_path.parent().unwrap()).unwrap();
        // A genuine migrated v1 store: no `owners` entry for this Project
        // owner, only the legacy name-keyed pair a Global-only checker left
        // behind.
        std::fs::write(
            &update_check_path,
            serde_json::json!({
                "checked_at": "2026-01-01T00:00:00Z",
                "gh_status": {"kind": "ok"},
                "skills": {
                    "alpha": {
                        "repo": "obra/write-tests",
                        "path": "skills/alpha",
                        "installed_commit": "old-a",
                        "latest_commit": "new-a",
                        "latest_commit_at": null,
                        "checked_at": "2026-01-01T00:00:00Z",
                        "error": null
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        skill_update_check::clear_owner_after_update(&app_data, owner_id, &[owner_id.to_string()])
            .unwrap();

        let store = skill_update_check::read_update_check_store(&app_data);
        assert!(
            !store.owners.contains_key(owner_id),
            "a Project owner must not inherit a Global-scoped legacy record: {:?}",
            store.owners
        );
    }

    #[test]
    fn build_update_request_refuses_the_three_desktop_preconditions_or_names_the_accepted_owner() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("app-data");
        std::fs::create_dir_all(&app_data).unwrap();
        let _home_guard = super::super::test_support::HomeGuard::new(&home);

        let agents_dir = home.join(".agents");
        for name in ["declared", "wildcard", "pinned"] {
            std::fs::create_dir_all(agents_dir.join("skills").join(name)).unwrap();
            std::fs::write(
                agents_dir.join("skills").join(name).join("SKILL.md"),
                "body",
            )
            .unwrap();
        }
        // "wildcard" has an agents.lock row but no `[[skills]]` manifest row
        // (has_manifest_row == false). "pinned" is declared with a `ref`
        // (needs a `latest_commit` from "Check now" before it can update).
        std::fs::write(
            agents_dir.join("agents.toml"),
            "[[skills]]\nname = \"declared\"\nsource = \"o/r\"\n\n[[skills]]\nname = \"pinned\"\nsource = \"o/r\"\nref = \"deadbeef\"\n",
        )
        .unwrap();
        std::fs::write(
            agents_dir.join("agents.lock"),
            "[skills.declared]\nsource = \"o/r\"\nresolved_path = \"skills/declared\"\nresolved_commit = \"aaa\"\n\
             [skills.wildcard]\nsource = \"o/r\"\nresolved_path = \"skills/wildcard\"\nresolved_commit = \"bbb\"\n\
             [skills.pinned]\nsource = \"o/r\"\nresolved_path = \"skills/pinned\"\nresolved_commit = \"ccc\"\n",
        )
        .unwrap();

        let snapshot = discovered_dotagents_snapshot(&home, &[]);
        // `classify_owner` (core) already routes a real wildcard scan to
        // `WildcardDotagents`, not `Dotagents` - so its `has_manifest_row`
        // check never fires from a live scan. It is still a real desktop
        // precondition (defends against a hand-built or stale `Dotagents`
        // deployment pointing at a wildcard ledger row), so this test
        // drives it directly with an explicit `owner_kind: Dotagents`
        // deployment rather than one `discovered_dotagents_snapshot` scanned.
        let declared_id = snapshot
            .skills
            .iter()
            .find(|s| s.name == "declared")
            .unwrap()
            .deployments
            .iter()
            .find(|d| d.owner_kind == super::super::skill_ownership::LifecycleOwnerKind::Dotagents)
            .unwrap()
            .id
            .clone();
        let dotagents_deployment_for = |name: &str| -> super::super::skill_dto::Deployment {
            super::super::skill_dto::Deployment {
                id: declared_id.clone(),
                owner_kind: super::super::skill_ownership::LifecycleOwnerKind::Dotagents,
                owner_id: Some(format!("owner:v1/global/{name}")),
                agent: "shared".to_string(),
                scope: "global".to_string(),
                path: agents_dir
                    .join("skills")
                    .join(name)
                    .to_string_lossy()
                    .to_string(),
                ..Default::default()
            }
        };

        // 1. Not in the matching agents.lock: a valid ledger match, but a
        // skill name the ledger has no entry for.
        let missing = installed_skill_fixture("missing");
        let err = build_update_request(
            &app_data,
            &snapshot,
            &missing,
            &dotagents_deployment_for("missing"),
        )
        .unwrap_err();
        assert!(err.contains("not in the matching agents.lock"), "{err}");

        // 2. Wildcard dotagents entry (has_manifest_row == false).
        let wildcard = installed_skill_fixture("wildcard");
        let err = build_update_request(
            &app_data,
            &snapshot,
            &wildcard,
            &dotagents_deployment_for("wildcard"),
        )
        .unwrap_err();
        assert!(err.contains("wildcard dotagents entry"), "{err}");

        // 3. Pinned entry needs "Check now": no update-check store entry
        // yet, so there is no `latest_commit` to pin the update to.
        let pinned = installed_skill_fixture("pinned");
        let err = build_update_request(
            &app_data,
            &snapshot,
            &pinned,
            &dotagents_deployment_for("pinned"),
        )
        .unwrap_err();
        assert!(err.contains("Check now"), "{err}");

        // 4. Accepted: a `SkillsSh`-owned deployment never touches the
        // ledger or the update-check store, so it always builds a request.
        let accepted_skill = installed_skill_fixture("accepted");
        let accepted_deployment = super::super::skill_dto::Deployment {
            owner_kind: super::super::skill_ownership::LifecycleOwnerKind::SkillsSh,
            owner_id: Some("owner:v1/global/accepted".to_string()),
            agent: "shared".to_string(),
            scope: "global".to_string(),
            path: home.join("accepted").to_string_lossy().to_string(),
            ..Default::default()
        };
        let req = build_update_request(&app_data, &snapshot, &accepted_skill, &accepted_deployment)
            .unwrap_or_else(|e| panic!("SkillsSh owner must be accepted, got: {e}"));
        assert_eq!(req.skill.0, "accepted");
    }

    /// A `ProcessSpawner`/runtime-builder pair for the thread-recording
    /// test below - mirrors `harness_first_run.rs`'s
    /// `ThreadRecordingSpawner`/`detect_with_runtime` test, adapted to
    /// `update_all_with_runtime`'s runtime-builder closure: `Copy` needs no
    /// spawner, so the closure itself is what records the pool thread.
    fn copy_update_request(
        skill: &str,
        scope: skill_studio_core::identity::RootScope,
        body: &[u8],
    ) -> skill_studio_core::dto::UpdateRequest {
        skill_studio_core::dto::UpdateRequest {
            skill: skill_studio_core::identity::SkillName(skill.to_string()),
            method: skill_studio_core::dto::InstallMethod::Copy,
            scope,
            files: vec![skill_studio_core::dto::InstallFile {
                relative_path: PathBuf::from("SKILL.md"),
                contents: body.to_vec(),
            }],
            source: None,
            ref_pin: None,
        }
    }

    /// `update_all_on_ten_outdated_fixtures_ends_with_ten_journal_entries_and_zero_main_thread_calls_over_one_frame`:
    /// installs ten `Copy` skills for real (so `update_all_with_runtime` has
    /// an existing deployment per skill to swap over), then updates all ten
    /// in the one `spawn_blocking` task `update_all_with_runtime` wraps its
    /// `ops::update_all` call in. Under a `current_thread` runtime the test
    /// task's own thread is the only async worker, so a recorded thread
    /// differing from it proves the write ran off the UI/test task - a
    /// deterministic fact, not a timing measurement. B3 (review round 1):
    /// the runtime-builder closure alone survived moving `ops::update_all`
    /// out of `spawn_blocking`, because it recorded its own thread inside
    /// `build_runtime`, not inside the op call itself - so this now also
    /// threads a recorder through `on_outcome`, which `ops::update_all`
    /// calls once per request, and asserts all ten land off the test task
    /// too. The ten resulting `update` journal rows are counted straight
    /// from the events store, independent of the returned `UpdateAllOutcome`.
    #[tokio::test(flavor = "current_thread")]
    async fn update_all_on_ten_outdated_fixtures_ends_with_ten_journal_entries_and_zero_main_thread_calls_over_one_frame(
    ) {
        use skill_studio_core::dto::{InstallFile, InstallMethod, InstallRequest};
        use skill_studio_core::identity::{RootScope, SkillName};
        use skill_studio_core::ops::{self, Operation, ResultEnvelope};
        use skill_studio_core::ports::OpContext;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let data_root = tmp.path().join("data");
        std::fs::create_dir_all(&home).unwrap();

        // The process's own PATH, not a real login-shell probe: this
        // fixture never spawns `npx`, so it shouldn't pay for (or risk
        // hanging on, once per skill below) a real `$SHELL -lic` spawn.
        let rt = super::super::core_runtime::build_runtime_write_at_with_search_dirs(
            &home,
            &data_root,
            super::super::core_runtime::process_path_search_dirs(),
        )
        .unwrap();
        let ctx = OpContext::uncancellable(skill_studio_core::identity::CorrelationId(
            ulid::Ulid::new().to_string(),
        ));
        for i in 0..10 {
            let req = InstallRequest {
                skill: SkillName(format!("fixture-{i}")),
                method: InstallMethod::Copy,
                scope: RootScope::Global,
                harnesses: Vec::new(),
                files: vec![InstallFile {
                    relative_path: PathBuf::from("SKILL.md"),
                    contents: b"original".to_vec(),
                }],
                source: None,
                trust_identity: None,
                trust_confirmed: false,
                save_as_preference: false,
            };
            let result = ops::install(&rt, &ctx, &req);
            let envelope = ResultEnvelope::from_result(Operation::Install, &rt.scope, &ctx, result);
            super::super::core_runtime::to_command_result(envelope)
                .unwrap_or_else(|e| panic!("fixture install {i} failed: {e}"));
        }

        let requests: Vec<_> = (0..10)
            .map(|i| copy_update_request(&format!("fixture-{i}"), RootScope::Global, b"updated"))
            .collect();

        let test_task_thread = std::thread::current().id();
        let runtime_built_on = std::sync::Arc::new(std::sync::Mutex::new(None));
        let record_build_thread = runtime_built_on.clone();
        let home_for_closure = home.clone();
        let data_root_for_closure = data_root.clone();
        let outcome_threads = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let record_outcome_thread = outcome_threads.clone();

        let outcome = update_all_with_runtime(
            requests,
            move || {
                *record_build_thread.lock().unwrap() = Some(std::thread::current().id());
                super::super::core_runtime::build_runtime_write_at_with_search_dirs(
                    &home_for_closure,
                    &data_root_for_closure,
                    super::super::core_runtime::process_path_search_dirs(),
                )
            },
            move |_, _| {
                record_outcome_thread
                    .lock()
                    .unwrap()
                    .push(std::thread::current().id());
            },
        )
        .await
        .unwrap();

        assert_eq!(outcome.items.len(), 10);
        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);

        let built_on = runtime_built_on.lock().unwrap().expect("runtime was built");
        assert_ne!(
            built_on, test_task_thread,
            "update_all_with_runtime must build the runtime and run ops::update_all \
             on a spawn_blocking pool thread, not the calling task"
        );

        let outcome_threads = outcome_threads.lock().unwrap();
        assert_eq!(outcome_threads.len(), 10, "{outcome_threads:?}");
        assert!(
            outcome_threads.iter().all(|id| *id != test_task_thread),
            "every on_outcome call must land on the spawn_blocking pool thread, \
             not the calling task: {outcome_threads:?}"
        );

        let history = rt
            .ports
            .history
            .open(
                &rt.scope,
                skill_studio_core::ports::HistoryAccess::ReadIfExists,
            )
            .unwrap()
            .expect("history store exists after the fixture installs");
        let rows = history
            .list(&skill_studio_core::events::EventFilter {
                skill: None,
                limit: 100,
                after: None,
            })
            .unwrap();
        let update_rows = rows
            .iter()
            .filter(|row| row.kind == skill_studio_core::events::EventKind::Update.as_str())
            .count();
        assert_eq!(
            update_rows,
            10,
            "{:?}",
            rows.iter().map(|r| &r.kind).collect::<Vec<_>>()
        );
    }

    #[test]
    fn update_all_leaves_the_badge_on_a_failed_item_or_names_the_cleared_row() {
        use skill_studio_core::dto::{UpdateAllItem, UpdateAllOutcome, UpdateOutcome};
        use skill_studio_core::identity::SkillName;

        let succeeded = UpdateOutcome {
            event_id: skill_studio_core::identity::EventId("evt-alpha".to_string()),
            skill: SkillName("alpha".to_string()),
            deployment_path: PathBuf::from("/home/.agents/skills/alpha"),
            tree_hash_before: "aaa".to_string(),
            tree_hash_after: "bbb".to_string(),
        };
        let outcome = UpdateAllOutcome {
            items: vec![
                UpdateAllItem {
                    skill: SkillName("alpha".to_string()),
                    outcome: Some(succeeded),
                },
                UpdateAllItem {
                    skill: SkillName("beta".to_string()),
                    outcome: None,
                },
            ],
            errors: std::collections::BTreeMap::from([(
                "beta".to_string(),
                "update failed".to_string(),
            )]),
        };
        let owners = vec![
            (
                "alpha".to_string(),
                Some("owner:v1/global/alpha".to_string()),
                PathBuf::from("/home/.agents/skills/alpha"),
            ),
            (
                "beta".to_string(),
                Some("owner:v1/global/beta".to_string()),
                PathBuf::from("/home/.agents/skills/beta"),
            ),
        ];

        let cleared = owners_to_clear(&outcome, &owners);

        assert_eq!(
            cleared,
            vec![(
                "alpha".to_string(),
                Some("owner:v1/global/alpha".to_string())
            )],
            "beta's failed item must not clear its badge: {cleared:?}"
        );
    }

    /// `update_all_clears_both_owners_of_a_skill_installed_twice_or_names_the_owner_left_outdated`
    /// (B1, review round 2): `skillUpdateOwnerTargets` sends one target per
    /// outdated owner, so a skill outdated for two owners (e.g. a
    /// Global copy and a Project copy of the same name) produces two
    /// requests sharing one `SkillName`, and `ops::update_all` returns two
    /// `UpdateAllItem`s with that same shared name. A name-keyed lookup map
    /// collapses those two owners to one entry, so the first owner's badge
    /// never clears; matching each item to its owner by `deployment_path`
    /// (B1, review round 3) keeps them apart since each owner's deployment
    /// lives at its own path. The two owners are a Global one and a Project
    /// one, each with its own `.agents/skills` root - real owner ids
    /// (`owner:v1/<scope>/[project/]<name>`, round 4 post-verdict fix) can't
    /// express two *Global* owners of one name, since scope plus name is
    /// the whole id there.
    #[test]
    fn update_all_clears_both_owners_of_a_skill_installed_twice_or_names_the_owner_left_outdated() {
        use skill_studio_core::dto::{UpdateAllItem, UpdateAllOutcome, UpdateOutcome};
        use skill_studio_core::identity::SkillName;

        let project_path = "/home/project-two";
        let project_owner_id = format!(
            "owner:v1/project/{}/alpha",
            super::super::skill_deployment::encode_id_path(project_path)
        );

        let outcome_for = |suffix: &str, deployment_path: &str| UpdateOutcome {
            event_id: skill_studio_core::identity::EventId(format!("evt-{suffix}")),
            skill: SkillName("alpha".to_string()),
            deployment_path: PathBuf::from(deployment_path),
            tree_hash_before: "aaa".to_string(),
            tree_hash_after: "bbb".to_string(),
        };
        let outcome = UpdateAllOutcome {
            items: vec![
                UpdateAllItem {
                    skill: SkillName("alpha".to_string()),
                    outcome: Some(outcome_for("global", "/home/.agents/skills/alpha")),
                },
                UpdateAllItem {
                    skill: SkillName("alpha".to_string()),
                    outcome: Some(outcome_for(
                        "project",
                        "/home/project-two/.agents/skills/alpha",
                    )),
                },
            ],
            errors: std::collections::BTreeMap::new(),
        };
        let owners = vec![
            (
                "alpha".to_string(),
                Some("owner:v1/global/alpha".to_string()),
                PathBuf::from("/home/.agents/skills/alpha"),
            ),
            (
                "alpha".to_string(),
                Some(project_owner_id.clone()),
                PathBuf::from("/home/project-two/.agents/skills/alpha"),
            ),
        ];

        let cleared = owners_to_clear(&outcome, &owners);

        assert_eq!(
            cleared,
            vec![
                (
                    "alpha".to_string(),
                    Some("owner:v1/global/alpha".to_string())
                ),
                ("alpha".to_string(), Some(project_owner_id)),
            ],
            "both owners of the twice-installed skill must clear: {cleared:?}"
        );
    }

    /// `update_all_clears_the_right_owner_when_items_finish_out_of_request_order_or_names_the_owner_left_outdated`
    /// (B1, review round 3): `dto.rs` documents `UpdateAllOutcome.items` as
    /// "in the order each one finished (not the order requested)". This
    /// hand-builds an outcome whose items are reversed relative to the
    /// request order to prove `owners_to_clear` still names the right owner
    /// - matching by `deployment_path` rather than zipping by index.
    #[test]
    fn update_all_clears_the_right_owner_when_items_finish_out_of_request_order_or_names_the_owner_left_outdated(
    ) {
        use skill_studio_core::dto::{UpdateAllItem, UpdateAllOutcome, UpdateOutcome};
        use skill_studio_core::identity::SkillName;

        let outcome_for = |name: &str, path: &str| UpdateOutcome {
            event_id: skill_studio_core::identity::EventId(format!("evt-{name}")),
            skill: SkillName(name.to_string()),
            deployment_path: PathBuf::from(path),
            tree_hash_before: "aaa".to_string(),
            tree_hash_after: "bbb".to_string(),
        };
        // Requested in order alpha, beta - `beta`'s update fails and
        // `alpha`'s succeeds, but `beta`'s (failed) item finishes first, so
        // `outcome.items` arrives reversed relative to the request. An
        // index zip would pair `beta`'s failed item with `alpha`'s request
        // (dropping it) and `alpha`'s succeeded item with `beta`'s request
        // (wrongly clearing `beta`'s badge instead of `alpha`'s).
        let outcome = UpdateAllOutcome {
            items: vec![
                UpdateAllItem {
                    skill: SkillName("beta".to_string()),
                    outcome: None,
                },
                UpdateAllItem {
                    skill: SkillName("alpha".to_string()),
                    outcome: Some(outcome_for("alpha", "/home/.agents/skills/alpha")),
                },
            ],
            errors: std::collections::BTreeMap::from([(
                "beta".to_string(),
                "update failed".to_string(),
            )]),
        };
        let owners = vec![
            (
                "alpha".to_string(),
                Some("owner:v1/global/alpha".to_string()),
                PathBuf::from("/home/.agents/skills/alpha"),
            ),
            (
                "beta".to_string(),
                Some("owner:v1/global/beta".to_string()),
                PathBuf::from("/home/.agents/skills/beta"),
            ),
        ];

        let cleared = owners_to_clear(&outcome, &owners);

        assert_eq!(
            cleared,
            vec![(
                "alpha".to_string(),
                Some("owner:v1/global/alpha".to_string())
            )],
            "beta's badge must stay on (its update failed); an index zip \
             clears beta's badge instead of alpha's: {cleared:?}"
        );
    }

    /// `remove_from_the_desktop_runs_the_op_on_a_blocking_thread_or_names_the_thread`:
    /// mirrors `harness_first_run.rs`'s
    /// `detect_runs_the_probes_on_a_blocking_thread_not_the_ui_task_or_names_the_task_it_blocks`.
    /// Under a `current_thread` runtime the test task's own thread is the
    /// only async worker, so the runtime builder must run somewhere else -
    /// `spawn_blocking`'s pool - for `remove_with_runtime` to be off the UI
    /// task. Fails if `remove_with_runtime` builds the runtime, or calls
    /// `ops::remove`, on the calling task instead of through
    /// `spawn_blocking`.
    #[tokio::test(flavor = "current_thread")]
    async fn remove_from_the_desktop_runs_the_op_on_a_blocking_thread_or_names_the_thread() {
        use skill_studio_core::harness::HarnessCatalog;
        use skill_studio_core::ports::{Ports, Runtime};
        use skill_studio_core::RuntimeScope;
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let lease_root = tmp.path().join("leases");
        let catalog = Arc::new(HarnessCatalog::builtin());
        let scope = RuntimeScope::fixture(home.clone());
        let db_path = scope.history_root.join("events.sqlite3");
        let ports: Ports =
            skill_studio_host::default_ports_with_history(lease_root, catalog, db_path);
        let rt = Runtime::new(&scope, ports).unwrap();

        let target = LifecycleTarget {
            deployment_id: Some("dep:v1/does-not-exist".to_string()),
            owner_id: None,
        };
        let test_task_thread = std::thread::current().id();
        let runtime_built_on: Arc<std::sync::Mutex<Option<std::thread::ThreadId>>> =
            Arc::new(std::sync::Mutex::new(None));
        let record_build_thread = runtime_built_on.clone();

        let _ = remove_with_runtime(
            target,
            || panic!("resolve_snapshot must not run for a direct deployment_id target"),
            move |_deployment_id| {
                *record_build_thread.lock().unwrap() = Some(std::thread::current().id());
                Ok(rt)
            },
        )
        .await;

        let build_thread = runtime_built_on
            .lock()
            .unwrap()
            .expect("the runtime builder never ran");
        assert_ne!(
            build_thread, test_task_thread,
            "remove_with_runtime built the runtime (and ran ops::remove) on the test task's own \
             thread ({test_task_thread:?}) instead of a spawn_blocking pool thread"
        );
    }

    /// Every UI remove path builds its target through
    /// `lifecycleTargetForSkill`, which sends `{ owner_id }` (no
    /// `deployment_id`) whenever the scope has one mutable owner - Fork,
    /// `SkillsSh` and Dotagents. `remove_with_runtime` must resolve that
    /// owner target against a fresh snapshot and hand the runtime builder
    /// the resolved deployment's id, not reject it for lacking one.
    #[tokio::test(flavor = "current_thread")]
    async fn remove_of_an_owner_target_resolves_the_canonical_deployment_or_names_the_missing_id() {
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let (snapshot, target) = single_owner_target_fixture(tmp.path());
        let expected_id = snapshot.skills[0].deployments[0].id.clone();

        let built_with_id: Arc<std::sync::Mutex<Option<String>>> =
            Arc::new(std::sync::Mutex::new(None));
        let record_id = built_with_id.clone();

        let _ = remove_with_runtime(
            target,
            move || Ok(snapshot),
            move |deployment_id| {
                *record_id.lock().unwrap() = Some(deployment_id.as_str().to_string());
                Err("stop before a real runtime is needed".to_string())
            },
        )
        .await;

        let resolved_id = built_with_id
            .lock()
            .unwrap()
            .clone()
            .expect("the runtime builder never ran, so the owner target was never resolved");
        assert_eq!(
            resolved_id, expected_id,
            "remove_with_runtime did not resolve the owner target to its deployment id"
        );
    }
}

/// Removes one deployment through `skill_studio_core::ops::remove` - the
/// desktop no longer walks its own Copy/Fork/Dotagents/SkillsSh branches;
/// `ops::remove` already knows how to quarantine (Copy, Fork) or shell out
/// (Dotagents, `SkillsSh`) for every owner kind, and prunes quarantine as a
/// side effect of the removal (unit 3.9b, mirroring `skill_park.rs`'s
/// adapters over `ops::park`/`ops::unpark`).
#[tauri::command]
pub async fn remove_skill(
    target: LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<RemoveOutcome, String> {
    let snapshot_app = app.clone();
    crate::timing_log::time_command_async(
        &app,
        "remove_skill",
        remove_with_runtime(
            target,
            move || {
                let refresh_state = snapshot_app.state::<SkillRefreshState>();
                rebuild_fresh_lifecycle_snapshot(&snapshot_app, &refresh_state)
            },
            |_deployment_id| super::core_runtime::build_runtime_write(),
        ),
    )
    .await
}

/// The command body, kept apart so the test that pins `ops::remove` to
/// `spawn_blocking` can run it without a `tauri::AppHandle` - same split
/// `harness_first_run.rs`'s `detect_with_runtime` uses. The runtime is
/// built inside the blocking closure too, so `Runtime::new` never runs on
/// the async task.
///
/// `resolve_snapshot` is only called for an owner-only target
/// (`{ owner_id }`, no `deployment_id`) - every UI remove path builds its
/// target that way whenever the scope has one mutable owner
/// (`lifecycleTargetForSkill`), so `remove_with_runtime` resolves it to a
/// deployment id the same way `update_skill` does, against a freshly
/// rebuilt snapshot. Deferred to a closure so the direct-`deployment_id`
/// path (the common case) never pays for a snapshot rebuild, and so the
/// rebuild - which walks the filesystem - runs inside `spawn_blocking`
/// alongside `build_runtime`, not on the async task.
pub(crate) async fn remove_with_runtime(
    target: LifecycleTarget,
    resolve_snapshot: impl FnOnce() -> Result<skill_refresh::SkillSnapshot, String> + Send + 'static,
    build_runtime: impl FnOnce(&DeploymentId) -> Result<skill_studio_core::ports::Runtime, String>
        + Send
        + 'static,
) -> Result<RemoveOutcome, String> {
    if target.deployment_id.is_none() && target.owner_id.is_none() {
        return Err("Remove requires a deployment id".to_string());
    }
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let deployment_id = if let Some(raw) = target.deployment_id.as_deref() {
            DeploymentId::parse(raw).map_err(|e| e.message)?
        } else {
            let snapshot = resolve_snapshot()?;
            let (_, deployment) = resolve_lifecycle_target(&snapshot, &target, "Remove")?;
            DeploymentId::parse(&deployment.id).map_err(|e| e.message)?
        };
        let rt = build_runtime(&deployment_id)?;
        let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
        let result = ops::remove(&rt, &ctx, &RemoveRequest { deployment_id });
        let envelope = ResultEnvelope::from_result(Operation::Remove, &rt.scope, &ctx, result);
        super::core_runtime::to_command_result(envelope)
    })
    .await;
    crate::timing_log::join_result_to_err("remove_skill", joined)
}

/// Maximum number of bytes read from an installed skill's SKILL.md, to keep
/// a runaway file from blocking the UI thread on a slow disk.
const MAX_SKILL_MD_BYTES: usize = 2 * 1024 * 1024;

/// Require that `path` belongs to an installed skill in the current
/// snapshot, so `read_installed_skill_md` / `open_skill_path` can't be used
/// to read or open an arbitrary path on disk.
pub(crate) fn require_snapshot_owns_path(
    refresh_state: &tauri::State<SkillRefreshState>,
    path: &std::path::Path,
) -> Result<(), String> {
    let snapshot = refresh_state.snapshot.read().ok().and_then(|g| g.clone());
    match &snapshot {
        Some(snapshot) if skill_refresh::snapshot_owns_path(snapshot, path) => Ok(()),
        _ => Err(format!(
            "Path is not an installed skill: {}",
            path.display()
        )),
    }
}

/// Resolves `path_buf` to a canonical, existing `SKILL.md` file path, without
/// checking ownership or plugin status - callers apply those separately.
/// Shared by `read_installed_skill_md` and `write_installed_skill_md`.
pub(crate) fn canonicalize_skill_md(
    path_buf: &std::path::Path,
    path: &str,
) -> Result<std::path::PathBuf, String> {
    if path_buf.file_name().and_then(|n| n.to_str()) != Some("SKILL.md") {
        return Err(format!("Path is not an installed skill: {path}"));
    }
    let canonical =
        std::fs::canonicalize(path_buf).map_err(|e| format!("Failed to open {path}: {e}"))?;
    let is_file = std::fs::symlink_metadata(&canonical).is_ok_and(|m| m.is_file());
    if !is_file {
        return Err(format!("Path is not an installed skill: {path}"));
    }
    Ok(canonical)
}

/// Read up to 2 MiB of an installed skill's `SKILL.md` straight off disk, for
/// the installed-skill detail page's SKILL.md viewer - works for
/// manual/plugin skills that have no remote source, unlike the skills.sh
/// browse panel's `getSkillDetails`. Restricted to `SKILL.md` files
/// belonging to a deployment in the current snapshot, to keep this from
/// becoming an arbitrary-file read.
#[tauri::command]
pub async fn read_installed_skill_md(
    path: String,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "read_installed_skill_md", move || {
        let refresh_state = app.state::<SkillRefreshState>();
        let path_buf = std::path::PathBuf::from(&path);
        require_snapshot_owns_path(&refresh_state, &path_buf)?;
        canonicalize_skill_md(&path_buf, &path)?;

        let mut file = File::open(&path).map_err(|e| format!("Failed to open {path}: {e}"))?;
        let mut buf = vec![0u8; MAX_SKILL_MD_BYTES];
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("Failed to read {path}: {e}"))?;
        buf.truncate(n);
        Ok(String::from_utf8_lossy(&buf).into_owned())
    })
    .await
}

/// Refuses a `write_installed_skill_md` request that targets a path outside
/// the current snapshot, or a `SKILL.md` owned by a plugin-managed
/// deployment (the harness owns that file, not the user). Pulled out of the
/// command so it's testable without a `tauri::AppHandle`.
pub(crate) fn check_skill_md_write_allowed(
    snapshot: Option<&skill_refresh::SkillSnapshot>,
    path: &std::path::Path,
) -> Result<(), String> {
    let owning_deployment =
        snapshot.and_then(|s| skill_refresh::snapshot_deployment_owning_path(s, path));
    match owning_deployment {
        None => Err(format!(
            "Path is not an installed skill: {}",
            path.display()
        )),
        Some(d) if d.plugin.is_some() => {
            Err("Skill is managed by a plugin and cannot be edited here".to_string())
        }
        Some(_) => Ok(()),
    }
}

/// Runs every check `write_installed_skill_md` and
/// `write_installed_skill_md_if_unchanged` share - ownership, canonicalization,
/// the size limit, and the plugin-managed refusal - and returns the canonical
/// path to write to.
fn validate_skill_md_write(
    path: &str,
    content: &str,
    refresh_state: &tauri::State<SkillRefreshState>,
) -> Result<std::path::PathBuf, String> {
    let path_buf = std::path::PathBuf::from(path);
    require_snapshot_owns_path(refresh_state, &path_buf)?;
    let canonical = canonicalize_skill_md(&path_buf, path)?;
    if content.len() > MAX_SKILL_MD_BYTES {
        return Err(format!(
            "SKILL.md is too large to save ({} bytes, max {})",
            content.len(),
            MAX_SKILL_MD_BYTES
        ));
    }

    let snapshot = refresh_state.snapshot.read().ok().and_then(|g| g.clone());
    check_skill_md_write_allowed(snapshot.as_ref(), &path_buf)?;
    Ok(canonical)
}

/// Write `content` to an installed skill's `SKILL.md`, for the detail
/// drawer's inline editor and Audit proposal Apply. Same ownership check as
/// `read_installed_skill_md`, plus a refusal when the owning deployment is
/// plugin-managed. Refuses the write (rather than silently overwriting) when
/// the file's current content doesn't match `expected_content` - the copy the
/// caller last loaded, so an ordinary stale baseline is detected before
/// writing. Marks the snapshot dirty afterward so the background loop picks
/// up the new content and token/byte counts, rather than rescanning every
/// skill on this thread.
#[tauri::command]
pub async fn write_installed_skill_md_if_unchanged(
    path: String,
    expected_content: String,
    content: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(
        &timing_app,
        "write_installed_skill_md_if_unchanged",
        move || {
            let refresh_state = app.state::<SkillRefreshState>();
            let canonical = validate_skill_md_write(&path, &content, &refresh_state)?;
            write_skill_md_compare_and_swap(&canonical, &expected_content, &content)?;
            skill_refresh::request_snapshot_rebuild(&app);
            Ok(())
        },
    )
    .await
}

/// Reveal a skill's folder in Finder, or open it in the user's default
/// editor, via macOS's `open` CLI. Restricted to paths belonging to a
/// deployment in the current snapshot. `async` so a cold `editor` mode - which
/// can start the login shell to read `$EDITOR` - never runs on the main
/// thread.
#[tauri::command(async)]
// Tauri commands deserialize their arguments fresh per invocation, so `path`
// and `mode` can't be borrowed from the caller - they must be owned.
#[allow(clippy::needless_pass_by_value)]
pub fn open_skill_path(
    path: String,
    mode: String,
    refresh_state: tauri::State<SkillRefreshState>,
) -> Result<(), String> {
    require_snapshot_owns_path(&refresh_state, std::path::Path::new(&path))?;

    let mut script_to_clean_up: Option<PathBuf> = None;
    let args: Vec<String> = match mode.as_str() {
        "reveal" => vec!["-R".to_string(), path.clone()],
        // `-t` would mean the system default *text* editor, which is TextEdit
        // on a stock machine - see `skill_editor` for the setting behind this.
        "editor" => {
            let home = dirs::home_dir().ok_or("Could not find home directory")?;
            match skill_editor::editor_launch(&home) {
                skill_editor::EditorLaunch::Open(mut args) => {
                    args.push(path.clone());
                    args
                }
                skill_editor::EditorLaunch::Terminal { command } => {
                    let script =
                        skill_editor::write_terminal_launch_script(Path::new(&path), &command)?;
                    let script_arg = script.to_string_lossy().to_string();
                    script_to_clean_up = Some(script);
                    vec![script_arg]
                }
            }
        }
        other => return Err(format!("Unknown open mode: {other}")),
    };

    let output = Command::new("open")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to open {path}: {e}"))?;

    if !output.status.success() {
        if let Some(script) = &script_to_clean_up {
            let _ = std::fs::remove_file(script);
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("Failed to open {path}: {stderr}"));
    }
    Ok(())
}

/// Everything the Settings "Open in editor" card shows: the automatic-row
/// label, the installed/saved apps, the `$EDITOR` row (if any), and the
/// still-usable saved choice. Reads the login shell for `$VISUAL`/`$EDITOR`,
/// so it runs off the main thread.
#[tauri::command]
pub async fn get_editor_choices(
    app: tauri::AppHandle,
) -> Result<skill_editor::EditorChoices, String> {
    crate::timing_log::time_command_async(&app, "get_editor_choices", async move {
        tauri::async_runtime::spawn_blocking(|| {
            let home = dirs::home_dir().ok_or("Could not find home directory")?;
            Ok(skill_editor::editor_choices(&home))
        })
        .await
        .map_err(|e| format!("Failed to read editor choices: {e}"))?
    })
    .await
}

/// Settings' "Command health" card and `skill-studio health`: the last 7
/// days of `timing.jsonl` (unit 0.1), folded to one row per command by
/// `skill_studio_core::health::health_rollup`. Reads and folds run in
/// `spawn_blocking` - the log can grow to `timing_log::ROTATE_AT_BYTES`
/// (5 MiB) before it rotates, and parsing that off the main thread is the
/// same reasoning `get_installed_skills` already applies to its own read.
#[tauri::command]
pub async fn command_health(
    app: tauri::AppHandle,
) -> Result<Vec<skill_studio_core::dto::CommandHealth>, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_async(&timing_app, "command_health", async move {
        tauri::async_runtime::spawn_blocking(move || {
            let rows = crate::timing_log::read_rows(&app);
            Ok(skill_studio_core::health::health_rollup(
                &rows,
                chrono::Utc::now(),
                std::time::Duration::from_secs(7 * 24 * 3600),
            ))
        })
        .await
        .map_err(|e| format!("Failed to compute command health: {e}"))?
    })
    .await
}

/// `async` because saving `"$EDITOR"` can start the login shell to check that
/// a terminal editor is actually set - see `skill_editor::set_preferred_editor`.
#[tauri::command(async)]
pub fn set_preferred_editor(app_name: Option<String>) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    skill_editor::set_preferred_editor(&home, app_name)
}

/// Resolves one target's deployment into the `UpdateRequest`
/// `skill_studio_core::ops::update` needs: `method`/`scope` from the
/// deployment, and - `Dotagents` only - `source`/`ref_pin` from the
/// matching ownership ledger entry. `ops::update`'s own argv builder
/// (`ops_update::update_cli_args_and_cwd`) takes an already-resolved
/// `ref_pin`; resolving a `declared_ref` ledger entry to a commit stays the
/// caller's job (see `ops_update`'s module doc), the same lookup the old
/// `update_skill` body ran before shelling out itself. Takes a bare
/// `app_data` path rather than a `tauri::AppHandle` (N3, review round 1) -
/// the only thing it ever needed off the handle - so a table test over the
/// three desktop-owned preconditions below can call it without a running
/// Tauri app.
fn build_update_request(
    app_data: &Path,
    snapshot: &skill_refresh::SkillSnapshot,
    skill: &InstalledSkill,
    deployment: &super::skill_dto::Deployment,
) -> Result<skill_studio_core::dto::UpdateRequest, String> {
    use skill_studio_core::dto::{InstallMethod, UpdateRequest};
    use skill_studio_core::identity::{ProjectRef, RootScope, SkillName};

    let scope = match (deployment.scope.as_str(), &deployment.project_path) {
        ("global", _) => RootScope::Global,
        ("project", Some(project_path)) => {
            RootScope::Project(ProjectRef(PathBuf::from(project_path)))
        }
        _ => {
            return Err(format!(
                "Update is not available for {} scope",
                deployment.scope
            ))
        }
    };
    let skill_name = SkillName(skill.name.clone());

    match deployment.owner_kind {
        super::skill_ownership::LifecycleOwnerKind::Dotagents => {
            let home = dirs::home_dir().ok_or("Could not find home directory")?;
            let project_paths: Vec<PathBuf> = snapshot.projects.iter().map(Into::into).collect();
            let ledgers = super::skill_ownership::load_ownership_ledgers(&home, &project_paths);
            let ledger = ledger_matching_deployment(&ledgers, deployment)
                .ok_or("Update is not available: the matching ownership ledger is missing")?;
            let entry = ledger
                .dotagents
                .iter()
                .find(|entry| entry.name == skill.name)
                .ok_or_else(|| {
                    format!(
                        "Update is not available: {} is not in the matching agents.lock",
                        skill.name
                    )
                })?;
            if !entry.has_manifest_row {
                return Err(format!(
                    "Update is not available: {} is a wildcard dotagents entry",
                    skill.name
                ));
            }
            let ref_pin = if entry.declared_ref.is_some() {
                let store = skill_update_check::read_update_check_store(app_data);
                let owner_id = deployment.owner_id.as_deref().ok_or(
                    "Update is not available: the selected deployment has no owner identity",
                )?;
                let current_owner_ids = skill_refresh::snapshot_owner_ids(&snapshot.skills);
                let latest =
                    skill_update_check::state_for_owner(&store, owner_id, &current_owner_ids)
                        .and_then(|state| state.latest_commit.clone());
                Some(latest.ok_or_else(|| {
                    format!(
                        "Update is not available yet: run \"Check now\" to find {}'s latest commit first",
                        skill.name
                    )
                })?)
            } else {
                None
            };
            Ok(UpdateRequest {
                skill: skill_name,
                method: InstallMethod::Dotagents,
                scope,
                files: Vec::new(),
                source: Some(entry.source.clone()),
                ref_pin,
            })
        }
        super::skill_ownership::LifecycleOwnerKind::SkillsSh => Ok(UpdateRequest {
            skill: skill_name,
            method: InstallMethod::SkillsSh,
            scope,
            files: Vec::new(),
            source: None,
            ref_pin: None,
        }),
        super::skill_ownership::LifecycleOwnerKind::Fork => {
            Err("Forked skills update with Pull upstream".to_string())
        }
        _ => Err("Update is not available for this deployment owner".to_string()),
    }
}

/// Clears `skill`'s outdated badge for one owner right away, instead of the
/// old `check_now_for_owner` full rescan (`gh api` calls plus a snapshot
/// rebuild) the pre-3.6b `update_skill` body ran after every successful
/// update - the background loop's own 6h currency check (unit 3.4)
/// reconciles the rest. `owner_id: None` (an owner-less deployment, which
/// `Update`'s own preconditions never actually allow through) leaves the
/// badge alone rather than guessing which entry to drop.
fn clear_update_flag(skill: &mut InstalledSkill, owner_id: Option<&str>) {
    let Some(owner_id) = owner_id else { return };
    skill.update_owner_ids.retain(|id| id != owner_id);
    skill
        .update_owners
        .retain(|update| update.owner_id != owner_id);
    skill.has_update = !skill.update_owner_ids.is_empty();
    if skill.update_owner_ids.is_empty() {
        skill.update_commit = None;
        skill.update_commit_at = None;
    }
}

/// The post-`ops::update` housekeeping one successfully updated owner needs:
/// write the just-updated commit into its persisted update-check state
/// (`skill_update_check::clear_owner_after_update` - review round 1's B1
/// fix, sharpened in round 2's B2 to write the commit rather than remove
/// the entry, so the next full rebuild's `apply_skill_snapshot_overlays`
/// doesn't recompute `has_update` from a stale pair and bring the badge
/// back), then patch the same owner's badge off the in-memory snapshot.
/// Takes a bare `app_data` path and `SkillRefreshState` rather than an
/// `AppHandle` so a test can drive it without a running Tauri app; returns
/// the snapshot `patch_snapshot` built (`None` when there was no snapshot
/// yet to patch) so a caller with an `AppHandle` can still emit it.
///
/// `current_owner_ids` is the caller's already-in-hand set (both
/// `update_skill` and `update_all_skills` hold the fresh snapshot
/// `rebuild_fresh_lifecycle_snapshot` just returned) rather than a second
/// `refresh_state.snapshot` lock acquisition here - re-locking would also
/// silently fall back to an empty set (disabling the legacy fallback below)
/// whenever the fresh snapshot hadn't been published to `refresh_state` yet.
fn clear_outdated_state(
    app_data: &Path,
    refresh_state: &SkillRefreshState,
    skill_name: &str,
    owner_id: Option<&str>,
    current_owner_ids: &[String],
) -> Result<Option<skill_refresh::SkillSnapshot>, String> {
    if let Some(owner_id) = owner_id {
        // `legacy_fallback_name` (inside `clear_owner_after_update`) needs
        // every currently-known owner id to tell a sole Global owner from
        // a name shared by more than one - the same set `state_for_owner`
        // checks against on the read side.
        skill_update_check::clear_owner_after_update(app_data, owner_id, current_owner_ids)?;
    }
    skill_refresh::patch_snapshot(refresh_state, |snapshot| {
        if let Some(entry) = snapshot.skills.iter_mut().find(|s| s.name == skill_name) {
            clear_update_flag(entry, owner_id);
        }
    })
}

/// Runs `clear_outdated_state` for one owner and, when it produced a fresh
/// snapshot, emits it - the `AppHandle`-holding half production commands use;
/// tests call `clear_outdated_state` directly instead.
fn clear_outdated_state_and_emit(
    app: &tauri::AppHandle,
    refresh_state: &SkillRefreshState,
    skill_name: &str,
    owner_id: Option<&str>,
    current_owner_ids: &[String],
) {
    let app_data = match app.path().app_data_dir() {
        Ok(app_data) => app_data,
        Err(e) => {
            eprintln!("[update] could not resolve app data dir: {e}");
            return;
        }
    };
    match clear_outdated_state(
        &app_data,
        refresh_state,
        skill_name,
        owner_id,
        current_owner_ids,
    ) {
        Ok(Some(built)) => {
            if let Err(e) = tauri::Emitter::emit(app, skill_refresh::SNAPSHOT_EVENT, &built) {
                eprintln!("[update] snapshot emit failed: {e}");
            }
        }
        Ok(None) => {}
        Err(e) => eprintln!("[update] outdated-state patch failed: {e}"),
    }
}

/// Thin adapter over `skill_studio_core::ops::update`: resolves the target
/// deployment, then hands its `Dotagents`/`SkillsSh` write to the core op
/// (Lease, journal row with a backup and inverse, `npx` through the
/// spawner port) - the same function the CLI's `update` subcommand and the
/// MCP server's `update` tool call. Old path deleted: `update_skill` no
/// longer shells out to `npx` itself, and `skill_lifecycle.rs`'s
/// `skills_sh_update_args`/`dotagents_update_args` argv builders are gone -
/// `ops_update::update_cli_args_and_cwd` owns that argv now.
#[tauri::command]
pub async fn update_skill(
    target: LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<skill_studio_core::dto::UpdateOutcome, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "update_skill", move || {
        let refresh_state = app.state::<SkillRefreshState>();
        let snapshot = rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
        let (skill, deployment) = resolve_lifecycle_target(&snapshot, &target, "Update")?;
        let app_data = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("."));
        let req = build_update_request(&app_data, &snapshot, &skill, &deployment)?;

        let rt = super::core_runtime::build_runtime_write()?;
        let ctx = skill_studio_core::ports::OpContext::uncancellable(
            skill_studio_core::identity::CorrelationId(ulid::Ulid::new().to_string()),
        );
        let result = skill_studio_core::ops::update(&rt, &ctx, &req);
        let envelope = skill_studio_core::ops::ResultEnvelope::from_result(
            skill_studio_core::ops::Operation::Update,
            &rt.scope,
            &ctx,
            result,
        );
        let outcome = super::core_runtime::to_command_result(envelope)?;

        clear_outdated_state_and_emit(
            &app,
            &refresh_state,
            &skill.name,
            deployment.owner_id.as_deref(),
            &skill_refresh::snapshot_owner_ids(&snapshot.skills),
        );
        Ok(outcome)
    })
    .await
}

/// Test-only: the batch write itself, isolated from target resolution and
/// `app.state()` so the thread-recording test below can pin it to
/// `spawn_blocking` without a real `tauri::AppHandle` - the same split
/// `harness_first_run.rs`'s `detect_with_runtime` uses. The sync body
/// (`build_runtime` then `ops::update_all`) is [`run_update_all_sync`],
/// shared with production `update_all_skills` (N1, review round 2) so the
/// ten-fixture thread-recording test below still proves something about the
/// exact call production makes, not a parallel copy of it - the earlier
/// split (production called `ops::update_all` directly inside its own
/// `time_command_blocking` closure) let that closure's body drift from this
/// one with nothing to notice. `on_outcome` is threaded straight through to
/// `ops::update_all` (B3: the review round 1 fix) rather than hardcoded to
/// a no-op here, so the thread-recording test can observe every one of the
/// batch's per-skill calls landing on this `spawn_blocking` closure's own
/// pool thread, not just the closure that builds the `Runtime`.
#[cfg(test)]
async fn update_all_with_runtime(
    requests: Vec<skill_studio_core::dto::UpdateRequest>,
    build_runtime: impl FnOnce() -> Result<skill_studio_core::ports::Runtime, String> + Send + 'static,
    on_outcome: impl FnMut(
            &skill_studio_core::identity::SkillName,
            &Result<skill_studio_core::dto::UpdateOutcome, skill_studio_core::error::CoreError>,
        ) + Send
        + 'static,
) -> Result<skill_studio_core::dto::UpdateAllOutcome, String> {
    let joined = tauri::async_runtime::spawn_blocking(move || {
        run_update_all_sync(&requests, build_runtime, on_outcome)
    })
    .await;
    crate::timing_log::join_result_to_err("update_all_skills", joined)
}

/// The sync body a `spawn_blocking` closure runs for one update-all batch:
/// build the `Runtime`, then run every request through `ops::update_all`.
/// Shared by `update_all_with_runtime` (test-only, wraps this in its own
/// `spawn_blocking` since a unit test builds a bare `Runtime` fixture
/// without an `AppHandle`) and production `update_all_skills` (already
/// inside the `spawn_blocking` closure `time_command_blocking` provides, so
/// it calls this directly rather than nesting a second one) - one body, so
/// a change to either caller's shape can't drift the two apart (N1, review
/// round 2).
fn run_update_all_sync(
    requests: &[skill_studio_core::dto::UpdateRequest],
    build_runtime: impl FnOnce() -> Result<skill_studio_core::ports::Runtime, String>,
    mut on_outcome: impl FnMut(
        &skill_studio_core::identity::SkillName,
        &Result<skill_studio_core::dto::UpdateOutcome, skill_studio_core::error::CoreError>,
    ),
) -> Result<skill_studio_core::dto::UpdateAllOutcome, String> {
    let rt = build_runtime()?;
    let ctx = skill_studio_core::ports::OpContext::uncancellable(
        skill_studio_core::identity::CorrelationId(ulid::Ulid::new().to_string()),
    );
    Ok(skill_studio_core::ops::update_all(
        &rt,
        &ctx,
        requests,
        &mut on_outcome,
    ))
}

/// Which owners a batch's succeeded items should clear (N1: the review
/// round 1 fix) - a failed item (`item.outcome` is `None`) keeps its badge
/// on so the row still reads as outdated, so this drops it rather than
/// clearing it alongside the succeeded ones. Split out of `update_all_skills`
/// so a test can drive the filter without a real `tauri::AppHandle`.
///
/// Takes `owners` as a `Vec<(name, owner, deployment_path)>` and matches
/// each succeeded item to its owner by `deployment_path` (B1, review round
/// 3) rather than by request-order index: `dto.rs` documents
/// `UpdateAllOutcome.items` as "in the order each one finished (not the
/// order requested)", so zipping by index clears the wrong owner's badge
/// the moment `update_all` stops finishing requests strictly in order.
/// Matching by name alone doesn't work either - `skillUpdateOwnerTargets`
/// sends one target per outdated owner, so a skill outdated for two owners
/// produces two requests sharing one name - but each owner's deployment
/// lives at its own path, so the path is unique per request even when the
/// name is not.
fn owners_to_clear(
    outcome: &skill_studio_core::dto::UpdateAllOutcome,
    owners: &[(String, Option<String>, PathBuf)],
) -> Vec<(String, Option<String>)> {
    let by_path: std::collections::HashMap<&Path, (&str, Option<&str>)> = owners
        .iter()
        .map(|(name, owner, path)| (path.as_path(), (name.as_str(), owner.as_deref())))
        .collect();
    outcome
        .items
        .iter()
        .filter_map(|item| item.outcome.as_ref())
        .filter_map(|update_outcome| by_path.get(update_outcome.deployment_path.as_path()))
        .map(|(name, owner)| ((*name).to_string(), owner.map(str::to_string)))
        .collect()
}

/// "Update all": resolves every target, then runs `ops::update_all` over all
/// of them via `run_update_all_sync` (shared with the test-only
/// `update_all_with_runtime`, N1 review round 2) in the one `spawn_blocking`
/// task `time_command_blocking` wraps the whole body in (N2: the review
/// round 1 fix - resolving targets reads ledgers off disk, which no longer
/// runs untimed on the Tokio worker) - each skill still gets its own
/// journal row (`ops::update_all`'s own per-request loop), but no part of
/// resolving targets, reading ledgers, or writing skills touches the UI
/// task. `on_outcome` is a no-op here: the
/// frontend's existing "Updated N of M" toast reads the returned
/// `UpdateAllOutcome` once the whole batch finishes rather than a per-skill
/// progress event, matching the shared brief's "no new component". Only a
/// succeeded item's owner has its badge cleared (N1, via `owners_to_clear`);
/// a failed item's badge stays on so the row still reads as outdated.
#[tauri::command]
pub async fn update_all_skills(
    targets: Vec<LifecycleTarget>,
    app: tauri::AppHandle,
) -> Result<skill_studio_core::dto::UpdateAllOutcome, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "update_all_skills", move || {
        let refresh_state = app.state::<SkillRefreshState>();
        let snapshot = rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
        let app_data = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("."));
        let mut requests = Vec::with_capacity(targets.len());
        let mut owners = Vec::with_capacity(targets.len());
        for target in &targets {
            let (skill, deployment) = resolve_lifecycle_target(&snapshot, target, "Update")?;
            requests.push(build_update_request(
                &app_data,
                &snapshot,
                &skill,
                &deployment,
            )?);
            owners.push((
                skill.name.clone(),
                deployment.owner_id.clone(),
                PathBuf::from(&deployment.path),
            ));
        }

        let outcome = run_update_all_sync(
            &requests,
            super::core_runtime::build_runtime_write,
            |_, _| {},
        )?;

        let current_owner_ids = skill_refresh::snapshot_owner_ids(&snapshot.skills);
        for (skill_name, owner_id) in owners_to_clear(&outcome, &owners) {
            clear_outdated_state_and_emit(
                &app,
                &refresh_state,
                &skill_name,
                owner_id.as_deref(),
                &current_owner_ids,
            );
        }
        Ok(outcome)
    })
    .await
}

/// Runs one Claude-Code-only plugin lifecycle action: checks `harness`,
/// holds the write lease for the CLI call, then requests a snapshot rebuild.
/// Shared by [`set_plugin_enabled`] and [`uninstall_plugin`], which differ
/// only in which `claude plugin` subcommand `action` runs.
fn run_plugin_lifecycle_action(
    harness: &str,
    app: &tauri::AppHandle,
    home: &Path,
    action: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    super::skill_plugin_lifecycle::require_claude_code_harness(harness)?;
    let write_lease = super::write_lease::WriteLease::default();
    let _guard = write_lease.try_acquire(home)?;
    action()?;
    skill_refresh::request_snapshot_rebuild(app);
    Ok(())
}

/// Disable or re-enable one Claude Code plugin (`claude plugin
/// disable|enable <plugin_id> -s user`). Applies to every skill the plugin
/// ships - Claude Code tracks `enabledPlugins` per plugin, not per skill.
#[tauri::command]
pub async fn set_plugin_enabled(
    plugin_id: String,
    harness: String,
    enabled: bool,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "set_plugin_enabled", move || {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        run_plugin_lifecycle_action(&harness, &app, &home, || {
            super::skill_plugin_lifecycle::set_plugin_enabled_with(
                &RealCommandRunner::new(),
                &plugin_id,
                enabled,
            )
        })
    })
    .await
}

/// Uninstall one Claude Code plugin (`claude plugin uninstall <plugin_id>
/// -s user -y`). Removes the `enabledPlugins` entry; Claude Code sweeps the
/// cache directory later.
#[tauri::command]
pub async fn uninstall_plugin(
    plugin_id: String,
    harness: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "uninstall_plugin", move || {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        run_plugin_lifecycle_action(&harness, &app, &home, || {
            super::skill_plugin_lifecycle::uninstall_plugin_with(
                &RealCommandRunner::new(),
                &plugin_id,
            )
        })
    })
    .await
}
