// ============================================================================
// Skills Module - Skill Assembly
// Pure merge of core's scan Inventory into InstalledSkill records, one per
// skill name across all its deployments. Never touches the filesystem -
// every rule here is testable with hand-built DeploymentDto fixtures. Every
// scan fact (identity, destination, owner, backing, content facts) comes
// straight from `skill_studio_core::ops::scan`'s output; this module only
// adds assembly concerns scan doesn't have an opinion on: lock-file
// metadata, per-skill aggregation, and the LinkedTo target id core's DTO
// intentionally omits (see `resolve_linked_backing_ids`).
// ============================================================================

use std::collections::HashMap;
use std::path::Path;

use skill_studio_core::dto::{DeploymentDto, InstalledSkillDto};
use skill_studio_core::identity::{
    AgentId as CoreAgentId, BackingRelationship as CoreBackingRelationship,
    DeploymentMutability as CoreDeploymentMutability, LifecycleOwnerKind as CoreLifecycleOwnerKind,
    RootKind, RootRef, RootScope, SkillDestination as CoreSkillDestination, SourceKind,
};

use super::frontmatter::invocation_policy_from;
use super::lock_file::SkillLockFile;
use super::skill_deployment::{
    BackingRelationship, DeploymentMutability, SkillDestination, UNIVERSAL_ROOT_LABEL,
};
use super::skill_dto::{Deployment, DisabledBy, InstalledSkill, PluginInfo};
use super::skill_ownership::LifecycleOwnerKind;

fn destination_from_core(destination: CoreSkillDestination) -> SkillDestination {
    match destination {
        CoreSkillDestination::Universal => SkillDestination::Universal,
        CoreSkillDestination::PerHarness => SkillDestination::PerHarness,
    }
}

fn owner_kind_from_core(kind: CoreLifecycleOwnerKind) -> LifecycleOwnerKind {
    match kind {
        CoreLifecycleOwnerKind::SkillsSh => LifecycleOwnerKind::SkillsSh,
        CoreLifecycleOwnerKind::Dotagents => LifecycleOwnerKind::Dotagents,
        CoreLifecycleOwnerKind::Copy => LifecycleOwnerKind::Copy,
        CoreLifecycleOwnerKind::Fork => LifecycleOwnerKind::Fork,
        CoreLifecycleOwnerKind::Plugin => LifecycleOwnerKind::Plugin,
        CoreLifecycleOwnerKind::InRepo => LifecycleOwnerKind::InRepo,
        CoreLifecycleOwnerKind::Manual => LifecycleOwnerKind::Manual,
        CoreLifecycleOwnerKind::WildcardDotagents => LifecycleOwnerKind::WildcardDotagents,
        CoreLifecycleOwnerKind::Ambiguous => LifecycleOwnerKind::Ambiguous,
    }
}

fn mutability_from_core(mutability: CoreDeploymentMutability) -> DeploymentMutability {
    match mutability {
        CoreDeploymentMutability::Mutable => DeploymentMutability::Mutable,
        CoreDeploymentMutability::ReadOnly => DeploymentMutability::ReadOnly,
    }
}

fn backing_from_core(backing: CoreBackingRelationship) -> BackingRelationship {
    match backing {
        CoreBackingRelationship::Canonical => BackingRelationship::Canonical,
        CoreBackingRelationship::Independent => BackingRelationship::Independent,
        // Core's `BackingRelationship` carries no target id (a documented
        // parity exclusion - the id format is a core-internal detail).
        // `resolve_linked_backing_ids` fills this in once every deployment
        // in this skill's group has been assembled.
        CoreBackingRelationship::LinkedTo => BackingRelationship::LinkedTo {
            deployment_id: String::new(),
        },
    }
}

/// Desktop's `agent` label - the display name for a harness, or the
/// compatibility labels `shared`/`parked` for the roots that aren't
/// harness-owned. `RootRef::kind` alone decides which case applies; `harness`
/// is only consulted for the harness-owned cases (`scan_targets`/
/// `plugin_scan_targets` in `ops.rs` set it exactly there, `None` otherwise).
fn agent_label_from_core(root: &RootRef, harness: Option<&CoreAgentId>) -> String {
    match &root.kind {
        RootKind::Universal => UNIVERSAL_ROOT_LABEL.to_string(),
        RootKind::Parked => "parked".to_string(),
        RootKind::Harness(_) | RootKind::Legacy(_) | RootKind::PluginCache(_) => harness
            .map(|id| agent_display_label(id.as_str()).to_string())
            .unwrap_or_default(),
    }
}

