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

use std::fs;
use std::path::{Path, PathBuf};

use super::codex_skill_config;
use super::event_commands::EventStoreState;
use super::event_store::{fingerprint_path, EventDraft, EventStatus, InverseOp};
use super::opencode_skill_permission;
use super::skill_agent_runner::validate_skill_dir_name;
use super::skill_deployment::{BackingRelationship, SkillDestination};
use super::skill_discovery::STUDIO_DISABLED_DIR_NAME;
use super::skill_dto::{DisabledBy, HarnessVisibilityTarget, LifecycleTarget};
use super::skill_fork::ForkMutationLock;
use super::skill_fork_registry::{
    read_fork_registry, write_fork_registry, ClaudeLinkRemoved, CopyDeploymentRecord, ForkRegistry,
};
use super::skill_refresh::{self, SkillRefreshState};

enum ClaudeLinkState {
    PerSkill,
    WholeDir,
    None,
}

fn resolve_native_harness_target<'a>(
    snapshot: &'a super::skill_refresh::SkillSnapshot,
    target: &HarnessVisibilityTarget,
) -> Result<
    (
        &'a super::skill_dto::InstalledSkill,
        &'a super::skill_dto::Deployment,
        PathBuf,
    ),
    String,
> {
    let deployment_id = &target.deployment_id;
    let (skill, deployment) = super::skill_lifecycle::find_deployment(snapshot, deployment_id)?;
    super::skill_lifecycle::revalidate_deployment(deployment, deployment_id)?;
    super::skill_lifecycle::require_direct_deployment_mutable(deployment, "Harness disable")?;
    let agent = target.reader_agent.cli_name();
    let expected_agent = match agent {
        "codex" => "Codex",
        "opencode" | "open-code" => "OpenCode",
        "claude-code" => "Claude Code",
        other => return Err(format!("{other} has no native per-skill disable")),
    };
    let is_universal = deployment.destination == SkillDestination::Universal
        && matches!(deployment.backing, BackingRelationship::Canonical);
    if is_universal && !matches!(deployment.scope.as_str(), "global" | "project") {
        return Err(format!(
            "{expected_agent} cannot read a Universal deployment in {} scope",
            deployment.scope
        ));
    }
    if is_universal && deployment.scope == "project" && deployment.project_path.is_none() {
        return Err(format!(
            "{expected_agent} cannot verify the selected Universal project scope"
        ));
    }
    if !is_universal && deployment.agent != expected_agent {
        return Err(format!(
            "Deployment {deployment_id} belongs to {}, not {expected_agent}",
            deployment.agent
        ));
    }
    let same_visibility_scope = |candidate: &super::skill_dto::Deployment| {
        candidate.scope == deployment.scope && candidate.project_path == deployment.project_path
    };
    if expected_agent == "OpenCode" {
        refuse_opencode_name_collision(
            &deployment.scope,
            deployment.project_path.as_deref(),
            skill.deployments.iter().filter_map(|candidate| {
                (candidate.agent == "OpenCode"
                    || (candidate.destination == SkillDestination::Universal
                        && matches!(candidate.backing, BackingRelationship::Canonical)))
                .then_some((candidate.scope.as_str(), candidate.project_path.as_deref()))
            }),
        )?;
    }
    let adapter_path = if expected_agent == "Claude Code" && is_universal {
        skill
            .deployments
            .iter()
            .find(|candidate| {
                candidate.agent == "Claude Code"
                    && same_visibility_scope(candidate)
                    && matches!(
                        &candidate.backing,
                        BackingRelationship::LinkedTo { deployment_id: backing_id }
                            if backing_id == deployment_id
                    )
            })
            .map(|candidate| PathBuf::from(&candidate.path))
            .ok_or("Claude Code cannot read the selected Universal deployment in this scope")?
    } else {
        PathBuf::from(&deployment.path)
    };
    Ok((skill, deployment, adapter_path))
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
    let mut projects = super::project_discovery::discover_skill_projects(home);
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
fn set_claude_code_enabled(
    home: &Path,
    name: &str,
    deployment_id: &str,
    link_path: &Path,
    expected_target: &Path,
    enabled: bool,
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
        write_fork_registry(home, &registry)?;
        return Ok(());
    }

    match claude_link_state_at(link_path) {
        ClaudeLinkState::WholeDir => Err(
            "Claude Code reads the whole shared folder for skills, not a per-skill symlink - it cannot be disabled for just this skill".to_string(),
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
            fs::remove_file(link_path)
                .map_err(|error| format!("Failed to remove {}: {error}", link_path.display()))?;
            if target.as_os_str().is_empty() {
                return Err(format!(
                    "\"{name}\" is not deployed to Claude Code via a per-skill symlink"
                ));
            }
            registry
                .harness_disabled
                .entry(registry_key)
                .or_default()
                .insert(
                    "claude-code".to_string(),
                    ClaudeLinkRemoved {
                        deployment_id: deployment_id.to_string(),
                        link_target: target,
                    },
                );
            write_fork_registry(home, &registry)
        }
    }
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
        "claude-code" => Err("Claude Code visibility needs an exact deployment target".to_string()),
        "pi" | "cursor" | "grok-build" => Err(format!(
            "{agent} has no per-skill disable - it reads the shared skills folder directly"
        )),
        other => Err(format!("Unknown harness: {other}")),
    }
}

