//! `split`: replaces one Universal skill folder with a real copy in each
//! chosen harness's own skills folder.
//!
//! Harnesses the caller does not choose lose the skill: the Universal folder
//! and every per-skill link into it go away. A Codex copy keeps the skill off
//! when Codex's config turned the Universal folder off (its rows are keyed by
//! path). The op writes no other harness config and never
//! touches `.skill-lock.json`, so `npx skills update` keeps pointing at a
//! Universal copy that no longer exists.

use std::path::{Path, PathBuf};

use crate::dto::{SplitCopy, SplitOutcome, SplitRequest};
use crate::error::{CoreError, ErrorCode};
use crate::events::{EventDraft, EventKind, EventStatus};
use crate::identity::{AgentId, BackingRelationship, RootKind, RootScope};
use crate::ops::Operation;
use crate::ports::{FileKind, MutationSession, OpContext, Runtime, ScopeFs};

/// The line every surface shows after a split.
pub const SPLIT_UPDATE_NOTE: &str =
    "npx skills update only updates the Universal copy, so these copies no longer get its updates.";

/// The skills folder `harness` reads in `scope`, or `None` for a harness
/// `split` does not write to.
///
/// Global roots follow the scope's own overrides where the core has one
/// (`codex_home`, `opencode_config_root`). Claude Code and Grok Build use
/// the home default: the core reads no environment variable, and neither
/// `CLAUDE_CONFIG_DIR` nor `GROK_HOME` reaches it through the scope.
pub fn split_target_root(rt: &Runtime, scope: &RootScope, harness: &AgentId) -> Option<PathBuf> {
    let home = &rt.scope.home.lexical;
    match scope {
        RootScope::Global => Some(match harness.as_str() {
            AgentId::CLAUDE_CODE => home.join(".claude").join("skills"),
            AgentId::CODEX => rt.scope.codex_home.join("skills"),
            AgentId::OPEN_CODE => rt
                .scope
                .raw
                .opencode_config_root
                .clone()
                .unwrap_or_else(|| home.join(".config").join("opencode"))
                .join("skills"),
            AgentId::PI => home.join(".pi").join("agent").join("skills"),
            AgentId::CURSOR => home.join(".cursor").join("skills"),
            AgentId::GROK_BUILD => home.join(".grok").join("skills"),
            _ => return None,
        }),
        RootScope::Project(project) => {
            let dir = match harness.as_str() {
                AgentId::CLAUDE_CODE => ".claude",
                AgentId::CODEX => ".codex",
                AgentId::OPEN_CODE => ".opencode",
                AgentId::PI => ".pi",
                AgentId::CURSOR => ".cursor",
                AgentId::GROK_BUILD => ".grok",
                _ => return None,
            };
            Some(project.0.join(dir).join("skills"))
        }
    }
}

/// Splits a Universal deployment into one real copy per chosen harness.
///
/// Preconditions, all checked before the first write: exclusive lease; the
/// deployment resolves exactly once, lives at the Universal root, and holds
/// its own bytes (so parked, plugin, and link deployments are refused);
/// `harnesses` is non-empty and names only harnesses `split` can write to;
/// no chosen harness reads its skills folder through a whole-folder link
/// into the Universal root; and nothing but a link into this Universal
/// folder already sits at a copy's destination.
///
/// Sequence: back up the Universal folder, record the journal row, remove
/// every link into the folder, write each copy, then move the Universal
/// folder into the Universal root's quarantine (pruned by the same caps as
/// `remove`'s). Undo (`restore_event`) writes the Universal folder back from
/// the backup, removes the copies, and recreates the links.
pub fn split(rt: &Runtime, ctx: &OpContext, req: &SplitRequest) -> Result<SplitOutcome, CoreError> {
    rt.run(Operation::Split, ctx, || split_body(rt, ctx, req))
}

