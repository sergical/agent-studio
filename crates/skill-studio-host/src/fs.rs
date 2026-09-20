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

    fn fsops_device_inode(&self, path: &Path) -> io::Result<(u64, u64)> {
        let meta = fs::symlink_metadata(path)?;
        Ok((meta.dev(), meta.ino()))
    }

    fn fsops_fsync_file(&self, path: &Path) -> io::Result<()> {
        fs::File::open(path)?.sync_all()
    }

    fn fsops_fsync_dir(&self, path: &Path) -> io::Result<()> {
        // A directory can be opened read-only and fsynced on Unix, which is
        // how a rename or a create inside it is made durable: the file's
        // own fsync only guarantees its contents, not that its name is
        // findable in the parent after a crash.
        fs::File::open(path)?.sync_all()
    }

    fn fsops_create_dir(&self, path: &Path) -> io::Result<()> {
        fs::create_dir(path)
    }

    fn fsops_write_new_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        use std::io::Write;
        file.write_all(bytes)
    }

    fn fsops_rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    fn fsops_symlink(&self, target: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    fn fsops_remove_dir(&self, path: &Path) -> io::Result<()> {
        fs::remove_dir(path)
    }

    fn fsops_remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    fn fsops_exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
        macos_exchange::exchange(a, b)
    }
}

/// Atomically exchanges two paths on the real filesystem: [`crate::fs`]'s
/// only `unsafe` code, kept to this one FFI call so [`fsops::swap`]
/// (`skill_studio_core::fsops`) has one crash-critical step instead of the
/// two ordinary renames a "move old aside, then move new in" sequence would
/// need - a process killed between those two renames would leave neither
/// the old nor the new folder at the final name.
///
/// [`fsops::swap`]: skill_studio_core::fsops::swap
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos_exchange {
    use std::ffi::CString;
    use std::io;
    use std::os::raw::{c_char, c_int, c_uint};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    /// From `<sys/fcntl.h>`: swap the two paths' directory entries.
    const RENAME_SWAP: c_uint = 0x0000_0002;

    extern "C" {
        // macOS-only libc entry point (10.12+); not part of the `std`
        // surface, so it is declared by hand instead of pulling in `libc`
        // for one function.
        fn renamex_np(from: *const c_char, to: *const c_char, flags: c_uint) -> c_int;
    }

    fn to_cstring(path: &Path) -> io::Result<CString> {
        CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
    }

    pub(super) fn exchange(a: &Path, b: &Path) -> io::Result<()> {
        let a = to_cstring(a)?;
        let b = to_cstring(b)?;
        // SAFETY: `a` and `b` are NUL-terminated `CString`s kept alive for
        // the duration of the call; `renamex_np` only reads them and
        // returns a plain `c_int` status, matching the C prototype above.
        let rc = unsafe { renamex_np(a.as_ptr(), b.as_ptr(), RENAME_SWAP) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod macos_exchange {
    use std::io;
    use std::path::Path;

    /// Non-atomic fallback for platforms without `renamex_np`. Skill Studio
    /// ships macOS only (see the crate doc comment); this exists so the
    /// crate still builds elsewhere, not to give the same crash guarantee.
    pub(super) fn exchange(a: &Path, b: &Path) -> io::Result<()> {
        let tmp = a.with_file_name(format!(
            ".exchange-{}-{}",
            std::process::id(),
            super::TMP_COUNTER.fetch_add(1, super::Ordering::Relaxed)
        ));
        std::fs::rename(a, &tmp)?;
        std::fs::rename(b, a)?;
        std::fs::rename(&tmp, b)
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

    /// The core's fake `read_link` mirrors this kind so the journal's
    /// "never replace a regular file at the link path" reversal check is
    /// exercised against the same error the real adapter raises.
    #[test]
    fn read_link_on_a_regular_file_reports_invalid_input_or_names_the_kind_it_returned() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plain.txt");
        fs::write(&file, b"x").unwrap();
        let err = RealFs::new().read_link(&file).unwrap_err();
        assert_eq!(
            err.kind(),
            io::ErrorKind::InvalidInput,
            "read_link on a regular file must report InvalidInput, not {err:?}"
        );
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

    #[test]
    fn canonicalize_resolves_a_real_path_or_returns_an_empty_default_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("skill.md");
        fs::write(&file, b"x").unwrap();

        let resolved = RealFs::new().canonicalize(&file).unwrap();
        let expected = fs::canonicalize(&file).unwrap();
        assert_eq!(
            resolved, expected,
            "canonicalize must resolve the real path, not a default (empty) one"
        );
        assert_ne!(resolved, PathBuf::default());
    }

    #[test]
    fn rename_moves_the_file_on_disk_or_leaves_the_original_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let fs_adapter = RealFs::new();
        let scope = scope_for(dir.path());
        let from_path = dir.path().join("old.md");
        let to_path = dir.path().join("new.md");
        fs::write(&from_path, b"payload").unwrap();
        let from = confine(&scope, &fs_adapter, &from_path).unwrap();
        let to = confine(&scope, &fs_adapter, &to_path).unwrap();

        let guard = crate::lease::FileLease::new(dir.path().join("leases"));
        let held = skill_studio_core::ports::acquire_exclusive(&guard, &scope).unwrap();

        fs_adapter.rename(&held, &from, &to).unwrap();

        assert!(
            !from_path.exists(),
            "rename must move the file, not leave the original in place"
        );
        assert_eq!(
            fs::read(&to_path).unwrap(),
            b"payload",
            "rename must move the file's real bytes to the new path, not return Ok without \
             moving anything"
        );
    }

    #[test]
    fn create_dir_all_makes_every_missing_ancestor_or_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let fs_adapter = RealFs::new();
        let scope = scope_for(dir.path());
        // `confine` canonicalizes the target's immediate parent, so that one
        // must already exist; `create_dir_all` is what creates the target
        // itself (and would create further missing levels beneath an
        // existing parent too - `fs::create_dir_all` handles that, this
        // adapter only forwards to it).
        fs::create_dir_all(dir.path().join("a")).unwrap();
        let nested = dir.path().join("a").join("b");
        let scoped = confine(&scope, &fs_adapter, &nested).unwrap();

        let guard = crate::lease::FileLease::new(dir.path().join("leases"));
        let held = skill_studio_core::ports::acquire_exclusive(&guard, &scope).unwrap();

        fs_adapter.create_dir_all(&held, &scoped).unwrap();

        assert!(
            nested.is_dir(),
            "create_dir_all must create every missing ancestor directory, not return Ok \
             without creating anything"
        );
    }

    #[test]
    fn symlink_creates_a_real_link_pointing_at_the_target_or_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let fs_adapter = RealFs::new();
        let scope = scope_for(dir.path());
        let target_path = dir.path().join("target.md");
        fs::write(&target_path, b"x").unwrap();
        let link_path = dir.path().join("link.md");
        let target = confine(&scope, &fs_adapter, &target_path).unwrap();
        let link = confine(&scope, &fs_adapter, &link_path).unwrap();

        let guard = crate::lease::FileLease::new(dir.path().join("leases"));
        let held = skill_studio_core::ports::acquire_exclusive(&guard, &scope).unwrap();

        fs_adapter.symlink(&held, &target, &link).unwrap();

        let read_back = fs::read_link(&link_path).unwrap();
        assert_eq!(
            read_back, target_path,
            "symlink must create a real link pointing at the target, not return Ok without \
             creating anything"
        );
    }
}
