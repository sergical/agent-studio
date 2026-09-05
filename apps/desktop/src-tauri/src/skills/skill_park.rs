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
// dropping the parked copy if it's byte-identical to the reinstalled one, or
// trashing it (under `~/.agents/skills-trash`) if it differs.
// ============================================================================

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::provenance::SourceKind;
use super::skill_agent_runner::validate_skill_dir_name;
use super::skill_dto::LifecycleTarget;
use super::skill_fork::ForkMutationLock;
use super::skill_fork_registry::{
    read_fork_registry, trial_key, write_fork_registry, ParkedRecord, TrialScope,
};
use super::skill_lifecycle::{
    find_deployment, require_global_universal_park_target, revalidate_deployment,
};
use super::skill_refresh::{self, SkillRefreshState};

/// `~/.agents/skills-parked`.
fn skills_parked_root(home: &Path) -> PathBuf {
    home.join(".agents").join("skills-parked")
}

fn shared_skill_dir(home: &Path, name: &str) -> PathBuf {
    home.join(".agents").join("skills").join(name)
}

pub(crate) fn claude_link_path(home: &Path, name: &str) -> PathBuf {
    home.join(".claude").join("skills").join(name)
}

/// Removes `~/.claude/skills/<name>` and returns its raw `read_link` target
/// when it's a *per-skill* symlink - `None` when there's nothing there, or
/// when `~/.claude/skills` itself is the whole-dir symlink (in which case
/// the child path resolves straight through to the shared folder and is
/// never itself a symlink, so this can't accidentally remove it). Shared
/// with `skill_harness_disable`'s Claude Code per-skill toggle.
pub(crate) fn take_claude_link(home: &Path, name: &str) -> Result<Option<PathBuf>, String> {
    let link_path = claude_link_path(home, name);
    let Ok(meta) = fs::symlink_metadata(&link_path) else {
        return Ok(None);
    };
    if !meta.file_type().is_symlink() {
        return Ok(None);
    }
    let target = fs::read_link(&link_path)
        .map_err(|e| format!("Failed to read {}: {e}", link_path.display()))?;
    fs::remove_file(&link_path)
        .map_err(|e| format!("Failed to remove {}: {e}", link_path.display()))?;
    Ok(Some(target))
}

/// Recreates `~/.claude/skills/<name>` -> `target` (as recorded when it was
/// removed), unless something is already there or `~/.claude/skills` is now
/// the whole-dir symlink (which already covers it).
pub(crate) fn restore_claude_link(home: &Path, name: &str, target: &Path) -> Result<(), String> {
    let claude_skills_dir = home.join(".claude").join("skills");
    if let Ok(meta) = fs::symlink_metadata(&claude_skills_dir) {
        if meta.file_type().is_symlink() {
            return Ok(());
        }
    } else {
        fs::create_dir_all(&claude_skills_dir)
            .map_err(|e| format!("Failed to create {}: {e}", claude_skills_dir.display()))?;
    }
    let link_path = claude_link_path(home, name);
    if fs::symlink_metadata(&link_path).is_ok() {
        return Ok(());
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, &link_path)
        .map_err(|e| format!("Failed to symlink {}: {e}", link_path.display()))?;
    #[cfg(not(unix))]
    return Err("Symlinking is only supported on Unix".to_string());
    Ok(())
}

/// One filesystem entry as seen by `dir_trees_identical` - keeping symlinks
/// and directories as their own variants (rather than skipping or flattening
/// them) means a symlink-vs-file swap, or a divergent symlink target, shows
/// up as a genuine inequality instead of silently comparing equal.
#[derive(Debug, PartialEq, Eq)]
enum TreeEntry {
    File(Vec<u8>),
    Symlink(PathBuf),
    Dir,
}

