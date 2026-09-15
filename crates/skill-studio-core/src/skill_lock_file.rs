// ============================================================================
// Skills Module - Lock File
// Read and parse the skill lock file (~/.agents/.skill-lock.json)
// ============================================================================

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Installed skill entry in lock file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledSkillEntry {
    pub source: String,
    #[serde(rename = "sourceType")]
    pub source_type: String,
    #[serde(rename = "sourceUrl")]
    pub source_url: String,
    #[serde(rename = "skillPath", default)]
    pub skill_path: Option<String>,
    #[serde(rename = "skillFolderHash")]
    pub skill_folder_hash: String,
    #[serde(rename = "installedAt")]
    pub installed_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// Lock file structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillLockFile {
    pub version: u32,
    pub skills: std::collections::HashMap<String, InstalledSkillEntry>,
}

pub fn empty_lock_file() -> SkillLockFile {
    SkillLockFile {
        version: 3,
        skills: std::collections::HashMap::new(),
    }
}

/// `~/.agents/.skill-lock.json` - the path every reader of the shared lock
/// file, real or a test's temp home, resolves against.
pub fn lock_file_path(home: &Path) -> PathBuf {
    home.join(".agents").join(".skill-lock.json")
}

/// Read and parse the skill lock file at an explicit path.
pub fn read_lock_file_at(lock_path: &Path) -> Result<SkillLockFile, String> {
    Ok(read_lock_file_optional_at(lock_path)?.unwrap_or_else(empty_lock_file))
}

/// Strict ownership-reader variant. `None` means only that the file itself
/// was absent; callers must retain an error rather than classify from empty
/// lock data.
pub fn read_lock_file_optional_at(lock_path: &Path) -> Result<Option<SkillLockFile>, String> {
    let content = match fs::read_to_string(lock_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if matches!(fs::symlink_metadata(lock_path), Err(metadata_error) if metadata_error.kind() == std::io::ErrorKind::NotFound)
            {
                return Ok(None);
            }
            return Err(format!("Failed to read lock file: {error}"));
        }
        Err(error) => return Err(format!("Failed to read lock file: {error}")),
    };
    parse_lock_file(&content).map(Some)
}

pub(crate) fn parse_lock_file(content: &str) -> Result<SkillLockFile, String> {
    serde_json::from_str(content).map_err(|error| format!("Failed to parse lock file: {error}"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn missing_lock_file_is_optional() {
        let tmp = tempfile::tempdir().unwrap();

        let lock = read_lock_file_at(&tmp.path().join(".agents/.skill-lock.json")).unwrap();

        assert_eq!(lock.version, 3);
        assert!(lock.skills.is_empty());
    }

    #[test]
    fn missing_lock_file_returns_none_from_optional_reader() {
        let tmp = tempfile::tempdir().unwrap();

        let lock =
            read_lock_file_optional_at(&tmp.path().join(".agents/.skill-lock.json")).unwrap();

        assert!(lock.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_lock_file_symlink_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let lock_path = tmp.path().join("lock.json");
        std::os::unix::fs::symlink("missing.json", &lock_path).unwrap();

        let error = read_lock_file_optional_at(&lock_path).unwrap_err();

        assert!(error.starts_with("Failed to read lock file:"));
        assert!(read_lock_file_at(&lock_path).is_err());
        assert!(fs::symlink_metadata(&lock_path)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn malformed_lock_file_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let lock_path = tmp.path().join(".agents/.skill-lock.json");
        fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        fs::write(&lock_path, "not json").unwrap();

        let error = read_lock_file_at(&lock_path).unwrap_err();

        assert!(error.starts_with("Failed to parse lock file:"));
    }

    #[test]
    fn non_directory_lock_parent_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_path = tmp.path().join(".agents");
        fs::write(&agents_path, "not a directory").unwrap();

        let error = read_lock_file_at(&agents_path.join(".skill-lock.json")).unwrap_err();

        assert!(error.starts_with("Failed to read lock file:"));
    }

    #[test]
    fn directory_at_lock_path_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let lock_path = tmp.path().join(".agents/.skill-lock.json");
        fs::create_dir_all(&lock_path).unwrap();

        let error = read_lock_file_at(&lock_path).unwrap_err();

        assert!(error.starts_with("Failed to read lock file:"));
    }
}