fn split_body(
    rt: &Runtime,
    ctx: &OpContext,
    req: &SplitRequest,
) -> Result<SplitOutcome, CoreError> {
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
            "only a Universal skill can be split; parked, plugin, and per-harness skills cannot",
        )
        .at(&deployment.path));
    }
    if deployment.backing != BackingRelationship::Canonical {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "only the Universal folder itself can be split, not a link to it",
        )
        .at(&deployment.path));
    }
    let mut harnesses: Vec<AgentId> = Vec::new();
    for harness in &req.harnesses {
        if !harnesses.contains(harness) {
            harnesses.push(harness.clone());
        }
    }
    if harnesses.is_empty() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "pick at least one harness to keep the skill; to drop it everywhere, use remove",
        ));
    }

    let skill = crate::ops::resolve_skill(&session.fresh, &deployment.id)?.clone();
    let fs = rt.ports.fs.as_ref();
    let universal_root = deployment
        .path
        .parent()
        .ok_or_else(|| {
            CoreError::new(ErrorCode::Io, "a Universal deployment path has no parent")
                .at(&deployment.path)
        })?
        .to_path_buf();
    let canonical_universal_root = fs
        .canonicalize(&universal_root)
        .map_err(|e| CoreError::io(&universal_root, e))?;
    let canonical_skill = fs
        .canonicalize(&deployment.path)
        .map_err(|e| CoreError::io(&deployment.path, e))?;

    let mut links: Vec<PathBuf> = crate::ops::find_all_links(&skill, &deployment.path, fs)
        .into_iter()
        .map(|d| d.path.clone())
        .collect();

    let mut copies: Vec<SplitCopy> = Vec::new();
    for harness in &harnesses {
        let root = split_target_root(rt, &deployment.root.scope, harness).ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                format!("split cannot write a copy for `{}`", harness.as_str()),
            )
        })?;
        refuse_whole_folder_link(fs, harness, &root, &canonical_universal_root)?;
        let target = root.join(&skill.name.0);
        if let Ok(facts) = fs.symlink_metadata(&target) {
            let links_here = facts.kind == FileKind::Symlink
                && fs.canonicalize(&target).ok().as_deref() == Some(canonical_skill.as_path());
            if !links_here {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "{} already exists and is not a link to this Universal skill; move or remove it first",
                        target.display()
                    ),
                )
                .at(&target));
            }
            if !links.contains(&target) {
                links.push(target.clone());
            }
        }
        copies.push(SplitCopy {
            harness: harness.clone(),
            path: target,
        });
    }

    // The copies are written from the backup, which cannot carry a link.
    if let Some(nested) = find_nested_symlink(fs, &deployment.path) {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            format!(
                "{} is a link inside the skill folder; split copies only regular files, so replace the link with a file first",
                nested.display()
            ),
        )
        .at(&nested));
    }

    let link_targets: Vec<(PathBuf, PathBuf)> = links
        .iter()
        .filter_map(|link| fs.read_link(link).ok().map(|target| (link.clone(), target)))
        .collect();
    let scoped_links = links
        .iter()
        .map(|link| crate::ports::confine(&rt.scope, fs, link))
        .collect::<Result<Vec<_>, _>>()?;
    let begin_step = crate::timing::step(clock, "begin_session", step_start);

    let step_start = clock.monotonic();
    let scope_label = crate::ops::scope_label(&deployment.root.scope).to_string();
    let project_path = match &deployment.root.scope {
        RootScope::Global => None,
        RootScope::Project(project) => Some(project.0.clone()),
    };
    let quarantine_dir = universal_root.join(crate::doctor::QUARANTINE_DIR_NAME);
    let id = rt.ports.ids.next_event_id();
    let quarantine_target = quarantine_dir.join(format!("{}-{}", skill.name.0, id.0));

    let manifest =
        session
            .store
            .backup_paths(&session.guard, &id, std::slice::from_ref(&deployment.path))?;
    let backup_entry = manifest.entries.first().ok_or_else(|| {
        CoreError::new(ErrorCode::Io, "the backup manifest has no entry").at(&deployment.path)
    })?;
    let pre_fingerprint = backup_entry.fingerprint.clone().ok_or_else(|| {
        CoreError::new(
            ErrorCode::Io,
            "the Universal folder vanished before its backup",
        )
        .at(&deployment.path)
    })?;
    let files = session
        .store
        .read_backup_files(&manifest.backup_dir, &backup_entry.relative)?;
    // Recorded before the write as the Universal folder's own fingerprint,
    // then replaced by each copy's real one once it is written (a copy has
    // no empty folders, so the two can differ). Undo compares against it to
    // see whether a copy was edited after the split.
    let copy_fingerprints: Vec<(PathBuf, crate::identity::Fingerprint)> = copies
        .iter()
        .map(|copy| (copy.path.clone(), pre_fingerprint.clone()))
        .collect();
    let inverse = crate::events::with_remove_copies(
        crate::events::restore_backup_inverse_with_links(
            &deployment.path,
            Some(&pre_fingerprint),
            None,
            &link_targets,
        ),
        &copy_fingerprints,
    );

    let draft = EventDraft {
        kind: EventKind::Split,
        skill: skill.name.clone(),
        harness: None,
        scope: Some(scope_label),
        project_path,
        payload: serde_json::json!({
            "deployment_id": deployment.id.as_str(),
            "from": deployment.path,
            "to": quarantine_target,
            "copies": copies,
            "links": links,
        }),
        inverse: Some(inverse),
        backup_dir: Some(manifest.backup_dir.clone()),
    };
    session.store.record(&session.guard, &id, &draft)?;

    let write_result = write_split(
        rt,
        &session,
        fs,
        &SplitWrites {
            links: &links,
            scoped_links: &scoped_links,
            copies: &copies,
            files: &files,
            universal: &deployment.path,
            quarantine_dir: &quarantine_dir,
            quarantine_target: &quarantine_target,
        },
    );
    if let Err(e) = write_result {
        roll_back_split(rt, &session, fs, &copies, &link_targets);
        let _ = session
            .store
            .finish(&session.guard, &id, EventStatus::Failed, None);
        return Err(e);
    }
    let written: Vec<(PathBuf, crate::identity::Fingerprint)> = copies
        .iter()
        .filter_map(|copy| {
            crate::events::fingerprint_path(fs, &copy.path)
                .ok()
                .flatten()
                .map(|fingerprint| (copy.path.clone(), fingerprint))
        })
        .collect();
    let patch = crate::events::with_remove_copies(serde_json::json!({}), &written);
    let _ = session.store.patch_inverse(&session.guard, &id, patch);
    session
        .store
        .finish(&session.guard, &id, EventStatus::Done, None)?;
    crate::ops_remove::prune_quarantine(rt, &mut session, fs, &quarantine_dir, &skill.name);
    session.finish(rt, ctx);
    let write_step = crate::timing::step(clock, "split", step_start);
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "split",
        op_start,
        vec![begin_step, write_step],
    ));
    Ok(SplitOutcome {
        event_id: id,
        deployment_id: deployment.id,
        skill: skill.name,
        copies,
        removed_links: links,
        quarantine_path: quarantine_target,
        update_note: SPLIT_UPDATE_NOTE.to_string(),
    })
}