/// Walks `dir` (rooted at `root`, for relative-path keys), recording every
/// entry into `out`. Any unreadable entry - a directory that can't be listed,
/// a file that can't be read, a symlink whose target can't be resolved - is
/// an `Err`, not a silently skipped entry: `dir_trees_identical`'s caller
/// must never conclude "identical" from a walk that couldn't actually see
/// everything.
fn collect_tree(
    root: &Path,
    dir: &Path,
    out: &mut BTreeMap<PathBuf, TreeEntry>,
) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|e| format!("Failed to read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|e| format!("Failed to read an entry of {}: {e}", dir.display()))?;
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .map_err(|e| format!("{} is not inside {}: {e}", path.display(), root.display()))?
            .to_path_buf();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("Failed to read the type of {}: {e}", path.display()))?;
        if file_type.is_symlink() {
            let target = fs::read_link(&path)
                .map_err(|e| format!("Failed to read the symlink {}: {e}", path.display()))?;
            out.insert(rel, TreeEntry::Symlink(target));
        } else if file_type.is_dir() {
            out.insert(rel.clone(), TreeEntry::Dir);
            collect_tree(root, &path, out)?;
        } else {
            let bytes =
                fs::read(&path).map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
            out.insert(rel, TreeEntry::File(bytes));
        }
    }
    Ok(())
}

/// Whether `a` and `b` hold exactly the same entries: same relative paths,
/// same entry types (file/symlink/dir), same file bytes, same symlink
/// targets. `Err` when either tree couldn't be fully read (permission
/// denied, a broken symlink, etc.) - the caller must treat that the same as
/// `Ok(false)` (never delete on an inconclusive comparison).
fn dir_trees_identical(a: &Path, b: &Path) -> Result<bool, String> {
    let mut fa = BTreeMap::new();
    let mut fb = BTreeMap::new();
    collect_tree(a, a, &mut fa)?;
    collect_tree(b, b, &mut fb)?;
    Ok(fa == fb)
}

/// If `name` has a running global trial, point its active paths at the new
/// location. A parked trial has no active Claude link. The parked record keeps
/// the removed link's raw target for restoration.
fn retarget_trial(
    registry: &mut super::skill_fork_registry::ForkRegistry,
    name: &str,
    old_skill_dir: &Path,
    skill_dir: PathBuf,
    deployment_id: String,
    claude_link: Option<PathBuf>,
) {
    let legacy_key = trial_key(TrialScope::Global, name);
    let key = registry
        .trials
        .iter()
        .find(|(key, trial)| {
            trial.scope == TrialScope::Global
                && trial.skill_dir == old_skill_dir
                && (trial.deployment_id.is_empty()
                    || super::skill_deployment::parse_deployment_id(&trial.deployment_id)
                        .is_some_and(|parsed| parsed.name == name)
                    || *key == &legacy_key)
        })
        .map(|(key, _)| key.clone());
    if let Some(key) = key {
        let mut trial = registry.trials.remove(&key).expect("key came from map");
        trial.skill_dir = skill_dir;
        trial.deployment_id = deployment_id.clone();
        trial.claude_link = claude_link;
        registry.trials.insert(
            super::skill_fork_registry::deployment_trial_key(&deployment_id),
            trial,
        );
    }
}

/// `park_skill`'s logic, taking `home` directly so it's testable without a
/// Tauri `AppHandle`. `source_kind` is the skill's current `SourceKind`
/// (from the snapshot), recorded so the parked skill still shows an accurate
/// badge even though it has no deployment left for `classify_source_kind` to
/// look at.
pub fn park_skill_with(
    home: &Path,
    name: &str,
    source_kind: SourceKind,
    now: DateTime<Utc>,
) -> Result<ParkedRecord, String> {
    park_skill_impl(home, name, source_kind, now, write_fork_registry)
}

