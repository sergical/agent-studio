// ============================================================================
// Skills Module - Tauri Commands
// IPC commands for skill discovery, installation, and management
// ============================================================================

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use super::agents::{AgentId, AgentTarget};
use super::api;
use super::lock_file;
use super::project_discovery;
use super::skill_add::{CommandRunner, RealCommandRunner};
use super::skill_agent_runner::validate_skill_dir_name;
use super::skill_dto::{
    InstallResult, InstallScope, InstalledSkill, LifecycleTarget, PaginatedSkillsResponse,
    SkillDetails, SkillsShAccessInfo,
};
use super::skill_editor;
use super::skill_fork;
use super::skill_fork_registry;
use super::skill_lifecycle::{
    dotagents_update_args, ledger_matching_deployment, rebuild_fresh_lifecycle_snapshot,
    resolve_lifecycle_target, skills_sh_remove_args_for_scope, skills_sh_update_args,
};
use super::skill_refresh::{self, SkillRefreshState};
use super::skill_trial;
use super::skill_update_check;
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

/// `set_skills_sh_api_key`'s logic against an arbitrary home dir, so tests
/// don't need to touch the real `~/.agents`.
fn save_skills_sh_api_key(home: &std::path::Path, key: &str) -> Result<(), String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("The API key can't be empty".to_string());
    }
    let mut registry = skill_fork_registry::read_fork_registry(home)?;
    registry.skills_sh_api_key = Some(trimmed.to_string());
    skill_fork_registry::write_fork_registry(home, &registry)
}

/// `get_skills_sh_access`'s logic against an arbitrary home dir, so tests
/// don't need to touch the real `~/.agents`.
fn skills_sh_access_info(home: &std::path::Path) -> Result<SkillsShAccessInfo, String> {
    Ok(match api::resolve_skills_sh_access(home)? {
        api::SkillsShAccess::Direct { .. } => SkillsShAccessInfo {
            mode: "direct".to_string(),
            server_url: None,
        },
        api::SkillsShAccess::Server { base_url } => SkillsShAccessInfo {
            mode: "server".to_string(),
            server_url: Some(base_url.trim_end_matches("/api/v1").to_string()),
        },
    })
}

/// Whether discovery goes straight to skills.sh with a developer-override key
/// (`"direct"`) or through the local Skill Studio server (`"server"`, with
/// its URL) - the Settings page's status line and the Browse tab's error
/// messaging both read this instead of the old key-only status.
#[tauri::command]
pub fn get_skills_sh_access() -> Result<SkillsShAccessInfo, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    skills_sh_access_info(&home)
}

/// Saves `key` as `skills_sh_api_key` in `~/.agents/skill-studio.json`,
/// preserving every other field. Refuses an empty (or all-whitespace) key -
/// the Settings page's Save button.
#[tauri::command]
pub fn set_skills_sh_api_key(key: String) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    save_skills_sh_api_key(&home, &key)
}

/// Search for skills on skills.sh
#[tauri::command]
pub async fn search_skills(
    query: String,
    limit: Option<u32>,
) -> Result<PaginatedSkillsResponse, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let access = api::resolve_skills_sh_access(&home)?;
    api::search_skills(&access, &query, limit).await
}

/// Get popular skills (sorted by install count)
#[tauri::command]
pub async fn get_popular_skills(
    page: Option<u32>,
    per_page: Option<u32>,
) -> Result<PaginatedSkillsResponse, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let access = api::resolve_skills_sh_access(&home)?;
    api::get_popular_skills(&access, page, per_page).await
}

/// Get skill details from skills.sh
#[tauri::command]
pub async fn get_skill_details(skill_id: String) -> Result<SkillDetails, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let access = api::resolve_skills_sh_access(&home)?;
    api::get_skill_details(&access, &skill_id).await
}

/// Get all installed skills. Returns the background-refreshed snapshot's
/// skills (see `skill_refresh`) when it already accounts for every path in
/// `project_paths` and no mutation is pending (`skills_dirty`); otherwise
/// registers the missing paths and rebuilds the snapshot synchronously (so
/// this read-after-write sees fresh data), which also covers the case where
/// the background snapshot hasn't landed yet or a mutation just landed and
/// the background rebuild hasn't caught up.
#[tauri::command]
pub fn get_installed_skills(
    project_paths: Option<Vec<String>>,
    refresh_state: tauri::State<SkillRefreshState>,
    app: tauri::AppHandle,
) -> Result<Vec<InstalledSkill>, String> {
    let requested = project_paths.unwrap_or_default();
    let snapshot = refresh_state.snapshot.read().ok().and_then(|g| g.clone());

    if let Some(snapshot) = &snapshot {
        if !refresh_state.is_skills_dirty()
            && snapshot_covers_projects(&requested, &snapshot.projects)
        {
            return Ok(snapshot.skills.clone());
        }
    }

    refresh_state.add_extra_projects(requested);
    let rebuilt = skill_refresh::rebuild_snapshot_now(&app, &refresh_state)?;
    Ok(rebuilt.skills)
}

