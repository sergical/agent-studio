//! [`ScopeFs`] over real `std::fs`.

use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use skill_studio_core::ports::{
    DirEntryFacts, ExclusiveGuard, FileFacts, FileKind, ScopeFs, ScopedPath,
};

/// A counter mixed into temp file names so concurrent writers on the same
/// process never collide.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `ScopeFs` backed by the real filesystem.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealFs;

impl RealFs {
    /// Builds a new adapter. Holds no state; every call goes straight to the OS.
    pub fn new() -> Self {
        RealFs
    }
}

fn to_facts(meta: &fs::Metadata) -> FileFacts {
    let kind = if meta.file_type().is_symlink() {
        FileKind::Symlink
    } else if meta.is_dir() {
        FileKind::Dir
    } else if meta.is_file() {
        FileKind::File
    } else {
        FileKind::Other
    };
    let len = if kind == FileKind::File {
        meta.len()
    } else {
        0
    };
    let modified = meta.modified().ok().map(DateTime::<Utc>::from);
    FileFacts {
        kind,
        len,
        modified,
        mode: Some(meta.mode()),
    }
}

impl ScopeFs for RealFs {
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(path)
    }

    fn symlink_metadata(&self, path: &Path) -> io::Result<FileFacts> {
        fs::symlink_metadata(path).map(|m| to_facts(&m))
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        fs::read_link(path)
    }

    fn ancestor_holds(&self, start: &Path, name: &str) -> io::Result<bool> {
        skill_studio_core::ports::ancestor_holds(self, start, name)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntryFacts>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let kind = if file_type.is_symlink() {
                FileKind::Symlink
            } else if file_type.is_dir() {
                FileKind::Dir
            } else if file_type.is_file() {
                FileKind::File
            } else {
                FileKind::Other
            };
            out.push(DirEntryFacts {
                name: entry.file_name().to_string_lossy().into_owned(),
                kind,
            });
        }
        Ok(out)
    }

    fn read_capped(&self, path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
        let len = fs::metadata(path)?.len();
        if len > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} is {len} bytes, over the {max_bytes} byte cap",
                    path.display()
                ),
            ));
        }
        fs::read(path)
    }

    fn read_prefix(&self, path: &Path, limit: u64) -> io::Result<(Vec<u8>, bool)> {
        use std::io::Read;
        let mut file = fs::File::open(path)?;
        let mut bytes = Vec::new();
        // Read one byte past the limit so truncation can be told apart from
        // "exactly at the limit" without ever buffering more than that.
        file.by_ref().take(limit + 1).read_to_end(&mut bytes)?;
        let truncated = bytes.len() as u64 > limit;
        if truncated {
            bytes.truncate(limit as usize);
        }
        Ok((bytes, truncated))
    }

    fn write_atomic(
        &self,
        _guard: &ExclusiveGuard,
        path: &ScopedPath,
        bytes: &[u8],
    ) -> io::Result<()> {
        let path = path.as_path();
        let dir = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
            .to_string_lossy();
        let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_path = dir.join(format!(".{file_name}.tmp-{}-{counter}", std::process::id()));
        let existing_mode = fs::symlink_metadata(path).ok().map(|m| m.mode());
        fs::write(&tmp_path, bytes)?;
        if let Some(mode) = existing_mode {
            fs::set_permissions(&tmp_path, fs::Permissions::from_mode(mode))?;
        }
        fs::rename(&tmp_path, path)
    }

    fn rename(
        &self,
        _guard: &ExclusiveGuard,
        from: &ScopedPath,
        to: &ScopedPath,
    ) -> io::Result<()> {
        fs::rename(from.as_path(), to.as_path())
    }

    fn remove_file(&self, _guard: &ExclusiveGuard, path: &ScopedPath) -> io::Result<()> {
        fs::remove_file(path.as_path())
    }

    fn create_dir_all(&self, _guard: &ExclusiveGuard, path: &ScopedPath) -> io::Result<()> {
        fs::create_dir_all(path.as_path())
    }

    fn symlink(
        &self,
        _guard: &ExclusiveGuard,
        target: &ScopedPath,
        link: &ScopedPath,
    ) -> io::Result<()> {
        // `target` is a `ScopedPath`, so it was already proven to lie inside
        // the scope by `confine` before this call was made.
        std::os::unix::fs::symlink(target.as_path(), link.as_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skill_studio_core::ports::confine;
    use skill_studio_core::scope::{NormalizedScope, RuntimeScope};

    fn scope_for(root: &Path) -> NormalizedScope {
        let raw = RuntimeScope::fixture(root);
        NormalizedScope::normalize_with_discovery(&raw, &RealFs::new(), None).unwrap()
    }

    #[test]
    fn round_trips_a_file_through_write_atomic_and_read_capped() {
        let dir = tempfile::tempdir().unwrap();
        let fs_adapter = RealFs::new();
        let scope = scope_for(dir.path());
        let target = dir.path().join("skill.md");
        let scoped = confine(&scope, &fs_adapter, &target).unwrap();

        // Reuse a fake guard: write_atomic never inspects it, only requires
        // one exist as proof the caller holds the exclusive lease.
        let guard = crate::lease::FileLease::new(dir.path().join("leases"));
        let held = skill_studio_core::ports::acquire_exclusive(&guard, &scope).unwrap();

        fs_adapter
            .write_atomic(&held, &scoped, b"---\nname: demo\n---\nbody")
            .unwrap();
        let bytes = fs_adapter.read_capped(&target, 4096).unwrap();
        assert_eq!(bytes, b"---\nname: demo\n---\nbody");
    }

    #[test]
    fn read_capped_rejects_a_file_over_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        fs::write(&path, vec![0u8; 16]).unwrap();
        let err = RealFs::new().read_capped(&path, 4).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn symlink_metadata_reports_symlink_without_following() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        fs::write(&target, b"x").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let facts = RealFs::new().symlink_metadata(&link).unwrap();
        assert_eq!(facts.kind, FileKind::Symlink);
    }

    #[test]
    fn read_prefix_truncates_a_file_over_the_limit_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        fs::write(&path, vec![b'x'; 16]).unwrap();
        let (bytes, truncated) = RealFs::new().read_prefix(&path, 4).unwrap();
        assert_eq!(bytes, vec![b'x'; 4]);
        assert!(truncated);
    }

    #[test]
    fn read_prefix_reports_no_truncation_for_a_file_at_or_under_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.txt");
        fs::write(&path, b"hello").unwrap();
        let (bytes, truncated) = RealFs::new().read_prefix(&path, 5).unwrap();
        assert_eq!(bytes, b"hello");
        assert!(!truncated);
    }

    #[test]
    fn read_dir_lists_entries() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), b"a").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        let mut entries = RealFs::new().read_dir(dir.path()).unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "a.txt");
        assert_eq!(entries[0].kind, FileKind::File);
        assert_eq!(entries[1].name, "sub");
        assert_eq!(entries[1].kind, FileKind::Dir);
    }
}