/// `park_skill_with`'s logic, taking the registry-write step as a closure so
/// tests can inject a failing writer to exercise the rollback path without
/// needing a real filesystem failure.
fn park_skill_impl(
    home: &Path,
    name: &str,
    source_kind: SourceKind,
    now: DateTime<Utc>,
    write_registry: impl FnOnce(&Path, &super::skill_fork_registry::ForkRegistry) -> Result<(), String>,
) -> Result<ParkedRecord, String> {
    validate_skill_dir_name(name)?;

    let shared_dir = shared_skill_dir(home, name);
    if !shared_dir.is_dir() {
        return Err(format!(
            "\"{name}\" is not deployed in the shared folder (~/.agents/skills/{name})"
        ));
    }

    let parked_root = skills_parked_root(home);
    let parked_dir = parked_root.join(name);
    if parked_dir.exists() {
        return Err(format!("\"{name}\" is already parked"));
    }

    // Read and validate the registry before touching the filesystem: a
    // malformed registry must fail here, before the folder has moved or the
    // Claude Code link has been removed.
    let mut registry = read_fork_registry(home)?;

    fs::create_dir_all(&parked_root)
        .map_err(|e| format!("Failed to create {}: {e}", parked_root.display()))?;

    let claude_link = take_claude_link(home, name)?;

    if let Err(e) = fs::rename(&shared_dir, &parked_dir) {
        // Roll back the removed link before surfacing the failure.
        if let Some(target) = &claude_link {
            let _ = restore_claude_link(home, name, target);
        }
        return Err(format!(
            "Failed to move {} to {}: {e}",
            shared_dir.display(),
            parked_dir.display()
        ));
    }

    let record = ParkedRecord {
        deployment_id: super::skill_deployment::deployment_id(
            name,
            "parked",
            super::skill_deployment::SkillDestination::Universal,
            "universal",
            None,
            &parked_dir,
        ),
        skill_dir: parked_dir.clone(),
        parked_at: now.to_rfc3339(),
        source_kind,
        claude_link: claude_link.clone(),
    };
    registry.parked.insert(name.to_string(), record.clone());
    retarget_trial(
        &mut registry,
        name,
        &shared_dir,
        parked_dir.clone(),
        record.deployment_id.clone(),
        None,
    );

    if let Err(e) = write_registry(home, &registry) {
        // Roll back: move the folder back and recreate the removed link.
        let _ = fs::rename(&parked_dir, &shared_dir);
        if let Some(target) = &claude_link {
            let _ = restore_claude_link(home, name, target);
        }
        return Err(format!("Failed to record \"{name}\" as parked: {e}"));
    }

    Ok(record)
}

/// What `unpark_skill_with` did, for the toast the command surfaces.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum UnparkOutcome {
    /// The folder moved back to the shared root as normal.
    Restored,
    /// The shared folder already existed (reinstalled while parked) and was
    /// byte-identical to the parked copy - the parked copy was dropped.
    Reconciled,
    /// The shared folder already existed and differed from the parked copy -
    /// the parked copy was moved to `~/.agents/skills-trash` instead of being
    /// silently discarded or overwriting the reinstalled one.
    ConflictTrashed { trash_path: String },
}

/// `unpark_skill`'s logic. See `UnparkOutcome` for the three ways this can
/// resolve; the "parked-but-reinstalled" case (the shared folder was
/// recreated while parked) is detected here from `shared_dir.exists()`.
pub fn unpark_skill_with(
    home: &Path,
    name: &str,
    now: DateTime<Utc>,
) -> Result<UnparkOutcome, String> {
    unpark_skill_impl(home, name, now, write_fork_registry)
}

