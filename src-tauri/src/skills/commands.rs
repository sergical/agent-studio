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
use super::skill_dto::{
    InstallRequest, InstallResult, InstalledSkill, PaginatedSkillsResponse, SkillSearchResult,
};
use super::skill_refresh::{self, SkillRefreshState};
use super::skill_update_check;
use tauri::Manager;

/// Search for skills on skills.sh
#[tauri::command]
pub async fn search_skills(
    query: String,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<PaginatedSkillsResponse, String> {
    api::search_skills(&query, limit, offset).await
}

/// Get popular skills (sorted by install count)
#[tauri::command]
pub async fn get_popular_skills(
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<PaginatedSkillsResponse, String> {
    api::get_popular_skills(limit, offset).await
}

/// Get skill details from skills.sh
#[tauri::command]
pub async fn get_skill_details(skill_id: String) -> Result<SkillSearchResult, String> {
    api::get_skill_details(&skill_id).await
}

/// Get all installed skills. Returns the background-refreshed snapshot's
/// skills (see `skill_refresh`) when it already accounts for every path in
/// `project_paths`; otherwise registers the missing paths and rebuilds the
/// snapshot synchronously (so this read-after-write sees fresh data), which
/// also covers the case where the background snapshot hasn't landed yet.
#[tauri::command]
pub fn get_installed_skills(
    project_paths: Option<Vec<String>>,
    refresh_state: tauri::State<SkillRefreshState>,
    app: tauri::AppHandle,
) -> Result<Vec<InstalledSkill>, String> {
    let requested = project_paths.unwrap_or_default();
    let snapshot = refresh_state.snapshot.read().ok().and_then(|g| g.clone());

    if let Some(snapshot) = &snapshot {
        if snapshot_covers_projects(&requested, &snapshot.projects) {
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
fn push_agent_args(args: &mut Vec<String>, agents: &[AgentId]) -> Result<(), String> {
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
                    symlink_is_broken: false,
                    symlink_error: None,
                    project_path: None,
                    content_hash: String::new(),
                }],
                has_spec: false,
                description: None,
                spec_violations: Vec::new(),
                skill_md_tokens: 0,
                folder_bytes: 0,
                file_count: 0,
                content_hash: String::new(),
                content_hashes: Vec::new(),
                modified_at: None,
                frontmatter_fields: BTreeMap::new(),
                folder_truncated: false,
            }],
            projects: Vec::new(),
            invocations: Vec::new(),
            heatmap: InvocationHeatmap::default(),
            scanned_at: Utc::now().to_rfc3339(),
            last_test_by_skill: Default::default(),
            update_check: Default::default(),
        }
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
    refresh_state: tauri::State<'_, SkillRefreshState>,
) -> Result<InstallResult, String> {
    // Parse skill_source - could be "owner/repo" or "owner/repo/skill-name"
    // or just "skill-name" for well-known skills
    let (repo_source, skill_name) = parse_skill_source(&request.skill_source);

    let mut args = vec!["skills".to_string(), "add".to_string(), repo_source.clone()];

    // Always add --yes for non-interactive mode
    args.push("--yes".to_string());

    // Add scope flag
    if request.scope == super::skill_dto::InstallScope::Global {
        args.push("--global".to_string());
    } else if let Some(ref project_path) = request.project_path {
        args.push("--cwd".to_string());
        args.push(project_path.clone());
    }

    // Add specific skill if we have one (for multi-skill repos)
    if let Some(ref name) = skill_name {
        args.push("--skill".to_string());
        args.push(name.clone());
    }

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

        if let Err(e) = skill_refresh::rebuild_snapshot_now(&app, &refresh_state) {
            eprintln!("[install_skill] snapshot rebuild failed: {e}");
        }
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

/// Remove a skill using npx skills CLI
#[tauri::command]
pub async fn remove_skill(
    skill_name: String,
    global: bool,
    app: tauri::AppHandle,
    refresh_state: tauri::State<'_, SkillRefreshState>,
) -> Result<InstallResult, String> {
    let mut args = vec![
        "skills".to_string(),
        "remove".to_string(),
        skill_name.clone(),
    ];

    // Add --yes for non-interactive mode (CLI has its own confirmation prompt)
    args.push("--yes".to_string());

    if global {
        args.push("--global".to_string());
    }

    // Log the command for debugging
    eprintln!("[remove_skill] Running: npx {}", args.join(" "));

    let output = Command::new("npx")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to execute npx skills: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    eprintln!("[remove_skill] Exit code: {:?}", output.status.code());
    eprintln!("[remove_skill] stdout: {}", stdout);
    eprintln!("[remove_skill] stderr: {}", stderr);

    if output.status.success() {
        if let Err(e) = skill_refresh::rebuild_snapshot_now(&app, &refresh_state) {
            eprintln!("[remove_skill] snapshot rebuild failed: {e}");
        }
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

/// Maximum number of bytes read from an installed skill's SKILL.md, to keep
/// a runaway file from blocking the UI thread on a slow disk.
const MAX_SKILL_MD_BYTES: usize = 2 * 1024 * 1024;

/// Require that `path` belongs to an installed skill in the current
/// snapshot, so `read_installed_skill_md` / `open_skill_path` can't be used
/// to read or open an arbitrary path on disk.
fn require_snapshot_owns_path(
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
fn canonicalize_skill_md(
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
/// the detail panel's SKILL.md viewer. Unlike `github-skill-source.ts`'s
/// fetch-from-GitHub path, this works for manual/plugin skills that have no
/// remote source. Restricted to `SKILL.md` files belonging to a deployment in
/// the current snapshot, to keep this from becoming an arbitrary-file read.
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
fn check_skill_md_write_allowed(
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
fn atomic_write_skill_md(canonical: &std::path::Path, content: &str) -> Result<(), String> {
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
/// plus a refusal when the owning deployment is plugin-managed. Rebuilds the
/// snapshot afterward so the new content and token/byte counts are reflected
/// immediately.
#[tauri::command]
pub fn write_installed_skill_md(
    path: String,
    content: String,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
) -> Result<(), String> {
    let canonical = validate_skill_md_write(&path, &content, &refresh_state)?;
    atomic_write_skill_md(&canonical, &content)?;
    if let Err(e) = skill_refresh::rebuild_snapshot_now(&app, &refresh_state) {
        eprintln!("[write_installed_skill_md] snapshot rebuild failed: {e}");
    }
    Ok(())
}

/// Refuses when `canonical`'s current content differs from `expected_content`
/// (the file drifted on disk since the caller loaded it), otherwise writes
/// atomically. Pulled out of the command so it's testable without a snapshot
/// or `tauri::AppHandle`.
fn write_skill_md_compare_and_swap(
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
    if let Err(e) = skill_refresh::rebuild_snapshot_now(&app, &refresh_state) {
        eprintln!("[write_installed_skill_md_if_unchanged] snapshot rebuild failed: {e}");
    }
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

    let flag = match mode.as_str() {
        "reveal" => "-R",
        "editor" => "-t",
        other => return Err(format!("Unknown open mode: {other}")),
    };

    Command::new("open")
        .args([flag, &path])
        .output()
        .map_err(|e| format!("Failed to open {}: {}", path, e))?;
    Ok(())
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

/// Update a skill, using whichever CLI owns it: `dotagents` for a
/// dotagents-managed skill (`add` re-pins it to the latest commit; `install`
/// re-runs the wildcard sync for a skill with no `[[skills]]` row), `npx
/// skills update` for a skills.sh skill. Manual/plugin skills have no owning
/// CLI to update through, so they're rejected up front.
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

    let (tool, mut args): (&str, Vec<String>) = match source_kind {
        Some(SourceKind::Manual) | Some(SourceKind::Plugin) => {
            return Err("Update is not available for manually installed skills".to_string());
        }
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
        if let Err(e) = skill_refresh::rebuild_snapshot_now(&app, &refresh_state) {
            eprintln!("[update_skill] snapshot rebuild failed: {e}");
        }
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
