//! `ops::remove`: takes one mutable deployment off disk, for all four owner
//! kinds `LifecycleOwnerKind::is_mutable` allows (`Copy`, `Fork`,
//! `Dotagents`, `SkillsSh`) - `docs/action-map/primitives-and-call-stack.md`'s
//! Remove row.
//!
//! `Copy`/`Fork` hold their own bytes, so this op renames the deployment's
//! directory straight into `<universal_root>/.skill-studio-quarantine/`
//! (never deletes it) - the same direct-`rename` shape `ops::park` already
//! uses to move a deployment out of the universal root, not
//! `fsops::stage`/`swap`: `swap` always *replaces* the final name with a
//! staged folder, and remove has no replacement to put there (see
//! `fsops::swap`'s own doc comment). `Dotagents`/`SkillsSh` call
//! `npx ... remove <name>` through the process-spawner port and let the CLI
//! delete its own files, matching `ops_install`'s `Dotagents`/`SkillsSh`
//! path and the desktop's current behavior (`docs/action-map/remove-and-
//! update.md`'s current-state table: neither variant quarantines today).
//!
//! Every branch records its journal row - with an archival `backup_paths`
//! copy for `Dotagents`/`SkillsSh`, whose bytes this op is about to let the
//! CLI delete - before the first write, matching `ops::park`. The row's
//! `inverse` is `None` for every branch, also matching `park`: the generic
//! `restore_backup`/`read_backup_bytes` machinery restores one file's bytes
//! at a time (`ops::restore_event`'s `RestorePlan::Write` branch), not a
//! whole directory tree, so it cannot undo any of the four branches here
//! any more than it can undo `park`. A dedicated `unremove` op, mirroring
//! `ops::unpark`'s own hand-rolled reversal, is deferred - see the unit's
//! follow-up notes.
//!
//! Quarantine retention: after a `Copy`/`Fork` removal, this op prunes the
//! oldest entries in the same universal root's quarantine directory back
//! down to `doctor::QUARANTINE_RETENTION_CAP`, while still holding the
//! exclusive lease this call already acquired - `doctor::
//! check_quarantine_within_cap` only detects the violation (see that
//! function's own doc: "pruning without a lease ... is not safe to do from
//! here"); this is that repair. The prune itself does not get its own
//! journal row - see the follow-up notes.

use std::path::{Path, PathBuf};

use crate::dto::{RemoveOutcome, RemoveRequest};
use crate::error::{CoreError, ErrorCode};
use crate::events::{EventDraft, EventKind, EventStatus};
use crate::identity::{BackingRelationship, LifecycleOwnerKind, RootKind, RootScope};
use crate::ports::{ExclusiveGuard, MutationSession, OpContext, ProcessSpec, Runtime, ScopeFs};

/// The `npx` package `req`'s owner kind shells out to, or `None` for
/// `Copy`/`Fork` (which never call `npx`) - mirrors `ops_install_cli`'s own
/// `cli_package`.
fn cli_package(owner_kind: LifecycleOwnerKind) -> Option<&'static str> {
    match owner_kind {
        LifecycleOwnerKind::Dotagents => Some("@sentry/dotagents"),
        LifecycleOwnerKind::SkillsSh => Some("skills"),
        _ => None,
    }
}

