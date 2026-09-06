// ============================================================================
// Skills Module - skill_deployment
// Stable identity for one on-disk skill deployment. Discovery still walks
// the filesystem; this module only names what it found: scope, destination
// (Universal vs Per harness), lexical path, owner, and backing relationship.
// Compatibility: scanner roots still use the label "shared"; that identifier
// is not a user-facing destination. Universal is the domain term.
// ============================================================================

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::skill_dto::InstallScope;

/// Where a skill is installed relative to harness folders. Universal owns
/// `.agents/skills`; Per harness owns an independent copy in one harness dir
/// and never writes `.agents/skills`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SkillDestination {
    Universal,
    #[default]
    PerHarness,
}

impl SkillDestination {
    /// Wire string used in deployment / owner ids.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Universal => "universal",
            Self::PerHarness => "per-harness",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "universal" => Some(Self::Universal),
            "per-harness" => Some(Self::PerHarness),
            _ => None,
        }
    }
}

/// How this deployment relates to a Universal folder of the same skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum BackingRelationship {
    /// This directory is the Universal `.agents/skills/<name>` folder.
    Canonical,
    /// A per-skill or whole-dir link into a Universal folder.
    LinkedTo { deployment_id: String },
    /// An independent harness copy that does not back or follow Universal.
    #[default]
    Independent,
}

/// Whether Skill Studio may mutate this deployment through an owner adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DeploymentMutability {
    Mutable,
    #[default]
    ReadOnly,
}

/// Compatibility root label the scanner still emits for `.agents/skills`.
pub const UNIVERSAL_ROOT_LABEL: &str = "shared";

/// True when a scanner / DTO agent label is the Universal root, including
/// the compatibility identifier `shared`.
pub fn is_universal_root_label(label: &str) -> bool {
    label == UNIVERSAL_ROOT_LABEL || label == "universal"
}

/// The Universal skills directory for `scope` under `home` or `project`.
pub fn universal_skills_dir(
    home: &Path,
    scope: InstallScope,
    project_path: Option<&Path>,
) -> PathBuf {
    match scope {
        InstallScope::Global => home.join(".agents").join("skills"),
        InstallScope::Project => project_path
            .unwrap_or_else(|| Path::new(""))
            .join(".agents")
            .join("skills"),
    }
}

/// The `.agents` directory that owns ledgers for a Universal skills root.
pub fn agents_root_for_skills_dir(skills_dir: &Path) -> Option<PathBuf> {
    let parent = skills_dir.parent()?;
    if parent.file_name().and_then(|n| n.to_str()) == Some(".agents") {
        Some(parent.to_path_buf())
    } else {
        None
    }
}

/// True when `path` is lexically under a `.agents/skills` directory.
pub fn path_is_under_universal_skills(path: &Path) -> bool {
    let comps: Vec<_> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    comps.windows(2).any(|w| w == [".agents", "skills"])
}

/// Percent-encode `/` and `%` so a project path can sit in a single id slot.
pub fn encode_id_path(path: &str) -> String {
    path.replace('%', "%25").replace('/', "%2F")
}

fn decode_id_path(encoded: &str) -> String {
    encoded.replace("%2F", "/").replace("%25", "%")
}

/// Stable id for one deployment. Format:
/// `dep:v1/{scope}/{slot}/{destination}/{name}/{project}/{lexical-entry}`
/// where `slot` is `universal` or a harness cli name, and `project` is `-`
/// for global / plugin / parked.
pub fn deployment_id(
    name: &str,
    scope: &str,
    destination: SkillDestination,
    slot: &str,
    project_path: Option<&str>,
    lexical_entry: &Path,
) -> String {
    let project = match project_path {
        Some(path) if !path.is_empty() => encode_id_path(path),
        _ => "-".to_string(),
    };
    format!(
        "dep:v1/{scope}/{slot}/{}/{name}/{project}/{}",
        destination.as_str(),
        encode_id_path(&lexical_entry.to_string_lossy())
    )
}

