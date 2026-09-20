//! `ops::update`: refreshes one already-installed skill in place, by
//! [`InstallMethod::Copy`], `Dotagents`, or `SkillsSh` - the same three
//! methods `ops_install` writes, reusing its lease/journal/registry helpers
//! (`crate::ops_install::{journal_root, ensure_journal_root, scope_root,
//! registry_path, read_registry_document, write_registry_document,
//! method_wire_name, copy_deployment_id}`, all made `pub(crate)` for this
//! module - see that module's own doc for what each one does).
//!
//! Every method takes the per-scope exclusive lease
//! ([`crate::ports::MutationSession::begin`]), records an `update` journal
//! row - a backup of the *existing* destination (never "absent", unlike
//! install's: `update` only ever runs over a deployment already on disk) and
//! the `restore_backup` inverse that undoes it - before the first byte
//! moves, exactly the guarantee `docs/action-map/install.md` "Desired
//! state" names and `docs/action-map/remove-and-update.md` "Desired state"
//! extends to update ("record a journal event with a backup and an inverse
//! before the first write").
//!
//! `Copy` re-stages fresh `req.files` beside the destination and swaps them
//! in through [`crate::fsops::swap`], which - since a folder already sits at
//! `final_name` this time - takes its own "exchange, then move the old one
//! into `quarantine_dir`" path, so the previous tree lands in
//! [`crate::doctor::QUARANTINE_DIR_NAME`] - the same folder the doctor
//! prune and check sweep, not an update-specific name - rather than being
//! deleted. [`crate::fsops::swap`]'s `quarantine_dir` is confined under the
//! same [`crate::fsops::Root`] as `stage`/`final_name` (see that function's
//! own doc), so it cannot resolve to the desktop's separate
//! `<home>/.agents/skills-trash` without changing that primitive's contract
//! - the brief for this unit named `skills-trash` as the model location, but
//!   this reuses `fsops::swap`'s own quarantine convention instead of
//!   widening the primitive; see the unit's PR body for that deviation.
//!
//! `Dotagents`/`SkillsSh` re-run the same `npx ... add`/`npx skills update`
//! call the desktop's `skill_lifecycle.rs` shells out to, in place over the
//! existing destination - not staged, for the same reason `ops_install`'s
//! own CLI methods are not: redirecting the CLI into a temporary home to
//! force a stage-and-swap would fight its own layout assumptions (see the
//! shared brief's Correction section, and `ops_install`'s module doc). The
//! journal row's backup of the destination, taken before this call, is what
//! stands in for the "old tree" a crash mid-CLI-call would otherwise lose.

use std::path::{Path, PathBuf};

use crate::dto::{InstallMethod, UpdateAllItem, UpdateAllOutcome, UpdateOutcome, UpdateRequest};
use crate::error::{CoreError, ErrorCode};
use crate::events::{fingerprint_path, EventDraft, EventKind, EventStatus};
use crate::fsops::{self, Root};
use crate::identity::{PlanId, RootScope, SkillName, UNIVERSAL_ROOT_RELATIVE};
use crate::journal::{FsJournal, PlanWriter};
use crate::ops_install;
use crate::ports::{ExclusiveGuard, MutationSession, OpContext, PlanStatus, Runtime, ScopeFs};

