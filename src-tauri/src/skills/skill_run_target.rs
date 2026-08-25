// ============================================================================
// Skills Module - skill_run_target
// Prepares the working directory a "Test" run executes in - a scratch
// folder, a detached git worktree, or the project's own tree in place - and
// the diff/apply/discard operations each kind supports afterward.
// ============================================================================

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::skill_agent_runner::copy_skill_dir_for_run_target;

/// Which kind of working directory a "Test" run executes in.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillRunTargetKind {
    Scratch,
    Worktree,
    InPlace,
}

/// Request to prepare one run target.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillRunTargetRequest {
    pub kind: SkillRunTargetKind,
    pub skill_name: String,
    pub skill_folder: String,
    /// Other own skills to also install (Scratch only): (name, folder path).
    #[serde(default)]
    pub extra_skills: Vec<(String, String)>,
    /// `=== path` fixture text (Scratch only) - see `write_fixture_files`.
    pub fixture: Option<String>,
    /// Required for Worktree and InPlace.
    pub project_path: Option<String>,
}

/// A prepared working directory a run can execute in, and what it takes to
/// clean it up or fold its changes back afterward.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRunTarget {
    pub kind: SkillRunTargetKind,
    pub cwd: String,
    /// Set for Scratch (the scratch dir) and Worktree (the worktree path).
    pub cleanup_path: Option<String>,
    /// The project HEAD sha the worktree was branched from (Worktree only).
    pub git_head: Option<String>,
}

/// Prepares the working directory for a "Test" run, per `request.kind`.
#[tauri::command]
pub fn prepare_skill_run_target(
    app: AppHandle,
    request: SkillRunTargetRequest,
) -> Result<SkillRunTarget, String> {
    match request.kind {
        SkillRunTargetKind::Scratch => prepare_scratch(&app, &request),
        SkillRunTargetKind::Worktree => prepare_worktree(&app, &request),
        SkillRunTargetKind::InPlace => prepare_in_place(&request),
    }
}

// ============================================================================
// Scratch
// ============================================================================

/// One file parsed out of a `=== path` fixture block.
struct FixtureFile {
    relative_path: String,
    body: String,
}

/// Parses the `=== path/relative.ext` fixture format: each line starting with
/// `=== ` opens a file whose body is every following line up to the next
/// `===` marker (or the end of the text). Rejects any path containing `..`
/// or starting with `/`, so a fixture can't write outside the run target.
fn parse_fixture(fixture: &str) -> Result<Vec<FixtureFile>, String> {
    let mut files = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;

    for line in fixture.lines() {
        if let Some(path) = line.strip_prefix("=== ") {
            if let Some((relative_path, body_lines)) = current.take() {
                files.push(FixtureFile {
                    relative_path,
                    body: body_lines.join("\n"),
                });
            }
            let path = path.trim().to_string();
            if path.contains("..") || path.starts_with('/') {
                return Err(format!("Invalid fixture path: {path:?}"));
            }
            if path.is_empty() {
                return Err("Fixture path cannot be empty".to_string());
            }
            current = Some((path, Vec::new()));
        } else if let Some((_, body_lines)) = current.as_mut() {
            body_lines.push(line);
        }
    }
    if let Some((relative_path, body_lines)) = current {
        files.push(FixtureFile {
            relative_path,
            body: body_lines.join("\n"),
        });
    }
    Ok(files)
}

/// Writes every file `parse_fixture` extracts from `fixture` under `root`,
/// creating parent directories as needed.
fn write_fixture_files(root: &Path, fixture: &str) -> Result<(), String> {
    for file in parse_fixture(fixture)? {
        let dest = root.join(&file.relative_path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
        }
        fs::write(&dest, file.body)
            .map_err(|e| format!("Could not write {}: {e}", dest.display()))?;
    }
    Ok(())
}

fn prepare_scratch(
    app: &AppHandle,
    request: &SkillRunTargetRequest,
) -> Result<SkillRunTarget, String> {
    let mut skills = vec![(request.skill_name.clone(), request.skill_folder.clone())];
    skills.extend(request.extra_skills.iter().cloned());
    let scratch_dir = super::skill_agent_runner::create_skill_scratch_dir(app.clone(), skills)?;

    if let Some(fixture) = &request.fixture {
        write_fixture_files(Path::new(&scratch_dir), fixture)?;
    }

    Ok(SkillRunTarget {
        kind: SkillRunTargetKind::Scratch,
        cwd: scratch_dir.clone(),
        cleanup_path: Some(scratch_dir),
        git_head: None,
    })
}

// ============================================================================
// Worktree
// ============================================================================

fn run_git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    run_git_raw(cwd, args).map(|out| out.trim().to_string())
}

/// Like `run_git`, but returns stdout untouched - needed for `git diff`,
/// where trimming would drop the trailing newline `git apply` requires on
/// the last line of a patch.
fn run_git_raw(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("Could not run git {}: {e}", args.join(" ")))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn worktree_root(app: &AppHandle) -> Result<PathBuf, String> {
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("Could not resolve app cache dir: {e}"))?;
    Ok(cache_dir.join("skill-studio").join("worktrees"))
}

