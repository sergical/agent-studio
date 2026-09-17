//! `~/.agents/.skill-lock.json` - the shared install ledger written by
//! `npx skills`.
//!
//! Ported from the desktop app's `skills/lock_file.rs`. The core never
//! resolves the home directory itself: callers pass the already-normalized
//! home root (from [`crate::scope::NormalizedScope`]) and read the bytes
//! through [`crate::ports::ScopeFs::read_capped`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, ErrorCode};
use crate::ports::ScopeFs;

/// Largest lock file the core will read. Larger is treated as corrupt
/// rather than silently truncated.
pub const LOCK_FILE_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// One installed skill's provenance, as recorded by `npx skills`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledSkillEntry {
    /// `owner/repo` or other source identifier.
    pub source: String,
    /// Where `source` came from (e.g. `"github"`).
    #[serde(rename = "sourceType")]
    pub source_type: String,
    /// Canonical URL for `source`.
    #[serde(rename = "sourceUrl")]
    pub source_url: String,
    /// Path of the skill within its source, when it isn't the source root.
    #[serde(rename = "skillPath", default)]
    pub skill_path: Option<String>,
    /// Content hash of the installed skill folder at install time.
    #[serde(rename = "skillFolderHash")]
    pub skill_folder_hash: String,
    /// ISO-8601 install timestamp.
    #[serde(rename = "installedAt")]
    pub installed_at: String,
    /// ISO-8601 last-update timestamp.
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// The lock file's top-level shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillLockFile {
    /// Lock file format version.
    pub version: u32,
    /// Installed skills, keyed by skill name.
    pub skills: HashMap<String, InstalledSkillEntry>,
}

/// The default, empty lock file returned when none exists on disk yet.
fn empty_lock_file() -> SkillLockFile {
    SkillLockFile {
        version: 3,
        skills: HashMap::new(),
    }
}

/// `<home>/.agents/.skill-lock.json` - the path every reader of the shared
/// lock file, real or fixture home, resolves against.
pub fn lock_file_path(home: &Path) -> PathBuf {
    home.join(".agents").join(".skill-lock.json")
}

/// Reads and parses the lock file at `path` through `fs`.
///
/// A missing file is not an error: it yields the same empty, version-3 lock
/// file a fresh install would produce. A file larger than
/// [`LOCK_FILE_MAX_BYTES`] or one that fails to parse as JSON is
/// [`ErrorCode::Io`].
pub fn read_lock_file(fs: &dyn ScopeFs, path: &Path) -> Result<SkillLockFile, CoreError> {
    let bytes = match fs.read_capped(path, LOCK_FILE_MAX_BYTES) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(empty_lock_file()),
        Err(e) => return Err(CoreError::io(path, e)),
    };
    serde_json::from_slice(&bytes).map_err(|e| {
        CoreError::new(ErrorCode::Io, format!("failed to parse lock file: {e}")).at(path)
    })
}

/// Whether `skill_name` has an entry in `lock`.
pub fn is_skill_installed(lock: &SkillLockFile, skill_name: &str) -> bool {
    lock.skills.contains_key(skill_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureBuilder;

    #[test]
    fn missing_lock_file_yields_empty_default() {
        let fs = FixtureBuilder::new().dir("/home").build_fs();
        let lock = read_lock_file(&fs, &lock_file_path(Path::new("/home"))).unwrap();
        assert_eq!(lock.version, 3);
        assert!(lock.skills.is_empty());
    }

    #[test]
    fn parses_an_existing_lock_file() {
        let json = r#"{
            "version": 3,
            "skills": {
                "write-tests": {
                    "source": "owner/repo",
                    "sourceType": "github",
                    "sourceUrl": "https://github.com/owner/repo",
                    "skillFolderHash": "abc123",
                    "installedAt": "2024-01-31T00:00:00Z",
                    "updatedAt": "2024-01-31T00:00:00Z"
                }
            }
        }"#;
        let fs = FixtureBuilder::new()
            .dir("/home/.agents")
            .file("/home/.agents/.skill-lock.json", json.as_bytes())
            .build_fs();
        let lock = read_lock_file(&fs, &lock_file_path(Path::new("/home"))).unwrap();
        assert_eq!(lock.version, 3);
        assert!(is_skill_installed(&lock, "write-tests"));
        assert!(!is_skill_installed(&lock, "other-skill"));
    }

    #[test]
    fn malformed_lock_file_is_an_io_error() {
        let fs = FixtureBuilder::new()
            .dir("/home/.agents")
            .file("/home/.agents/.skill-lock.json", b"not json")
            .build_fs();
        let err = read_lock_file(&fs, &lock_file_path(Path::new("/home"))).unwrap_err();
        assert_eq!(err.code, ErrorCode::Io);
    }
}