/// Whether the *published* snapshot already accounts for every path in
/// `requested`. Pulled out into a pure function so it can be unit tested:
/// this must only compare against `snapshot.projects`, never against
/// caller-registered `extra_projects`, since a path registered but not yet
/// rebuilt into the snapshot would otherwise look "covered" while the
/// snapshot's `skills` still doesn't include it.
fn snapshot_covers_projects(requested: &[String], snapshot_projects: &[String]) -> bool {
    requested.iter().all(|p| snapshot_projects.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CountingLifecycleRunner(std::sync::atomic::AtomicUsize);

    impl CommandRunner for CountingLifecycleRunner {
        fn run_npx(&self, _args: &[String], _cwd: Option<&Path>) -> Result<(), String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn snapshot_covers_projects_requires_published_membership() {
        let snapshot_projects = vec!["/work/known".to_string()];

        assert!(snapshot_covers_projects(
            &["/work/known".to_string()],
            &snapshot_projects
        ));
        // A caller-registered path that hasn't landed in the snapshot yet
        // must NOT be treated as covered, even though it would be in
        // `extra_projects`.
        assert!(!snapshot_covers_projects(
            &["/work/not-yet-rebuilt".to_string()],
            &snapshot_projects
        ));
    }

    /// A minimal `SkillSnapshot` with one skill deployed at `dep_dir`, with
    /// or without a plugin deployment, for `check_skill_md_write_allowed` tests.
    fn fixture_snapshot(
        dep_dir: &std::path::Path,
        plugin: Option<super::super::skill_dto::PluginInfo>,
    ) -> skill_refresh::SkillSnapshot {
        use super::super::provenance::SourceKind;
        use super::super::skill_dto::{Deployment, InstalledSkill};
        use super::super::skill_invocations::InvocationHeatmap;
        use chrono::Utc;
        use std::collections::BTreeMap;

        skill_refresh::SkillSnapshot {
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

        atomic_write_skill_md(&skill_md, "---\nname: foo\n---\nupdated body").unwrap();

        let round_tripped = std::fs::read_to_string(&skill_md).unwrap();
        assert_eq!(round_tripped, "---\nname: foo\n---\nupdated body");
    }

    #[test]
    fn atomic_write_round_trips_twice_in_a_row() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_md = tmp.path().join("SKILL.md");
        std::fs::write(&skill_md, "original").unwrap();

        atomic_write_skill_md(&skill_md, "first save").unwrap();
        assert_eq!(std::fs::read_to_string(&skill_md).unwrap(), "first save");

        atomic_write_skill_md(&skill_md, "second save").unwrap();
        assert_eq!(std::fs::read_to_string(&skill_md).unwrap(), "second save");
    }

    #[test]
    fn atomic_write_leaves_no_temp_file_on_failed_rename() {
        let tmp = tempfile::tempdir().unwrap();
        // `canonical` names a directory, not a file: the rename onto it fails,
        // and the temp file created alongside it must not survive.
        let canonical = tmp.path().join("SKILL.md");
        std::fs::create_dir_all(&canonical).unwrap();

        let err = atomic_write_skill_md(&canonical, "content");
        assert!(err.is_err());

        let leftover_temp_files = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
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

    fn dotagents_skill(
        name: &str,
        declared_ref: Option<&str>,
        has_manifest_row: bool,
    ) -> super::super::dotagents_ledger::DotagentsSkill {
        super::super::dotagents_ledger::DotagentsSkill {
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

    struct DotagentsRemovalRunner {
        fail: bool,
    }

    impl CommandRunner for DotagentsRemovalRunner {
        fn run_npx(&self, _args: &[String], _cwd: Option<&Path>) -> Result<(), String> {
            if self.fail {
                Err("injected dotagents failure".to_string())
            } else {
                Ok(())
            }
        }
    }

    fn write_dotagents_owner(agents_dir: &Path, name: &str) {
        std::fs::create_dir_all(agents_dir.join("skills").join(name)).unwrap();
        std::fs::write(
            agents_dir.join("agents.toml"),
            format!("[[skills]]\nname = \"{name}\"\nsource = \"o/r\"\n"),
        )
        .unwrap();
        std::fs::write(
            agents_dir.join("agents.lock"),
            format!(
                "[skills.{name}]\nsource = \"o/r\"\nresolved_path = \"skills/{name}\"\nresolved_commit = \"abc\"\n"
            ),
        )
        .unwrap();
        std::fs::write(
            agents_dir.join("skills").join(name).join("SKILL.md"),
            "body",
        )
        .unwrap();
    }

    fn discovered_dotagents_snapshot(
        home: &Path,
        projects: &[PathBuf],
    ) -> skill_refresh::SkillSnapshot {
        let candidates = super::super::skill_discovery::discover_skill_candidates(home, projects);
        let ledgers = super::super::skill_ownership::load_ownership_ledgers(home, projects);
        let lock = super::super::lock_file::SkillLockFile {
            version: 3,
            skills: Default::default(),
        };
        let skills = super::super::skill_assembly::assemble_installed_skills(
            candidates,
            &lock,
            &ledgers,
            &Default::default(),
        );
        let mut snapshot = fixture_snapshot(home, None);
        snapshot.skills = skills;
        snapshot
    }

    fn dotagents_removal_snapshot(
        canonical_path: &Path,
        link_path: Option<&Path>,
        scope: &str,
        project_path: Option<&Path>,
    ) -> skill_refresh::SkillSnapshot {
        use super::super::skill_deployment::{
            deployment_id, BackingRelationship, DeploymentMutability, SkillDestination,
        };
        use super::super::skill_ownership::LifecycleOwnerKind;

        let mut snapshot = fixture_snapshot(canonical_path, None);
        let owner_id = match project_path {
            Some(project) => format!(
                "owner:v1/project/{}/foo",
                super::super::skill_deployment::encode_id_path(&project.to_string_lossy())
            ),
            None => "owner:v1/global/foo".to_string(),
        };
        let canonical_id = deployment_id(
            "foo",
            scope,
            SkillDestination::Universal,
            "universal",
            project_path.and_then(Path::to_str),
            canonical_path,
        );
        let canonical = &mut snapshot.skills[0].deployments[0];
        canonical.id = canonical_id.clone();
        canonical.path = canonical_path.to_string_lossy().into_owned();
        canonical.scope = scope.to_string();
        canonical.project_path = project_path.map(|path| path.to_string_lossy().into_owned());
        canonical.destination = SkillDestination::Universal;
        canonical.owner_kind = LifecycleOwnerKind::Dotagents;
        canonical.owner_id = Some(owner_id.clone());
        canonical.mutability = DeploymentMutability::Mutable;
        canonical.backing = BackingRelationship::Canonical;
        if let Some(link_path) = link_path {
            let mut link = canonical.clone();
            link.id = deployment_id(
                "foo",
                scope,
                SkillDestination::Universal,
                "claude-code",
                project_path.and_then(Path::to_str),
                link_path,
            );
            link.path = link_path.to_string_lossy().into_owned();
            link.agent = "Claude Code".to_string();
            link.is_symlink = true;
            link.resolved_path = std::fs::canonicalize(link_path)
                .ok()
                .map(|path| path.to_string_lossy().into_owned());
            link.backing = BackingRelationship::LinkedTo {
                deployment_id: canonical_id,
            };
            link.owner_id = Some(owner_id);
            snapshot.skills[0].deployments.push(link);
        }
        snapshot
    }

    #[test]
    fn dotagents_global_removal_stages_only_exact_backed_claude_link() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let canonical = home.join(".agents/skills/foo");
        let link = home.join(".claude/skills/foo");
        let independent = home.join(".codex/skills/foo");
        for path in [&canonical, &independent] {
            std::fs::create_dir_all(path).unwrap();
            std::fs::write(path.join("SKILL.md"), "body").unwrap();
        }
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../../.agents/skills/foo", &link).unwrap();
        let snapshot = dotagents_removal_snapshot(&canonical, Some(&link), "global", None);

        remove_dotagents_deployment_with(
            DotagentsRemovalContext {
                home: &home,
                snapshot: &snapshot,
                skill_name: "foo",
                deployment: &snapshot.skills[0].deployments[0],
                scope: InstallScope::Global,
                project_path: None,
            },
            &DotagentsRemovalRunner { fail: false },
            |stage| std::fs::remove_dir_all(stage).map_err(|error| error.to_string()),
        )
        .unwrap();

        assert!(std::fs::symlink_metadata(link).is_err());
        assert!(independent.join("SKILL.md").is_file());
    }

    #[test]
    fn dotagents_project_removal_failure_restores_exact_claude_link() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let canonical = project.join(".agents/skills/foo");
        let link = project.join(".claude/skills/foo");
        std::fs::create_dir_all(&canonical).unwrap();
        std::fs::write(canonical.join("SKILL.md"), "body").unwrap();
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../../.agents/skills/foo", &link).unwrap();
        let snapshot =
            dotagents_removal_snapshot(&canonical, Some(&link), "project", Some(&project));

        let error = remove_dotagents_deployment_with(
            DotagentsRemovalContext {
                home: &home,
                snapshot: &snapshot,
                skill_name: "foo",
                deployment: &snapshot.skills[0].deployments[0],
                scope: InstallScope::Project,
                project_path: Some(&project),
            },
            &DotagentsRemovalRunner { fail: true },
            |_| Ok(()),
        )
        .unwrap_err();

        assert_eq!(error, "injected dotagents failure");
        assert_eq!(
            std::fs::read_link(link).unwrap(),
            PathBuf::from("../../.agents/skills/foo")
        );
    }

    #[test]
    fn dotagents_removal_does_not_touch_independent_claude_collision() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let canonical = home.join(".agents/skills/foo");
        let collision = home.join(".claude/skills/foo");
        for path in [&canonical, &collision] {
            std::fs::create_dir_all(path).unwrap();
            std::fs::write(path.join("SKILL.md"), path.to_string_lossy().as_bytes()).unwrap();
        }
        let snapshot = dotagents_removal_snapshot(&canonical, None, "global", None);

        remove_dotagents_deployment_with(
            DotagentsRemovalContext {
                home: &home,
                snapshot: &snapshot,
                skill_name: "foo",
                deployment: &snapshot.skills[0].deployments[0],
                scope: InstallScope::Global,
                project_path: None,
            },
            &DotagentsRemovalRunner { fail: false },
            |_| Ok(()),
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(collision.join("SKILL.md")).unwrap(),
            collision.to_string_lossy()
        );
    }

    #[test]
    fn discovered_dotagents_links_inherit_exact_owner_and_are_removed_in_both_scopes() {
        use super::super::skill_deployment::{BackingRelationship, DeploymentMutability};

        for project_scoped in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            let project = tmp.path().join("project");
            let scope_root = if project_scoped { &project } else { &home };
            let agents_dir = scope_root.join(".agents");
            let canonical = agents_dir.join("skills/foo");
            let link = scope_root.join(".claude/skills/foo");
            write_dotagents_owner(&agents_dir, "foo");
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink("../../.agents/skills/foo", &link).unwrap();
            let projects = if project_scoped {
                vec![project.clone()]
            } else {
                Vec::new()
            };
            let snapshot = discovered_dotagents_snapshot(&home, &projects);
            let skill = snapshot
                .skills
                .iter()
                .find(|skill| skill.name == "foo")
                .unwrap();
            let canonical_deployment = skill
                .deployments
                .iter()
                .find(|deployment| deployment.path == canonical.to_string_lossy())
                .unwrap();
            let linked = skill
                .deployments
                .iter()
                .find(|deployment| deployment.path == link.to_string_lossy())
                .unwrap();
            assert_eq!(
                linked.owner_id, canonical_deployment.owner_id,
                "deployments: {:#?}",
                skill.deployments
            );
            assert_eq!(linked.owner_kind, canonical_deployment.owner_kind);
            assert_eq!(linked.mutability, DeploymentMutability::ReadOnly);
            assert!(matches!(
                &linked.backing,
                BackingRelationship::LinkedTo { deployment_id }
                    if deployment_id == &canonical_deployment.id
            ));

            remove_dotagents_deployment_with(
                DotagentsRemovalContext {
                    home: &home,
                    snapshot: &snapshot,
                    skill_name: "foo",
                    deployment: canonical_deployment,
                    scope: if project_scoped {
                        InstallScope::Project
                    } else {
                        InstallScope::Global
                    },
                    project_path: project_scoped.then_some(project.as_path()),
                },
                &DotagentsRemovalRunner { fail: false },
                |stage| std::fs::remove_dir_all(stage).map_err(|error| error.to_string()),
            )
            .unwrap();
            assert!(std::fs::symlink_metadata(link).is_err());
        }
    }

    #[test]
    fn dotagents_removal_ignores_wrong_target_link_and_same_name_independent_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        write_dotagents_owner(&home.join(".agents"), "foo");
        let other = home.join(".agents/skills/other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("SKILL.md"), "other").unwrap();
        let wrong_link = home.join(".claude/skills/foo");
        let independent = home.join(".codex/skills/foo");
        std::fs::create_dir_all(wrong_link.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&independent).unwrap();
        std::fs::write(independent.join("SKILL.md"), "independent").unwrap();
        std::os::unix::fs::symlink("../../.agents/skills/other", &wrong_link).unwrap();
        let snapshot = discovered_dotagents_snapshot(&home, &[]);
        let skill = snapshot
            .skills
            .iter()
            .find(|skill| skill.name == "foo")
            .unwrap();
        let canonical = skill
            .deployments
            .iter()
            .find(|deployment| deployment.path == home.join(".agents/skills/foo").to_string_lossy())
            .unwrap();
        let wrong = skill
            .deployments
            .iter()
            .find(|deployment| deployment.path == wrong_link.to_string_lossy())
            .unwrap();
        assert_ne!(wrong.owner_id, canonical.owner_id);

        remove_dotagents_deployment_with(
            DotagentsRemovalContext {
                home: &home,
                snapshot: &snapshot,
                skill_name: "foo",
                deployment: canonical,
                scope: InstallScope::Global,
                project_path: None,
            },
            &DotagentsRemovalRunner { fail: false },
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_link(wrong_link).unwrap(),
            PathBuf::from("../../.agents/skills/other")
        );
        assert_eq!(
            std::fs::read_to_string(independent.join("SKILL.md")).unwrap(),
            "independent"
        );
    }

    #[test]
    fn removing_project_universal_copy_removes_only_exact_backed_links() {
        use super::super::skill_deployment::{
            deployment_id, BackingRelationship, SkillDestination,
        };

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let global = home.join(".agents/skills/foo");
        let selected = project.join(".agents/skills/foo");
        let global_link = home.join(".claude/skills/foo");
        let selected_link = project.join(".claude/skills/foo");
        let independent = project.join(".codex/skills/foo");
        for path in [&global, &selected, &independent] {
            std::fs::create_dir_all(path).unwrap();
            std::fs::write(path.join("SKILL.md"), path.to_string_lossy().as_bytes()).unwrap();
        }
        std::fs::create_dir_all(global_link.parent().unwrap()).unwrap();
        std::fs::create_dir_all(selected_link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&global, &global_link).unwrap();
        std::os::unix::fs::symlink(&selected, &selected_link).unwrap();
        let global_id = deployment_id(
            "foo",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            &global,
        );
        let selected_id = deployment_id(
            "foo",
            "project",
            SkillDestination::Universal,
            "universal",
            project.to_str(),
            &selected,
        );
        let mut snapshot = fixture_snapshot(&selected, None);
        let canonical = &mut snapshot.skills[0].deployments[0];
        canonical.id = selected_id.clone();
        canonical.agent = "shared".to_string();
        canonical.scope = "project".to_string();
        canonical.project_path = Some(project.to_string_lossy().to_string());
        canonical.destination = SkillDestination::Universal;
        canonical.backing = BackingRelationship::Canonical;
        canonical.content_hash =
            super::super::skill_discovery::live_skill_content_hash(&selected).unwrap();
        let mut project_link_deployment = canonical.clone();
        project_link_deployment.id = deployment_id(
            "foo",
            "project",
            SkillDestination::Universal,
            "claude-code",
            project.to_str(),
            &selected_link,
        );
        project_link_deployment.agent = "Claude Code".to_string();
        project_link_deployment.path = selected_link.to_string_lossy().to_string();
        project_link_deployment.is_symlink = true;
        project_link_deployment.backing = BackingRelationship::LinkedTo {
            deployment_id: selected_id.clone(),
        };
        let mut global_link_deployment = project_link_deployment.clone();
        global_link_deployment.id = deployment_id(
            "foo",
            "global",
            SkillDestination::Universal,
            "claude-code",
            None,
            &global_link,
        );
        global_link_deployment.scope = "global".to_string();
        global_link_deployment.project_path = None;
        global_link_deployment.path = global_link.to_string_lossy().to_string();
        global_link_deployment.backing = BackingRelationship::LinkedTo {
            deployment_id: global_id,
        };
        let mut independent_deployment = project_link_deployment.clone();
        independent_deployment.id = "independent".to_string();
        independent_deployment.agent = "Codex".to_string();
        independent_deployment.path = independent.to_string_lossy().to_string();
        independent_deployment.is_symlink = false;
        independent_deployment.destination = SkillDestination::PerHarness;
        independent_deployment.backing = BackingRelationship::Independent;
        snapshot.skills[0].deployments.extend([
            project_link_deployment,
            global_link_deployment,
            independent_deployment,
        ]);

        let expected_link_id = snapshot.skills[0].deployments[1].id.clone();
        let ownership = skill_fork_registry::CopyDeploymentRecord {
            deployment_id: selected_id.clone(),
            name: "foo".to_string(),
            path: selected.clone(),
            scope: InstallScope::Project,
            destination: SkillDestination::Universal,
            slot: "universal".to_string(),
            project_path: Some(project.to_string_lossy().to_string()),
            content_hash: snapshot.skills[0].deployments[0].content_hash.clone(),
            disabled: false,
        };
        let mut registry = skill_fork_registry::ForkRegistry::default();
        registry
            .copies
            .insert(ownership.deployment_id.clone(), ownership.clone());
        let removed_ids = remove_copy_deployment(
            &home,
            &snapshot,
            &snapshot.skills[0].deployments[0],
            &ownership,
            &mut registry,
            |_, _| Ok(()),
        )
        .unwrap();

        assert_eq!(removed_ids, vec![selected_id, expected_link_id]);
        assert!(registry.copies.is_empty());
        assert!(!selected.exists());
        assert!(std::fs::symlink_metadata(selected_link).is_err());
        assert!(global.join("SKILL.md").is_file());
        assert!(std::fs::symlink_metadata(global_link).is_ok());
        assert!(independent.join("SKILL.md").is_file());
    }

    #[test]
    fn copy_remove_registry_failure_restores_canonical_and_dependent_link_ownership() {
        use super::super::skill_deployment::{
            deployment_id, BackingRelationship, DeploymentMutability, SkillDestination,
        };
        use super::super::skill_ownership::LifecycleOwnerKind;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let canonical_path = home.join(".agents/skills/foo");
        let link_path = home.join(".claude/skills/foo");
        std::fs::create_dir_all(&canonical_path).unwrap();
        std::fs::write(canonical_path.join("SKILL.md"), "original").unwrap();
        std::fs::create_dir_all(link_path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../../.agents/skills/foo", &link_path).unwrap();

        let canonical_id = deployment_id(
            "foo",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            &canonical_path,
        );
        let link_id = deployment_id(
            "foo",
            "global",
            SkillDestination::Universal,
            "claude-code",
            None,
            &link_path,
        );
        let mut snapshot = fixture_snapshot(&canonical_path, None);
        let canonical = &mut snapshot.skills[0].deployments[0];
        canonical.id = canonical_id.clone();
        canonical.agent = "shared".to_string();
        canonical.destination = SkillDestination::Universal;
        canonical.owner_kind = LifecycleOwnerKind::Copy;
        canonical.mutability = DeploymentMutability::Mutable;
        canonical.backing = BackingRelationship::Canonical;
        canonical.scope = "global".to_string();
        canonical.content_hash =
            super::super::skill_discovery::live_skill_content_hash(&canonical_path).unwrap();
        let mut link = canonical.clone();
        link.id = link_id.clone();
        link.agent = "Claude Code".to_string();
        link.path = link_path.to_string_lossy().to_string();
        link.is_symlink = true;
        link.backing = BackingRelationship::LinkedTo {
            deployment_id: canonical_id.clone(),
        };
        snapshot.skills[0].deployments.push(link);
        let snapshot_before = serde_json::to_value(&snapshot.skills[0].deployments).unwrap();

        let ownership = skill_fork_registry::CopyDeploymentRecord {
            deployment_id: canonical_id.clone(),
            name: "foo".to_string(),
            path: canonical_path.clone(),
            scope: InstallScope::Global,
            destination: SkillDestination::Universal,
            slot: "universal".to_string(),
            project_path: None,
            content_hash: snapshot.skills[0].deployments[0].content_hash.clone(),
            disabled: false,
        };
        let link_ownership = skill_fork_registry::CopyDeploymentRecord {
            deployment_id: link_id.clone(),
            path: link_path.clone(),
            slot: "claude-code".to_string(),
            ..ownership.clone()
        };
        let mut registry = skill_fork_registry::ForkRegistry::default();
        registry
            .copies
            .insert(canonical_id.clone(), ownership.clone());
        registry.copies.insert(link_id.clone(), link_ownership);
        skill_fork_registry::write_fork_registry(&home, &registry).unwrap();

        let error = remove_copy_deployment(
            &home,
            &snapshot,
            &snapshot.skills[0].deployments[0],
            &ownership,
            &mut registry,
            |_, _| Err("injected registry write failure".to_string()),
        )
        .unwrap_err();

        assert!(error.contains("injected registry write failure"), "{error}");
        assert_eq!(
            serde_json::to_value(&snapshot.skills[0].deployments).unwrap(),
            snapshot_before
        );
        assert!(canonical_path.join("SKILL.md").is_file());
        assert_eq!(
            std::fs::read_to_string(canonical_path.join("SKILL.md")).unwrap(),
            "original"
        );
        assert!(std::fs::symlink_metadata(&link_path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_link(&link_path).unwrap(),
            PathBuf::from("../../.agents/skills/foo")
        );
        assert!(registry.copies.contains_key(&canonical_id));
        assert!(registry.copies.contains_key(&link_id));
        let persisted_registry = skill_fork_registry::read_fork_registry(&home).unwrap();
        assert!(persisted_registry.copies.contains_key(&canonical_id));
        assert!(persisted_registry.copies.contains_key(&link_id));
        assert_eq!(
            super::super::skill_discovery::live_skill_content_hash(&canonical_path).unwrap(),
            ownership.content_hash
        );
    }

    #[test]
    fn fork_remove_registry_failure_restores_directory_and_persisted_record() {
        use super::super::skill_fork_registry::{ForkRecord, OriginTool};

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("app-data");
        let skill_dir = home.join(".agents/skills/find-bugs");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "original fork").unwrap();
        let deployment_id = super::super::skill_deployment::deployment_id(
            "find-bugs",
            "global",
            super::super::skill_deployment::SkillDestination::Universal,
            "universal",
            None,
            &skill_dir,
        );
        let content_hash =
            super::super::skill_discovery::live_skill_content_hash(&skill_dir).unwrap();
        let mut registry = skill_fork_registry::read_fork_registry(&home).unwrap();
        registry.forks.insert(
            "find-bugs".to_string(),
            ForkRecord {
                deployment_id: deployment_id.clone(),
                skill_dir: skill_dir.clone(),
                forked_at: "2026-01-01T00:00:00Z".to_string(),
                origin_tool: OriginTool::SkillsSh,
                origin_source: "owner/repo".to_string(),
                repo: "owner/repo".to_string(),
                path: "skills/find-bugs".to_string(),
                declared_ref: None,
                base_commit: "a".repeat(40),
            },
        );
        skill_fork_registry::write_fork_registry(&home, &registry).unwrap();

        let error = remove_forked_skill_with(
            &home,
            &app_data,
            "find-bugs",
            &deployment_id,
            &skill_dir,
            &content_hash,
            |_, _| Err("injected registry write failure".to_string()),
        )
        .unwrap_err();

        assert!(error.contains("injected registry write failure"), "{error}");
        assert_eq!(
            std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
            "original fork"
        );
        assert!(skill_fork_registry::read_fork_registry(&home)
            .unwrap()
            .forks
            .contains_key("find-bugs"));
    }

    fn removable_copy_fixture(
        path: &Path,
    ) -> (
        skill_refresh::SkillSnapshot,
        skill_fork_registry::CopyDeploymentRecord,
    ) {
        use super::super::skill_deployment::{
            deployment_id, BackingRelationship, DeploymentMutability, SkillDestination,
        };
        use super::super::skill_ownership::LifecycleOwnerKind;

        let mut snapshot = fixture_snapshot(path, None);
        let deployment = &mut snapshot.skills[0].deployments[0];
        deployment.id = deployment_id(
            "foo",
            "global",
            SkillDestination::PerHarness,
            "claude-code",
            None,
            path,
        );
        deployment.destination = SkillDestination::PerHarness;
        deployment.owner_kind = LifecycleOwnerKind::Copy;
        deployment.mutability = DeploymentMutability::Mutable;
        deployment.backing = BackingRelationship::Independent;
        deployment.scope = "global".to_string();
        deployment.project_path = None;
        deployment.content_hash =
            super::super::skill_discovery::live_skill_content_hash(path).unwrap();
        let ownership = skill_fork_registry::CopyDeploymentRecord {
            deployment_id: deployment.id.clone(),
            name: "foo".to_string(),
            path: path.to_path_buf(),
            scope: InstallScope::Global,
            destination: SkillDestination::PerHarness,
            slot: "claude-code".to_string(),
            project_path: None,
            content_hash: deployment.content_hash.clone(),
            disabled: false,
        };
        (snapshot, ownership)
    }

    #[test]
    fn copy_remove_refuses_content_edited_after_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("foo");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("SKILL.md"), "original").unwrap();
        let (snapshot, ownership) = removable_copy_fixture(&path);
        std::fs::write(path.join("SKILL.md"), "edited after discovery").unwrap();
        let mut registry = skill_fork_registry::ForkRegistry::default();
        registry
            .copies
            .insert(ownership.deployment_id.clone(), ownership.clone());

        let error = remove_copy_deployment(
            tmp.path(),
            &snapshot,
            &snapshot.skills[0].deployments[0],
            &ownership,
            &mut registry,
            |_, _| Ok(()),
        )
        .unwrap_err();

        assert!(error.contains("content changed after discovery"), "{error}");
        assert_eq!(
            std::fs::read_to_string(path.join("SKILL.md")).unwrap(),
            "edited after discovery"
        );
    }

    #[test]
    fn copy_remove_refuses_a_replaced_path_without_deleting_replacement() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("foo");
        let replacement = tmp.path().join("replacement");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("SKILL.md"), "original").unwrap();
        let (snapshot, ownership) = removable_copy_fixture(&path);
        std::fs::remove_dir_all(&path).unwrap();
        std::fs::create_dir_all(&replacement).unwrap();
        std::fs::write(replacement.join("SKILL.md"), "replacement").unwrap();
        std::os::unix::fs::symlink(&replacement, &path).unwrap();
        let mut registry = skill_fork_registry::ForkRegistry::default();
        registry
            .copies
            .insert(ownership.deployment_id.clone(), ownership.clone());

        let error = remove_copy_deployment(
            tmp.path(),
            &snapshot,
            &snapshot.skills[0].deployments[0],
            &ownership,
            &mut registry,
            |_, _| Ok(()),
        )
        .unwrap_err();

        assert!(
            error.contains("no longer the selected Copy directory"),
            "{error}"
        );
        assert!(std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(replacement.join("SKILL.md")).unwrap(),
            "replacement"
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

    #[test]
    fn skills_sh_access_info_is_server_mode_before_a_key_is_set() {
        let tmp = tempfile::tempdir().unwrap();
        let info = skills_sh_access_info(tmp.path()).unwrap();
        assert_eq!(info.mode, "server");
        assert_eq!(info.server_url, Some("http://127.0.0.1:8787".to_string()));
    }

    #[test]
    fn save_skills_sh_api_key_then_access_info_is_direct() {
        let tmp = tempfile::tempdir().unwrap();
        save_skills_sh_api_key(tmp.path(), "sk-test-key").unwrap();
        let info = skills_sh_access_info(tmp.path()).unwrap();
        assert_eq!(info.mode, "direct");
        assert_eq!(info.server_url, None);
    }

    #[test]
    fn save_skills_sh_api_key_rejects_a_blank_key() {
        let tmp = tempfile::tempdir().unwrap();
        let err = save_skills_sh_api_key(tmp.path(), "   ").unwrap_err();
        assert!(err.contains("can't be empty"));
    }

    #[test]
    fn save_skills_sh_api_key_preserves_other_registry_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let mut registry = skill_fork_registry::read_fork_registry(tmp.path()).unwrap();
        registry.preferred_editor = Some("Visual Studio Code".to_string());
        skill_fork_registry::write_fork_registry(tmp.path(), &registry).unwrap();

        save_skills_sh_api_key(tmp.path(), "sk-test-key").unwrap();

        let round_tripped = skill_fork_registry::read_fork_registry(tmp.path()).unwrap();
        assert_eq!(
            round_tripped.preferred_editor,
            Some("Visual Studio Code".to_string())
        );
        assert_eq!(
            round_tripped.skills_sh_api_key,
            Some("sk-test-key".to_string())
        );
    }
}

