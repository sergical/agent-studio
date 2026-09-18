//! Journal: the crash-safety primitive every `fsops` call records against.
//!
//! A plan's manifest and the plan itself are fsynced to disk, through
//! [`Journal::begin`], before the plan's first step runs - see
//! `docs/action-map/plan.md` unit 1.2. [`FsJournal`] is the reference
//! [`Journal`] implementation, built only on [`ScopeFs`] so a host can reuse
//! it verbatim rather than reimplementing the write order. [`reconcile`]
//! resolves every plan a crash left `Pending` by undoing its recorded steps
//! in reverse - or, for a plan with none, marking it `Failed` since nothing
//! mutated anything yet. [`trim_backups`] enforces a size-and-age quota on
//! the backups plans keep for undo. Neither one ever deletes a plan row.
//!
//! [`PlanWriter`]'s `record_*` methods are how `crate::fsops`'s four
//! primitives themselves record the step that ran - see that module - so a
//! caller cannot call a primitive without a plan step landing for it. Each
//! `record_*` call happens *before* its primitive's own mutation, with
//! everything reversal will need - the quarantine path, the previous link
//! target, the pre-write backup - already computed and durable: `stage`
//! records its temp path before creating it, `swap` records the exchange's
//! quarantine destination before running it, `link` records the previous
//! target before renaming the new one into place, and `write_file` fsyncs
//! its backup before renaming the new bytes into place. A crash can now only
//! ever land between "step recorded" and "mutation done", never the other
//! way around, so [`reverse_steps`] inspects the disk for each step to tell
//! whether its mutation actually landed and reverses only what did -
//! idempotent, since a step whose mutation never landed is already a no-op
//! to reverse.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::error::{CoreError, ErrorCode};
use crate::fsops::{self, Root};
use crate::identity::PlanId;
use crate::ports::{
    ExclusiveGuard, FileKind, Journal, PlanBackupEntry, PlanRecord, PlanStatus, PlanStep, ScopeFs,
};

/// Caps how many bytes [`FsJournal`] will read back for one plan or manifest
/// file; a journal file this large is corrupt, not merely large.
const MAX_JOURNAL_JSON_BYTES: u64 = 16 * 1024 * 1024;

/// Caps how many bytes [`FsJournal::read_backup`] will read back for one
/// `write_file` backup. `fsops::write_file` reads the pre-write bytes it
/// hands to [`Journal::write_backup`] with `u64::MAX` - a user's file is
/// not a journal-corruption signal the way an oversized plan/manifest JSON
/// file is - so readback matches that instead of reusing
/// `MAX_JOURNAL_JSON_BYTES`: a backup larger than that cap must still be
/// restorable, not leave the plan `Interrupted` with the post-write bytes
/// stuck in place.
const MAX_BACKUP_BYTES: u64 = u64::MAX;

/// A [`Journal`] implementation built only on [`ScopeFs`], so both the core's
/// own tests and a host adapter can use it directly.
///
/// Layout under `root`: `plans/<id>/manifest.json` (the plan's
/// [`PlanBackupEntry`] list, written first), `plans/<id>/plan.json` (the
/// full [`PlanRecord`], written second), and `plans/<id>/backups/<relative>`
/// for the backed-up bytes [`Self::remove_backup`] trims.
pub struct FsJournal {
    root: PathBuf,
    fs: Arc<dyn ScopeFs>,
}

impl FsJournal {
    /// Roots the journal at `root`, reading and writing through `fs`.
    pub fn new(root: PathBuf, fs: Arc<dyn ScopeFs>) -> Self {
        FsJournal { root, fs }
    }

    fn plan_dir(&self, id: &PlanId) -> PathBuf {
        self.root.join("plans").join(&id.0)
    }

    fn manifest_path(&self, id: &PlanId) -> PathBuf {
        self.plan_dir(id).join("manifest.json")
    }

    fn plan_path(&self, id: &PlanId) -> PathBuf {
        self.plan_dir(id).join("plan.json")
    }

    fn backup_path(&self, id: &PlanId, relative: &str) -> PathBuf {
        self.plan_dir(id).join("backups").join(relative)
    }

    /// Creates `path` and every missing ancestor under `root`, fsyncing each
    /// one it creates. A no-op when `path` already exists.
    fn ensure_dir(&self, path: &Path) -> std::io::Result<()> {
        if self.fs.symlink_metadata(path).is_ok() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            if parent != path {
                self.ensure_dir(parent)?;
            }
        }
        match self.fs.fsops_create_dir(path) {
            Ok(()) => self.fs.fsops_fsync_dir(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Writes `bytes` to `path` through `tmp`, fsync, and rename, so `path`
    /// only ever shows a complete write - the same durability
    /// `fsops::write_file` gives a caller's own files.
    fn write_through_tmp(&self, path: &Path, tmp: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let parent = path.parent().unwrap_or(path);
        self.ensure_dir(parent)?;
        if self.fs.symlink_metadata(tmp).is_ok() {
            self.fs.fsops_remove_file(tmp)?;
        }
        self.fs.fsops_write_new_file(tmp, bytes)?;
        self.fs.fsops_fsync_file(tmp)?;
        self.fs.fsops_rename(tmp, path)?;
        self.fs.fsops_fsync_dir(parent)
    }

    /// Writes JSON `bytes` to `path` durably - see [`Self::write_through_tmp`].
    fn write_json(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        self.write_through_tmp(path, &path.with_extension("json.tmp"), bytes)
    }

    /// Writes raw `bytes` to `path` durably - see [`Self::write_through_tmp`].
    fn write_bytes(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        self.write_through_tmp(path, &path.with_extension("tmp"), bytes)
    }

    fn read_json<T: serde::de::DeserializeOwned>(&self, path: &Path) -> Result<T, CoreError> {
        let bytes = self
            .fs
            .read_capped(path, MAX_JOURNAL_JSON_BYTES)
            .map_err(|e| CoreError::io(path, e))?;
        serde_json::from_slice(&bytes).map_err(|e| {
            CoreError::new(ErrorCode::Io, format!("corrupt journal file: {e}")).at(path)
        })
    }

    fn write_record(&self, record: &PlanRecord) -> Result<(), CoreError> {
        let path = self.plan_path(&record.id);
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(&path))?;
        self.write_json(&path, &bytes)
            .map_err(|e| CoreError::io(path, e))
    }
}

impl Journal for FsJournal {
    fn begin(&self, _guard: &ExclusiveGuard, plan: &PlanRecord) -> Result<(), CoreError> {
        if plan.status != PlanStatus::Pending {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "a plan must begin Pending",
            ));
        }
        let manifest_path = self.manifest_path(&plan.id);
        let manifest_bytes = serde_json::to_vec_pretty(&plan.backups)
            .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(&manifest_path))?;
        self.write_json(&manifest_path, &manifest_bytes)
            .map_err(|e| CoreError::io(manifest_path, e))?;
        // The plan itself is written only after the manifest above is
        // durable: a crash between the two leaves an orphaned manifest and
        // no plan row, never a plan row whose manifest never landed.
        self.write_record(plan)
    }

    fn record_step(
        &self,
        _guard: &ExclusiveGuard,
        id: &PlanId,
        step: PlanStep,
    ) -> Result<(), CoreError> {
        let mut record: PlanRecord = self.read_json(&self.plan_path(id))?;
        // A `WriteFile` step's backup was written by `write_backup` before
        // this call, but the manifest `begin` fsynced only knows the
        // backups the caller had at the plan's start; append it here too so
        // `trim_backups` and a later `all()` both see it.
        if let PlanStep::WriteFile {
            backup: Some(entry),
            ..
        } = &step
        {
            record.backups.push(entry.clone());
        }
        record.steps.push(step);
        self.write_record(&record)
    }

    fn finish(
        &self,
        _guard: &ExclusiveGuard,
        id: &PlanId,
        status: PlanStatus,
    ) -> Result<(), CoreError> {
        if status == PlanStatus::Pending {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "finish must not set Pending",
            ));
        }
        let mut record: PlanRecord = self.read_json(&self.plan_path(id))?;
        record.status = status;
        self.write_record(&record)
    }

    fn all(&self) -> Result<Vec<PlanRecord>, CoreError> {
        let plans_dir = self.root.join("plans");
        let entries = match self.fs.read_dir(&plans_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(CoreError::io(plans_dir, e)),
        };
        let mut records = Vec::new();
        for entry in entries {
            if entry.kind != FileKind::Dir {
                continue;
            }
            let plan_path = plans_dir.join(&entry.name).join("plan.json");
            records.push(self.read_json::<PlanRecord>(&plan_path)?);
        }
        records.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(records)
    }

    fn pending(&self) -> Result<Vec<PlanRecord>, CoreError> {
        Ok(self
            .all()?
            .into_iter()
            .filter(|p| p.status == PlanStatus::Pending)
            .collect())
    }

    fn remove_backup(
        &self,
        _guard: &ExclusiveGuard,
        id: &PlanId,
        relative: &str,
    ) -> Result<(), CoreError> {
        let path = self.backup_path(id, relative);
        match self.fs.fsops_remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CoreError::io(path, e)),
        }
    }

    fn write_backup(
        &self,
        _guard: &ExclusiveGuard,
        id: &PlanId,
        relative: &str,
        bytes: &[u8],
    ) -> Result<(), CoreError> {
        let path = self.backup_path(id, relative);
        self.write_bytes(&path, bytes)
            .map_err(|e| CoreError::io(path, e))
    }

    fn read_backup(&self, id: &PlanId, relative: &str) -> Result<Vec<u8>, CoreError> {
        let path = self.backup_path(id, relative);
        self.fs
            .read_capped(&path, MAX_BACKUP_BYTES)
            .map_err(|e| CoreError::io(path, e))
    }
}

