use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::skill_deployment::InstallScope;
use crate::skill_inventory::InstalledSkill;
use crate::skill_ownership::{owner_id_for, LifecycleOwnerKind, OwnershipReadReport};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "entry", rename_all = "kebab-case")]
pub enum LedgerSource {
    SkillsSh(crate::skill_lock_file::InstalledSkillEntry),
    ProjectSkillsSh(crate::skill_project_lock::ProjectSkillEntry),
    Dotagents(crate::skill_dotagents_ledger::DotagentsSkill),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerOnlySkill {
    pub owner_id: String,
    pub name: String,
    pub scope: InstallScope,
    pub project_path: Option<PathBuf>,
    pub owner_kind: LifecycleOwnerKind,
    pub sources: Vec<LedgerSource>,
}

/// Ledger owners without a matching observed deployment. Membership coverage must
/// establish absence before a caller presents these as missing installations.
/// These records provide no deployment identity or mutation authorization.
pub(crate) fn ledger_only_skills(
    ownership: &OwnershipReadReport,
    skills: &[InstalledSkill],
    names: Option<&BTreeSet<String>>,
) -> Vec<LedgerOnlySkill> {
    let observed_owners = skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .filter_map(|deployment| deployment.owner_id.as_deref())
        .collect::<BTreeSet<_>>();
    let observed_paths = skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .map(|deployment| Path::new(&deployment.path))
        .collect::<BTreeSet<_>>();
    let mut records = Vec::new();
    for scope in &ownership.scopes {
        let Ok(ledger) = scope.as_ledger() else {
            continue;
        };
        let declared = ledger
            .lock
            .skills
            .keys()
            .chain(
                ledger
                    .project_lock
                    .iter()
                    .flat_map(|lock| lock.skills.keys()),
            )
            .chain(ledger.dotagents.iter().map(|skill| &skill.name))
            .collect::<BTreeSet<_>>();
        for name in declared {
            if names.is_some_and(|names| !names.contains(name)) {
                continue;
            }
            let owner_id = owner_id_for(&ledger, name);
            let canonical_entry = ledger.agents_dir.join("skills").join(name);
            let parked_entry = ledger.agents_dir.join("skills-parked").join(name);
            let disabled_entry = ledger
                .agents_dir
                .join("skills")
                .join(crate::skill_discovery::STUDIO_DISABLED_DIR_NAME)
                .join(name);
            let observed = observed_owners.contains(owner_id.as_str())
                || observed_paths.contains(canonical_entry.as_path())
                || observed_paths.contains(disabled_entry.as_path())
                || (ledger.scope == InstallScope::Global
                    && observed_paths.contains(parked_entry.as_path()));
            if observed {
                continue;
            }
            let mut sources = Vec::new();
            if let Some(entry) = ledger.lock.skills.get(name) {
                sources.push(LedgerSource::SkillsSh(entry.clone()));
            }
            if let Some(entry) = ledger
                .project_lock
                .as_ref()
                .and_then(|lock| lock.skills.get(name))
            {
                sources.push(LedgerSource::ProjectSkillsSh(entry.clone()));
            }
            let skills_sh = !sources.is_empty();
            let dotagents = ledger.dotagents.iter().find(|skill| &skill.name == name);
            let owner_kind = match (skills_sh, dotagents) {
                (true, Some(_)) => LifecycleOwnerKind::Ambiguous,
                (true, None) => LifecycleOwnerKind::SkillsSh,
                (false, Some(skill)) if skill.has_manifest_row => LifecycleOwnerKind::Dotagents,
                _ => LifecycleOwnerKind::WildcardDotagents,
            };
            if let Some(entry) = dotagents {
                sources.push(LedgerSource::Dotagents(entry.clone()));
            }
            records.push(LedgerOnlySkill {
                owner_id,
                name: name.clone(),
                scope: ledger.scope.clone(),
                project_path: ledger.project_path.clone(),
                owner_kind,
                sources,
            });
        }
    }
    records.sort_by(|left, right| left.owner_id.cmp(&right.owner_id));
    records
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::skill_plugins::SkillDiscoveryReadContext;
    use crate::skill_service::ScopedSkillService;
    use std::fs;
    use std::time::Duration;

    fn global_lock() -> &'static str {
        r#"{"version":3,"skills":{"alpha":{"source":"owner/repo","sourceType":"github","sourceUrl":"https://example.test/repo","skillFolderHash":"global-hash","installedAt":"installed","updatedAt":"updated"}}}"#
    }

