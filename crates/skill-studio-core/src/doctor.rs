//! The six doctor invariants named in `docs/action-map/lifecycle-states.md`'s
//! Invariants section, checked independently of any single command's own
//! rollback logic so a violation left by an old bug or a manual edit
//! surfaces even when no command is running. [`ops::fix_skill`] runs every
//! check and applies whichever repair exists; anything it cannot repair is
//! returned as a [`DoctorViolation`] naming the path.
//!
//! Invariants 1 ([`link resolves inside its
//! root`](DoctorInvariant::LinkResolvesInRoot)), 5
//! ([`QuarantineWithinCap`](DoctorInvariant::QuarantineWithinCap)) and 6
//! ([`JournalHasNoOpenPlan`](DoctorInvariant::JournalHasNoOpenPlan)) have a
//! repair the core can run safely on its own. Invariants 2-4 are
//! detect-only here: [`crate::ownership::HomeRegistry`] and
//! [`crate::lock_file::SkillLockFile`] are partial views of documents whose
//! full shape (trials, parked records, packs, ...) core does not own -
//! `crate::registry`'s own doc comment says as much - so writing either
//! back from here would silently drop fields core never read. Repairing
//! those two is `ops::fix_skill`'s deferred follow-up; this module still
//! detects and names them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::dto::{Diagnosis, IssueKind};
use crate::identity::SkillName;
use crate::lock_file::{lock_file_path, read_lock_file, SkillLockFile};
use crate::ownership::read_home_registry;
use crate::ports::{Journal, ScopeFs};

/// Relative path of the universal shared skills root under a scope home.
const UNIVERSAL_SKILLS_RELATIVE: &str = ".agents/skills";
/// Relative path of the parked holding directory under a scope home.
const PARKED_RELATIVE: &str = ".agents/skills-parked";
/// Quarantine holding directory under the universal root, per
/// `docs/action-map/primitives-and-call-stack.md`'s `<root>/.skill-studio-quarantine/<id>` convention.
const QUARANTINE_DIR_NAME: &str = ".skill-studio-quarantine";
/// Retention cap for quarantine entries under the universal root. Chosen as
/// a round number generous enough for normal use; not measured against
/// production quarantine growth, so `ops::fix_skill`'s follow-up list names
/// tuning it as unmeasured.
pub const QUARANTINE_RETENTION_CAP: usize = 20;

/// One of the six doctor invariants from `lifecycle-states.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DoctorInvariant {
    /// 1. Every link resolves inside its root.
    LinkResolvesInRoot,
    /// 2. Every registry entry has a folder.
    RegistryEntryHasFolder,
    /// 3. Every lockfile entry has a folder.
    LockfileEntryHasFolder,
    /// 4. No folder is in two states at once.
    NoFolderInTwoStates,
    /// 5. Quarantine stays within its retention cap.
    QuarantineWithinCap,
    /// 6. The journal has no open plan at rest.
    JournalHasNoOpenPlan,
}

/// One violation of a doctor invariant, named with the offending path so a
/// caller that cannot repair it automatically can still tell the user
/// where to look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorViolation {
    /// Which invariant is violated.
    pub invariant: DoctorInvariant,
    /// Skill the violation belongs to, when it names one.
    pub skill: Option<SkillName>,
    /// Path of the offending entry, when the invariant names one.
    pub path: PathBuf,
    /// Message for a person.
    pub message: String,
}

/// Invariant 1: every link resolves inside its root. Reuses `diagnose`'s
/// own `BrokenLink` detection rather than re-deriving it, so this check and
/// `diagnose`'s never disagree by construction.
pub fn check_link_resolves_in_root(diagnosis: &Diagnosis) -> Vec<DoctorViolation> {
    diagnosis
        .issues
        .iter()
        .filter(|issue| issue.kind == IssueKind::BrokenLink)
        .map(|issue| DoctorViolation {
            invariant: DoctorInvariant::LinkResolvesInRoot,
            skill: Some(issue.skill.clone()),
            path: PathBuf::new(),
            message: issue.message.clone(),
        })
        .collect()
}