/// List project directories discovered from Codex config and Claude Code
/// transcripts that have a first-class agent's skill directory. Returns the
/// background snapshot's project list when one exists.
#[tauri::command]
pub fn list_skill_projects(
    refresh_state: tauri::State<SkillRefreshState>,
) -> Result<Vec<String>, String> {
    if let Ok(guard) = refresh_state.snapshot.read() {
        if let Some(snapshot) = guard.as_ref() {
            return Ok(snapshot.projects.clone());
        }
    }

    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    Ok(project_discovery::discover_skill_projects(&home)
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect())
}

/// Check if a skill is installed
#[tauri::command]
pub fn is_skill_installed(skill_name: String) -> Result<bool, String> {
    lock_file::is_skill_installed(&skill_name)
}

/// Get all supported agent targets
#[tauri::command]
pub fn get_agent_targets() -> Vec<AgentTarget> {
    let home = dirs::home_dir().unwrap_or_default();
    let home_str = home.to_string_lossy();

    AgentId::all()
        .into_iter()
        .map(|id| AgentTarget {
            name: id.display_name().to_string(),
            project_path: id.project_path().to_string(),
            global_path: format!("{}/{}", home_str, id.global_path()),
            id,
        })
        .collect()
}

fn remove_copy_deployment(
    home: &Path,
    snapshot: &skill_refresh::SkillSnapshot,
    deployment: &super::skill_dto::Deployment,
    ownership: &skill_fork_registry::CopyDeploymentRecord,
    registry: &mut skill_fork_registry::ForkRegistry,
    write_registry: impl FnOnce(&Path, &skill_fork_registry::ForkRegistry) -> Result<(), String>,
) -> Result<Vec<String>, String> {
    use super::skill_deployment::BackingRelationship;

    let deployment_path = Path::new(&deployment.path);
    let parsed = super::skill_deployment::parse_deployment_id(&deployment.id)
        .ok_or_else(|| format!("Remove refused: invalid deployment id {}", deployment.id))?;
    let expected_scope = match ownership.scope {
        super::skill_dto::InstallScope::Global => "global",
        super::skill_dto::InstallScope::Project => "project",
    };
    if ownership.deployment_id != deployment.id
        || ownership.path != deployment_path
        || ownership.destination != deployment.destination
        || ownership.project_path != deployment.project_path
        || ownership.disabled != deployment.disabled
        || parsed.scope != expected_scope
        || parsed.destination != ownership.destination
        || parsed.slot != ownership.slot
        || parsed.project_path != ownership.project_path
        || parsed.lexical_path != ownership.path
    {
        return Err(
            "Remove refused: Copy ownership identity no longer matches the selected deployment"
                .to_string(),
        );
    }
    let metadata = std::fs::symlink_metadata(deployment_path).map_err(|error| {
        format!(
            "Remove refused: failed to inspect {}: {error}",
            deployment.path
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "Remove refused: {} is no longer the selected Copy directory",
            deployment.path
        ));
    }
    let live_hash = super::skill_discovery::live_skill_content_hash(deployment_path)?;
    if deployment.content_hash.is_empty()
        || ownership.content_hash.is_empty()
        || live_hash != deployment.content_hash
        || live_hash != ownership.content_hash
    {
        return Err(format!(
            "Remove refused: {} content changed after discovery",
            deployment.path
        ));
    }
    let mut removals = vec![(deployment.id.clone(), deployment_path.to_path_buf())];
    if deployment.destination == super::skill_deployment::SkillDestination::Universal
        && matches!(deployment.backing, BackingRelationship::Canonical)
    {
        let expected_target = std::fs::canonicalize(deployment_path)
            .map_err(|error| format!("Failed to resolve {}: {error}", deployment.path))?;
        for candidate in snapshot
            .skills
            .iter()
            .flat_map(|skill| skill.deployments.iter())
        {
            if candidate.scope != deployment.scope
                || candidate.project_path != deployment.project_path
                || !matches!(
                    &candidate.backing,
                    BackingRelationship::LinkedTo { deployment_id }
                        if deployment_id == &deployment.id
                )
            {
                continue;
            }
            let link = PathBuf::from(&candidate.path);
            let metadata = std::fs::symlink_metadata(&link)
                .map_err(|error| format!("Failed to verify {}: {error}", link.display()))?;
            if !metadata.file_type().is_symlink() {
                return Err(format!(
                    "Remove refused: {} is no longer a dependent symlink",
                    link.display()
                ));
            }
            let actual_target = std::fs::canonicalize(&link)
                .map_err(|error| format!("Failed to resolve {}: {error}", link.display()))?;
            if actual_target != expected_target {
                return Err(format!(
                    "{} no longer points to the selected Universal deployment",
                    link.display()
                ));
            }
            removals.push((candidate.id.clone(), link));
        }
    }

    let stage_id = COPY_REMOVAL_COUNTER.fetch_add(1, Ordering::SeqCst);
    let stage_root = home
        .join(".agents")
        .join("skills-trash")
        .join(format!(".copy-remove-{}-{stage_id}", std::process::id()));
    std::fs::create_dir_all(&stage_root)
        .map_err(|error| format!("Failed to create {}: {error}", stage_root.display()))?;
    let staged: Vec<_> = removals
        .iter()
        .enumerate()
        .map(|(index, (_, path))| {
            let backup = stage_root.join(index.to_string());
            super::event_store::copy_recursive(path, &backup)?;
            let original_fingerprint = super::event_store::fingerprint_path(path);
            let backup_fingerprint = super::event_store::fingerprint_path(&backup);
            if original_fingerprint != backup_fingerprint {
                return Err(format!(
                    "Copy removal backup verification failed for {}",
                    path.display()
                ));
            }
            Ok((path.clone(), backup))
        })
        .collect::<Result<_, String>>()?;

    for (path, _) in &staged {
        if let Err(error) = remove_copy_path(path) {
            let rollback = restore_copy_removal_paths(&staged);
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback_error) => {
                    format!("{error}; failed to restore Copy removal backup: {rollback_error}")
                }
            });
        }
    }

    let original_registry = registry.clone();
    let removed_ids: Vec<String> = removals.iter().map(|(id, _)| id.clone()).collect();
    for removed_id in &removed_ids {
        registry.copies.remove(removed_id);
    }
    registry.trials.retain(|_, trial| {
        trial.deployment_id != deployment.id
            && !removals.iter().any(|(_, path)| {
                trial.skill_dir == *path || trial.claude_link.as_ref() == Some(path)
            })
    });

    if let Err(write_error) = write_registry(home, registry) {
        *registry = original_registry;
        let rollback = restore_copy_removal_paths(&staged);
        return Err(match rollback {
            Ok(()) => format!(
                "Failed to persist Copy ownership removal; restored every deployment: {write_error}"
            ),
            Err(rollback_error) => format!(
                "Failed to persist Copy ownership removal ({write_error}) and failed to restore every deployment from {}: {rollback_error}",
                stage_root.display()
            ),
        });
    }

    if let Err(error) = std::fs::remove_dir_all(&stage_root) {
        eprintln!(
            "[remove_skill] Copy removal succeeded, but backup cleanup failed at {}: {error}",
            stage_root.display()
        );
    }
    Ok(removed_ids)
}

