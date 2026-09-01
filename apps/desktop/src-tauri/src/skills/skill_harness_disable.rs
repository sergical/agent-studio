// ============================================================================
// Skills Module - skill_harness_disable
// Per-harness disable, distinct from `skill_park` (which disables a skill
// everywhere by moving its shared folder aside). Three native mechanisms,
// one per harness that has one, plus a universal fallback for the rest:
//   - Codex: `~/.codex/config.toml` `[[skills.config]] enabled = false`
//     (codex_skill_config.rs).
//   - OpenCode: `~/.config/opencode/opencode.json` `permission.skill.<name>
//     = "deny"` (opencode_skill_permission.rs).
//   - Claude Code: no native per-skill switch, so this removes/recreates the
//     per-skill symlink under `~/.claude/skills/<name>` and records the fact
//     in the registry's `harness_disabled` bucket (skill_park.rs's
//     `take_claude_link`/`restore_claude_link`, shared with parking).
//   - Every other deployment (plain directory copies, project-scope
//     symlinks, pi/Cursor/Grok Build): `disable_deployment_at`/
//     `restore_deployment_at` rename the deployment's directory into a
//     sibling `.skill-studio-disabled/` holding directory in the same skills
//     root. Harnesses scan their skills root one level deep, so the moved
//     entry becomes invisible to them without touching its content -
//     `skill_discovery.rs` walks the holding directory the same way so the
//     UI still shows it (as disabled). Shared-root and plugin-cache
//     deployments refuse this - see `set_deployment_enabled`.
// ============================================================================

use std::path::{Path, PathBuf};

use super::codex_skill_config;
use super::event_commands::EventStoreState;
use super::event_store::{fingerprint_path, EventDraft, EventStatus, InverseOp};
use super::opencode_skill_permission;
use super::skill_agent_runner::validate_skill_dir_name;
use super::skill_discovery::STUDIO_DISABLED_DIR_NAME;
use super::skill_dto::DisabledBy;
use super::skill_fork::ForkMutationLock;
use super::skill_fork_registry::{read_fork_registry, write_fork_registry, ClaudeLinkRemoved};
use super::skill_park::{
    claude_link_state, restore_claude_link, take_claude_link, ClaudeLinkState,
};
use super::skill_refresh::{self, SkillRefreshState};

/// Disable a deployment with no native per-harness switch by renaming its
/// directory into a sibling `.skill-studio-disabled/` holding directory in
/// its skills root (creating it if missing). Returns the moved path.
/// Refuses if the destination already exists. A relative symlink is
/// recreated one level deeper at the destination (target prefixed with
/// `../`) rather than renamed as-is, so it still resolves to the same
/// canonical target from inside the holding directory; an absolute symlink,
/// or a plain directory, is renamed as-is.
pub fn disable_deployment_at(path: &Path) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("\"{}\" has no file name", path.display()))?;
    let root = path
        .parent()
        .ok_or_else(|| format!("\"{}\" has no parent directory", path.display()))?;
    refuse_shared_root(root)?;
    let holding_dir = root.join(STUDIO_DISABLED_DIR_NAME);
    std::fs::create_dir_all(&holding_dir)
        .map_err(|e| format!("Failed to create {}: {e}", holding_dir.display()))?;
    let dest = holding_dir.join(name);
    move_deployment(path, &dest, |target| Path::new("..").join(target))
}