/// The `npx` argv `update_via_cli` hands the spawner, and the process cwd to
/// run it in - ported from the desktop's own builders: skills.sh from
/// `skill_lifecycle.rs`'s `skills_sh_update_args` (`npx skills update <name>
/// [--global]`), dotagents from `dotagents_update_args` (`npx -y
/// @sentry/dotagents [--project] add <source> --name <name> [--ref
/// <commit>]`) - both run with the process cwd set to the project path for
/// a project-scope update (`commands.rs`'s `run_update_skill`:
/// `command.current_dir(project_path)`), the same fix `install`'s own
/// `cli_args_and_cwd` carries for a project-scope install (`skills@1.7.0`
/// has neither a `--cwd` nor a `--project` flag; `add`/`update`/`remove`
/// all run in the project directory as the process's own cwd instead).
fn update_cli_args_and_cwd(
    method: InstallMethod,
    skill: &SkillName,
    source: Option<&str>,
    ref_pin: Option<&str>,
    scope: &RootScope,
) -> (Vec<String>, Option<PathBuf>) {
    let cwd = match scope {
        RootScope::Global => None,
        RootScope::Project(project) => Some(project.0.clone()),
    };
    match method {
        InstallMethod::SkillsSh => {
            let mut args = vec!["skills".to_string(), "update".to_string(), skill.0.clone()];
            if matches!(scope, RootScope::Global) {
                args.push("--global".to_string());
            }
            (args, cwd)
        }
        InstallMethod::Dotagents => {
            let mut args = vec!["-y".to_string(), "@sentry/dotagents".to_string()];
            if matches!(scope, RootScope::Project(_)) {
                args.push("--project".to_string());
            }
            args.push("add".to_string());
            args.push(source.unwrap_or_default().to_string());
            args.push("--name".to_string());
            args.push(skill.0.clone());
            if let Some(commit) = ref_pin {
                args.push("--ref".to_string());
                args.push(commit.to_string());
            }
            (args, cwd)
        }
        InstallMethod::Copy => (Vec::new(), None),
    }
}

/// `Dotagents`/`SkillsSh` preconditions (U5): a missing source or a host
/// build with no process spawner - `update` calls this before `backup_paths`
/// records anything, so either failure leaves no journal row, matching
/// `ops_install`'s own validation order.
fn validate_cli_request(rt: &Runtime, req: &UpdateRequest) -> Result<(), CoreError> {
    if req.method == InstallMethod::Dotagents && req.source.is_none() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "a dotagents update needs a source",
        ));
    }
    if matches!(
        req.method,
        InstallMethod::Dotagents | InstallMethod::SkillsSh
    ) && rt.ports.spawner.is_none()
    {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "this host build has no process spawner; dotagents/skills.sh updates are not available",
        ));
    }
    Ok(())
}

/// `Dotagents`/`SkillsSh`: runs `req.method`'s argv (see
/// [`update_cli_args_and_cwd`]) through the process-spawner port and checks
/// the destination still exists afterward. Assumes [`validate_cli_request`]
/// already ran (`update` calls it before the first write).
fn update_via_cli(
    rt: &Runtime,
    ctx: &OpContext,
    req: &UpdateRequest,
    destination: &Path,
) -> Result<(), CoreError> {
    let spawner = rt.ports.spawner.as_ref().ok_or_else(|| {
        CoreError::new(
            ErrorCode::Unsupported,
            "this host build has no process spawner; dotagents/skills.sh updates are not available",
        )
    })?;
    let (args, cwd) = update_cli_args_and_cwd(
        req.method,
        &req.skill,
        req.source.as_deref(),
        req.ref_pin.as_deref(),
        &req.scope,
    );
    let spec = crate::ports::ProcessSpec {
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
    if rt.ports.fs.symlink_metadata(destination).is_err() {
        return Err(
            CoreError::new(ErrorCode::Io, "the CLI removed the expected destination")
                .at(destination),
        );
    }
    Ok(())
}

/// `Copy`: stages `files` under [`ops_install::journal_root`], then swaps it
/// into `<universal_root>/<skill>`, quarantining whatever already sat there
/// - see the module doc for why `QUARANTINE_DIR_NAME`, not the desktop's
///   `skills-trash`.
fn update_copy(
    rt: &Runtime,
    guard: &ExclusiveGuard,
    universal_root: &Path,
    skill: &SkillName,
    files: &[crate::dto::InstallFile],
) -> Result<(), CoreError> {
    let fs = rt.ports.fs.clone();
    ops_install::ensure_journal_root(rt, guard, fs.as_ref())?;
    let journal_root = ops_install::journal_root(&rt.scope.home.lexical);
    let journal = FsJournal::new(journal_root, fs.clone());

    let root = Root::open(fs.as_ref(), universal_root.to_path_buf())
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(universal_root))?;
    let plan_id = PlanId(rt.ports.ids.next_event_id().0);
    let plan = PlanWriter::begin(
        &journal,
        guard,
        plan_id,
        rt.ports.clock.now(),
        format!("update {}", skill.0),
        universal_root.to_path_buf(),
        Vec::new(),
    )
    .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()))?;

    let contents: Vec<(PathBuf, Vec<u8>)> = files
        .iter()
        .map(|f| (f.relative_path.clone(), f.contents.clone()))
        .collect();
    let staged = fsops::stage(&root, &plan, &contents)
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(universal_root))?;
    let final_name = Path::new(&skill.0);
    // Same directory the doctor prune and check sweep, not a
    // update-specific name: a quarantine folder the prune never sees would
    // grow unbounded.
    let quarantine_dir = Path::new(crate::doctor::QUARANTINE_DIR_NAME);
    fsops::swap(&root, &plan, final_name, &staged, quarantine_dir)
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(universal_root))?;
    plan.finish(PlanStatus::Done)
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()))?;
    Ok(())
}

