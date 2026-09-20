//! `~/.agents/.skill-lock.json` - the shared install ledger written by
//! `npx skills`.
//!
//! Ported from the desktop app's `skills/lock_file.rs`. The core never
//! resolves the home directory itself: callers pass the already-normalized
//! home root (from [`crate::scope::NormalizedScope`]) and read the bytes
//! through [`crate::ports::ScopeFs::read_capped`].

use std::collections::{HashMap, HashSet};
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

/// `<project>/skills-lock.json`'s file name - the CLI's own project-scope
/// lock file (schema version 1), written next to the project root rather
/// than under `.agents`, and shaped differently from the shared
/// `.skill-lock.json` above (no `sourceUrl`/`skillFolderHash`/timestamps,
/// just `source`/`sourceType`/`computedHash`). The core only reads it, to
/// classify a project-scope skills.sh install's ownership; `npx skills`
/// keeps owning the write.
pub const PROJECT_LOCK_FILE_NAME: &str = "skills-lock.json";

/// `<project>/skills-lock.json`'s path.
pub fn project_lock_file_path(project: &Path) -> PathBuf {
    project.join(PROJECT_LOCK_FILE_NAME)
}

/// Only the key set of a v1 `skills-lock.json`'s `skills` map - every other
/// field is per-entry provenance `is_skill_installed`'s callers don't need.
#[derive(Debug, Clone, Deserialize, Default)]
struct ProjectLockFile {
    #[serde(default)]
    skills: HashMap<String, serde_json::Value>,
}

/// The skill names a project-scope `skills-lock.json` at `path` names, or an
/// empty set when the file is missing, oversized, or not valid JSON -
/// matching [`read_lock_file`]'s "no ledger" behavior instead of failing a
/// scan over a file `npx skills` may not have written yet.
pub fn read_project_lock_skill_names(fs: &dyn ScopeFs, path: &Path) -> HashSet<String> {
    let Ok(bytes) = fs.read_capped(path, LOCK_FILE_MAX_BYTES) else {
        return HashSet::new();
    };
    serde_json::from_slice::<ProjectLockFile>(&bytes)
        .map(|f| f.skills.into_keys().collect())
        .unwrap_or_default()
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

    /// A real v1 `skills-lock.json` sample (PR #295's
    /// `03-add-local-folder-project` fixture) parses to its one skill name,
    /// even though it carries none of the shared lock file's required
    /// fields (`sourceUrl`, `skillFolderHash`, timestamps) - proof the two
    /// shapes are read independently, not through the same struct.
    #[test]
    fn reads_the_v1_project_lock_file_shape_the_shared_lock_file_cannot_parse() {
        let json = r#"{
            "version": 1,
            "skills": {
                "my-local-skill": {
                    "source": "../../../my-skill",
                    "sourceType": "local",
                    "computedHash": "222854256926340ae167d8b3e6c43ab9755db110c5e7a79fe0fff971605a61d5"
                }
            }
        }"#;
        let fs = FixtureBuilder::new()
            .dir("/proj")
            .file("/proj/skills-lock.json", json.as_bytes())
            .build_fs();
        assert!(read_lock_file(&fs, &project_lock_file_path(Path::new("/proj"))).is_err());

        let names = read_project_lock_skill_names(&fs, &project_lock_file_path(Path::new("/proj")));
        assert_eq!(names, HashSet::from(["my-local-skill".to_string()]));
    }

    #[test]
    fn missing_project_lock_file_yields_no_names_or_names_the_wrongly_failed_scan() {
        let fs = FixtureBuilder::new().dir("/proj").build_fs();
        let names = read_project_lock_skill_names(&fs, &project_lock_file_path(Path::new("/proj")));
        assert!(names.is_empty());
    }

    /// `read_project_lock_skill_names_returns_empty_or_the_names_it_can_still_read_for_every_malformed_shape`:
    /// table over the shapes `npx skills` could plausibly leave behind - a
    /// truncated write, a schema bump, a hand-edited file, or one too big to
    /// be this file at all - each named by what it returns rather than by
    /// how it fails, since this reader never surfaces an error (it matches
    /// `read_lock_file`'s "no ledger" behavior, see its own doc comment).
    #[test]
    fn read_project_lock_skill_names_returns_empty_or_the_names_it_can_still_read_for_every_malformed_shape(
    ) {
        let cases: Vec<(&str, &[u8], HashSet<String>)> = vec![
            (
                "malformed JSON yields no names",
                b"not json at all",
                HashSet::new(),
            ),
            (
                // No version check exists in `read_project_lock_skill_names`
                // (unlike `read_lock_file`'s shared-lock schema): a version
                // bump alone doesn't invalidate the `skills` map it already
                // parsed, so this still returns the one name.
                "a version other than 1 still yields the names it can parse",
                br#"{"version": 2, "skills": {"a-skill": {"source": "x"}}}"#,
                HashSet::from(["a-skill".to_string()]),
            ),
            (
                "a non-object skills value yields no names",
                br#"{"version": 1, "skills": "not-an-object"}"#,
                HashSet::new(),
            ),
        ];
        for (label, bytes, expected) in cases {
            let fs = FixtureBuilder::new()
                .dir("/proj")
                .file("/proj/skills-lock.json", bytes)
                .build_fs();
            let names =
                read_project_lock_skill_names(&fs, &project_lock_file_path(Path::new("/proj")));
            assert_eq!(names, expected, "{label}");
        }
    }

    #[test]
    fn read_project_lock_skill_names_over_a_file_past_the_size_cap_yields_no_names() {
        let mut json = String::from(r#"{"version": 1, "skills": {"a": {"padding": ""#);
        json.push_str(&"x".repeat(LOCK_FILE_MAX_BYTES as usize + 1));
        json.push_str(r#""}}}"#);
        let fs = FixtureBuilder::new()
            .dir("/proj")
            .file("/proj/skills-lock.json", json.as_bytes())
            .build_fs();
        let names = read_project_lock_skill_names(&fs, &project_lock_file_path(Path::new("/proj")));
        assert!(
            names.is_empty(),
            "a file over the size cap must yield no names, not a truncated parse"
        );
    }
}