fn agent_display_label(id: &str) -> &'static str {
    match id {
        CoreAgentId::CLAUDE_CODE => "Claude Code",
        CoreAgentId::CODEX => "Codex",
        CoreAgentId::OPEN_CODE => "OpenCode",
        CoreAgentId::PI => "pi",
        CoreAgentId::CURSOR => "Cursor",
        CoreAgentId::GROK_BUILD => "Grok Build",
        // Core only ever hands back the six harnesses above.
        _ => "unknown",
    }
}

/// Desktop's `scope` string: "global" | "project" | "plugin" | "parked".
fn scope_from_core(root: &RootRef) -> String {
    match root.kind {
        RootKind::PluginCache(_) => "plugin".to_string(),
        RootKind::Parked => "parked".to_string(),
        _ => match &root.scope {
            RootScope::Global => "global".to_string(),
            RootScope::Project(_) => "project".to_string(),
        },
    }
}

fn project_path_from_core(root: &RootRef) -> Option<String> {
    match &root.scope {
        RootScope::Global => None,
        RootScope::Project(project) => Some(project.0.to_string_lossy().to_string()),
    }
}

fn deployment_from_core(dto: &DeploymentDto) -> Deployment {
    let agent = agent_label_from_core(&dto.root, dto.harness.as_ref());
    // Typed frontmatter, not the stringified `frontmatter_fields`: the two
    // disagree when the typed parse fails (a quoted `"true"`, an unrelated
    // malformed key), and a deployment whose SKILL.md does not parse has
    // always read as `Both` - the snapshot-level policy in `skill_refresh`
    // is the one that works from strings.
    let disable_model = dto
        .frontmatter
        .as_ref()
        .and_then(|f| f.disable_model_invocation);
    let user_invocable = dto.frontmatter.as_ref().and_then(|f| f.user_invocable);
    let claude_plugin_disabled = dto
        .plugin
        .as_ref()
        .is_some_and(|p| p.enabled == Some(false));
    Deployment {
        id: dto.id.as_str().to_string(),
        destination: destination_from_core(dto.destination),
        owner_kind: owner_kind_from_core(dto.owner_kind),
        owner_id: dto.owner_id.as_ref().map(|id| id.as_str().to_string()),
        mutability: mutability_from_core(dto.mutability),
        backing: backing_from_core(dto.backing),
        plugin: dto.plugin.as_ref().map(|source| PluginInfo {
            name: source.plugin.clone(),
            version: source.version.clone(),
            harness: agent.clone(),
            marketplace: source.marketplace.clone(),
            id: format!("{}@{}", source.plugin, source.marketplace),
        }),
        scope: scope_from_core(&dto.root),
        path: dto.path.to_string_lossy().to_string(),
        is_symlink: dto.is_symlink,
        symlink_target: dto
            .link_target
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        symlink_is_broken: dto.symlink_is_broken,
        symlink_error: dto.symlink_error.clone(),
        project_path: project_path_from_core(&dto.root),
        resolved_path: dto
            .resolved_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        content_hash: dto.content_hash.clone(),
        // Native disable mechanisms (Codex config, OpenCode permission,
        // Claude link removed) are overlaid unconditionally after assembly
        // in `skill_refresh::apply_skill_snapshot_overlays` - not scan
        // facts. Only the "moved aside by Skill Studio" mechanism and the
        // Claude Code plugin-enabled state are scan facts: `StudioMoved`
        // wins when both apply, since it is set first here.
        disabled: dto.studio_disabled || claude_plugin_disabled,
        disabled_by: if dto.studio_disabled {
            Some(DisabledBy::StudioMoved)
        } else if claude_plugin_disabled {
            Some(DisabledBy::ClaudePluginDisabled)
        } else {
            None
        },
        disabled_readers: Vec::new(),
        codex_implicit_invocation: None,
        shared_via_whole_dir_link: dto.shared_via_whole_dir_link,
        spec_violations: dto.spec_violations.clone(),
        invocation: invocation_policy_from(disable_model, user_invocable).0,
        agent,
    }
}

