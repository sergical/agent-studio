// ============================================================================
// Skills Module - Directory Scanner
// Discovers installed skills by scanning the skill directories of the four
// first-class agents (Claude Code, Codex, OpenCode, pi), the shared
// `.agents/skills` root, and native plugin caches, then classifies how each
// one got there (skills.sh, a plugin, dotagents, or a manual drop-in).
// ============================================================================

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::agents::AgentId;
use super::frontmatter::{parse_frontmatter, validate_skill};
use super::lock_file;
use super::lock_file::SkillLockFile;
use super::plugins;
use super::provenance::{classify_shared_root_source_kind, classify_source_kind, SourceKind};
use super::skill_dto::{Deployment, InstalledSkill};

/// The four first-class agents whose skill directories are scanned for
/// native provenance detection.
const FIRST_CLASS_AGENTS: &[AgentId] = &[
    AgentId::ClaudeCode,
    AgentId::Codex,
    AgentId::OpenCode,
    AgentId::Pi,
];

/// A skill directory root to scan, tied to the agent/scope it belongs to.
/// Beyond each first-class agent's own directory (from `AgentId`'s path
/// methods, the single source of truth), this also covers the two roots
/// that don't belong to a single agent: OpenCode's older singular `skill/`
/// directory (kept as a fallback alongside `skills/`), and the shared
/// `.agents/skills` root that Codex, pi, and OpenCode all read.
enum SkillRoot {
    Agent(AgentId),
    OpenCodeSkillFallback,
    Shared,
}

impl SkillRoot {
    /// All roots to scan, in a fixed order.
    fn all() -> Vec<SkillRoot> {
        let mut roots: Vec<SkillRoot> = FIRST_CLASS_AGENTS
            .iter()
            .map(|&id| SkillRoot::Agent(id))
            .collect();
        roots.push(SkillRoot::OpenCodeSkillFallback);
        roots.push(SkillRoot::Shared);
        roots
    }

    fn agent_label(&self) -> &'static str {
        match self {
            SkillRoot::Agent(id) => id.display_name(),
            SkillRoot::OpenCodeSkillFallback => AgentId::OpenCode.display_name(),
            SkillRoot::Shared => "shared",
        }
    }

    /// Shared `.agents/skills` roots are classified by looking for
    /// agents.toml/agents.lock next to them, not by the per-agent
    /// symlink/plugin rules used for the other roots.
    fn is_shared(&self) -> bool {
        matches!(self, SkillRoot::Shared)
    }

    fn global_dir(&self, home: &Path) -> PathBuf {
        match self {
            SkillRoot::Agent(id) => id.global_skills_dir(home),
            SkillRoot::OpenCodeSkillFallback => home.join(".config/opencode/skill"),
            SkillRoot::Shared => home.join(".agents/skills"),
        }
    }

    fn project_dir(&self, project: &Path) -> PathBuf {
        match self {
            SkillRoot::Agent(id) => id.project_skills_dir(project),
            SkillRoot::OpenCodeSkillFallback => project.join(".opencode/skill"),
            SkillRoot::Shared => project.join(".agents/skills"),
        }
    }
}

/// A resolved skill directory root, ready to scan.
struct AgentSkillRoot {
    agent_label: &'static str,
    scope: &'static str,
    path: PathBuf,
    is_shared: bool,
}

/// Skill directory roots to scan: the four first-class agents' global and
/// per-project paths, plus the shared `.agents/skills` root that Codex, pi,
/// and OpenCode also read.
fn candidate_roots(home: &Path, project_paths: &[String]) -> Vec<AgentSkillRoot> {
    let mut roots = Vec::new();

    for root in SkillRoot::all() {
        roots.push(AgentSkillRoot {
            agent_label: root.agent_label(),
            scope: "global",
            path: root.global_dir(home),
            is_shared: root.is_shared(),
        });
    }

    for p in project_paths {
        let base = PathBuf::from(p);
        for root in SkillRoot::all() {
            roots.push(AgentSkillRoot {
                agent_label: root.agent_label(),
                scope: "project",
                path: root.project_dir(&base),
                is_shared: root.is_shared(),
            });
        }
    }

    roots
}

/// A skill "ships specs" (the getsentry/skillet pattern) when it has a
/// spec.md file or an evals/ subdirectory alongside SKILL.md.
fn has_spec(skill_dir: &Path) -> bool {
    skill_dir.join("spec.md").exists() || skill_dir.join("evals").is_dir()
}

/// Build a fresh InstalledSkill, seeding metadata from the lock file entry
/// when one exists for this skill name, or generic "local directory"
/// metadata otherwise.
fn new_installed_skill(
    name: &str,
    lock: &SkillLockFile,
    source_kind: SourceKind,
) -> InstalledSkill {
    if let Some(entry) = lock.skills.get(name) {
        InstalledSkill {
            name: name.to_string(),
            source: entry.source.clone(),
            source_type: entry.source_type.clone(),
            source_url: Some(entry.source_url.clone()),
            skill_path: entry.skill_path.clone(),
            installed_at: entry.installed_at.clone(),
            updated_at: Some(entry.updated_at.clone()),
            has_update: false,
            source_kind,
            deployments: Vec::new(),
            has_spec: false,
            description: None,
            spec_violations: Vec::new(),
        }
    } else {
        InstalledSkill {
            name: name.to_string(),
            source: "local".to_string(),
            source_type: "directory".to_string(),
            source_url: None,
            skill_path: None,
            installed_at: String::new(),
            updated_at: None,
            has_update: false,
            source_kind,
            deployments: Vec::new(),
            has_spec: false,
            description: None,
            spec_violations: Vec::new(),
        }
    }
}