fn prepare_worktree(
    app: &AppHandle,
    request: &SkillRunTargetRequest,
) -> Result<SkillRunTarget, String> {
    let project_path = request
        .project_path
        .as_deref()
        .ok_or("Worktree requires a project")?;
    let project = Path::new(project_path);

    let toplevel = run_git(project, &["rev-parse", "--show-toplevel"])
        .map_err(|e| format!("Not a git repository: {e}"))?;
    let git_head = run_git(project, &["rev-parse", "HEAD"])?;

    let stamp = format!(
        "{}-{}",
        Utc::now().format("%Y%m%dT%H%M%S%.f"),
        std::process::id()
    );
    let worktree_path = worktree_root(app)?.join(stamp);
    fs::create_dir_all(worktree_path.parent().unwrap())
        .map_err(|e| format!("Could not create worktree root: {e}"))?;

    run_git(
        Path::new(&toplevel),
        &[
            "worktree",
            "add",
            "--detach",
            &worktree_path.to_string_lossy(),
            "HEAD",
        ],
    )?;

    let skill_dest = worktree_path
        .join(".agents")
        .join("skills")
        .join(&request.skill_name);
    if !skill_dest.exists() {
        copy_skill_dir_for_run_target(Path::new(&request.skill_folder), &skill_dest)
            .map_err(|e| format!("Could not install skill into worktree: {e}"))?;
        for agent_dir in [".claude/skills", ".pi/skills"] {
            let link_dir = worktree_path.join(agent_dir);
            fs::create_dir_all(&link_dir)
                .map_err(|e| format!("Could not create {agent_dir}: {e}"))?;
            let target = Path::new("../../.agents/skills").join(&request.skill_name);
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, link_dir.join(&request.skill_name))
                .map_err(|e| format!("Could not symlink {agent_dir}: {e}"))?;
        }
    }

    Ok(SkillRunTarget {
        kind: SkillRunTargetKind::Worktree,
        cwd: worktree_path.to_string_lossy().to_string(),
        cleanup_path: Some(worktree_path.to_string_lossy().to_string()),
        git_head: Some(git_head),
    })
}

// ============================================================================
// InPlace
// ============================================================================

fn prepare_in_place(request: &SkillRunTargetRequest) -> Result<SkillRunTarget, String> {
    let project_path = request
        .project_path
        .as_deref()
        .ok_or("In place requires a project")?;
    let project = Path::new(project_path);

    let status = run_git(project, &["status", "--porcelain"])?;
    if !status.is_empty() {
        return Err(
            "Working tree has uncommitted changes. Commit or stash them first.".to_string(),
        );
    }

    Ok(SkillRunTarget {
        kind: SkillRunTargetKind::InPlace,
        cwd: project_path.to_string(),
        cleanup_path: None,
        git_head: None,
    })
}

/// Reveals a Scratch target's folder in Finder. Restricted to `Scratch`
/// targets - Worktree and InPlace cwds live under the project the caller
/// already has `open_skill_path` access to, or under the app cache, neither
/// of which this command needs to widen access to.
#[tauri::command]
pub fn reveal_skill_run_target(target: SkillRunTarget) -> Result<(), String> {
    if target.kind != SkillRunTargetKind::Scratch {
        return Err("Only a scratch target's folder can be revealed this way".to_string());
    }
    Command::new("open")
        .args(["-R", &target.cwd])
        .output()
        .map_err(|e| format!("Failed to reveal {}: {e}", target.cwd))?;
    Ok(())
}

// ============================================================================
// Diff / apply / discard
// ============================================================================

/// The unified diff for everything that changed in `target.cwd` since it was
/// prepared, untracked files included (via a throwaway `git add -N .`, undone
/// right after). Empty string when the tree is clean.
#[tauri::command]
pub fn skill_run_target_diff(target: SkillRunTarget) -> Result<String, String> {
    let cwd = Path::new(&target.cwd);
    run_git(cwd, &["add", "-N", "."])?;
    let diff = run_git_raw(cwd, &["diff"]);
    // Always undo the intent-to-add, even if `git diff` failed.
    let _ = run_git(cwd, &["reset", "-q"]);
    diff
}

/// Applies a Worktree target's diff onto `project_path` with `git apply
/// --3way`, so the worktree's changes fold back into the project. Only
/// meaningful for Worktree targets.
#[tauri::command]
pub fn apply_skill_run_target_diff(
    target: SkillRunTarget,
    project_path: String,
) -> Result<(), String> {
    if target.kind != SkillRunTargetKind::Worktree {
        return Err("Only a worktree target's diff can be applied".to_string());
    }
    let diff = skill_run_target_diff(target)?;
    let tmp = std::env::temp_dir().join(format!("skill-studio-apply-{}.patch", std::process::id()));
    fs::write(&tmp, &diff).map_err(|e| format!("Could not write patch file: {e}"))?;

    let result = run_git(
        Path::new(&project_path),
        &["apply", "--3way", &tmp.to_string_lossy()],
    );
    let _ = fs::remove_file(&tmp);
    result.map(|_| ())
}

