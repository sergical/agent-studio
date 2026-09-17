// ============================================================================
// Skills Module - Project Folders
// The Settings "Project folders" card's data: every folder discovery found
// or the user added by hand, each labelled with where it came from, so the
// UI can offer the right action ("Stop tracking" vs. "Remove") and show a
// folder that vanished instead of dropping it silently.
// ============================================================================

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use skill_studio_core::ports::ScopeFs;
use skill_studio_core::tracked_projects::{self, expand_home, TrackedProjects};

/// Where a [`ProjectFolder`] came from - decides which action the row offers.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectFolderSource {
    /// Found in a harness's own history (Codex config, Claude Code
    /// transcripts, ...). Offers "Stop tracking".
    Discovered,
    /// Added by the user through "Add project…"/"Add folder…". Offers
    /// "Remove".
    Added,
}

/// One row of the Settings "Project folders" card.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ProjectFolder {
    /// The path as the caller or the file gave it, same as
    /// `SkillSnapshot::projects` - deployments' `project_path` compares
    /// against this exact string.
    pub path: String,
    pub source: ProjectFolderSource,
    /// True when the path no longer exists on disk - shown as "Folder not
    /// found" instead of being dropped, since a folder the user added by
    /// hand shouldn't disappear from the list without a trace.
    pub missing: bool,
    /// `None` for a plain folder; for a `*`-suffixed pattern, how many of
    /// its currently matching folders are in the resolved set. A pattern
    /// gets one row of its own instead of one row per matched folder.
    pub matches: Option<u32>,
}

/// Builds the card's rows from `discovered` (this run's discovery pass) and
/// `tracked` (the saved added/excluded lists): the folders `tracked.resolve`
/// would hand to a snapshot, labelled by source, one row per `*` pattern
/// instead of one row per folder it matched, plus any plain `tracked.added`
/// path that no longer exists.
pub fn project_folders(
    fs: &dyn ScopeFs,
    home: &Path,
    discovered: Vec<PathBuf>,
    tracked: &TrackedProjects,
) -> Vec<ProjectFolder> {
    let discovered_canonical: Vec<PathBuf> = discovered
        .iter()
        .filter_map(|p| fs.canonicalize(p).ok())
        .collect();

    // Canonical, not lexical: a plain added path reached through a symlink must still match the
    // resolved root's canonical path, or it would lose its row whenever a pattern also matches it.
    let plain_added_canonical: Vec<PathBuf> = tracked
        .added
        .iter()
        .filter(|path| !tracked_projects::is_pattern(path))
        .filter_map(|path| fs.canonicalize(&expand_home(path, home)).ok())
        .collect();

    let pattern_matches = tracked.pattern_matches(fs, home);
    let pattern_child_canonical: Vec<PathBuf> = pattern_matches
        .iter()
        .flat_map(|pm| pm.folders.iter())
        .filter_map(|p| fs.canonicalize(p).ok())
        .collect();

    let resolved = tracked.resolve(fs, home, discovered);
    let mut rows: Vec<ProjectFolder> = resolved
        .iter()
        .filter(|root| {
            let pattern_only = pattern_child_canonical.contains(&root.canonical)
                && !discovered_canonical.contains(&root.canonical)
                && !plain_added_canonical.contains(&root.canonical);
            !pattern_only
        })
        .map(|root| {
            let source = if discovered_canonical.contains(&root.canonical) {
                ProjectFolderSource::Discovered
            } else {
                ProjectFolderSource::Added
            };
            ProjectFolder {
                path: root.lexical.to_string_lossy().to_string(),
                source,
                missing: false,
                matches: None,
            }
        })
        .collect();

    for pm in &pattern_matches {
        let matched = pm
            .folders
            .iter()
            .filter_map(|folder| fs.canonicalize(folder).ok())
            .filter(|canonical| resolved.iter().any(|root| &root.canonical == canonical))
            .count() as u32;
        rows.push(ProjectFolder {
            path: pm.pattern.to_string_lossy().to_string(),
            source: ProjectFolderSource::Added,
            missing: !pm.parent_found,
            matches: Some(matched),
        });
    }

    rows.extend(
        tracked
            .added
            .iter()
            .filter(|path| !tracked_projects::is_pattern(path))
            .filter(|path| fs.canonicalize(&expand_home(path, home)).is_err())
            .map(|path| ProjectFolder {
                path: path.to_string_lossy().to_string(),
                source: ProjectFolderSource::Added,
                missing: true,
                matches: None,
            }),
    );
    rows
}

