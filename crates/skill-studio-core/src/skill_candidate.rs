// ============================================================================
// Skills Module - Skill Candidate
// The fact record produced by skill_discovery.rs while walking the
// filesystem. Every classification/merge rule (provenance.rs,
// skill_assembly.rs) consumes only these hand-buildable facts, so they're
// testable without touching disk.
// ============================================================================

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::skill_document::SkillFrontmatter;
use crate::skill_plugins::PluginInfo;

/// Plugin ownership evidence is deliberately three-valued. A failed or
/// incomplete lookup must not fall through to manual ownership.
#[derive(Debug, Clone)]
pub enum PluginEvidence {
    Confirmed(PluginInfo),
    Absent,
    Unknown,
}

/// Git ancestry observed through the caller's retained read scope. Truncated
/// ancestry and failed reads are distinct from a confirmed absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitRepoEvidence {
    Present,
    Absent,
    /// No marker was found before the declared read boundary.
    Truncated,
    /// A metadata read failed inside the declared scope.
    Unknown,
}

/// A single skill directory found while walking an agent skill root or a
/// plugin cache. A skill dir is a directory (or symlink to one) containing
/// SKILL.md.
#[derive(Debug, Clone)]
pub struct SkillCandidate {
    pub name: String,
    pub path: PathBuf,
    pub root_label: String,
    /// The wire scope this candidate maps to: "global" | "project" | "plugin".
    pub scope: String,
    pub project_path: Option<PathBuf>,
    pub is_symlink: bool,
    /// Canonicalized symlink target, when `is_symlink` and the target resolves.
    pub symlink_target: Option<PathBuf>,
    /// Canonical path of this candidate's directory when it differs from
    /// `path` - set when any ancestor (other than `path` itself) is a
    /// symlink, e.g. a `.claude/skills` root linked to `.agents/skills`, so
    /// the frontend can tell "same folder through a linked root" from a
    /// separate copy. `None` for symlinked entries - `symlink_target`
    /// already carries that.
    pub resolved_path: Option<PathBuf>,
    /// True when `is_symlink` but the target doesn't exist.
    pub symlink_is_broken: bool,
    /// Set when `is_symlink` and resolving the target failed for a reason
    /// other than "doesn't exist" (permission denied, symlink loop, etc.).
    pub symlink_error: Option<String>,
    pub plugin: PluginEvidence,
    pub frontmatter: Option<SkillFrontmatter>,
    /// Every top-level frontmatter key, stringified.
    pub frontmatter_fields: BTreeMap<String, String>,
    pub spec_violations: Vec<String>,
    pub has_spec: bool,
    pub folder_bytes: u64,
    pub file_count: u32,
    pub skill_md_tokens: u32,
    /// Token count of just `"{name}: {description}"` - the prompt cost the
    /// model actually pays per turn, as opposed to `skill_md_tokens` which
    /// counts the whole file.
    pub description_tokens: u32,
    /// sha256 over the sorted (relative path, bytes) pairs of the skill folder.
    pub content_hash: String,
    /// RFC3339 of the newest file mtime in the skill folder.
    pub modified_at: Option<String>,
    /// True when the folder walk hit the 2,000-file / 64 MiB cap and stopped
    /// early - `folder_bytes`/`file_count`/`content_hash` are partial.
    pub folder_truncated: bool,
    pub git_repo: GitRepoEvidence,
    /// True when this candidate was found inside a root's
    /// `.skill-studio-disabled/` holding directory - see
    /// `skill_harness_disable`'s universal move-aside disable. Maps to
    /// `Deployment.disabled` + `DisabledBy::StudioMoved` in `skill_assembly`.
    pub studio_disabled: bool,
    /// True when this candidate's skills root (its entry's parent) is itself
    /// a symlink resolving into the shared `.agents/skills` folder - see
    /// `Deployment.shared_via_whole_dir_link`.
    pub shared_via_whole_dir_link: bool,
}
