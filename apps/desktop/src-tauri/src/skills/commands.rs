// ============================================================================
// Skills Module - Tauri Commands
// IPC commands for skill discovery, installation, and management
// ============================================================================

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use super::agents::{AgentId, AgentTarget};
use super::api;
use super::dotagents_ledger::{self, DotagentsSkill};
use super::lock_file;
use super::project_discovery;
use super::provenance::SourceKind;
use super::skill_agent_runner::validate_skill_dir_name;
use super::skill_dto::{
    InstallRequest, InstallResult, InstalledSkill, PaginatedSkillsResponse, SkillDetails,
    SkillsShAccessInfo,
};
use super::skill_editor;
use super::skill_fork;
use super::skill_fork_registry;
use super::skill_refresh::{self, SkillRefreshState};
use super::skill_trial;
use super::skill_update_check;
use tauri::Manager;

/// The `npx skills add <repo> --yes [--global | --cwd <path>] [--skill
/// <name>]` argv `install_skill` runs, minus the trailing `--agent ...`
/// flags - pulled out so `skill_fork`'s reinstall path can build the same
/// argv without duplicating it.
pub(crate) fn skills_sh_add_args(
    repo_source: &str,
    skill_name: Option<&str>,
    global: bool,
    project_path: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "skills".to_string(),
        "add".to_string(),
        repo_source.to_string(),
    ];
    args.push("--yes".to_string());
    if global {
        args.push("--global".to_string());
    } else if let Some(project_path) = project_path {
        args.push("--cwd".to_string());
        args.push(project_path.to_string());
    }
    if let Some(name) = skill_name {
        args.push("--skill".to_string());
        args.push(name.to_string());
    }
    args
}