/// A plan begun through [`Journal::begin`], held open while its steps run.
///
/// Invariant: dropping a `PlanWriter` without calling [`Self::finish`]
/// leaves the plan `Pending` on disk - the same state a real crash leaves -
/// so [`reconcile`] is the only thing that resolves it.
pub struct PlanWriter<'a> {
    journal: &'a dyn Journal,
    guard: &'a ExclusiveGuard,
    id: PlanId,
}

impl<'a> PlanWriter<'a> {
    /// Begins a plan: writes its manifest and itself, both durable, before
    /// returning. Nothing the caller does after a successful `begin` can be
    /// an unrecorded first step.
    pub fn begin(
        journal: &'a dyn Journal,
        guard: &'a ExclusiveGuard,
        id: PlanId,
        created_at: DateTime<Utc>,
        label: impl Into<String>,
        root: PathBuf,
        backups: Vec<PlanBackupEntry>,
    ) -> Result<Self, CoreError> {
        let record = PlanRecord {
            id: id.clone(),
            created_at,
            label: label.into(),
            root,
            backups,
            steps: Vec::new(),
            status: PlanStatus::Pending,
        };
        journal.begin(guard, &record)?;
        Ok(PlanWriter { journal, guard, id })
    }

    /// The plan's id.
    pub fn id(&self) -> &PlanId {
        &self.id
    }

    /// Records that [`crate::fsops::stage`] is about to build `staged` (its
    /// temp path). Called before the folder is created, so a crash before
    /// it exists leaves nothing for reversal's `remove_tree` to remove -
    /// nothing else has touched it either way.
    pub fn record_stage(&self, staged: &Path) -> Result<(), CoreError> {
        self.journal.record_step(
            self.guard,
            &self.id,
            PlanStep::Stage {
                staged: staged.to_path_buf(),
            },
        )
    }

    /// Records that [`crate::fsops::swap`] is about to put a folder at
    /// `path` by exchanging it with `staged` (captured at `staged_binding`,
    /// its device/inode right before the exchange). `quarantined` is where
    /// the folder that sits at `path` before the exchange will be relocated
    /// to, when one is there - `None` when `path` does not exist yet
    /// (`swap` will create it fresh). Called before the exchange runs.
    pub fn record_swap(
        &self,
        path: &Path,
        staged: &Path,
        staged_binding: (u64, u64),
        quarantined: Option<PathBuf>,
    ) -> Result<(), CoreError> {
        self.journal.record_step(
            self.guard,
            &self.id,
            PlanStep::Swap {
                path: path.to_path_buf(),
                staged: staged.to_path_buf(),
                staged_binding,
                quarantined,
            },
        )
    }

    /// Records that [`crate::fsops::link`] is about to set `path` to point
    /// at `target`. `previous_target` is what `path` pointed at before,
    /// when it already existed as a symlink. Called before the rename that
    /// makes the new link visible.
    pub fn record_link(
        &self,
        path: &Path,
        target: &Path,
        previous_target: Option<PathBuf>,
    ) -> Result<(), CoreError> {
        self.journal.record_step(
            self.guard,
            &self.id,
            PlanStep::Link {
                path: path.to_path_buf(),
                target: target.to_path_buf(),
                previous_target,
            },
        )
    }

    /// Records that [`crate::fsops::write_file`] is about to write `path`.
    /// When `previous` holds the file's pre-write bytes, writes them
    /// durably into this plan's own backup store first (via
    /// [`Journal::write_backup`]) and points the recorded step at that
    /// backup, so reversal can read them back. Called before the rename
    /// that puts the new bytes in place.
    pub fn record_write_file(&self, path: &Path, previous: Option<&[u8]>) -> Result<(), CoreError> {
        let backup = match previous {
            Some(bytes) => {
                let relative = backup_relative_name(path);
                self.journal
                    .write_backup(self.guard, &self.id, &relative, bytes)?;
                Some(PlanBackupEntry {
                    original: path.to_path_buf(),
                    relative,
                    bytes: bytes.len() as u64,
                })
            }
            None => None,
        };
        self.journal.record_step(
            self.guard,
            &self.id,
            PlanStep::WriteFile {
                path: path.to_path_buf(),
                backup,
            },
        )
    }

    /// Sets the plan's final status. Never `Pending`.
    pub fn finish(self, status: PlanStatus) -> Result<(), CoreError> {
        self.journal.finish(self.guard, &self.id, status)
    }
}

/// Turns an absolute path into a name unique enough to use as a backup's
/// `relative` under the plan's own backup directory - the leaf name plus a
/// counter, so two `write_file` steps against files with the same leaf
/// name never collide.
fn backup_relative_name(path: &Path) -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let leaf = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("backup");
    format!("{n}-{leaf}")
}

/// A plan [`reconcile`] found still `Pending` and could not undo, listed
/// with the I/O error reversal hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptedPlan {
    /// The plan's id.
    pub id: PlanId,
    /// What went wrong undoing it.
    pub error: String,
}

/// What startup reconciliation did to every plan it found `Pending`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconciliation {
    /// Plans that recorded at least one step and were fully undone, in
    /// reverse order, through [`ScopeFs`]: marked `Reversed`.
    pub reversed: Vec<PlanId>,
    /// Plans that recorded at least one step but undoing one hit an I/O
    /// error: marked `Interrupted` and listed with that error, since the
    /// on-disk state can no longer be trusted as either the pre-plan or the
    /// post-plan shape.
    pub interrupted: Vec<InterruptedPlan>,
    /// Plans that recorded no step at all: nothing had mutated anything
    /// yet, so the plan is resolved by marking it `Failed`, the same
    /// terminal state a plan that failed its own first step would reach.
    pub resolved_without_steps: Vec<PlanId>,
}

/// Resolves every plan [`Journal::pending`] still reports, newest first
/// (LIFO), never deleting a row: a plan with recorded steps has each one
/// undone, in reverse order, through `fs` - `Reversed` on success,
/// `Interrupted` (listed with the error) the moment one step's undo fails.
/// A plan with no recorded steps becomes `Failed` (nothing on disk needed
/// undoing). Idempotent - a second call finds nothing left `Pending` (a
/// `Reversed` or `Interrupted` plan is terminal either way).
///
/// Newest-first matters when two pending plans stack on the same path -
/// plan A swaps it, left `Pending`; plan B swaps it again, over A's result,
/// and also crashes `Pending`. Oldest-first would reverse A while B's
/// result still sits at the path: A's landed check finds someone else's
/// binding there, treats A as never landed, and marks it `Reversed`
/// without restoring anything; B then reverses correctly, but onto A's
/// (wrong) starting point, stranding what A actually replaced in A's own
/// quarantine folder. Reversing B first before A restores each plan's own
/// precondition in turn, the same order a stack of edits always undoes in.
pub fn reconcile(
    journal: &dyn Journal,
    guard: &ExclusiveGuard,
    fs: &dyn ScopeFs,
) -> Result<Reconciliation, CoreError> {
    let mut report = Reconciliation::default();
    for plan in journal.pending()?.into_iter().rev() {
        if plan.steps.is_empty() {
            journal.finish(guard, &plan.id, PlanStatus::Failed)?;
            report.resolved_without_steps.push(plan.id);
            continue;
        }
        match reverse_steps(journal, &plan, fs) {
            Ok(()) => {
                journal.finish(guard, &plan.id, PlanStatus::Reversed)?;
                report.reversed.push(plan.id);
            }
            Err(error) => {
                journal.finish(guard, &plan.id, PlanStatus::Interrupted)?;
                report.interrupted.push(InterruptedPlan {
                    id: plan.id,
                    error: error.to_string(),
                });
            }
        }
    }
    Ok(report)
}

/// Every path inside `step` that reversal would write to - what
/// [`reverse_steps`] confines under the plan's root before touching
/// anything for that step.
fn step_paths(step: &PlanStep) -> Vec<&Path> {
    match step {
        PlanStep::Stage { staged } => vec![staged],
        PlanStep::Swap {
            path,
            staged,
            quarantined,
            ..
        } => {
            let mut paths = vec![path.as_path(), staged.as_path()];
            if let Some(quarantined) = quarantined {
                paths.push(quarantined);
            }
            paths
        }
        PlanStep::Link { path, .. } | PlanStep::WriteFile { path, .. } => vec![path],
    }
}

