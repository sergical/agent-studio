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
use tauri::{AppHandle, Emitter, Manager};

use super::agents::AgentId;
use super::commands::skills_sh_remove_args;
use super::skill_add::{maybe_claude_code_symlink, CommandRunner, RealCommandRunner};
use super::skill_fork::ForkMutationLock;
use super::skill_fork_registry::{
    name_from_trial_key, read_fork_registry, trial_key, write_fork_registry, AddMethod,
    TrialRecord, TrialScope,
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
    name: &str,
    scope: TrialScope,
    project_path: Option<&str>,
    method: AddMethod,
    skill_dir: PathBuf,
    claude_link: Option<PathBuf>,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let mut registry = read_fork_registry(home)?;
    registry.trials.insert(
        trial_key(scope, name),
        TrialRecord {
            started_at: now.to_rfc3339(),
            expires_at: (now + chrono::Duration::hours(24)).to_rfc3339(),
            method,
            scope,
            project_path: project_path.map(String::from),
            skill_dir,
            claude_link,
        },
    );
    write_fork_registry(home, &registry)
}

/// Drops `name`'s trial record for `scope`, if any - called by
/// `remove_skill`, `unfork_skill`, and `fork_skill` so a removed, un-forked,
/// or forked skill never leaves a stale trial behind it.
pub fn drop_trial_record(home: &Path, name: &str, scope: TrialScope) -> Result<(), String> {
    let mut registry = read_fork_registry(home)?;
    if registry.trials.remove(&trial_key(scope, name)).is_none() {
        return Ok(());
    }
    write_fork_registry(home, &registry)
}

/// The `--project`-aware `npx -y @sentry/dotagents remove <name>` argv - the
/// existing `dotagents_remove_args` in `commands.rs` has no `--project`
/// support, and a project-scoped trial needs it.
fn dotagents_remove_args_scoped(name: &str, is_project: bool) -> Vec<String> {
    let mut args = vec!["-y".to_string(), "@sentry/dotagents".to_string()];
    if is_project {
        args.push("--project".to_string());
    }
    args.push("remove".to_string());
    args.push(name.to_string());
    args
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
    let trash_path = trash_root.join(format!("{name}-{stamp}"));
    // Trash copy first - removal must never run without one already in place.
    // Symlinks are recreated, never followed, so a symlinked file or
    // subdirectory inside the skill isn't silently dropped from the backup.
    copy_dir_preserving_symlinks(skill_dir, &trash_path)?;
    if count_entries(&trash_path) != count_entries(skill_dir) {
        return Err(format!(
            "Trash copy of {} is incomplete; not removing the original",
            skill_dir.display()
        ));
    }

    match trial.method {
        AddMethod::Dotagents => {
            let args = dotagents_remove_args_scoped(name, is_project);
            let cwd = if is_project {
                project_path.map(PathBuf::from)
            } else {
                None
            };
            runner.run_npx(&args, cwd.as_deref())?;
        }
        AddMethod::SkillsSh => {
            let args = skills_sh_remove_args(name, !is_project);
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

    // Only the exact link `add_skill` recorded for this trial, and only
    // while it's still a symlink - never a directory, and never a link that
    // was replaced or repointed since.
    if let Some(link) = &trial.claude_link {
        if fs::symlink_metadata(link)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            let _ = fs::remove_file(link);
        }
    }

    Ok(trash_path.to_string_lossy().to_string())
}

/// Removes every trial past `now`'s `expires_at`, one at a time. A skill
/// whose removal fails keeps its trial record (and whatever trash copy was
/// made) so the next tick retries it; only skills that fully expired are
/// returned and dropped from the registry.
pub fn run_trial_expiry_pass(
    home: &Path,
    now: DateTime<Utc>,
    runner: &dyn CommandRunner,
) -> Vec<ExpiredTrial> {
    let mut registry = match read_fork_registry(home) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skill_trial] {e}");
            return vec![];
        }
    };

    let due: Vec<String> = registry
        .trials
        .iter()
        .filter(|(_, t)| {
            DateTime::parse_from_rfc3339(&t.expires_at)
                .map(|d| d.with_timezone(&Utc) <= now)
                .unwrap_or(false)
        })
        .map(|(key, _)| key.clone())
        .collect();

    let mut expired = Vec::new();
    for key in due {
        let trial = registry.trials[&key].clone();
        let name = name_from_trial_key(&key).to_string();
        match expire_one(home, &name, &trial, now, runner) {
            Ok(trash_path) => {
                registry.trials.remove(&key);
                expired.push(ExpiredTrial { name, trash_path });
            }
            Err(e) => eprintln!("[skill_trial] failed to expire {name}: {e}"),
        }
    }

    if !expired.is_empty() {
        if let Err(e) = write_fork_registry(home, &registry) {
            eprintln!("[skill_trial] failed to write registry after expiry: {e}");
        }
    }
    expired
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
        run_trial_expiry_pass(&home, Utc::now(), &runner)
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
    name: String,
    scope: TrialScope,
    project_path: Option<String>,
    app: AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let _ = &refresh_state;
    // Not part of the `trials` key (see `trial_key`) - accepted so the
    // caller can pass a deployment's scope/project uniformly with
    // `add_skill`'s request shape.
    let _ = &project_path;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let mut registry = read_fork_registry(&home)?;
    registry.trials.remove(&trial_key(scope, &name));
    write_fork_registry(&home, &registry)?;
    skill_refresh::request_snapshot_rebuild(&app);
    Ok(())
}

