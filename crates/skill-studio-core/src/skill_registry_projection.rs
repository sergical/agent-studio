use std::path::Path;

use crate::skill_fork_registry::{ForkRegistry, TrialScope};
use crate::skill_inventory::InstalledSkill;

/// Apply loaded registry facts to freshly assembled inventory without reading files.
/// Run before adapter update checks, which depend on the projected fork sources.
pub fn apply_registry_facts(
    home: &Path,
    skills: &mut [InstalledSkill],
    fork_registry: &ForkRegistry,
) {
    for skill in skills.iter_mut() {
        skill.fork = None;
        skill.trial = None;
        skill.trials.clear();
        skill.parked = false;
        skill.parked_at = None;
    }

    // A forked skill is no longer in any ledger, so `classify_source_kind`
    // (which only sees on-disk facts) can't tell it apart from a plain
    // manual directory - the fork registry is the only source of truth for
    // it. Forking only ever applies to the shared `.agents/skills` root, so
    // a same-named project-scoped skill is left alone.
    for skill in skills.iter_mut() {
        let Some(record) = fork_registry.forks.get(&skill.name) else {
            continue;
        };
        let expected_path = if record.skill_dir.as_os_str().is_empty() {
            home.join(".agents/skills").join(&skill.name)
        } else {
            record.skill_dir.clone()
        };
        let Some(deployment) = skill.deployments.iter_mut().find(|deployment| {
            deployment.scope == "global"
                && deployment.destination == crate::skill_deployment::SkillDestination::Universal
                && matches!(
                    deployment.backing,
                    crate::skill_deployment::BackingRelationship::Canonical
                )
                && Path::new(&deployment.path) == expected_path
                && (record.deployment_id.is_empty() || deployment.id == record.deployment_id)
        }) else {
            continue;
        };
        if deployment.owner_kind == crate::skill_ownership::LifecycleOwnerKind::Unknown {
            continue;
        }
        deployment.owner_kind = crate::skill_ownership::LifecycleOwnerKind::Fork;
        deployment.owner_revision =
            crate::skill_fork_registry::RegistryOwnerRecord::Fork(record).revision();
        deployment.owner_id = Some(format!("owner:v1/global/{}", skill.name));
        deployment.mutability = crate::skill_deployment::DeploymentMutability::Mutable;
        skill.source_kind = crate::skill_provenance::SourceKind::Fork;
        skill.fork = Some(crate::skill_inventory::ForkInfo {
            origin_tool: record.origin_tool,
            origin_source: record.origin_source.clone(),
            repo: record.repo.clone(),
            base_commit: record.base_commit.clone(),
            forked_at: record.forked_at.clone(),
        });
        let owner_id = format!("owner:v1/global/{}", skill.name);
        skill
            .update_sources
            .retain(|source| source.owner_id != owner_id);
        skill
            .update_sources
            .push(crate::skill_inventory::OwnerUpdateSource {
                owner_id,
                repo: record.repo.clone(),
                path: Some(record.path.clone()),
                source_ref: record.declared_ref.clone(),
                baseline_identity: Some(format!("fork-base-commit:{}", record.base_commit)),
            });
    }

    // New trial records identify one exact deployment. Version 1 records use
    // scope/name keys and are accepted only when their stored path and scope
    // resolve to exactly one current deployment.
    for skill in skills.iter_mut() {
        let matches: Vec<_> = fork_registry
            .trials
            .values()
            .filter_map(|trial| {
                if trial.status == crate::skill_fork_registry::TrialStatus::RecoveryRequired
                    && crate::skill_deployment::parse_deployment_id(&trial.deployment_id)
                        .is_some_and(|parsed| parsed.name == skill.name)
                {
                    return Some((trial, trial.deployment_id.clone()));
                }
                let candidates: Vec<_> = skill
                    .deployments
                    .iter()
                    .filter(|deployment| {
                        if !trial.deployment_id.is_empty() {
                            return deployment.id == trial.deployment_id;
                        }
                        let scope_matches = match trial.scope {
                            TrialScope::Global => deployment.scope == "global",
                            TrialScope::Project => {
                                deployment.scope == "project"
                                    && deployment.project_path.as_deref()
                                        == trial.project_path.as_deref()
                            }
                        };
                        scope_matches && Path::new(&deployment.path) == trial.skill_dir
                    })
                    .collect();
                (candidates.len() == 1).then(|| (trial, candidates[0].id.clone()))
            })
            .collect();
        skill.trials = matches
            .into_iter()
            .map(|(trial, deployment_id)| crate::skill_inventory::TrialInfo {
                deployment_id,
                expires_at: trial.expires_at.clone(),
                method: trial.method,
                status: trial.status,
                scope: trial.scope,
                project_path: trial.project_path.clone(),
            })
            .collect();
        if skill.trials.len() == 1 {
            skill.trial = skill.trials.first().cloned();
        }
    }

    // Parked skills have no deployment left for `classify_source_kind` to
    // look at, so both the "parked" flag and the badge come straight from
    // the registry's `parked` record instead.
    for skill in skills.iter_mut() {
        if let Some(record) = fork_registry.parked.get(&skill.name).filter(|record| {
            let expected = if record.skill_dir.as_os_str().is_empty() {
                home.join(".agents/skills-parked").join(&skill.name)
            } else {
                record.skill_dir.clone()
            };
            skill.deployments.iter().any(|deployment| {
                deployment.scope == "parked"
                    && Path::new(&deployment.path) == expected
                    && (record.deployment_id.is_empty() || deployment.id == record.deployment_id)
            })
        }) {
            skill.parked = true;
            skill.parked_at = Some(record.parked_at.clone());
            skill.source_kind = record.source_kind;
        }
    }
}
