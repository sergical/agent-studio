// ============================================================================
// Skills Module - Skill Assembly
// Pure merge of SkillCandidate facts into InstalledSkill records, one per
// skill name across all its deployments. Never touches the filesystem -
// every rule here is testable with hand-built candidates.
// ============================================================================

use std::collections::HashMap;

use super::lock_file::SkillLockFile;
use super::provenance::{classify_source_kind, SourceKind};
use super::skill_candidate::SkillCandidate;
use super::skill_dto::{Deployment, DisabledBy, InstalledSkill};

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
        parked: false,
        parked_at: None,
        invocation: crate::skills::frontmatter::InvocationPolicy::Both,
    }
}

/// Merge candidates by name into one InstalledSkill per skill, with one
/// Deployment per candidate. Picks the source kind by precedence
/// dotagents > plugin > skills-sh > manual, dedupes violations, seeds from
/// the lock entry, and keeps lock-only skills (in the lock file, not found
/// on disk) with empty deployments and source_kind skills-sh.
pub fn assemble_installed_skills(
    candidates: Vec<SkillCandidate>,
    lock: &SkillLockFile,
) -> Vec<InstalledSkill> {
    let mut by_name: HashMap<String, InstalledSkill> = HashMap::new();

    for candidate in candidates {
        let source_kind = classify_source_kind(&candidate, lock);
        let record = by_name
            .entry(candidate.name.clone())
            .or_insert_with(|| new_installed_skill(&candidate.name, lock, source_kind));

        if source_kind < record.source_kind {
            record.source_kind = source_kind;
        }
        for violation in &candidate.spec_violations {
            if !record.spec_violations.contains(violation) {
                record.spec_violations.push(violation.clone());
            }
        }
        if !candidate.content_hash.is_empty()
            && !record.content_hashes.contains(&candidate.content_hash)
        {
            record.content_hashes.push(candidate.content_hash.clone());
        }

        // Aggregate facts come from the first deployment whose content was
        // actually readable (non-empty hash), not the first by order: a
        // broken symlink deployment seen before a valid one must not blank
        // out the skill's real hash/tokens/description/etc.
        if !candidate.content_hash.is_empty() && record.content_hash.is_empty() {
            record.skill_md_tokens = candidate.skill_md_tokens;
            record.description_tokens = candidate.description_tokens;
            record.folder_bytes = candidate.folder_bytes;
            record.file_count = candidate.file_count;
            record.content_hash = candidate.content_hash.clone();
            record.modified_at = candidate.modified_at.clone();
            record.frontmatter_fields = candidate.frontmatter_fields.clone();
            record.folder_truncated = candidate.folder_truncated;
            record.has_spec = candidate.has_spec;
            record.description = candidate
                .frontmatter
                .as_ref()
                .and_then(|f| f.description.clone());
        }

        record.deployments.push(Deployment {
            agent: candidate.root_label.clone(),
            scope: candidate.scope.clone(),
            path: candidate.path.to_string_lossy().to_string(),
            is_symlink: candidate.is_symlink,
            plugin: candidate.plugin.clone(),
            symlink_target: candidate
                .symlink_target
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            resolved_path: candidate
                .resolved_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            symlink_is_broken: candidate.symlink_is_broken,
            symlink_error: candidate.symlink_error.clone(),
            project_path: candidate
                .project_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            content_hash: candidate.content_hash.clone(),
            disabled: candidate.studio_disabled,
            disabled_by: candidate.studio_disabled.then_some(DisabledBy::StudioMoved),
            disabled_readers: Vec::new(),
            codex_implicit_invocation: None,
            shared_via_whole_dir_link: candidate.shared_via_whole_dir_link,
        });
    }

    // Keep lock-file entries that weren't found on disk (e.g. deployed to an
    // agent we don't scan, or removed manually without updating the lock).
    for name in lock.skills.keys() {
        by_name
            .entry(name.clone())
            .or_insert_with(|| new_installed_skill(name, lock, SourceKind::SkillsSh));
    }

    let mut skills: Vec<InstalledSkill> = by_name.into_values().collect();
    // HashMap order is random per process; a stable name order keeps every
    // list (Home updates, Skills) from reshuffling between rescans.
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::path::PathBuf;

    use super::*;
    use crate::skills::lock_file::InstalledSkillEntry;

    fn candidate(name: &str, root_label: &str) -> SkillCandidate {
        SkillCandidate {
            name: name.to_string(),
            path: PathBuf::from(format!("/tmp/{root_label}/{name}")),
            root_label: root_label.to_string(),
            scope: "global".to_string(),
            project_path: None,
            is_symlink: false,
            symlink_target: None,
            resolved_path: None,
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
            description_tokens: 0,
            content_hash: String::new(),
            modified_at: None,
            folder_truncated: false,
            in_git_repo: false,
            studio_disabled: false,
            shared_via_whole_dir_link: false,
        }
    }

    fn empty_lock() -> SkillLockFile {
        SkillLockFile {
            version: 3,
            skills: HashMap::new(),
        }
    }

    #[test]
    fn precedence_dotagents_beats_plugin_beats_skills_sh_beats_manual() {
        let mut dotagents = candidate("my-skill", "shared");
        dotagents.shared_root_has_lock_entry = true;

        let mut plugin = candidate("my-skill", "Claude Code");
        plugin.plugin = Some(crate::skills::skill_dto::PluginInfo {
            name: "some-plugin".to_string(),
            version: None,
            harness: "Claude Code".to_string(),
        });

        let manual = candidate("my-skill", "Codex");

        let skills = assemble_installed_skills(vec![manual, plugin, dotagents], &empty_lock());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].source_kind, SourceKind::Dotagents);
        assert_eq!(skills[0].deployments.len(), 3);
    }

    #[test]
    fn assembled_skills_are_sorted_by_name() {
        let skills = assemble_installed_skills(
            vec![
                candidate("zeta", "Codex"),
                candidate("alpha", "Codex"),
                candidate("mid", "pi"),
            ],
            &empty_lock(),
        );
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mid", "zeta"]);
    }

    #[test]
    fn merge_of_one_skill_across_three_roots_yields_three_deployments() {
        let a = candidate("my-skill", "Claude Code");
        let b = candidate("my-skill", "Codex");
        let c = candidate("my-skill", "pi");

        let skills = assemble_installed_skills(vec![a, b, c], &empty_lock());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].deployments.len(), 3);
    }

    #[test]
    fn violations_are_deduped() {
        let mut a = candidate("my-skill", "Claude Code");
        a.spec_violations = vec!["missing required frontmatter field: description".to_string()];
        let mut b = candidate("my-skill", "Codex");
        b.spec_violations = vec!["missing required frontmatter field: description".to_string()];

        let skills = assemble_installed_skills(vec![a, b], &empty_lock());
        assert_eq!(skills[0].spec_violations.len(), 1);
    }

    #[test]
    fn lock_entry_seeds_source_metadata() {
        let c = candidate("write-tests", "Claude Code");
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

        let skills = assemble_installed_skills(vec![c], &lock);
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

        let skills = assemble_installed_skills(vec![], &lock);
        assert_eq!(skills.len(), 1);
        assert!(skills[0].deployments.is_empty());
        assert_eq!(skills[0].source_kind, SourceKind::SkillsSh);
    }

    #[test]
    fn aggregate_facts_come_from_first_readable_candidate_not_first_by_order() {
        let mut broken = candidate("my-skill", "Claude Code");
        broken.content_hash = String::new();
        broken.symlink_is_broken = true;

        let mut valid = candidate("my-skill", "Codex");
        valid.content_hash = "hash-codex".to_string();
        valid.skill_md_tokens = 42;
        valid.description_tokens = 7;
        valid.folder_bytes = 100;
        valid.file_count = 3;

        let skills = assemble_installed_skills(vec![broken, valid], &empty_lock());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].deployments.len(), 2);
        assert_eq!(skills[0].content_hash, "hash-codex");
        assert_eq!(skills[0].skill_md_tokens, 42);
        assert_eq!(skills[0].description_tokens, 7);
        assert_eq!(skills[0].folder_bytes, 100);
        assert_eq!(skills[0].file_count, 3);
    }

    #[test]
    fn distinct_content_hashes_are_collected() {
        let mut a = candidate("my-skill", "Claude Code");
        a.content_hash = "hash-a".to_string();
        let mut b = candidate("my-skill", "Codex");
        b.content_hash = "hash-b".to_string();
        let mut c = candidate("my-skill", "pi");
        c.content_hash = "hash-a".to_string();

        let skills = assemble_installed_skills(vec![a, b, c], &empty_lock());
        assert_eq!(skills[0].content_hashes.len(), 2);
    }

    #[test]
    fn each_deployment_carries_its_own_content_hash() {
        let mut a = candidate("my-skill", "Claude Code");
        a.content_hash = "hash-a".to_string();
        let mut b = candidate("my-skill", "Codex");
        b.content_hash = "hash-b".to_string();

        let skills = assemble_installed_skills(vec![a, b], &empty_lock());
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