/// `<name>-YYYYMMDD-HHMMSS` -> `name`. The suffix is always exactly 16 chars
/// (`-` + 8 digits + `-` + 6 digits), so it can be stripped without a regex
/// crate.
fn strip_trash_suffix(dir_name: &str) -> Option<String> {
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

    let dir_name = trash
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
        let mut registry = read_fork_registry(home).unwrap();
        registry.trials.insert(
            trial_key(TrialScope::Global, name),
            TrialRecord {
                started_at: (expires_at - chrono::Duration::hours(24)).to_rfc3339(),
                expires_at: expires_at.to_rfc3339(),
                method,
                scope: TrialScope::Global,
                project_path: None,
                skill_dir,
                claude_link,
            },
        );
        write_fork_registry(home, &registry).unwrap();
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
        let expired = run_trial_expiry_pass(home, now, &runner);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].name, "old-one");
        assert!(!home.join(".agents/skills/old-one").exists());
        assert!(home.join(".agents/skills/fresh-one").exists());
        let registry = read_fork_registry(home).unwrap();
        assert!(!registry
            .trials
            .contains_key(&trial_key(TrialScope::Global, "old-one")));
        assert!(registry
            .trials
            .contains_key(&trial_key(TrialScope::Global, "fresh-one")));
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
        let expired = run_trial_expiry_pass(home, now, &runner);
        assert!(expired.is_empty());
        // The folder is untouched (the CLI would have removed it, but our
        // fake failed before that), and the trial record is kept for retry.
        assert!(home.join(".agents/skills/find-bugs").exists());
        let registry = read_fork_registry(home).unwrap();
        assert!(registry
            .trials
            .contains_key(&trial_key(TrialScope::Global, "find-bugs")));
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

        run_trial_expiry_pass(home, now, &FakeRunner::default());

        let trash_root = home.join(".agents/skills-trash");
        let trashed = fs::read_dir(&trash_root)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
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
        fs::create_dir_all(project.join(".agents/skills/find-bugs")).unwrap();
        let mut registry = read_fork_registry(&home).unwrap();
        let now = Utc::now();
        registry.trials.insert(
            trial_key(TrialScope::Project, "find-bugs"),
            TrialRecord {
                started_at: (now - chrono::Duration::hours(24)).to_rfc3339(),
                expires_at: (now - chrono::Duration::hours(1)).to_rfc3339(),
                method: AddMethod::Dotagents,
                scope: TrialScope::Project,
                project_path: Some(project.to_string_lossy().to_string()),
                skill_dir: project.join(".agents/skills/find-bugs"),
                claude_link: None,
            },
        );
        write_fork_registry(&home, &registry).unwrap();

        let runner = FakeRunner::default();
        let expired = run_trial_expiry_pass(&home, now, &runner);
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
            Some(link.clone()),
        );
        std::os::unix::fs::symlink(home.join(".agents/skills/find-bugs"), &link).unwrap();
        // A plain directory for a different, unrelated skill - must survive.
        fs::create_dir_all(home.join(".claude/skills/unrelated")).unwrap();

        run_trial_expiry_pass(home, now, &FakeRunner::default());
        assert!(fs::symlink_metadata(&link).is_err());
        assert!(home.join(".claude/skills/unrelated").is_dir());
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
        registry
            .trials
            .remove(&trial_key(TrialScope::Global, "find-bugs"));
        write_fork_registry(home, &registry).unwrap();

        let registry = read_fork_registry(home).unwrap();
        assert!(!registry
            .trials
            .contains_key(&trial_key(TrialScope::Global, "find-bugs")));
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
        let project_skill_dir = project.join(".agents/skills/find-bugs");
        fs::create_dir_all(&project_skill_dir).unwrap();
        let mut registry = read_fork_registry(&home).unwrap();
        registry.trials.insert(
            trial_key(TrialScope::Project, "find-bugs"),
            TrialRecord {
                started_at: (now - chrono::Duration::hours(24)).to_rfc3339(),
                expires_at: (now + chrono::Duration::hours(23)).to_rfc3339(),
                method: AddMethod::Copy,
                scope: TrialScope::Project,
                project_path: Some(project.to_string_lossy().to_string()),
                skill_dir: project_skill_dir.clone(),
                claude_link: None,
            },
        );
        write_fork_registry(&home, &registry).unwrap();

        let expired = run_trial_expiry_pass(&home, now, &FakeRunner::default());
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].name, "find-bugs");
        // The global trial expired; the project-scoped one (not due yet) is untouched.
        assert!(!home.join(".agents/skills/find-bugs").exists());
        assert!(project_skill_dir.exists());
        let registry = read_fork_registry(&home).unwrap();
        assert!(!registry
            .trials
            .contains_key(&trial_key(TrialScope::Global, "find-bugs")));
        assert!(registry
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