/// Build a fresh InstalledSkill, seeding metadata from the lock file entry
/// when one exists for this skill name, or generic "local directory"
/// metadata otherwise.
fn new_installed_skill(
    name: &str,
    lock: &SkillLockFile,
    source_kind: SourceKind,
) -> InstalledSkill {
    let (source, source_type, source_url, skill_path, installed_at, updated_at) =
        match lock.skills.get(name) {
            Some(entry) => (
                entry.source.clone(),
                entry.source_type.clone(),
                Some(entry.source_url.clone()),
                entry.skill_path.clone(),
                entry.installed_at.clone(),
                Some(entry.updated_at.clone()),
            ),
            None => (
                "local".to_string(),
                "directory".to_string(),
                None,
                None,
                String::new(),
                None,
            ),
        };

    InstalledSkill {
        name: name.to_string(),
        source,
        source_type,
        source_url,
        skill_path,
        installed_at,
        updated_at,
        has_update: false,
        update_owner_ids: Vec::new(),
        update_owners: Vec::new(),
        update_commit: None,
        update_commit_at: None,
        source_kind,
        deployments: Vec::new(),
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
        frontmatter_fields: std::collections::BTreeMap::new(),
        folder_truncated: false,
        fork: None,
        trial: None,
        trials: Vec::new(),
        parked: false,
        parked_at: None,
        invocation: crate::skills::frontmatter::InvocationPolicy::Both,
    }
}

