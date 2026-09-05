// ============================================================================
// Skills Module - skill_lifecycle
// Resolves a deployment or owner id from a current snapshot, revalidates
// path/owner, and previews owner-wide mutations. Commands acquire
// ForkMutationLock before calling into this module.
// ============================================================================

use std::path::{Path, PathBuf};

use super::dotagents_ledger::DotagentsSkill;
use super::skill_deployment::{
    parse_deployment_id, BackingRelationship, DeploymentMutability, SkillDestination,
};
use super::skill_dto::{Deployment, InstallScope, InstalledSkill, LifecycleTarget};
use super::skill_ownership::{parse_owner_id, LifecycleOwnerKind, OwnershipLedgers};
use super::skill_refresh::{self, SkillRefreshState, SkillSnapshot};

/// A lifecycle target resolved from disk and ledgers while the caller holds
/// the mutation lock. The snapshot is retained because owner adapters need
/// the same filesystem view that authorized the mutation.
pub struct FreshLifecycleResolution {
    pub snapshot: SkillSnapshot,
    pub skill: InstalledSkill,
    pub deployment: Deployment,
}

/// One deployment a mutation will touch, for preview and confirmation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AffectedDeployment {
    pub id: String,
    pub name: String,
    pub path: String,
    pub scope: String,
    pub destination: SkillDestination,
    pub owner_kind: LifecycleOwnerKind,
}

/// Snapshot lookup that refuses stale ids.
pub fn find_deployment<'a>(
    snapshot: &'a SkillSnapshot,
    deployment_id: &str,
) -> Result<(&'a InstalledSkill, &'a Deployment), String> {
    for skill in &snapshot.skills {
        if let Some(dep) = skill.deployments.iter().find(|d| d.id == deployment_id) {
            return Ok((skill, dep));
        }
    }
    Err(format!(
        "Deployment {deployment_id} is not in the current snapshot"
    ))
}

/// Re-check that the live path still matches the id's scope/destination/name.
pub fn revalidate_deployment(deployment: &Deployment, expected_id: &str) -> Result<(), String> {
    if deployment.id != expected_id {
        return Err(format!(
            "Deployment id drifted: expected {expected_id}, found {}",
            deployment.id
        ));
    }
    let parsed = parse_deployment_id(expected_id)
        .ok_or_else(|| format!("Not a deployment id: {expected_id}"))?;
    let path = Path::new(&deployment.path);
    let leaf = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if leaf != parsed.name && !deployment.path.contains(&parsed.name) {
        return Err(format!(
            "Deployment path {} no longer matches {}",
            deployment.path, parsed.name
        ));
    }
    if deployment.scope != parsed.scope {
        return Err(format!(
            "Deployment scope drifted: id has {}, snapshot has {}",
            parsed.scope, deployment.scope
        ));
    }
    if deployment.destination != parsed.destination {
        return Err(format!(
            "Deployment destination drifted: id has {}, snapshot has {}",
            parsed.destination.as_str(),
            deployment.destination.as_str()
        ));
    }
    Ok(())
}

fn revalidate_deployment_fingerprint(deployment: &Deployment, action: &str) -> Result<(), String> {
    if deployment.content_hash.is_empty() {
        return Err(format!(
            "{action} refused: {} has no verifiable content fingerprint",
            deployment.path
        ));
    }
    let live_hash = super::skill_discovery::live_skill_content_hash(Path::new(&deployment.path))?;
    if live_hash != deployment.content_hash {
        return Err(format!(
            "{action} refused: {} changed during lifecycle resolution",
            deployment.path
        ));
    }
    Ok(())
}