/// Restore a deployment `disable_deployment_at` moved aside, renaming it back
/// from `<root>/.skill-studio-disabled/<name>` to `<root>/<name>`. `path`
/// must sit directly inside a `.skill-studio-disabled` directory. Refuses if
/// the original position is already occupied. Reverses the relative-symlink
/// adjustment `disable_deployment_at` made, by stripping one leading `../`.
pub fn restore_deployment_at(path: &Path) -> Result<PathBuf, String> {
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

/// Shared move for `disable_deployment_at`/`restore_deployment_at`: refuses
/// if `dest` already exists, relinks a relative symlink one level shallower
/// or deeper via `adjust_relative_target` so it keeps resolving to the same
/// canonical target, and otherwise renames `path` to `dest` as-is.
fn move_deployment(
    path: &Path,
    dest: &Path,
    adjust_relative_target: impl FnOnce(PathBuf) -> PathBuf,
) -> Result<PathBuf, String> {
    if std::fs::symlink_metadata(dest).is_ok() {
        return Err(format!("\"{}\" already exists", dest.display()));
    }

    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(path)
            .map_err(|e| format!("Failed to read symlink {}: {e}", path.display()))?;
        if target.is_relative() {
            // Create the adjusted destination first so a failure at any point
            // leaves the original link in place; only then remove the source.
            let adjusted = adjust_relative_target(target);
            create_symlink(&adjusted, dest)?;
            if let Err(e) = std::fs::remove_file(path) {
                let _ = std::fs::remove_file(dest);
                return Err(format!("Failed to remove {}: {e}", path.display()));
            }
            return Ok(dest.to_path_buf());
        }
    }

    std::fs::rename(path, dest).map_err(|e| {
        format!(
            "Failed to move {} to {}: {e}",
            path.display(),
            dest.display()
        )
    })?;
    Ok(dest.to_path_buf())
}

/// Refuse to move a deployment whose skills root physically resolves into a
/// shared `.agents/skills` folder. The agent-label check in
/// `set_deployment_enabled` misses this case: a harness dir that is itself a
/// symlink into the shared root (e.g. `~/.claude/skills -> ../.agents/skills`)
/// makes the "Claude Code" deployment's real location the shared copy, and
/// moving it would disable the skill for every harness at once.
fn refuse_shared_root(root: &Path) -> Result<(), String> {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let components: Vec<_> = canonical
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    if components.windows(2).any(|w| w == [".agents", "skills"]) {
        return Err(format!(
            "\"{}\" resolves into the shared .agents/skills folder - disabling it here would \
             disable the skill for every harness. Park the skill instead.",
            root.display()
        ));
    }
    Ok(())
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

/// The deterministic destination `disable_deployment_at`/`restore_deployment_at`
/// compute for `path`, without performing the move - so the event can be
/// recorded before the mutation runs. `enabled` selects which direction:
/// `true` mirrors `restore_deployment_at` (moving out of the holding
/// directory), `false` mirrors `disable_deployment_at` (moving into it).
fn move_aside_dest(path: &Path, enabled: bool) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("\"{}\" has no file name", path.display()))?;
    if enabled {
        let holding_dir = path
            .parent()
            .ok_or_else(|| format!("\"{}\" has no parent directory", path.display()))?;
        let root = holding_dir
            .parent()
            .ok_or_else(|| format!("\"{}\" has no parent directory", holding_dir.display()))?;
        Ok(root.join(name))
    } else {
        let root = path
            .parent()
            .ok_or_else(|| format!("\"{}\" has no parent directory", path.display()))?;
        Ok(root.join(STUDIO_DISABLED_DIR_NAME).join(name))
    }
}

/// Records the pending `move_aside_disable`/`move_aside_restore` event for a
/// `set_deployment_enabled` move, before the move happens. `pre_fingerprint`
/// is `path`'s fingerprint right now - the destination the inverse restores,
/// since `path` is still at its pre-mutation position. Returns the event id
/// and `path` (the inverse's destination), for `finish_move_aside_event`.
fn record_move_aside_event(
    store: &super::event_store::EventStore,
    name: &str,
    path: &Path,
    enabled: bool,
) -> Result<(String, PathBuf), String> {
    let dest = move_aside_dest(path, enabled)?;
    let pre_fingerprint = fingerprint_path(path);
    let id = super::event_store::allocate_id();
    let inverse = InverseOp::MoveBack {
        from: dest.clone(),
        to: path.to_path_buf(),
        pre_fingerprint,
        post_fingerprint: None,
    };
    store.record(
        &id,
        EventDraft {
            kind: if enabled {
                "move_aside_restore".to_string()
            } else {
                "move_aside_disable".to_string()
            },
            skill: name.to_string(),
            harness: None,
            scope: None,
            project_path: None,
            payload: serde_json::json!({ "from": path, "to": dest }),
            inverse: Some(
                serde_json::to_value(&inverse)
                    .map_err(|e| format!("Failed to serialize inverse: {e}"))?,
            ),
            backup_dir: None,
        },
    )?;
    Ok((id, path.to_path_buf()))
}