/// Builds the argv `remove` hands the spawner, and the process cwd to run
/// it in - ported from the desktop's `commands.rs` builders:
/// `skills_sh_remove_args_for_scope` (`npx skills remove <name> --yes
/// [--global]`, cwd unset - the desktop only ever sets `current_dir` for a
/// project, see below) and `dotagents_remove_args` (`npx -y @sentry/dotagents
/// [--project] remove <name>`). Unlike `ops_install_cli::cli_args_and_cwd`,
/// both kinds here get the project path as their process cwd for a project
/// scope - `commands.rs`'s `remove_skill` sets `command.current_dir(path)`
/// whenever `project_path` is `Some`, for both CLI kinds alike.
fn remove_cli_args_and_cwd(
    owner_kind: LifecycleOwnerKind,
    name: &str,
    scope: &RootScope,
) -> (Vec<String>, Option<PathBuf>) {
    let cwd = match scope {
        RootScope::Global => None,
        RootScope::Project(project) => Some(project.0.clone()),
    };
    match owner_kind {
        LifecycleOwnerKind::SkillsSh => {
            let mut args = vec![
                "skills".to_string(),
                "remove".to_string(),
                name.to_string(),
                "--yes".to_string(),
            ];
            if matches!(scope, RootScope::Global) {
                args.push("--global".to_string());
            }
            (args, cwd)
        }
        LifecycleOwnerKind::Dotagents => {
            let mut args = vec!["-y".to_string(), "@sentry/dotagents".to_string()];
            if matches!(scope, RootScope::Project(_)) {
                args.push("--project".to_string());
            }
            args.push("remove".to_string());
            args.push(name.to_string());
            (args, cwd)
        }
        _ => (Vec::new(), None),
    }
}

/// Runs `owner_kind`'s `remove` argv through the spawner port and checks
/// `path` no longer exists - mirrors `ops_install_cli::install_via_cli`'s
/// own post-call check, in reverse.
fn remove_via_cli(
    rt: &Runtime,
    ctx: &OpContext,
    owner_kind: LifecycleOwnerKind,
    name: &str,
    scope: &RootScope,
    path: &Path,
) -> Result<(), CoreError> {
    if cli_package(owner_kind).is_none() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "remove_via_cli is never called for Copy/Fork",
        ));
    }
    let spawner = rt.ports.spawner.as_ref().ok_or_else(|| {
        CoreError::new(
            ErrorCode::Unsupported,
            "this host build has no process spawner; dotagents/skills.sh removal is not available",
        )
    })?;
    let (args, cwd) = remove_cli_args_and_cwd(owner_kind, name, scope);
    let spec = ProcessSpec {
        program: "npx".to_string(),
        args,
        cwd,
        env: Vec::new(),
        timeout_ms: 120_000,
    };
    let output = spawner.run(&spec, ctx.cancel.as_ref())?;
    if output.status != Some(0) {
        return Err(CoreError::new(
            ErrorCode::Io,
            format!("npx exited with {:?}: {}", output.status, output.stderr),
        ));
    }
    if rt.ports.fs.symlink_metadata(path).is_ok() {
        return Err(CoreError::new(
            ErrorCode::Io,
            "the CLI did not remove the expected destination",
        )
        .at(path));
    }
    Ok(())
}

/// Removes one entry from the scope home's registry `copies` map, keyed by
/// deployment id - the write-back half of `ops_install::install_and_link`'s
/// own `Copy` branch, which inserts under the same key.
fn drop_copy_registry_entry(
    rt: &Runtime,
    guard: &ExclusiveGuard,
    fs: &dyn ScopeFs,
    deployment_id: &str,
) -> Result<(), CoreError> {
    let home = rt.scope.home.lexical.clone();
    let mut document = crate::ops_install::read_registry_document(fs, &home)?;
    let changed = document
        .get_mut("copies")
        .and_then(serde_json::Value::as_object_mut)
        .is_some_and(|copies| copies.shift_remove(deployment_id).is_some());
    if changed {
        crate::ops_install::write_registry_document(guard, fs, &home, document)?;
    }
    Ok(())
}

/// The chronological sort key for a quarantine entry named
/// `<skill>-<event-id>`: the `event-id` suffix, whose `ulid` text form sorts
/// chronologically (see `crate::identity::EventId`). Sorting by the suffix
/// rather than the full name matters because the skill name prefix varies
/// entry to entry and would otherwise dominate the comparison - a skill
/// named `alpha` would always look "oldest" next to one named `zeta`,
/// regardless of when each was actually quarantined.
fn quarantine_sort_key(name: &str) -> &str {
    name.rsplit_once('-').map_or(name, |(_, suffix)| suffix)
}