/// Merge core's scan Inventory by name into one InstalledSkill per skill,
/// with one Deployment per `DeploymentDto`. Every deployment-level fact
/// (identity, destination, owner, mutability, backing, content facts) comes
/// straight from core's `scan`; this only aggregates by name (source_kind
/// precedence, deduped violations/hashes, first-readable-wins aggregate
/// facts), seeds from the lock entry, keeps lock-only skills (in the lock
/// file, not found by scan) with empty deployments and source_kind
/// skills-sh, and resolves the one identity core's DTO omits: the target id
/// of a `LinkedTo` backing relationship (see `resolve_linked_backing_ids`).
pub fn assemble_installed_skills(
    core_skills: Vec<InstalledSkillDto>,
    lock: &SkillLockFile,
) -> Vec<InstalledSkill> {
    let mut by_name: HashMap<String, InstalledSkill> = HashMap::new();

    for core_skill in &core_skills {
        let name = &core_skill.name.0;
        for deployment_dto in &core_skill.deployments {
            let source_kind = deployment_dto.source_kind;
            let record = by_name
                .entry(name.clone())
                .or_insert_with(|| new_installed_skill(name, lock, source_kind));

            if source_kind < record.source_kind {
                record.source_kind = source_kind;
            }
            for violation in &deployment_dto.spec_violations {
                if !record.spec_violations.contains(violation) {
                    record.spec_violations.push(violation.clone());
                }
            }
            if !deployment_dto.content_hash.is_empty()
                && !record.content_hashes.contains(&deployment_dto.content_hash)
            {
                record
                    .content_hashes
                    .push(deployment_dto.content_hash.clone());
            }

            // Aggregate facts come from the first deployment whose content
            // was actually readable (non-empty hash), not the first by
            // order: a broken symlink deployment seen before a valid one
            // must not blank out the skill's real hash/tokens/etc.
            if !deployment_dto.content_hash.is_empty() && record.content_hash.is_empty() {
                record.skill_md_tokens = deployment_dto.skill_md_tokens;
                record.description_tokens = deployment_dto.description_tokens;
                record.folder_bytes = deployment_dto.folder_bytes;
                record.file_count = deployment_dto.file_count;
                record.content_hash = deployment_dto.content_hash.clone();
                record.modified_at = deployment_dto.modified_at.map(|t| t.to_rfc3339());
                record.frontmatter_fields = deployment_dto.frontmatter_fields.clone();
                record.folder_truncated = deployment_dto.folder_truncated;
                record.has_spec = deployment_dto.has_spec;
                record.description = deployment_dto
                    .frontmatter
                    .as_ref()
                    .and_then(|f| f.description.clone());
            }

            record
                .deployments
                .push(deployment_from_core(deployment_dto));
        }
    }

    // Keep lock-file entries that weren't found on disk (e.g. deployed to an
    // agent we don't scan, or removed manually without updating the lock).
    for name in lock.skills.keys() {
        by_name
            .entry(name.clone())
            .or_insert_with(|| new_installed_skill(name, lock, SourceKind::SkillsSh));
    }

    let mut skills: Vec<InstalledSkill> = by_name.into_values().collect();
    resolve_linked_backing_ids(&mut skills);
    // HashMap order is random per process; a stable name order keeps every
    // list (Home updates, Skills) from reshuffling between rescans.
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Fills in the target id `backing_from_core` left blank for every `LinkedTo`
/// deployment, by matching its resolved path against the Universal canonical
/// deployment for the same skill/scope/project. Owner identity and
/// mutability are not touched here - core's `propagate_verified_linked_owners`
/// (`ops.rs`) already corrected those before this DTO left `scan`, so redoing
/// that pass here would be redundant.
fn resolve_linked_backing_ids(skills: &mut [InstalledSkill]) {
    let canonical_owners: Vec<_> = skills
        .iter()
        .flat_map(|skill| {
            skill
                .deployments
                .iter()
                .filter(|deployment| {
                    deployment.destination == SkillDestination::Universal
                        && matches!(deployment.backing, BackingRelationship::Canonical)
                })
                .map(move |deployment| {
                    (
                        deployment.id.clone(),
                        (
                            skill.name.clone(),
                            deployment
                                .resolved_path
                                .clone()
                                .unwrap_or_else(|| deployment.path.clone()),
                            deployment.scope.clone(),
                            deployment.project_path.clone(),
                        ),
                    )
                })
        })
        .collect();

    for skill in skills {
        for deployment in &mut skill.deployments {
            if !matches!(deployment.backing, BackingRelationship::LinkedTo { .. }) {
                continue;
            }
            let Some(linked_resolved_path) = deployment.resolved_path.as_deref() else {
                continue;
            };
            let mut matches = canonical_owners.iter().filter(|(_, owner)| {
                let (name, canonical_resolved_path, scope, project_path) = owner;
                skill.name == *name
                    && deployment.scope == *scope
                    && deployment.project_path == *project_path
                    && Path::new(linked_resolved_path) == Path::new(canonical_resolved_path)
            });
            let Some((canonical_id, _)) = matches.next() else {
                continue;
            };
            if matches.next().is_some() {
                continue;
            }
            deployment.backing = BackingRelationship::LinkedTo {
                deployment_id: canonical_id.clone(),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::path::{Path, PathBuf};

    use skill_studio_core::identity::{DeploymentId, ProjectRef, SkillName};

    use super::*;
    use crate::skills::lock_file::InstalledSkillEntry;

    /// Global root for a desktop-style root label ("shared", "Claude Code",
    /// "Codex", "pi", "Cursor", "Grok Build", "OpenCode"), plus the harness
    /// id feeding that root when it has one.
    fn root_for_label(root_label: &str) -> (RootRef, Option<CoreAgentId>) {
        if root_label == "shared" {
            return (
                RootRef::new(RootScope::Global, RootKind::Universal).unwrap(),
                None,
            );
        }
        let harness = CoreAgentId::from(match root_label {
            "Claude Code" => CoreAgentId::CLAUDE_CODE,
            "Codex" => CoreAgentId::CODEX,
            "pi" => CoreAgentId::PI,
            "Cursor" => CoreAgentId::CURSOR,
            "Grok Build" => CoreAgentId::GROK_BUILD,
            "OpenCode" => CoreAgentId::OPEN_CODE,
            other => panic!("unknown fixture root label {other}"),
        });
        (
            RootRef::new(RootScope::Global, RootKind::Harness(harness.clone())).unwrap(),
            Some(harness),
        )
    }

    /// A minimal, independent (non-linked) deployment, matching the shape
    /// `ops::scan` would hand back for a plain on-disk skill folder.
    fn deployment_dto(name: &str, root_label: &str) -> DeploymentDto {
        let (root, harness) = root_for_label(root_label);
        let destination = if root_label == "shared" {
            CoreSkillDestination::Universal
        } else {
            CoreSkillDestination::PerHarness
        };
        DeploymentDto {
            id: DeploymentId::parse(&format!("dep:v1/test/{root_label}/{name}")).unwrap(),
            root,
            harness,
            path: PathBuf::from(format!("/tmp/{root_label}/{name}")),
            destination,
            backing: CoreBackingRelationship::Independent,
            mutability: CoreDeploymentMutability::Mutable,
            link_target: None,
            shared_via_whole_dir_link: false,
            is_symlink: false,
            resolved_path: None,
            symlink_is_broken: false,
            symlink_error: None,
            owner_kind: CoreLifecycleOwnerKind::Manual,
            owner_id: None,
            content_fingerprint: None,
            disabled_by: None,
            disabled_readers: Vec::new(),
            spec_violations: Vec::new(),
            plugin: None,
            frontmatter: None,
            frontmatter_fields: BTreeMap::new(),
            has_spec: false,
            folder_bytes: 0,
            file_count: 0,
            skill_md_tokens: 0,
            description_tokens: 0,
            content_hash: String::new(),
            modified_at: None,
            folder_truncated: false,
            in_git_repo: false,
            studio_disabled: false,
            source_kind: SourceKind::Manual,
        }
    }

    /// Same as `deployment_dto`, but under a tracked project rather than the
    /// global scope.
    fn deployment_dto_project(name: &str, root_label: &str, project: &Path) -> DeploymentDto {
        let mut dto = deployment_dto(name, root_label);
        dto.root = RootRef::new(
            RootScope::Project(ProjectRef(project.to_path_buf())),
            dto.root.kind,
        )
        .unwrap();
        dto
    }

    /// Groups a flat list of (name, deployment) fixtures into core's
    /// `InstalledSkillDto` shape, one entry per distinct name, preserving
    /// first-seen order - mirrors how `ops::scan` groups deployments.
    fn skills_from(deployments: Vec<(&str, DeploymentDto)>) -> Vec<InstalledSkillDto> {
        let mut order: Vec<String> = Vec::new();
        let mut by_name: HashMap<String, Vec<DeploymentDto>> = HashMap::new();
        for (name, dto) in deployments {
            if !by_name.contains_key(name) {
                order.push(name.to_string());
            }
            by_name.entry(name.to_string()).or_default().push(dto);
        }
        order
            .into_iter()
            .map(|name| InstalledSkillDto {
                deployments: by_name.remove(&name).unwrap(),
                description: None,
                name: SkillName(name),
            })
            .collect()
    }

    fn empty_lock() -> SkillLockFile {
        SkillLockFile {
            version: 3,
            skills: HashMap::new(),
        }
    }

    #[test]
    fn precedence_dotagents_beats_plugin_beats_skills_sh_beats_manual() {
        let mut dotagents = deployment_dto("my-skill", "shared");
        dotagents.source_kind = SourceKind::Dotagents;

        let mut plugin = deployment_dto("my-skill", "Claude Code");
        plugin.source_kind = SourceKind::Plugin;
        plugin.plugin = Some(skill_studio_core::dto::PluginSourceDto {
            marketplace: "some-marketplace".to_string(),
            plugin: "some-plugin".to_string(),
            version: None,
            enabled: None,
        });

        let mut manual = deployment_dto("my-skill", "Codex");
        manual.source_kind = SourceKind::Manual;

        let skills = assemble_installed_skills(
            skills_from(vec![
                ("my-skill", manual),
                ("my-skill", plugin),
                ("my-skill", dotagents),
            ]),
            &empty_lock(),
        );
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].source_kind, SourceKind::Dotagents);
        assert_eq!(skills[0].deployments.len(), 3);
    }

    #[test]
    fn claude_plugin_disabled_by_harness_disables_the_deployment() {
        let mut plugin = deployment_dto("my-skill", "Claude Code");
        plugin.plugin = Some(skill_studio_core::dto::PluginSourceDto {
            marketplace: "some-marketplace".to_string(),
            plugin: "some-plugin".to_string(),
            version: None,
            enabled: Some(false),
        });

        let skills =
            assemble_installed_skills(skills_from(vec![("my-skill", plugin)]), &empty_lock());
        let deployment = &skills[0].deployments[0];
        assert!(deployment.disabled);
        assert_eq!(
            deployment.disabled_by,
            Some(DisabledBy::ClaudePluginDisabled)
        );
    }

    #[test]
    fn claude_plugin_enabled_by_harness_leaves_the_deployment_enabled() {
        let mut plugin = deployment_dto("my-skill", "Claude Code");
        plugin.plugin = Some(skill_studio_core::dto::PluginSourceDto {
            marketplace: "some-marketplace".to_string(),
            plugin: "some-plugin".to_string(),
            version: None,
            enabled: Some(true),
        });

        let skills =
            assemble_installed_skills(skills_from(vec![("my-skill", plugin)]), &empty_lock());
        let deployment = &skills[0].deployments[0];
        assert!(!deployment.disabled);
        assert_eq!(deployment.disabled_by, None);
    }

    #[test]
    fn assembled_skills_are_sorted_by_name() {
        let skills = assemble_installed_skills(
            skills_from(vec![
                ("zeta", deployment_dto("zeta", "Codex")),
                ("alpha", deployment_dto("alpha", "Codex")),
                ("mid", deployment_dto("mid", "pi")),
            ]),
            &empty_lock(),
        );
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mid", "zeta"]);
    }

    #[test]
    fn merge_of_one_skill_across_three_roots_yields_three_deployments() {
        let skills = assemble_installed_skills(
            skills_from(vec![
                ("my-skill", deployment_dto("my-skill", "Claude Code")),
                ("my-skill", deployment_dto("my-skill", "Codex")),
                ("my-skill", deployment_dto("my-skill", "pi")),
            ]),
            &empty_lock(),
        );
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].deployments.len(), 3);
    }

    #[test]
    fn violations_are_deduped() {
        let mut a = deployment_dto("my-skill", "Claude Code");
        a.spec_violations = vec!["missing required frontmatter field: description".to_string()];
        let mut b = deployment_dto("my-skill", "Codex");
        b.spec_violations = vec!["missing required frontmatter field: description".to_string()];

        let skills = assemble_installed_skills(
            skills_from(vec![("my-skill", a), ("my-skill", b)]),
            &empty_lock(),
        );
        assert_eq!(skills[0].spec_violations.len(), 1);
    }

    #[test]
    fn lock_entry_seeds_source_metadata() {
        let c = deployment_dto("write-tests", "Claude Code");
        let mut lock = empty_lock();
        lock.skills.insert(
            "write-tests".to_string(),
            InstalledSkillEntry {
                source: "obra/write-tests".to_string(),
                source_type: "github".to_string(),
                source_url: "https://github.com/obra/write-tests".to_string(),
                skill_path: None,
                skill_folder_hash: "abc".to_string(),
                installed_at: "2024-01-01T00:00:00Z".to_string(),
                updated_at: "2024-02-01T00:00:00Z".to_string(),
            },
        );

        let skills = assemble_installed_skills(skills_from(vec![("write-tests", c)]), &lock);
        let skill = &skills[0];
        assert_eq!(skill.source, "obra/write-tests");
        assert_eq!(
            skill.source_url.as_deref(),
            Some("https://github.com/obra/write-tests")
        );
        assert_eq!(skill.installed_at, "2024-01-01T00:00:00Z");
    }

    #[test]
    fn lock_only_skill_is_retained_with_empty_deployments() {
        let mut lock = empty_lock();
        lock.skills.insert(
            "gone-skill".to_string(),
            InstalledSkillEntry {
                source: "obra/gone-skill".to_string(),
                source_type: "github".to_string(),
                source_url: "https://github.com/obra/gone-skill".to_string(),
                skill_path: None,
                skill_folder_hash: "abc".to_string(),
                installed_at: "2024-01-01T00:00:00Z".to_string(),
                updated_at: "2024-01-01T00:00:00Z".to_string(),
            },
        );

        let skills = assemble_installed_skills(Vec::new(), &lock);
        assert_eq!(skills.len(), 1);
        assert!(skills[0].deployments.is_empty());
        assert_eq!(skills[0].source_kind, SourceKind::SkillsSh);
    }

    #[test]
    fn aggregate_facts_come_from_first_readable_candidate_not_first_by_order() {
        let mut broken = deployment_dto("my-skill", "Claude Code");
        broken.content_hash = String::new();
        broken.symlink_is_broken = true;

        let mut valid = deployment_dto("my-skill", "Codex");
        valid.content_hash = "hash-codex".to_string();
        valid.skill_md_tokens = 42;
        valid.description_tokens = 7;
        valid.folder_bytes = 100;
        valid.file_count = 3;

        let skills = assemble_installed_skills(
            skills_from(vec![("my-skill", broken), ("my-skill", valid)]),
            &empty_lock(),
        );
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].deployments.len(), 2);
        assert_eq!(skills[0].content_hash, "hash-codex");
        assert_eq!(skills[0].skill_md_tokens, 42);
        assert_eq!(skills[0].description_tokens, 7);
        assert_eq!(skills[0].folder_bytes, 100);
        assert_eq!(skills[0].file_count, 3);
    }

    #[test]
    fn violations_are_attributed_to_the_deployment_that_has_them() {
        let global = deployment_dto("motion", "Claude Code");
        let mut project =
            deployment_dto_project("motion", "Claude Code", Path::new("/tmp/project"));
        project.spec_violations = vec!["missing required frontmatter field: name".to_string()];

        let skills = assemble_installed_skills(
            skills_from(vec![("motion", global), ("motion", project)]),
            &empty_lock(),
        );
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].spec_violations.len(), 1);

        let by_scope: HashMap<&str, &Vec<String>> = skills[0]
            .deployments
            .iter()
            .map(|d| (d.scope.as_str(), &d.spec_violations))
            .collect();
        assert!(by_scope.get("global").unwrap().is_empty());
        assert_eq!(
            by_scope.get("project").unwrap().as_slice(),
            &["missing required frontmatter field: name".to_string()]
        );
    }

    #[test]
    fn invocation_follows_the_deployment_own_frontmatter() {
        let global = deployment_dto("motion", "Claude Code");
        let mut project =
            deployment_dto_project("motion", "Claude Code", Path::new("/tmp/project"));
        project.frontmatter = Some(skill_studio_core::frontmatter::SkillFrontmatter {
            disable_model_invocation: Some(true),
            ..Default::default()
        });

        let skills = assemble_installed_skills(
            skills_from(vec![("motion", global), ("motion", project)]),
            &empty_lock(),
        );
        assert_eq!(skills.len(), 1);

        let by_scope: HashMap<&str, crate::skills::frontmatter::InvocationPolicy> = skills[0]
            .deployments
            .iter()
            .map(|d| (d.scope.as_str(), d.invocation))
            .collect();
        assert_eq!(
            by_scope.get("global"),
            Some(&crate::skills::frontmatter::InvocationPolicy::Both)
        );
        assert_eq!(
            by_scope.get("project"),
            Some(&crate::skills::frontmatter::InvocationPolicy::UserOnly)
        );
    }

    #[test]
    fn each_deployment_carries_its_own_content_hash() {
        let mut a = deployment_dto("my-skill", "Claude Code");
        a.content_hash = "hash-a".to_string();
        let mut b = deployment_dto("my-skill", "Codex");
        b.content_hash = "hash-b".to_string();

        let skills = assemble_installed_skills(
            skills_from(vec![("my-skill", a), ("my-skill", b)]),
            &empty_lock(),
        );
        assert_eq!(skills[0].content_hashes.len(), 2);

        let by_agent: HashMap<&str, &str> = skills[0]
            .deployments
            .iter()
            .map(|d| (d.agent.as_str(), d.content_hash.as_str()))
            .collect();
        assert_eq!(by_agent.get("Claude Code"), Some(&"hash-a"));
        assert_eq!(by_agent.get("Codex"), Some(&"hash-b"));
    }
}