/// Resolve one deployment or owner group against a newly rebuilt snapshot.
/// Callers must acquire `ForkMutationLock` first. The rebuild lock is only
/// held while reading filesystem state, so watcher refreshes cannot overlap
/// assembly and no refresh lock remains held during the mutation.
pub fn resolve_fresh_lifecycle_target(
    app: &tauri::AppHandle,
    refresh_state: &SkillRefreshState,
    target: &LifecycleTarget,
    action: &str,
) -> Result<FreshLifecycleResolution, String> {
    let snapshot = rebuild_fresh_lifecycle_snapshot(app, refresh_state)?;
    let (skill, deployment) = resolve_lifecycle_target(&snapshot, target, action)?;
    Ok(FreshLifecycleResolution {
        snapshot,
        skill,
        deployment,
    })
}

/// Rebuild the filesystem and ledger view for a mutation with custom target
/// semantics, such as a broken-link repair or restoring a parked deployment.
pub fn rebuild_fresh_lifecycle_snapshot(
    app: &tauri::AppHandle,
    refresh_state: &SkillRefreshState,
) -> Result<SkillSnapshot, String> {
    skill_refresh::rebuild_snapshot_now(app, refresh_state)
}

/// Resolve and verify a lifecycle target inside one already-fresh snapshot.
/// Owner targets validate every deployment that the owner CLI can touch, not
/// only the canonical deployment used to select the adapter.
pub fn resolve_lifecycle_target(
    snapshot: &SkillSnapshot,
    target: &LifecycleTarget,
    action: &str,
) -> Result<(InstalledSkill, Deployment), String> {
    match (&target.deployment_id, &target.owner_id) {
        (Some(id), None) => {
            let (skill, deployment) = find_deployment(snapshot, id)?;
            revalidate_deployment(deployment, id)?;
            require_direct_deployment_mutable(deployment, action)?;
            revalidate_deployment_fingerprint(deployment, action)?;
            Ok((skill.clone(), deployment.clone()))
        }
        (None, Some(owner_id)) => {
            let affected = preview_owner_deployments(snapshot, owner_id)?;
            for affected_deployment in &affected {
                let (_, deployment) = find_deployment(snapshot, &affected_deployment.id)?;
                revalidate_deployment(deployment, &affected_deployment.id)?;
                if deployment.owner_id.as_deref() != Some(owner_id) {
                    return Err(format!(
                        "{action} refused: deployment {} changed owner",
                        deployment.id
                    ));
                }
                revalidate_deployment_fingerprint(deployment, action)?;
            }
            let selected = affected
                .iter()
                .find(|affected_deployment| {
                    matches!(
                        find_deployment(snapshot, &affected_deployment.id),
                        Ok((_, found)) if matches!(found.backing, BackingRelationship::Canonical)
                    )
                })
                .unwrap_or(&affected[0]);
            let (skill, deployment) = find_deployment(snapshot, &selected.id)?;
            require_owner_adapter_deployment_mutable(deployment, action)?;
            Ok((skill.clone(), deployment.clone()))
        }
        _ => Err("Lifecycle target must contain exactly one deployment_id or owner_id".to_string()),
    }
}

/// Park and unpark only move the Global Universal folder. Project and Per
/// harness copies stay independent.
pub fn require_global_universal_park_target(deployment: &Deployment) -> Result<(), String> {
    if deployment.scope == "parked" {
        return Ok(());
    }
    if deployment.scope == "global"
        && deployment.destination == SkillDestination::Universal
        && matches!(
            deployment.backing,
            super::skill_deployment::BackingRelationship::Canonical
        )
        && deployment.plugin.is_none()
    {
        return Ok(());
    }
    Err(
        "Park is only available for the Global Universal folder. Project and Per harness copies stay independent."
            .to_string(),
    )
}

/// Require a direct deployment target to be safe for an owner adapter.
pub fn require_direct_deployment_mutable(
    deployment: &Deployment,
    action: &str,
) -> Result<(), String> {
    if deployment.owner_kind.is_mutable() && deployment.mutability == DeploymentMutability::Mutable
    {
        return Ok(());
    }
    Err(format!(
        "{action} is not available: {} is {} (read-only)",
        deployment.path,
        deployment.owner_kind.as_str()
    ))
}

