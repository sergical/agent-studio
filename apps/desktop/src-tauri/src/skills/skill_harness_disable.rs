// ============================================================================
// Skills Module - skill_harness_disable
// The way back for a legacy row the removed move-aside disable left under
// `.skill-studio-disabled/`. Skill Studio no longer turns a skill off by
// writing an agent's own config (Codex `config.toml`, OpenCode
// `opencode.json`, Claude Code `settings.json`): those settings are only
// read and shown, and `park` is the only off. `restore_moved_deployment`
// remains because `ops::scan` still reports those legacy rows as
// `disabled_by: "studio-moved"`; `issue-4.4-followup-a.md` tracks a one-shot
// migration that retires `.skill-studio-disabled/` entirely, after which
// this module goes too.
// ============================================================================

use std::fs;
use std::path::{Path, PathBuf};

use tauri::Manager;

use super::skill_dto::{Deployment, DisabledBy, LifecycleTarget};
use super::skill_ownership::LifecycleOwnerKind;
use super::skill_refresh::{self, SkillRefreshState};

/// Name of the holding directory the universal move-aside disable renames a
/// deployment into. Core already defines this (`identity::MOVE_ASIDE_DIR_NAME`)
/// for `ops::scan`'s own one-level-reader skip; re-exported under its old
/// desktop name so every existing call site here keeps reading unchanged.
pub(crate) use skill_studio_core::identity::MOVE_ASIDE_DIR_NAME as STUDIO_DISABLED_DIR_NAME;

/// Restore a deployment the removed `disable_deployment_at` moved aside,
/// renaming it back from `<root>/.skill-studio-disabled/<name>` to
/// `<root>/<name>`. `path` must sit directly inside a
/// `.skill-studio-disabled` directory. Refuses if the original position is
/// already occupied. Reverses the relative-symlink adjustment the removed
/// disable made, by stripping one leading `../`. Kept for
/// `restore_moved_deployment` below - the only way back for a legacy row
/// the old move-aside disable left behind; see the module doc.
fn restore_deployment_at(path: &Path) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("\"{}\" has no file name", path.display()))?;
    let holding_dir = path
        .parent()
        .ok_or_else(|| format!("\"{}\" has no parent directory", path.display()))?;
    if holding_dir.file_name().and_then(|n| n.to_str()) != Some(STUDIO_DISABLED_DIR_NAME) {
        return Err(format!(
            "\"{}\" is not inside a {STUDIO_DISABLED_DIR_NAME} holding directory",
            path.display()
        ));
    }
    let root = holding_dir
        .parent()
        .ok_or_else(|| format!("\"{}\" has no parent directory", holding_dir.display()))?;
    let dest = root.join(name);
    move_deployment(path, &dest, |target| {
        target.strip_prefix("..").unwrap_or(&target).to_path_buf()
    })
}

/// Shared move for `restore_deployment_at`: refuses if `dest` already
/// exists, relinks a relative symlink one level shallower via
/// `adjust_relative_target` so it keeps resolving to the same canonical
/// target, and otherwise renames `path` to `dest` as-is.
fn move_deployment(
    path: &Path,
    dest: &Path,
    adjust_relative_target: impl FnOnce(PathBuf) -> PathBuf,
) -> Result<PathBuf, String> {
    if fs::symlink_metadata(dest).is_ok() {
        return Err(format!("\"{}\" already exists", dest.display()));
    }

    let meta = fs::symlink_metadata(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(path)
            .map_err(|e| format!("Failed to read symlink {}: {e}", path.display()))?;
        if target.is_relative() {
            // Create the adjusted destination first so a failure at any point
            // leaves the original link in place; only then remove the source.
            let adjusted = adjust_relative_target(target);
            create_symlink(&adjusted, dest)?;
            if let Err(e) = fs::remove_file(path) {
                let _ = fs::remove_file(dest);
                return Err(format!("Failed to remove {}: {e}", path.display()));
            }
            return Ok(dest.to_path_buf());
        }
    }

    fs::rename(path, dest).map_err(|e| {
        format!(
            "Failed to move {} to {}: {e}",
            path.display(),
            dest.display()
        )
    })?;
    Ok(dest.to_path_buf())
}

