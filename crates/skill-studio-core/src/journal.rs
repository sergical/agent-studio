//! Journal: the crash-safety primitive every `fsops` call records against.
//!
//! A plan's manifest and the plan itself are fsynced to disk, through
//! [`Journal::begin`], before the plan's first step runs - see
//! `docs/action-map/plan.md` unit 1.2. [`FsJournal`] is the reference
//! [`Journal`] implementation, built only on [`ScopeFs`] so a host can reuse
//! it verbatim rather than reimplementing the write order. [`reconcile`]
//! resolves every plan a crash left `Pending`, and [`trim_backups`] enforces
//! a size-and-age quota on the backups plans keep for undo. Neither one ever
//! deletes a plan row.
//!
//! [`journaled_stage`], [`journaled_swap`], [`journaled_link`], and
//! [`journaled_write_file`] wrap [`crate::fsops`]'s four primitives so a
//! caller that goes through them cannot call a primitive without also
//! recording the step that ran.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::error::{CoreError, ErrorCode};
use crate::fsops::{self, FsOpsError, ReadStamp, Root, Staged};
use crate::identity::PlanId;
use crate::ports::{
    ExclusiveGuard, FileKind, Journal, PlanBackupEntry, PlanRecord, PlanStatus, PlanStep, ScopeFs,
};

/// Caps how many bytes [`FsJournal`] will read back for one plan or manifest
/// file; a journal file this large is corrupt, not merely large.
const MAX_JOURNAL_JSON_BYTES: u64 = 16 * 1024 * 1024;

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

    /// Writes `bytes` to `path` through a temp file, fsync, and rename, so
    /// `path` only ever shows a complete write - the same durability
    /// `fsops::write_file` gives a caller's own files.
    fn write_json(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let parent = path.parent().unwrap_or(path);
        self.ensure_dir(parent)?;
        let tmp = path.with_extension("json.tmp");
        if self.fs.symlink_metadata(&tmp).is_ok() {
            self.fs.fsops_remove_file(&tmp)?;
        }
        self.fs.fsops_write_new_file(&tmp, bytes)?;
        self.fs.fsops_fsync_file(&tmp)?;
        self.fs.fsops_rename(&tmp, path)?;
        self.fs.fsops_fsync_dir(parent)
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

    /// Records that one `fsops` primitive ran against `path`.
    pub fn step(&self, name: &str, path: &Path) -> Result<(), CoreError> {
        self.journal.record_step(
            self.guard,
            &self.id,
            PlanStep {
                name: name.to_string(),
                path: path.to_path_buf(),
            },
        )
    }

    /// Sets the plan's final status. Never `Pending`.
    pub fn finish(self, status: PlanStatus) -> Result<(), CoreError> {
        self.journal.finish(self.guard, &self.id, status)
    }
}

/// Either an `fsops` primitive failed, or the journal call recording it did.
#[derive(Debug)]
pub enum JournalOpsError {
    /// The `fsops` primitive itself failed; nothing was journaled.
    FsOps(FsOpsError),
    /// The primitive ran but the journal step could not be recorded.
    Journal(CoreError),
}

impl fmt::Display for JournalOpsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JournalOpsError::FsOps(e) => write!(f, "{e}"),
            JournalOpsError::Journal(e) => write!(f, "ran but was not journaled: {e}"),
        }
    }
}

impl std::error::Error for JournalOpsError {}

/// Runs [`fsops::stage`], then records the step. See the module doc for why
/// callers that need every step journaled use this instead of calling
/// `fsops::stage` directly.
pub fn journaled_stage(
    plan: &PlanWriter<'_>,
    root: &Root<'_>,
    contents: &[(PathBuf, Vec<u8>)],
) -> Result<Staged, JournalOpsError> {
    let staged = fsops::stage(root, contents).map_err(JournalOpsError::FsOps)?;
    plan.step("stage", staged.path())
        .map_err(JournalOpsError::Journal)?;
    Ok(staged)
}

/// Runs [`fsops::swap`], then records the step.
pub fn journaled_swap(
    plan: &PlanWriter<'_>,
    root: &Root<'_>,
    final_name: &Path,
    staged: Staged,
    quarantine_dir: &Path,
) -> Result<(), JournalOpsError> {
    fsops::swap(root, final_name, staged, quarantine_dir).map_err(JournalOpsError::FsOps)?;
    plan.step("swap", final_name)
        .map_err(JournalOpsError::Journal)
}

/// Runs [`fsops::link`], then records the step.
pub fn journaled_link(
    plan: &PlanWriter<'_>,
    root: &Root<'_>,
    name: &Path,
    target: &Path,
) -> Result<(), JournalOpsError> {
    fsops::link(root, name, target).map_err(JournalOpsError::FsOps)?;
    plan.step("link", name).map_err(JournalOpsError::Journal)
}

/// Runs [`fsops::write_file`], then records the step.
pub fn journaled_write_file(
    plan: &PlanWriter<'_>,
    root: &Root<'_>,
    name: &Path,
    bytes: &[u8],
    expected: &ReadStamp,
) -> Result<(), JournalOpsError> {
    fsops::write_file(root, name, bytes, expected).map_err(JournalOpsError::FsOps)?;
    plan.step("write_file", name)
        .map_err(JournalOpsError::Journal)
}