/// Require the deployment selected to represent an owner group to be mutable.
/// Read-only dependent links in that group are validated separately and stay
/// in the affected set because the owner adapter can clean them up indirectly.
fn require_owner_adapter_deployment_mutable(
    deployment: &Deployment,
    action: &str,
) -> Result<(), String> {
    require_direct_deployment_mutable(deployment, action)
}

/// Every deployment sharing `owner_id` in the current snapshot, including
/// read-only dependent links that an owner-wide action can clean up.
pub fn preview_owner_deployments(
    snapshot: &SkillSnapshot,
    owner_id: &str,
) -> Result<Vec<AffectedDeployment>, String> {
    let parsed = parse_owner_id(owner_id).ok_or_else(|| format!("Not an owner id: {owner_id}"))?;
    let mut out = Vec::new();
    for skill in &snapshot.skills {
        if skill.name != parsed.name {
            continue;
        }
        for dep in &skill.deployments {
            if dep.owner_id.as_deref() != Some(owner_id) {
                continue;
            }
            out.push(AffectedDeployment {
                id: dep.id.clone(),
                name: skill.name.clone(),
                path: dep.path.clone(),
                scope: dep.scope.clone(),
                destination: dep.destination,
                owner_kind: dep.owner_kind,
            });
        }
    }
    if out.is_empty() {
        return Err(format!(
            "Owner {owner_id} has no matching deployments in the current snapshot"
        ));
    }
    Ok(out)
}

pub fn skills_sh_update_args(name: &str, scope: InstallScope) -> Vec<String> {
    let mut args = vec!["skills".to_string(), "update".to_string(), name.to_string()];
    if scope == InstallScope::Global {
        args.push("--global".to_string());
    }
    args
}

pub fn skills_sh_remove_args_for_scope(name: &str, scope: InstallScope) -> Vec<String> {
    let mut args = vec![
        "skills".to_string(),
        "remove".to_string(),
        name.to_string(),
        "--yes".to_string(),
    ];
    if scope == InstallScope::Global {
        args.push("--global".to_string());
    }
    args
}

pub fn dotagents_update_args(
    skill_name: &str,
    entry: Option<&DotagentsSkill>,
    latest_commit: Option<&str>,
    scope: InstallScope,
) -> Result<Vec<String>, String> {
    let Some(entry) = entry else {
        return Err(format!(
            "Update is not available: {skill_name} is not in the matching agents.lock"
        ));
    };
    if !entry.has_manifest_row {
        return Err(format!(
            "Update is not available: {skill_name} is a wildcard dotagents entry"
        ));
    }
    let mut args = vec!["-y".to_string(), "@sentry/dotagents".to_string()];
    if scope == InstallScope::Project {
        args.push("--project".to_string());
    }
    args.extend([
        "add".to_string(),
        entry.source.clone(),
        "--name".to_string(),
        skill_name.to_string(),
    ]);
    if entry.declared_ref.is_some() {
        match latest_commit {
            Some(latest) => {
                args.push("--ref".to_string());
                args.push(latest.to_string());
            }
            None => {
                return Err(format!(
                    "Update is not available yet: run \"Check now\" to find {skill_name}'s latest commit first"
                ));
            }
        }
    }
    Ok(args)
}

pub fn ledger_matching_deployment<'a>(
    ledgers: &'a [OwnershipLedgers],
    deployment: &Deployment,
) -> Option<&'a OwnershipLedgers> {
    let parsed = parse_deployment_id(&deployment.id)?;
    ledgers.iter().find(|ledger| match parsed.scope.as_str() {
        "global" => ledger.scope == InstallScope::Global,
        "project" => {
            ledger.scope == InstallScope::Project
                && ledger
                    .project_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    == parsed.project_path
        }
        _ => false,
    })
}

