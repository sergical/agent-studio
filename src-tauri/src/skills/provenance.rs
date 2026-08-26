// ============================================================================
// Skills Module - Source Provenance
// Classifies how a skill made it onto disk: "skills-sh" (present in the lock
// file), "plugin" (shipped by an agent plugin), "dotagents" (symlinked in by
// getsentry/dotagents), or "manual" (a plain directory with no other
// provenance signal). Ordered by precedence: dotagents > plugin > skills-sh
// > manual - the lowest-precedence signal wins when a skill is deployed to
// multiple agents with conflicting signals. Pure: works only from the facts
// a SkillCandidate already captured, never touches the filesystem itself.
// ============================================================================

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::lock_file::SkillLockFile;
use super::skill_candidate::SkillCandidate;

/// How a skill made it onto disk. Serializes to the same kebab-case strings
/// the frontend has always used ("skills-sh", "plugin", "dotagents",
/// "manual"), so this is a drop-in replacement for the old stringly-typed
/// source_kind field. Declaration order doubles as precedence order via the
/// derived `Ord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    Dotagents,
    Plugin,
    SkillsSh,
    Manual,
    /// Detached from its dotagents/skills.sh ledger via "Fork" so local
    /// edits survive `sync`/`update` - see `skill_fork_registry`.
    /// `classify_source_kind` never returns this; it's assigned afterward by
    /// `skill_refresh::build_snapshot` from the fork registry.
    Fork,
}

/// True when `path` resolves under a `.agents/skills/` directory, the
/// deployment location getsentry/dotagents symlinks skills into.
fn resolves_into_dotagents(path: &Path) -> bool {
    let comps: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    comps
        .windows(2)
        .any(|w| w[0] == ".agents" && w[1] == "skills")
}

/// Classify how a skill candidate got onto disk. See module docs for the
/// precedence order.
pub fn classify_source_kind(candidate: &SkillCandidate, lock: &SkillLockFile) -> SourceKind {
    if candidate.is_symlink {
        if let Some(target) = &candidate.symlink_target {
            if resolves_into_dotagents(target) {
                return SourceKind::Dotagents;
            }
        }
    }

    if candidate.root_label == "shared" && candidate.shared_root_has_lock_entry {
        return SourceKind::Dotagents;
    }

    if candidate.plugin.is_some() {
        return SourceKind::Plugin;
    }

    if lock.skills.contains_key(&candidate.name) {
        return SourceKind::SkillsSh;
    }

    SourceKind::Manual
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::path::PathBuf;

    use super::*;

    fn base_candidate(name: &str, root_label: &str) -> SkillCandidate {
        SkillCandidate {
            name: name.to_string(),
            path: PathBuf::from(format!("/tmp/{root_label}/{name}")),
            root_label: root_label.to_string(),
            scope: "global".to_string(),
            project_path: None,
            is_symlink: false,
            symlink_target: None,
            symlink_is_broken: false,
            symlink_error: None,
            plugin: None,
            shared_root_has_lock_entry: false,
            frontmatter: None,
            frontmatter_fields: BTreeMap::new(),
            spec_violations: Vec::new(),
            has_spec: false,
            folder_bytes: 0,
            file_count: 0,
            skill_md_tokens: 0,
            content_hash: String::new(),
            modified_at: None,
            folder_truncated: false,
        }
    }

    fn empty_lock() -> SkillLockFile {
        SkillLockFile {
            version: 3,
            skills: HashMap::new(),
        }
    }

    #[test]
    fn dotagents_symlink_is_detected() {
        let mut candidate = base_candidate("find-bugs", "Claude Code");
        candidate.is_symlink = true;
        candidate.symlink_target = Some(PathBuf::from("/home/u/.agents/skills/find-bugs"));

        let kind = classify_source_kind(&candidate, &empty_lock());
        assert_eq!(kind, SourceKind::Dotagents);
    }

    #[test]
    fn plugin_candidate_is_detected() {
        let mut candidate = base_candidate("lint-code", "Claude Code");
        candidate.plugin = Some(super::super::skill_dto::PluginInfo {
            name: "sentry-toolkit".to_string(),
            version: Some("1.0.0".to_string()),
            harness: "Claude Code".to_string(),
        });

        let kind = classify_source_kind(&candidate, &empty_lock());
        assert_eq!(kind, SourceKind::Plugin);
    }

    #[test]
    fn lock_file_name_is_skills_sh() {
        let candidate = base_candidate("write-tests", "Claude Code");
        let mut lock = empty_lock();
        lock.skills.insert(
            "write-tests".to_string(),
            crate::skills::lock_file::InstalledSkillEntry {
                source: "obra/write-tests".to_string(),
                source_type: "github".to_string(),
                source_url: "https://github.com/obra/write-tests".to_string(),
                skill_path: None,
                skill_folder_hash: "abc".to_string(),
                installed_at: "2024-01-01T00:00:00Z".to_string(),
                updated_at: "2024-01-01T00:00:00Z".to_string(),
            },
        );

        let kind = classify_source_kind(&candidate, &lock);
        assert_eq!(kind, SourceKind::SkillsSh);
    }

    #[test]
    fn plain_directory_is_manual() {
        let candidate = base_candidate("my-notes", "Claude Code");
        let kind = classify_source_kind(&candidate, &empty_lock());
        assert_eq!(kind, SourceKind::Manual);
    }

    #[test]
    fn shared_root_with_agents_lock_is_dotagents() {
        let mut candidate = base_candidate("some-skill", "shared");
        candidate.shared_root_has_lock_entry = true;

        let kind = classify_source_kind(&candidate, &empty_lock());
        assert_eq!(kind, SourceKind::Dotagents);
    }

    #[test]
    fn shared_root_without_agents_lock_falls_back_to_manual() {
        let candidate = base_candidate("some-skill", "shared");
        let kind = classify_source_kind(&candidate, &empty_lock());
        assert_eq!(kind, SourceKind::Manual);
    }
}