/// `Copy` only: both registry documents `write_copy_registry` will later
/// mutate, read up front - `update` calls this before `backup_paths` (U4),
/// so an unreadable registry fails before the first write, the same
/// ordering `ops_install::install`'s own registry read already uses,
/// instead of after `update_copy` has already swapped the new tree in.
struct CopyRegistryRead {
    root: PathBuf,
    document: serde_json::Map<String, serde_json::Value>,
    home_document: Option<serde_json::Map<String, serde_json::Value>>,
}

fn read_copy_registry(
    rt: &Runtime,
    fs: &dyn ScopeFs,
    scope: &RootScope,
) -> Result<CopyRegistryRead, CoreError> {
    let root = ops_install::scope_root(rt, scope);
    let home_root = rt.scope.home.lexical.clone();
    let document = ops_install::read_registry_document(fs, &root)?;
    let home_document = if root == home_root {
        None
    } else {
        Some(ops_install::read_registry_document(fs, &home_root)?)
    };
    Ok(CopyRegistryRead {
        root,
        document,
        home_document,
    })
}

/// `Copy` only: writes the `content_hash` `update_copy` produced (the same
/// key `ops_install::install_and_link` writes on first install, R1/R2
/// there) into the documents `read_copy_registry` already pulled before the
/// first write - only this write itself has to wait for the swap, since the
/// hash it records depends on the bytes the swap just landed.
fn write_copy_registry(
    session: &mut MutationSession,
    fs: &dyn ScopeFs,
    rt: &Runtime,
    req: &UpdateRequest,
    destination: &Path,
    mut read: CopyRegistryRead,
    content_hash: String,
) -> Result<(), CoreError> {
    let deployment_id = ops_install::copy_deployment_id(&req.scope, &req.skill, destination);
    let home_doc = read.home_document.as_mut().unwrap_or(&mut read.document);
    if let Some(serde_json::Value::Object(copies)) = home_doc.get_mut("copies") {
        if let Some(entry) = copies.get_mut(&deployment_id) {
            entry["content_hash"] = serde_json::Value::String(content_hash);
        }
    }
    if let Some(home_document) = read.home_document {
        ops_install::write_registry_document(
            &session.guard,
            fs,
            &rt.scope.home.lexical,
            home_document,
        )
    } else {
        ops_install::write_registry_document(&session.guard, fs, &read.root, read.document)
    }
}

/// The write step every `update` call shares, once its journal row is
/// already recorded: writes `req.method`'s fresh bytes over the existing
/// destination. Any failure here bubbles up so `update` can mark the row
/// `Failed`, matching `ops_install`'s F9. `copy_registry` is `Some` only for
/// `Copy` - `update` reads it before the first write (U4) and hands it here
/// to be written back once the swap has landed.
fn update_write(
    rt: &Runtime,
    ctx: &OpContext,
    session: &mut MutationSession,
    req: &UpdateRequest,
    universal_root: &Path,
    destination: &Path,
    copy_registry: Option<CopyRegistryRead>,
) -> Result<(), CoreError> {
    let fs = rt.ports.fs.as_ref();
    match req.method {
        InstallMethod::Copy => {
            update_copy(rt, &session.guard, universal_root, &req.skill, &req.files)?;
            let content_hash = crate::ops::skill_content_hash(fs, ctx, destination)?;
            let read = copy_registry.ok_or_else(|| {
                CoreError::new(
                    ErrorCode::Io,
                    "a copy update reached its write step with no pre-read registry documents",
                )
            })?;
            write_copy_registry(session, fs, rt, req, destination, read, content_hash)
        }
        InstallMethod::Dotagents | InstallMethod::SkillsSh => {
            update_via_cli(rt, ctx, req, destination)
        }
    }
}

