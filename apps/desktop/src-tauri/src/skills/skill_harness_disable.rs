// ============================================================================
// Skills Module - skill_harness_disable
// Per-harness disable, distinct from `skill_park` (which disables a skill
// everywhere by moving its shared folder aside). Three native mechanisms,
// one per harness that has one:
//   - Codex: `~/.codex/config.toml` `[[skills.config]] enabled = false`,
//     written through `skill_studio_core::ops::set_codex_skill_disabled`.
//   - OpenCode: `~/.config/opencode/opencode.json` (or its `XDG_CONFIG_HOME`/
//     `OPENCODE_CONFIG_DIR` override) `permission.skill.<name> = "deny"`,
//     via `skill_studio_core::opencode_config`.
//   - Claude Code: no native per-skill switch, so this removes/recreates the
//     per-skill symlink under `~/.claude/skills/<name>`.
// A deployment with none of these switches (plain directory copies,
// project-scope symlinks, pi/Cursor/Grok Build) has no per-harness off
// switch at all; the frontend offers `skill_park::park_skill` instead
// (unit 4.4 removed the `set_deployment_enabled` move-aside fallback that
// used to stand in for it here).
//
// `set_harness_enabled` (the native per-skill switch above) is a thin
// adapter over `skill_studio_core::ops::set_harness_enabled` (unit 3.8):
// the write path - journal-before-first-write, `SymlinkInverse` undo for
// Claude Code, "N of M" Codex partial-toggle reporting - lives in the core
// now. `set_new_universal_reader_enabled` (the post-install switch, called
// from `skill_add.rs`) still holds its own write logic pending a future
// unit; see issue #166's follow-ups.
// ============================================================================

use std::fs;
use std::path::{Path, PathBuf};

use tauri::Manager;

use skill_studio_core::dto::SetHarnessEnabledRequest;
use skill_studio_core::identity::{CorrelationId, SkillName};
use skill_studio_core::ops::{self, Operation, ResultEnvelope};
use skill_studio_core::ports::OpContext;

use super::skill_agent_runner::validate_skill_dir_name;
use super::skill_dto::HarnessVisibilityTarget;
use super::skill_fork_registry::{
    read_fork_registry, write_fork_registry_locked, ClaudeLinkRemoved, ForkRegistry,
};
use super::skill_refresh::{self, SkillRefreshState};

/// Name of the holding directory the universal move-aside disable renames a
/// deployment into. Core already defines this (`identity::MOVE_ASIDE_DIR_NAME`)
/// for `ops::scan`'s own one-level-reader skip; re-exported under its old
/// desktop name so every existing call site here keeps reading unchanged.
pub(crate) use skill_studio_core::identity::MOVE_ASIDE_DIR_NAME as STUDIO_DISABLED_DIR_NAME;

enum ClaudeLinkState {
    PerSkill,
    WholeDir,
    None,
}

fn refuse_opencode_name_collision<'a>(
    selected_scope: &str,
    selected_project: Option<&str>,
    visible_scopes: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> Result<(), String> {
    if visible_scopes.into_iter().any(|(scope, project)| {
        scope != selected_scope || (scope == "project" && project != selected_project)
    }) {
        return Err(
            "OpenCode disables skills by name. This name has more than one OpenCode deployment, so no deployment was changed."
                .to_string(),
        );
    }
    Ok(())
}