/// Invariant 2: every registry (`skill-studio.json` `forks`/`copies`)
/// entry has a folder at the path it names. Detect-only; see the module
/// doc comment for why.
pub fn check_registry_entry_has_folder(fs: &dyn ScopeFs, home: &Path) -> Vec<DoctorViolation> {
    let registry = read_home_registry(fs, home);
    let mut violations = Vec::new();
    for (name, record) in &registry.copies {
        if fs.symlink_metadata(&record.path).is_err() {
            violations.push(DoctorViolation {
                invariant: DoctorInvariant::RegistryEntryHasFolder,
                skill: Some(SkillName(name.clone())),
                path: record.path.clone(),
                message: format!(
                    "registry copy entry `{name}` names {} but no folder is there",
                    record.path.display()
                ),
            });
        }
    }
    for (name, record) in &registry.forks {
        let path = if record.skill_dir.as_os_str().is_empty() {
            home.join(UNIVERSAL_SKILLS_RELATIVE).join(name)
        } else {
            record.skill_dir.clone()
        };
        if fs.symlink_metadata(&path).is_err() {
            violations.push(DoctorViolation {
                invariant: DoctorInvariant::RegistryEntryHasFolder,
                skill: Some(SkillName(name.clone())),
                path: path.clone(),
                message: format!(
                    "registry fork entry `{name}` names {} but no folder is there",
                    path.display()
                ),
            });
        }
    }
    violations
}

/// Invariant 3: every `~/.agents/.skill-lock.json` entry has a folder under
/// the universal skills root. Detect-only; see the module doc comment.
pub fn check_lockfile_entry_has_folder(fs: &dyn ScopeFs, home: &Path) -> Vec<DoctorViolation> {
    let lock: SkillLockFile = read_lock_file(fs, &lock_file_path(home)).unwrap_or(SkillLockFile {
        version: 3,
        skills: HashMap::new(),
    });
    lock.skills
        .keys()
        .filter_map(|name| {
            let path = home.join(UNIVERSAL_SKILLS_RELATIVE).join(name);
            (fs.symlink_metadata(&path).is_err()).then(|| DoctorViolation {
                invariant: DoctorInvariant::LockfileEntryHasFolder,
                skill: Some(SkillName(name.clone())),
                path: path.clone(),
                message: format!(
                    "lockfile entry `{name}` names {} but no folder is there",
                    path.display()
                ),
            })
        })
        .collect()
}

/// Invariant 4: no folder is in two states at once - here, installed and
/// parked simultaneously, which the lifecycle table (`park_skill`) says
/// never happens once a park lands cleanly. Detect-only; there is no single
/// safe automatic choice between the two states.
pub fn check_no_folder_in_two_states(
    fs: &dyn ScopeFs,
    home: &Path,
    skill_names: &[SkillName],
) -> Vec<DoctorViolation> {
    skill_names
        .iter()
        .filter_map(|name| {
            let installed = home.join(UNIVERSAL_SKILLS_RELATIVE).join(&name.0);
            let parked = home.join(PARKED_RELATIVE).join(&name.0);
            let both =
                fs.symlink_metadata(&installed).is_ok() && fs.symlink_metadata(&parked).is_ok();
            both.then(|| DoctorViolation {
                invariant: DoctorInvariant::NoFolderInTwoStates,
                skill: Some(name.clone()),
                path: installed.clone(),
                message: format!(
                    "`{}` exists both installed at {} and parked at {}",
                    name.0,
                    installed.display(),
                    parked.display()
                ),
            })
        })
        .collect()
}

/// The universal root's quarantine holding directory.
fn quarantine_dir(home: &Path) -> PathBuf {
    home.join(UNIVERSAL_SKILLS_RELATIVE)
        .join(QUARANTINE_DIR_NAME)
}