/// Refreshes one already-installed skill by `req.method`, under the
/// exclusive lease over `req.scope`'s root - see the module doc for the
/// write shape each method takes.
pub fn update(
    rt: &Runtime,
    ctx: &OpContext,
    req: &UpdateRequest,
) -> Result<UpdateOutcome, CoreError> {
    ctx.checkpoint()?;
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
    let step_start = clock.monotonic();
    let session = MutationSession::begin(rt, ctx);
    ctx.take_timing();
    let mut session = session?;
    let begin_step = crate::timing::step(clock, "begin_session", step_start);

    let fs = rt.ports.fs.as_ref();
    let root = ops_install::scope_root(rt, &req.scope);
    let universal_root = root.join(UNIVERSAL_ROOT_RELATIVE);
    let destination = universal_root.join(&req.skill.0);
    if fs.symlink_metadata(&destination).is_err() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "update needs an existing deployment; none exists at this destination",
        )
        .at(&destination));
    }
    let tree_hash_before = crate::tree_hash::tree_hash(fs, &destination)?;

    // U5: both checks run before `backup_paths`, so a missing source or a
    // spawner-less host build leaves no journal row.
    validate_cli_request(rt, req)?;
    // U4: `Copy`'s registry documents are read here too, before the first
    // write, so an unreadable registry fails the same way - see
    // `read_copy_registry`'s own doc.
    let copy_registry = match req.method {
        InstallMethod::Copy => Some(read_copy_registry(rt, fs, &req.scope)?),
        InstallMethod::Dotagents | InstallMethod::SkillsSh => None,
    };

    let step_start = clock.monotonic();
    let id = rt.ports.ids.next_event_id();
    // The row goes down before the first write, same as install's own F7 -
    // this time the backup captures the real tree already on disk (never
    // "absent": `update` refuses above when nothing is there yet), which is
    // what the crash-window test and `ops::restore_event` undo against.
    let manifest =
        session
            .store
            .backup_paths(&session.guard, &id, std::slice::from_ref(&destination))?;
    // `pre` is the backup's own fingerprint of the tree `update` is about to
    // overwrite - never `None` here, since `update` already refused above
    // when the destination did not exist. `None` would tell `restore_event`
    // the path was absent before this event, which would make undo *remove*
    // the restored tree instead of writing it back.
    let pre_fingerprint = manifest
        .entries
        .first()
        .and_then(|e| e.fingerprint.as_ref());
    let inverse = crate::events::restore_backup_inverse(&destination, pre_fingerprint, None);
    let draft = EventDraft {
        kind: EventKind::Update,
        skill: req.skill.clone(),
        harness: None,
        scope: Some(crate::ops::scope_label(&req.scope).to_string()),
        project_path: match &req.scope {
            RootScope::Global => None,
            RootScope::Project(p) => Some(p.0.clone()),
        },
        payload: serde_json::json!({
            "method": ops_install::method_wire_name(req.method),
            "destination": destination,
            "source": req.source,
            "tree_hash_before": tree_hash_before,
        }),
        inverse: Some(inverse),
        backup_dir: Some(manifest.backup_dir.clone()),
    };
    session.store.record(&session.guard, &id, &draft)?;

    if let Err(e) = update_write(
        rt,
        ctx,
        &mut session,
        req,
        &universal_root,
        &destination,
        copy_registry,
    ) {
        let _ = session
            .store
            .finish(&session.guard, &id, EventStatus::Failed, None);
        return Err(e);
    }
    let tree_hash_after = crate::tree_hash::tree_hash(fs, &destination)?;
    // The post-fingerprint the row records, not `None`: `restore_event`
    // compares the live tree against this on undo (`expected =
    // post.unwrap_or("absent")`), so leaving it `None` would tell undo the
    // path was absent after this event and turn a plain restore into
    // `DriftConflict` against the tree `update` just wrote.
    let post_fingerprint = fingerprint_path(fs, &destination)?;
    session
        .store
        .finish(&session.guard, &id, EventStatus::Done, post_fingerprint)?;
    session.finish(rt, ctx);
    let write_step = crate::timing::step(clock, "write", step_start);
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "update",
        op_start,
        vec![begin_step, write_step],
    ));
    Ok(UpdateOutcome {
        event_id: id,
        skill: req.skill.clone(),
        deployment_path: destination,
        tree_hash_before,
        tree_hash_after,
    })
}