/// Parse a `dep:v1/...` id back into its slots. `None` when the string is
/// not a v1 deployment id.
pub fn parse_deployment_id(id: &str) -> Option<ParsedDeploymentId> {
    let rest = id.strip_prefix("dep:v1/")?;
    let mut parts = rest.splitn(6, '/');
    let scope = parts.next()?.to_string();
    let slot = parts.next()?.to_string();
    let destination = SkillDestination::parse(parts.next()?)?;
    let name = parts.next()?.to_string();
    let project = parts.next()?;
    let lexical_path = PathBuf::from(decode_id_path(parts.next()?));
    let project_path = if project == "-" {
        None
    } else {
        Some(decode_id_path(project))
    };
    Some(ParsedDeploymentId {
        scope,
        slot,
        destination,
        name,
        project_path,
        lexical_path,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDeploymentId {
    pub scope: String,
    pub slot: String,
    pub destination: SkillDestination,
    pub name: String,
    pub project_path: Option<String>,
    pub lexical_path: PathBuf,
}

/// Identity-relevant facts from one scanned skill candidate.
pub struct DeploymentCandidate<'a> {
    pub name: &'a str,
    pub root_label: &'a str,
    pub scope: &'a str,
    pub path: &'a Path,
    pub project_path: Option<&'a str>,
    pub is_symlink: bool,
    pub symlink_target: Option<&'a Path>,
    pub resolved_path: Option<&'a Path>,
    pub shared_via_whole_dir_link: bool,
}

/// Build the deployment id for a scanned candidate.
pub fn id_for_candidate(
    candidate: DeploymentCandidate<'_>,
) -> (String, SkillDestination, BackingRelationship) {
    let DeploymentCandidate {
        name,
        root_label,
        scope,
        path,
        project_path,
        is_symlink,
        symlink_target,
        resolved_path,
        shared_via_whole_dir_link,
    } = candidate;
    if is_universal_root_label(root_label) || scope == "parked" {
        let dest = SkillDestination::Universal;
        let id = deployment_id(name, scope, dest, "universal", project_path, path);
        return (id, dest, BackingRelationship::Canonical);
    }

    let linked = shared_via_whole_dir_link
        || (is_symlink && symlink_target.is_some_and(path_is_under_universal_skills));

    if linked {
        let dest = SkillDestination::Universal;
        let slot = harness_slot(root_label);
        let id = deployment_id(name, scope, dest, slot, project_path, path);
        let canonical_path = resolved_path.or(symlink_target).unwrap_or(path);
        let canonical = deployment_id(name, scope, dest, "universal", project_path, canonical_path);
        return (
            id,
            dest,
            BackingRelationship::LinkedTo {
                deployment_id: canonical,
            },
        );
    }

    let dest = SkillDestination::PerHarness;
    let slot = harness_slot(root_label);
    let id = deployment_id(name, scope, dest, slot, project_path, path);
    (id, dest, BackingRelationship::Independent)
}

fn harness_slot(root_label: &str) -> &'static str {
    match root_label {
        "Claude Code" => "claude-code",
        "Codex" => "codex",
        "OpenCode" => "opencode",
        "pi" => "pi",
        "Cursor" => "cursor",
        "Grok Build" => "grok-build",
        "shared" | "universal" => "universal",
        "parked" => "universal",
        _ => "other",
    }
}

/// Harnesses the Per-harness destination may copy into.
pub const PER_HARNESS_INSTALL_TARGETS: &[&str] = &[
    "claude-code",
    "codex",
    "opencode",
    "pi",
    "cursor",
    "grok-build",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universal_id_round_trips_global() {
        let id = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            Path::new("/home/.agents/skills/find-bugs"),
        );
        assert_eq!(
            id,
            "dep:v1/global/universal/universal/find-bugs/-/%2Fhome%2F.agents%2Fskills%2Ffind-bugs"
        );
        let parsed = parse_deployment_id(&id).unwrap();
        assert_eq!(parsed.name, "find-bugs");
        assert_eq!(parsed.destination, SkillDestination::Universal);
        assert_eq!(parsed.slot, "universal");
        assert!(parsed.project_path.is_none());
    }

    #[test]
    fn per_harness_id_includes_project_path() {
        let id = deployment_id(
            "find-bugs",
            "project",
            SkillDestination::PerHarness,
            "codex",
            Some("/Users/me/app"),
            Path::new("/Users/me/app/.codex/skills/find-bugs"),
        );
        let parsed = parse_deployment_id(&id).unwrap();
        assert_eq!(parsed.project_path.as_deref(), Some("/Users/me/app"));
        assert_eq!(parsed.slot, "codex");
        assert_eq!(parsed.destination, SkillDestination::PerHarness);
    }

    #[test]
    fn shared_root_candidate_is_canonical_universal() {
        let (id, dest, backing) = id_for_candidate(DeploymentCandidate {
            name: "find-bugs",
            root_label: "shared",
            scope: "global",
            path: Path::new("/home/.agents/skills/find-bugs"),
            project_path: None,
            is_symlink: false,
            symlink_target: None,
            resolved_path: None,
            shared_via_whole_dir_link: false,
        });
        assert_eq!(dest, SkillDestination::Universal);
        assert_eq!(backing, BackingRelationship::Canonical);
        assert!(id.contains("/universal/universal/"));
    }

    #[test]
    fn claude_symlink_into_universal_is_linked_not_per_harness() {
        let (id, dest, backing) = id_for_candidate(DeploymentCandidate {
            name: "find-bugs",
            root_label: "Claude Code",
            scope: "global",
            path: Path::new("/home/.claude/skills/find-bugs"),
            project_path: None,
            is_symlink: true,
            symlink_target: Some(Path::new("/home/.agents/skills/find-bugs")),
            resolved_path: Some(Path::new("/home/.agents/skills/find-bugs")),
            shared_via_whole_dir_link: false,
        });
        assert_eq!(dest, SkillDestination::Universal);
        match backing {
            BackingRelationship::LinkedTo { deployment_id } => {
                assert!(deployment_id.contains("/universal/universal/"));
            }
            other => panic!("expected LinkedTo, got {other:?}"),
        }
        assert!(id.contains("/claude-code/"));
    }

    #[test]
    fn whole_directory_links_use_the_resolved_universal_child_as_backing() {
        for (root_label, harness_path) in [
            ("Claude Code", "/home/.claude/skills/find-bugs"),
            ("OpenCode", "/home/.config/opencode/skills/find-bugs"),
        ] {
            let (_, destination, backing) = id_for_candidate(DeploymentCandidate {
                name: "find-bugs",
                root_label,
                scope: "global",
                path: Path::new(harness_path),
                project_path: None,
                is_symlink: false,
                symlink_target: None,
                resolved_path: Some(Path::new("/home/.agents/skills/find-bugs")),
                shared_via_whole_dir_link: true,
            });
            let expected = deployment_id(
                "find-bugs",
                "global",
                SkillDestination::Universal,
                "universal",
                None,
                Path::new("/home/.agents/skills/find-bugs"),
            );

            assert_eq!(destination, SkillDestination::Universal, "{root_label}");
            assert_eq!(
                backing,
                BackingRelationship::LinkedTo {
                    deployment_id: expected
                },
                "{root_label}"
            );
        }
    }

    #[test]
    fn independent_codex_copy_is_per_harness() {
        let (_id, dest, backing) = id_for_candidate(DeploymentCandidate {
            name: "find-bugs",
            root_label: "Codex",
            scope: "global",
            path: Path::new("/home/.codex/skills/find-bugs"),
            project_path: None,
            is_symlink: false,
            symlink_target: None,
            resolved_path: None,
            shared_via_whole_dir_link: false,
        });
        assert_eq!(dest, SkillDestination::PerHarness);
        assert_eq!(backing, BackingRelationship::Independent);
    }

    #[test]
    fn same_name_global_and_project_ids_differ() {
        let global = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            Path::new("/home/.agents/skills/find-bugs"),
        );
        let project = deployment_id(
            "find-bugs",
            "project",
            SkillDestination::Universal,
            "universal",
            Some("/work/app"),
            Path::new("/work/app/.agents/skills/find-bugs"),
        );
        assert_ne!(global, project);
    }

    #[test]
    fn is_universal_root_label_accepts_shared_compatibility_id() {
        assert!(is_universal_root_label("shared"));
        assert!(is_universal_root_label("universal"));
        assert!(!is_universal_root_label("Codex"));
    }
}