/// Confines `path` under `plan_root` via [`Root::confine`], so a corrupt or
/// hand-edited plan row can never make reversal touch something outside the
/// plan's own root. `path` is already absolute (every `fsops` primitive
/// resolves it through `Root::confine` before recording it), so this is a
/// prefix strip followed by the same ancestor-symlink check `fsops` itself
/// runs, not a second, weaker check.
fn confine_to_root(root: &Root, plan_root: &Path, path: &Path) -> std::io::Result<()> {
    let relative = path.strip_prefix(plan_root).map_err(|_| {
        std::io::Error::other(format!(
            "{}: escapes the plan root {}",
            path.display(),
            plan_root.display()
        ))
    })?;
    root.confine(relative).map(|_| ()).map_err(|e| {
        std::io::Error::other(format!("{}: escapes the plan root ({e})", path.display()))
    })
}

/// Undoes `plan`'s recorded steps in reverse order. Stops at the first
/// step whose undo fails; steps already undone stay undone (there is no
/// partial-undo rollback - a step's own undo is the smallest unit this
/// resolves).
///
/// Every step was recorded before its own mutation ran (see the module
/// doc), so a crash can only ever land between "recorded" and "mutation
/// landed" - never the reverse. Before undoing a step, this checks the disk
/// to tell whether the mutation it describes actually landed, and reverses
/// only what did:
/// - `Stage`: `remove_tree` is already a no-op when nothing is at `staged`.
/// - `Swap`: landed iff `path`'s current device/inode matches
///   `staged_binding` (the staged folder's identity, captured before the
///   exchange - a rename/exchange preserves identity across the name
///   change, so this holds regardless of whether the follow-up move into
///   `quarantined` also landed).
/// - `Link`: landed iff `path` is currently a symlink pointing at `target`,
///   or `path` is absent and `previous_target` is `Some` (a previous
///   reversal attempt that crashed mid-restore, back when the restore was
///   remove-then-create rather than the rename below). Restoring
///   `previous_target` itself goes through the same temp-name-then-rename
///   shape `fsops::link` uses, so `path` only ever shows a complete link -
///   never briefly absent - and a crash mid-restore is simply retried.
/// - `WriteFile`: landed iff the live bytes at `path` no longer match the
///   backup (the backup was fsynced durable before the rename that would
///   have changed them).
///
/// Each of these probes disk state through a fallible call (`read_link`,
/// `fsops_device_inode`, `read_capped`); only a `NotFound` error means "the
/// mutation never landed" - any other I/O error (permissions, EIO, ...)
/// leaves that unknown and is propagated, so the step's undo fails and the
/// plan resolves `Interrupted` instead of guessing "not landed" and
/// deleting or overwriting something that is, in fact, still live.
///
/// Every path a step names is confined under `plan.root` first (via
/// [`confine_to_root`]); a step naming a path outside it aborts reversal
/// immediately, naming the escaped path, without touching anything.
fn reverse_steps(
    journal: &dyn Journal,
    plan: &PlanRecord,
    fs: &dyn ScopeFs,
) -> std::io::Result<()> {
    let root =
        Root::open(fs, plan.root.clone()).map_err(|e| std::io::Error::other(e.to_string()))?;
    for step in plan.steps.iter().rev() {
        for path in step_paths(step) {
            confine_to_root(&root, &plan.root, path)?;
        }
        match step {
            PlanStep::Stage { staged } => remove_tree(fs, staged)?,
            PlanStep::Swap {
                path,
                staged,
                staged_binding,
                quarantined,
            } => {
                let landed = match fs.fsops_device_inode(path) {
                    Ok(inode) => inode == *staged_binding,
                    // Only a missing path means the exchange never ran -
                    // any other error (EACCES, EIO, ...) leaves whether it
                    // landed unknown, and guessing "no" here is what lets
                    // the paired Stage step's reversal delete a folder that
                    // is in fact still live at `path`.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                    Err(e) => return Err(e),
                };
                if !landed {
                    // The exchange never ran; `path` still shows whatever
                    // was there before this plan. Nothing to undo here -
                    // the preceding `Stage` step's own reversal removes
                    // the still-unswapped staged folder.
                    continue;
                }
                match quarantined {
                    Some(quarantined) if fs.symlink_metadata(quarantined).is_ok() => {
                        // Both the exchange and the follow-up move into
                        // quarantine landed.
                        fs.fsops_exchange(quarantined, path)?;
                        remove_tree(fs, quarantined)?;
                    }
                    Some(_) => {
                        // The exchange landed but the move into quarantine
                        // never did: the pre-swap folder is still sitting
                        // at `staged`'s temp name. Exchange it back; the
                        // preceding `Stage` step's own reversal then
                        // removes what is left at `staged` (the folder
                        // this step is undoing).
                        fs.fsops_exchange(staged, path)?;
                    }
                    None => remove_tree(fs, path)?,
                }
            }
            PlanStep::Link {
                path,
                target,
                previous_target,
            } => {
                let landed = match fs.read_link(path) {
                    Ok(current) => current == *target,
                    // A missing path is still "landed" when a previous
                    // reversal attempt got as far as removing the link but
                    // crashed before restoring `previous_target` - back
                    // when that was two separate calls with no link at
                    // `path` in between. Any other error leaves whether it
                    // landed unknown.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => previous_target.is_some(),
                    Err(e) => return Err(e),
                };
                if !landed {
                    continue;
                }
                match previous_target {
                    Some(previous_target) => {
                        // Same temp-name-then-rename shape `fsops::link`
                        // itself uses (fsops.rs): `path` only ever shows a
                        // complete link, so a crash mid-restore leaves the
                        // step still `landed` (still pointing at `target`,
                        // or already absent - both retried above) rather
                        // than losing the link entirely.
                        let parent = path.parent().unwrap_or(Path::new("/")).to_path_buf();
                        let leaf = path.file_name().and_then(|s| s.to_str()).unwrap_or("link");
                        let temp_prefix = format!(".{leaf}-");
                        // A crashed earlier restore attempt (this process or
                        // a previous one) may have left its own temp
                        // symlink under this same prefix behind - minted
                        // then never consumed because the crash landed
                        // before the rename below. Clear every such entry
                        // first so a retry always converges instead of
                        // accumulating orphaned temp links across retries.
                        for entry in fs.read_dir(&parent)? {
                            if !entry.name.starts_with(&temp_prefix) {
                                continue;
                            }
                            let stale = parent.join(&entry.name);
                            match fs.fsops_remove_file(&stale) {
                                Ok(()) => {}
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                                Err(e) => return Err(e),
                            }
                        }
                        let tmp_path = parent.join(format!("{temp_prefix}{}", fsops::unique_suffix()));
                        fs.fsops_symlink(previous_target, &tmp_path)?;
                        fs.fsops_rename(&tmp_path, path)?;
                    }
                    None => fs.fsops_remove_file(path)?,
                }
            }
            PlanStep::WriteFile { path, backup } => match backup {
                Some(entry) => {
                    let backup_bytes = journal
                        .read_backup(&plan.id, &entry.relative)
                        .map_err(|e| std::io::Error::other(e.to_string()))?;
                    let landed = match fs.read_capped(path, u64::MAX) {
                        Ok(live) => live != backup_bytes,
                        // Only a missing path means the write never landed
                        // (the rename always leaves a file at `path`) - any
                        // other error leaves whether it landed unknown.
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                        Err(e) => return Err(e),
                    };
                    if landed {
                        if fs.symlink_metadata(path).is_ok() {
                            fs.fsops_remove_file(path)?;
                        }
                        fs.fsops_write_new_file(path, &backup_bytes)?;
                        fs.fsops_fsync_file(path)?;
                    }
                }
                None => {
                    if fs.symlink_metadata(path).is_ok() {
                        fs.fsops_remove_file(path)?;
                    }
                }
            },
        }
    }
    Ok(())
}

/// Removes `path` and everything under it, deepest first, using only the
/// per-entry primitives [`ScopeFs`] offers (there is no recursive remove on
/// the port itself). A no-op when nothing is at `path`.
fn remove_tree(fs: &dyn ScopeFs, path: &Path) -> std::io::Result<()> {
    let facts = match fs.symlink_metadata(path) {
        Ok(facts) => facts,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if facts.kind != FileKind::Dir {
        return fs.fsops_remove_file(path);
    }
    for entry in fs.read_dir(path)? {
        remove_tree(fs, &path.join(&entry.name))?;
    }
    fs.fsops_remove_dir(path)
}

/// Size-and-age limit on the backups plans keep for undo.
#[derive(Debug, Clone, Copy)]
pub struct BackupQuota {
    /// Total bytes every kept backup may sum to.
    pub max_total_bytes: u64,
    /// A backup older than this, regardless of total size, is trimmed.
    pub max_age: Duration,
}

/// One backup [`trim_backups`] removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrimmedBackup {
    /// The plan the backup belonged to.
    pub plan: PlanId,
    /// The backup's relative path inside that plan.
    pub relative: String,
}

