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
use skill_studio_core::tracked_projects::TrackedProjects;

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
}

/// Builds the card's rows from `discovered` (this run's discovery pass) and
/// `tracked` (the saved added/excluded lists): the folders `tracked.resolve`
/// would hand to a snapshot, labelled by source, plus any `tracked.added`
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

    let resolved = tracked.resolve(fs, home, discovered);
    let mut rows: Vec<ProjectFolder> = resolved
        .iter()
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
            }
        })
        .collect();

    rows.extend(
        tracked
            .added
            .iter()
            .filter(|path| fs.canonicalize(path).is_err())
            .map(|path| ProjectFolder {
                path: path.to_string_lossy().to_string(),
                source: ProjectFolderSource::Added,
                missing: true,
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
                },
                ProjectFolder {
                    path: missing.to_string_lossy().to_string(),
                    source: ProjectFolderSource::Added,
                    missing: true,
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
}