/// Applies a post-install reader switch before a refreshed snapshot exists.
/// The caller supplies the exact Universal deployment and Claude link paths
/// that the completed install selected.
pub fn set_new_universal_reader_enabled(
    home: &Path,
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
    set_harness_enabled_with(home, name, agent, enabled, codex_skill_md_paths)
}

fn move_copy_deployment_and_update_registry(
    registry: &mut ForkRegistry,
    deployment: &super::skill_dto::Deployment,
    enabled: bool,
    write_registry: impl FnOnce(&ForkRegistry) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let old_record = registry
        .copies
        .get(&deployment.id)
        .cloned()
        .ok_or("Deployment disable is not available: Copy ownership record is missing")?;
    let parsed = super::skill_deployment::parse_deployment_id(&deployment.id)
        .ok_or_else(|| format!("Not a deployment id: {}", deployment.id))?;
    let record_scope = match &old_record.scope {
        super::skill_dto::InstallScope::Global => "global",
        super::skill_dto::InstallScope::Project => "project",
    };
    if old_record.deployment_id != deployment.id
        || old_record.name != parsed.name
        || old_record.path.as_path() != Path::new(&deployment.path)
        || old_record.destination != deployment.destination
        || old_record.project_path != deployment.project_path
        || record_scope != deployment.scope
        || parsed.scope != record_scope
        || parsed.slot != old_record.slot
        || parsed.destination != old_record.destination
        || parsed.project_path != old_record.project_path
        || parsed.lexical_path != old_record.path
        || old_record.disabled != enabled
    {
        return Err(
            "Deployment disable is not available: Copy ownership record does not match the selected deployment"
                .to_string(),
        );
    }

    let old_path = PathBuf::from(&deployment.path);
    let new_path = if enabled {
        restore_deployment_at(&old_path)
    } else {
        disable_deployment_at(&old_path)
    }?;
    let new_id = super::skill_deployment::deployment_id(
        &parsed.name,
        &parsed.scope,
        parsed.destination,
        &parsed.slot,
        parsed.project_path.as_deref(),
        &new_path,
    );
    let new_record = CopyDeploymentRecord {
        deployment_id: new_id.clone(),
        path: new_path.clone(),
        disabled: !enabled,
        ..old_record.clone()
    };
    registry.copies.remove(&deployment.id);
    registry.copies.insert(new_id.clone(), new_record);
    if let Err(write_error) = write_registry(registry) {
        registry.copies.remove(&new_id);
        registry
            .copies
            .insert(old_record.deployment_id.clone(), old_record);
        let rollback = if enabled {
            disable_deployment_at(&new_path)
        } else {
            restore_deployment_at(&new_path)
        };
        return match rollback {
            Ok(_) => Err(format!(
                "Failed to update Copy ownership; rolled back the deployment move: {write_error}"
            )),
            Err(rollback_error) => Err(format!(
                "Failed to update Copy ownership ({write_error}) and failed to roll back the deployment move: {rollback_error}"
            )),
        };
    }
    Ok(new_path)
}