/// `unpark_skill_with`'s logic, taking the registry-write step as a closure -
/// see `park_skill_impl`.
fn unpark_skill_impl(
    home: &Path,
    name: &str,
    now: DateTime<Utc>,
    write_registry: impl FnOnce(&Path, &super::skill_fork_registry::ForkRegistry) -> Result<(), String>,
) -> Result<UnparkOutcome, String> {
    validate_skill_dir_name(name)?;

    // Read and validate the registry before touching the filesystem: a
    // malformed registry must fail here, before the folder has moved.
    let mut registry = read_fork_registry(home)?;
    let record = registry
        .parked
        .get(name)
        .cloned()
        .ok_or_else(|| format!("\"{name}\" is not parked"))?;

    let parked_dir = skills_parked_root(home).join(name);
    if !parked_dir.is_dir() {
        return Err(format!(
            "Parked copy of \"{name}\" is missing at {}",
            parked_dir.display()
        ));
    }
    let shared_dir = shared_skill_dir(home, name);

    let outcome = if shared_dir.exists() {
        // `Err` (an entry couldn't be read at all) is treated the same as a
        // confirmed difference: an inconclusive comparison must never lead
        // to deleting the parked copy.
        if dir_trees_identical(&parked_dir, &shared_dir).unwrap_or(false) {
            fs::remove_dir_all(&parked_dir)
                .map_err(|e| format!("Failed to remove {}: {e}", parked_dir.display()))?;
            UnparkOutcome::Reconciled
        } else {
            let trash_root = home.join(".agents").join("skills-trash");
            fs::create_dir_all(&trash_root)
                .map_err(|e| format!("Failed to create {}: {e}", trash_root.display()))?;
            let trash_dir = trash_root.join(format!("{name}-{}", now.format("%Y%m%d-%H%M%S")));
            fs::rename(&parked_dir, &trash_dir).map_err(|e| {
                format!(
                    "Failed to move {} to {}: {e}",
                    parked_dir.display(),
                    trash_dir.display()
                )
            })?;
            UnparkOutcome::ConflictTrashed {
                trash_path: trash_dir.to_string_lossy().to_string(),
            }
        }
    } else {
        fs::rename(&parked_dir, &shared_dir).map_err(|e| {
            format!(
                "Failed to move {} to {}: {e}",
                parked_dir.display(),
                shared_dir.display()
            )
        })?;
        UnparkOutcome::Restored
    };

    // The registry is only written once the move has succeeded - see above.
    // The Claude Code link, though, is best-effort: if it fails to restore,
    // the record must still be dropped and the registry still written (the
    // skill genuinely is unparked), but the command reports the failure so
    // the user can fix the link by hand.
    let mut restored_link = None;
    let link_restore_err = if let Some(target) = &record.claude_link {
        match restore_claude_link(home, name, target) {
            Ok(()) => {
                restored_link = Some(claude_link_path(home, name));
                None
            }
            Err(e) => Some(e),
        }
    } else {
        None
    };

    registry.parked.remove(name);
    let global_id = super::skill_deployment::deployment_id(
        name,
        "global",
        super::skill_deployment::SkillDestination::Universal,
        "universal",
        None,
        &shared_dir,
    );
    retarget_trial(
        &mut registry,
        name,
        &parked_dir,
        shared_dir,
        global_id,
        restored_link,
    );
    write_registry(home, &registry)?;

    if let Some(e) = link_restore_err {
        let link = record
            .claude_link
            .expect("link_restore_err implies claude_link");
        return Err(format!(
            "\"{name}\" was unparked, but its Claude Code link could not be restored: {e}. Recreate {} -> {} by hand.",
            claude_link_path(home, name).display(),
            link.display()
        ));
    }

    Ok(outcome)
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
        "Park" => {
            super::skill_lifecycle::require_direct_deployment_mutable(deployment, action)?;
            if deployment.scope != "global" {
                return Err("Park is only available for the Global Universal folder.".to_string());
            }
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
pub fn park_skill(
    target: LifecycleTarget,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<ParkedRecord, String> {
    let _guard = fork_lock.try_acquire()?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;

    let snapshot = super::skill_lifecycle::resolve_fresh_lifecycle_target(
        &app,
        &refresh_state,
        &target,
        "Park",
    )?
    .snapshot;
    let (name, source_kind) = park_target_skill(&snapshot, &target, "Park")?;

    let result = park_skill_with(&home, &name, source_kind, Utc::now());
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
pub fn unpark_skill(
    target: LifecycleTarget,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<UnparkOutcome, String> {
    let _guard = fork_lock.try_acquire()?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    let (name, _) = park_target_skill(&snapshot, &target, "Unpark")?;
    let deployment_id = target.deployment_id.as_deref().expect("validated above");
    let deployment = find_deployment(&snapshot, deployment_id)?.1;
    let registry = read_fork_registry(&home)?;
    let record = registry
        .parked
        .get(&name)
        .ok_or_else(|| format!("\"{name}\" is not parked"))?;
    let expected = if record.skill_dir.as_os_str().is_empty() {
        skills_parked_root(&home).join(&name)
    } else {
        record.skill_dir.clone()
    };
    if (!record.deployment_id.is_empty() && record.deployment_id != deployment_id)
        || Path::new(&deployment.path) != expected
    {
        return Err(
            "The parked record does not belong to the selected Global Universal deployment"
                .to_string(),
        );
    }

    let result = unpark_skill_with(&home, &name, Utc::now());
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

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn park_moves_folder_and_writes_record() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");

        let record = park_skill_with(home, "find-bugs", SourceKind::Manual, now()).unwrap();
        assert_eq!(record.source_kind, SourceKind::Manual);
        assert!(!home.join(".agents/skills/find-bugs").exists());
        assert!(home
            .join(".agents/skills-parked/find-bugs/SKILL.md")
            .exists());

        let registry = read_fork_registry(home).unwrap();
        assert!(registry.parked.contains_key("find-bugs"));
    }

    #[test]
    fn park_leaves_same_name_project_universal_deployment_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        write_skill(&project.join(".agents/skills/find-bugs"), "find-bugs");

        park_skill_with(&home, "find-bugs", SourceKind::SkillsSh, now()).unwrap();

        assert!(project.join(".agents/skills/find-bugs/SKILL.md").is_file());
        assert!(home
            .join(".agents/skills-parked/find-bugs/SKILL.md")
            .is_file());
    }

    #[test]
    fn park_refuses_when_not_deployed_in_shared_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let err = park_skill_with(tmp.path(), "find-bugs", SourceKind::Manual, now()).unwrap_err();
        assert!(err.contains("not deployed in the shared folder"));
    }

    #[test]
    fn park_refuses_stale_parked_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        write_skill(&home.join(".agents/skills-parked/find-bugs"), "find-bugs");

        let err = park_skill_with(home, "find-bugs", SourceKind::Manual, now()).unwrap_err();
        assert!(err.contains("already parked"));
    }

    #[test]
    fn park_removes_per_skill_claude_link_and_records_it() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        std::os::unix::fs::symlink(
            "../../.agents/skills/find-bugs",
            home.join(".claude/skills/find-bugs"),
        )
        .unwrap();

        let record = park_skill_with(home, "find-bugs", SourceKind::Manual, now()).unwrap();
        assert!(!home.join(".claude/skills/find-bugs").exists());
        assert_eq!(
            record.claude_link,
            Some(PathBuf::from("../../.agents/skills/find-bugs"))
        );
    }

    #[test]
    fn park_leaves_whole_dir_claude_symlink_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude")).unwrap();
        std::os::unix::fs::symlink(home.join(".agents/skills"), home.join(".claude/skills"))
            .unwrap();

        let record = park_skill_with(home, "find-bugs", SourceKind::Manual, now()).unwrap();
        assert_eq!(record.claude_link, None);
        // The whole-dir symlink itself must survive untouched.
        assert!(fs::symlink_metadata(home.join(".claude/skills"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn unpark_round_trips_folder_and_claude_link() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        std::os::unix::fs::symlink(
            "../../.agents/skills/find-bugs",
            home.join(".claude/skills/find-bugs"),
        )
        .unwrap();

        park_skill_with(home, "find-bugs", SourceKind::Manual, now()).unwrap();
        let outcome = unpark_skill_with(home, "find-bugs", now()).unwrap();
        assert_eq!(outcome, UnparkOutcome::Restored);
        assert!(home.join(".agents/skills/find-bugs/SKILL.md").exists());
        assert!(!home.join(".agents/skills-parked/find-bugs").exists());
        assert!(fs::symlink_metadata(home.join(".claude/skills/find-bugs"))
            .unwrap()
            .file_type()
            .is_symlink());

        let registry = read_fork_registry(home).unwrap();
        assert!(!registry.parked.contains_key("find-bugs"));
    }

    #[test]
    fn unpark_refuses_when_not_parked() {
        let tmp = tempfile::tempdir().unwrap();
        let err = unpark_skill_with(tmp.path(), "find-bugs", now()).unwrap_err();
        assert!(err.contains("is not parked"));
    }

    #[test]
    fn unpark_reconciles_identical_reinstall_by_dropping_parked_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        park_skill_with(home, "find-bugs", SourceKind::Manual, now()).unwrap();
        // Simulate a reinstall that recreated an identical shared folder.
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");

        let outcome = unpark_skill_with(home, "find-bugs", now()).unwrap();
        assert_eq!(outcome, UnparkOutcome::Reconciled);
        assert!(!home.join(".agents/skills-parked/find-bugs").exists());
        assert!(home.join(".agents/skills/find-bugs/SKILL.md").exists());
    }

    #[test]
    fn unpark_trashes_divergent_reinstall_instead_of_overwriting() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        park_skill_with(home, "find-bugs", SourceKind::Manual, now()).unwrap();
        // Reinstall recreated the folder with different content.
        fs::create_dir_all(home.join(".agents/skills/find-bugs")).unwrap();
        fs::write(
            home.join(".agents/skills/find-bugs/SKILL.md"),
            "---\nname: find-bugs\ndescription: newer\n---\nNew body.",
        )
        .unwrap();

        let outcome = unpark_skill_with(home, "find-bugs", now()).unwrap();
        match outcome {
            UnparkOutcome::ConflictTrashed { trash_path } => {
                assert!(trash_path.contains("skills-trash/find-bugs-"));
                assert!(Path::new(&trash_path).join("SKILL.md").exists());
            }
            other => panic!("expected ConflictTrashed, got {other:?}"),
        }
        // The reinstalled copy is left alone.
        assert!(
            fs::read_to_string(home.join(".agents/skills/find-bugs/SKILL.md"))
                .unwrap()
                .contains("newer")
        );
        assert!(!home.join(".agents/skills-parked/find-bugs").exists());
    }

    #[test]
    fn park_and_unpark_retarget_a_running_trial() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        let link_path = home.join(".claude/skills/find-bugs");
        let link_target = PathBuf::from("../../.agents/skills/find-bugs");
        std::os::unix::fs::symlink(&link_target, &link_path).unwrap();

        let mut registry = read_fork_registry(home).unwrap();
        registry.trials.insert(
            trial_key(TrialScope::Global, "find-bugs"),
            super::super::skill_fork_registry::TrialRecord {
                deployment_id: String::new(),
                started_at: now().to_rfc3339(),
                expires_at: (now() + chrono::Duration::hours(24)).to_rfc3339(),
                status: super::super::skill_fork_registry::TrialStatus::Active,
                method: super::super::skill_fork_registry::AddMethod::Copy,
                scope: TrialScope::Global,
                project_path: None,
                skill_dir: home.join(".agents/skills/find-bugs"),
                deployment_fingerprint: String::new(),
                claude_link: Some(link_path.clone()),
                claude_link_target: Some(link_target.clone()),
            },
        );
        write_fork_registry(home, &registry).unwrap();

        park_skill_with(home, "find-bugs", SourceKind::Manual, now()).unwrap();
        let registry = read_fork_registry(home).unwrap();
        let parked_id = super::super::skill_deployment::deployment_id(
            "find-bugs",
            "parked",
            super::super::skill_deployment::SkillDestination::Universal,
            "universal",
            None,
            &home.join(".agents/skills-parked/find-bugs"),
        );
        let trial =
            &registry.trials[&super::super::skill_fork_registry::deployment_trial_key(&parked_id)];
        assert_eq!(
            trial.skill_dir,
            home.join(".agents/skills-parked/find-bugs")
        );
        assert_eq!(trial.claude_link, None);
        assert_eq!(trial.claude_link_target, Some(link_target.clone()));

        unpark_skill_with(home, "find-bugs", now()).unwrap();
        let registry = read_fork_registry(home).unwrap();
        let global_id = super::super::skill_deployment::deployment_id(
            "find-bugs",
            "global",
            super::super::skill_deployment::SkillDestination::Universal,
            "universal",
            None,
            &home.join(".agents/skills/find-bugs"),
        );
        let trial =
            &registry.trials[&super::super::skill_fork_registry::deployment_trial_key(&global_id)];
        assert_eq!(trial.skill_dir, home.join(".agents/skills/find-bugs"));
        assert_eq!(trial.claude_link, Some(link_path.clone()));
        assert_eq!(fs::read_link(&link_path).unwrap(), link_target);
    }

    // -- dir_trees_identical -------------------------------------------------

    #[test]
    fn dir_trees_identical_true_for_matching_trees() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(&tmp.path().join("a"), "find-bugs");
        write_skill(&tmp.path().join("b"), "find-bugs");
        assert_eq!(
            dir_trees_identical(&tmp.path().join("a"), &tmp.path().join("b")),
            Ok(true)
        );
    }

    #[test]
    fn dir_trees_identical_false_for_divergent_symlink_target() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        write_skill(&a, "find-bugs");
        write_skill(&b, "find-bugs");
        std::os::unix::fs::symlink("target-a", a.join("link")).unwrap();
        std::os::unix::fs::symlink("target-b", b.join("link")).unwrap();

        assert_eq!(dir_trees_identical(&a, &b), Ok(false));
    }

    #[test]
    fn dir_trees_identical_false_for_symlink_vs_regular_file() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        write_skill(&a, "find-bugs");
        write_skill(&b, "find-bugs");
        std::os::unix::fs::symlink("somewhere", a.join("extra")).unwrap();
        fs::write(b.join("extra"), "somewhere").unwrap();

        assert_eq!(dir_trees_identical(&a, &b), Ok(false));
    }

    #[test]
    fn dir_trees_identical_errs_on_unreadable_file() {
        // Skip under root, where permission bits don't block reads.
        if unsafe { libc_geteuid() } == 0 {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        write_skill(&a, "find-bugs");
        write_skill(&b, "find-bugs");
        let locked = a.join("SKILL.md");
        let mut perms = fs::metadata(&locked).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o000);
        fs::set_permissions(&locked, perms.clone()).unwrap();

        let result = dir_trees_identical(&a, &b);

        // Restore permissions so the tempdir can be cleaned up.
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o644);
        fs::set_permissions(&locked, perms).unwrap();

        assert!(result.is_err());
    }

    /// Thin wrapper so the unreadable-file test can skip itself when running
    /// as root (where permission bits don't block reads) without pulling in
    /// the `libc` crate as a new dependency.
    unsafe fn libc_geteuid() -> u32 {
        extern "C" {
            fn geteuid() -> u32;
        }
        geteuid()
    }

    #[test]
    fn unpark_never_deletes_on_unreadable_comparison() {
        if unsafe { libc_geteuid() } == 0 {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        park_skill_with(home, "find-bugs", SourceKind::Manual, now()).unwrap();
        // Reinstall recreated the folder, but make one of its files
        // unreadable so the comparison can't tell whether it's identical.
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        let locked = home.join(".agents/skills/find-bugs/SKILL.md");
        let mut perms = fs::metadata(&locked).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o000);
        fs::set_permissions(&locked, perms.clone()).unwrap();

        let outcome = unpark_skill_with(home, "find-bugs", now());

        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o644);
        fs::set_permissions(&locked, perms).unwrap();

        match outcome.unwrap() {
            UnparkOutcome::ConflictTrashed { .. } => {}
            other => {
                panic!("expected ConflictTrashed on an inconclusive comparison, got {other:?}")
            }
        }
    }

    // -- failure-atomicity ----------------------------------------------------

    #[test]
    fn park_malformed_registry_leaves_the_tree_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(home.join(".agents/skill-studio.json"), "not json").unwrap();

        let err = park_skill_with(home, "find-bugs", SourceKind::Manual, now()).unwrap_err();
        assert!(err.contains("malformed"));
        assert!(home.join(".agents/skills/find-bugs/SKILL.md").exists());
        assert!(!home.join(".agents/skills-parked/find-bugs").exists());
    }

    #[test]
    fn unpark_malformed_registry_leaves_the_tree_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        park_skill_with(home, "find-bugs", SourceKind::Manual, now()).unwrap();
        fs::write(home.join(".agents/skill-studio.json"), "not json").unwrap();

        let err = unpark_skill_with(home, "find-bugs", now()).unwrap_err();
        assert!(err.contains("malformed"));
        assert!(home
            .join(".agents/skills-parked/find-bugs/SKILL.md")
            .exists());
        assert!(!home.join(".agents/skills/find-bugs").exists());
    }

    #[test]
    fn park_registry_write_failure_rolls_back_rename_and_link() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        std::os::unix::fs::symlink(
            "../../.agents/skills/find-bugs",
            home.join(".claude/skills/find-bugs"),
        )
        .unwrap();

        let err = park_skill_impl(home, "find-bugs", SourceKind::Manual, now(), |_, _| {
            Err("simulated write failure".to_string())
        })
        .unwrap_err();

        assert!(err.contains("simulated write failure"));
        assert!(home.join(".agents/skills/find-bugs/SKILL.md").exists());
        assert!(!home.join(".agents/skills-parked/find-bugs").exists());
        assert!(fs::symlink_metadata(home.join(".claude/skills/find-bugs"))
            .unwrap()
            .file_type()
            .is_symlink());
        let registry = read_fork_registry(home).unwrap();
        assert!(!registry.parked.contains_key("find-bugs"));
    }

    #[test]
    fn unpark_registry_write_failure_still_leaves_folder_and_link_restored() {
        // Per spec: unlike park, unpark's fs move already succeeded by the
        // time the registry write is attempted, so a write failure can't roll
        // that back - it can only surface as an error while the filesystem
        // state (the whole point of unparking) is left correctly restored.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        std::os::unix::fs::symlink(
            "../../.agents/skills/find-bugs",
            home.join(".claude/skills/find-bugs"),
        )
        .unwrap();
        park_skill_with(home, "find-bugs", SourceKind::Manual, now()).unwrap();

        let err = unpark_skill_impl(home, "find-bugs", now(), |_, _| {
            Err("simulated write failure".to_string())
        })
        .unwrap_err();

        assert!(err.contains("simulated write failure"));
        assert!(home.join(".agents/skills/find-bugs/SKILL.md").exists());
        assert!(fs::symlink_metadata(home.join(".claude/skills/find-bugs"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn park_rename_failure_leaves_registry_and_link_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        std::os::unix::fs::symlink(
            "../../.agents/skills/find-bugs",
            home.join(".claude/skills/find-bugs"),
        )
        .unwrap();
        // Make the parked root exist but unwritable, so the rename into it
        // fails after the Claude Code link has already been removed.
        let parked_root = home.join(".agents/skills-parked");
        fs::create_dir_all(&parked_root).unwrap();
        let mut perms = fs::metadata(&parked_root).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o500);
        fs::set_permissions(&parked_root, perms.clone()).unwrap();

        let result = park_skill_with(home, "find-bugs", SourceKind::Manual, now());

        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        fs::set_permissions(&parked_root, perms).unwrap();

        assert!(result.is_err());
        assert!(home.join(".agents/skills/find-bugs/SKILL.md").exists());
        assert!(fs::symlink_metadata(home.join(".claude/skills/find-bugs"))
            .unwrap()
            .file_type()
            .is_symlink());
        let registry = read_fork_registry(home).unwrap();
        assert!(!registry.parked.contains_key("find-bugs"));
    }
}