/// Invariant 5: quarantine stays within [`QUARANTINE_RETENTION_CAP`].
pub fn check_quarantine_within_cap(fs: &dyn ScopeFs, home: &Path) -> Vec<DoctorViolation> {
    let dir = quarantine_dir(home);
    let entries = fs.read_dir(&dir).unwrap_or_default();
    if entries.len() > QUARANTINE_RETENTION_CAP {
        vec![DoctorViolation {
            invariant: DoctorInvariant::QuarantineWithinCap,
            skill: None,
            path: dir.clone(),
            message: format!(
                "{} entries in {} exceed the retention cap of {}",
                entries.len(),
                dir.display(),
                QUARANTINE_RETENTION_CAP
            ),
        }]
    } else {
        Vec::new()
    }
}

/// Repairs invariant 5 by removing the oldest entries (by name, which
/// carries each entry's creation-order suffix) until the cap holds.
/// Returns how many entries were removed.
pub fn repair_quarantine_within_cap(fs: &dyn ScopeFs, home: &Path) -> std::io::Result<usize> {
    let dir = quarantine_dir(home);
    let mut entries = fs.read_dir(&dir)?;
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let mut removed = 0;
    while entries.len() > QUARANTINE_RETENTION_CAP {
        let victim = entries.remove(0);
        fs.fsops_remove_dir(&dir.join(&victim.name))?;
        removed += 1;
    }
    Ok(removed)
}

