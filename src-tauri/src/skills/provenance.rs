// ============================================================================
// Skills Module - Source Provenance
// Classifies how a skill made it onto disk: "skills-sh" (present in the lock
// file), "plugin" (shipped by an agent plugin), "dotagents" (symlinked in by
// getsentry/dotagents), or "manual" (a plain directory with no other
// provenance signal). Ordered by precedence: dotagents > plugin > skills-sh
// > manual - the lowest-precedence signal wins when a skill is deployed to
// multiple agents with conflicting signals.
// ============================================================================

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::plugins;

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

/// True when any path component is `plugins`, matching how agent plugin
/// systems (e.g. Claude Code's `~/.claude/plugins/<plugin>/skills/...`) lay
/// out skills they ship. Kept only as a fallback for plugin layouts we
/// don't recognize a manifest for - `plugins::find_plugin_root` is the
/// authoritative signal.
fn under_plugins_dir(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "plugins")
}

/// Classify how `entry_path` (a direct child of an agent's skills root) got
/// onto disk. See module docs for the precedence order.
pub fn classify_source_kind(
    entry_path: &Path,
    skill_name: &str,
    lock_names: &HashSet<String>,
    agent_label: &str,
) -> SourceKind {
    if let Ok(meta) = fs::symlink_metadata(entry_path) {
        if meta.file_type().is_symlink() {
            let target = fs::canonicalize(entry_path).unwrap_or_else(|_| {
                fs::read_link(entry_path).unwrap_or_else(|_| entry_path.to_path_buf())
            });
            if resolves_into_dotagents(&target) {
                return SourceKind::Dotagents;
            }
        }
    }

    if plugins::find_plugin_root(entry_path, agent_label).is_some() {
        return SourceKind::Plugin;
    }

    let resolved = fs::canonicalize(entry_path).unwrap_or_else(|_| entry_path.to_path_buf());
    if under_plugins_dir(&resolved) || under_plugins_dir(entry_path) {
        return SourceKind::Plugin;
    }

    if lock_names.contains(skill_name) {
        return SourceKind::SkillsSh;
    }

    SourceKind::Manual
}

/// Classify a skill living directly under a shared `.agents/skills` root
/// (used by Codex, pi, and OpenCode). `agents_root` is the `.agents`
/// directory itself, not the `skills` subdirectory.
pub fn classify_shared_root_source_kind(
    agents_root: &Path,
    skill_name: &str,
    lock_names: &HashSet<String>,
) -> SourceKind {
    if agents_root.join("agents.toml").exists() || agents_root.join("agents.lock").exists() {
        return SourceKind::Dotagents;
    }
    if lock_names.contains(skill_name) {
        return SourceKind::SkillsSh;
    }
    SourceKind::Manual
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotagents_symlink_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let dotagents_skill = tmp.path().join(".agents/skills/find-bugs");
        fs::create_dir_all(&dotagents_skill).unwrap();
        fs::write(
            dotagents_skill.join("SKILL.md"),
            "---\nname: find-bugs\n---\n",
        )
        .unwrap();

        let claude_skills = tmp.path().join(".claude/skills");
        fs::create_dir_all(&claude_skills).unwrap();
        let link = claude_skills.join("find-bugs");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dotagents_skill, &link).unwrap();

        let kind = classify_source_kind(&link, "find-bugs", &HashSet::new(), "Claude Code");
        assert_eq!(kind, SourceKind::Dotagents);
    }

    #[test]
    fn plugin_path_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_skill = tmp
            .path()
            .join(".claude/plugins/some-plugin/skills/lint-code");
        fs::create_dir_all(&plugin_skill).unwrap();
        fs::write(plugin_skill.join("SKILL.md"), "---\nname: lint-code\n---\n").unwrap();

        let kind = classify_source_kind(&plugin_skill, "lint-code", &HashSet::new(), "Claude Code");
        assert_eq!(kind, SourceKind::Plugin);
    }

    #[test]
    fn lock_file_name_is_skills_sh() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join(".claude/skills/write-tests");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: write-tests\n---\n").unwrap();

        let mut lock_names = HashSet::new();
        lock_names.insert("write-tests".to_string());

        let kind = classify_source_kind(&skill_dir, "write-tests", &lock_names, "Claude Code");
        assert_eq!(kind, SourceKind::SkillsSh);
    }

    #[test]
    fn plain_directory_is_manual() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join(".claude/skills/my-notes");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: my-notes\n---\n").unwrap();

        let kind = classify_source_kind(&skill_dir, "my-notes", &HashSet::new(), "Claude Code");
        assert_eq!(kind, SourceKind::Manual);
    }

    #[test]
    fn shared_root_with_agents_lock_is_dotagents() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_root = tmp.path().join(".agents");
        fs::create_dir_all(agents_root.join("skills")).unwrap();
        fs::write(agents_root.join("agents.lock"), "").unwrap();

        let kind = classify_shared_root_source_kind(&agents_root, "some-skill", &HashSet::new());
        assert_eq!(kind, SourceKind::Dotagents);
    }

    #[test]
    fn shared_root_without_agents_lock_falls_back_to_manual() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_root = tmp.path().join(".agents");
        fs::create_dir_all(agents_root.join("skills")).unwrap();

        let kind = classify_shared_root_source_kind(&agents_root, "some-skill", &HashSet::new());
        assert_eq!(kind, SourceKind::Manual);
    }
}
