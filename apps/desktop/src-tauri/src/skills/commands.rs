// ============================================================================
// Skills Module - Tauri Commands
// IPC commands for skill discovery, installation, and management
// ============================================================================

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::api;
use super::skill_add::RealCommandRunner;
use super::skill_dto::{
    InstallResult, InstallScope, InstalledSkill, LifecycleTarget, PaginatedSkillsResponse,
    SkillDetails,
};
use super::skill_editor;
use super::skill_lifecycle::{
    dotagents_update_args, ledger_matching_deployment, rebuild_fresh_lifecycle_snapshot,
    resolve_lifecycle_target, skills_sh_update_args,
};
use super::skill_md_write::write_skill_md_compare_and_swap;
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
    use super::super::skill_add::CommandRunner;
    use super::super::skill_lifecycle::skills_sh_remove_args_for_scope;
    use super::super::skill_md_write::write_skill_md;
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
                trial: None,
                trials: Vec::new(),
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
    fn single_owner_target_fixture(
        root: &Path,
    ) -> (skill_refresh::SkillSnapshot, LifecycleTarget) {
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
    fn skills_sh_remove_args_selects_global_flag() {
        assert_eq!(
            skills_sh_remove_args_for_scope("foo", InstallScope::Global),
            vec!["skills", "remove", "foo", "--yes", "--global"]
        );
        assert_eq!(
            skills_sh_remove_args_for_scope("foo", InstallScope::Project),
            vec!["skills", "remove", "foo", "--yes"]
        );
    }

    #[test]
    fn dotagents_remove_args_selects_project_mode() {
        assert_eq!(
            dotagents_remove_args("foo", InstallScope::Project),
            vec!["-y", "@sentry/dotagents", "--project", "remove", "foo"]
        );
        assert_eq!(
            dotagents_remove_args("foo", InstallScope::Global),
            vec!["-y", "@sentry/dotagents", "remove", "foo"]
        );
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

    fn dotagents_skill(
        name: &str,
        declared_ref: Option<&str>,
        has_manifest_row: bool,
    ) -> skill_studio_core::dotagents_ledger::DotagentsSkill {
        skill_studio_core::dotagents_ledger::DotagentsSkill {
            name: name.to_string(),
            source: format!("getsentry/{name}"),
            github_repo: Some(format!("getsentry/{name}")),
            path: format!("skills/{name}"),
            installed_commit: Some("a".repeat(40)),
            declared_ref: declared_ref.map(str::to_string),
            has_manifest_row,
        }
    }

    #[test]
    fn dotagents_update_args_rejects_skill_with_no_matching_ledger_entry() {
        let err = dotagents_update_args("manual-in-shared-root", None, None, InstallScope::Global)
            .unwrap_err();
        assert_eq!(
            err,
            "Update is not available: manual-in-shared-root is not in the matching agents.lock"
        );
    }

    #[test]
    fn dotagents_update_args_rejects_wildcard_read_only_entry() {
        let entry = dotagents_skill("find-bugs", None, false);
        let err = dotagents_update_args("find-bugs", Some(&entry), None, InstallScope::Global)
            .unwrap_err();
        assert_eq!(
            err,
            "Update is not available: find-bugs is a wildcard dotagents entry"
        );
    }

    #[test]
    fn dotagents_update_args_named_unpinned_entry_uses_add_without_ref() {
        let entry = dotagents_skill("find-bugs", None, true);
        let args =
            dotagents_update_args("find-bugs", Some(&entry), None, InstallScope::Global).unwrap();
        assert_eq!(
            args,
            vec![
                "-y",
                "@sentry/dotagents",
                "add",
                "getsentry/find-bugs",
                "--name",
                "find-bugs"
            ]
        );
    }

    #[test]
    fn dotagents_update_args_selects_project_mode() {
        let entry = dotagents_skill("find-bugs", None, true);
        let args =
            dotagents_update_args("find-bugs", Some(&entry), None, InstallScope::Project).unwrap();
        assert_eq!(
            args,
            vec![
                "-y",
                "@sentry/dotagents",
                "--project",
                "add",
                "getsentry/find-bugs",
                "--name",
                "find-bugs"
            ]
        );
    }

    #[test]
    fn dotagents_update_args_pinned_entry_needs_latest_commit() {
        let entry = dotagents_skill("find-bugs", Some("aaaa"), true);
        let err = dotagents_update_args("find-bugs", Some(&entry), None, InstallScope::Global)
            .unwrap_err();
        assert!(err.contains("Check now"));
    }

    #[test]
    fn dotagents_update_args_pinned_entry_re_pins_to_latest_commit() {
        let entry = dotagents_skill("find-bugs", Some("aaaa"), true);
        let latest = "b".repeat(40);
        let args = dotagents_update_args(
            "find-bugs",
            Some(&entry),
            Some(&latest),
            InstallScope::Global,
        )
        .unwrap();
        assert_eq!(
            args,
            vec![
                "-y",
                "@sentry/dotagents",
                "add",
                "getsentry/find-bugs",
                "--name",
                "find-bugs",
                "--ref",
                &latest,
            ]
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
    async fn remove_of_an_owner_target_resolves_the_canonical_deployment_or_names_the_missing_id()
    {
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let (snapshot, target) = single_owner_target_fixture(tmp.path());
        let expected_id = snapshot.skills[0].deployments[0].id.clone();

        let built_with_id: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
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
        let deployment_id = match target.deployment_id.as_deref() {
            Some(raw) => DeploymentId::parse(raw).map_err(|e| e.message)?,
            None => {
                let snapshot = resolve_snapshot()?;
                let (_, deployment) = resolve_lifecycle_target(&snapshot, &target, "Remove")?;
                DeploymentId::parse(&deployment.id).map_err(|e| e.message)?
            }
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

/// Update a skill, using whichever CLI owns it: `dotagents` for a
/// named dotagents-managed skill (`add` re-pins it to the latest commit), `npx
/// skills update` for a skills.sh skill. Manual/plugin skills have no owning
/// CLI to update through, and wildcard dotagents entries are read-only, so
/// they are rejected up front.
#[tauri::command]
pub async fn update_skill(
    target: LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<InstallResult, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "update_skill", move || {
        let refresh_state = app.state::<SkillRefreshState>();
        let update_check_state = app.state::<skill_update_check::UpdateCheckState>();
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let write_lease = super::write_lease::WriteLease::default();
        let _guard = write_lease.try_acquire(&home)?;
        let snapshot = rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
        let (skill, deployment) = resolve_lifecycle_target(&snapshot, &target, "Update")?;
        let skill_name = skill.name;
        let scope = if deployment.scope == "global" {
            super::skill_dto::InstallScope::Global
        } else if deployment.scope == "project" {
            super::skill_dto::InstallScope::Project
        } else {
            return Err(format!(
                "Update is not available for {} scope",
                deployment.scope
            ));
        };
        let project_paths: Vec<std::path::PathBuf> =
            snapshot.projects.iter().map(Into::into).collect();
        let ledgers = super::skill_ownership::load_ownership_ledgers(&home, &project_paths);

        let (tool, args): (&str, Vec<String>) = match deployment.owner_kind {
            super::skill_ownership::LifecycleOwnerKind::Dotagents => {
                let ledger = ledger_matching_deployment(&ledgers, &deployment)
                    .ok_or("Update is not available: the matching ownership ledger is missing")?;
                let entry = ledger
                    .dotagents
                    .iter()
                    .find(|entry| entry.name == skill_name);
                let latest_commit = if entry.is_some_and(|e| e.declared_ref.is_some()) {
                    let app_data = app
                        .path()
                        .app_data_dir()
                        .unwrap_or_else(|_| std::path::PathBuf::from("."));
                    let store = skill_update_check::read_update_check_store(&app_data);
                    let owner_id = deployment.owner_id.as_deref().ok_or(
                        "Update is not available: the selected deployment has no owner identity",
                    )?;
                    let current_owner_ids: Vec<String> = snapshot
                        .skills
                        .iter()
                        .flat_map(|skill| skill.deployments.iter())
                        .filter_map(|deployment| deployment.owner_id.clone())
                        .collect();
                    skill_update_check::state_for_owner(&store, owner_id, &current_owner_ids)
                        .and_then(|state| state.latest_commit.clone())
                } else {
                    None
                };
                let args =
                    dotagents_update_args(&skill_name, entry, latest_commit.as_deref(), scope)?;
                ("dotagents", args)
            }
            super::skill_ownership::LifecycleOwnerKind::SkillsSh => {
                ("skills-sh", skills_sh_update_args(&skill_name, scope))
            }
            super::skill_ownership::LifecycleOwnerKind::Fork => {
                return Err("Forked skills update with Pull upstream".to_string())
            }
            _ => return Err("Update is not available for this deployment owner".to_string()),
        };

        let npx_command = format!("npx {}", args.join(" "));
        let mut command = Command::new("npx");
        command.args(&args);
        if let Some(project_path) = &deployment.project_path {
            command.current_dir(project_path);
        }
        let output = command
            .output()
            .map_err(|e| format!("Failed to execute npx: {e}"))?;

        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            let owner_id = deployment
                .owner_id
                .as_deref()
                .ok_or("Update is not available: the selected deployment has no owner identity")?;
            skill_update_check::check_now_for_owner(
                &app,
                &update_check_state,
                owner_id,
                &project_paths,
            );
            skill_refresh::request_snapshot_rebuild(&app);
            Ok(InstallResult {
                success: true,
                skill_name,
                installed_path: None,
                error: None,
                tool: Some(tool.to_string()),
                command: Some(npx_command),
            })
        } else {
            Ok(InstallResult {
                success: false,
                skill_name,
                installed_path: None,
                error: Some(stderr),
                tool: Some(tool.to_string()),
                command: Some(npx_command),
            })
        }
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