static COPY_REMOVAL_COUNTER: AtomicU64 = AtomicU64::new(0);
static DOTAGENTS_LINK_REMOVAL_COUNTER: AtomicU64 = AtomicU64::new(0);

fn remove_copy_path(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("Failed to inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .map_err(|error| format!("Failed to remove {}: {error}", path.display()))
}

fn restore_copy_removal_paths(staged: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    for (path, backup) in staged {
        if std::fs::symlink_metadata(path).is_ok() {
            remove_copy_path(path)?;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
        }
        super::event_store::copy_recursive(backup, path)?;
        if super::event_store::fingerprint_path(path)
            != super::event_store::fingerprint_path(backup)
        {
            return Err(format!("Restored Copy does not match {}", backup.display()));
        }
    }
    Ok(())
}

fn restore_staged_dotagents_links(staged: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    for (original, backup) in staged.iter().rev() {
        if std::fs::symlink_metadata(original).is_ok() {
            return Err(format!(
                "Refusing to replace {} while restoring its staged Claude link",
                original.display()
            ));
        }
        if let Some(parent) = original.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
        }
        std::fs::rename(backup, original).map_err(|error| {
            format!(
                "Failed to restore staged Claude link {}: {error}",
                original.display()
            )
        })?;
    }
    Ok(())
}