/// Prunes the oldest entries in `quarantine_dir` back down to
/// `doctor::QUARANTINE_RETENTION_CAP`, oldest-first by
/// [`quarantine_sort_key`]. Best-effort: a single entry this cannot remove
/// (for example, a concurrent reader) is left for the next remove's prune
/// rather than failing this one's own result.
fn prune_quarantine(fs: &dyn ScopeFs, quarantine_dir: &Path) {
    let mut entries = fs.read_dir(quarantine_dir).unwrap_or_default();
    if entries.len() <= crate::doctor::QUARANTINE_RETENTION_CAP {
        return;
    }
    entries.sort_by(|a, b| quarantine_sort_key(&a.name).cmp(quarantine_sort_key(&b.name)));
    let excess = entries.len() - crate::doctor::QUARANTINE_RETENTION_CAP;
    for entry in entries.into_iter().take(excess) {
        remove_tree_best_effort(fs, &quarantine_dir.join(&entry.name));
    }
}

/// Deletes `path` and everything under it. `fsops_remove_dir` (like
/// `std::fs::remove_dir`) refuses a non-empty directory, so a quarantined
/// skill's folder needs its own contents removed bottom-up first -
/// best-effort throughout, matching `prune_quarantine`'s own doc: a stray
/// entry this cannot fully clear is left for the next prune.
fn remove_tree_best_effort(fs: &dyn ScopeFs, path: &Path) {
    let Ok(entries) = fs.read_dir(path) else {
        return;
    };
    for entry in entries {
        let child = path.join(&entry.name);
        match entry.kind {
            crate::ports::FileKind::Dir => remove_tree_best_effort(fs, &child),
            crate::ports::FileKind::File
            | crate::ports::FileKind::Symlink
            | crate::ports::FileKind::Other => {
                let _ = fs.fsops_remove_file(&child);
            }
        }
    }
    let _ = fs.fsops_remove_dir(path);
}