/// Refuses a harness whose skills folder is itself a link into the
/// Universal root: its copy would land inside the folder being split.
fn refuse_whole_folder_link(
    fs: &dyn ScopeFs,
    harness: &AgentId,
    root: &Path,
    canonical_universal_root: &Path,
) -> Result<(), CoreError> {
    let Ok(canonical_root) = fs.canonicalize(root) else {
        return Ok(());
    };
    if canonical_root.starts_with(canonical_universal_root) {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            format!(
                "{} is a link to the Universal folder, so {} has no folder of its own to copy into. \
                 Use \"Convert to per-skill links…\" first. Note: `dotagents sync` may undo that \
                 conversion, because it moves a real skills folder back into .agents/skills.",
                root.display(),
                harness.as_str()
            ),
        )
        .at(root));
    }
    Ok(())
}

struct SplitWrites<'a> {
    links: &'a [PathBuf],
    scoped_links: &'a [crate::ports::ScopedPath],
    copies: &'a [SplitCopy],
    files: &'a [crate::fsops::StageFile],
    universal: &'a Path,
    quarantine_dir: &'a Path,
    quarantine_target: &'a Path,
}

/// Links come down first: a Claude Code copy lands where its link was.
fn write_split(
    rt: &Runtime,
    session: &MutationSession,
    fs: &dyn ScopeFs,
    writes: &SplitWrites<'_>,
) -> Result<(), CoreError> {
    for (link, scoped_link) in writes.links.iter().zip(writes.scoped_links) {
        fs.remove_file(&session.guard, scoped_link)
            .map_err(|e| CoreError::io(link, e))?;
    }
    for copy in writes.copies {
        let root = copy.path.parent().unwrap_or(&copy.path);
        crate::ops::ensure_dir_all(rt, session, fs, root)?;
        crate::ops::restore_write_dir(rt, &session.guard, &copy.path, writes.files)?;
        if copy.harness.as_str() == AgentId::CODEX {
            crate::ops::codex_carry_disabled_row(
                rt,
                &session.guard,
                &writes.universal.join("SKILL.md"),
                &copy.path.join("SKILL.md"),
            )?;
        }
    }
    crate::ops::ensure_dir_all(rt, session, fs, writes.quarantine_dir)?;
    let scoped_from = crate::ports::confine(&rt.scope, fs, writes.universal)?;
    let scoped_to = crate::ports::confine(&rt.scope, fs, writes.quarantine_target)?;
    fs.rename(&session.guard, &scoped_from, &scoped_to)
        .map_err(|e| CoreError::io(writes.universal, e))
}

/// Undoes a split that failed part-way, best-effort: the caller still
/// returns the original error. A copy folder is new (its place held at most
/// a link), so any real folder there is ours to remove.
fn roll_back_split(
    rt: &Runtime,
    session: &MutationSession,
    fs: &dyn ScopeFs,
    copies: &[SplitCopy],
    link_targets: &[(PathBuf, PathBuf)],
) {
    for copy in copies {
        if fs
            .symlink_metadata(&copy.path)
            .is_ok_and(|facts| facts.kind == FileKind::Dir)
        {
            crate::ops_remove::remove_tree_best_effort(fs, &copy.path);
        }
    }
    for (link, target) in link_targets {
        if fs.symlink_metadata(link).is_err() {
            let _ = crate::ops::recreate_link(rt, &session.guard, link, target);
        }
    }
}

fn find_nested_symlink(fs: &dyn ScopeFs, dir: &Path) -> Option<PathBuf> {
    for entry in fs.read_dir(dir).ok()? {
        let child = dir.join(&entry.name);
        match entry.kind {
            FileKind::Symlink => return Some(child),
            FileKind::Dir => {
                if let Some(found) = find_nested_symlink(fs, &child) {
                    return Some(found);
                }
            }
            FileKind::File | FileKind::Other => {}
        }
    }
    None
}