struct DotagentsRemovalContext<'a> {
    home: &'a Path,
    snapshot: &'a skill_refresh::SkillSnapshot,
    skill_name: &'a str,
    deployment: &'a super::skill_dto::Deployment,
    scope: InstallScope,
    project_path: Option<&'a Path>,
}

fn remove_dotagents_deployment_with(
    context: DotagentsRemovalContext<'_>,
    runner: &dyn CommandRunner,
    cleanup_stage: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    use super::skill_deployment::{parse_deployment_id, BackingRelationship, SkillDestination};

    let DotagentsRemovalContext {
        home,
        snapshot,
        skill_name,
        deployment,
        scope,
        project_path,
    } = context;

    if deployment.destination != SkillDestination::Universal
        || !matches!(deployment.backing, BackingRelationship::Canonical)
    {
        return Err("dotagents removal requires its canonical Universal deployment".to_string());
    }
    let canonical_path = std::fs::canonicalize(&deployment.path)
        .map_err(|error| format!("Failed to resolve {}: {error}", deployment.path))?;
    let mut links = Vec::new();
    for candidate in snapshot
        .skills
        .iter()
        .flat_map(|skill| skill.deployments.iter())
    {
        let parsed_candidate = parse_deployment_id(&candidate.id);
        let is_exact_dependent_link = candidate.scope == deployment.scope
            && candidate.project_path == deployment.project_path
            && candidate.owner_id == deployment.owner_id
            && candidate.destination == SkillDestination::Universal
            && parsed_candidate.as_ref().is_some_and(|parsed| {
                parsed.name == skill_name
                    && parsed.scope == deployment.scope
                    && parsed.project_path == deployment.project_path
                    && parsed.lexical_path == Path::new(&candidate.path)
            })
            && matches!(
                &candidate.backing,
                BackingRelationship::LinkedTo { deployment_id }
                    if deployment_id == &deployment.id
            );
        if !is_exact_dependent_link {
            continue;
        }
        if candidate.resolved_path.as_deref().map(Path::new) != Some(canonical_path.as_path()) {
            return Err(format!(
                "dotagents removal refused: {} no longer resolves to the selected Universal deployment",
                candidate.path
            ));
        }
        let link = PathBuf::from(&candidate.path);
        let metadata = std::fs::symlink_metadata(&link).map_err(|error| {
            format!(
                "dotagents removal refused: failed to inspect dependent link {}: {error}",
                link.display()
            )
        })?;
        if candidate.shared_via_whole_dir_link && !candidate.is_symlink {
            if !metadata.is_dir()
                || std::fs::canonicalize(&link).ok().as_ref() != Some(&canonical_path)
            {
                return Err(format!(
                    "dotagents removal refused: {} is no longer a verified whole-root child",
                    link.display()
                ));
            }
            continue;
        }
        if !candidate.is_symlink
            || !metadata.file_type().is_symlink()
            || std::fs::canonicalize(&link).ok().as_ref() != Some(&canonical_path)
        {
            return Err(format!(
                "dotagents removal refused: {} no longer points to the selected Universal deployment",
                link.display()
            ));
        }
        links.push(link);
    }

    let stage_id = DOTAGENTS_LINK_REMOVAL_COUNTER.fetch_add(1, Ordering::SeqCst);
    let stage_root = home.join(".agents").join("skills-trash").join(format!(
        ".dotagents-link-remove-{}-{stage_id}",
        std::process::id()
    ));
    let mut staged = Vec::new();
    if !links.is_empty() {
        std::fs::create_dir_all(&stage_root)
            .map_err(|error| format!("Failed to create {}: {error}", stage_root.display()))?;
        for (index, link) in links.iter().enumerate() {
            let backup = stage_root.join(index.to_string());
            if let Err(error) = std::fs::rename(link, &backup) {
                let rollback = restore_staged_dotagents_links(&staged);
                return Err(match rollback {
                    Ok(()) => format!("Failed to stage Claude link {}: {error}", link.display()),
                    Err(rollback_error) => format!(
                        "Failed to stage Claude link {}: {error}; recovery links remain at {}: {rollback_error}",
                        link.display(),
                        stage_root.display()
                    ),
                });
            }
            staged.push((link.clone(), backup));
        }
    }

    let args = dotagents_remove_args(skill_name, scope);
    if let Err(cli_error) = runner.run_npx(&args, project_path) {
        return Err(match restore_staged_dotagents_links(&staged) {
            Ok(()) => cli_error,
            Err(restore_error) => format!(
                "dotagents removal failed: {cli_error}; Claude link recovery remains at {}: {restore_error}",
                stage_root.display()
            ),
        });
    }

    if !staged.is_empty() {
        cleanup_stage(&stage_root).map_err(|cleanup_error| {
            format!(
                "dotagents removed {skill_name}, but staged Claude link cleanup failed at {}: {cleanup_error}. The staged links remain there for recovery.",
                stage_root.display()
            )
        })?;
    }
    Ok(())
}