/// Trims backups over `quota`, oldest first, using each backup's plan's
/// `created_at` as its age. Age violations are removed first, then size
/// violations, until both the age and the total-size budget hold; never
/// touches a plan row, only the backup bytes [`Journal::remove_backup`]
/// deletes.
pub fn trim_backups(
    journal: &dyn Journal,
    guard: &ExclusiveGuard,
    now: DateTime<Utc>,
    quota: &BackupQuota,
) -> Result<Vec<TrimmedBackup>, CoreError> {
    let mut entries: Vec<(DateTime<Utc>, PlanId, PlanBackupEntry)> = journal
        .all()?
        .into_iter()
        .flat_map(|plan| {
            plan.backups
                .into_iter()
                .map(move |backup| (plan.created_at, plan.id.clone(), backup))
        })
        .collect();
    entries.sort_by_key(|a| a.0);

    let mut trimmed = Vec::new();
    let mut kept = Vec::new();
    for entry in entries {
        let age = now.signed_duration_since(entry.0);
        let over_age = age.to_std().map(|d| d > quota.max_age).unwrap_or(false);
        if over_age {
            journal.remove_backup(guard, &entry.1, &entry.2.relative)?;
            trimmed.push(TrimmedBackup {
                plan: entry.1,
                relative: entry.2.relative,
            });
        } else {
            kept.push(entry);
        }
    }

    let mut total: u64 = kept.iter().map(|(_, _, backup)| backup.bytes).sum();
    let mut i = 0;
    while total > quota.max_total_bytes && i < kept.len() {
        let (_, plan, backup) = kept[i].clone();
        journal.remove_backup(guard, &plan, &backup.relative)?;
        total -= backup.bytes;
        trimmed.push(TrimmedBackup {
            plan,
            relative: backup.relative,
        });
        i += 1;
    }

    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsops::{self, Root};
    use crate::ports::{LeaseMode, LeaseProvider};
    use crate::testing::{FailingFs, FakeLease, FixtureBuilder};

    fn guard(lease: &FakeLease) -> ExclusiveGuard {
        let handle = lease
            .acquire(&[], LeaseMode::Exclusive, Duration::from_secs(0))
            .expect("acquire exclusive lease");
        ExclusiveGuard::from_handle(handle)
    }

    fn journal_over(fs: Arc<dyn ScopeFs>) -> FsJournal {
        FsJournal::new(PathBuf::from("/journal"), fs)
    }

    /// Given a plan begun through `Journal::begin`, when nothing has run
    /// yet, then the manifest and the plan file both already exist on disk;
    /// on failure the panic names the first step that ran instead (there is
    /// none - `begin` alone must already have written both).
    #[test]
    fn journal_writes_the_manifest_and_plan_before_the_first_step_or_names_the_step_that_ran_first()
    {
        let fs: Arc<dyn ScopeFs> = Arc::new(FixtureBuilder::new().dir("/journal").build_fs());
        let journal = journal_over(fs.clone());
        let lease = FakeLease::default();
        let g = guard(&lease);

        let id = PlanId("01PLAN0000000000000000000".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            id.clone(),
            Utc::now(),
            "test plan",
            PathBuf::from("/root"),
            vec![PlanBackupEntry {
                original: PathBuf::from("/root/skill/SKILL.md"),
                relative: "skill/SKILL.md".into(),
                bytes: 3,
            }],
        )
        .expect("begin must write the manifest and plan before returning");

        assert!(
            fs.symlink_metadata(Path::new(
                "/journal/plans/01PLAN0000000000000000000/manifest.json"
            ))
            .is_ok(),
            "begin returned Ok, so the manifest must already be on disk - no step ran first"
        );
        assert!(
            fs.symlink_metadata(Path::new(
                "/journal/plans/01PLAN0000000000000000000/plan.json"
            ))
            .is_ok(),
            "begin returned Ok, so the plan file must already be on disk - no step ran first"
        );

        let record = journal
            .all()
            .expect("read plans back")
            .into_iter()
            .find(|p| p.id == id)
            .expect("the plan begun above");
        assert!(
            record.steps.is_empty(),
            "no step has run yet; the plan must record none"
        );

        plan.finish(PlanStatus::Done).expect("finish");
    }

    /// Given a plan, when it is begun and then finished, then its status
    /// reads `Pending` after `begin` and `Done` only after `finish`; on
    /// failure the panic names whichever transition did not happen.
    #[test]
    fn journal_marks_a_row_pending_before_the_mutation_and_done_after_the_last_step_or_names_the_missing_transition(
    ) {
        let fs: Arc<dyn ScopeFs> = Arc::new(FixtureBuilder::new().dir("/journal").build_fs());
        let journal = journal_over(fs);
        let lease = FakeLease::default();
        let g = guard(&lease);

        let id = PlanId("01PLAN0000000000000000001".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            id.clone(),
            Utc::now(),
            "test plan",
            PathBuf::from("/root"),
            Vec::new(),
        )
        .expect("begin");

        let after_begin = journal
            .all()
            .expect("read")
            .into_iter()
            .find(|p| p.id == id)
            .expect("plan exists after begin");
        assert_eq!(
            after_begin.status,
            PlanStatus::Pending,
            "must be Pending before the mutation starts, not {:?}",
            after_begin.status
        );

        plan.record_write_file(Path::new("/root/skill/SKILL.md"), None)
            .expect("record the last step");
        journal
            .finish(&g, &id, PlanStatus::Done)
            .expect("finish must mark the row Done");

        let after_finish = journal
            .all()
            .expect("read")
            .into_iter()
            .find(|p| p.id == id)
            .expect("plan still exists after finish");
        assert_eq!(
            after_finish.status,
            PlanStatus::Done,
            "must be Done after the last step, not {:?}",
            after_finish.status
        );
    }

    /// Given a fixture with an existing `skill` folder, when a plan runs two
    /// of its three intended steps (`stage`+`swap` replacing `skill`, then
    /// `link` pointing `skill-current` at it) and the process dies before
    /// its third step and before `finish`, then startup reconciliation
    /// undoes both recorded steps in reverse and the tree under `/root`
    /// reads back byte-for-byte identical to the snapshot taken before the
    /// plan ran; on failure the panic names the diverging path or the plan
    /// left open.
    #[test]
    fn startup_reconciliation_after_a_simulated_crash_completes_or_reverses_every_pending_plan_or_names_the_plan_left_open(
    ) {
        let fs: Arc<dyn ScopeFs> = Arc::new(
            FixtureBuilder::new()
                .dir("/journal")
                .dir("/root")
                .dir("/root/skill")
                .file("/root/skill/SKILL.md", b"pre-plan content")
                .build_fs(),
        );
        let root_path = PathBuf::from("/root");
        let before = tree_snapshot(fs.as_ref(), &root_path);

        let journal = journal_over(fs.clone());
        let lease = FakeLease::default();
        let g = guard(&lease);
        let root = Root::open(fs.as_ref(), root_path.clone()).expect("open root");

        let id = PlanId("01PLAN0000000000000000002".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            id.clone(),
            Utc::now(),
            "crashes after two of three steps",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin");

        let staged = fsops::stage(
            &root,
            &plan,
            &[(PathBuf::from("SKILL.md"), b"new content".to_vec())],
        )
        .expect("stage");
        fsops::swap(
            &root,
            &plan,
            Path::new("skill"),
            &staged,
            Path::new(".trash"),
        )
        .expect("swap over the existing skill folder");
        fsops::link(&root, &plan, Path::new("skill-current"), Path::new("skill")).expect("link");
        // The plan's third step (e.g. a write_file into the new folder)
        // never runs: the process dies here, before `finish`, and `plan`
        // is simply dropped rather than resolved.
        drop(plan);

        let after_crash = tree_snapshot(fs.as_ref(), &root_path);
        assert_ne!(
            after_crash, before,
            "the swap and link above must actually have changed the tree, or this test proves nothing"
        );

        let report = reconcile(&journal, &g, fs.as_ref()).expect("reconciliation must run");
        assert!(
            report.reversed.contains(&id),
            "a plan whose recorded steps could all be undone must resolve as Reversed, not {report:?}"
        );

        let after_reconcile = tree_snapshot(fs.as_ref(), &root_path);
        assert_eq!(
            after_reconcile, before,
            "reconciliation must put the tree back exactly as the pre-plan snapshot"
        );

        let after = journal
            .all()
            .expect("read")
            .into_iter()
            .find(|p| p.id == id)
            .unwrap_or_else(|| panic!("plan {} must still exist after reconciliation", id.0));
        assert_eq!(
            after.status,
            PlanStatus::Reversed,
            "plan {} was left open (still Pending) or not resolved as Reversed, was {:?}",
            id.0,
            after.status
        );
    }

    /// Given two plans left `Pending` by a crash - one with a `link` step
    /// reconciliation can cleanly undo, and one whose `write_file` step's
    /// backup bytes were deleted from disk before reconciliation runs (so
    /// undoing it cannot proceed) - when reconciliation runs, then both rows
    /// still exist afterward, the first is `Reversed`, and the second is
    /// `Interrupted` and named in the report; on failure the panic names
    /// whichever row went missing or was not listed.
    #[test]
    fn startup_reconciliation_never_deletes_a_row_and_lists_every_interrupted_plan_or_names_the_missing_row(
    ) {
        let fs: Arc<dyn ScopeFs> = Arc::new(
            FixtureBuilder::new()
                .dir("/journal")
                .dir("/root")
                .file("/root/file.txt", b"original")
                .build_fs(),
        );
        let root_path = PathBuf::from("/root");
        let journal = journal_over(fs.clone());
        let lease = FakeLease::default();
        let g = guard(&lease);
        let root = Root::open(fs.as_ref(), root_path.clone()).expect("open root");

        let with_step = PlanId("01PLAN0000000000000000003".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            with_step.clone(),
            Utc::now(),
            "with a cleanly reversible step",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin");
        fsops::link(&root, &plan, Path::new("skill-link"), Path::new("file.txt")).expect("link");
        drop(plan);

        let unresolvable = PlanId("01PLAN0000000000000000004".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            unresolvable.clone(),
            Utc::now(),
            "its backup is deleted before reconciliation",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin");
        let target = root_path.join("file.txt");
        let stamp = fsops::read_stamp(fs.as_ref(), &target).expect("read the stamp");
        fsops::write_file(&root, &plan, Path::new("file.txt"), b"changed", &stamp)
            .expect("write_file");
        drop(plan);

        let record = journal
            .all()
            .expect("read")
            .into_iter()
            .find(|p| p.id == unresolvable)
            .expect("the plan begun above");
        let backup_relative = record
            .backups
            .first()
            .expect("write_file over an existing file must have recorded a backup")
            .relative
            .clone();
        fs.fsops_remove_file(&PathBuf::from(format!(
            "/journal/plans/{}/backups/{backup_relative}",
            unresolvable.0
        )))
        .expect("delete the backup bytes so reversal cannot proceed");

        let report = reconcile(&journal, &g, fs.as_ref()).expect("reconciliation must run");
        assert!(
            report.reversed.contains(&with_step),
            "the plan with a cleanly reversible step must be listed as reversed"
        );
        assert!(
            report.interrupted.iter().any(|p| p.id == unresolvable),
            "the plan whose backup was deleted must be listed as interrupted, naming the plan: {report:?}"
        );

        let after = journal.all().expect("read every row back");
        for id in [&with_step, &unresolvable] {
            assert!(
                after.iter().any(|p| &p.id == id),
                "row {} went missing after reconciliation",
                id.0
            );
        }
        let after_unresolvable = after
            .iter()
            .find(|p| p.id == unresolvable)
            .expect("row still exists");
        assert_eq!(
            after_unresolvable.status,
            PlanStatus::Interrupted,
            "plan {} must be Interrupted since its backup could not be read back, was {:?}",
            unresolvable.0,
            after_unresolvable.status
        );
    }

    /// Given a root with an existing `skill` folder, when `swap`'s exchange
    /// lands but the process dies before the follow-up move of the old
    /// folder into quarantine, then startup reconciliation still restores
    /// `skill` to exactly its pre-plan bytes - using the quarantine path
    /// and the staged folder's identity `swap` recorded *before* the
    /// exchange ran, not the (never-written) quarantine folder itself; on
    /// failure the panic names the diverging path, which would be the old
    /// folder deleted rather than restored.
    #[test]
    fn a_crash_between_the_swap_exchange_and_its_journal_row_reverses_to_the_old_folder_or_names_the_folder_it_deleted(
    ) {
        let fixture = FixtureBuilder::new()
            .dir("/journal")
            .dir("/root")
            .dir("/root/skill")
            .file("/root/skill/SKILL.md", b"old content")
            .build_fs();
        let root_path = PathBuf::from("/root");
        let before = tree_snapshot(&fixture, &root_path);

        let failing = FailingFs::wrap(Arc::new(fixture.clone()));
        let root = Root::open(&failing, root_path.clone()).expect("open root");
        let journal = journal_over(Arc::new(fixture.clone()));
        let lease = FakeLease::default();
        let g = guard(&lease);

        let id = PlanId("01PLANSWAPCRASH00000000001".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            id.clone(),
            Utc::now(),
            "crash between exchange and quarantine move",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin");

        let staged = fsops::stage(
            &root,
            &plan,
            &[(PathBuf::from("SKILL.md"), b"new content".to_vec())],
        )
        .expect("stage");
        // The exchange itself (a `fsops_exchange` call) is unaffected; only
        // the follow-up `fsops_rename` that moves the old folder into
        // quarantine fails, landing the crash exactly between the two.
        failing.fail_next_fsops_rename();
        fsops::swap(
            &root,
            &plan,
            Path::new("skill"),
            &staged,
            Path::new(".trash"),
        )
        .expect_err("the quarantine move must fail, or this test proves nothing about the window between it and the exchange");
        // The process dies here: `plan` is dropped, never finished.
        drop(plan);

        let report = reconcile(&journal, &g, &fixture).expect("reconciliation must run");
        assert!(
            report.reversed.contains(&id),
            "a swap step whose exchange landed must still be fully reversible, not {report:?}"
        );

        let after = tree_snapshot(&fixture, &root_path);
        assert_eq!(
            after, before,
            "reconciliation must restore the old folder byte-for-byte, or name the folder it deleted"
        );
    }

    /// Given a swap plan whose exchange landed but the process died before
    /// the follow-up move into quarantine (the same window as the test
    /// above), when reconciliation's probe of the swapped path's identity
    /// then hits a non-`NotFound` I/O error - EACCES, EIO, or similar,
    /// rather than the path simply being absent - it must not read that as
    /// "the exchange never landed": doing so would let the paired `Stage`
    /// step's reversal delete the folder that is, in fact, still live at
    /// `path`. The plan resolves `Interrupted` and the error is named in
    /// the report; on failure the panic names the folder it deleted anyway.
    #[test]
    fn an_io_error_while_probing_a_swapped_path_interrupts_the_plan_or_names_the_old_folder_it_deleted(
    ) {
        let fixture = FixtureBuilder::new()
            .dir("/journal")
            .dir("/root")
            .dir("/root/skill")
            .file("/root/skill/SKILL.md", b"old content")
            .build_fs();
        let root_path = PathBuf::from("/root");

        let failing = FailingFs::wrap(Arc::new(fixture.clone()));
        let root = Root::open(&failing, root_path.clone()).expect("open root");
        let journal = journal_over(Arc::new(fixture.clone()));
        let lease = FakeLease::default();
        let g = guard(&lease);

        let id = PlanId("01PLANIOPROBE0000000000001".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            id.clone(),
            Utc::now(),
            "crash between exchange and quarantine move, then a probe I/O error",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin");

        let staged = fsops::stage(
            &root,
            &plan,
            &[(PathBuf::from("SKILL.md"), b"new content".to_vec())],
        )
        .expect("stage");
        failing.fail_next_fsops_rename();
        fsops::swap(
            &root,
            &plan,
            Path::new("skill"),
            &staged,
            Path::new(".trash"),
        )
        .expect_err("the quarantine move must fail, or this test proves nothing about the window between it and the exchange");
        // The process dies here: `plan` is dropped, never finished.
        drop(plan);

        let before_reconcile = tree_snapshot(&fixture, &root_path);
        let staged_path = staged.path().to_path_buf();
        assert!(
            failing.symlink_metadata(&staged_path).is_ok(),
            "the staged folder must still be on disk before reconciliation runs"
        );

        // `reverse_steps` opens `Root` (one `fsops_device_inode` call) before
        // it ever reaches the `Swap` step's own probe (the second call): arm
        // the second call so the failure lands on the probe under test
        // rather than being consumed by `Root::open`.
        failing.fail_nth_fsops_device_inode(2);
        let report = reconcile(&journal, &g, &failing).expect("reconciliation must run");
        let interrupted = report
            .interrupted
            .iter()
            .find(|p| p.id == id)
            .unwrap_or_else(|| {
                panic!("a probe error other than NotFound must interrupt the plan, not {report:?}")
            });
        assert!(
            interrupted.error.contains("fsops_device_inode"),
            "the report must name the error the probe hit, got: {}",
            interrupted.error
        );

        let after = tree_snapshot(&fixture, &root_path);
        assert_eq!(
            after, before_reconcile,
            "an I/O error while probing must not delete or change anything, or names the old folder it deleted"
        );
        assert!(
            failing.symlink_metadata(&staged_path).is_ok(),
            "the Swap step's own probe must interrupt the plan before the paired Stage step's reversal runs, or names the staged folder it deleted"
        );
    }

    /// Given an existing file, when `write_file`'s rename lands but the
    /// process dies before the primitive returns, then startup
    /// reconciliation restores the previous bytes from the backup
    /// `write_file` fsynced *before* that rename ran; on failure the panic
    /// names the lost backup (the live content stuck on the new bytes).
    #[test]
    fn a_crash_after_the_write_file_rename_restores_the_previous_bytes_or_names_the_lost_backup() {
        let fixture = FixtureBuilder::new()
            .dir("/journal")
            .dir("/root")
            .file("/root/file.txt", b"original")
            .build_fs();
        let root_path = PathBuf::from("/root");
        let before = tree_snapshot(&fixture, &root_path);

        let failing = FailingFs::wrap(Arc::new(fixture.clone()));
        let root = Root::open(&failing, root_path.clone()).expect("open root");
        let journal = journal_over(Arc::new(fixture.clone()));
        let lease = FakeLease::default();
        let g = guard(&lease);

        let id = PlanId("01PLANWRITECRASH0000000001".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            id.clone(),
            Utc::now(),
            "crash after the write_file rename",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin");

        let target = root_path.join("file.txt");
        let stamp = fsops::read_stamp(&failing, &target).expect("read the stamp");
        // The rename that lands the new bytes is unaffected; the very next
        // call - `fsync_up_to_root`'s first `fsops_fsync_dir` - fails,
        // landing the crash right after the rename.
        failing.fail_next_fsops_fsync_dir();
        fsops::write_file(&root, &plan, Path::new("file.txt"), b"new bytes", &stamp)
            .expect_err("the fsync after the rename must fail, or this test proves nothing about the window after it");
        drop(plan);

        let report = reconcile(&journal, &g, &fixture).expect("reconciliation must run");
        assert!(
            report.reversed.contains(&id),
            "a write_file step whose rename landed must still be fully reversible, not {report:?}"
        );

        let after = tree_snapshot(&fixture, &root_path);
        assert_eq!(
            after, before,
            "reconciliation must restore the previous bytes, or name the lost backup"
        );
    }

    /// Given an existing file one byte larger than `MAX_JOURNAL_JSON_BYTES`
    /// (the cap `read_backup` used to reuse from plan/manifest JSON
    /// readback), when `write_file` backs it up (with no cap, matching how
    /// it read the pre-write bytes) and then crashes before `finish`, then
    /// startup reconciliation still reads that backup back and restores it
    /// byte-for-byte; on failure the panic names the cap that blocked it
    /// (the post-write bytes left in place, plan `Interrupted`).
    #[test]
    fn a_backup_larger_than_the_journal_json_cap_still_restores_or_names_the_cap_that_blocked_it() {
        let big = vec![b'A'; (16 * 1024 * 1024) + 1];
        let fixture = FixtureBuilder::new()
            .dir("/journal")
            .dir("/root")
            .file("/root/file.txt", &big)
            .build_fs();
        let root_path = PathBuf::from("/root");
        let before = tree_snapshot(&fixture, &root_path);

        let failing = FailingFs::wrap(Arc::new(fixture.clone()));
        let root = Root::open(&failing, root_path.clone()).expect("open root");
        let journal = journal_over(Arc::new(fixture.clone()));
        let lease = FakeLease::default();
        let g = guard(&lease);

        let id = PlanId("01PLANBIGBACKUP0000000001".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            id.clone(),
            Utc::now(),
            "write over a file whose backup exceeds the journal JSON cap",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin");

        let target = root_path.join("file.txt");
        let stamp = fsops::read_stamp(&failing, &target).expect("read the stamp");
        // Same window as the write_file crash test above: the rename lands,
        // the fsync right after it fails.
        failing.fail_next_fsops_fsync_dir();
        fsops::write_file(&root, &plan, Path::new("file.txt"), b"new bytes", &stamp)
            .expect_err("the fsync after the rename must fail, or this test proves nothing about the window after it");
        drop(plan);

        let report = reconcile(&journal, &g, &fixture).expect("reconciliation must run");
        assert!(
            report.reversed.contains(&id),
            "a write_file step whose backup exceeds MAX_JOURNAL_JSON_BYTES must still be reversible, not {report:?}"
        );

        let after = tree_snapshot(&fixture, &root_path);
        assert_eq!(
            after, before,
            "reconciliation must restore the oversized backup byte-for-byte, or names the cap that blocked it"
        );
    }

    /// Given an existing symlink, when `link`'s rename lands but the
    /// process dies before the primitive returns, then startup
    /// reconciliation restores the previous target `link` recorded before
    /// that rename ran; on failure the panic names the target it forgot
    /// (the live link stuck on the new target instead).
    #[test]
    fn a_crash_after_the_link_rename_restores_the_previous_link_target_or_names_the_target_it_forgot(
    ) {
        let fixture = FixtureBuilder::new()
            .dir("/journal")
            .dir("/root")
            .file("/root/old.txt", b"old")
            .file("/root/new.txt", b"new")
            .build_fs();
        let root_path = PathBuf::from("/root");
        let journal = journal_over(Arc::new(fixture.clone()));
        let lease = FakeLease::default();
        let g = guard(&lease);

        // Seed an existing link pointing at `old.txt`, fully committed, so
        // this test's own crash is about *replacing* a link, not creating
        // one from nothing.
        let seed_root = Root::open(&fixture, root_path.clone()).expect("open root");
        let seed_id = PlanId("01PLANLINKSEED000000000001".into());
        let seed_plan = PlanWriter::begin(
            &journal,
            &g,
            seed_id,
            Utc::now(),
            "seed the existing link",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin seed plan");
        fsops::link(
            &seed_root,
            &seed_plan,
            Path::new("skill-current"),
            Path::new("old.txt"),
        )
        .expect("seed link");
        seed_plan
            .finish(PlanStatus::Done)
            .expect("finish seed plan");

        let before = tree_snapshot(&fixture, &root_path);

        let failing = FailingFs::wrap(Arc::new(fixture.clone()));
        let root = Root::open(&failing, root_path.clone()).expect("open root");
        let id = PlanId("01PLANLINKCRASH0000000001".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            id.clone(),
            Utc::now(),
            "crash after the link rename",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin");
        // Same window as the write_file test above: the rename lands, the
        // fsync right after it fails.
        failing.fail_next_fsops_fsync_dir();
        fsops::link(&root, &plan, Path::new("skill-current"), Path::new("new.txt"))
            .expect_err("the fsync after the rename must fail, or this test proves nothing about the window after it");
        drop(plan);

        let report = reconcile(&journal, &g, &fixture).expect("reconciliation must run");
        assert!(
            report.reversed.contains(&id),
            "a link step whose rename landed must still be fully reversible, not {report:?}"
        );

        let after = tree_snapshot(&fixture, &root_path);
        assert_eq!(
            after, before,
            "reconciliation must restore the previous link target, or name the target it forgot"
        );
    }

    /// Given an existing symlink whose target reconciliation is replacing -
    /// the same seeded-link setup as the test above - when the *reversal's
    /// own restore* is what crashes (the temp-symlink-then-rename that
    /// swaps `path` onto `previous_target`), then the plan row stays
    /// `Pending` on disk (the crash landed before reconciliation's own
    /// `finish` call - the exact window `reconcile` already protects for a
    /// forward mutation), `path` never shows an absent link in between, and
    /// the next full reconciliation - with a healthy filesystem - retries
    /// and fully restores `previous_target`; on failure the panic names the
    /// target it lost.
    #[test]
    fn a_crash_inside_link_reversal_restores_the_previous_target_on_the_next_reconcile_or_names_the_target_it_lost(
    ) {
        let fixture = FixtureBuilder::new()
            .dir("/journal")
            .dir("/root")
            .file("/root/old.txt", b"old")
            .file("/root/new.txt", b"new")
            .build_fs();
        let root_path = PathBuf::from("/root");
        let journal = journal_over(Arc::new(fixture.clone()));
        let lease = FakeLease::default();
        let g = guard(&lease);

        // Seed an existing link pointing at `old.txt`, fully committed.
        let seed_root = Root::open(&fixture, root_path.clone()).expect("open root");
        let seed_id = PlanId("01PLANLINKATOMSEED00000001".into());
        let seed_plan = PlanWriter::begin(
            &journal,
            &g,
            seed_id,
            Utc::now(),
            "seed the existing link",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin seed plan");
        fsops::link(
            &seed_root,
            &seed_plan,
            Path::new("skill-current"),
            Path::new("old.txt"),
        )
        .expect("seed link");
        seed_plan
            .finish(PlanStatus::Done)
            .expect("finish seed plan");
        let pristine = tree_snapshot(&fixture, &root_path);

        // A second plan replaces the link with one pointing at `new.txt`,
        // fully landing (no crash in the forward direction), then is left
        // Pending - the ordinary "crash before finish" case.
        let root = Root::open(&fixture, root_path.clone()).expect("open root");
        let id = PlanId("01PLANLINKATOM0000000001".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            id.clone(),
            Utc::now(),
            "replace the link, then crash before finish",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin");
        fsops::link(
            &root,
            &plan,
            Path::new("skill-current"),
            Path::new("new.txt"),
        )
        .expect("link");
        drop(plan);

        // Simulates the reversal itself crashing partway through its own
        // restore - calling `reverse_steps` directly, the way `reconcile`
        // does internally, so the row's status is never written either
        // way: still `Pending` on disk, same as a real process death.
        let record = journal
            .all()
            .expect("read")
            .into_iter()
            .find(|p| p.id == id)
            .expect("the plan begun above");
        let failing = FailingFs::wrap(Arc::new(fixture.clone()));
        failing.fail_next_fsops_rename();
        reverse_steps(&journal, &record, &failing).expect_err(
            "the rename inside the atomic link restore must fail, or this test proves nothing about a crash mid-restore",
        );

        let after_first_attempt = fixture
            .read_link(Path::new("/root/skill-current"))
            .expect("a crash mid-restore must never leave `path` without any link at all");
        assert_eq!(
            after_first_attempt,
            PathBuf::from("new.txt"),
            "the temp-then-rename shape must leave `path` showing its pre-restore link until the rename lands, or names the target it lost"
        );

        let report = reconcile(&journal, &g, &fixture).expect("reconciliation must run");
        assert!(
            report.reversed.contains(&id),
            "the retried reconciliation must fully reverse the link step, not {report:?}"
        );

        let restored = fixture
            .read_link(Path::new("/root/skill-current"))
            .expect("read the restored link");
        assert_eq!(
            restored,
            PathBuf::from("old.txt"),
            "reconciliation must restore the previous target, or name the target it lost"
        );

        let after = tree_snapshot(&fixture, &root_path);
        assert_eq!(
            after, pristine,
            "a fully retried reversal must leave no leaked temp entry behind, or names the extra entry"
        );
    }

    /// Given a link restore left mid-flight by a process that crashed
    /// between minting its temp symlink and renaming it over `path` - the
    /// same window as the test above - when a *fresh* process retries
    /// reconciliation, seeded with the exact leaked temp entry a crashed
    /// first mint (this process's own `.<leaf>-0`, or another process
    /// reusing the same leaf) would have left, then the retry still
    /// converges: it clears every stale entry under that leaf's temp prefix
    /// before minting its own, rather than failing `AlreadyExists` and
    /// leaving `path` stuck on the plan's new target; on failure the panic
    /// names the temp link it collided with.
    #[test]
    fn a_link_restore_retried_by_a_fresh_process_converges_or_names_the_temp_link_it_collided_with(
    ) {
        let fixture = FixtureBuilder::new()
            .dir("/journal")
            .dir("/root")
            .file("/root/old.txt", b"old")
            .file("/root/new.txt", b"new")
            .build_fs();
        let root_path = PathBuf::from("/root");
        let journal = journal_over(Arc::new(fixture.clone()));
        let lease = FakeLease::default();
        let g = guard(&lease);

        // Seed an existing link pointing at `old.txt`, fully committed.
        let seed_root = Root::open(&fixture, root_path.clone()).expect("open root");
        let seed_id = PlanId("01PLANLINKCOLLSEED0000001".into());
        let seed_plan = PlanWriter::begin(
            &journal,
            &g,
            seed_id,
            Utc::now(),
            "seed the existing link",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin seed plan");
        fsops::link(
            &seed_root,
            &seed_plan,
            Path::new("skill-current"),
            Path::new("old.txt"),
        )
        .expect("seed link");
        seed_plan
            .finish(PlanStatus::Done)
            .expect("finish seed plan");

        let pristine = tree_snapshot(&fixture, &root_path);

        // A second plan replaces the link with one pointing at `new.txt`,
        // fully landing, then is left `Pending` - the plan a fresh process
        // finds and retries reversing on.
        let root = Root::open(&fixture, root_path.clone()).expect("open root");
        let id = PlanId("01PLANLINKCOLL0000000001".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            id.clone(),
            Utc::now(),
            "replace the link, then crash before finish",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin");
        fsops::link(
            &root,
            &plan,
            Path::new("skill-current"),
            Path::new("new.txt"),
        )
        .expect("link");
        drop(plan);

        // Simulates the leaked temp symlink a first reversal attempt left
        // behind after minting it but before the rename that would have
        // consumed it: the exact name a fresh process's first mint of this
        // formula (`fsops::unique_suffix`, pid-then-counter) would produce -
        // this test's own throwaway call names the value only to keep the
        // fixture path visibly derived from that same formula, not to
        // predict what the retry below will itself mint.
        let leaked_temp = root_path.join(format!(".skill-current-{}", fsops::unique_suffix()));
        fixture
            .fsops_symlink(Path::new("old.txt"), &leaked_temp)
            .expect("seed the leaked temp symlink a crashed first attempt left behind");

        let report = reconcile(&journal, &g, &fixture).expect("reconciliation must run");
        assert!(
            report.reversed.contains(&id),
            "a fresh process's retried reversal must still converge despite the name collision, not {report:?}"
        );

        let restored = fixture
            .read_link(Path::new("/root/skill-current"))
            .expect("read the restored link");
        assert_eq!(
            restored,
            PathBuf::from("old.txt"),
            "reconciliation must restore the previous target despite the collision, or name the temp link it collided with"
        );

        let after = tree_snapshot(&fixture, &root_path);
        assert_eq!(
            after, pristine,
            "a converged retry must leave no leaked temp entry behind, or name the temp link it collided with"
        );
    }

    /// Given a step recorded before its mutation ran, when the mutation
    /// itself then fails and never lands - the crash landing in the window
    /// `record_*` guarantees now exists between "recorded" and "mutated" -
    /// then startup reconciliation must still resolve the plan, doing
    /// nothing to a tree that already matches the pre-plan snapshot; on
    /// failure the panic names the step reversal undid a second time
    /// (proof it was not the no-op reversing an unlanded step must be).
    #[test]
    fn a_step_recorded_before_a_mutation_that_never_landed_reverses_to_a_no_op_or_names_the_step_it_undid_twice(
    ) {
        let fixture = FixtureBuilder::new()
            .dir("/journal")
            .dir("/root")
            .build_fs();
        let root_path = PathBuf::from("/root");
        let before = tree_snapshot(&fixture, &root_path);

        let failing = FailingFs::wrap(Arc::new(fixture.clone()));
        let root = Root::open(&failing, root_path.clone()).expect("open root");
        let journal = journal_over(Arc::new(fixture.clone()));
        let lease = FakeLease::default();
        let g = guard(&lease);

        let id = PlanId("01PLANNEVERLANDED000000001".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            id.clone(),
            Utc::now(),
            "step recorded, mutation never lands",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin");

        // `newskill` does not exist yet, so `swap` takes its fresh-target
        // branch: no quarantine, just a rename of the staged folder into
        // place. That rename is the one made to fail, so the only trace
        // left behind is the staged folder itself - which the paired
        // `Stage` step's own (unconditional) reversal already removes,
        // proving the no-op holds even with two steps recorded and only
        // one of them mutating anything.
        let staged = fsops::stage(
            &root,
            &plan,
            &[(PathBuf::from("SKILL.md"), b"new content".to_vec())],
        )
        .expect("stage");
        failing.fail_next_fsops_rename();
        fsops::swap(&root, &plan, Path::new("newskill"), &staged, Path::new(".trash"))
            .expect_err("the rename must fail, or this test proves nothing about a step whose mutation never lands");
        drop(plan);

        let report =
            reconcile(&journal, &g, &fixture).expect("reconciliation must run without error");
        assert!(
            report.reversed.contains(&id),
            "a step recorded but never landed must still resolve as a (no-op) reversal, not {report:?}"
        );

        let after = tree_snapshot(&fixture, &root_path);
        assert_eq!(
            after, before,
            "reversing a step whose mutation never landed must be a no-op, or name the step it undid twice"
        );
    }

    /// Given a plan row whose one recorded step names a path outside the
    /// plan's own root - the shape a corrupt or hand-edited `plan.json`
    /// would take - when reconciliation runs, then it never touches that
    /// path, marks the plan `Interrupted`, and names the escaped path in
    /// the report; on failure the panic names whichever assertion the
    /// escape slipped past.
    #[test]
    fn reconcile_never_touches_a_path_outside_the_plan_root_or_names_the_escaped_path() {
        let fixture = FixtureBuilder::new()
            .dir("/journal")
            .dir("/root")
            .dir("/outside")
            .file("/outside/real.txt", b"do not touch")
            .build_fs();
        let root_path = PathBuf::from("/root");
        let journal = journal_over(Arc::new(fixture.clone()));
        let lease = FakeLease::default();
        let g = guard(&lease);

        let escaped = PathBuf::from("/outside/real.txt");
        let id = PlanId("01PLANESCAPE000000000001".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            id.clone(),
            Utc::now(),
            "hand-edited row naming a path outside the root",
            root_path,
            Vec::new(),
        )
        .expect("begin");
        // Bypasses `fsops::stage`'s own confinement on purpose: this
        // simulates a plan row a crash or a bug left pointing outside its
        // root, not a step `fsops` itself would ever record.
        plan.record_stage(&escaped)
            .expect("record the escaping step");
        drop(plan);

        let report = reconcile(&journal, &g, &fixture).expect("reconciliation must run");
        let interrupted = report
            .interrupted
            .iter()
            .find(|p| p.id == id)
            .unwrap_or_else(|| panic!("plan {} must be Interrupted, not {report:?}", id.0));
        assert!(
            interrupted
                .error
                .contains(&escaped.to_string_lossy().into_owned()),
            "the report must name the escaped path, got: {}",
            interrupted.error
        );

        assert_eq!(
            fixture
                .read_capped(&escaped, u64::MAX)
                .expect("the file outside the root must be untouched"),
            b"do not touch",
            "reconciliation must never touch a path outside the plan root"
        );
    }

    /// Given two plans stacked on the same folder - plan A swaps `skill`
    /// from `old0` to `new1` (quarantining `old0`) and is left `Pending`;
    /// plan B then swaps `skill` again, from `new1` to `new2`
    /// (quarantining `new1`), and also crashes `Pending` - when startup
    /// reconciliation runs, then it reverses B before A (LIFO), restoring
    /// `skill` to `old0` byte-for-byte; on failure the panic names the
    /// generation `skill` was left at (oldest-first strands `old0` in A's
    /// own quarantine folder and leaves `skill` at `new1` instead).
    #[test]
    fn stacked_pending_plans_reverse_newest_first_or_names_the_folder_left_at_the_wrong_generation()
    {
        let fixture = FixtureBuilder::new()
            .dir("/journal")
            .dir("/root")
            .dir("/root/skill")
            .file("/root/skill/SKILL.md", b"old0")
            .build_fs();
        let root_path = PathBuf::from("/root");
        let before = tree_snapshot(&fixture, &root_path);

        let journal = journal_over(Arc::new(fixture.clone()));
        let lease = FakeLease::default();
        let g = guard(&lease);
        let root = Root::open(&fixture, root_path.clone()).expect("open root");

        let plan_a = PlanId("01PLANSTACKA0000000000001".into());
        let a = PlanWriter::begin(
            &journal,
            &g,
            plan_a.clone(),
            Utc::now(),
            "plan A: old0 -> new1, left Pending",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin A");
        let staged_a = fsops::stage(&root, &a, &[(PathBuf::from("SKILL.md"), b"new1".to_vec())])
            .expect("stage A");
        fsops::swap(
            &root,
            &a,
            Path::new("skill"),
            &staged_a,
            Path::new(".trash-a"),
        )
        .expect("swap A over the pre-plan folder");
        drop(a);

        let plan_b = PlanId("01PLANSTACKB0000000000002".into());
        let b = PlanWriter::begin(
            &journal,
            &g,
            plan_b.clone(),
            Utc::now(),
            "plan B: new1 -> new2, then crashes",
            root_path.clone(),
            Vec::new(),
        )
        .expect("begin B");
        let staged_b = fsops::stage(&root, &b, &[(PathBuf::from("SKILL.md"), b"new2".to_vec())])
            .expect("stage B");
        fsops::swap(
            &root,
            &b,
            Path::new("skill"),
            &staged_b,
            Path::new(".trash-b"),
        )
        .expect("swap B over plan A's result");
        drop(b);

        let report = reconcile(&journal, &g, &fixture).expect("reconciliation must run");
        assert!(
            report.reversed.contains(&plan_a) && report.reversed.contains(&plan_b),
            "both stacked plans must resolve as Reversed, not {report:?}"
        );

        let after = tree_snapshot(&fixture, &root_path);
        assert_eq!(
            after, before,
            "reversing newest-first must restore `skill` to old0 (the true pre-plan-A snapshot), or names the folder left at the wrong generation"
        );
    }

    /// Recursively reads every file's and symlink's content under `root`,
    /// keyed by path, so a test can compare a tree before and after some
    /// operation without typing literal paths into the assertion.
    fn tree_snapshot(
        fs: &dyn ScopeFs,
        root: &Path,
    ) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        let mut out = std::collections::BTreeMap::new();
        collect_tree(fs, root, &mut out);
        out
    }

    fn collect_tree(
        fs: &dyn ScopeFs,
        dir: &Path,
        out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>,
    ) {
        let mut entries = fs.read_dir(dir).expect("read_dir");
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        for entry in entries {
            let path = dir.join(&entry.name);
            match entry.kind {
                FileKind::Dir => collect_tree(fs, &path, out),
                FileKind::File => {
                    let bytes = fs.read_capped(&path, u64::MAX).expect("read file");
                    out.insert(path, bytes);
                }
                FileKind::Symlink => {
                    let target = fs.read_link(&path).expect("read_link");
                    out.insert(path, target.to_string_lossy().into_owned().into_bytes());
                }
                FileKind::Other => {}
            }
        }
    }

    /// Given a backup set with one entry older than the age quota, one
    /// entry that pushes the kept total over the size quota, and one recent,
    /// small entry that must survive, when `trim_backups` runs, then only
    /// the survivor is left; on failure the panic names whichever stale or
    /// oversized entry survived instead.
    #[test]
    fn a_backup_set_over_the_size_or_age_quota_is_trimmed_oldest_first_or_names_the_surviving_stale_entry(
    ) {
        let fs: Arc<dyn ScopeFs> = Arc::new(FixtureBuilder::new().dir("/journal").build_fs());
        let journal = journal_over(fs.clone());
        let lease = FakeLease::default();
        let g = guard(&lease);
        let now = Utc::now();

        let stale_id = PlanId("01PLAN0000000000000000005".into());
        write_backup(&fs, &stale_id, "stale.bin", b"01");
        begin_with_backups(
            &journal,
            &g,
            stale_id.clone(),
            now - chrono::Duration::days(30),
            vec![PlanBackupEntry {
                original: PathBuf::from("/root/stale"),
                relative: "stale.bin".into(),
                bytes: 2,
            }],
        );

        let oversized_id = PlanId("01PLAN0000000000000000006".into());
        write_backup(&fs, &oversized_id, "big.bin", &[0u8; 100]);
        begin_with_backups(
            &journal,
            &g,
            oversized_id.clone(),
            now - chrono::Duration::minutes(5),
            vec![PlanBackupEntry {
                original: PathBuf::from("/root/big"),
                relative: "big.bin".into(),
                bytes: 100,
            }],
        );

        let survivor_id = PlanId("01PLAN0000000000000000007".into());
        write_backup(&fs, &survivor_id, "small.bin", b"x");
        begin_with_backups(
            &journal,
            &g,
            survivor_id.clone(),
            now - chrono::Duration::minutes(1),
            vec![PlanBackupEntry {
                original: PathBuf::from("/root/small"),
                relative: "small.bin".into(),
                bytes: 1,
            }],
        );

        let quota = BackupQuota {
            max_total_bytes: 10,
            max_age: Duration::from_secs(3600),
        };
        let trimmed = trim_backups(&journal, &g, now, &quota).expect("trim must run");
        let trimmed_ids: Vec<&PlanId> = trimmed.iter().map(|t| &t.plan).collect();

        assert!(
            trimmed_ids.contains(&&stale_id),
            "the entry over the age quota must be trimmed"
        );
        assert!(
            trimmed_ids.contains(&&oversized_id),
            "the entry that pushes the kept total over the size quota must be trimmed"
        );
        assert!(
            !trimmed_ids.contains(&&survivor_id),
            "surviving stale entry: {survivor_id:?} was trimmed but should have stayed within quota"
        );
        assert!(
            fs.symlink_metadata(Path::new(&format!(
                "/journal/plans/{}/backups/small.bin",
                survivor_id.0
            )))
            .is_ok(),
            "the survivor's bytes must remain on disk"
        );
    }

    fn write_backup(fs: &Arc<dyn ScopeFs>, id: &PlanId, relative: &str, bytes: &[u8]) {
        let dir = PathBuf::from(format!("/journal/plans/{}/backups", id.0));
        let mut built = PathBuf::from("/journal");
        for component in dir.strip_prefix("/journal").unwrap().components() {
            built.push(component);
            let _ = fs.fsops_create_dir(&built);
        }
        fs.fsops_write_new_file(&dir.join(relative), bytes)
            .expect("write backup bytes");
    }

    fn begin_with_backups(
        journal: &FsJournal,
        g: &ExclusiveGuard,
        id: PlanId,
        created_at: DateTime<Utc>,
        backups: Vec<PlanBackupEntry>,
    ) {
        let plan = PlanWriter::begin(
            journal,
            g,
            id,
            created_at,
            "backup fixture",
            PathBuf::from("/root"),
            backups,
        )
        .expect("begin");
        plan.finish(PlanStatus::Done).expect("finish");
    }
}