/// Reverts (InPlace) or removes (Worktree/Scratch) whatever `prepare_skill_run_target`
/// produced.
#[tauri::command]
pub fn discard_skill_run_target(target: SkillRunTarget) -> Result<(), String> {
    match target.kind {
        SkillRunTargetKind::Worktree => {
            let Some(path) = &target.cleanup_path else {
                return Ok(());
            };
            // The project path is needed as the cwd `git worktree remove` runs
            // in; `target.cwd` *is* the worktree, so removing it from within
            // itself would fail once the directory is gone. Run from the
            // worktree's parent-of-parent instead: the worktree metadata lives
            // under the main repo's `.git/worktrees`, and `git worktree
            // remove` accepts an absolute path regardless of cwd as long as
            // it's run inside *some* checkout of the same repo. We use the
            // worktree itself for `list`/fallback `prune`, which don't need
            // the directory to still exist.
            if run_git(Path::new(path), &["worktree", "remove", "--force", path]).is_err() {
                let _ = run_git(Path::new(path), &["worktree", "prune"]);
            }
            let _ = fs::remove_dir_all(path);
            Ok(())
        }
        SkillRunTargetKind::InPlace => {
            let cwd = Path::new(&target.cwd);
            let status = run_git(cwd, &["status", "--porcelain"])?;
            let paths: Vec<&str> = status
                .lines()
                .filter_map(|line| line.get(3..))
                .filter(|p| !p.starts_with(".git"))
                .collect();
            if paths.is_empty() {
                return Ok(());
            }
            let mut checkout_args = vec!["checkout", "--"];
            checkout_args.extend(paths.iter().copied());
            // `checkout --` only covers tracked-file modifications; untracked
            // additions need `clean -fd` scoped to the same paths.
            let _ = run_git(cwd, &checkout_args);
            let mut clean_args = vec!["clean", "-fd", "--"];
            clean_args.extend(paths.iter().copied());
            let _ = run_git(cwd, &clean_args);
            Ok(())
        }
        SkillRunTargetKind::Scratch => {
            let Some(path) = &target.cleanup_path else {
                return Ok(());
            };
            fs::remove_dir_all(path).map_err(|e| format!("Could not remove scratch dir: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn init_repo(dir: &Path) {
        run_git(dir, &["init", "-q"]).unwrap();
        run_git(dir, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(dir, &["config", "user.name", "Test"]).unwrap();
        fs::write(dir.join("README.md"), "hello\n").unwrap();
        run_git(dir, &["add", "."]).unwrap();
        run_git(dir, &["commit", "-q", "-m", "initial"]).unwrap();
    }

    #[test]
    fn fixture_parser_rejects_traversal() {
        assert!(parse_fixture("=== ../escape.txt\nbody").is_err());
        assert!(parse_fixture("=== /abs.txt\nbody").is_err());
    }

    #[test]
    fn fixture_parser_splits_multiple_files() {
        let files = parse_fixture("=== a.txt\nfoo\nbar\n=== b/c.txt\nbaz").unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].relative_path, "a.txt");
        assert_eq!(files[0].body, "foo\nbar");
        assert_eq!(files[1].relative_path, "b/c.txt");
        assert_eq!(files[1].body, "baz");
    }

    #[test]
    fn in_place_refuses_a_dirty_tree() {
        if !git_available() {
            return;
        }
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        fs::write(dir.path().join("README.md"), "dirty\n").unwrap();

        let request = SkillRunTargetRequest {
            kind: SkillRunTargetKind::InPlace,
            skill_name: "demo".to_string(),
            skill_folder: String::new(),
            extra_skills: vec![],
            fixture: None,
            project_path: Some(dir.path().to_string_lossy().to_string()),
        };
        let err = prepare_in_place(&request).unwrap_err();
        assert!(err.contains("uncommitted changes"));
    }

    #[test]
    fn worktree_add_diff_apply_discard_round_trip() {
        if !git_available() {
            return;
        }
        let project = tempdir().unwrap();
        init_repo(project.path());
        let cache = tempdir().unwrap();

        let toplevel = run_git(project.path(), &["rev-parse", "--show-toplevel"]).unwrap();
        let worktree_path = cache.path().join("wt");
        run_git(
            Path::new(&toplevel),
            &[
                "worktree",
                "add",
                "--detach",
                &worktree_path.to_string_lossy(),
                "HEAD",
            ],
        )
        .unwrap();

        fs::write(worktree_path.join("new-file.txt"), "content\n").unwrap();
        let target = SkillRunTarget {
            kind: SkillRunTargetKind::Worktree,
            cwd: worktree_path.to_string_lossy().to_string(),
            cleanup_path: Some(worktree_path.to_string_lossy().to_string()),
            git_head: None,
        };

        let diff = skill_run_target_diff(target.clone()).unwrap();
        assert!(diff.contains("new-file.txt"));

        apply_skill_run_target_diff(target.clone(), project.path().to_string_lossy().to_string())
            .unwrap();
        assert!(project.path().join("new-file.txt").exists());

        discard_skill_run_target(target).unwrap();
        assert!(!worktree_path.exists());
    }
}