/// Remove a skill using npx skills CLI. `project_path` is `None` for a
/// global removal, or the project directory to remove from - validated
/// against the snapshot and passed as the CLI's `current_dir` so the removal
/// can't land on the desktop process's own cwd.
#[tauri::command]
pub async fn remove_skill(
    target: LifecycleTarget,
    app: tauri::AppHandle,
    refresh_state: tauri::State<'_, SkillRefreshState>,
    fork_lock: tauri::State<'_, skill_fork::ForkMutationLock>,
) -> Result<InstallResult, String> {
    // Held for the whole removal (ownership check, CLI removal or direct
    // delete, registry update, rebuild) so a concurrent fork/pull/unfork
    // can't race a removal - `ForkMutationLock` isn't reentrant, so
    // `remove_forked_skill` must not acquire it again itself.
    let _guard = fork_lock.try_acquire()?;

    let snapshot = rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    let (skill, deployment) = resolve_lifecycle_target(&snapshot, &target, "Remove")?;
    let skill_name = skill.name;
    let scope = if deployment.scope == "global" {
        super::skill_dto::InstallScope::Global
    } else if deployment.scope == "project" {
        super::skill_dto::InstallScope::Project
    } else {
        return Err(format!(
            "Remove is not available for {} scope",
            deployment.scope
        ));
    };
    let project_path = deployment.project_path.clone();
    let global = scope == super::skill_dto::InstallScope::Global;

    // A forked skill is a plain directory under `.agents/skills`, in no
    // ledger the CLI could remove from - delete it directly and drop its
    // fork-registry record and snapshot instead of shelling out. Forks only
    // ever live in the shared global folder, so this only applies globally.
    let is_fork =
        global && deployment.owner_kind == super::skill_ownership::LifecycleOwnerKind::Fork;
    if is_fork {
        return remove_forked_skill(
            skill_name,
            deployment.id,
            deployment.path,
            deployment.content_hash,
            app,
        );
    }

    let trial_scope = if global {
        skill_fork_registry::TrialScope::Global
    } else {
        skill_fork_registry::TrialScope::Project
    };
    if deployment.owner_kind == super::skill_ownership::LifecycleOwnerKind::Dotagents {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        remove_dotagents_deployment_with(
            DotagentsRemovalContext {
                home: &home,
                snapshot: &snapshot,
                skill_name: &skill_name,
                deployment: &deployment,
                scope,
                project_path: project_path.as_deref().map(Path::new),
            },
            &RealCommandRunner,
            |stage_root| std::fs::remove_dir_all(stage_root).map_err(|error| error.to_string()),
        )?;
        skill_trial::drop_trial_record(
            &home,
            &deployment.id,
            &skill_name,
            trial_scope,
            Path::new(&deployment.path),
        )
        .map_err(|error| {
            format!(
                "dotagents removed {skill_name}, but Skill Studio could not clear its trial record: {error}"
            )
        })?;
        skill_refresh::request_snapshot_rebuild(&app);
        return Ok(InstallResult {
            success: true,
            skill_name,
            installed_path: None,
            error: None,
            tool: Some("dotagents".to_string()),
            command: None,
        });
    }
    let args = match deployment.owner_kind {
        super::skill_ownership::LifecycleOwnerKind::SkillsSh => {
            skills_sh_remove_args_for_scope(&skill_name, scope.clone())
        }
        super::skill_ownership::LifecycleOwnerKind::Dotagents => unreachable!(),
        super::skill_ownership::LifecycleOwnerKind::Copy => {
            let home = dirs::home_dir().ok_or("Could not find home directory")?;
            let mut registry = skill_fork_registry::read_fork_registry(&home)?;
            let copy_record = registry
                .copies
                .get(&deployment.id)
                .cloned()
                .ok_or("Remove is not available: Copy ownership record is missing")?;
            if copy_record.deployment_id != deployment.id
                || copy_record.name != skill_name
                || copy_record.path.as_path() != Path::new(&deployment.path)
                || copy_record.scope != scope
                || copy_record.destination != deployment.destination
                || copy_record.project_path != deployment.project_path
                || copy_record.disabled != deployment.disabled
            {
                return Err(
                    "Remove is not available: Copy ownership record does not match the selected deployment"
                        .to_string(),
                );
            }
            remove_copy_deployment(
                &home,
                &snapshot,
                &deployment,
                &copy_record,
                &mut registry,
                skill_fork_registry::write_fork_registry,
            )?;
            skill_refresh::request_snapshot_rebuild(&app);
            return Ok(InstallResult {
                success: true,
                skill_name,
                installed_path: None,
                error: None,
                tool: Some("copy".to_string()),
                command: None,
            });
        }
        _ => return Err("Remove is not available for this deployment owner".to_string()),
    };

    // Log the command for debugging
    eprintln!("[remove_skill] Running: npx {}", args.join(" "));

    let mut command = Command::new("npx");
    command.args(&args);
    if let Some(path) = &project_path {
        command.current_dir(path);
    }
    let output = command
        .output()
        .map_err(|e| format!("Failed to execute npx skills: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    eprintln!("[remove_skill] Exit code: {:?}", output.status.code());
    eprintln!("[remove_skill] stdout: {}", stdout);
    eprintln!("[remove_skill] stderr: {}", stderr);

    if output.status.success() {
        if let Some(home) = dirs::home_dir() {
            if let Err(e) = skill_trial::drop_trial_record(
                &home,
                &deployment.id,
                &skill_name,
                trial_scope,
                Path::new(&deployment.path),
            ) {
                eprintln!("[remove_skill] failed to drop trial record: {e}");
            }
        }
        skill_refresh::request_snapshot_rebuild(&app);
        Ok(InstallResult {
            success: true,
            skill_name,
            installed_path: None,
            error: None,
            tool: None,
            command: None,
        })
    } else {
        Ok(InstallResult {
            success: false,
            skill_name,
            installed_path: None,
            error: Some(if stderr.is_empty() { stdout } else { stderr }),
            tool: None,
            command: None,
        })
    }
}

/// `remove_skill`'s path for a forked skill: it's not in any ledger, so
/// there's nothing for a CLI to remove - delete the directory directly and
/// drop the fork-registry record and snapshot.
fn remove_forked_skill(
    skill_name: String,
    deployment_id: String,
    deployment_path: String,
    deployment_content_hash: String,
    app: tauri::AppHandle,
) -> Result<InstallResult, String> {
    // Callers hold `ForkMutationLock` for the whole `remove_skill` call - the
    // mutex isn't reentrant, so this function must not acquire it again.
    validate_skill_dir_name(&skill_name)?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let app_data = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    remove_forked_skill_with(
        &home,
        &app_data,
        &skill_name,
        &deployment_id,
        Path::new(&deployment_path),
        &deployment_content_hash,
        skill_fork_registry::write_fork_registry,
    )?;

    skill_refresh::request_snapshot_rebuild(&app);
    Ok(InstallResult {
        success: true,
        skill_name,
        installed_path: None,
        error: None,
        tool: None,
        command: None,
    })
}

fn remove_forked_skill_with(
    home: &Path,
    app_data: &Path,
    skill_name: &str,
    deployment_id: &str,
    skill_dir: &Path,
    deployment_content_hash: &str,
    write_registry: impl FnOnce(&Path, &skill_fork_registry::ForkRegistry) -> Result<(), String>,
) -> Result<(), String> {
    let registry = skill_fork_registry::read_fork_registry(home)?;
    let record = registry
        .forks
        .get(skill_name)
        .cloned()
        .ok_or_else(|| format!("`{skill_name}` is not forked"))?;
    if (!record.deployment_id.is_empty() && record.deployment_id != deployment_id)
        || (!record.skill_dir.as_os_str().is_empty() && record.skill_dir != skill_dir)
    {
        return Err("The fork record does not belong to the selected deployment".to_string());
    }
    let metadata = std::fs::symlink_metadata(skill_dir)
        .map_err(|error| format!("Failed to inspect {}: {error}", skill_dir.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "Fork removal refused: {} is no longer the selected directory",
            skill_dir.display()
        ));
    }
    let live_hash = super::skill_discovery::live_skill_content_hash(skill_dir)?;
    if deployment_content_hash.is_empty() || live_hash != deployment_content_hash {
        return Err(format!(
            "Fork removal refused: {} content changed after discovery",
            skill_dir.display()
        ));
    }

    let original_fingerprint = super::event_store::fingerprint_path(skill_dir);
    let stage_id = FORK_REMOVAL_COUNTER.fetch_add(1, Ordering::SeqCst);
    let backup = home
        .join(".agents")
        .join("skills-trash")
        .join(format!(".fork-remove-{}-{stage_id}", std::process::id()));
    std::fs::create_dir_all(backup.parent().expect("backup has a parent"))
        .map_err(|error| format!("Failed to create fork removal trash: {error}"))?;
    std::fs::rename(skill_dir, &backup).map_err(|error| {
        format!(
            "Failed to stage {} for removal at {}: {error}",
            skill_dir.display(),
            backup.display()
        )
    })?;
    if super::event_store::fingerprint_path(&backup) != original_fingerprint {
        let _ = std::fs::rename(&backup, skill_dir);
        return Err("Fork removal backup verification failed".to_string());
    }

    let mut updated_registry = registry.clone();
    updated_registry.forks.remove(skill_name);
    // Forking only ever applies to the global scope (see `skill_fork`), so
    // a forked skill's trial, if any, is always keyed as global.
    updated_registry
        .trials
        .remove(&skill_fork_registry::trial_key(
            skill_fork_registry::TrialScope::Global,
            skill_name,
        ));
    updated_registry
        .trials
        .remove(&skill_fork_registry::deployment_trial_key(deployment_id));
    if let Err(write_error) = write_registry(home, &updated_registry) {
        let restore_result = std::fs::rename(&backup, skill_dir);
        return Err(match restore_result {
            Ok(()) => format!(
                "Failed to persist fork removal; restored {skill_name}: {write_error}"
            ),
            Err(restore_error) => format!(
                "Failed to persist fork removal ({write_error}) and failed to restore {} from {}: {restore_error}",
                skill_dir.display(),
                backup.display()
            ),
        });
    }
    if let Err(error) = std::fs::remove_dir_all(&backup) {
        eprintln!(
            "[remove_skill] fork removal succeeded, but backup cleanup failed at {}: {error}",
            backup.display()
        );
    }
    let _ = std::fs::remove_dir_all(skill_fork_registry::fork_snapshot_dir(app_data, skill_name));
    Ok(())
}

static FORK_REMOVAL_COUNTER: AtomicU64 = AtomicU64::new(0);

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
        std::fs::canonicalize(path_buf).map_err(|e| format!("Failed to open {}: {}", path, e))?;
    let is_file = std::fs::symlink_metadata(&canonical)
        .map(|m| m.is_file())
        .unwrap_or(false);
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
pub fn read_installed_skill_md(
    path: String,
    refresh_state: tauri::State<SkillRefreshState>,
) -> Result<String, String> {
    let path_buf = std::path::PathBuf::from(&path);
    require_snapshot_owns_path(&refresh_state, &path_buf)?;
    canonicalize_skill_md(&path_buf, &path)?;

    let mut file = File::open(&path).map_err(|e| format!("Failed to open {}: {}", path, e))?;
    let mut buf = vec![0u8; MAX_SKILL_MD_BYTES];
    let n = file
        .read(&mut buf)
        .map_err(|e| format!("Failed to read {}: {}", path, e))?;
    buf.truncate(n);
    Ok(String::from_utf8_lossy(&buf).into_owned())
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

/// Counter appended to the atomic-write temp filename, on top of the pid and
/// a timestamp, so two saves landing in the same process within the same
/// nanosecond still get distinct temp files.
static ATOMIC_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `content` to `canonical` atomically: a temp file in the same
/// directory, then a rename, so a crash mid-write can't leave a truncated
/// `SKILL.md` behind. Pulled out of the command so it's testable with a
/// plain tempdir, no snapshot or `tauri::AppHandle` needed.
///
/// The temp filename is unique per call (pid + a process-wide counter +
/// wall-clock nanos) and created with `create_new` so a concurrent save, or a
/// pre-existing symlink at that path, can't be interleaved or truncated.
pub(crate) fn atomic_write_skill_md(
    canonical: &std::path::Path,
    content: &str,
) -> Result<(), String> {
    let parent = canonical.parent().ok_or_else(|| {
        format!(
            "Failed to resolve parent directory of {}",
            canonical.display()
        )
    })?;
    let counter = ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_path = parent.join(format!(
        ".SKILL.md.tmp-{}-{}-{}",
        std::process::id(),
        counter,
        nanos
    ));

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .map_err(|e| format!("Failed to create {}: {}", tmp_path.display(), e))?;
    let write_result = file
        .write_all(content.as_bytes())
        .and_then(|_| file.sync_all());
    drop(file);
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("Failed to write {}: {}", tmp_path.display(), e));
    }

    std::fs::rename(&tmp_path, canonical).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to save {}: {}", canonical.display(), e)
    })
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
/// drawer's inline editor. Same ownership check as `read_installed_skill_md`,
/// plus a refusal when the owning deployment is plugin-managed. Marks the
/// snapshot dirty afterward so the background loop picks up the new content
/// and token/byte counts, rather than rescanning every skill on this thread.
#[tauri::command]
pub fn write_installed_skill_md(
    path: String,
    content: String,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
) -> Result<(), String> {
    let canonical = validate_skill_md_write(&path, &content, &refresh_state)?;
    atomic_write_skill_md(&canonical, &content)?;
    skill_refresh::request_snapshot_rebuild(&app);
    Ok(())
}