fn path_entry_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn guard_new_opencode_deployment(
    home: &Path,
    name: &str,
    deployment_id: &str,
) -> Result<(), String> {
    let parsed = super::skill_deployment::parse_deployment_id(deployment_id)
        .ok_or_else(|| format!("Not a deployment id: {deployment_id}"))?;
    let mut visible = Vec::new();
    for root in [
        home.join(".agents/skills"),
        home.join(".config/opencode/skills"),
        home.join(".config/opencode/skill"),
    ] {
        if path_entry_exists(&root.join(name)) {
            visible.push(("global".to_string(), None));
        }
    }
    let mut projects = skill_studio_host::discover_skill_projects(home);
    if let Some(project) = parsed.project_path.as_deref() {
        let project = PathBuf::from(project);
        if !projects.contains(&project) {
            projects.push(project);
        }
    }
    for project in projects {
        for root in [
            project.join(".agents/skills"),
            project.join(".opencode/skills"),
            project.join(".opencode/skill"),
        ] {
            if path_entry_exists(&root.join(name)) {
                visible.push((
                    "project".to_string(),
                    Some(project.to_string_lossy().to_string()),
                ));
                break;
            }
        }
    }
    refuse_opencode_name_collision(
        &parsed.scope,
        parsed.project_path.as_deref(),
        visible
            .iter()
            .map(|(scope, project)| (scope.as_str(), project.as_deref())),
    )
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

/// Disables (or re-enables) `name` for Claude Code by removing (or
/// recreating) its per-skill symlink under `~/.claude/skills/<name>`.
/// Refuses when Claude Code has no per-skill symlink to remove - either
/// nothing is deployed there, or `~/.claude/skills` is the whole-dir symlink
/// to the shared root, which covers every skill at once and can't be
/// toggled per skill.
fn set_claude_code_enabled(
    guard: &super::write_lease::WriteLeaseGuard,
    home: &Path,
    name: &str,
    deployment_id: &str,
    link_path: &Path,
    expected_target: &Path,
    enabled: bool,
) -> Result<(), String> {
    set_claude_code_enabled_with_registry_writer(
        home,
        name,
        deployment_id,
        link_path,
        expected_target,
        enabled,
        |registry| write_fork_registry_locked(guard, home, registry),
    )
}

#[allow(clippy::too_many_arguments)]
fn set_claude_code_enabled_with_registry_writer(
    home: &Path,
    name: &str,
    deployment_id: &str,
    link_path: &Path,
    expected_target: &Path,
    enabled: bool,
    write_registry: impl FnOnce(&ForkRegistry) -> Result<(), String>,
) -> Result<(), String> {
    let mut registry = read_fork_registry(home)?;
    let registry_key = format!("deployment/{deployment_id}");

    if enabled {
        let record_key = if registry.harness_disabled.contains_key(&registry_key) {
            registry_key.clone()
        } else if registry
            .harness_disabled
            .get(name)
            .and_then(|by_harness| by_harness.get("claude-code"))
            .is_some_and(|record| record.deployment_id == deployment_id)
        {
            name.to_string()
        } else {
            return Ok(());
        };
        let Some(record) = registry
            .harness_disabled
            .get(&record_key)
            .and_then(|by_harness| by_harness.get("claude-code"))
            .cloned()
        else {
            return Ok(()); // already enabled, nothing recorded - idempotent.
        };
        if record.deployment_id != deployment_id {
            return Err("The Claude Code disable record belongs to another deployment".to_string());
        }
        restore_claude_link_at(link_path, &record.link_target)?;
        if let Some(by_harness) = registry.harness_disabled.get_mut(&record_key) {
            by_harness.remove("claude-code");
            if by_harness.is_empty() {
                registry.harness_disabled.remove(&record_key);
            }
        }
        write_registry(&registry)?;
        return Ok(());
    }

    match claude_link_state_at(link_path) {
        ClaudeLinkState::WholeDir => Err(
            "Claude Code reads the whole Universal folder for skills, not a per-skill symlink - it cannot be disabled for just this skill".to_string(),
        ),
        ClaudeLinkState::None => {
            Err(format!("\"{name}\" is not deployed to Claude Code via a per-skill symlink"))
        }
        ClaudeLinkState::PerSkill => {
            let target = fs::read_link(link_path)
                .map_err(|error| format!("Failed to read {}: {error}", link_path.display()))?;
            let resolved = fs::canonicalize(link_path)
                .map_err(|error| format!("Failed to resolve {}: {error}", link_path.display()))?;
            let expected = fs::canonicalize(expected_target).map_err(|error| {
                format!("Failed to resolve {}: {error}", expected_target.display())
            })?;
            if resolved != expected {
                return Err(format!(
                    "{} no longer points to the selected Universal deployment",
                    link_path.display()
                ));
            }
            if target.as_os_str().is_empty() {
                return Err(format!(
                    "\"{name}\" is not deployed to Claude Code via a per-skill symlink"
                ));
            }
            fs::remove_file(link_path)
                .map_err(|error| format!("Failed to remove {}: {error}", link_path.display()))?;
            registry
                .harness_disabled
                .entry(registry_key)
                .or_default()
                .insert(
                    "claude-code".to_string(),
                    ClaudeLinkRemoved {
                        deployment_id: deployment_id.to_string(),
                        link_target: target.clone(),
                    },
                );
            if let Err(write_error) = write_registry(&registry) {
                return match recreate_removed_claude_link(link_path, &target) {
                    Ok(()) => Err(write_error),
                    Err(rollback_error) => Err(format!(
                        "Failed to record the Claude Code disable ({write_error}) and failed to restore {} -> {}: {rollback_error}. Recreate that symlink manually.",
                        link_path.display(),
                        target.display()
                    )),
                };
            }
            Ok(())
        }
    }
}

/// Recreates the exact raw target of a Claude link removed by a disable that
/// could not be committed. Unlike the idempotent enable helper, this refuses
/// any path that appeared after removal so rollback never hides a collision.
fn recreate_removed_claude_link(link_path: &Path, target: &Path) -> Result<(), String> {
    let parent = link_path.parent().ok_or("Claude Code link has no parent")?;
    if fs::symlink_metadata(parent).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(format!(
            "{} became a whole-directory symlink",
            parent.display()
        ));
    }
    match fs::symlink_metadata(link_path) {
        Ok(_) => {
            return Err(format!(
                "{} became occupied before rollback",
                link_path.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "Failed to inspect {} before rollback: {error}",
                link_path.display()
            ));
        }
    }
    create_symlink(target, link_path)
}

fn claude_link_state_at(link_path: &Path) -> ClaudeLinkState {
    if fs::symlink_metadata(link_path.parent().unwrap_or_else(|| Path::new("")))
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return ClaudeLinkState::WholeDir;
    }
    match fs::symlink_metadata(link_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => ClaudeLinkState::PerSkill,
        _ => ClaudeLinkState::None,
    }
}

fn restore_claude_link_at(link_path: &Path, target: &Path) -> Result<(), String> {
    let parent = link_path.parent().ok_or("Claude Code link has no parent")?;
    if fs::symlink_metadata(parent).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Ok(());
    }
    fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
    if fs::symlink_metadata(link_path).is_ok() {
        return Ok(());
    }
    create_symlink(target, link_path)
}

