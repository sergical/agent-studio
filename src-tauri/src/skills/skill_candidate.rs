// ============================================================================
// Skills Module - Skill Candidate
// The fact record produced by skill_discovery.rs while walking the
// filesystem. Every classification/merge rule (provenance.rs,
// skill_assembly.rs) consumes only these hand-buildable facts, so they're
// testable without touching disk.
// ============================================================================

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::frontmatter::SkillFrontmatter;
use super::skill_dto::PluginInfo;

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
    /// True when `is_symlink` but the target doesn't exist.
    pub symlink_is_broken: bool,
    /// Set when `is_symlink` and resolving the target failed for a reason
    /// other than "doesn't exist" (permission denied, symlink loop, etc.).
    pub symlink_error: Option<String>,
    pub plugin: Option<PluginInfo>,
    /// True when this candidate came from the shared `.agents/skills` root
    /// and that root's `agents.toml`/`agents.lock` (getsentry/dotagents'
    /// own lock file) is present.
    pub shared_root_has_lock_entry: bool,
    pub frontmatter: Option<SkillFrontmatter>,
    /// Every top-level frontmatter key, stringified.
    pub frontmatter_fields: BTreeMap<String, String>,
    pub spec_violations: Vec<String>,
    pub has_spec: bool,
    pub folder_bytes: u64,
    pub file_count: u32,
    pub skill_md_tokens: u32,
    /// sha256 over the sorted (relative path, bytes) pairs of the skill folder.
    pub content_hash: String,
    /// RFC3339 of the newest file mtime in the skill folder.
    pub modified_at: Option<String>,
    /// True when the folder walk hit the 2,000-file / 64 MiB cap and stopped
    /// early - `folder_bytes`/`file_count`/`content_hash` are partial.
    pub folder_truncated: bool,
}