/// Parsed data pulled from a single skill directory, shared by the
/// agent-root scan and the plugin-cache scan below.
struct ScannedSkill {
    name: String,
    description: Option<String>,
    violations: Vec<String>,
    has_spec: bool,
}

/// Read and validate a skill directory's SKILL.md. Returns `None` if the
/// directory has no readable SKILL.md.
fn scan_skill_dir(skill_dir: &Path) -> Option<ScannedSkill> {
    let content = fs::read_to_string(skill_dir.join("SKILL.md")).ok()?;
    let dir_name = skill_dir.file_name()?.to_string_lossy().to_string();
    let frontmatter = parse_frontmatter(&content);
    let name = frontmatter
        .as_ref()
        .and_then(|f| f.name.clone())
        .unwrap_or_else(|| dir_name.clone());
    let violations = validate_skill(&dir_name, frontmatter.as_ref(), content.lines().count());
    let description = frontmatter.and_then(|f| f.description);
    Some(ScannedSkill {
        name,
        description,
        violations,
        has_spec: has_spec(skill_dir),
    })
}

/// Merge a scanned deployment into `by_name`, updating the merged skill's
/// description, spec compliance, and provenance precedence.
fn merge_deployment(
    by_name: &mut HashMap<String, InstalledSkill>,
    lock: &SkillLockFile,
    scanned: ScannedSkill,
    source_kind: SourceKind,
    deployment: Deployment,
) {
    let record = by_name
        .entry(scanned.name.clone())
        .or_insert_with(|| new_installed_skill(&scanned.name, lock, source_kind));
    record.deployments.push(deployment);
    record.has_spec = record.has_spec || scanned.has_spec;
    if record.description.is_none() {
        record.description = scanned.description;
    }
    for violation in scanned.violations {
        if !record.spec_violations.contains(&violation) {
            record.spec_violations.push(violation);
        }
    }
    if source_kind < record.source_kind {
        record.source_kind = source_kind;
    }
}

/// Discover installed skills by scanning the first-class agents' skill
/// directories, the shared `.agents/skills` root, and native plugin caches,
/// merging in the ~/.agents/.skill-lock.json entries. One result per skill
/// name; a skill deployed to several agents gets several entries in
/// `deployments`. Lock-file-only skills that aren't found on disk are kept
/// with empty deployments and source_kind "skills-sh".
pub fn scan_installed_skills(
    project_paths: Option<Vec<String>>,
) -> Result<Vec<InstalledSkill>, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let project_paths = project_paths.unwrap_or_default();
    let lock = lock_file::read_lock_file()?;
    let lock_names: HashSet<String> = lock.skills.keys().cloned().collect();

    let mut by_name: HashMap<String, InstalledSkill> = HashMap::new();

    for root in candidate_roots(&home, &project_paths) {
        let Ok(entries) = fs::read_dir(&root.path) else {
            continue;
        };
        for entry in entries.flatten() {
            let child_path = entry.path();
            let is_dir = fs::metadata(&child_path)
                .map(|m| m.is_dir())
                .unwrap_or(false);
            if !is_dir {
                continue;
            }

            let Some(scanned) = scan_skill_dir(&child_path) else {
                continue;
            };

            let is_symlink = fs::symlink_metadata(&child_path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);

            let source_kind = if root.is_shared {
                let agents_root = root.path.parent().unwrap_or(&root.path);
                classify_shared_root_source_kind(agents_root, &scanned.name, &lock_names)
            } else {
                classify_source_kind(&child_path, &scanned.name, &lock_names, root.agent_label)
            };

            let deployment = Deployment {
                agent: root.agent_label.to_string(),
                scope: root.scope.to_string(),
                path: child_path.to_string_lossy().to_string(),
                is_symlink,
                plugin: None,
            };

            merge_deployment(&mut by_name, &lock, scanned, source_kind, deployment);
        }
    }

    // Native plugin enumeration: skills bundled inside a Claude Code or
    // Codex plugin, found by walking their plugin caches for manifests.
    for plugin_skill in plugins::scan_plugin_skills(&home) {
        let Some(scanned) = scan_skill_dir(&plugin_skill.skill_dir) else {
            continue;
        };
        let is_symlink = fs::symlink_metadata(&plugin_skill.skill_dir)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);

        let deployment = Deployment {
            agent: plugin_skill.plugin.harness.clone(),
            scope: "plugin".to_string(),
            path: plugin_skill.skill_dir.to_string_lossy().to_string(),
            is_symlink,
            plugin: Some(plugin_skill.plugin),
        };

        merge_deployment(&mut by_name, &lock, scanned, SourceKind::Plugin, deployment);
    }

    // Keep lock-file entries that weren't found on disk (e.g. deployed to an
    // agent we don't scan, or removed manually without updating the lock).
    for name in lock.skills.keys() {
        by_name
            .entry(name.clone())
            .or_insert_with(|| new_installed_skill(name, &lock, SourceKind::SkillsSh));
    }

    Ok(by_name.into_values().collect())
}