/// `set_harness_enabled`'s logic, taking `home` (and, for Codex, every
/// deployment path Codex can see the skill at - its own dir and any shared
/// root) directly so it's testable without a Tauri `AppHandle`. `agent` is an
/// `AgentId::cli_name()`, e.g. `"codex"`, `"opencode"`, `"claude-code"`.
/// `data_root` is the lease/history root Codex's write shares with every
/// other desktop mutation (`super::core_runtime::data_root()` in
/// production); a test passes its own tempdir root for isolation. `guard`
/// is the caller's already-held exclusive lease on `data_root` - the Codex
/// arm reuses it rather than acquiring a second one, which would
/// self-deadlock (advisory locks don't nest in-process).
pub fn set_harness_enabled_with(
    home: &Path,
    data_root: &Path,
    name: &str,
    agent: &str,
    enabled: bool,
    codex_skill_md_paths: &[PathBuf],
    guard: &super::write_lease::WriteLeaseGuard,
) -> Result<(), String> {
    validate_skill_dir_name(name)?;
    match agent {
        "codex" => {
            if codex_skill_md_paths.is_empty() {
                return Err(format!("No Codex-visible deployment found for \"{name}\""));
            }
            let rt = super::core_runtime::build_runtime_write_at(home, data_root)?;
            let ctx = skill_studio_core::ports::OpContext::uncancellable(
                skill_studio_core::identity::CorrelationId(ulid::Ulid::new().to_string()),
            );
            for path in codex_skill_md_paths {
                skill_studio_core::ops::set_codex_skill_disabled_with(
                    &rt,
                    &ctx,
                    guard.as_exclusive_guard(),
                    path,
                    !enabled,
                )
                .map_err(|e| e.message)?;
            }
            Ok(())
        }
        // The frontend's AgentId spells it "open-code"; the CLI name is "opencode".
        "opencode" | "open-code" => {
            let config_dir = skill_refresh::opencode_config_root(home);
            // Core's scope normalization canonicalizes the write's home
            // (`config_dir`'s parent), which requires it to already exist -
            // same bootstrapping gap `write_fork_registry` has for a
            // never-before-seen `~/.agents`. `$XDG_CONFIG_HOME/opencode`'s
            // default, `~/.config/opencode`, is commonly two levels deeper
            // than `home` on a fresh install, so create the whole chain
            // here rather than just one level.
            fs::create_dir_all(&config_dir)
                .map_err(|e| format!("Failed to create {}: {e}", config_dir.display()))?;
            let real_fs = skill_studio_host::RealFs::new();
            // `set_skill_denied_with` trusts its caller to already hold the
            // exclusive lease it needs - `config_dir`'s canonical parent -
            // and `guard` here is keyed to `home` instead, which is the
            // same root only when `OPENCODE_CONFIG_DIR`/`XDG_CONFIG_HOME`
            // happens to point `config_dir` directly under `home`. Reuse
            // `guard` only when its keys actually cover that root; fall
            // back to `set_skill_denied`, which acquires its own
            // correctly-scoped `FileLease`, otherwise.
            let config_home = config_dir.parent().ok_or_else(|| {
                format!(
                    "{} has no parent to scope the write to",
                    config_dir.display()
                )
            })?;
            let canonical_config_home = config_home
                .canonicalize()
                .map_err(|e| format!("Failed to canonicalize {}: {e}", config_home.display()))?;
            let covered_by_guard = guard
                .as_exclusive_guard()
                .keys()
                .iter()
                .any(|key| key.canonical_root == canonical_config_home);
            if covered_by_guard {
                skill_studio_core::opencode_config::set_skill_denied_with(
                    &real_fs,
                    guard.as_exclusive_guard(),
                    &config_dir,
                    name,
                    !enabled,
                )
                .map_err(|e| e.to_string())
            } else {
                let leases = skill_studio_host::FileLease::new(data_root.join("leases"));
                skill_studio_core::opencode_config::set_skill_denied(
                    &leases,
                    &real_fs,
                    &config_dir,
                    name,
                    !enabled,
                )
                .map_err(|e| e.to_string())
            }
        }
        "claude-code" => Err("Claude Code visibility needs an exact deployment target".to_string()),
        "pi" | "cursor" | "grok-build" => Err(format!(
            "{agent} has no per-skill disable - it reads the Universal folder directly"
        )),
        other => Err(format!("Unknown harness: {other}")),
    }
}

/// Applies a post-install reader switch before a refreshed snapshot exists.
/// The caller supplies the exact Universal deployment and Claude link paths
/// that the completed install selected.
#[allow(clippy::too_many_arguments)]
pub fn set_new_universal_reader_enabled(
    guard: &super::write_lease::WriteLeaseGuard,
    home: &Path,
    data_root: &Path,
    name: &str,
    target: &HarnessVisibilityTarget,
    enabled: bool,
    universal_path: &Path,
    claude_link_path: &Path,
    codex_skill_md_paths: &[PathBuf],
) -> Result<(), String> {
    let agent = target.reader_agent.cli_name();
    if agent == "claude-code" {
        return set_claude_code_enabled(
            guard,
            home,
            name,
            &target.deployment_id,
            claude_link_path,
            universal_path,
            enabled,
        );
    }
    if matches!(agent, "opencode" | "open-code") {
        guard_new_opencode_deployment(home, name, &target.deployment_id)?;
    }
    set_harness_enabled_with(
        home,
        data_root,
        name,
        agent,
        enabled,
        codex_skill_md_paths,
        guard,
    )
}