/// Invariant 6: the journal has no open plan at rest.
pub fn check_journal_has_no_open_plan(journal: &dyn Journal) -> Vec<DoctorViolation> {
    journal
        .pending()
        .unwrap_or_default()
        .into_iter()
        .map(|plan| DoctorViolation {
            invariant: DoctorInvariant::JournalHasNoOpenPlan,
            skill: None,
            path: plan.root.clone(),
            message: format!("plan {} ({}) is still open", plan.id.0, plan.label),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureBuilder;

    const HOME: &str = "/home";

    fn home() -> PathBuf {
        PathBuf::from(HOME)
    }

    #[test]
    fn quarantine_over_cap_is_flagged_and_repair_trims_it_to_the_cap() {
        let mut builder = FixtureBuilder::new().dir(&format!("{HOME}/{UNIVERSAL_SKILLS_RELATIVE}"));
        for i in 0..(QUARANTINE_RETENTION_CAP + 3) {
            builder = builder.dir(&format!(
                "{HOME}/{UNIVERSAL_SKILLS_RELATIVE}/{QUARANTINE_DIR_NAME}/{i:04}-quarantined"
            ));
        }
        let fs = builder.build_fs();

        let violations = check_quarantine_within_cap(&fs, &home());
        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[0].invariant,
            DoctorInvariant::QuarantineWithinCap
        );

        let removed = repair_quarantine_within_cap(&fs, &home()).unwrap();
        assert_eq!(removed, 3);
        assert!(check_quarantine_within_cap(&fs, &home()).is_empty());
    }

    #[test]
    fn lockfile_entry_with_no_folder_is_flagged_and_clears_once_the_folder_exists() {
        let fs = FixtureBuilder::new()
            .dir(&format!("{HOME}/{UNIVERSAL_SKILLS_RELATIVE}"))
            .file(
                &format!("{HOME}/.agents/.skill-lock.json"),
                br#"{"version":3,"skills":{"ghost-skill":{"source":"o/r","sourceType":"github","sourceUrl":"https://example.com","skillFolderHash":"abc","installedAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z"}}}"#,
            )
            .build_fs();

        let violations = check_lockfile_entry_has_folder(&fs, &home());
        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[0].invariant,
            DoctorInvariant::LockfileEntryHasFolder
        );

        // The "repair" this invariant has today is naming the path so the
        // user (or a follow-up unit) can restore the folder or drop the
        // stale entry; simulating that restoration here proves the check
        // clears once it happens, without core writing the partial
        // document itself (see the module doc comment).
        fs.fsops_create_dir(&home().join(UNIVERSAL_SKILLS_RELATIVE).join("ghost-skill"))
            .unwrap();
        assert!(check_lockfile_entry_has_folder(&fs, &home()).is_empty());
    }

    #[test]
    fn skill_parked_and_installed_at_once_is_flagged_and_clears_once_one_state_is_removed() {
        let name = SkillName("double-state".to_string());
        let fs = FixtureBuilder::new()
            .dir(&format!("{HOME}/{UNIVERSAL_SKILLS_RELATIVE}/double-state"))
            .dir(&format!("{HOME}/{PARKED_RELATIVE}/double-state"))
            .build_fs();

        let violations = check_no_folder_in_two_states(&fs, &home(), std::slice::from_ref(&name));
        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[0].invariant,
            DoctorInvariant::NoFolderInTwoStates
        );

        fs.fsops_remove_dir(&home().join(PARKED_RELATIVE).join(&name.0))
            .unwrap();
        assert!(check_no_folder_in_two_states(&fs, &home(), &[name]).is_empty());
    }

    #[test]
    fn stale_registry_copy_entry_is_flagged_and_clears_once_the_folder_exists() {
        let expected_path = format!("{HOME}/{UNIVERSAL_SKILLS_RELATIVE}/stale-copy");
        let registry = format!(
            r#"{{"copies":{{"stale-copy":{{"name":"stale-copy","path":"{expected_path}","scope":"global","destination":"universal"}}}}}}"#
        );
        let fs = FixtureBuilder::new()
            .dir(&format!("{HOME}/{UNIVERSAL_SKILLS_RELATIVE}"))
            .file(
                &format!("{HOME}/.agents/skill-studio.json"),
                registry.as_bytes(),
            )
            .build_fs();

        let violations = check_registry_entry_has_folder(&fs, &home());
        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[0].invariant,
            DoctorInvariant::RegistryEntryHasFolder
        );

        fs.fsops_create_dir(&home().join(UNIVERSAL_SKILLS_RELATIVE).join("stale-copy"))
            .unwrap();
        assert!(check_registry_entry_has_folder(&fs, &home()).is_empty());
    }

    #[test]
    fn open_journal_plan_is_flagged_and_reconcile_clears_it() {
        use std::sync::Arc;
        use std::time::Duration;

        use chrono::Utc;

        use crate::identity::PlanId;
        use crate::journal::{FsJournal, PlanWriter};
        use crate::ports::{ExclusiveGuard, Journal, LeaseMode, LeaseProvider};
        use crate::testing::FakeLease;

        let fs: Arc<dyn ScopeFs> = Arc::new(
            FixtureBuilder::new()
                .dir(&format!("{HOME}/{UNIVERSAL_SKILLS_RELATIVE}"))
                .dir("/journal")
                .build_fs(),
        );
        let journal = FsJournal::new(PathBuf::from("/journal"), fs.clone());
        let lease = FakeLease::default();
        let handle = lease
            .acquire(&[], LeaseMode::Exclusive, Duration::from_secs(0))
            .unwrap();
        let guard = ExclusiveGuard::from_handle(handle);

        let plan = PlanWriter::begin(
            &journal,
            &guard,
            PlanId("01PLANDOCTOR00000000001".into()),
            Utc::now(),
            "doctor test plan",
            home().join(UNIVERSAL_SKILLS_RELATIVE),
            Vec::new(),
        )
        .unwrap();
        let id = plan.id().clone();
        assert!(!check_journal_has_no_open_plan(&journal).is_empty());

        crate::journal::reconcile(&journal, &guard, fs.as_ref()).unwrap();
        assert!(check_journal_has_no_open_plan(&journal).is_empty());
        assert!(journal.all().unwrap().iter().any(|p| p.id == id));
    }
}