/// Refuses when `canonical`'s current content differs from `expected_content`
/// (the file drifted on disk since the caller loaded it), otherwise writes
/// atomically. Pulled out of the command so it's testable without a snapshot
/// or `tauri::AppHandle`.
pub(crate) fn write_skill_md_compare_and_swap(
    canonical: &std::path::Path,
    expected_content: &str,
    content: &str,
) -> Result<(), String> {
    let current = std::fs::read_to_string(canonical)
        .map_err(|e| format!("Failed to open {}: {}", canonical.display(), e))?;
    if current != expected_content {
        return Err(
            "SKILL.md changed on disk since it was loaded. Reload the file and run the audit again."
                .to_string(),
        );
    }
    atomic_write_skill_md(canonical, content)
}

/// Like `write_installed_skill_md`, but refuses the write (rather than
/// silently overwriting) when the file's current content doesn't match
/// `expected_content` - the copy the caller last loaded. Used by the Audit
/// proposal's Apply action so a save made elsewhere while the proposal was
/// open can't be clobbered.
#[tauri::command]
pub fn write_installed_skill_md_if_unchanged(
    path: String,
    expected_content: String,
    content: String,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
) -> Result<(), String> {
    let canonical = validate_skill_md_write(&path, &content, &refresh_state)?;
    write_skill_md_compare_and_swap(&canonical, &expected_content, &content)?;
    skill_refresh::request_snapshot_rebuild(&app);
    Ok(())
}

