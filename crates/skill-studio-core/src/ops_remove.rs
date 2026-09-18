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
//! copy of the deployment's tree - before the first write, matching
//! `ops::park`. Unlike `park`, this op's `inverse` is a real
//! `restore_backup_inverse` for every branch: `BackupEntry::is_dir` lets the
//! generic `restore_event`/`RestorePlan::WriteDir` machinery replay a whole
//! directory tree back to its pre-remove bytes (see that field's own doc),
//! so a dedicated `unremove` op is not needed here the way `ops::unpark`
//! needed one.
//!
//! Quarantine retention: after a `Copy`/`Fork` removal, this op prunes the
//! oldest and the age-expired entries in the same universal root's
//! quarantine directory (see `prune_quarantine`'s own doc for both caps),
//! while still holding the exclusive lease this call already acquired -
//! `doctor::check_quarantine_within_cap` only detects the violation (see
//! that function's own doc: "pruning without a lease ... is not safe to do
//! from here"); this is that repair. Each prune that deletes at least one
//! entry gets its own `quarantine_prune` journal row.

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
    drop_registry_entry(rt, guard, fs, "copies", deployment_id)
}

/// Removes one entry from the scope home's registry `forks` map, keyed by
/// skill name (`ownership::HomeRegistry::forks`'s own key) - the write-back
/// half of whatever recorded the fork, mirroring [`drop_copy_registry_entry`]
/// for the other lifecycle owner kind that keeps its own registry row.
fn drop_fork_registry_entry(
    rt: &Runtime,
    guard: &ExclusiveGuard,
    fs: &dyn ScopeFs,
    name: &str,
) -> Result<(), CoreError> {
    drop_registry_entry(rt, guard, fs, "forks", name)
}