/// Patches the recorded event's post-fingerprint and finishes it `done` or
/// `failed`, matching the outcome of the move `record_move_aside_event`
/// preceded. `original` is `path`'s pre-mutation position (now absent on
/// success - the content moved to `dest`).
fn finish_move_aside_event(
    store: &super::event_store::EventStore,
    id: &str,
    original: &Path,
    result: &Result<PathBuf, String>,
) {
    match result {
        Ok(_) => {
            let post_fp = fingerprint_path(original);
            if let Err(e) = store.patch_inverse_post_fingerprint(id, &post_fp) {
                eprintln!("[set_deployment_enabled] failed to patch event {id}: {e}");
            }
            if let Err(e) = store.finish(id, EventStatus::Done) {
                eprintln!("[set_deployment_enabled] failed to finish event {id}: {e}");
            }
        }
        Err(_) => {
            if let Err(e) = store.finish(id, EventStatus::Failed) {
                eprintln!("[set_deployment_enabled] failed to finish event {id}: {e}");
            }
        }
    }
}

/// Disables (or re-enables) `name` for Claude Code by removing (or
/// recreating) its per-skill symlink under `~/.claude/skills/<name>`.
/// Refuses when Claude Code has no per-skill symlink to remove - either
/// nothing is deployed there, or `~/.claude/skills` is the whole-dir symlink
/// to the shared root, which covers every skill at once and can't be
/// toggled per skill.
fn set_claude_code_enabled(home: &Path, name: &str, enabled: bool) -> Result<(), String> {
    let mut registry = read_fork_registry(home)?;

    if enabled {
        let Some(record) = registry
            .harness_disabled
            .get(name)
            .and_then(|by_harness| by_harness.get("claude-code"))
            .cloned()
        else {
            return Ok(()); // already enabled, nothing recorded - idempotent.
        };
        restore_claude_link(home, name, &record.link_target)?;
        if let Some(by_harness) = registry.harness_disabled.get_mut(name) {
            by_harness.remove("claude-code");
            if by_harness.is_empty() {
                registry.harness_disabled.remove(name);
            }
        }
        write_fork_registry(home, &registry)?;
        return Ok(());
    }

    match claude_link_state(home, name) {
        ClaudeLinkState::WholeDir => Err(
            "Claude Code reads the whole shared folder for skills, not a per-skill symlink - it cannot be disabled for just this skill".to_string(),
        ),
        ClaudeLinkState::None => {
            Err(format!("\"{name}\" is not deployed to Claude Code via a per-skill symlink"))
        }
        ClaudeLinkState::PerSkill => {
            let Some(target) = take_claude_link(home, name)? else {
                return Err(format!(
                    "\"{name}\" is not deployed to Claude Code via a per-skill symlink"
                ));
            };
            registry
                .harness_disabled
                .entry(name.to_string())
                .or_default()
                .insert(
                    "claude-code".to_string(),
                    ClaudeLinkRemoved { link_target: target },
                );
            write_fork_registry(home, &registry)
        }
    }
}

/// `set_harness_enabled`'s logic, taking `home` (and, for Codex, every
/// deployment path Codex can see the skill at - its own dir and any shared
/// root) directly so it's testable without a Tauri `AppHandle`. `agent` is an
/// `AgentId::cli_name()`, e.g. `"codex"`, `"opencode"`, `"claude-code"`.
pub fn set_harness_enabled_with(
    home: &Path,
    name: &str,
    agent: &str,
    enabled: bool,
    codex_skill_md_paths: &[PathBuf],
) -> Result<(), String> {
    validate_skill_dir_name(name)?;
    match agent {
        "codex" => {
            if codex_skill_md_paths.is_empty() {
                return Err(format!("No Codex-visible deployment found for \"{name}\""));
            }
            for path in codex_skill_md_paths {
                codex_skill_config::set_skill_disabled(home, path, !enabled)?;
            }
            Ok(())
        }
        // The frontend's AgentId spells it "open-code"; the CLI name is "opencode".
        "opencode" | "open-code" => {
            opencode_skill_permission::set_skill_denied(home, name, !enabled)
        }
        "claude-code" => set_claude_code_enabled(home, name, enabled),
        "pi" | "cursor" | "grok-build" => Err(format!(
            "{agent} has no per-skill disable - it reads the shared skills folder directly"
        )),
        other => Err(format!("Unknown harness: {other}")),
    }
}

