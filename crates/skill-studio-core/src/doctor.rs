//! The six doctor invariants named in `docs/action-map/lifecycle-states.md`'s
//! Invariants section, checked independently of any single command's own
//! rollback logic so a violation left by an old bug or a manual edit
//! surfaces even when no command is running. [`ops::fix_skill`] runs checks
//! 1-5 and applies whichever repair exists; anything it cannot repair is
//! returned as a [`DoctorViolation`] naming the path.
//!
//! Invariant 6 ([`JournalHasNoOpenPlan`](DoctorInvariant::JournalHasNoOpenPlan))
//! has a repair the core can run safely on its own
//! (`crate::journal::reconcile`, run at startup). Invariants 1-5 are
//! detect-only here: invariant 1's repair is the desktop's journaled
//! `repair_skill_link`, not duplicated in core; invariant 5's repair (prune
//! quarantine to its retention cap) is unit 3.9's, which owns "quarantine
//! with a retention cap" - pruning without a lease or a journal entry is not
//! safe to do from here. [`crate::ownership::HomeRegistry`] and
//! [`crate::lock_file::SkillLockFile`] are partial views of documents whose
//! full shape (trials, parked records, packs, ...) core does not own -
//! `crate::registry`'s own doc comment says as much - so writing either
//! back from here would silently drop fields core never read. Repairing
//! those two is `ops::fix_skill`'s deferred follow-up; this module still
//! detects and names them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::dto::{Diagnosis, Inventory, IssueKind};
use crate::identity::{BackingRelationship, DeploymentId, RootKind, SkillName};
use crate::lock_file::{lock_file_path, read_lock_file, SkillLockFile};
use crate::ownership::read_home_registry;
use crate::ports::{Journal, ScopeFs};

/// Relative path of the universal shared skills root under a scope home.
const UNIVERSAL_SKILLS_RELATIVE: &str = ".agents/skills";
/// Quarantine holding directory under the universal root, per
/// `docs/action-map/primitives-and-call-stack.md`'s `<root>/.skill-studio-quarantine/<id>` convention.
/// `pub(crate)`, not private: unit 3.9 owns the retention-cap repair and
/// reaches for this same directory name rather than redefining it.
pub(crate) const QUARANTINE_DIR_NAME: &str = ".skill-studio-quarantine";
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

