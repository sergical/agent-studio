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
use crate::ports::{confine, ExclusiveGuard, ScopeFs};
use crate::scope::NormalizedScope;

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
    /// Every other key the entry carries, kept verbatim. `npx skills` owns
    /// this file and writes keys this reader does not model (`dismissed`,
    /// `lastSelectedAgents`, and whatever a newer release adds); without a
    /// catch-all a read/write round trip through the core would drop them.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
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

/// The lock file's name inside an `.agents` directory - the one string
/// every reader of the shared lock file joins onto its own root, so it
/// isn't duplicated at each call site.
pub const LOCK_FILE_NAME: &str = ".skill-lock.json";

/// `<home>/.agents/.skill-lock.json` - the path every reader of the shared
/// lock file, real or fixture home, resolves against.
pub fn lock_file_path(home: &Path) -> PathBuf {
    lock_file_path_in(&home.join(".agents"))
}

/// `<agents_dir>/.skill-lock.json` - for callers that already have an
/// `.agents` directory in hand (a project's, not just the home's).
pub fn lock_file_path_in(agents_dir: &Path) -> PathBuf {
    agents_dir.join(LOCK_FILE_NAME)
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

/// The raw JSON value for `skill_name`'s row in the lock file at `path`,
/// kept exactly as written - unknown fields included - so a caller that
/// saves it before letting `npx skills remove` drop the row (`ops::remove`)
/// can hand it to [`restore_lock_entry`] byte-for-byte, rather than losing
/// whatever [`InstalledSkillEntry`]'s typed fields do not model. `None` when
/// the file is missing or `skill_name` has no entry.
pub fn read_lock_entry_value(
    fs: &dyn ScopeFs,
    path: &Path,
    skill_name: &str,
) -> Result<Option<serde_json::Value>, CoreError> {
    let bytes = match fs.read_capped(path, LOCK_FILE_MAX_BYTES) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(CoreError::io(path, e)),
    };
    let doc: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        CoreError::new(ErrorCode::Io, format!("failed to parse lock file: {e}")).at(path)
    })?;
    Ok(doc
        .get("skills")
        .and_then(|skills| skills.get(skill_name))
        .cloned())
}

/// Writes `entry` back into the lock file at `path` under `skill_name`,
/// through the caller's already-held exclusive lease - the same
/// read-whole-document/mutate-one-key/write-atomic shape
/// `ops_remove::drop_registry_entry` uses to drop a `skill-studio.json` row,
/// run in reverse, keeping every other key (including the file's own
/// `version`) untouched so `npx skills`, not this write, still owns the
/// file's schema. A no-op when `skill_name` already has an entry: a
/// reinstall that raced the undo keeps its own row rather than losing it to
/// the one being restored.
pub fn restore_lock_entry(
    guard: &ExclusiveGuard,
    fs: &dyn ScopeFs,
    scope: &NormalizedScope,
    path: &Path,
    skill_name: &str,
    entry: &serde_json::Value,
) -> Result<(), CoreError> {
    let mut doc: serde_json::Value = match fs.read_capped(path, LOCK_FILE_MAX_BYTES) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            CoreError::new(ErrorCode::Io, format!("failed to parse lock file: {e}")).at(path)
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let empty = empty_lock_file();
            serde_json::json!({ "version": empty.version, "skills": {} })
        }
        Err(e) => return Err(CoreError::io(path, e)),
    };
    let skills = doc
        .as_object_mut()
        .ok_or_else(|| CoreError::new(ErrorCode::Io, "lock file is not a JSON object").at(path))?
        .entry("skills")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let skills = skills.as_object_mut().ok_or_else(|| {
        CoreError::new(ErrorCode::Io, "lock file's \"skills\" is not a JSON object").at(path)
    })?;
    if skills.contains_key(skill_name) {
        return Ok(());
    }
    skills.insert(skill_name.to_string(), entry.clone());
    let bytes = serde_json::to_vec(&doc).map_err(|e| {
        CoreError::new(ErrorCode::Io, format!("failed to serialize lock file: {e}")).at(path)
    })?;
    if let Some(parent) = path.parent() {
        let scoped_parent = confine(scope, fs, parent)?;
        fs.create_dir_all(guard, &scoped_parent)
            .map_err(|e| CoreError::io(parent, e))?;
    }
    let scoped = confine(scope, fs, path)?;
    fs.write_atomic(guard, &scoped, &bytes)
        .map_err(|e| CoreError::io(path, e))
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