/// Removes one deployment, by `deployment.owner_kind` - see the module doc
/// for the write shape each branch takes.
///
/// Preconditions: exclusive lease; the deployment must resolve exactly once,
/// live at the universal root ([`RootKind::Universal`]), hold its own bytes
/// ([`BackingRelationship::Canonical`]), and have a mutable owner kind
/// ([`LifecycleOwnerKind::is_mutable`]).
pub fn remove(
    rt: &Runtime,
    ctx: &OpContext,
    req: &RemoveRequest,
) -> Result<RemoveOutcome, CoreError> {
    ctx.checkpoint()?;
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
    let step_start = clock.monotonic();
    let session = MutationSession::begin(rt, ctx);
    ctx.take_timing();
    let mut session = session?;

    let deployment = session.resolve_exact(&req.deployment_id)?.clone();
    if deployment.root.kind != RootKind::Universal {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "only a universal deployment can be removed",
        )
        .at(&deployment.path));
    }
    if deployment.backing != BackingRelationship::Canonical {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "only the deployment holding the bytes can be removed, not a link",
        )
        .at(&deployment.path));
    }
    if !deployment.owner_kind.is_mutable() {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "this deployment's owner kind does not allow Skill Studio to remove it",
        )
        .at(&deployment.path));
    }
    let skill = crate::ops::resolve_skill(&session.fresh, &deployment.id)?.clone();
    let fs = rt.ports.fs.as_ref();
    let tree_hash_before = crate::tree_hash::tree_hash(fs, &deployment.path)?;
    let claude_link = crate::ops::find_claude_link(&skill, &deployment.path, fs).cloned();
    let begin_step = crate::timing::step(clock, "begin_session", step_start);

    let step_start = clock.monotonic();
    let scope_label = crate::ops::scope_label(&deployment.root.scope).to_string();
    let project_path = match &deployment.root.scope {
        RootScope::Global => None,
        RootScope::Project(project) => Some(project.0.clone()),
    };

    let universal_root = deployment
        .path
        .parent()
        .ok_or_else(|| {
            CoreError::new(ErrorCode::Io, "a universal deployment path has no parent")
                .at(&deployment.path)
        })?
        .to_path_buf();
    let quarantine_dir = universal_root.join(crate::doctor::QUARANTINE_DIR_NAME);

    let id = rt.ports.ids.next_event_id();
    let is_quarantined = matches!(
        deployment.owner_kind,
        LifecycleOwnerKind::Copy | LifecycleOwnerKind::Fork
    );
    let quarantine_target =
        is_quarantined.then(|| quarantine_dir.join(format!("{}-{}", skill.name.0, id.0)));

    let mut backup_dir = None;
    if !is_quarantined {
        // `Dotagents`/`SkillsSh`: the CLI deletes the live bytes itself, so
        // an archival copy is taken before that call, same as `install`
        // takes one of the (normally absent) destination before its first
        // write - see the module doc on why this does not also wire up a
        // `restore_backup` inverse.
        let mut targets = vec![deployment.path.clone()];
        if let Some(link) = &claude_link {
            targets.push(link.path.clone());
        }
        let manifest = session.store.backup_paths(&session.guard, &id, &targets)?;
        backup_dir = Some(manifest.backup_dir);
    }

    let draft = EventDraft {
        kind: EventKind::Remove,
        skill: skill.name.clone(),
        harness: None,
        scope: Some(scope_label),
        project_path,
        payload: serde_json::json!({
            "deployment_id": deployment.id.as_str(),
            "owner_kind": deployment.owner_kind,
            "from": deployment.path,
            "to": quarantine_target,
            "claude_link": claude_link.as_ref().map(|l| &l.path),
        }),
        // See the module doc: no branch here supports a generic
        // `restore_backup` undo, same as `park`.
        inverse: None,
        backup_dir,
    };
    session.store.record(&session.guard, &id, &draft)?;

    let write_result = remove_and_link(
        rt,
        ctx,
        &mut session,
        fs,
        &deployment.path,
        &deployment.id,
        deployment.owner_kind,
        &deployment.root.scope,
        &skill.name.0,
        claude_link.as_ref().map(|l| l.path.as_path()),
        quarantine_target.as_deref(),
    );
    if let Err(e) = write_result {
        let _ = session
            .store
            .finish(&session.guard, &id, EventStatus::Failed, None);
        return Err(e);
    }
    session
        .store
        .finish(&session.guard, &id, EventStatus::Done, None)?;
    if is_quarantined {
        // Still under this call's exclusive lease - see the module doc on
        // why the prune itself carries no journal row.
        prune_quarantine(fs, &quarantine_dir);
    }
    session.finish(rt, ctx);
    let write_step = crate::timing::step(clock, "remove", step_start);
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "remove",
        op_start,
        vec![begin_step, write_step],
    ));
    Ok(RemoveOutcome {
        event_id: id,
        deployment_id: deployment.id,
        skill: skill.name,
        tree_hash_before,
        quarantine_path: quarantine_target,
    })
}