#[tauri::command]
pub fn set_harness_enabled(
    target: HarnessVisibilityTarget,
    enabled: bool,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    let agent = target.reader_agent.cli_name();
    let (skill, deployment, adapter_path) = resolve_native_harness_target(&snapshot, &target)?;
    let deployment_id = deployment.id.as_str();
    let expected_agent = match agent {
        "codex" => "Codex",
        "opencode" | "open-code" => "OpenCode",
        "claude-code" => "Claude Code",
        other => return Err(format!("{other} has no native per-skill disable")),
    };
    let codex_skill_md_paths = if expected_agent == "Codex" {
        vec![adapter_path.join("SKILL.md")]
    } else {
        Vec::new()
    };
    let result = if expected_agent == "Claude Code" {
        set_claude_code_enabled(
            &home,
            &skill.name,
            deployment_id,
            &adapter_path,
            Path::new(&deployment.path),
            enabled,
        )
    } else {
        set_harness_enabled_with(&home, &skill.name, agent, enabled, &codex_skill_md_paths)
    };
    if result.is_ok() {
        // Surgical: mark the harness's deployments right away; the background
        // loop's full rebuild (skills_dirty) re-derives the true state - which
        // mechanism disabled it, and the symlink Claude Code's removal took.
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
    target: LifecycleTarget,
    enabled: bool,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
    event_store: tauri::State<EventStoreState>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let deployment_id = target
        .deployment_id
        .as_deref()
        .ok_or("Deployment disable needs one deployment_id")?;
    if target.owner_id.is_some() {
        return Err("Deployment disable targets one deployment, not an owner group".to_string());
    }
    let resolved = super::skill_lifecycle::resolve_fresh_lifecycle_target(
        &app,
        &refresh_state,
        &target,
        "Deployment disable",
    )?;
    let skill = resolved.skill;
    let deployment = resolved.deployment;
    let name = skill.name;
    let path = deployment.path.clone();
    let path_buf = PathBuf::from(&path);
    if !enabled {
        if deployment.agent == "shared" {
            return Err(
                "Cannot disable a Universal root for one harness. Park the skill to disable it for every harness."
                    .to_string(),
            );
        }
        if deployment.plugin.is_some() {
            return Err("Plugin-provided deployments can't be disabled".to_string());
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

    let result = if deployment.owner_kind == super::skill_ownership::LifecycleOwnerKind::Copy {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let mut registry = read_fork_registry(&home)?;
        move_copy_deployment_and_update_registry(&mut registry, &deployment, enabled, |registry| {
            write_fork_registry(&home, registry)
        })
    } else if enabled {
        restore_deployment_at(&path_buf)
    } else {
        disable_deployment_at(&path_buf)
    };

    if let (Some(store), Some((id, original))) = (store, &event) {
        finish_move_aside_event(store, id, original, &result);
    }
    drop(store_guard);

    let new_path = result?;
    let parsed = super::skill_deployment::parse_deployment_id(deployment_id)
        .ok_or_else(|| format!("Not a deployment id: {deployment_id}"))?;
    let new_id = super::skill_deployment::deployment_id(
        &parsed.name,
        &parsed.scope,
        parsed.destination,
        &parsed.slot,
        parsed.project_path.as_deref(),
        &new_path,
    );

    // Surgical: patch the moved deployment's path and disabled state right
    // away; the background loop's full rebuild (skills_dirty) reconciles the
    // rest (frontmatter fields, hashes) moments later.
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
    use crate::skills::frontmatter::InvocationPolicy;
    use crate::skills::provenance::SourceKind;
    use crate::skills::skill_deployment::{
        deployment_id, BackingRelationship, DeploymentMutability, SkillDestination,
    };
    use crate::skills::skill_dto::{Deployment, InstalledSkill};
    use crate::skills::skill_invocations::InvocationHeatmap;
    use crate::skills::skill_ownership::LifecycleOwnerKind;
    use crate::skills::skill_refresh::SkillSnapshot;
    use std::collections::BTreeMap;
    use std::fs;

    fn write_skill(dir: &Path, name: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test\n---\nBody."),
        )
        .unwrap();
    }

    fn native_snapshot(agent: &str, entries: &[(&str, &str, Option<&str>)]) -> SkillSnapshot {
        let deployments = entries
            .iter()
            .map(|(scope, path, project)| {
                let slot = if agent == "Codex" {
                    "codex"
                } else {
                    "opencode"
                };
                Deployment {
                    id: deployment_id(
                        "find-bugs",
                        scope,
                        SkillDestination::PerHarness,
                        slot,
                        *project,
                        Path::new(path),
                    ),
                    destination: SkillDestination::PerHarness,
                    owner_kind: LifecycleOwnerKind::Copy,
                    owner_id: None,
                    mutability: DeploymentMutability::Mutable,
                    backing: BackingRelationship::Independent,
                    agent: agent.to_string(),
                    scope: scope.to_string(),
                    path: path.to_string(),
                    project_path: project.map(str::to_string),
                    invocation: InvocationPolicy::Both,
                    ..Default::default()
                }
            })
            .collect();
        SkillSnapshot {
            skills: vec![InstalledSkill {
                name: "find-bugs".to_string(),
                source: "copy".to_string(),
                source_type: "copy".to_string(),
                source_url: None,
                skill_path: None,
                installed_at: chrono::Utc::now().to_rfc3339(),
                updated_at: None,
                has_update: false,
                update_owner_ids: Vec::new(),
                update_owners: Vec::new(),
                update_commit: None,
                update_commit_at: None,
                source_kind: SourceKind::Manual,
                deployments,
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
                invocation: InvocationPolicy::Both,
            }],
            projects: Vec::new(),
            invocations: Vec::new(),
            heatmap: InvocationHeatmap::default(),
            scanned_at: chrono::Utc::now().to_rfc3339(),
            last_test_by_skill: Default::default(),
            update_check: Default::default(),
            opencode_config_kind: None,
        }
    }

    #[test]
    fn codex_target_resolves_only_the_selected_project_deployment() {
        let snapshot = native_snapshot(
            "Codex",
            &[
                ("global", "/home/.codex/skills/find-bugs", None),
                (
                    "project",
                    "/work/app/.codex/skills/find-bugs",
                    Some("/work/app"),
                ),
            ],
        );
        let selected = &snapshot.skills[0].deployments[1];
        let target = HarnessVisibilityTarget {
            deployment_id: selected.id.clone(),
            reader_agent: crate::skills::agents::AgentId::Codex,
        };
        let (_, resolved, adapter_path) =
            resolve_native_harness_target(&snapshot, &target).unwrap();
        assert_eq!(resolved.path, "/work/app/.codex/skills/find-bugs");
        assert_eq!(adapter_path, Path::new("/work/app/.codex/skills/find-bugs"));
    }

    #[test]
    fn native_harness_mutation_refuses_target_missing_from_fresh_snapshot() {
        let cached = native_snapshot(
            "Codex",
            &[("global", "/home/.codex/skills/find-bugs", None)],
        );
        let target = HarnessVisibilityTarget {
            deployment_id: cached.skills[0].deployments[0].id.clone(),
            reader_agent: crate::skills::agents::AgentId::Codex,
        };
        assert!(resolve_native_harness_target(&cached, &target).is_ok());

        let mut fresh = cached;
        fresh.skills[0].deployments.clear();
        assert!(resolve_native_harness_target(&fresh, &target).is_err());
    }

    #[test]
    fn synthesized_codex_reader_targets_exact_universal_deployment() {
        let mut snapshot = native_snapshot(
            "Codex",
            &[(
                "project",
                "/work/app/.agents/skills/find-bugs",
                Some("/work/app"),
            )],
        );
        let deployment = &mut snapshot.skills[0].deployments[0];
        deployment.id = deployment_id(
            "find-bugs",
            "project",
            SkillDestination::Universal,
            "universal",
            Some("/work/app"),
            Path::new("/work/app/.agents/skills/find-bugs"),
        );
        deployment.destination = SkillDestination::Universal;
        deployment.backing = BackingRelationship::Canonical;
        deployment.agent = "shared".to_string();
        let target = HarnessVisibilityTarget {
            deployment_id: deployment.id.clone(),
            reader_agent: crate::skills::agents::AgentId::Codex,
        };

        let (_, resolved, adapter_path) =
            resolve_native_harness_target(&snapshot, &target).unwrap();

        assert_eq!(resolved.id, target.deployment_id);
        assert_eq!(
            adapter_path,
            Path::new("/work/app/.agents/skills/find-bugs")
        );
    }

    #[test]
    fn synthesized_opencode_reader_accepts_one_universal_scope() {
        let mut snapshot = native_snapshot(
            "OpenCode",
            &[(
                "project",
                "/work/app/.agents/skills/find-bugs",
                Some("/work/app"),
            )],
        );
        let deployment = &mut snapshot.skills[0].deployments[0];
        deployment.id = deployment_id(
            "find-bugs",
            "project",
            SkillDestination::Universal,
            "universal",
            Some("/work/app"),
            Path::new("/work/app/.agents/skills/find-bugs"),
        );
        deployment.destination = SkillDestination::Universal;
        deployment.backing = BackingRelationship::Canonical;
        deployment.agent = "shared".to_string();
        let target = HarnessVisibilityTarget {
            deployment_id: deployment.id.clone(),
            reader_agent: crate::skills::agents::AgentId::OpenCode,
        };

        let (_, resolved, adapter_path) =
            resolve_native_harness_target(&snapshot, &target).unwrap();

        assert_eq!(resolved.id, target.deployment_id);
        assert_eq!(
            adapter_path,
            Path::new("/work/app/.agents/skills/find-bugs")
        );
    }

    #[test]
    fn opencode_name_switch_refuses_same_name_scope_collision() {
        let snapshot = native_snapshot(
            "OpenCode",
            &[
                ("global", "/home/.config/opencode/skills/find-bugs", None),
                (
                    "project",
                    "/work/app/.opencode/skills/find-bugs",
                    Some("/work/app"),
                ),
            ],
        );
        let target = HarnessVisibilityTarget {
            deployment_id: snapshot.skills[0].deployments[0].id.clone(),
            reader_agent: crate::skills::agents::AgentId::OpenCode,
        };
        let error = resolve_native_harness_target(&snapshot, &target).unwrap_err();
        assert!(
            error.contains("more than one OpenCode deployment"),
            "{error}"
        );
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
    fn new_project_opencode_disable_refuses_a_global_same_name_deployment() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
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
            &home,
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
        assert!(opencode_skill_permission::read_denied_patterns(&home).is_empty());
        assert!(home.join(".agents/skills/find-bugs/SKILL.md").is_file());
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
        set_claude_code_enabled(home, "find-bugs", &deployment_id, &link, &universal, false)
            .unwrap();
        assert!(!home.join(".claude/skills/find-bugs").exists());
        let registry = read_fork_registry(home).unwrap();
        assert_eq!(
            registry.harness_disabled[&format!("deployment/{deployment_id}")]["claude-code"]
                .link_target,
            std::path::PathBuf::from("../../.agents/skills/find-bugs")
        );

        set_claude_code_enabled(home, "find-bugs", &deployment_id, &link, &universal, true)
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
    fn claude_code_disable_refuses_on_whole_dir_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".agents/skills/find-bugs"), "find-bugs");
        fs::create_dir_all(home.join(".claude")).unwrap();
        std::os::unix::fs::symlink(home.join(".agents/skills"), home.join(".claude/skills"))
            .unwrap();

        let err = set_claude_code_enabled(
            home,
            "find-bugs",
            "dep",
            &home.join(".claude/skills/find-bugs"),
            &home.join(".agents/skills/find-bugs"),
            false,
        )
        .unwrap_err();
        assert!(err.contains("whole shared folder"));
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

    fn copy_deployment(path: &Path, disabled: bool) -> Deployment {
        let id = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::PerHarness,
            "cursor",
            None,
            path,
        );
        Deployment {
            id,
            destination: SkillDestination::PerHarness,
            owner_kind: LifecycleOwnerKind::Copy,
            mutability: DeploymentMutability::Mutable,
            backing: BackingRelationship::Independent,
            agent: "Cursor".to_string(),
            scope: "global".to_string(),
            path: path.to_string_lossy().to_string(),
            content_hash: crate::skills::skill_discovery::live_skill_content_hash(path).unwrap(),
            disabled,
            disabled_by: disabled.then_some(DisabledBy::StudioMoved),
            ..Default::default()
        }
    }

    fn copy_record(deployment: &Deployment) -> CopyDeploymentRecord {
        CopyDeploymentRecord {
            deployment_id: deployment.id.clone(),
            name: "find-bugs".to_string(),
            path: PathBuf::from(&deployment.path),
            scope: crate::skills::skill_dto::InstallScope::Global,
            destination: SkillDestination::PerHarness,
            slot: "cursor".to_string(),
            project_path: None,
            content_hash: deployment.content_hash.clone(),
            disabled: deployment.disabled,
        }
    }

    #[test]
    fn copy_disable_rebuild_and_reenable_round_trip_preserves_ownership() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let original = home.join(".cursor/skills/find-bugs");
        write_skill(&original, "find-bugs");
        let initial = copy_deployment(&original, false);
        let mut registry = ForkRegistry::default();
        registry
            .copies
            .insert(initial.id.clone(), copy_record(&initial));

        let disabled_path =
            move_copy_deployment_and_update_registry(&mut registry, &initial, false, |registry| {
                write_fork_registry(home, registry)
            })
            .unwrap();
        let candidates = crate::skills::skill_discovery::discover_skill_candidates(home, &[]);
        let candidate = candidates
            .iter()
            .find(|candidate| candidate.path == disabled_path)
            .unwrap();
        assert!(!candidate.content_hash.is_empty());
        assert_eq!(candidate.content_hash, initial.content_hash);
        let (disabled_id, destination, _) = crate::skills::skill_deployment::id_for_candidate(
            crate::skills::skill_deployment::DeploymentCandidate {
                name: &candidate.name,
                root_label: &candidate.root_label,
                scope: &candidate.scope,
                path: &candidate.path,
                project_path: candidate
                    .project_path
                    .as_ref()
                    .and_then(|path| path.to_str()),
                is_symlink: candidate.is_symlink,
                symlink_target: candidate.symlink_target.as_deref(),
                resolved_path: candidate.resolved_path.as_deref(),
                shared_via_whole_dir_link: candidate.shared_via_whole_dir_link,
            },
        );
        let (owner, _, _) = crate::skills::skill_ownership::classify_lifecycle_owner(
            candidate,
            &[],
            destination,
            &disabled_id,
            &read_fork_registry(home).unwrap().copies,
        );
        assert_eq!(owner, LifecycleOwnerKind::Copy);
        assert!(owner.is_mutable());

        let disabled = copy_deployment(&disabled_path, true);
        assert_eq!(disabled.id, disabled_id);
        let restored =
            move_copy_deployment_and_update_registry(&mut registry, &disabled, true, |registry| {
                write_fork_registry(home, registry)
            })
            .unwrap();
        assert_eq!(restored, original);
        assert!(restored.join("SKILL.md").is_file());
        let persisted = read_fork_registry(home).unwrap();
        assert!(persisted.copies.contains_key(&initial.id));
        assert!(!persisted.copies[&initial.id].disabled);
    }

    #[test]
    fn copy_disable_registry_failure_rolls_back_filesystem_and_leaves_snapshot_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let original = tmp.path().join(".cursor/skills/find-bugs");
        write_skill(&original, "find-bugs");
        let deployment = copy_deployment(&original, false);
        let snapshot_identity = (
            deployment.id.clone(),
            deployment.path.clone(),
            deployment.disabled,
        );
        let mut registry = ForkRegistry::default();
        registry
            .copies
            .insert(deployment.id.clone(), copy_record(&deployment));

        let error =
            move_copy_deployment_and_update_registry(&mut registry, &deployment, false, |_| {
                Err("injected registry failure".to_string())
            })
            .unwrap_err();

        assert!(error.contains("rolled back"), "{error}");
        assert!(original.join("SKILL.md").is_file());
        assert!(!original
            .parent()
            .unwrap()
            .join(".skill-studio-disabled/find-bugs")
            .exists());
        assert_eq!(
            (
                deployment.id.clone(),
                deployment.path.clone(),
                deployment.disabled
            ),
            snapshot_identity
        );
        assert!(registry.copies.contains_key(&deployment.id));
    }

    #[test]
    fn move_aside_disable_leaves_same_name_other_scope_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home/.cursor/skills/find-bugs");
        let project = tmp.path().join("project/.cursor/skills/find-bugs");
        write_skill(&global, "find-bugs");
        write_skill(&project, "find-bugs");

        disable_deployment_at(&project).unwrap();

        assert!(global.join("SKILL.md").is_file());
        assert!(!project.exists());
        assert!(project
            .parent()
            .unwrap()
            .join(".skill-studio-disabled/find-bugs/SKILL.md")
            .is_file());
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