/// Thin adapter over `skill_studio_core::ops::set_harness_enabled`: the
/// write path (journal-before-first-write, one step per Codex path, the
/// `SymlinkInverse` undo for Claude Code) lives in the core now, the same
/// function the CLI's `set-harness-enabled` subcommand calls. The skill name
/// the core op keys on is read out of `target.deployment_id`'s own encoding
/// (`skill_deployment::parse_deployment_id`) - a pure string parse, not a
/// snapshot rebuild - so this command takes no lock of its own; the core
/// op's `MutationSession` lease is the only one that matters.
#[tauri::command]
pub async fn set_harness_enabled(
    target: HarnessVisibilityTarget,
    enabled: bool,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "set_harness_enabled", move || {
        let refresh_state = app.state::<SkillRefreshState>();
        let parsed = super::skill_deployment::parse_deployment_id(&target.deployment_id)
            .ok_or_else(|| format!("Not a deployment id: {}", target.deployment_id))?;
        let harness =
            skill_studio_core::identity::AgentId::parse_harness(target.reader_agent.cli_name())
                .map_err(|e| e.message)?;
        let rt = super::core_runtime::build_runtime_write()?;
        let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
        let result = ops::set_harness_enabled(
            &rt,
            &ctx,
            &SetHarnessEnabledRequest {
                skill: SkillName(parsed.name),
                harness,
                enabled,
                project_path: parsed.project_path.map(std::path::PathBuf::from),
            },
        );
        let envelope =
            ResultEnvelope::from_result(Operation::SetHarnessEnabled, &rt.scope, &ctx, result);
        let _outcome = super::core_runtime::to_command_result(envelope)?;

        // Surgical: mark the harness's deployments right away; the background
        // loop's full rebuild (skills_dirty) re-derives the true state - which
        // mechanism disabled it, and the symlink Claude Code's removal took.
        let deployment_id = target.deployment_id.clone();
        if let Err(e) = skill_refresh::patch_snapshot_and_emit(&app, &refresh_state, |snapshot| {
            let Some(deployment) = snapshot
                .skills
                .iter_mut()
                .flat_map(|skill| skill.deployments.iter_mut())
                .find(|deployment| deployment.id == deployment_id)
            else {
                return;
            };
            deployment.disabled = !enabled;
            if enabled {
                deployment.disabled_by = None;
            }
        }) {
            eprintln!("[set_harness_enabled] snapshot patch failed: {e}");
        }
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::super::skill_fork_registry::write_fork_registry;
    use super::*;

    fn test_guard(home: &Path) -> super::super::write_lease::WriteLeaseGuard {
        super::super::write_lease::WriteLease::default()
            .try_acquire(home)
            .unwrap()
    }
    use crate::skills::skill_deployment::{deployment_id, SkillDestination};
    use std::fs;

    use super::super::test_support::{pin_opencode_env, write_skill, OpencodeHomeGuard};

    /// Test-only stand-in for the old `opencode_skill_permission::read_denied_patterns(home)`:
    /// resolves the config directory the same way `set_harness_enabled_with`
    /// now does, so a test still reads back the file the "opencode" branch
    /// just wrote.
    fn opencode_denies(home: &Path, name: &str) -> bool {
        let fs = skill_studio_host::RealFs::new();
        let config_dir = skill_studio_host::opencode_config_dir(home);
        skill_studio_core::opencode_config::read_skill_rules(&fs, &config_dir).is_denied(name)
    }

    /// Reads every `path` a Codex `[[skills.config]] enabled = false` row
    /// names, uncanonicalized - matches what `ops::set_codex_skill_disabled`
    /// writes (the raw `skill_md_path` it was given), so a round-trip test
    /// can compare against the path it passed in without going through the
    /// filesystem again.
    fn read_codex_disabled_skill_md_paths_for_test(home: &Path) -> Vec<PathBuf> {
        let path = home.join(".codex").join("config.toml");
        let Ok(content) = fs::read_to_string(&path) else {
            return Vec::new();
        };
        let Ok(table) = content.parse::<toml::Table>() else {
            return Vec::new();
        };
        toml::Value::Table(table)
            .get("skills")
            .and_then(|s| s.get("config"))
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
            .filter(|row| row.get("enabled").and_then(toml::Value::as_bool) == Some(false))
            .filter_map(|row| row.get("path").and_then(toml::Value::as_str))
            .map(PathBuf::from)
            .collect()
    }

    #[test]
    fn codex_disable_and_reenable_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let data_root = home.join(".skill-studio");
        let skill_md = home.join("skills/find-bugs/SKILL.md");
        write_skill(skill_md.parent().unwrap(), "find-bugs");

        set_harness_enabled_with(
            home,
            &data_root,
            "find-bugs",
            "codex",
            false,
            std::slice::from_ref(&skill_md),
            &test_guard(home),
        )
        .unwrap();
        assert_eq!(
            read_codex_disabled_skill_md_paths_for_test(home),
            vec![skill_md.clone()]
        );

        set_harness_enabled_with(
            home,
            &data_root,
            "find-bugs",
            "codex",
            true,
            std::slice::from_ref(&skill_md),
            &test_guard(home),
        )
        .unwrap();
        assert!(read_codex_disabled_skill_md_paths_for_test(home).is_empty());
    }

    /// `codex_disable_runs_under_the_shared_data_root_or_names_the_separate_lease_root`:
    /// a Codex disable's lease/journal artifacts land under the `data_root`
    /// the caller gives it - not a second `home/.skill-studio` tree built
    /// independently of `super::core_runtime::data_root()` - so Codex
    /// disable shares one lock and event store with `park` and every other
    /// desktop mutation. The guard passed in is built the same way the real
    /// `set_harness_enabled` command builds it: rooted at the shared
    /// `data_root`'s `leases` directory, not a lease root the op derives on
    /// its own.
    #[test]
    fn codex_disable_runs_under_the_shared_data_root_or_names_the_separate_lease_root() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let shared_data_root = tmp.path().join("shared-data-root");
        let skill_md = home.join("skills/find-bugs/SKILL.md");
        write_skill(skill_md.parent().unwrap(), "find-bugs");

        let guard =
            super::super::write_lease::WriteLease::with_lease_root(shared_data_root.join("leases"))
                .try_acquire(home)
                .unwrap();
        set_harness_enabled_with(
            home,
            &shared_data_root,
            "find-bugs",
            "codex",
            false,
            std::slice::from_ref(&skill_md),
            &guard,
        )
        .unwrap();

        assert!(
            shared_data_root.join("leases").exists(),
            "the lease root the caller gave it was never used: {}",
            shared_data_root.display()
        );
        assert!(
            !home.join(".skill-studio").exists(),
            "a second, separate lease root was created under home/.skill-studio"
        );
    }

    #[test]
    fn codex_disable_without_a_deployment_path_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        let data_root = tmp.path().join(".skill-studio");
        let err = set_harness_enabled_with(
            tmp.path(),
            &data_root,
            "find-bugs",
            "codex",
            false,
            &[],
            &test_guard(tmp.path()),
        )
        .unwrap_err();
        assert!(err.contains("No Codex-visible deployment"));
    }

    /// `codex_toggle_under_the_desktop_write_lease_succeeds_or_names_the_scope_busy_deadlock`:
    /// `set_harness_enabled`'s command already holds the root's `WriteLease`
    /// before it reaches the Codex arm - see `set_harness_enabled_with`'s
    /// `guard` parameter. Before the fix, the Codex arm acquired a second,
    /// conflicting exclusive lease on the same root instead of reusing the
    /// caller's, so the toggle waited out the lease timeout and failed with
    /// `scope_busy`. Holding the lease here the same way the command does -
    /// then calling through `set_harness_enabled_with` - reproduces that
    /// self-deadlock if the fix regresses.
    #[test]
    fn codex_toggle_under_the_desktop_write_lease_succeeds_or_names_the_scope_busy_deadlock() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let data_root = tmp.path().join("data-root");
        let skill_md = home.join("skills/find-bugs/SKILL.md");
        write_skill(skill_md.parent().unwrap(), "find-bugs");

        let write_lease =
            super::super::write_lease::WriteLease::with_lease_root(data_root.join("leases"));
        let guard = write_lease.try_acquire(&home).unwrap();

        let result = set_harness_enabled_with(
            &home,
            &data_root,
            "find-bugs",
            "codex",
            false,
            std::slice::from_ref(&skill_md),
            &guard,
        );
        assert!(
            result.is_ok(),
            "expected the disable to succeed while the caller holds the root's write \
             lease, not to self-deadlock on a second acquire: {result:?}"
        );

        let disabled_paths = read_codex_disabled_skill_md_paths_for_test(&home);
        assert_eq!(
            disabled_paths,
            vec![skill_md],
            "expected one [[skills.config]] row with enabled = false for the fixture's \
             SKILL.md path in {}",
            home.join(".codex/config.toml").display()
        );
    }

    #[test]
    fn opencode_disable_and_reenable_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let _guard = OpencodeHomeGuard::new(home);
        let data_root = home.join(".skill-studio");

        set_harness_enabled_with(
            home,
            &data_root,
            "find-bugs",
            "opencode",
            false,
            &[],
            &test_guard(home),
        )
        .unwrap();
        assert!(opencode_denies(home, "find-bugs"));

        set_harness_enabled_with(
            home,
            &data_root,
            "find-bugs",
            "opencode",
            true,
            &[],
            &test_guard(home),
        )
        .unwrap();
        assert!(!opencode_denies(home, "find-bugs"));
    }

    /// Flow: the default layout - `XDG_CONFIG_HOME` pinned under `home` by
    /// `OpencodeHomeGuard`, so `config_dir`'s parent (`home/.config`) is a
    /// *different* root than `home` itself, the root the desktop's
    /// `WriteLease` (`test_guard(home)`) is keyed to. Another writer (e.g. a
    /// concurrent CLI run) already holds the exclusive lease scoped to that
    /// exact root - the same root `set_skill_denied`'s own
    /// `home_only_scope` acquires.
    /// Expectation: `set_harness_enabled_with`'s `OpenCode` arm refuses
    /// (busy) rather than writing, because it acquires its own lease on
    /// `config_dir`'s parent instead of only trusting the caller's
    /// `home`-rooted guard.
    /// Failure: the write proceeds anyway - which would mean the desktop's
    /// `home` guard was reused (or coverage skipped) for a root it doesn't
    /// actually cover, racing the other writer.
    #[test]
    fn opencode_disable_refuses_a_writer_already_holding_the_config_dirs_own_lease_or_writes_past_it(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let _opencode_guard = OpencodeHomeGuard::new(home);
        let data_root = home.join(".skill-studio");
        let config_dir = skill_studio_host::opencode_config_dir(home);
        let config_home = config_dir.parent().unwrap();
        std::fs::create_dir_all(config_home).unwrap();

        let ports = skill_studio_core::ports::Ports {
            fs: std::sync::Arc::new(skill_studio_host::RealFs::new()),
            clock: std::sync::Arc::new(skill_studio_core::testing::FakeClock::at(0)),
            ids: std::sync::Arc::new(skill_studio_core::testing::FakeIds::default()),
            leases: std::sync::Arc::new(skill_studio_host::FileLease::new(
                data_root.join("leases"),
            )),
            history: std::sync::Arc::new(skill_studio_core::testing::NoHistory),
            sink: std::sync::Arc::new(skill_studio_core::testing::RecordingSink::default()),
            spawner: None,
            discovery: None,
            tools: None,
            catalog: std::sync::Arc::new(skill_studio_core::harness::HarnessCatalog::builtin()),
        };
        let rt = skill_studio_core::ports::Runtime::new(
            &skill_studio_core::scope::RuntimeScope::fixture(config_home),
            ports,
        )
        .unwrap();
        let other_guard =
            skill_studio_core::ports::acquire_exclusive(rt.ports.leases.as_ref(), &rt.scope)
                .unwrap();

        let err = set_harness_enabled_with(
            home,
            &data_root,
            "find-bugs",
            "opencode",
            false,
            &[],
            &test_guard(home),
        )
        .expect_err(
            "the OpenCode write proceeded despite another writer already holding config_dir's own lease",
        );
        assert!(
            err.contains("holds the lease") || err.contains("busy"),
            "error {err} doesn't look like a lease refusal"
        );
        // The fallback lease used to root itself at `config_home/.leases`,
        // leaving a stray lock directory there on every OpenCode toggle.
        // It now shares `data_root/leases` with every other write, so
        // `config_home` itself must stay untouched by leasing.
        assert!(
            !config_home.join(".leases").exists(),
            "a .leases directory was created under config_home; the fallback lease still isn't rooted at the shared data_root"
        );
        drop(other_guard);
    }

    #[test]
    fn new_project_opencode_disable_refuses_a_global_same_name_deployment() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let _guard = OpencodeHomeGuard::new(&home);
        let project = tmp.path().join("project");
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        let project_skill = project.join(".agents/skills/find-bugs");
        write_skill(&project_skill, "find-bugs");
        let deployment_id = deployment_id(
            "find-bugs",
            "project",
            SkillDestination::Universal,
            "universal",
            project.to_str(),
            &project_skill,
        );
        let target = HarnessVisibilityTarget {
            deployment_id,
            reader_agent: crate::skills::agents::AgentId::OpenCode,
        };

        let error = set_new_universal_reader_enabled(
            &test_guard(&home),
            &home,
            &home.join(".skill-studio"),
            "find-bugs",
            &target,
            false,
            &project_skill,
            &project.join(".claude/skills/find-bugs"),
            &[],
        )
        .unwrap_err();

        assert!(
            error.contains("more than one OpenCode deployment"),
            "{error}"
        );
        assert!(!opencode_denies(&home, "find-bugs"));
        assert!(home.join(".agents/skills/find-bugs/SKILL.md").is_file());
    }

    /// Flow: while `OpencodeHomeGuard` already holds the lock for `home`,
    /// something overwrites `XDG_CONFIG_HOME` to an unrelated real directory
    /// (simulating GitHub's `ubuntu-latest` runner, which exports
    /// `XDG_CONFIG_HOME=/home/runner/.config`, racing in between another
    /// guarded test's pin and its read).
    /// Expectation: re-pinning under the same guard overrides it, so
    /// `opencode_config_dir(home)` resolves under the fixture `home`, not
    /// the unrelated directory.
    /// Failure here would mean every OpenCode-writing test in this module
    /// reads and writes that one shared real directory on CI instead of its
    /// own fixture, racing every other such test.
    #[test]
    fn opencode_desktop_tests_read_the_temp_home_config_under_xdg_config_home_or_names_the_shared_real_directory(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let unrelated = tmp.path().join("unrelated-xdg-config");

        let _guard = OpencodeHomeGuard::new(&home);
        // SAFETY: `_guard` holds `OpencodeHomeGuard`'s lock, serializing
        // every test in this module that touches this var.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &unrelated);
        }
        pin_opencode_env(&home);

        let resolved = skill_studio_host::opencode_config_dir(&home);

        assert_eq!(
            resolved,
            home.join(".config/opencode"),
            "opencode_config_dir resolved the ambient XDG_CONFIG_HOME ({}) instead of the guarded home",
            unrelated.display()
        );
    }

    /// Flow: `SKILL_STUDIO_FIXTURE` is set (a checklist/fixture run) and
    /// `XDG_CONFIG_HOME` points at a real, unrelated `opencode.json` that
    /// already denies `real-file-marker` - the same shape a developer's own
    /// `~/.config/opencode/opencode.json` could take. A skill is then denied
    /// through `skill_refresh::opencode_config_root(home)`.
    /// Expectation: the write lands under the fixture `home`
    /// (`home/.config/opencode/opencode.json`), and the real, unrelated file
    /// under `XDG_CONFIG_HOME` is never read or written - it still denies
    /// only `real-file-marker`, not the skill this test disabled.
    /// Failure: either the write lands under the real `XDG_CONFIG_HOME`
    /// directory instead of the fixture, or the real file's own deny rule
    /// changes - either would mean a fixture/checklist run can touch a
    /// developer's real `OpenCode` config.
    #[test]
    fn fixture_mode_reads_and_writes_opencode_config_under_the_fixture_or_names_the_real_file_it_touched(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".config")).unwrap();
        let real_xdg_config_home = tmp.path().join("real-xdg-config");
        let real_opencode_dir = real_xdg_config_home.join("opencode");
        fs::create_dir_all(&real_opencode_dir).unwrap();
        fs::write(
            real_opencode_dir.join("opencode.json"),
            r#"{"permission": {"skill": {"real-file-marker": "deny"}}}"#,
        )
        .unwrap();

        // `OpencodeHomeGuard` already sets `SKILL_STUDIO_FIXTURE=1` (and
        // restores it on drop); `XDG_CONFIG_HOME` is then pointed at a
        // real, unrelated directory to prove fixture mode ignores it below.
        let _guard = OpencodeHomeGuard::new(&home);
        // SAFETY: `_guard` holds `OpencodeHomeGuard`'s lock, serializing
        // every test in this module that touches these vars.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &real_xdg_config_home);
        }

        let config_dir = skill_refresh::opencode_config_root(&home);
        assert_eq!(
            config_dir,
            skill_studio_host::opencode_config_dir_under(&home),
            "fixture mode resolved a config dir outside the fixture home"
        );
        let fs_port = skill_studio_host::RealFs::new();
        let leases = skill_studio_host::FileLease::new(home.join(".leases"));
        skill_studio_core::opencode_config::set_skill_denied(
            &leases,
            &fs_port,
            &config_dir,
            "epsilon",
            true,
        )
        .unwrap();

        assert!(
            skill_studio_core::opencode_config::read_skill_rules(
                &fs_port,
                &home.join(".config/opencode")
            )
            .is_denied("epsilon"),
            "the deny write did not land under the fixture home"
        );
        let real_rules =
            skill_studio_core::opencode_config::read_skill_rules(&fs_port, &real_opencode_dir);
        assert!(
            real_rules.is_denied("real-file-marker"),
            "the real, unrelated opencode.json under XDG_CONFIG_HOME lost its own rule"
        );
        assert!(
            !real_rules.is_denied("epsilon"),
            "the real, unrelated opencode.json under XDG_CONFIG_HOME was touched"
        );
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

        let universal = home.join(".agents/skills/find-bugs");
        let link = home.join(".claude/skills/find-bugs");
        let deployment_id = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            &universal,
        );
        set_claude_code_enabled(
            &test_guard(home),
            home,
            "find-bugs",
            &deployment_id,
            &link,
            &universal,
            false,
        )
        .unwrap();
        assert!(!home.join(".claude/skills/find-bugs").exists());
        let registry = read_fork_registry(home).unwrap();
        assert_eq!(
            registry.harness_disabled[&format!("deployment/{deployment_id}")]["claude-code"]
                .link_target,
            std::path::PathBuf::from("../../.agents/skills/find-bugs")
        );

        set_claude_code_enabled(
            &test_guard(home),
            home,
            "find-bugs",
            &deployment_id,
            &link,
            &universal,
            true,
        )
        .unwrap();
        assert!(fs::symlink_metadata(home.join(".claude/skills/find-bugs"))
            .unwrap()
            .file_type()
            .is_symlink());
        let registry = read_fork_registry(home).unwrap();
        assert!(!registry
            .harness_disabled
            .contains_key(&format!("deployment/{deployment_id}")));
    }

    #[test]
    fn claude_disable_registry_failure_restores_exact_relative_link_and_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let universal = home.join(".agents/skills/find-bugs");
        write_skill(&universal, "find-bugs");
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        let link = home.join(".claude/skills/find-bugs");
        let raw_target = PathBuf::from("../../.agents/skills/find-bugs");
        std::os::unix::fs::symlink(&raw_target, &link).unwrap();
        let deployment_id = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            &universal,
        );
        let registry = ForkRegistry {
            server_url: Some("https://registry.example.test".to_string()),
            ..ForkRegistry::default()
        };
        write_fork_registry(home, &registry).unwrap();
        let registry_path = crate::skills::skill_fork_registry::fork_registry_path(home);
        let registry_before = fs::read(&registry_path).unwrap();

        let error = set_claude_code_enabled_with_registry_writer(
            home,
            "find-bugs",
            &deployment_id,
            &link,
            &universal,
            false,
            |_| Err("injected registry-write failure".to_string()),
        )
        .unwrap_err();

        assert_eq!(error, "injected registry-write failure");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), raw_target);
        assert_eq!(fs::read(&registry_path).unwrap(), registry_before);
        assert!(read_fork_registry(home)
            .unwrap()
            .harness_disabled
            .is_empty());

        set_claude_code_enabled(
            &test_guard(home),
            home,
            "find-bugs",
            &deployment_id,
            &link,
            &universal,
            true,
        )
        .unwrap();
        assert_eq!(
            fs::read_link(&link).unwrap(),
            Path::new("../../.agents/skills/find-bugs")
        );
    }

    #[test]
    fn claude_disable_reports_manual_recovery_when_link_rollback_is_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let universal = home.join(".agents/skills/find-bugs");
        write_skill(&universal, "find-bugs");
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        let link = home.join(".claude/skills/find-bugs");
        std::os::unix::fs::symlink("../../.agents/skills/find-bugs", &link).unwrap();
        let deployment_id = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            &universal,
        );

        let error = set_claude_code_enabled_with_registry_writer(
            home,
            "find-bugs",
            &deployment_id,
            &link,
            &universal,
            false,
            |_| {
                fs::write(&link, "rollback blocker").unwrap();
                Err("injected registry-write failure".to_string())
            },
        )
        .unwrap_err();

        assert!(error.contains("injected registry-write failure"), "{error}");
        assert!(error.contains("failed to restore"), "{error}");
        assert!(error.contains("Recreate that symlink manually"), "{error}");
        assert_eq!(fs::read_to_string(&link).unwrap(), "rollback blocker");
        assert!(read_fork_registry(home)
            .unwrap()
            .harness_disabled
            .is_empty());
    }

    #[test]
    fn claude_disable_refuses_missing_link_and_regular_file_without_registry_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let universal = home.join(".agents/skills/find-bugs");
        write_skill(&universal, "find-bugs");
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        let link = home.join(".claude/skills/find-bugs");

        let missing_error = set_claude_code_enabled(
            &test_guard(home),
            home,
            "find-bugs",
            "deployment-id",
            &link,
            &universal,
            false,
        )
        .unwrap_err();
        assert!(missing_error.contains("not deployed to Claude Code"));

        fs::write(&link, "user-owned file").unwrap();
        let file_error = set_claude_code_enabled(
            &test_guard(home),
            home,
            "find-bugs",
            "deployment-id",
            &link,
            &universal,
            false,
        )
        .unwrap_err();
        assert!(file_error.contains("not deployed to Claude Code"));
        assert_eq!(fs::read_to_string(&link).unwrap(), "user-owned file");
        assert!(read_fork_registry(home)
            .unwrap()
            .harness_disabled
            .is_empty());
    }

    #[test]
    fn claude_code_disable_refuses_on_whole_dir_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude")).unwrap();
        std::os::unix::fs::symlink(home.join(".agents/skills"), home.join(".claude/skills"))
            .unwrap();

        let err = set_claude_code_enabled(
            &test_guard(home),
            home,
            "find-bugs",
            "dep",
            &home.join(".claude/skills/find-bugs"),
            &home.join(".agents/skills/find-bugs"),
            false,
        )
        .unwrap_err();
        assert!(err.contains("whole Universal folder"));
    }

    #[test]
    fn project_claude_disable_never_touches_same_name_global_link() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let global_skill = home.join(".agents/skills/find-bugs");
        let project_skill = project.join(".agents/skills/find-bugs");
        write_skill(&global_skill, "find-bugs");
        write_skill(&project_skill, "find-bugs");
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        fs::create_dir_all(project.join(".claude/skills")).unwrap();
        let global_link = home.join(".claude/skills/find-bugs");
        let project_link = project.join(".claude/skills/find-bugs");
        std::os::unix::fs::symlink(&global_skill, &global_link).unwrap();
        std::os::unix::fs::symlink(&project_skill, &project_link).unwrap();
        let project_id = deployment_id(
            "find-bugs",
            "project",
            SkillDestination::Universal,
            "universal",
            project.to_str(),
            &project_skill,
        );

        set_claude_code_enabled(
            &test_guard(&home),
            &home,
            "find-bugs",
            &project_id,
            &project_link,
            &project_skill,
            false,
        )
        .unwrap();

        assert!(fs::symlink_metadata(global_link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(fs::symlink_metadata(project_link).is_err());
    }

    /// pi's `settings.json` `skills` exclusion entry format is undocumented
    /// (docs/action-map/harnesses/pi.md, "How the app turns a skill off"),
    /// so this stays the named refusal rather than a silent no-op that
    /// looks like the skill was actually turned off for pi.
    #[test]
    fn pi_disable_without_a_confirmed_exclusion_format_still_returns_the_named_refusal_or_names_the_silent_no_op(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let data_root = tmp.path().join(".skill-studio");
        let err = set_harness_enabled_with(
            tmp.path(),
            &data_root,
            "find-bugs",
            "pi",
            false,
            &[],
            &test_guard(tmp.path()),
        )
        .unwrap_err();
        assert!(
            err.contains("pi has no per-skill disable"),
            "expected the named refusal, not a silent no-op: {err}"
        );
    }

    #[test]
    fn pi_cursor_and_grok_build_refuse() {
        let tmp = tempfile::tempdir().unwrap();
        let data_root = tmp.path().join(".skill-studio");
        for agent in ["pi", "cursor", "grok-build"] {
            let err = set_harness_enabled_with(
                tmp.path(),
                &data_root,
                "find-bugs",
                agent,
                false,
                &[],
                &test_guard(tmp.path()),
            )
            .unwrap_err();
            assert!(err.contains("no per-skill disable"), "{agent}: {err}");
        }
    }
}