/// Claude Code skills dir that must match the selected Universal root.
pub fn claude_skills_dir_for_scope(
    home: &Path,
    scope: InstallScope,
    project_path: Option<&Path>,
) -> PathBuf {
    match scope {
        InstallScope::Global => home.join(".claude").join("skills"),
        InstallScope::Project => project_path
            .unwrap_or_else(|| Path::new(""))
            .join(".claude")
            .join("skills"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::frontmatter::InvocationPolicy;
    use crate::skills::provenance::SourceKind;
    use crate::skills::skill_deployment::{
        deployment_id, BackingRelationship, DeploymentMutability,
    };
    use crate::skills::skill_invocations::InvocationHeatmap;
    use chrono::Utc;
    use std::collections::BTreeMap;

    fn dep(id: &str, name: &str, path: &str, scope: &str, dest: SkillDestination) -> Deployment {
        Deployment {
            id: id.to_string(),
            destination: dest,
            owner_kind: LifecycleOwnerKind::SkillsSh,
            owner_id: Some(format!("owner:v1/{scope}/{name}")),
            mutability: DeploymentMutability::Mutable,
            backing: if dest == SkillDestination::Universal {
                BackingRelationship::Canonical
            } else {
                BackingRelationship::Independent
            },
            agent: if dest == SkillDestination::Universal {
                "shared".to_string()
            } else {
                "Codex".to_string()
            },
            scope: scope.to_string(),
            path: path.to_string(),
            is_symlink: false,
            plugin: None,
            symlink_target: None,
            resolved_path: None,
            symlink_is_broken: false,
            symlink_error: None,
            project_path: None,
            content_hash: String::new(),
            disabled: false,
            disabled_by: None,
            disabled_readers: Vec::new(),
            codex_implicit_invocation: None,
            shared_via_whole_dir_link: false,
            spec_violations: Vec::new(),
            invocation: InvocationPolicy::Both,
        }
    }

    fn snapshot(deployments: Vec<Deployment>) -> SkillSnapshot {
        SkillSnapshot {
            skills: vec![InstalledSkill {
                name: "find-bugs".to_string(),
                source: "o/r".to_string(),
                source_type: "github".to_string(),
                source_url: None,
                skill_path: None,
                installed_at: Utc::now().to_rfc3339(),
                updated_at: None,
                has_update: false,
                update_owner_ids: Vec::new(),
                update_owners: Vec::new(),
                update_commit: None,
                update_commit_at: None,
                source_kind: SourceKind::SkillsSh,
                deployments,
                has_spec: false,
                description: None,
                spec_violations: Vec::new(),
                skill_md_tokens: 0,
                description_tokens: 0,
                folder_bytes: 0,
                file_count: 0,
                content_hash: String::new(),
                content_hashes: Vec::new(),
                modified_at: None,
                frontmatter_fields: BTreeMap::new(),
                folder_truncated: false,
                fork: None,
                trial: None,
                trials: Vec::new(),
                parked: false,
                parked_at: None,
                invocation: InvocationPolicy::Both,
            }],
            projects: Vec::new(),
            invocations: Vec::new(),
            heatmap: InvocationHeatmap::default(),
            scanned_at: Utc::now().to_rfc3339(),
            last_test_by_skill: Default::default(),
            update_check: Default::default(),
            opencode_config_kind: None,
        }
    }

    #[test]
    fn find_deployment_requires_current_id() {
        let id = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            Path::new("/h/.agents/skills/find-bugs"),
        );
        let snap = snapshot(vec![dep(
            &id,
            "find-bugs",
            "/h/.agents/skills/find-bugs",
            "global",
            SkillDestination::Universal,
        )]);
        assert!(find_deployment(&snap, &id).is_ok());
        assert!(find_deployment(&snap, "dep:v1/project/universal/universal/find-bugs/-").is_err());
    }

    #[test]
    fn owner_preview_does_not_include_other_scope() {
        let global_id = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            Path::new("/h/.agents/skills/find-bugs"),
        );
        let project_id = deployment_id(
            "find-bugs",
            "project",
            SkillDestination::Universal,
            "universal",
            Some("/work/app"),
            Path::new("/work/app/.agents/skills/find-bugs"),
        );
        let mut g = dep(
            &global_id,
            "find-bugs",
            "/h/.agents/skills/find-bugs",
            "global",
            SkillDestination::Universal,
        );
        g.owner_id = Some("owner:v1/global/find-bugs".to_string());
        let mut p = dep(
            &project_id,
            "find-bugs",
            "/work/app/.agents/skills/find-bugs",
            "project",
            SkillDestination::Universal,
        );
        p.owner_id = Some("owner:v1/project/%2Fwork%2Fapp/find-bugs".to_string());
        p.project_path = Some("/work/app".to_string());
        let snap = snapshot(vec![g, p]);
        let preview = preview_owner_deployments(&snap, "owner:v1/global/find-bugs").unwrap();
        assert_eq!(preview.len(), 1);
        assert_eq!(preview[0].id, global_id);
    }

    #[test]
    fn direct_deployment_requires_mutable_owner_kind_and_mutability() {
        let mut d = dep(
            "dep:v1/global/codex/per-harness/find-bugs/-",
            "find-bugs",
            "/h/.codex/skills/find-bugs",
            "global",
            SkillDestination::PerHarness,
        );
        d.owner_kind = LifecycleOwnerKind::Manual;
        d.mutability = DeploymentMutability::ReadOnly;
        let err = require_direct_deployment_mutable(&d, "Update").unwrap_err();
        assert!(err.contains("read-only"));

        d.owner_kind = LifecycleOwnerKind::SkillsSh;
        let err = require_direct_deployment_mutable(&d, "Update").unwrap_err();
        assert!(err.contains("read-only"));
    }

    #[test]
    fn skills_sh_update_args_global_flag() {
        assert!(
            skills_sh_update_args("foo", InstallScope::Global).contains(&"--global".to_string())
        );
        assert!(
            !skills_sh_update_args("foo", InstallScope::Project).contains(&"--global".to_string())
        );
    }

    #[test]
    fn lifecycle_resolution_refuses_owner_changed_after_cached_snapshot() {
        let id = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            Path::new("/h/.agents/skills/find-bugs"),
        );
        let mut cached_deployment = dep(
            &id,
            "find-bugs",
            "/h/.agents/skills/find-bugs",
            "global",
            SkillDestination::Universal,
        );
        cached_deployment.owner_id = Some("owner:v1/global/find-bugs".to_string());
        let cached = snapshot(vec![cached_deployment.clone()]);
        assert!(preview_owner_deployments(&cached, "owner:v1/global/find-bugs").is_ok());

        cached_deployment.owner_kind = LifecycleOwnerKind::Ambiguous;
        cached_deployment.owner_id = None;
        cached_deployment.mutability = DeploymentMutability::ReadOnly;
        let fresh = snapshot(vec![cached_deployment]);
        let error = resolve_lifecycle_target(
            &fresh,
            &LifecycleTarget {
                deployment_id: None,
                owner_id: Some("owner:v1/global/find-bugs".to_string()),
            },
            "Remove",
        )
        .unwrap_err();

        assert!(error.contains("no matching deployments"));
    }

    #[test]
    fn lifecycle_resolution_refuses_selected_deployment_changed_to_ambiguous_owner() {
        let id = deployment_id(
            "find-bugs",
            "project",
            SkillDestination::Universal,
            "universal",
            Some("/work/app"),
            Path::new("/work/app/.agents/skills/find-bugs"),
        );
        let mut fresh_deployment = dep(
            &id,
            "find-bugs",
            "/work/app/.agents/skills/find-bugs",
            "project",
            SkillDestination::Universal,
        );
        fresh_deployment.owner_kind = LifecycleOwnerKind::Ambiguous;
        fresh_deployment.owner_id = None;
        fresh_deployment.mutability = DeploymentMutability::ReadOnly;
        let fresh = snapshot(vec![fresh_deployment]);

        let error = resolve_lifecycle_target(
            &fresh,
            &LifecycleTarget {
                deployment_id: Some(id),
                owner_id: None,
            },
            "Update",
        )
        .unwrap_err();

        assert!(error.contains("read-only"));
    }

    #[test]
    fn owner_resolution_allows_read_only_dependent_links() {
        let tmp = tempfile::tempdir().unwrap();
        let canonical_path = tmp.path().join(".agents/skills/find-bugs");
        std::fs::create_dir_all(&canonical_path).unwrap();
        std::fs::write(
            canonical_path.join("SKILL.md"),
            "---\nname: find-bugs\n---\n",
        )
        .unwrap();
        let linked_path = tmp.path().join(".codex/skills/find-bugs");
        std::fs::create_dir_all(linked_path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&canonical_path, &linked_path).unwrap();

        let canonical_id = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            &canonical_path,
        );
        let linked_id = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::PerHarness,
            "codex",
            None,
            &linked_path,
        );
        let content_hash =
            crate::skills::skill_discovery::live_skill_content_hash(&canonical_path).unwrap();
        let owner_id = "owner:v1/global/find-bugs";
        let mut canonical = dep(
            &canonical_id,
            "find-bugs",
            canonical_path.to_str().unwrap(),
            "global",
            SkillDestination::Universal,
        );
        canonical.owner_id = Some(owner_id.to_string());
        canonical.content_hash.clone_from(&content_hash);
        let mut linked = dep(
            &linked_id,
            "find-bugs",
            linked_path.to_str().unwrap(),
            "global",
            SkillDestination::PerHarness,
        );
        linked.owner_id = Some(owner_id.to_string());
        linked.mutability = DeploymentMutability::ReadOnly;
        linked.backing = BackingRelationship::LinkedTo {
            deployment_id: canonical_id.clone(),
        };
        linked.content_hash = content_hash;
        let snap = snapshot(vec![canonical, linked]);

        let affected = preview_owner_deployments(&snap, owner_id).unwrap();
        assert_eq!(affected.len(), 2);
        let (_, selected) = resolve_lifecycle_target(
            &snap,
            &LifecycleTarget {
                deployment_id: None,
                owner_id: Some(owner_id.to_string()),
            },
            "Remove",
        )
        .unwrap();
        assert_eq!(selected.id, canonical_id);
    }

    #[test]
    fn owner_resolution_requires_mutable_canonical_deployment() {
        let tmp = tempfile::tempdir().unwrap();
        let canonical_path = tmp.path().join(".agents/skills/find-bugs");
        std::fs::create_dir_all(&canonical_path).unwrap();
        std::fs::write(
            canonical_path.join("SKILL.md"),
            "---\nname: find-bugs\n---\n",
        )
        .unwrap();
        let canonical_id = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            &canonical_path,
        );
        let mut canonical = dep(
            &canonical_id,
            "find-bugs",
            canonical_path.to_str().unwrap(),
            "global",
            SkillDestination::Universal,
        );
        canonical.content_hash =
            crate::skills::skill_discovery::live_skill_content_hash(&canonical_path).unwrap();
        canonical.mutability = DeploymentMutability::ReadOnly;
        let snap = snapshot(vec![canonical]);

        let error = resolve_lifecycle_target(
            &snap,
            &LifecycleTarget {
                deployment_id: None,
                owner_id: Some("owner:v1/global/find-bugs".to_string()),
            },
            "Update",
        )
        .unwrap_err();

        assert!(error.contains("read-only"));
    }

    #[test]
    fn park_refuses_project_and_per_harness() {
        let mut project = dep(
            "dep:v1/project/universal/universal/find-bugs/%2Fwork/-",
            "find-bugs",
            "/work/.agents/skills/find-bugs",
            "project",
            SkillDestination::Universal,
        );
        project.scope = "project".to_string();
        assert!(require_global_universal_park_target(&project).is_err());

        let per = dep(
            "dep:v1/global/codex/per-harness/find-bugs/-",
            "find-bugs",
            "/h/.codex/skills/find-bugs",
            "global",
            SkillDestination::PerHarness,
        );
        assert!(require_global_universal_park_target(&per).is_err());
    }
}
