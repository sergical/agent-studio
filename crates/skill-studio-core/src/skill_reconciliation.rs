use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::skill_inventory::InstalledSkill;

#[derive(Debug, PartialEq, Eq)]
pub enum NamedReplacementError {
    PartialRow(String),
    UnselectedReplacement(String),
    ConflictingRow(String),
}

impl std::fmt::Display for NamedReplacementError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (reason, name) = match self {
            Self::PartialRow(name) => ("cannot recompute a partially selected row", name),
            Self::UnselectedReplacement(name) => {
                ("replacement is outside the requested selection", name)
            }
            Self::ConflictingRow(name) => ("replacement conflicts with another row", name),
        };
        write!(formatter, "{reason}: {name}; a full refresh is required")
    }
}

impl std::error::Error for NamedReplacementError {}

pub fn deployment_is_selected(
    path: &Path,
    names: &BTreeSet<String>,
    targeted_paths: &BTreeSet<PathBuf>,
) -> bool {
    targeted_paths.contains(path)
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| names.contains(name))
}

/// Apply a named read after the caller has established membership and ownership coverage.
/// Validation completes before mutation. Remaining deployments in a partially selected
/// row need fresh aggregate facts, which this wire record cannot reconstruct.
pub fn replace_named_skills(
    skills: &mut Vec<InstalledSkill>,
    names: &BTreeSet<String>,
    targeted_paths: &BTreeSet<PathBuf>,
    mut replacements: Vec<InstalledSkill>,
) -> Result<(), NamedReplacementError> {
    let selected = |path: &str| deployment_is_selected(Path::new(path), names, targeted_paths);
    let mut removed = BTreeSet::new();
    let mut retained_names = BTreeSet::new();
    for (index, skill) in skills.iter().enumerate() {
        let selected_count = skill
            .deployments
            .iter()
            .filter(|deployment| selected(&deployment.path))
            .count();
        if selected_count > 0 && selected_count < skill.deployments.len() {
            return Err(NamedReplacementError::PartialRow(skill.name.clone()));
        }
        if selected_count > 0 || (skill.deployments.is_empty() && names.contains(&skill.name)) {
            removed.insert(index);
        } else {
            retained_names.insert(skill.name.as_str());
        }
    }
    for skill in &replacements {
        let in_selection = if skill.deployments.is_empty() {
            names.contains(&skill.name)
        } else {
            skill
                .deployments
                .iter()
                .all(|deployment| selected(&deployment.path))
        };
        if !in_selection {
            return Err(NamedReplacementError::UnselectedReplacement(
                skill.name.clone(),
            ));
        }
        if !retained_names.insert(&skill.name) {
            return Err(NamedReplacementError::ConflictingRow(skill.name.clone()));
        }
    }
    for skill in &mut replacements {
        skill.deployments.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| left.path.cmp(&right.path))
        });
    }
    let mut index = 0;
    skills.retain(|_| {
        let keep = !removed.contains(&index);
        index += 1;
        keep
    });
    skills.extend(replacements);
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_inventory::Deployment;

    fn row(name: &str, paths: &[&str]) -> InstalledSkill {
        let lock = crate::skill_lock_file::SkillLockFile {
            version: 3,
            skills: std::collections::HashMap::from([(
                name.to_string(),
                crate::skill_lock_file::InstalledSkillEntry {
                    source: "owner/repo".to_string(),
                    source_type: "github".to_string(),
                    source_url: "https://example.test/repo".to_string(),
                    skill_path: None,
                    skill_folder_hash: "hash".to_string(),
                    installed_at: "before".to_string(),
                    updated_at: "before".to_string(),
                },
            )]),
        };
        let mut skill = crate::skill_assembly::assemble_installed_skills(
            vec![],
            &lock,
            &crate::skill_ownership::OwnershipReadReport::empty(),
            &Default::default(),
        )
        .remove(0);
        skill.deployments = paths
            .iter()
            .map(|path| Deployment {
                id: format!("id:{path}"),
                path: path.to_string(),
                ..Default::default()
            })
            .collect();
        skill
    }

    #[test]
    fn replacement_preserves_unrelated_rows_and_handles_deletion_and_ledger_only_state() {
        let ledger = row("unrelated-ledger", &[]);
        let mut deployed = row("unrelated-deployed", &["/z/other", "/a/other"]);
        deployed.description = Some("preserve these details and deployment order".to_string());
        let ledger_before = serde_json::to_value(&ledger).unwrap();
        let deployed_before = serde_json::to_value(&deployed).unwrap();
        let mut skills = vec![
            ledger,
            deployed,
            row("alpha", &["/home/skills/alpha"]),
            row("deleted-plugin", &["/cache/plugin/skills/deleted-plugin"]),
            row("old-ledger", &[]),
        ];
        let names = BTreeSet::from([
            "alpha".to_string(),
            "deleted-plugin".to_string(),
            "old-ledger".to_string(),
            "new".to_string(),
        ]);
        replace_named_skills(
            &mut skills,
            &names,
            &BTreeSet::new(),
            vec![row("alpha", &[]), row("new", &["/home/skills/new"])],
        )
        .unwrap();
        assert_eq!(
            skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "new", "unrelated-deployed", "unrelated-ledger"]
        );
        assert!(skills[0].deployments.is_empty());
        assert_eq!(serde_json::to_value(&skills[2]).unwrap(), deployed_before);
        assert_eq!(serde_json::to_value(&skills[3]).unwrap(), ledger_before);
    }

    #[test]
    fn replacement_selects_lexical_paths_instead_of_old_document_names() {
        let mut skills = vec![
            row("old-document-name", &["/root/lexical"]),
            row("keep", &[]),
        ];
        replace_named_skills(
            &mut skills,
            &BTreeSet::from(["lexical".to_string()]),
            &BTreeSet::new(),
            vec![row("lexical", &["/root/lexical"])],
        )
        .unwrap();
        assert_eq!(
            skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["keep", "lexical"]
        );
    }

    #[test]
    fn invalid_replacements_leave_the_entire_snapshot_unchanged() {
        let names = BTreeSet::from(["alpha".to_string()]);
        let scenarios = [
            (
                vec![row("mixed", &["/root/alpha", "/root/other"])],
                vec![],
                NamedReplacementError::PartialRow("mixed".to_string()),
            ),
            (
                vec![row("keep", &[])],
                vec![row("other", &[])],
                NamedReplacementError::UnselectedReplacement("other".to_string()),
            ),
            (
                vec![row("keep", &[])],
                vec![row("alpha", &["/root/other"])],
                NamedReplacementError::UnselectedReplacement("alpha".to_string()),
            ),
            (
                vec![row("keep", &[])],
                vec![row("keep", &["/root/alpha"])],
                NamedReplacementError::ConflictingRow("keep".to_string()),
            ),
            (
                vec![row("keep", &[])],
                vec![row("alpha", &[]), row("alpha", &[])],
                NamedReplacementError::ConflictingRow("alpha".to_string()),
            ),
        ];
        for (mut skills, replacements, expected) in scenarios {
            let before = serde_json::to_value(&skills).unwrap();
            assert_eq!(
                replace_named_skills(&mut skills, &names, &BTreeSet::new(), replacements),
                Err(expected)
            );
            assert_eq!(serde_json::to_value(&skills).unwrap(), before);
        }
    }
}