/// The Settings "Project folders" card's rows, freshly discovered - runs a
/// full harness-history scan, so it's spawned off the main thread.
#[tauri::command]
pub async fn list_project_folders() -> Result<Vec<ProjectFolder>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let discovered = skill_studio_host::discover_skill_projects(&home);
        let tracked = TrackedProjects::read(&skill_studio_host::RealFs, &home);
        Ok(project_folders(
            &skill_studio_host::RealFs,
            &home,
            discovered,
            &tracked,
        ))
    })
    .await
    .map_err(|e| format!("Listing project folders failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn home_and_project(name: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join(name);
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&project).unwrap();
        (tmp, home, project)
    }

    #[test]
    fn discovered_only_folder_is_discovered() {
        let (_tmp, home, project) = home_and_project("proj");
        let tracked = TrackedProjects::default();
        let rows = project_folders(
            &skill_studio_host::RealFs,
            &home,
            vec![project.clone()],
            &tracked,
        );
        assert_eq!(
            rows,
            [ProjectFolder {
                path: project.to_string_lossy().to_string(),
                source: ProjectFolderSource::Discovered,
                missing: false,
                matches: None,
            }]
        );
    }

    #[test]
    fn added_only_existing_folder_is_added() {
        let (_tmp, home, project) = home_and_project("proj");
        let tracked = TrackedProjects {
            added: vec![project.clone()],
            ..Default::default()
        };
        let rows = project_folders(&skill_studio_host::RealFs, &home, vec![], &tracked);
        assert_eq!(
            rows,
            [ProjectFolder {
                path: project.to_string_lossy().to_string(),
                source: ProjectFolderSource::Added,
                missing: false,
                matches: None,
            }]
        );
    }

    #[test]
    fn discovered_and_added_folder_is_discovered_and_listed_once() {
        let (_tmp, home, project) = home_and_project("proj");
        let tracked = TrackedProjects {
            added: vec![project.clone()],
            ..Default::default()
        };
        let rows = project_folders(
            &skill_studio_host::RealFs,
            &home,
            vec![project.clone()],
            &tracked,
        );
        assert_eq!(
            rows,
            [ProjectFolder {
                path: project.to_string_lossy().to_string(),
                source: ProjectFolderSource::Discovered,
                missing: false,
                matches: None,
            }]
        );
    }

    #[test]
    fn added_folder_that_does_not_exist_is_added_and_missing_and_listed_last() {
        let (_tmp, home, project) = home_and_project("proj");
        let missing = home.join("gone");
        let tracked = TrackedProjects {
            added: vec![project.clone(), missing.clone()],
            ..Default::default()
        };
        let rows = project_folders(&skill_studio_host::RealFs, &home, vec![], &tracked);
        assert_eq!(
            rows,
            [
                ProjectFolder {
                    path: project.to_string_lossy().to_string(),
                    source: ProjectFolderSource::Added,
                    missing: false,
                    matches: None,
                },
                ProjectFolder {
                    path: missing.to_string_lossy().to_string(),
                    source: ProjectFolderSource::Added,
                    missing: true,
                    matches: None,
                },
            ]
        );
    }

    #[test]
    fn excluded_discovered_folder_is_absent() {
        let (_tmp, home, project) = home_and_project("proj");
        let tracked = TrackedProjects {
            excluded: vec![project.clone()],
            ..Default::default()
        };
        let rows = project_folders(&skill_studio_host::RealFs, &home, vec![project], &tracked);
        assert!(rows.is_empty());
    }

    #[test]
    fn home_in_added_is_absent() {
        let (_tmp, home, _project) = home_and_project("proj");
        let tracked = TrackedProjects {
            added: vec![home.clone()],
            ..Default::default()
        };
        let rows = project_folders(&skill_studio_host::RealFs, &home, vec![], &tracked);
        assert!(rows.is_empty());
    }

    #[test]
    fn pattern_gets_one_row_with_a_count_and_no_child_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join("src/a/.claude/skills/x")).unwrap();
        fs::write(home.join("src/a/.claude/skills/x/SKILL.md"), "").unwrap();
        fs::create_dir_all(home.join("src/b/.claude/skills/x")).unwrap();
        fs::write(home.join("src/b/.claude/skills/x/SKILL.md"), "").unwrap();

        let tracked = TrackedProjects {
            added: vec![PathBuf::from("~/src/*")],
            ..Default::default()
        };
        let rows = project_folders(&skill_studio_host::RealFs, &home, vec![], &tracked);

        assert_eq!(
            rows,
            [ProjectFolder {
                path: "~/src/*".to_string(),
                source: ProjectFolderSource::Added,
                missing: false,
                matches: Some(2),
            }]
        );
    }

    #[test]
    fn a_pattern_child_that_is_also_discovered_keeps_its_discovered_row() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = home.join("src/a");
        fs::create_dir_all(project.join(".claude/skills/x")).unwrap();
        fs::write(project.join(".claude/skills/x/SKILL.md"), "").unwrap();

        let tracked = TrackedProjects {
            added: vec![PathBuf::from("~/src/*")],
            ..Default::default()
        };
        let rows = project_folders(
            &skill_studio_host::RealFs,
            &home,
            vec![project.clone()],
            &tracked,
        );

        assert_eq!(
            rows,
            [
                ProjectFolder {
                    path: project.to_string_lossy().to_string(),
                    source: ProjectFolderSource::Discovered,
                    missing: false,
                    matches: None,
                },
                ProjectFolder {
                    path: "~/src/*".to_string(),
                    source: ProjectFolderSource::Added,
                    missing: false,
                    matches: Some(1),
                },
            ]
        );
    }

    #[test]
    fn a_pattern_with_a_missing_parent_is_a_missing_row() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let tracked = TrackedProjects {
            added: vec![PathBuf::from("~/nope/*")],
            ..Default::default()
        };
        let rows = project_folders(&skill_studio_host::RealFs, &home, vec![], &tracked);

        assert_eq!(
            rows,
            [ProjectFolder {
                path: "~/nope/*".to_string(),
                source: ProjectFolderSource::Added,
                missing: true,
                matches: Some(0),
            }]
        );
    }
}