fn drop_registry_entry(
    rt: &Runtime,
    guard: &ExclusiveGuard,
    fs: &dyn ScopeFs,
    map_key: &str,
    entry_key: &str,
) -> Result<(), CoreError> {
    let home = rt.scope.home.lexical.clone();
    let mut document = crate::ops_install::read_registry_document(fs, &home)?;
    let changed = document
        .get_mut(map_key)
        .and_then(serde_json::Value::as_object_mut)
        .is_some_and(|map| map.shift_remove(entry_key).is_some());
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

/// Age cap for a quarantine entry, alongside the count cap
/// (`doctor::QUARANTINE_RETENTION_CAP`): an entry older than this is pruned
/// even while the directory is under the count cap, so an idle install does
/// not carry a removed tree forever. Unmeasured against production
/// quarantine growth, like the count cap itself (its own doc).
const QUARANTINE_AGE_CAP: chrono::Duration = chrono::Duration::days(30);

/// Whether the quarantine entry whose [`quarantine_sort_key`] is `event_id`
/// is still referenced by a `remove` row that has not reached
/// [`EventStatus::Done`] - pruning it out from under a `Pending` or `Failed`
/// remove would delete the very backup that row's own undo (or a retry)
/// still needs. A row this cannot find (already gone, or never written) is
/// not "open", so it does not block the prune.
///
/// This exemption holds regardless of either cap in [`prune_quarantine`]:
/// an entry whose own `remove` row stays `Failed` is exempt from both the
/// count cap and [`QUARANTINE_AGE_CAP`] until that row is restored (through
/// `ops::restore_event`) or its remove is retried and reaches `Done` -
/// `quarantine_prune_keeps_the_entry_of_a_failed_remove_or_names_the_lost_entry`
/// covers the count cap; the age cap shares this same check
/// (`issue-3.9a-followup-a.md` tracks adding the age-cap counterpart).
fn is_referenced_by_open_remove(session: &MutationSession, event_id: &str) -> bool {
    let Ok(Some(record)) = session
        .store
        .get(&crate::identity::EventId(event_id.to_string()))
    else {
        return false;
    };
    record.kind == EventKind::Remove.as_str() && record.status != EventStatus::Done
}

/// Prunes `quarantine_dir` down to `doctor::QUARANTINE_RETENTION_CAP`
/// entries and drops anything older than [`QUARANTINE_AGE_CAP`], oldest-
/// first by [`quarantine_sort_key`] - except an entry still referenced by an
/// open `remove` row (see [`is_referenced_by_open_remove`]), which is kept
/// regardless of age or cap. Records one `quarantine_prune` journal row
/// naming every entry it actually deleted, when it deletes at least one -
/// still under `remove`'s own exclusive lease. Best-effort on the deletes
/// themselves: a single entry this cannot remove (for example, a concurrent
/// reader) is left for the next remove's prune rather than failing this
/// one's own result.
fn prune_quarantine(
    rt: &Runtime,
    session: &mut MutationSession,
    fs: &dyn ScopeFs,
    quarantine_dir: &Path,
    triggering_skill: &crate::identity::SkillName,
) {
    let mut entries = fs.read_dir(quarantine_dir).unwrap_or_default();
    entries.retain(|e| !is_referenced_by_open_remove(session, quarantine_sort_key(&e.name)));
    entries.sort_by(|a, b| quarantine_sort_key(&a.name).cmp(quarantine_sort_key(&b.name)));

    let now = rt.ports.clock.now();
    let mut to_prune: Vec<String> = Vec::new();
    let mut kept = Vec::new();
    for entry in entries {
        let expired = ulid::Ulid::from_string(quarantine_sort_key(&entry.name))
            .ok()
            .is_some_and(|ulid| {
                let ts = chrono::DateTime::<chrono::Utc>::from(
                    std::time::UNIX_EPOCH + std::time::Duration::from_millis(ulid.timestamp_ms()),
                );
                now.signed_duration_since(ts) > QUARANTINE_AGE_CAP
            });
        if expired {
            to_prune.push(entry.name);
        } else {
            kept.push(entry);
        }
    }
    if kept.len() > crate::doctor::QUARANTINE_RETENTION_CAP {
        let excess = kept.len() - crate::doctor::QUARANTINE_RETENTION_CAP;
        to_prune.extend(kept.into_iter().take(excess).map(|e| e.name));
    }
    if to_prune.is_empty() {
        return;
    }
    for name in &to_prune {
        remove_tree_best_effort(fs, &quarantine_dir.join(name));
    }
    let id = rt.ports.ids.next_event_id();
    let draft = EventDraft {
        kind: EventKind::QuarantinePrune,
        skill: triggering_skill.clone(),
        harness: None,
        scope: None,
        project_path: None,
        payload: serde_json::json!({ "pruned": to_prune }),
        inverse: None,
        backup_dir: None,
    };
    if session.store.record(&session.guard, &id, &draft).is_ok() {
        let _ = session
            .store
            .finish(&session.guard, &id, EventStatus::Done, None);
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

/// [`remove_and_link`]'s arguments, grouped into one struct so the function
/// itself does not need `#[allow(clippy::too_many_arguments)]`.
struct RemoveAndLinkArgs<'a> {
    path: &'a Path,
    deployment_id: &'a crate::identity::DeploymentId,
    owner_kind: LifecycleOwnerKind,
    scope: &'a RootScope,
    name: &'a str,
    link_paths: &'a [PathBuf],
    quarantine_target: Option<&'a Path>,
}

/// Removes one deployment, by `deployment.owner_kind` - see the module doc
/// for the write shape each branch takes. See [`remove_and_link`] for why
/// this needs an ordered write, not just a call.
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
    // Every harness's link, not just Claude Code's - `RemoveRequest` has no
    // `harnesses` field to restrict this to (see `dto::RemoveRequest`), so
    // every link found under the deployment's own tree is dropped.
    let links: Vec<PathBuf> = crate::ops::find_all_links(&skill, &deployment.path, fs)
        .into_iter()
        .map(|d| d.path.clone())
        .collect();
    // Each link's own target, read before anything moves, so the inverse
    // below can recreate it verbatim - the CLI kinds' own links target
    // `deployment.path` itself, about to be renamed away or deleted, so
    // reading the target after the write would find nothing to read.
    let link_targets: Vec<(PathBuf, PathBuf)> = links
        .iter()
        .filter_map(|link| fs.read_link(link).ok().map(|target| (link.clone(), target)))
        .collect();
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

    // Every owner kind gets a real archival copy of the tree before the
    // first write - see the module doc on why this also wires up a real
    // `restore_backup_inverse`, unlike `park`. `deployment.path` is listed
    // first so its manifest entry (and thus `pre_fingerprint` below) is
    // `manifest.entries[0]` regardless of what else this backs up.
    // `Copy`/`Fork` also get their own registry.json backed up in the same
    // manifest, since removing either drops a row from it
    // (`drop_copy_registry_entry`/`drop_fork_registry_entry` below) that a
    // restore should bring back, not just the tree's bytes -
    // `restore_event` replays every entry in a manifest, not only the one
    // matching its primary `path`, for exactly this reason. Each harness
    // link this removes goes in `inverse.links` instead (see
    // `restore_backup_inverse_with_links`'s own doc): a symlink copied into
    // a backup manifest would restore as a plain file, not a link.
    let mut backup_targets = vec![deployment.path.clone()];
    if matches!(
        deployment.owner_kind,
        LifecycleOwnerKind::Copy | LifecycleOwnerKind::Fork
    ) {
        backup_targets.push(crate::ops_install::registry_path(&rt.scope.home.lexical));
    }
    let manifest = session
        .store
        .backup_paths(&session.guard, &id, &backup_targets)?;
    let pre_fingerprint = manifest
        .entries
        .first()
        .and_then(|e| e.fingerprint.as_ref());
    let inverse = crate::events::restore_backup_inverse_with_links(
        &deployment.path,
        pre_fingerprint,
        None,
        &link_targets,
    );
    let backup_dir = Some(manifest.backup_dir);

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
            "links": links,
        }),
        inverse: Some(inverse),
        backup_dir,
    };
    session.store.record(&session.guard, &id, &draft)?;

    let write_result = remove_and_link(
        rt,
        ctx,
        &mut session,
        fs,
        &RemoveAndLinkArgs {
            path: &deployment.path,
            deployment_id: &deployment.id,
            owner_kind: deployment.owner_kind,
            scope: &deployment.root.scope,
            name: &skill.name.0,
            link_paths: &links,
            quarantine_target: quarantine_target.as_deref(),
        },
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
        // Still under this call's exclusive lease - see `prune_quarantine`'s
        // own doc.
        prune_quarantine(rt, &mut session, fs, &quarantine_dir, &skill.name);
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
/// is already recorded: renames the tree into quarantine (`Copy`/`Fork`) or
/// runs the CLI's own `remove` (`Dotagents`/`SkillsSh`) FIRST, then removes
/// every harness's link. The tree op runs first, not last as an earlier
/// revision had it, so a failed link removal never leaves the tree gone but
/// the journal row (and its `restore_backup` inverse) pointing at a path
/// whose links were already dropped out from under it - and so a failure
/// partway through link cleanup, after the tree op already succeeded, still
/// leaves `remove` free to mark the row `Done`: the deployment itself is
/// gone either way, which is what the row records.
fn remove_and_link(
    rt: &Runtime,
    ctx: &OpContext,
    session: &mut MutationSession,
    fs: &dyn ScopeFs,
    args: &RemoveAndLinkArgs<'_>,
) -> Result<(), CoreError> {
    match args.owner_kind {
        LifecycleOwnerKind::Copy | LifecycleOwnerKind::Fork => {
            let quarantine_target = args.quarantine_target.ok_or_else(|| {
                CoreError::new(ErrorCode::Io, "a quarantined removal always has a target")
            })?;
            crate::ops::ensure_dir_all(
                rt,
                session,
                fs,
                quarantine_target.parent().unwrap_or(quarantine_target),
            )?;
            let scoped_from = crate::ports::confine(&rt.scope, fs, args.path)?;
            let scoped_to = crate::ports::confine(&rt.scope, fs, quarantine_target)?;
            fs.rename(&session.guard, &scoped_from, &scoped_to)
                .map_err(|e| CoreError::io(args.path, e))?;
            match args.owner_kind {
                LifecycleOwnerKind::Copy => {
                    drop_copy_registry_entry(rt, &session.guard, fs, args.deployment_id.as_str())?;
                }
                LifecycleOwnerKind::Fork => {
                    drop_fork_registry_entry(rt, &session.guard, fs, args.name)?;
                }
                _ => unreachable!("matched above"),
            }
        }
        LifecycleOwnerKind::Dotagents | LifecycleOwnerKind::SkillsSh => {
            remove_via_cli(rt, ctx, args.owner_kind, args.name, args.scope, args.path)?;
        }
        _ => {
            return Err(CoreError::new(
                ErrorCode::Unsupported,
                "this owner kind is not mutable and was already refused before this step",
            ))
        }
    }
    for link_path in args.link_paths {
        // The real `npx skills remove`/`npx -y @sentry/dotagents remove`
        // (no `--agent` given) already deletes every per-agent link itself
        // before this op ever reaches its own link loop (skills CLI v1.5.23
        // `dist/cli.mjs:6217-6263`) - so for `Dotagents`/`SkillsSh` a link
        // this finds already gone is the expected, successful case, not a
        // crash partway through. `Copy`/`Fork` links are never touched by
        // any CLI, so for them a missing link would instead mean this same
        // op already ran once for this deployment; treating it as removed
        // either way keeps `remove` idempotent rather than failing a row
        // whose deployment, links, and lock entry are already gone. Only
        // `NotFound` gets this tolerance: any other `symlink_metadata` error
        // (a `PermissionDenied` on an unreadable parent, for example) means
        // this cannot tell whether the link is actually gone, so it fails
        // the row rather than reporting `Done` with a link still on disk.
        match fs.symlink_metadata(link_path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(CoreError::io(link_path, e)),
            Ok(_) => {}
        }
        let scoped_link = crate::ports::confine(&rt.scope, fs, link_path)?;
        fs.remove_file(&session.guard, &scoped_link)
            .map_err(|e| CoreError::io(link_path, e))?;
    }
    Ok(())
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
