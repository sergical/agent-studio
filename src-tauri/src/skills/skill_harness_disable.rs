// ============================================================================
// Skills Module - skill_harness_disable
// Per-harness disable, distinct from `skill_park` (which disables a skill
// everywhere by moving its shared folder aside). Three mechanisms, one per
// harness that has a native switch, plus a refusal for the rest:
//   - Codex: `~/.codex/config.toml` `[[skills.config]] enabled = false`
//     (codex_skill_config.rs).
//   - OpenCode: `~/.config/opencode/opencode.json` `permission.skill.<name>
//     = "deny"` (opencode_skill_permission.rs).
//   - Claude Code: no native per-skill switch, so this removes/recreates the
//     per-skill symlink under `~/.claude/skills/<name>` and records the fact
//     in the registry's `harness_disabled` bucket (skill_park.rs's
//     `take_claude_link`/`restore_claude_link`, shared with parking).
//   - pi, Cursor, Grok Build: no per-skill switch and no substitute exists
//     (they read the shared folder directly), so this refuses outright
//     rather than silently doing nothing.
// ============================================================================

use std::path::Path;

use super::codex_skill_config;
use super::opencode_skill_permission;
use super::skill_agent_runner::validate_skill_dir_name;
use super::skill_fork::ForkMutationLock;
use super::skill_fork_registry::{read_fork_registry, write_fork_registry, ClaudeLinkRemoved};
use super::skill_park::{
    claude_link_state, restore_claude_link, take_claude_link, ClaudeLinkState,
};
use super::skill_refresh::{self, SkillRefreshState};

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

/// `set_harness_enabled`'s logic, taking `home` (and, for Codex, the
/// deployment's canonical `SKILL.md` path) directly so it's testable
/// without a Tauri `AppHandle`. `agent` is an `AgentId::cli_name()`, e.g.
/// `"codex"`, `"opencode"`, `"claude-code"`.
pub fn set_harness_enabled_with(
    home: &Path,
    name: &str,
    agent: &str,
    enabled: bool,
    codex_skill_md_path: Option<&Path>,
) -> Result<(), String> {
    validate_skill_dir_name(name)?;
    match agent {
        "codex" => {
            let path = codex_skill_md_path
                .ok_or_else(|| format!("No Codex deployment found for \"{name}\""))?;
            codex_skill_config::set_skill_disabled(home, path, !enabled)
        }
        "opencode" => opencode_skill_permission::set_skill_denied(home, name, !enabled),
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

    let codex_skill_md_path = if agent == "codex" {
        let snapshot = refresh_state.snapshot.read().ok().and_then(|g| g.clone());
        snapshot
            .as_ref()
            .and_then(|s| s.skills.iter().find(|s| s.name == name))
            .and_then(|s| s.deployments.iter().find(|d| d.agent == "Codex"))
            .map(|d| std::path::PathBuf::from(&d.path).join("SKILL.md"))
    } else {
        None
    };

    let result = set_harness_enabled_with(
        &home,
        &name,
        &agent,
        enabled,
        codex_skill_md_path.as_deref(),
    );
    if result.is_ok() {
        if let Err(e) = skill_refresh::rebuild_snapshot_now(&app, &refresh_state) {
            eprintln!("[set_harness_enabled] snapshot rebuild failed: {e}");
        }
    }
    result
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

        set_harness_enabled_with(home, "find-bugs", "codex", false, Some(&skill_md)).unwrap();
        assert_eq!(
            codex_skill_config::read_disabled_skill_md_paths(home),
            vec![fs::canonicalize(&skill_md).unwrap()]
        );

        set_harness_enabled_with(home, "find-bugs", "codex", true, Some(&skill_md)).unwrap();
        assert!(codex_skill_config::read_disabled_skill_md_paths(home).is_empty());
    }

    #[test]
    fn codex_disable_without_a_deployment_path_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        let err =
            set_harness_enabled_with(tmp.path(), "find-bugs", "codex", false, None).unwrap_err();
        assert!(err.contains("No Codex deployment"));
    }

    #[test]
    fn opencode_disable_and_reenable_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        set_harness_enabled_with(home, "find-bugs", "opencode", false, None).unwrap();
        assert_eq!(
            opencode_skill_permission::read_denied_patterns(home),
            vec!["find-bugs".to_string()]
        );

        set_harness_enabled_with(home, "find-bugs", "opencode", true, None).unwrap();
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

        set_harness_enabled_with(home, "find-bugs", "claude-code", false, None).unwrap();
        assert!(!home.join(".claude/skills/find-bugs").exists());
        let registry = read_fork_registry(home).unwrap();
        assert_eq!(
            registry.harness_disabled["find-bugs"]["claude-code"].link_target,
            std::path::PathBuf::from("../../.agents/skills/find-bugs")
        );

        set_harness_enabled_with(home, "find-bugs", "claude-code", true, None).unwrap();
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
            set_harness_enabled_with(home, "find-bugs", "claude-code", false, None).unwrap_err();
        assert!(err.contains("whole shared folder"));
    }

    #[test]
    fn pi_cursor_and_grok_build_refuse() {
        let tmp = tempfile::tempdir().unwrap();
        for agent in ["pi", "cursor", "grok-build"] {
            let err =
                set_harness_enabled_with(tmp.path(), "find-bugs", agent, false, None).unwrap_err();
            assert!(err.contains("no per-skill disable"), "{agent}: {err}");
        }
    }
}