/// The `npx skills remove <name> --yes [--global]` argv `remove_skill` runs -
/// pulled out so `skill_fork`'s remove path can build the same argv without
/// duplicating it.
pub(crate) fn skills_sh_remove_args(skill_name: &str, global: bool) -> Vec<String> {
    let mut args = vec![
        "skills".to_string(),
        "remove".to_string(),
        skill_name.to_string(),
        "--yes".to_string(),
    ];
    if global {
        args.push("--global".to_string());
    }
    args
}

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
pub(crate) fn dotagents_remove_args(name: &str) -> Vec<String> {
    vec![
        "-y".to_string(),
        "@sentry/dotagents".to_string(),
        "remove".to_string(),
        name.to_string(),
    ]
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

/// Append `--agent <cli_name>` for each requested agent, run before spawning
/// `npx skills`. Grok Build is scanned for coverage/health but is not an
/// install target: `npx skills` has no entry for it, it only reads
/// `~/.agents/skills`.
pub(crate) fn push_agent_args(args: &mut Vec<String>, agents: &[AgentId]) -> Result<(), String> {
    for agent in agents {
        if *agent == AgentId::GrokBuild {
            return Err(
                "Grok Build is not an npx skills install target; it reads ~/.agents/skills"
                    .to_string(),
            );
        }
        args.push("--agent".to_string());
        args.push(agent.cli_name().to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
                    symlink_target: None,
                    resolved_path: None,
                    symlink_is_broken: false,
                    symlink_error: None,
                    project_path: None,
                    content_hash: String::new(),
                    disabled: false,
                    disabled_by: None,
                    disabled_readers: Vec::new(),
                    codex_implicit_invocation: None,
                    shared_via_whole_dir_link: false,
                    spec_violations: Vec::new(),
                    invocation: super::super::frontmatter::InvocationPolicy::Both,
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

    #[test]
    fn skills_sh_remove_args_selects_global_flag() {
        assert_eq!(
            skills_sh_remove_args("foo", true),
            vec!["skills", "remove", "foo", "--yes", "--global"]
        );
        assert_eq!(
            skills_sh_remove_args("foo", false),
            vec!["skills", "remove", "foo", "--yes"]
        );
    }

    #[test]
    fn validate_remove_project_path_accepts_a_known_project() {
        let dep_dir = std::path::Path::new("/repo/.claude/skills/foo");
        let mut snapshot = fixture_snapshot(dep_dir, None);
        snapshot.skills[0].deployments[0].project_path = Some("/repo".to_string());

        assert!(validate_remove_project_path(Some(&snapshot), "foo", "/repo").is_ok());
    }

    #[test]
    fn validate_remove_project_path_rejects_a_path_not_in_the_snapshot() {
        let dep_dir = std::path::Path::new("/repo/.claude/skills/foo");
        let mut snapshot = fixture_snapshot(dep_dir, None);
        snapshot.skills[0].deployments[0].project_path = Some("/repo".to_string());

        let err = validate_remove_project_path(Some(&snapshot), "foo", "/elsewhere").unwrap_err();
        assert!(err.contains("/elsewhere"), "{err}");
    }

    #[test]
    fn is_in_repo_skill_true_for_in_repo_in_snapshot() {
        let dep_dir = std::path::Path::new("/repo/.claude/skills/my-notes");
        let mut snapshot = fixture_snapshot(dep_dir, None);
        snapshot.skills[0].source_kind = SourceKind::InRepo;
        assert!(is_in_repo_skill(Some(&snapshot), "foo"));
    }

    #[test]
    fn is_in_repo_skill_false_for_other_source_kinds() {
        let dep_dir = std::path::Path::new("/repo/.claude/skills/foo");
        let mut snapshot = fixture_snapshot(dep_dir, None);
        for kind in [
            SourceKind::Manual,
            SourceKind::SkillsSh,
            SourceKind::Fork,
            SourceKind::Dotagents,
            SourceKind::Plugin,
        ] {
            snapshot.skills[0].source_kind = kind;
            assert!(
                !is_in_repo_skill(Some(&snapshot), "foo"),
                "{kind:?} should not classify as in-repo"
            );
        }
    }

    #[test]
    fn is_in_repo_skill_false_when_absent_from_snapshot() {
        let dep_dir = std::path::Path::new("/repo/.claude/skills/foo");
        let snapshot = fixture_snapshot(dep_dir, None);
        assert!(!is_in_repo_skill(Some(&snapshot), "not-installed"));
        assert!(!is_in_repo_skill(None, "foo"));
    }

    #[test]
    fn update_rejection_rejects_in_repo_like_manual() {
        assert_eq!(
            update_rejection(Some(SourceKind::InRepo)),
            Some("Update is not available for manually installed skills")
        );
        assert_eq!(
            update_rejection(Some(SourceKind::Manual)),
            Some("Update is not available for manually installed skills")
        );
        assert_eq!(
            update_rejection(Some(SourceKind::Plugin)),
            Some("Update is not available for manually installed skills")
        );
        assert_eq!(
            update_rejection(Some(SourceKind::Fork)),
            Some("Forked skills update with Pull upstream")
        );
        // SkillsSh, Dotagents, and an unknown skill proceed to the owning CLI.
        assert_eq!(update_rejection(Some(SourceKind::SkillsSh)), None);
        assert_eq!(update_rejection(Some(SourceKind::Dotagents)), None);
        assert_eq!(update_rejection(None), None);
    }

    #[test]
    fn push_agent_args_rejects_grok_build() {
        let mut args = vec!["skills".to_string(), "add".to_string()];
        let err =
            push_agent_args(&mut args, &[AgentId::ClaudeCode, AgentId::GrokBuild]).unwrap_err();
        assert_eq!(
            err,
            "Grok Build is not an npx skills install target; it reads ~/.agents/skills"
        );
    }

    #[test]
    fn push_agent_args_accepts_installable_agents() {
        let mut args = Vec::new();
        push_agent_args(&mut args, &[AgentId::ClaudeCode, AgentId::Cursor]).unwrap();
        assert_eq!(args, vec!["--agent", "claude-code", "--agent", "cursor"]);
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
    ) -> DotagentsSkill {
        DotagentsSkill {
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
    fn dotagents_update_args_rejects_skill_with_no_ledger_entry() {
        let err = dotagents_update_args("manual-in-shared-root", None, None).unwrap_err();
        assert_eq!(
            err,
            "Update is not available: manual-in-shared-root is not in ~/.agents/agents.lock"
        );
    }

    #[test]
    fn dotagents_update_args_wildcard_entry_reinstalls() {
        let entry = dotagents_skill("find-bugs", None, false);
        let args = dotagents_update_args("find-bugs", Some(&entry), None).unwrap();
        assert_eq!(args, vec!["-y", "@sentry/dotagents", "install"]);
    }

    #[test]
    fn dotagents_update_args_named_unpinned_entry_uses_add_without_ref() {
        let entry = dotagents_skill("find-bugs", None, true);
        let args = dotagents_update_args("find-bugs", Some(&entry), None).unwrap();
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
    fn dotagents_update_args_pinned_entry_needs_latest_commit() {
        let entry = dotagents_skill("find-bugs", Some("aaaa"), true);
        let err = dotagents_update_args("find-bugs", Some(&entry), None).unwrap_err();
        assert!(err.contains("Check now"));
    }

    #[test]
    fn dotagents_update_args_pinned_entry_re_pins_to_latest_commit() {
        let entry = dotagents_skill("find-bugs", Some("aaaa"), true);
        let latest = "b".repeat(40);
        let args = dotagents_update_args("find-bugs", Some(&entry), Some(&latest)).unwrap();
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

/// Install a skill using npx skills CLI
#[tauri::command]
pub async fn install_skill(
    request: InstallRequest,
    app: tauri::AppHandle,
) -> Result<InstallResult, String> {
    // Parse skill_source - could be "owner/repo" or "owner/repo/skill-name"
    // or just "skill-name" for well-known skills
    let (repo_source, skill_name) = parse_skill_source(&request.skill_source);

    let mut args = skills_sh_add_args(
        &repo_source,
        skill_name.as_deref(),
        request.scope == super::skill_dto::InstallScope::Global,
        request.project_path.as_deref(),
    );

    // Add agent targets if specified
    push_agent_args(&mut args, &request.agents)?;

    // Log the command for debugging
    eprintln!("[install_skill] Running: npx {}", args.join(" "));

    // Execute npx skills command
    let output = Command::new("npx")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to execute npx skills: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    eprintln!("[install_skill] Exit code: {:?}", output.status.code());
    eprintln!("[install_skill] stdout: {}", stdout);
    eprintln!("[install_skill] stderr: {}", stderr);

    if output.status.success() {
        // Use parsed skill name or fallback
        let result_name = skill_name.unwrap_or_else(|| {
            repo_source
                .split('/')
                .next_back()
                .unwrap_or(&repo_source)
                .to_string()
        });

        // A harness that couldn't be switched off is logged, not returned:
        // `InstallResult` has no warning channel, and the skill is installed.
        if !request.disabled_harnesses.is_empty() {
            let home = dirs::home_dir().ok_or("Could not find home directory")?;
            let codex_dir = if request.scope == super::skill_dto::InstallScope::Global {
                AgentId::Codex.global_skills_dir(&home)
            } else {
                AgentId::Codex.project_skills_dir(std::path::Path::new(
                    request.project_path.as_deref().unwrap_or(""),
                ))
            };
            let codex_paths = vec![codex_dir.join(&result_name).join("SKILL.md")];
            for agent in &request.disabled_harnesses {
                if let Err(e) = super::skill_harness_disable::set_harness_enabled_with(
                    &home,
                    &result_name,
                    agent.cli_name(),
                    false,
                    &codex_paths,
                ) {
                    eprintln!(
                        "[install_skill] could not disable {}: {e}",
                        agent.cli_name()
                    );
                }
            }
        }

        skill_refresh::request_snapshot_rebuild(&app);
        Ok(InstallResult {
            success: true,
            skill_name: result_name,
            installed_path: None,
            error: None,
            tool: None,
            command: None,
        })
    } else {
        Ok(InstallResult {
            success: false,
            skill_name: request.skill_source.clone(),
            installed_path: None,
            error: Some(if stderr.is_empty() { stdout } else { stderr }),
            tool: None,
            command: None,
        })
    }
}

/// Parse skill source into (repo, optional skill name)
/// Examples:
///   "vercel-labs/skills" -> ("vercel-labs/skills", None)
///   "obra/superpowers/brainstorming" -> ("obra/superpowers", Some("brainstorming"))
///   "sentry-cli" -> ("sentry-cli", None) - for well-known skills
fn parse_skill_source(source: &str) -> (String, Option<String>) {
    let parts: Vec<&str> = source.split('/').collect();
    match parts.len() {
        // Well-known skill or single name
        0 | 1 => (source.to_string(), None),
        // owner/repo format
        2 => (source.to_string(), None),
        // owner/repo/skill-name format
        _ => {
            let repo = format!("{}/{}", parts[0], parts[1]);
            let skill = parts[2..].join("/");
            (repo, Some(skill))
        }
    }
}

/// Validates `project_path` against `skill_name`'s known project-scope
/// deployments in `snapshot` - `remove_skill`'s guard against running the CLI
/// (or a fork's directory delete) somewhere other than the process cwd,
/// which is what let "Remove from <project>" silently act on the desktop
/// app's own working directory instead of the project.
pub(crate) fn validate_remove_project_path(
    snapshot: Option<&skill_refresh::SkillSnapshot>,
    skill_name: &str,
    project_path: &str,
) -> Result<(), String> {
    let owns = snapshot
        .and_then(|s| s.skills.iter().find(|sk| sk.name == skill_name))
        .map(|sk| {
            sk.deployments
                .iter()
                .any(|d| d.scope == "project" && d.project_path.as_deref() == Some(project_path))
        })
        .unwrap_or(false);
    if owns {
        Ok(())
    } else {
        Err(format!(
            "{project_path} is not a known project location for {skill_name}"
        ))
    }
}

/// Whether `skill_name` is an in-repo skill in `snapshot` - a plain directory
/// inside a git working tree that no skill-manager ledger tracks (see
/// `provenance::classify_source_kind`). `remove_skill` rejects these up front
/// rather than routing them through `npx skills remove`, which exits 0 on
/// names absent from its lock file and would report a silent success that the
/// snapshot rebuild immediately re-discovers. Pulled out so the dispatch is
/// testable without a `tauri::AppHandle` or `SkillRefreshState`.
pub(crate) fn is_in_repo_skill(
    snapshot: Option<&skill_refresh::SkillSnapshot>,
    skill_name: &str,
) -> bool {
    snapshot
        .and_then(|s| s.skills.iter().find(|s| s.name == skill_name))
        .map(|s| s.source_kind)
        == Some(SourceKind::InRepo)
}

/// Remove a skill using npx skills CLI. `project_path` is `None` for a
/// global removal, or the project directory to remove from - validated
/// against the snapshot and passed as the CLI's `current_dir` so the removal
/// can't land on the desktop process's own cwd.
#[tauri::command]
pub async fn remove_skill(
    skill_name: String,
    project_path: Option<String>,
    app: tauri::AppHandle,
    refresh_state: tauri::State<'_, SkillRefreshState>,
    fork_lock: tauri::State<'_, skill_fork::ForkMutationLock>,
) -> Result<InstallResult, String> {
    // Held for the whole removal (ownership check, CLI removal or direct
    // delete, registry update, rebuild) so a concurrent fork/pull/unfork
    // can't race a removal - `ForkMutationLock` isn't reentrant, so
    // `remove_forked_skill` must not acquire it again itself.
    let _guard = fork_lock.try_acquire()?;

    let snapshot = refresh_state.snapshot.read().ok().and_then(|g| g.clone());
    if let Some(path) = &project_path {
        validate_remove_project_path(snapshot.as_ref(), &skill_name, path)?;
    }
    let global = project_path.is_none();

    // A forked skill is a plain directory under `.agents/skills`, in no
    // ledger the CLI could remove from - delete it directly and drop its
    // fork-registry record and snapshot instead of shelling out. Forks only
    // ever live in the shared global folder, so this only applies globally.
    let is_fork = global
        && snapshot
            .as_ref()
            .and_then(|s| s.skills.iter().find(|s| s.name == skill_name))
            .map(|s| s.source_kind)
            == Some(SourceKind::Fork);
    if is_fork {
        return remove_forked_skill(skill_name, app);
    }

    // An in-repo skill is a plain directory inside a git working tree that no
    // skill-manager ledger tracks - the skills.sh lock file doesn't contain
    // it, so `npx skills remove <name>` exits 0 printing "No skills found to
    // remove" and leaves the directory in place. Reject up front instead of
    // no-op'ing through the CLI and reporting a silent success the snapshot
    // rebuild immediately re-discovers; the directory should be removed from
    // the git working tree directly.
    if is_in_repo_skill(snapshot.as_ref(), &skill_name) {
        return Ok(InstallResult {
            success: false,
            skill_name,
            installed_path: None,
            error: Some(
                "In-repo skills aren't tracked by skills.sh; remove the directory from your git working tree"
                    .to_string(),
            ),
            tool: None,
            command: None,
        });
    }

    let scope = if global {
        skill_fork_registry::TrialScope::Global
    } else {
        skill_fork_registry::TrialScope::Project
    };
    let args = skills_sh_remove_args(&skill_name, global);

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
            if let Err(e) = skill_trial::drop_trial_record(&home, &skill_name, scope) {
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
fn remove_forked_skill(skill_name: String, app: tauri::AppHandle) -> Result<InstallResult, String> {
    // Callers hold `ForkMutationLock` for the whole `remove_skill` call - the
    // mutex isn't reentrant, so this function must not acquire it again.
    validate_skill_dir_name(&skill_name)?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let skill_dir = home.join(".agents").join("skills").join(&skill_name);
    if skill_dir.exists() {
        std::fs::remove_dir_all(&skill_dir)
            .map_err(|e| format!("Failed to remove {}: {e}", skill_dir.display()))?;
    }

    let mut registry = skill_fork_registry::read_fork_registry(&home)?;
    registry.forks.remove(&skill_name);
    // Forking only ever applies to the global scope (see `skill_fork`), so
    // a forked skill's trial, if any, is always keyed as global.
    registry.trials.remove(&skill_fork_registry::trial_key(
        skill_fork_registry::TrialScope::Global,
        &skill_name,
    ));
    skill_fork_registry::write_fork_registry(&home, &registry)?;

    let app_data = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let _ = std::fs::remove_dir_all(skill_fork_registry::fork_snapshot_dir(
        &app_data,
        &skill_name,
    ));

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

/// Build the `npx @sentry/dotagents ...` args for updating one
/// dotagents-managed skill, or the error `update_skill` should return
/// instead of running anything. `entry` is this skill's row from
/// `agents.lock` - `None` when the skill only *looks* dotagents-managed
/// (it's under a shared root next to an `agents.lock`) but was actually
/// dropped in manually, since `provenance::classify_source_kind` can't tell
/// the two apart without the ledger. `latest_commit` is only consulted for a
/// pinned (`declared_ref.is_some()`) entry.
fn dotagents_update_args(
    skill_name: &str,
    entry: Option<&DotagentsSkill>,
    latest_commit: Option<&str>,
) -> Result<Vec<String>, String> {
    let Some(entry) = entry else {
        return Err(format!(
            "Update is not available: {skill_name} is not in ~/.agents/agents.lock"
        ));
    };

    if !entry.has_manifest_row {
        // Wildcard (`--all`) entry: no per-skill row to re-pin, so re-run
        // the whole sync.
        return Ok(vec![
            "-y".to_string(),
            "@sentry/dotagents".to_string(),
            "install".to_string(),
        ]);
    }

    let mut args = vec![
        "-y".to_string(),
        "@sentry/dotagents".to_string(),
        "add".to_string(),
        entry.source.clone(),
        "--name".to_string(),
        skill_name.to_string(),
    ];
    if entry.declared_ref.is_some() {
        match latest_commit {
            Some(latest) => {
                args.push("--ref".to_string());
                args.push(latest.to_string());
            }
            None => {
                return Err(format!(
                    "Update is not available yet: run \"Check now\" to find {skill_name}'s latest commit first"
                ));
            }
        }
    }
    Ok(args)
}

/// `update_skill`'s rejection of source kinds no owning CLI can update
/// through - manual, plugin, and in-repo skills aren't tracked by any
/// skill-manager ledger, so `npx skills update` exits 0 on them without doing
/// anything (it finds no matching name in its lock file); forks update via
/// "Pull upstream" instead. Returns the error message `update_skill` should
/// return, or `None` to proceed with the owning CLI. Pulled out so the
/// dispatch is testable without a `tauri::AppHandle`.
pub(crate) fn update_rejection(source_kind: Option<SourceKind>) -> Option<&'static str> {
    match source_kind {
        Some(SourceKind::Manual) | Some(SourceKind::Plugin) | Some(SourceKind::InRepo) => {
            Some("Update is not available for manually installed skills")
        }
        Some(SourceKind::Fork) => Some("Forked skills update with Pull upstream"),
        _ => None,
    }
}

/// Update a skill, using whichever CLI owns it: `dotagents` for a
/// dotagents-managed skill (`add` re-pins it to the latest commit; `install`
/// re-runs the wildcard sync for a skill with no `[[skills]]` row), `npx
/// skills update` for a skills.sh skill. Manual/plugin/in-repo skills have no
/// owning CLI to update through (the skills.sh lock file doesn't track them),
/// so they're rejected up front.
#[tauri::command]
pub async fn update_skill(
    skill_name: String,
    global: bool,
    app: tauri::AppHandle,
    refresh_state: tauri::State<'_, SkillRefreshState>,
    update_check_state: tauri::State<'_, skill_update_check::UpdateCheckState>,
) -> Result<InstallResult, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let snapshot = refresh_state.snapshot.read().ok().and_then(|g| g.clone());
    let source_kind = snapshot
        .as_ref()
        .and_then(|s| s.skills.iter().find(|s| s.name == skill_name))
        .map(|s| s.source_kind);

    if let Some(msg) = update_rejection(source_kind) {
        return Err(msg.to_string());
    }

    let (tool, mut args): (&str, Vec<String>) = match source_kind {
        Some(SourceKind::Dotagents) => {
            let ledger = dotagents_ledger::read_dotagents_ledger(&home.join(".agents"))?;
            let entry = ledger.iter().find(|s| s.name == skill_name);
            let latest_commit = if entry.is_some_and(|e| e.declared_ref.is_some()) {
                let app_data = app
                    .path()
                    .app_data_dir()
                    .unwrap_or_else(|_| std::path::PathBuf::from("."));
                skill_update_check::read_update_check_store(&app_data)
                    .skills
                    .get(&skill_name)
                    .and_then(|state| state.latest_commit.clone())
            } else {
                None
            };
            let args = dotagents_update_args(&skill_name, entry, latest_commit.as_deref())?;
            ("dotagents", args)
        }
        // SkillsSh, or a skill not found in the snapshot yet - fall back to
        // the CLI that owns everything else.
        _ => (
            "skills-sh",
            vec![
                "skills".to_string(),
                "update".to_string(),
                skill_name.clone(),
            ],
        ),
    };

    if tool == "skills-sh" && global {
        args.push("--global".to_string());
    }

    let npx_command = format!("npx {}", args.join(" "));
    let output = Command::new("npx")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to execute npx: {}", e))?;

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        skill_update_check::check_now_for_skill(&app, &update_check_state, &skill_name);
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