    #[test]
    fn disabled_and_parked_deployments_are_not_ledger_only() {
        for directory in ["skills/.skill-studio-disabled", "skills-parked"] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let agents = home.join(".agents");
            let skill = agents.join(directory).join("alpha");
            fs::create_dir_all(&skill).unwrap();
            fs::write(
                skill.join("SKILL.md"),
                "---\nname: alpha\ndescription: fixture\n---\nbody\n",
            )
            .unwrap();
            fs::write(agents.join(".skill-lock.json"), global_lock()).unwrap();
            let mut service = ScopedSkillService::new(SkillDiscoveryReadContext::bind(
                home,
                vec![],
                vec![],
                vec![],
            ));
            let inventory = service.scan(None, Some(Duration::from_secs(10))).unwrap();
            assert_eq!(inventory.skills.len(), 1);
            assert_eq!(inventory.skills[0].deployments.len(), 1);
            assert!(inventory.ledger_only.is_empty());
        }
    }

    #[test]
    fn service_keeps_same_named_ledger_owners_separate_and_filters_named_reads() {
        for failed_project in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let first = temp.path().join("first");
            let second = temp.path().join("second");
            for root in [&home, &first, &second] {
                fs::create_dir_all(root.join(".agents")).unwrap();
            }
            fs::write(home.join(".agents/.skill-lock.json"), global_lock()).unwrap();
            let project_lock = r#"{"version":1,"skills":{"alpha":{"source":"../local","sourceType":"local","computedHash":"project-hash","ref":"release","subagents":["review"],"wellKnownDigest":"digest"},"beta":{"source":"../beta","sourceType":"local","computedHash":"beta-hash"}}}"#;
            fs::write(first.join("skills-lock.json"), project_lock).unwrap();
            fs::write(
                second.join("skills-lock.json"),
                if failed_project {
                    "invalid"
                } else {
                    project_lock
                },
            )
            .unwrap();
            let installed = first.join(".agents/skills/alpha");
            fs::create_dir_all(&installed).unwrap();
            fs::write(
                installed.join("SKILL.md"),
                "---\nname: alpha\ndescription: fixture\n---\nbody\n",
            )
            .unwrap();
            let plugin = home.join(".claude/plugins/cache/plugin");
            fs::create_dir_all(plugin.join("skills/alpha")).unwrap();
            fs::write(plugin.join("plugin.json"), r#"{"name":"plugin"}"#).unwrap();
            fs::write(
                plugin.join("skills/alpha/SKILL.md"),
                "---\nname: alpha\ndescription: plugin\n---\nbody\n",
            )
            .unwrap();
            let context = SkillDiscoveryReadContext::bind(
                home,
                vec![first.clone(), second.clone()],
                vec![],
                vec![],
            );
            let mut service = ScopedSkillService::new(context);
            for named in [false, true] {
                let names = BTreeSet::from(["alpha".to_string()]);
                let inventory = service
                    .scan(named.then_some(&names), Some(Duration::from_secs(10)))
                    .unwrap();
                let rows = inventory.ledger_only;
                assert_eq!(
                    rows.len(),
                    match (failed_project, named) {
                        (false, false) => 4,
                        (false, true) => 2,
                        (true, false) => 2,
                        (true, true) => 1,
                    }
                );
                assert_eq!(
                    rows.iter()
                        .map(|row| &row.owner_id)
                        .collect::<BTreeSet<_>>()
                        .len(),
                    rows.len()
                );
                assert!(!rows
                    .iter()
                    .any(|row| row.name == "alpha" && row.project_path.as_ref() == Some(&first)));
                let global = rows
                    .iter()
                    .find(|row| row.scope == InstallScope::Global)
                    .unwrap();
                assert_eq!(global.owner_id, "owner:v1/global/alpha");
                assert!(
                    matches!(&global.sources[0], LedgerSource::SkillsSh(entry) if entry.skill_folder_hash == "global-hash" && entry.installed_at == "installed")
                );
                if !failed_project {
                    let project = rows
                        .iter()
                        .find(|row| {
                            row.name == "alpha" && row.project_path.as_ref() == Some(&second)
                        })
                        .unwrap();
                    assert_ne!(project.owner_id, global.owner_id);
                    assert_eq!(project.owner_kind, LifecycleOwnerKind::SkillsSh);
                    assert!(
                        matches!(&project.sources[0], LedgerSource::ProjectSkillsSh(entry)
                        if entry.source == "../local" && entry.computed_hash == "project-hash" && entry.source_url.is_none()
                            && entry.source_ref.as_deref() == Some("release") && entry.subagents.as_ref().unwrap() == &["review"] && entry.well_known_digest.as_deref() == Some("digest"))
                    );
                }
                assert_eq!(inventory.ownership.failures().is_empty(), !failed_project);
            }
        }
    }

    #[test]
    fn dotagents_evidence_preserves_wildcard_and_conflicting_owner_state() {
        for skills_sh in [false, true] {
            for declared in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path().join("home");
                let agents = home.join(".agents");
                fs::create_dir_all(&agents).unwrap();
                fs::write(agents.join("agents.lock"), "[skills.alpha]\nsource = \"owner/repo\"\nresolved_path = \"skills/alpha\"\nresolved_commit = \"commit\"\n").unwrap();
                if declared {
                    fs::write(
                        agents.join("agents.toml"),
                        "[[skills]]\nname = \"alpha\"\nref = \"release\"\n",
                    )
                    .unwrap();
                }
                if skills_sh {
                    fs::write(agents.join(".skill-lock.json"), global_lock()).unwrap();
                }
                let mut service = ScopedSkillService::new(SkillDiscoveryReadContext::bind(
                    home,
                    vec![],
                    vec![],
                    vec![],
                ));
                let inventory = service.scan(None, Some(Duration::from_secs(10))).unwrap();
                assert_eq!(inventory.ledger_only.len(), 1);
                let row = &inventory.ledger_only[0];
                assert_eq!(row.sources.len(), if skills_sh { 2 } else { 1 });
                assert_eq!(
                    row.owner_kind,
                    if skills_sh {
                        LifecycleOwnerKind::Ambiguous
                    } else if declared {
                        LifecycleOwnerKind::Dotagents
                    } else {
                        LifecycleOwnerKind::WildcardDotagents
                    }
                );
                assert!(row.sources.iter().any(|source| matches!(source, LedgerSource::Dotagents(entry) if entry.installed_commit.as_deref() == Some("commit") && entry.has_manifest_row == declared)));
            }
        }
    }
}