/// Reveal a skill's folder in Finder, or open it in the user's default
/// editor, via macOS's `open` CLI. Restricted to paths belonging to a
/// deployment in the current snapshot.
#[tauri::command]
pub fn open_skill_path(
    path: String,
    mode: String,
    refresh_state: tauri::State<SkillRefreshState>,
) -> Result<(), String> {
    require_snapshot_owns_path(&refresh_state, std::path::Path::new(&path))?;

    let args = match mode.as_str() {
        "reveal" => vec!["-R".to_string()],
        // `-t` would mean the system default *text* editor, which is TextEdit
        // on a stock machine - see `skill_editor` for the setting behind this.
        "editor" => {
            let home = dirs::home_dir().ok_or("Could not find home directory")?;
            skill_editor::open_editor_args(
                skill_editor::preferred_editor(&home).as_deref(),
                &skill_editor::installed_editors(&home),
            )
        }
        other => return Err(format!("Unknown open mode: {other}")),
    };

    Command::new("open")
        .args(&args)
        .arg(&path)
        .output()
        .map_err(|e| format!("Failed to open {}: {}", path, e))?;
    Ok(())
}

/// The editors installed on this machine, for the Settings picker.
#[tauri::command]
pub fn list_installed_editors() -> Result<Vec<skill_editor::EditorOption>, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    Ok(skill_editor::installed_editors(&home))
}

/// The application "Open in editor" currently uses, or `None` for the system
/// default.
#[tauri::command]
pub fn get_preferred_editor() -> Result<Option<String>, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    Ok(skill_editor::preferred_editor(&home))
}

#[tauri::command]
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
    refresh_state: tauri::State<'_, SkillRefreshState>,
    update_check_state: tauri::State<'_, skill_update_check::UpdateCheckState>,
    fork_lock: tauri::State<'_, skill_fork::ForkMutationLock>,
) -> Result<InstallResult, String> {
    let _guard = fork_lock.try_acquire()?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
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
    let project_paths: Vec<std::path::PathBuf> = snapshot.projects.iter().map(Into::into).collect();
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
                dotagents_update_args(&skill_name, entry, latest_commit.as_deref(), scope.clone())?;
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
        .map_err(|e| format!("Failed to execute npx: {}", e))?;

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
}