/// Runs [`update`] once per entry in `requests`, each its own journal row
/// (`update`'s own lease/journal shape, taken and released per call - no
/// batch-wide lease), calling `on_outcome` as each one finishes so a caller
/// (the desktop's "update all") can update its list in place without
/// waiting for the whole batch - no UI-thread work is this crate's concern;
/// which thread a caller runs this loop on is tested where that caller
/// lives, per this unit's split.
pub fn update_all(
    rt: &Runtime,
    ctx: &OpContext,
    requests: &[UpdateRequest],
    mut on_outcome: impl FnMut(&SkillName, &Result<UpdateOutcome, CoreError>),
) -> UpdateAllOutcome {
    let mut items = Vec::with_capacity(requests.len());
    let mut errors = std::collections::BTreeMap::new();
    for req in requests {
        let result = update(rt, ctx, req);
        on_outcome(&req.skill, &result);
        match result {
            Ok(outcome) => items.push(UpdateAllItem {
                skill: req.skill.clone(),
                outcome: Some(outcome),
            }),
            Err(e) => {
                errors.insert(req.skill.0.clone(), e.message.clone());
                items.push(UpdateAllItem {
                    skill: req.skill.clone(),
                    outcome: None,
                });
            }
        }
    }
    UpdateAllOutcome { items, errors }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::ProjectRef;

    /// `update_cli_args_and_cwd_matches_the_desktop_builders_verbatim_or_names_the_drifted_argv`:
    /// table test over {global, project} x {`SkillsSh`, `Dotagents`} x
    /// {unpinned, pinned `--ref`} - a drift from `skill_lifecycle.rs`'s
    /// `skills_sh_update_args`/`dotagents_update_args` would otherwise go
    /// unnoticed until a real `npx` call failed.
    #[test]
    fn update_cli_args_and_cwd_matches_the_desktop_builders_verbatim_or_names_the_drifted_argv() {
        let skill = SkillName("alpha".to_string());
        let project = RootScope::Project(ProjectRef(PathBuf::from("/proj")));

        type Case<'a> = (
            &'a str,
            InstallMethod,
            &'a RootScope,
            Option<&'a str>,
            Vec<&'a str>,
            Option<PathBuf>,
        );
        let cases: Vec<Case> = vec![
            (
                "skills.sh global",
                InstallMethod::SkillsSh,
                &RootScope::Global,
                None,
                vec!["skills", "update", "alpha", "--global"],
                None,
            ),
            (
                "skills.sh project",
                InstallMethod::SkillsSh,
                &project,
                None,
                vec!["skills", "update", "alpha"],
                Some(PathBuf::from("/proj")),
            ),
            (
                "dotagents global, unpinned",
                InstallMethod::Dotagents,
                &RootScope::Global,
                None,
                vec!["-y", "@sentry/dotagents", "add", "src", "--name", "alpha"],
                None,
            ),
            (
                "dotagents project, pinned",
                InstallMethod::Dotagents,
                &project,
                Some("deadbeef"),
                vec![
                    "-y",
                    "@sentry/dotagents",
                    "--project",
                    "add",
                    "src",
                    "--name",
                    "alpha",
                    "--ref",
                    "deadbeef",
                ],
                Some(PathBuf::from("/proj")),
            ),
        ];

        for (label, method, scope, ref_pin, expected_args, expected_cwd) in cases {
            let (args, cwd) = update_cli_args_and_cwd(method, &skill, Some("src"), ref_pin, scope);
            let expected_args: Vec<String> = expected_args.into_iter().map(String::from).collect();
            assert_eq!(args, expected_args, "{label}: argv");
            assert_eq!(cwd, expected_cwd, "{label}: cwd");
        }
    }
}