/// Resolves `deployment_id` to its path via `inventory`, so a check can
/// name the real file instead of leaving `path` empty.
fn deployment_path(inventory: &Inventory, deployment_id: &DeploymentId) -> Option<PathBuf> {
    inventory
        .skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .find(|deployment| &deployment.id == deployment_id)
        .map(|deployment| deployment.path.clone())
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
            path: issue
                .deployment_id
                .as_ref()
                .and_then(|id| deployment_path(&diagnosis.inventory, id))
                .unwrap_or_default(),
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

/// Invariant 3: every `~/.agents/.skill-lock.json` entry has a folder
/// somewhere `inventory` scanned. Detect-only; see the module doc comment.
///
/// Resolved through `inventory`'s own deployments rather than
/// `fs.symlink_metadata(home.join(UNIVERSAL_SKILLS_RELATIVE).join(name))`
/// directly: a lockfile entry deployed only into one harness root (for
/// example `.claude/skills`, never linked into the universal root) is a
/// real folder, not a violation, and `inventory` already knows every root a
/// scan covered.
pub fn check_lockfile_entry_has_folder(
    fs: &dyn ScopeFs,
    home: &Path,
    inventory: &Inventory,
) -> Vec<DoctorViolation> {
    let lock: SkillLockFile = read_lock_file(fs, &lock_file_path(home)).unwrap_or(SkillLockFile {
        version: 3,
        skills: HashMap::new(),
    });
    lock.skills
        .into_keys()
        .filter(|name| {
            // A `LinkedTo` deployment is a link back to a `Canonical`/
            // `Independent` one; a dangling link's target is gone, so
            // counting it as "has a folder" would hide a stale lockfile
            // row behind the very link that no longer resolves.
            !inventory.skills.iter().any(|skill| {
                skill.name.0 == *name
                    && skill.deployments.iter().any(|deployment| {
                        matches!(
                            deployment.backing,
                            BackingRelationship::Canonical | BackingRelationship::Independent
                        )
                    })
            })
        })
        .map(|name| {
            let path = home.join(UNIVERSAL_SKILLS_RELATIVE).join(&name);
            DoctorViolation {
                invariant: DoctorInvariant::LockfileEntryHasFolder,
                skill: Some(SkillName(name.clone())),
                path: path.clone(),
                message: format!(
                    "lockfile entry `{name}` names {} but no folder is there",
                    path.display()
                ),
            }
        })
        .collect()
}

/// Invariant 4: no folder is in two states at once - here, installed and
/// parked simultaneously, which the lifecycle table (`park_skill`) says
/// never happens once a park lands cleanly. Detect-only; there is no single
/// safe automatic choice between the two states.
///
/// Resolved through `inventory`'s own deployments rather than
/// `fs.symlink_metadata` on the universal and parked roots directly: a
/// skill installed only via a per-harness canonical copy (no universal-root
/// entry) plus a parked folder is the same violation, and comparing only
/// the two fixed paths missed it.
pub fn check_no_folder_in_two_states(
    inventory: &Inventory,
    skill_names: &[SkillName],
) -> Vec<DoctorViolation> {
    skill_names
        .iter()
        .filter_map(|name| {
            let skill = inventory.skills.iter().find(|skill| &skill.name == name)?;
            let parked = skill
                .deployments
                .iter()
                .find(|deployment| matches!(deployment.root.kind, RootKind::Parked))?;
            let installed = skill
                .deployments
                .iter()
                .find(|deployment| !matches!(deployment.root.kind, RootKind::Parked))?;
            Some(DoctorViolation {
                invariant: DoctorInvariant::NoFolderInTwoStates,
                skill: Some(name.clone()),
                path: installed.path.clone(),
                message: format!(
                    "`{}` exists both installed at {} and parked at {}",
                    name.0,
                    installed.path.display(),
                    parked.path.display()
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
    use std::sync::Arc;

    use super::*;
    use crate::dto::ScanRequest;
    use crate::harness::HarnessCatalog;
    use crate::ops;
    use crate::ports::{Ports, Runtime};
    use crate::testing::golden::{ctx, scope_for};
    use crate::testing::{FakeClock, FakeIds, FakeLease, FixtureBuilder, NoHistory, RecordingSink};

    const HOME: &str = "/home";
    /// Relative path of the parked holding directory under a scope home;
    /// only fixtures need it now that [`check_no_folder_in_two_states`]
    /// reads the parked path off `inventory` instead of building it.
    const PARKED_RELATIVE: &str = ".agents/skills-parked";
    /// Minimal frontmatter `scan` needs to recognize a directory as a skill.
    fn skill_md(name: &str) -> Vec<u8> {
        format!("---\nname: {name}\ndescription: fixture skill.\n---\nBody.\n").into_bytes()
    }

    fn home() -> PathBuf {
        PathBuf::from(HOME)
    }

    /// Scans `fs` from `HOME` so a test can assert a check against the same
    /// `Inventory` `ops::fix_skill` would build, not a hand-rolled one.
    fn inventory_for(fs: &Arc<dyn ScopeFs>) -> Inventory {
        let ports = Ports {
            fs: fs.clone(),
            clock: Arc::new(FakeClock::at(0)),
            ids: Arc::new(FakeIds::default()),
            leases: Arc::new(FakeLease::default()),
            history: Arc::new(NoHistory),
            sink: Arc::new(RecordingSink::default()),
            spawner: None,
            discovery: None,
            tools: None,
            catalog: Arc::new(HarnessCatalog::builtin()),
        };
        let scope = scope_for("doctor", &home());
        let rt = Runtime::new(&scope, ports).expect("runtime");
        ops::scan(&rt, &ctx(), &ScanRequest::default()).expect("scan")
    }

    #[test]
    fn quarantine_over_cap_is_reported_with_its_path_and_count_or_names_the_missed_entry() {
        let mut builder = FixtureBuilder::new().dir(&format!("{HOME}/{UNIVERSAL_SKILLS_RELATIVE}"));
        let over_by = 3;
        for i in 0..(QUARANTINE_RETENTION_CAP + over_by) {
            builder = builder.file(
                &format!(
                    "{HOME}/{UNIVERSAL_SKILLS_RELATIVE}/{QUARANTINE_DIR_NAME}/{i:04}-quarantined/SKILL.md"
                ),
                &skill_md("quarantined"),
            );
        }
        let fs = builder.build_fs();
        let dir = home()
            .join(UNIVERSAL_SKILLS_RELATIVE)
            .join(QUARANTINE_DIR_NAME);

        let violations = check_quarantine_within_cap(&fs, &home());
        assert_eq!(
            violations.len(),
            1,
            "expected exactly one violation, got {violations:?}"
        );
        let violation = &violations[0];
        assert_eq!(violation.invariant, DoctorInvariant::QuarantineWithinCap);
        assert_eq!(
            violation.path, dir,
            "violation did not name the quarantine path"
        );
        let total = QUARANTINE_RETENTION_CAP + over_by;
        assert!(
            violation.message.contains(&total.to_string()),
            "message did not name the entry count {total}: {}",
            violation.message
        );
        assert!(
            violation
                .message
                .contains(&QUARANTINE_RETENTION_CAP.to_string()),
            "message did not name the cap {QUARANTINE_RETENTION_CAP}: {}",
            violation.message
        );
    }

    #[test]
    fn lockfile_entry_with_no_folder_anywhere_is_flagged_or_a_per_harness_copy_clears_it() {
        let fs: Arc<dyn ScopeFs> = Arc::new(
            FixtureBuilder::new()
                .dir(&format!("{HOME}/{UNIVERSAL_SKILLS_RELATIVE}"))
                .file(
                    &format!("{HOME}/.claude/skills/harness-only/SKILL.md"),
                    &skill_md("harness-only"),
                )
                .file(
                    &format!("{HOME}/.agents/.skill-lock.json"),
                    br#"{"version":3,"skills":{
                        "ghost-skill":{"source":"o/r","sourceType":"github","sourceUrl":"https://example.com","skillFolderHash":"abc","installedAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z"},
                        "harness-only":{"source":"o/r","sourceType":"github","sourceUrl":"https://example.com","skillFolderHash":"abc","installedAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z"}
                    }}"#,
                )
                .build_fs(),
        );

        let violations = check_lockfile_entry_has_folder(fs.as_ref(), &home(), &inventory_for(&fs));
        assert_eq!(
            violations.len(),
            1,
            "harness-only must not be a false positive: {violations:?}"
        );
        assert_eq!(
            violations[0].invariant,
            DoctorInvariant::LockfileEntryHasFolder
        );
        assert_eq!(violations[0].skill, Some(SkillName("ghost-skill".into())));

        // The "repair" this invariant has today is naming the path so the
        // user (or a follow-up unit) can restore the folder or drop the
        // stale entry; simulating that restoration here proves the check
        // clears once it happens, without core writing the partial
        // document itself (see the module doc comment).
        fs.fsops_create_dir(&home().join(UNIVERSAL_SKILLS_RELATIVE).join("ghost-skill"))
            .unwrap();
        fs.fsops_write_new_file(
            &home()
                .join(UNIVERSAL_SKILLS_RELATIVE)
                .join("ghost-skill")
                .join("SKILL.md"),
            &skill_md("ghost-skill"),
        )
        .unwrap();
        assert!(
            check_lockfile_entry_has_folder(fs.as_ref(), &home(), &inventory_for(&fs)).is_empty()
        );
    }

    /// A lockfile entry whose only deployment is a dangling per-harness
    /// link (the alias's target is never created, same shape as
    /// `testing::fixtures::broken_link`): the link's own `LinkedTo`
    /// deployment must not count as "has a folder", or the stale lockfile
    /// row hides behind a link that resolves nowhere.
    #[test]
    fn lockfile_entry_whose_only_deployment_is_a_dangling_link_is_flagged_or_names_the_hidden_row()
    {
        let fs: Arc<dyn ScopeFs> = Arc::new(
            FixtureBuilder::new()
                .dir(&format!("{HOME}/{UNIVERSAL_SKILLS_RELATIVE}"))
                .alias(
                    &format!("{HOME}/.claude/skills/ghost"),
                    &format!("../../{UNIVERSAL_SKILLS_RELATIVE}/missing"),
                )
                .file(
                    &format!("{HOME}/.agents/.skill-lock.json"),
                    br#"{"version":3,"skills":{
                        "ghost":{"source":"o/r","sourceType":"github","sourceUrl":"https://example.com","skillFolderHash":"abc","installedAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z"}
                    }}"#,
                )
                .build_fs(),
        );

        let violations = check_lockfile_entry_has_folder(fs.as_ref(), &home(), &inventory_for(&fs));
        assert_eq!(
            violations.len(),
            1,
            "a dangling link must not hide the stale lockfile row: {violations:?}"
        );
        assert_eq!(
            violations[0].invariant,
            DoctorInvariant::LockfileEntryHasFolder
        );
        assert_eq!(violations[0].skill, Some(SkillName("ghost".into())));
    }

    #[test]
    fn skill_parked_and_installed_via_a_harness_copy_is_flagged_or_names_the_missed_root() {
        let name = SkillName("double-state".to_string());
        let fs: Arc<dyn ScopeFs> = Arc::new(
            FixtureBuilder::new()
                .dir(&format!("{HOME}/{UNIVERSAL_SKILLS_RELATIVE}"))
                .file(
                    &format!("{HOME}/.claude/skills/double-state/SKILL.md"),
                    &skill_md("double-state"),
                )
                .file(
                    &format!("{HOME}/{PARKED_RELATIVE}/double-state/SKILL.md"),
                    &skill_md("double-state"),
                )
                .build_fs(),
        );

        let violations =
            check_no_folder_in_two_states(&inventory_for(&fs), std::slice::from_ref(&name));
        assert_eq!(
            violations.len(),
            1,
            "a per-harness canonical copy plus a parked folder must be caught: {violations:?}"
        );
        assert_eq!(
            violations[0].invariant,
            DoctorInvariant::NoFolderInTwoStates
        );

        let parked_dir = home().join(PARKED_RELATIVE).join(&name.0);
        fs.fsops_remove_file(&parked_dir.join("SKILL.md")).unwrap();
        fs.fsops_remove_dir(&parked_dir).unwrap();
        assert!(check_no_folder_in_two_states(&inventory_for(&fs), &[name]).is_empty());
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
