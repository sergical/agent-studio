//! `ops::doctor`: one read-only pass over every lifecycle invariant named
//! in `docs/action-map/lifecycle-states.md`'s Invariants section, over the
//! whole scope the running `Runtime` knows - not one skill at a time, the
//! way `ops::fix_skill` runs invariants 1-5 scoped to a single skill while
//! repairing what it can. `doctor` repairs nothing; it only names what it
//! finds, so a caller running it at startup or on demand never mutates
//! disk.
//!
//! Invariants 1-4 are checked once per skill the scan found, over the
//! `Diagnosis`'s own `Inventory` (never a second `scan`, matching
//! `fix_skill`'s reuse of `diagnose`'s inventory). Invariant 5 (quarantine)
//! and invariant 6 (journal) each check one thing regardless of skill
//! count, so they run once each rather than once per skill.
//!
//! The journal is constructed here rather than threaded through `Ports`,
//! matching the shared brief's correction: `Ports` carries no `Journal`
//! field today, and `check_journal_has_no_open_plan` only needs to read
//! it, so `FsJournal::new(journal_root, fs)` at the read site is the
//! smaller seam than widening `Ports` for every caller.

use crate::doctor::{
    check_journal_has_no_open_plan, check_link_resolves_in_root, check_lockfile_entry_has_folder,
    check_no_folder_in_two_states, check_quarantine_within_cap, check_registry_entry_has_folder,
};
use crate::dto::{DoctorReport, DoctorRequest, DoctorViolation, ScanRequest};
use crate::error::CoreError;
use crate::journal::FsJournal;
use crate::ops::diagnose;
use crate::ops_install::journal_root;
use crate::ports::{OpContext, Runtime};

/// Runs the six `doctor` invariant checks over the whole scope and returns
/// every violation found, with no repair attempted.
///
/// Preconditions: same as [`crate::ops::scan`] (shared lease, taken and
/// released by [`diagnose`]'s own [`crate::ops::scan`] call - matching
/// `ops::fix_skill`, this op reads `fs` directly afterward rather than
/// holding a second lease of its own, since nothing here writes).
pub fn doctor(
    rt: &Runtime,
    ctx: &OpContext,
    _req: &DoctorRequest,
) -> Result<DoctorReport, CoreError> {
    ctx.checkpoint()?;
    let diagnosis = diagnose(rt, ctx, &ScanRequest::default())?;

    let fs = rt.ports.fs.as_ref();
    let home = &rt.scope.home.lexical;
    let skill_names: Vec<_> = diagnosis
        .inventory
        .skills
        .iter()
        .map(|skill| skill.name.clone())
        .collect();

    let journal = FsJournal::new(journal_root(home), rt.ports.fs.clone());

    let violations: Vec<DoctorViolation> = check_link_resolves_in_root(&diagnosis)
        .into_iter()
        .chain(check_registry_entry_has_folder(fs, home))
        .chain(check_lockfile_entry_has_folder(
            fs,
            home,
            &diagnosis.inventory,
        ))
        .chain(check_no_folder_in_two_states(
            &diagnosis.inventory,
            &skill_names,
        ))
        .chain(check_quarantine_within_cap(fs, home))
        .chain(check_journal_has_no_open_plan(&journal))
        .map(|violation| DoctorViolation {
            invariant: violation.invariant,
            path: violation.path,
            detail: violation.message,
        })
        .collect();

    ctx.take_timing();
    Ok(DoctorReport {
        violations,
        checked: skill_names.len() as u32,
    })
}