/// What startup reconciliation did to every plan it found `Pending`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconciliation {
    /// Plans that recorded at least one step before the crash: marked
    /// `Interrupted` and listed, since only the caller who began the plan
    /// knows how to finish or undo its partial work.
    pub interrupted: Vec<PlanId>,
    /// Plans that recorded no step at all: nothing had mutated anything
    /// yet, so the plan is resolved by marking it `Failed`, the same
    /// terminal state a plan that failed its own first step would reach.
    pub resolved_without_steps: Vec<PlanId>,
}

/// Resolves every plan [`Journal::pending`] still reports, never deleting a
/// row: a plan with recorded steps becomes `Interrupted` (listed for a
/// caller to inspect or restore), a plan with none becomes `Failed` (nothing
/// on disk needed undoing). Idempotent - a second call finds nothing left
/// `Pending`.
pub fn reconcile(
    journal: &dyn Journal,
    guard: &ExclusiveGuard,
) -> Result<Reconciliation, CoreError> {
    let mut report = Reconciliation::default();
    for plan in journal.pending()? {
        if plan.steps.is_empty() {
            journal.finish(guard, &plan.id, PlanStatus::Failed)?;
            report.resolved_without_steps.push(plan.id);
        } else {
            journal.finish(guard, &plan.id, PlanStatus::Interrupted)?;
            report.interrupted.push(plan.id);
        }
    }
    Ok(report)
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
    entries.sort_by(|a, b| a.0.cmp(&b.0));

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
    use crate::ports::{LeaseMode, LeaseProvider};
    use crate::testing::{FakeLease, FixtureBuilder};

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

        plan.step("write_file", Path::new("/root/skill/SKILL.md"))
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

    /// Given a plan begun and left with one recorded step (a simulated
    /// crash after a mid-plan step, before `finish` ran), when startup
    /// reconciliation runs, then the plan no longer reads `Pending` - it is
    /// resolved as `Interrupted`, not left open; on failure the panic names
    /// the plan still `Pending`.
    #[test]
    fn startup_reconciliation_after_a_simulated_crash_completes_or_reverses_every_pending_plan_or_names_the_plan_left_open(
    ) {
        let fs: Arc<dyn ScopeFs> = Arc::new(FixtureBuilder::new().dir("/journal").build_fs());
        let journal = journal_over(fs);
        let lease = FakeLease::default();
        let g = guard(&lease);

        let id = PlanId("01PLAN0000000000000000002".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            id.clone(),
            Utc::now(),
            "crashes mid-plan",
            PathBuf::from("/root"),
            Vec::new(),
        )
        .expect("begin");
        plan.step("stage", Path::new("/root/.stage-tmp"))
            .expect("record the one step that ran before the crash");
        // The process dies here: `finish` never runs, and `plan` (the
        // `PlanWriter`) is simply dropped rather than resolved.
        drop(plan);

        let report = reconcile(&journal, &g).expect("reconciliation must run");
        assert!(
            report.interrupted.contains(&id),
            "a plan with a recorded step must resolve as Interrupted, not stay open"
        );

        let after = journal
            .all()
            .expect("read")
            .into_iter()
            .find(|p| p.id == id)
            .unwrap_or_else(|| panic!("plan {} must still exist after reconciliation", id.0));
        assert_ne!(
            after.status,
            PlanStatus::Pending,
            "plan {} was left open (still Pending) after reconciliation",
            id.0
        );
    }

    /// Given two plans left `Pending` by a crash, one with a recorded step
    /// and one with none, when reconciliation runs, then both rows still
    /// exist afterward and both appear in the reconciliation report; on
    /// failure the panic names whichever row went missing.
    #[test]
    fn startup_reconciliation_never_deletes_a_row_and_lists_every_interrupted_plan_or_names_the_missing_row(
    ) {
        let fs: Arc<dyn ScopeFs> = Arc::new(FixtureBuilder::new().dir("/journal").build_fs());
        let journal = journal_over(fs);
        let lease = FakeLease::default();
        let g = guard(&lease);

        let with_step = PlanId("01PLAN0000000000000000003".into());
        let plan = PlanWriter::begin(
            &journal,
            &g,
            with_step.clone(),
            Utc::now(),
            "with a step",
            PathBuf::from("/root"),
            Vec::new(),
        )
        .expect("begin");
        plan.step("link", Path::new("/root/link")).expect("step");
        drop(plan);

        let without_step = PlanId("01PLAN0000000000000000004".into());
        drop(
            PlanWriter::begin(
                &journal,
                &g,
                without_step.clone(),
                Utc::now(),
                "with no step",
                PathBuf::from("/root"),
                Vec::new(),
            )
            .expect("begin"),
        );

        let report = reconcile(&journal, &g).expect("reconciliation must run");
        assert!(
            report.interrupted.contains(&with_step),
            "the plan with a recorded step must be listed as interrupted"
        );
        assert!(
            report.resolved_without_steps.contains(&without_step),
            "the plan with no recorded step must still be listed, resolved as Failed"
        );

        let after = journal.all().expect("read every row back");
        for id in [&with_step, &without_step] {
            assert!(
                after.iter().any(|p| &p.id == id),
                "row {} went missing after reconciliation",
                id.0
            );
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
