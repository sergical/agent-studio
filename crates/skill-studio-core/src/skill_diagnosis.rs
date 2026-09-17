use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::skill_agents::AgentId;
use crate::skill_inventory::{Deployment, InstalledSkill};
use crate::skill_ledger_inventory::LedgerOnlySkill;
use crate::skill_read::DiscoveryExtent;
use crate::skill_service::{
    inventory_read_blockers, InventoryCompleteness, InventoryRead, SkillScope,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeploymentEvidence {
    pub deployment_id: Option<String>,
    pub path: String,
}

impl From<&Deployment> for DeploymentEvidence {
    fn from(deployment: &Deployment) -> Self {
        Self {
            deployment_id: (!deployment.id.is_empty()).then(|| deployment.id.clone()),
            path: deployment.path.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentHashGroup {
    pub content_hash: String,
    pub deployments: Vec<DeploymentEvidence>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LedgerAbsence {
    ConfirmedAbsent,
    NotObserved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SkillDiagnostic {
    ParkedButReinstalled {
        skill_name: String,
        deployments: Vec<DeploymentEvidence>,
    },
    LinkedRoot {
        harness: AgentId,
        root: PathBuf,
        deployments: Vec<DeploymentEvidence>,
    },
    DivergentCopies {
        skill_name: String,
        groups: Vec<ContentHashGroup>,
    },
    BrokenSymlink {
        skill_name: String,
        deployment: DeploymentEvidence,
        target: Option<String>,
    },
    BlockingSpecViolation {
        skill_name: String,
        deployment: DeploymentEvidence,
        violations: Vec<String>,
    },
    LedgerOnly {
        owner: LedgerOnlySkill,
        absence: LedgerAbsence,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnosis {
    pub scope: SkillScope,
    pub completeness: InventoryCompleteness,
    pub extent: DiscoveryExtent,
    pub issues: Vec<SkillDiagnostic>,
}

pub fn is_blocking_spec_violation(violation: &str) -> bool {
    [
        "invalid YAML frontmatter",
        "missing required frontmatter field: name",
        "missing required frontmatter field: description",
        "name \"",
    ]
    .iter()
    .any(|prefix| violation.starts_with(prefix))
}

/// Pure projection of one inventory read. Findings are evidence, not write authority.
pub fn diagnose(inventory: &InventoryRead) -> Diagnosis {
    let absence_known = inventory_read_blockers(
        &inventory.scope,
        inventory.extent,
        &inventory.source_coverage,
        &inventory.ownership,
    )
    .is_empty();
    let mut issues = diagnose_deployments(&inventory.skills);
    let mut ledger = inventory.ledger_only.iter().collect::<Vec<_>>();
    ledger.sort_by(|a, b| (&a.name, &a.owner_id).cmp(&(&b.name, &b.owner_id)));
    issues.extend(ledger.into_iter().map(|owner| SkillDiagnostic::LedgerOnly {
        owner: owner.clone(),
        absence: if absence_known {
            LedgerAbsence::ConfirmedAbsent
        } else {
            LedgerAbsence::NotObserved
        },
    }));
    Diagnosis {
        scope: inventory.scope.clone(),
        completeness: if absence_known {
            inventory.completeness
        } else {
            InventoryCompleteness::Partial
        },
        extent: inventory.extent,
        issues,
    }
}

fn diagnose_deployments(skills: &[InstalledSkill]) -> Vec<SkillDiagnostic> {
    let mut parked = Vec::new();
    let mut linked = BTreeMap::<(String, PathBuf), (AgentId, Vec<DeploymentEvidence>)>::new();
    let mut divergent = Vec::new();
    let mut broken = Vec::new();
    let mut spec = Vec::new();
    let mut skills = skills.iter().collect::<Vec<_>>();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    for skill in skills {
        let mut deployments = skill.deployments.iter().collect::<Vec<_>>();
        deployments.sort_by(|a, b| (&a.path, &a.id).cmp(&(&b.path, &b.id)));
        if skill.parked {
            let active = deployments
                .iter()
                .filter(|d| d.scope != "parked" && d.plugin.is_none())
                .map(|d| DeploymentEvidence::from(*d))
                .collect::<Vec<_>>();
            if !active.is_empty() {
                parked.push(SkillDiagnostic::ParkedButReinstalled {
                    skill_name: skill.name.clone(),
                    deployments: active,
                });
            }
        }
        let mut hashes = BTreeMap::<String, Vec<DeploymentEvidence>>::new();
        for deployment in deployments {
            if deployment.plugin.is_none() && !deployment.content_hash.is_empty() {
                hashes
                    .entry(deployment.content_hash.clone())
                    .or_default()
                    .push(deployment.into());
            }
            if deployment.symlink_is_broken {
                broken.push(SkillDiagnostic::BrokenSymlink {
                    skill_name: skill.name.clone(),
                    deployment: deployment.into(),
                    target: deployment.symlink_target.clone(),
                });
            }
            let mut violations = deployment
                .spec_violations
                .iter()
                .filter(|v| is_blocking_spec_violation(v))
                .cloned()
                .collect::<Vec<_>>();
            violations.sort();
            violations.dedup();
            if !violations.is_empty() {
                spec.push(SkillDiagnostic::BlockingSpecViolation {
                    skill_name: skill.name.clone(),
                    deployment: deployment.into(),
                    violations,
                });
            }
            if deployment.scope == "global" && deployment.shared_via_whole_dir_link {
                let harness = [
                    AgentId::ClaudeCode,
                    AgentId::Codex,
                    AgentId::OpenCode,
                    AgentId::Pi,
                    AgentId::Cursor,
                    AgentId::GrokBuild,
                ]
                .into_iter()
                .find(|agent| agent.display_name() == deployment.agent);
                if let (Some(harness), Some(root)) = (harness, Path::new(&deployment.path).parent())
                {
                    linked
                        .entry((harness.cli_name().to_string(), root.to_path_buf()))
                        .or_insert_with(|| (harness, Vec::new()))
                        .1
                        .push(deployment.into());
                }
            }
        }
        if hashes.len() > 1 {
            divergent.push(SkillDiagnostic::DivergentCopies {
                skill_name: skill.name.clone(),
                groups: hashes
                    .into_iter()
                    .map(|(content_hash, deployments)| ContentHashGroup {
                        content_hash,
                        deployments,
                    })
                    .collect(),
            });
        }
    }
    let mut issues = parked;
    for ((_, root), (harness, mut deployments)) in linked {
        deployments.sort();
        deployments.dedup();
        issues.push(SkillDiagnostic::LinkedRoot {
            harness,
            root,
            deployments,
        });
    }
    issues.extend(divergent);
    issues.extend(broken);
    issues.extend(spec);
    issues
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosisMergeError {
    InvalidExtent,
    ScopeChanged,
    InvalidSelection,
}

impl std::fmt::Display for DiagnosisMergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidExtent => {
                "Diagnosis reconciliation requires a full baseline and named update"
            }
            Self::ScopeChanged => "Diagnosis scope changed; a full refresh is required",
            Self::InvalidSelection => {
                "Diagnosis update contains an unselected ledger owner or empty selection"
            }
        })
    }
}
impl std::error::Error for DiagnosisMergeError {}

/// Call after the inventory's named replacement has succeeded. Root and hash
/// findings are recomputed from all retained deployments, never spliced by name.
pub fn reconcile_named_diagnosis(
    current: &Diagnosis,
    skills: &[InstalledSkill],
    names: &BTreeSet<String>,
    updated: &Diagnosis,
) -> Result<Diagnosis, DiagnosisMergeError> {
    if current.extent != DiscoveryExtent::Full || updated.extent != DiscoveryExtent::Named {
        return Err(DiagnosisMergeError::InvalidExtent);
    }
    if current.scope != updated.scope {
        return Err(DiagnosisMergeError::ScopeChanged);
    }
    if names.is_empty()
        || updated.issues.iter().any(|issue| {
            matches!(issue,
        SkillDiagnostic::LedgerOnly { owner, .. } if !names.contains(&owner.name))
        })
    {
        return Err(DiagnosisMergeError::InvalidSelection);
    }
    let mut issues = diagnose_deployments(skills);
    let mut ledger = current
        .issues
        .iter()
        .filter_map(|issue| match issue {
            SkillDiagnostic::LedgerOnly { owner, absence } if !names.contains(&owner.name) => {
                Some((owner, absence))
            }
            _ => None,
        })
        .chain(updated.issues.iter().filter_map(|issue| match issue {
            SkillDiagnostic::LedgerOnly { owner, absence } => Some((owner, absence)),
            _ => None,
        }))
        .collect::<Vec<_>>();
    ledger.sort_by(|(a, _), (b, _)| (&a.name, &a.owner_id).cmp(&(&b.name, &b.owner_id)));
    issues.extend(
        ledger
            .into_iter()
            .map(|(owner, absence)| SkillDiagnostic::LedgerOnly {
                owner: owner.clone(),
                absence: *absence,
            }),
    );
    Ok(Diagnosis {
        scope: current.scope.clone(),
        extent: DiscoveryExtent::Full,
        completeness: if current.completeness == InventoryCompleteness::Complete
            && updated.completeness == InventoryCompleteness::Complete
        {
            InventoryCompleteness::Complete
        } else {
            InventoryCompleteness::Partial
        },
        issues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_inventory::InstalledSkill;
    use crate::skill_ownership::OwnershipReadReport;
    use crate::skill_plugins::PluginInfo;
    use crate::skill_service::{ReplacementSafety, SkillScope};

    fn skill(name: &str, deployments: Vec<Deployment>) -> InstalledSkill {
        let mut skill: InstalledSkill = serde_json::from_value(serde_json::json!({
            "name": name, "source": "fixture/repo", "source_type": "github",
            "source_url": null, "skill_path": null, "installed_at": "before",
            "updated_at": null, "has_update": false, "source_kind": "manual"
        }))
        .unwrap();
        skill.deployments = deployments;
        skill
    }

    fn deployment(name: &str, path: &str, hash: &str) -> Deployment {
        Deployment {
            id: format!("dep:{path}"),
            agent: name.to_string(),
            scope: "global".to_string(),
            path: path.to_string(),
            content_hash: hash.to_string(),
            ..Default::default()
        }
    }

    fn inventory(skills: Vec<InstalledSkill>) -> InventoryRead {
        InventoryRead {
            scope: SkillScope {
                home: "/fixture".into(),
                projects: vec![],
                backing_roots: vec![],
                plugin_ownership_roots: vec![],
            },
            skills,
            ledger_only: vec![],
            completeness: InventoryCompleteness::Partial,
            replacement_safety: ReplacementSafety::NotApplicable,
            discovery_issues: vec![],
            source_coverage: vec![],
            extent: DiscoveryExtent::Full,
            ownership: OwnershipReadReport::empty(),
        }
    }

    #[test]
    fn diagnosis_preserves_exact_targets_and_groups_without_plugin_hashes() {
        let mut linked = deployment("Claude Code", "/fixture/.claude/skills/alpha", "a");
        linked.shared_via_whole_dir_link = true;
        let mut differing = deployment("Codex", "/project/.codex/skills/alpha", "b");
        differing.scope = "project".into();
        differing.shared_via_whole_dir_link = true;
        differing.spec_violations = vec![
            "missing required frontmatter field: description".into(),
            "description exceeds 1024 characters".into(),
        ];
        let mut plugin = deployment("Codex", "/plugin/alpha", "plugin-hash");
        plugin.plugin = Some(PluginInfo {
            name: "package".into(),
            version: None,
            harness: "Codex".into(),
        });
        let mut broken = deployment("pi", "/fixture/.pi/alpha", "");
        broken.symlink_is_broken = true;
        broken.symlink_target = Some("/missing/alpha".into());
        let mut parked = deployment("shared", "/fixture/parked/alpha", "a");
        parked.scope = "parked".into();
        let mut alpha = skill("alpha", vec![plugin, broken, differing, linked, parked]);
        alpha.parked = true;
        let mut sibling = deployment("Claude Code", "/fixture/.claude/skills/beta", "a");
        sibling.shared_via_whole_dir_link = true;
        let mut denied = deployment("pi", "/fixture/.pi/beta", "");
        denied.symlink_error = Some("Permission denied".into());
        let mut input = inventory(vec![skill("beta", vec![sibling, denied]), alpha]);
        let value = serde_json::to_value(diagnose(&input)).unwrap();
        let issues = value["issues"].as_array().unwrap();
        assert_eq!(
            issues
                .iter()
                .map(|issue| issue["kind"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "parked-but-reinstalled",
                "linked-root",
                "divergent-copies",
                "broken-symlink",
                "blocking-spec-violation"
            ]
        );
        assert_eq!(issues[0]["deployments"].as_array().unwrap().len(), 3);
        assert_eq!(issues[1]["harness"], "claude-code");
        assert_eq!(issues[1]["deployments"].as_array().unwrap().len(), 2);
        assert_eq!(issues[2]["groups"].as_array().unwrap().len(), 2);
        assert_eq!(
            issues[2]["groups"][0]["deployments"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(issues[3]["deployment"]["path"], "/fixture/.pi/alpha");
        assert_eq!(
            issues[4]["deployment"]["path"],
            "/project/.codex/skills/alpha"
        );
        assert_eq!(issues[4]["violations"].as_array().unwrap().len(), 1);
        input.skills.reverse();
        for skill in &mut input.skills {
            skill.deployments.reverse();
        }
        assert_eq!(serde_json::to_value(diagnose(&input)).unwrap(), value);
    }

    #[test]
    fn plugin_with_same_name_does_not_count_as_reinstalled_parked_skill() {
        let mut parked = deployment("shared", "/fixture/parked/alpha", "a");
        parked.scope = "parked".into();
        let mut plugin = deployment("Codex", "/plugin/alpha", "b");
        plugin.plugin = Some(PluginInfo {
            name: "package".into(),
            version: None,
            harness: "Codex".into(),
        });
        let mut row = skill("alpha", vec![parked, plugin]);
        row.parked = true;
        assert!(diagnose(&inventory(vec![row])).issues.is_empty());
    }

    #[test]
    fn empty_ids_and_nonblocking_notes_do_not_create_repair_authority() {
        let mut copy = deployment("shared", "/fixture/alpha", "a");
        copy.id.clear();
        copy.spec_violations = vec!["name \"Alpha\" must be lowercase".into()];
        let mut row = skill("alpha", vec![copy]);
        row.spec_violations = vec!["missing required frontmatter field: description".into()];
        let value = serde_json::to_value(diagnose(&inventory(vec![row]))).unwrap();
        assert!(value["issues"][0]["deployment"]["deployment_id"].is_null());
        assert_eq!(
            value["issues"][0]["violations"].as_array().unwrap().len(),
            1
        );
        for note in [
            "description exceeds 1024 characters",
            "missing optional field",
            "",
        ] {
            assert!(!is_blocking_spec_violation(note));
        }
    }
    fn ledger_issue(name: &str, owner_id: &str, absence: LedgerAbsence) -> SkillDiagnostic {
        SkillDiagnostic::LedgerOnly {
            owner: LedgerOnlySkill {
                owner_id: owner_id.into(),
                name: name.into(),
                scope: crate::skill_deployment::InstallScope::Global,
                project_path: None,
                owner_kind: crate::skill_ownership::LifecycleOwnerKind::Manual,
                sources: vec![],
            },
            absence,
        }
    }

    #[test]
    fn named_merge_rebuilds_shared_root_findings_and_retains_unselected_ledger_evidence() {
        let mut a = deployment("Claude Code", "/fixture/.claude/skills/alpha", "a");
        a.shared_via_whole_dir_link = true;
        let mut b = deployment("Claude Code", "/fixture/.claude/skills/beta", "a");
        b.shared_via_whole_dir_link = true;
        let old = inventory(vec![
            skill("alpha", vec![a]),
            skill("beta", vec![b.clone()]),
        ]);
        let mut current = diagnose(&old);
        current.completeness = InventoryCompleteness::Complete;
        current.issues.push(ledger_issue(
            "alpha",
            "old-alpha",
            LedgerAbsence::NotObserved,
        ));
        current.issues.push(ledger_issue(
            "gamma",
            "keep-gamma",
            LedgerAbsence::NotObserved,
        ));
        let mut updated = diagnose(&inventory(vec![]));
        updated.extent = DiscoveryExtent::Named;
        updated.issues.push(ledger_issue(
            "alpha",
            "new-alpha",
            LedgerAbsence::ConfirmedAbsent,
        ));
        let names = BTreeSet::from(["alpha".to_string()]);
        let merged =
            reconcile_named_diagnosis(&current, &[skill("beta", vec![b])], &names, &updated)
                .unwrap();
        assert_eq!(merged.completeness, InventoryCompleteness::Partial);
        assert_eq!(merged.extent, DiscoveryExtent::Full);
        assert_eq!(merged.issues.len(), 3);
        match &merged.issues[0] {
            SkillDiagnostic::LinkedRoot { deployments, .. } => {
                assert_eq!(deployments.len(), 1);
                assert!(deployments[0].path.ends_with("/beta"));
            }
            other => panic!("unexpected: {other:?}"),
        }
        assert!(
            matches!(&merged.issues[1], SkillDiagnostic::LedgerOnly { owner, absence: LedgerAbsence::ConfirmedAbsent } if owner.owner_id == "new-alpha")
        );
        assert!(
            matches!(&merged.issues[2], SkillDiagnostic::LedgerOnly { owner, absence: LedgerAbsence::NotObserved } if owner.owner_id == "keep-gamma")
        );
        let json = serde_json::to_value(&merged).unwrap();
        let decoded: Diagnosis = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), json);
        let no_deployments = reconcile_named_diagnosis(&current, &[], &names, &updated).unwrap();
        assert!(no_deployments
            .issues
            .iter()
            .all(|issue| matches!(issue, SkillDiagnostic::LedgerOnly { .. })));
    }

    #[test]
    fn named_merge_refuses_different_scopes_extents_and_unselected_ledger_updates() {
        let current = diagnose(&inventory(vec![]));
        let mut updated = current.clone();
        let names = BTreeSet::from(["alpha".to_string()]);
        assert_eq!(
            reconcile_named_diagnosis(&current, &[], &names, &updated).unwrap_err(),
            DiagnosisMergeError::InvalidExtent
        );
        updated.extent = DiscoveryExtent::Named;
        updated.scope.projects.push("/another".into());
        assert_eq!(
            reconcile_named_diagnosis(&current, &[], &names, &updated).unwrap_err(),
            DiagnosisMergeError::ScopeChanged
        );
        updated.scope = current.scope.clone();
        updated.issues.push(ledger_issue(
            "beta",
            "beta-owner",
            LedgerAbsence::ConfirmedAbsent,
        ));
        assert_eq!(
            reconcile_named_diagnosis(&current, &[], &names, &updated).unwrap_err(),
            DiagnosisMergeError::InvalidSelection
        );
        updated.issues.clear();
        assert_eq!(
            reconcile_named_diagnosis(&current, &[], &BTreeSet::new(), &updated).unwrap_err(),
            DiagnosisMergeError::InvalidSelection
        );
    }
}