fn create_symlink(target: &Path, link: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
            .map_err(|e| format!("Failed to symlink {}: {e}", link.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        Err("Symlinking is only supported on Unix".to_string())
    }
}

/// Refuses a Copy-owned `studio-moved` row. A Copy-owned row also has an
/// entry in the fork registry's `copies` map, tracking its own path and
/// `disabled` flag. Restoring the folder here would leave that entry stale
/// (still pointing at `.skill-studio-disabled/`, still `disabled: true`)
/// since `restore_moved_deployment` only patches the scan snapshot, not the
/// registry - see `issue-4.4-followup-a.md`'s one-shot migration note.
fn refuse_registry_copy_restore(deployment: &Deployment) -> Result<(), String> {
    if deployment.owner_kind == LifecycleOwnerKind::Copy {
        return Err(format!(
            "\"{}\" is tracked by the fork registry; restore it by hand or wait for the \
             .skill-studio-disabled/ migration",
            deployment.path
        ));
    }
    Ok(())
}

/// The synchronous half of `restore_moved_deployment`: validates `deployment`
/// was actually moved aside, refuses a Copy-owned row
/// (`refuse_registry_copy_restore`), and performs the filesystem restore and
/// id recompute. Split out so it's testable with a `Deployment` fixture and
/// no `tauri::AppHandle` - the async command only adds the snapshot patch,
/// which does need one.
fn restore_moved_deployment_for(
    deployment: &Deployment,
    deployment_id: &str,
) -> Result<(PathBuf, String), String> {
    if deployment.disabled_by != Some(DisabledBy::StudioMoved) {
        return Err(format!(
            "\"{}\" was not moved aside by Skill Studio",
            deployment.path
        ));
    }
    refuse_registry_copy_restore(deployment)?;
    let path_buf = PathBuf::from(&deployment.path);
    let new_path = restore_deployment_at(&path_buf)?;
    let parsed = super::skill_deployment::parse_deployment_id(deployment_id)
        .ok_or_else(|| format!("Not a copy id: {deployment_id}"))?;
    let new_id = super::skill_deployment::deployment_id(
        &parsed.name,
        &parsed.scope,
        parsed.destination,
        &parsed.slot,
        parsed.project_path.as_deref(),
        &new_path,
    );
    Ok((new_path, new_id))
}

/// Restores a deployment the removed (unit 4.4) move-aside disable left
/// under `.skill-studio-disabled/`. `ops::scan` still reports those rows as
/// `disabled_by: "studio-moved"` (`crates/skill-studio-core/src/ops.rs`'s
/// `scan_move_aside_dir`) so the UI keeps showing them, but `unpark` refuses
/// them - their root is a plain directory, not `RootKind::Parked` - and no
/// native per-harness switch applies to a plain directory copy either. This
/// is the only way back for one of those legacy rows now that the disable
/// side (`disable_deployment_at`) is gone; `issue-4.4-followup-a.md` tracks
/// a one-shot migration that retires `.skill-studio-disabled/` entirely,
/// after which this command goes too. See
/// `docs/action-map/enable-and-links.md`. Delegates the synchronous checks
/// and restore to `restore_moved_deployment_for`.
#[tauri::command]
pub async fn restore_moved_deployment(
    target: LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "restore_moved_deployment", move || {
        let refresh_state = app.state::<SkillRefreshState>();
        let deployment_id = target
            .deployment_id
            .clone()
            .ok_or("Restoring a moved copy needs one copy id")?;
        let resolved = super::skill_lifecycle::resolve_fresh_lifecycle_target(
            &app,
            &refresh_state,
            &target,
            "Restore moved deployment",
        )?;
        let deployment = resolved.deployment;
        let (new_path, new_id) = restore_moved_deployment_for(&deployment, &deployment_id)?;

        // Surgical: patch the moved deployment's path and disabled state right
        // away, the background loop's full
        // rebuild (skills_dirty) reconciles the rest moments later.
        if let Err(e) = skill_refresh::patch_snapshot_and_emit(&app, &refresh_state, |snapshot| {
            let Some(deployment) = snapshot
                .skills
                .iter_mut()
                .flat_map(|skill| skill.deployments.iter_mut())
                .find(|deployment| deployment.id == deployment_id)
            else {
                return;
            };
            deployment.path = new_path.to_string_lossy().to_string();
            deployment.id = new_id;
            deployment.disabled = false;
            deployment.disabled_by = None;
        }) {
            eprintln!("[restore_moved_deployment] snapshot patch failed: {e}");
        }
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::test_support::write_skill;

    /// `restore_deployment_at`'s round trip for the legacy holding
    /// directory the removed `disable_deployment_at` used to create - the
    /// helper `restore_moved_deployment`'s Tauri command calls after
    /// resolving the target against a fresh snapshot.
    #[test]
    fn studio_moved_deployment_reenable_restores_folder_or_names_the_refusal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".cursor/skills");
        let holding_dir = root.join(STUDIO_DISABLED_DIR_NAME);
        write_skill(&holding_dir.join("find-bugs"), "find-bugs");

        let restored = restore_deployment_at(&holding_dir.join("find-bugs")).unwrap();
        assert_eq!(restored, root.join("find-bugs"));
        assert!(restored.join("SKILL.md").is_file());
        assert!(!holding_dir.join("find-bugs").exists());

        let err = restore_deployment_at(&root.join("find-bugs")).unwrap_err();
        assert!(
            err.contains(STUDIO_DISABLED_DIR_NAME),
            "expected the named refusal for a path outside the holding directory: {err}"
        );
    }

    /// Goes through `restore_moved_deployment_for`, the function the
    /// `restore_moved_deployment` command delegates to, so deleting its
    /// call to `refuse_registry_copy_restore` turns this test red instead of
    /// leaving it green against the guard in isolation.
    #[test]
    fn restore_moved_deployment_refuses_a_registry_copy_or_names_the_path() {
        let copy = Deployment {
            owner_kind: LifecycleOwnerKind::Copy,
            disabled_by: Some(DisabledBy::StudioMoved),
            path: "/home/.claude/skills/find-bugs".to_string(),
            ..Default::default()
        };
        let err = restore_moved_deployment_for(&copy, "dep:v1/project/claude-code/find-bugs")
            .unwrap_err();
        assert!(err.contains("/home/.claude/skills/find-bugs"), "{err}");
        assert!(err.contains("fork registry"), "{err}");
    }

    #[test]
    fn manual_row_restores_or_names_the_registry_copy_refusal() {
        let manual = Deployment {
            owner_kind: LifecycleOwnerKind::Manual,
            path: "/home/.claude/skills/find-bugs".to_string(),
            ..Default::default()
        };
        assert!(refuse_registry_copy_restore(&manual).is_ok());
    }
}