/// The write-and-link step every `remove` call shares, once its journal row
/// is already recorded: removes the Claude Code link (if any), then either
/// renames the tree into quarantine (`Copy`/`Fork`) or runs the CLI's own
/// `remove` (`Dotagents`/`SkillsSh`) - any failure here bubbles up so
/// `remove` can mark the row `Failed`, matching `ops_install::install`'s own
/// `install_and_link`.
#[allow(clippy::too_many_arguments)]
fn remove_and_link(
    rt: &Runtime,
    ctx: &OpContext,
    session: &mut MutationSession,
    fs: &dyn ScopeFs,
    path: &Path,
    deployment_id: &crate::identity::DeploymentId,
    owner_kind: LifecycleOwnerKind,
    scope: &RootScope,
    name: &str,
    claude_link_path: Option<&Path>,
    quarantine_target: Option<&Path>,
) -> Result<(), CoreError> {
    if let Some(link_path) = claude_link_path {
        let scoped_link = crate::ports::confine(&rt.scope, fs, link_path)?;
        fs.remove_file(&session.guard, &scoped_link)
            .map_err(|e| CoreError::io(link_path, e))?;
    }
    match owner_kind {
        LifecycleOwnerKind::Copy | LifecycleOwnerKind::Fork => {
            let quarantine_target = quarantine_target.ok_or_else(|| {
                CoreError::new(ErrorCode::Io, "a quarantined removal always has a target")
            })?;
            crate::ops::ensure_dir_all(
                rt,
                session,
                fs,
                quarantine_target.parent().unwrap_or(quarantine_target),
            )?;
            let scoped_from = crate::ports::confine(&rt.scope, fs, path)?;
            let scoped_to = crate::ports::confine(&rt.scope, fs, quarantine_target)?;
            fs.rename(&session.guard, &scoped_from, &scoped_to)
                .map_err(|e| CoreError::io(path, e))?;
            if owner_kind == LifecycleOwnerKind::Copy {
                drop_copy_registry_entry(rt, &session.guard, fs, deployment_id.as_str())?;
            }
            // `Fork`'s own registry row (a separate ledger from `Copy`'s
            // `copies` map) is left in place - see the unit's follow-up
            // notes.
            Ok(())
        }
        LifecycleOwnerKind::Dotagents | LifecycleOwnerKind::SkillsSh => {
            remove_via_cli(rt, ctx, owner_kind, name, scope, path)
        }
        _ => Err(CoreError::new(
            ErrorCode::Unsupported,
            "this owner kind is not mutable and was already refused before this step",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::ProjectRef;

    /// `remove_cli_args_and_cwd_matches_the_desktop_builders_verbatim_or_names_the_drifted_argv`:
    /// table test over {global, project} x {`SkillsSh`, `Dotagents`} -
    /// mirrors `ops_install_cli`'s own drift check, this time against
    /// `commands.rs`'s `skills_sh_remove_args_for_scope` and
    /// `dotagents_remove_args`.
    #[test]
    fn remove_cli_args_and_cwd_matches_the_desktop_builders_verbatim_or_names_the_drifted_argv() {
        let project = RootScope::Project(ProjectRef(PathBuf::from("/proj")));

        // This drift check's own row shape, not a domain type anything else
        // needs - named here purely to satisfy clippy's `type_complexity`,
        // matching `ops_install_cli`'s own `Case` alias.
        type Case<'a> = (
            &'a str,
            LifecycleOwnerKind,
            &'a RootScope,
            Vec<&'a str>,
            Option<PathBuf>,
        );
        let cases: Vec<Case> = vec![
            (
                "skills.sh global",
                LifecycleOwnerKind::SkillsSh,
                &RootScope::Global,
                vec!["skills", "remove", "alpha", "--yes", "--global"],
                None,
            ),
            (
                "skills.sh project",
                LifecycleOwnerKind::SkillsSh,
                &project,
                vec!["skills", "remove", "alpha", "--yes"],
                Some(PathBuf::from("/proj")),
            ),
            (
                "dotagents global",
                LifecycleOwnerKind::Dotagents,
                &RootScope::Global,
                vec!["-y", "@sentry/dotagents", "remove", "alpha"],
                None,
            ),
            (
                "dotagents project",
                LifecycleOwnerKind::Dotagents,
                &project,
                vec!["-y", "@sentry/dotagents", "--project", "remove", "alpha"],
                Some(PathBuf::from("/proj")),
            ),
        ];
        for (label, owner_kind, scope, expected_args, expected_cwd) in cases {
            let (args, cwd) = remove_cli_args_and_cwd(owner_kind, "alpha", scope);
            let expected_args: Vec<String> = expected_args.into_iter().map(String::from).collect();
            assert_eq!(args, expected_args, "{label}: argv");
            assert_eq!(cwd, expected_cwd, "{label}: cwd");
        }
    }
}