#[tauri::command]
pub fn set_harness_enabled(
    name: String,
    agent: String,
    enabled: bool,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;

    let codex_skill_md_paths: Vec<PathBuf> = if agent == "codex" {
        let snapshot = refresh_state.snapshot.read().ok().and_then(|g| g.clone());
        snapshot
            .as_ref()
            .and_then(|s| s.skills.iter().find(|s| s.name == name))
            .map(|s| {
                s.deployments
                    .iter()
                    .filter(|d| d.agent == "Codex" || d.agent == "shared")
                    .map(|d| PathBuf::from(&d.path).join("SKILL.md"))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let result = set_harness_enabled_with(&home, &name, &agent, enabled, &codex_skill_md_paths);
    if result.is_ok() {
        // Surgical: mark the harness's deployments right away; the background
        // loop's full rebuild (skills_dirty) re-derives the true state - which
        // mechanism disabled it, and the symlink Claude Code's removal took.
        let normalized_agent: String = agent.chars().filter(|c| *c != '-').collect();
        if let Err(e) = skill_refresh::patch_snapshot_and_emit(&app, &refresh_state, |snapshot| {
            let Some(skill) = snapshot.skills.iter_mut().find(|s| s.name == name) else {
                return;
            };
            for deployment in &mut skill.deployments {
                let label: String = deployment
                    .agent
                    .to_lowercase()
                    .chars()
                    .filter(|c| *c != ' ' && *c != '-')
                    .collect();
                if label == normalized_agent {
                    deployment.disabled = !enabled;
                    if enabled {
                        deployment.disabled_by = None;
                    }
                }
                if deployment.agent == "shared" && matches!(agent.as_str(), "codex" | "open-code") {
                    if enabled {
                        deployment.disabled_readers.retain(|a| a != &agent);
                    } else if !deployment.disabled_readers.contains(&agent) {
                        deployment.disabled_readers.push(agent.clone());
                    }
                }
            }
        }) {
            eprintln!("[set_harness_enabled] snapshot patch failed: {e}");
        }
    }
    result
}

/// Disable (or re-enable) a deployment by moving it into (or out of) its
/// skills root's `.skill-studio-disabled/` holding directory - the universal
/// fallback for harnesses with no native per-skill switch (see the module
/// doc). Refuses shared-root and plugin-provided deployments, which this
/// mechanism can't touch: a shared-root move would disable the skill for
/// every harness at once (that's `skill_park`'s job), and a plugin's skill
/// dir is owned by the plugin cache, not something Skill Studio should
/// rename.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn set_deployment_enabled(
    name: String,
    path: String,
    enabled: bool,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
    event_store: tauri::State<EventStoreState>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let path_buf = PathBuf::from(&path);
    super::commands::require_snapshot_owns_path(&refresh_state, &path_buf)?;

    let snapshot = refresh_state.snapshot.read().ok().and_then(|g| g.clone());
    let deployment = snapshot
        .as_ref()
        .and_then(|s| s.skills.iter().find(|s| s.name == name))
        .and_then(|s| s.deployments.iter().find(|d| d.path == path));
    if !enabled {
        if let Some(deployment) = deployment {
            if deployment.agent == "shared" {
                return Err(
                    "Shared-root deployments can't be disabled per harness - park the skill instead"
                        .to_string(),
                );
            }
            if deployment.plugin.is_some() {
                return Err("Plugin-provided deployments can't be disabled".to_string());
            }
        }
    }

    // The event id is allocated (and the pending row recorded) before the
    // move, from the same deterministic dest `disable_deployment_at`/
    // `restore_deployment_at` compute internally - see the module doc's
    // move-aside mechanism. `store_guard` may hold `None` if the event store
    // failed to open at startup; the move still happens (the mutation isn't
    // gated on event logging), it just isn't recorded/restorable.
    let store_guard = event_store
        .0
        .lock()
        .map_err(|e| format!("event store lock poisoned: {e}"))?;
    let store = store_guard.as_ref();
    let event = store
        .map(|store| record_move_aside_event(store, &name, &path_buf, enabled))
        .transpose()?;

    let result = if enabled {
        restore_deployment_at(&path_buf)
    } else {
        disable_deployment_at(&path_buf)
    };

    if let (Some(store), Some((id, original))) = (store, &event) {
        finish_move_aside_event(store, id, original, &result);
    }
    drop(store_guard);

    let new_path = result?;

    // Surgical: patch the moved deployment's path and disabled state right
    // away; the background loop's full rebuild (skills_dirty) reconciles the
    // rest (frontmatter fields, hashes) moments later.
    if let Err(e) = skill_refresh::patch_snapshot_and_emit(&app, &refresh_state, |snapshot| {
        let Some(skill) = snapshot.skills.iter_mut().find(|s| s.name == name) else {
            return;
        };
        let Some(deployment) = skill.deployments.iter_mut().find(|d| d.path == path) else {
            return;
        };
        deployment.path = new_path.to_string_lossy().to_string();
        deployment.disabled = !enabled;
        deployment.disabled_by = if enabled {
            None
        } else {
            Some(DisabledBy::StudioMoved)
        };
    }) {
        eprintln!("[set_deployment_enabled] snapshot patch failed: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_skill(dir: &Path, name: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test\n---\nBody."),
        )
        .unwrap();
    }

    #[test]
    fn codex_disable_and_reenable_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let skill_md = home.join("skills/find-bugs/SKILL.md");
        write_skill(skill_md.parent().unwrap(), "find-bugs");

        set_harness_enabled_with(
            home,
            "find-bugs",
            "codex",
            false,
            std::slice::from_ref(&skill_md),
        )
        .unwrap();
        assert_eq!(
            codex_skill_config::read_disabled_skill_md_paths(home),
            vec![fs::canonicalize(&skill_md).unwrap()]
        );

        set_harness_enabled_with(
            home,
            "find-bugs",
            "codex",
            true,
            std::slice::from_ref(&skill_md),
        )
        .unwrap();
        assert!(codex_skill_config::read_disabled_skill_md_paths(home).is_empty());
    }

    #[test]
    fn codex_disable_without_a_deployment_path_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        let err =
            set_harness_enabled_with(tmp.path(), "find-bugs", "codex", false, &[]).unwrap_err();
        assert!(err.contains("No Codex-visible deployment"));
    }

    #[test]
    fn opencode_disable_and_reenable_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        set_harness_enabled_with(home, "find-bugs", "opencode", false, &[]).unwrap();
        assert_eq!(
            opencode_skill_permission::read_denied_patterns(home),
            vec!["find-bugs".to_string()]
        );

        set_harness_enabled_with(home, "find-bugs", "opencode", true, &[]).unwrap();
        assert!(opencode_skill_permission::read_denied_patterns(home).is_empty());
    }

    #[test]
    fn claude_code_disable_removes_and_reenable_restores_the_per_skill_link() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        std::os::unix::fs::symlink(
            "../../.agents/skills/find-bugs",
            home.join(".claude/skills/find-bugs"),
        )
        .unwrap();

        set_harness_enabled_with(home, "find-bugs", "claude-code", false, &[]).unwrap();
        assert!(!home.join(".claude/skills/find-bugs").exists());
        let registry = read_fork_registry(home).unwrap();
        assert_eq!(
            registry.harness_disabled["find-bugs"]["claude-code"].link_target,
            std::path::PathBuf::from("../../.agents/skills/find-bugs")
        );

        set_harness_enabled_with(home, "find-bugs", "claude-code", true, &[]).unwrap();
        assert!(fs::symlink_metadata(home.join(".claude/skills/find-bugs"))
            .unwrap()
            .file_type()
            .is_symlink());
        let registry = read_fork_registry(home).unwrap();
        assert!(!registry.harness_disabled.contains_key("find-bugs"));
    }

    #[test]
    fn claude_code_disable_refuses_on_whole_dir_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude")).unwrap();
        std::os::unix::fs::symlink(home.join(".agents/skills"), home.join(".claude/skills"))
            .unwrap();

        let err =
            set_harness_enabled_with(home, "find-bugs", "claude-code", false, &[]).unwrap_err();
        assert!(err.contains("whole shared folder"));
    }

    #[test]
    fn pi_cursor_and_grok_build_refuse() {
        let tmp = tempfile::tempdir().unwrap();
        for agent in ["pi", "cursor", "grok-build"] {
            let err =
                set_harness_enabled_with(tmp.path(), "find-bugs", agent, false, &[]).unwrap_err();
            assert!(err.contains("no per-skill disable"), "{agent}: {err}");
        }
    }

    #[test]
    fn disable_then_restore_round_trips_a_plain_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".cursor/skills");
        write_skill(&root.join("find-bugs"), "find-bugs");

        let moved = disable_deployment_at(&root.join("find-bugs")).unwrap();
        assert_eq!(moved, root.join(".skill-studio-disabled/find-bugs"));
        assert!(!root.join("find-bugs").exists());
        assert!(moved.join("SKILL.md").is_file());

        let restored = restore_deployment_at(&moved).unwrap();
        assert_eq!(restored, root.join("find-bugs"));
        assert!(restored.join("SKILL.md").is_file());
        assert!(!moved.exists());
    }

    #[test]
    fn disable_refuses_a_root_that_resolves_into_the_shared_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude")).unwrap();
        std::os::unix::fs::symlink("../.agents/skills", home.join(".claude/skills")).unwrap();

        let err = disable_deployment_at(&home.join(".claude/skills/find-bugs")).unwrap_err();
        assert!(err.contains("shared .agents/skills"), "{err}");
        assert!(home.join(".agents/skills/find-bugs/SKILL.md").is_file());
        assert!(!home.join(".agents/skills/.skill-studio-disabled").exists());
    }

    #[test]
    fn disable_then_restore_round_trips_a_relative_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        let root = home.join(".claude/skills");
        fs::create_dir_all(&root).unwrap();
        let link = root.join("find-bugs");
        std::os::unix::fs::symlink("../../.agents/skills/find-bugs", &link).unwrap();

        let moved = disable_deployment_at(&link).unwrap();
        assert_eq!(moved, root.join(".skill-studio-disabled/find-bugs"));
        // Still resolves to the same canonical target from one level deeper.
        assert_eq!(
            fs::canonicalize(&moved).unwrap(),
            fs::canonicalize(home.join(".agents/skills/find-bugs")).unwrap()
        );
        assert_eq!(
            fs::read_link(&moved).unwrap(),
            std::path::PathBuf::from("../../../.agents/skills/find-bugs")
        );

        let restored = restore_deployment_at(&moved).unwrap();
        assert_eq!(restored, link);
        assert_eq!(
            fs::read_link(&restored).unwrap(),
            std::path::PathBuf::from("../../.agents/skills/find-bugs")
        );
    }

    #[test]
    fn disable_refuses_when_destination_already_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".cursor/skills");
        write_skill(&root.join("find-bugs"), "find-bugs");
        write_skill(&root.join(".skill-studio-disabled/find-bugs"), "find-bugs");

        let err = disable_deployment_at(&root.join("find-bugs")).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn restore_refuses_when_original_position_is_occupied() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".cursor/skills");
        write_skill(&root.join("find-bugs"), "find-bugs");
        write_skill(&root.join(".skill-studio-disabled/find-bugs"), "find-bugs");

        let err =
            restore_deployment_at(&root.join(".skill-studio-disabled/find-bugs")).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
    }
}
