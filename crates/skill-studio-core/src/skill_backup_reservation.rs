//! Destination capabilities for new backup operations. Source authorization,
//! complete mutation effects and recovery remain responsibilities of the service.
pub use crate::skill_backup_copy::{BackupCopyLimits, BackupCopyReport};
use crate::skill_scope::SkillReadScope;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, Metadata, MetadataExt, OpenOptions, OpenOptionsExt};
use std::{
    io,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub struct BackupStateRoot {
    pub(crate) path: std::path::PathBuf,
    scope: SkillReadScope,
    directory: Dir,
}

pub struct ReservedBackup<'root> {
    root: &'root BackupStateRoot,
    container: Dir,
    directory: Dir,
    id: String,
}

/// Existing snapshot access exposes inspection only, not reservation or publication.
pub struct ExistingBackup<'root> {
    binding: ReservedBackup<'root>,
}

impl ExistingBackup<'_> {
    pub fn revalidate(&self) -> io::Result<()> {
        self.binding.revalidate()
    }

    pub(crate) fn discard(self) -> io::Result<()> {
        self.binding.discard()
    }

    pub fn verify_entry(
        &self,
        name: &std::ffi::OsStr,
        expected_identity: &str,
        limits: BackupCopyLimits,
        cancellation: &crate::skill_coordination::CancellationToken,
    ) -> io::Result<BackupCopyReport> {
        self.binding
            .verify_entry(name, expected_identity, limits, cancellation)
    }

    pub(crate) fn verify_file(&self, name: &str, expected: &[u8]) -> io::Result<()> {
        self.verify_file_with(name, expected, || {})
    }

    fn verify_file_with(
        &self,
        name: &str,
        expected: &[u8],
        after_read: impl FnOnce(),
    ) -> io::Result<()> {
        if !crate::skill_backup_copy::valid_component(std::ffi::OsStr::new(name)) {
            return Err(io::Error::other("Invalid evidence file name"));
        }
        self.revalidate()?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        options.custom_flags(libc::O_NONBLOCK);
        let mut file = self.binding.directory.open_with(name, &options)?;
        let before = file.metadata()?;
        if !before.is_file() {
            return Err(io::Error::other("Evidence is not a regular file"));
        }
        let mut actual = Vec::new();
        (&mut file)
            .take(expected.len() as u64 + 1)
            .read_to_end(&mut actual)?;
        after_read();
        if actual != expected
            || !crate::skill_backup_copy::unchanged(&before, &file.metadata()?)
            || !crate::skill_backup_copy::unchanged(
                &before,
                &self.binding.directory.symlink_metadata(name)?,
            )
        {
            return Err(io::Error::other("Saved Fork document evidence changed"));
        }
        self.revalidate()
    }

    pub(crate) fn copy_verified_tree_to(
        &self,
        name: &std::ffi::OsStr,
        expected_identity: &str,
        destination: &BackupStateRoot,
        target: &std::ffi::OsStr,
        limits: BackupCopyLimits,
        cancellation: &crate::skill_coordination::CancellationToken,
    ) -> io::Result<BackupCopyReport> {
        let protected = self.binding.root.resolved_path()?.join("backups");
        let output = destination.resolved_path()?;
        if output.starts_with(&protected) || protected.starts_with(&output) {
            return Err(io::Error::other(
                "Snapshot export destination overlaps immutable backups",
            ));
        }
        self.revalidate()?;
        if !self.binding.directory.symlink_metadata(name)?.is_dir() {
            return Err(io::Error::other(
                "Snapshot export source must be a directory",
            ));
        }
        self.verify_entry(name, expected_identity, limits, cancellation)?;
        let report = crate::skill_backup_copy::copy_entry(
            &self.binding.directory,
            name,
            &destination.directory,
            target,
            limits,
            cancellation,
        )?;
        if report.tree_identity != expected_identity {
            return Err(io::Error::other(
                "Snapshot changed while creating its working base",
            ));
        }
        let saved = crate::skill_backup_copy::inspect_entry(
            &destination.directory,
            target,
            limits,
            cancellation,
        )?;
        if saved.tree_identity != expected_identity {
            return Err(io::Error::other(
                "Working base differs from its immutable snapshot",
            ));
        }
        self.verify_entry(name, expected_identity, limits, cancellation)?;
        destination
            .scope
            .revalidate_roots()
            .map_err(io::Error::other)?;
        sync_directory(&destination.directory)?;
        Ok(report)
    }
}

fn changed() -> io::Error {
    io::Error::other("Backup directory binding changed")
}
fn same_directory(left: &Metadata, right: &Metadata) -> bool {
    left.is_dir()
        && !left.file_type().is_symlink()
        && left.dev() == right.dev()
        && left.ino() == right.ino()
}
fn sync_directory(directory: &Dir) -> io::Result<()> {
    directory.open(".")?.sync_all()
}
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

impl BackupStateRoot {
    #[cfg(feature = "event-store")]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn publish_verified_fork_base(
        &self,
        lease: &crate::skill_coordination::FinalizedWriteLease<'_>,
        name: &str,
        event_id: &str,
        upstream_identity: &str,
        previous_identity: Option<&str>,
        limits: BackupCopyLimits,
        cancellation: &crate::skill_coordination::CancellationToken,
        validate_intent: impl Fn() -> Result<(), String>,
        mut after_publish: impl FnMut() -> Result<(), String>,
    ) -> Result<PathBuf, String> {
        use std::ffi::OsStr;

        lease.validate_state_tree(&self.path)?;
        validate_intent()?;
        if !crate::skill_backup_copy::valid_component(OsStr::new(name)) {
            return Err("Invalid Fork base cache name".into());
        }
        let backup = self
            .open_existing(event_id)
            .map_err(|error| error.to_string())?;
        backup
            .verify_entry(
                OsStr::new("upstream"),
                upstream_identity,
                limits,
                cancellation,
            )
            .map_err(|error| error.to_string())?;
        let mut directory = self
            .directory
            .try_clone()
            .map_err(|error| error.to_string())?;
        let mut path = self.path.clone();
        for child in ["skill-studio", "forks", name] {
            match directory.create_dir(child) {
                Ok(()) => sync_directory(&directory).map_err(|error| error.to_string())?,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.to_string()),
            }
            directory = directory
                .open_dir_nofollow(child)
                .map_err(|error| error.to_string())?;
            path.push(child);
        }
        let cache = BackupStateRoot::bind(&path).map_err(|error| error.to_string())?;
        if !same_directory(
            &directory
                .dir_metadata()
                .map_err(|error| error.to_string())?,
            &cache
                .directory
                .dir_metadata()
                .map_err(|error| error.to_string())?,
        ) {
            return Err("Fork cache directory changed during binding".into());
        }
        let inspect = |child: &str| -> Result<Option<String>, String> {
            match directory.symlink_metadata(child) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.to_string()),
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    crate::skill_backup_copy::inspect_entry(
                        &directory,
                        OsStr::new(child),
                        limits,
                        cancellation,
                    )
                    .map(|report| Some(report.tree_identity))
                    .map_err(|error| error.to_string())
                }
                Ok(_) => Err(format!(
                    "Fork cache entry {child} is not an independent directory"
                )),
            }
        };
        let verify = || -> Result<(), String> {
            lease.revalidate().map_err(|error| error.to_string())?;
            cache
                .scope
                .revalidate_roots()
                .map_err(|error| error.to_string())?;
            validate_intent()?;
            let report = crate::skill_backup_copy::inspect_entry(
                &directory,
                OsStr::new("base"),
                limits,
                cancellation,
            )
            .map_err(|error| error.to_string())?;
            if report.tree_identity != upstream_identity {
                return Err(
                    "Existing Fork merge base differs from the verified upstream snapshot".into(),
                );
            }
            lease.revalidate().map_err(|error| error.to_string())?;
            validate_intent()
        };
        let journal_name = format!(".previous-{event_id}");
        let stage_name = format!(".base-{event_id}");
        let base = inspect("base")?;
        let previous = inspect(&journal_name)?;
        if base.as_deref() == Some(upstream_identity) {
            if previous_identity.is_some_and(|expected| {
                expected != upstream_identity && previous.as_deref() != Some(expected)
            }) {
                return Err("Published Fork base has no matching prior-base journal".into());
            }
            if directory.symlink_metadata(&stage_name).is_ok() {
                let stage = directory
                    .open_dir_nofollow(&stage_name)
                    .map_err(|error| error.to_string())?;
                if stage
                    .entries()
                    .map_err(|error| error.to_string())?
                    .next()
                    .is_some()
                {
                    return Err("Published Fork base has a non-empty staging directory".into());
                }
                directory
                    .remove_dir(&stage_name)
                    .map_err(|error| error.to_string())?;
                sync_directory(&directory).map_err(|error| error.to_string())?;
            }
            verify()?;
            return Ok(path.join("base"));
        }
        match previous_identity {
            Some(expected) => match previous.as_deref() {
                Some(actual) if actual == expected => {
                    if base.is_some() {
                        return Err("Fork base changed after its prior value was journaled".into());
                    }
                }
                Some(_) => return Err("Prior Fork base journal changed".into()),
                None if base.as_deref() == Some(expected) => {
                    validate_intent()?;
                    #[cfg(any(
                        target_vendor = "apple",
                        target_os = "linux",
                        target_os = "android"
                    ))]
                    rustix::fs::renameat_with(
                        &directory,
                        "base",
                        &directory,
                        &journal_name,
                        rustix::fs::RenameFlags::NOREPLACE,
                    )
                    .map_err(|error| error.to_string())?;
                    #[cfg(not(any(
                        target_vendor = "apple",
                        target_os = "linux",
                        target_os = "android"
                    )))]
                    return Err(
                        "Atomic prior-base journaling is unsupported on this platform".into(),
                    );
                    sync_directory(&directory).map_err(|error| error.to_string())?;
                }
                None => return Err("Admitted prior Fork base is missing or changed".into()),
            },
            None if base.is_some() || previous.is_some() => {
                return Err("Unexpected Fork base appeared after admission".into())
            }
            None => {}
        }
        if directory.symlink_metadata(&stage_name).is_ok() {
            let reusable = directory
                .open_dir_nofollow(&stage_name)
                .ok()
                .and_then(|stage| {
                    crate::skill_backup_copy::inspect_entry(
                        &stage,
                        OsStr::new("base"),
                        limits,
                        cancellation,
                    )
                    .ok()
                })
                .is_some_and(|report| report.tree_identity == upstream_identity);
            if !reusable {
                directory
                    .remove_dir_all(&stage_name)
                    .map_err(|error| error.to_string())?;
                sync_directory(&directory).map_err(|error| error.to_string())?;
            }
        }
        if matches!(directory.symlink_metadata(&stage_name), Err(error) if error.kind() == io::ErrorKind::NotFound)
        {
            directory
                .create_dir(&stage_name)
                .map_err(|error| error.to_string())?;
            sync_directory(&directory).map_err(|error| error.to_string())?;
        }
        let stage_directory = directory
            .open_dir_nofollow(&stage_name)
            .map_err(|error| error.to_string())?;
        let stage =
            BackupStateRoot::bind(&path.join(&stage_name)).map_err(|error| error.to_string())?;
        if !same_directory(
            &stage_directory
                .dir_metadata()
                .map_err(|error| error.to_string())?,
            &stage
                .directory
                .dir_metadata()
                .map_err(|error| error.to_string())?,
        ) {
            return Err("Fork base staging changed during binding".into());
        }
        if matches!(stage.directory.symlink_metadata("base"), Err(error) if error.kind() == io::ErrorKind::NotFound)
        {
            backup
                .copy_verified_tree_to(
                    OsStr::new("upstream"),
                    upstream_identity,
                    &stage,
                    OsStr::new("base"),
                    limits,
                    cancellation,
                )
                .map_err(|error| error.to_string())?;
        }
        lease.revalidate().map_err(|error| error.to_string())?;
        stage
            .scope
            .revalidate_roots()
            .map_err(|error| error.to_string())?;
        validate_intent()?;
        #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
        rustix::fs::renameat_with(
            &stage.directory,
            "base",
            &directory,
            "base",
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(|error| error.to_string())?;
        #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
        return Err("Atomic merge-base publication is unsupported on this platform".into());
        sync_directory(&directory).map_err(|error| error.to_string())?;
        verify()?;
        after_publish()?;
        stage
            .scope
            .revalidate_roots()
            .map_err(|error| error.to_string())?;
        directory
            .remove_dir(&stage_name)
            .map_err(|error| error.to_string())?;
        sync_directory(&directory).map_err(|error| error.to_string())?;
        Ok(path.join("base"))
    }

    /// Bind an explicit existing root. No ambient HOME or cwd is consulted.
    pub fn bind(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Backup root must be absolute",
            ));
        }
        let scope = SkillReadScope::bind(&[path.to_path_buf()]).map_err(io::Error::other)?;
        let directory = scope
            .clone_bound_directory(path)
            .map_err(io::Error::other)?;
        Ok(Self {
            scope,
            directory: directory.into(),
            path: path.to_path_buf(),
        })
    }

    fn resolved_path(&self) -> io::Result<PathBuf> {
        self.scope
            .resolved_dir_path(&self.path)
            .map_err(io::Error::other)
    }

    pub fn open_existing(&self, id: &str) -> io::Result<ExistingBackup<'_>> {
        if !valid_id(id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid backup operation ID",
            ));
        }
        self.scope.revalidate_roots().map_err(|_| changed())?;
        let container = self.directory.open_dir_nofollow("backups")?;
        let directory = container.open_dir_nofollow(id)?;
        let binding = ReservedBackup {
            root: self,
            container,
            directory,
            id: id.into(),
        };
        binding.revalidate()?;
        Ok(ExistingBackup { binding })
    }

    pub fn reserve(&self, id: &str) -> io::Result<ReservedBackup<'_>> {
        if !valid_id(id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid backup operation ID",
            ));
        }
        self.scope.revalidate_roots().map_err(|_| changed())?;
        match self.directory.create_dir("backups") {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let container = self.directory.open_dir_nofollow("backups")?;
        container.create_dir(id)?;
        let directory = container.open_dir_nofollow(id)?;
        let result = ReservedBackup {
            root: self,
            container,
            directory,
            id: id.into(),
        };
        result.revalidate()?;
        sync_directory(&result.directory)?;
        sync_directory(&result.container)?;
        sync_directory(&self.directory)?;
        result.revalidate()?;
        Ok(result)
    }
}

impl ReservedBackup<'_> {
    pub fn revalidate(&self) -> io::Result<()> {
        self.root.scope.revalidate_roots().map_err(|_| changed())?;
        if !same_directory(
            &self.root.directory.symlink_metadata("backups")?,
            &self.container.dir_metadata()?,
        ) || !same_directory(
            &self.container.symlink_metadata(&self.id)?,
            &self.directory.dir_metadata()?,
        ) {
            return Err(changed());
        }
        Ok(())
    }

    pub(crate) fn discard(self) -> io::Result<()> {
        self.revalidate()?;
        self.container.remove_dir_all(&self.id)?;
        sync_directory(&self.container)?;
        sync_directory(&self.root.directory)?;
        self.root.scope.revalidate_roots().map_err(|_| changed())
    }

    /// Copy from an explicitly supplied source directory capability. Limits and
    /// cancellation bound cooperative work; failure may retain partial artifacts.
    pub fn copy_entry(
        &self,
        source: &Dir,
        name: &std::ffi::OsStr,
        target: &std::ffi::OsStr,
        limits: BackupCopyLimits,
        cancellation: &crate::skill_coordination::CancellationToken,
    ) -> io::Result<BackupCopyReport> {
        self.revalidate()?;
        let report = crate::skill_backup_copy::copy_entry(
            source,
            name,
            &self.directory,
            target,
            limits,
            cancellation,
        )?;
        self.revalidate()?;
        Ok(report)
    }

    /// Check saved content through the retained backup directory without creating a copy.
    pub fn verify_entry(
        &self,
        name: &std::ffi::OsStr,
        expected_identity: &str,
        limits: BackupCopyLimits,
        cancellation: &crate::skill_coordination::CancellationToken,
    ) -> io::Result<BackupCopyReport> {
        self.revalidate()?;
        let report =
            crate::skill_backup_copy::inspect_entry(&self.directory, name, limits, cancellation)?;
        self.revalidate()?;
        if report.tree_identity != expected_identity {
            return Err(io::Error::other("Backup tree identity changed"));
        }
        Ok(report)
    }

    /// Create one new direct child and sync its bytes and directory entry.
    /// Existing files and symlinks are refused. Failure may leave a partial file.
    pub fn write_new_file(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        self.write_new_file_with(name, bytes, || {})
    }

    fn write_new_file_with(
        &self,
        name: &str,
        bytes: &[u8],
        before_open: impl FnOnce(),
    ) -> io::Result<()> {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.len() > 255
            || name.contains(['/', '\\', '\0'])
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Backup file name must be one component",
            ));
        }
        self.revalidate()?;
        before_open();
        let mut file = self
            .directory
            .open_with(name, OpenOptions::new().write(true).create_new(true))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        sync_directory(&self.directory)?;
        self.revalidate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn evidence_read_refuses_replacement_and_in_place_changes() {
        for replace in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let root = BackupStateRoot::bind(temp.path()).unwrap();
            let backup = root.reserve("evidence").unwrap();
            backup.write_new_file("document", b"expected").unwrap();
            drop(backup);
            let backup = root.open_existing("evidence").unwrap();
            let path = temp.path().join("backups/evidence/document");
            assert!(backup
                .verify_file_with("document", b"expected", || {
                    if replace {
                        let replacement = temp.path().join("replacement");
                        fs::write(&replacement, b"changed").unwrap();
                        fs::rename(replacement, &path).unwrap();
                    } else {
                        fs::write(&path, b"changed").unwrap();
                    }
                })
                .is_err());
            assert_eq!(fs::read(path).unwrap(), b"changed");
        }
    }

    #[test]
    fn reopens_saved_tree_without_creating_missing_operations() {
        use crate::skill_coordination::CancellationToken;
        use std::ffi::OsStr;
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("document"), "saved content").unwrap();
        let input = Dir::open_ambient_dir(&source, cap_std::ambient_authority()).unwrap();
        let state = temp.path().join("state");
        fs::create_dir(&state).unwrap();
        let limits = BackupCopyLimits {
            max_bytes: 1024,
            max_entries: 10,
            max_depth: 5,
        };
        let report = {
            let root = BackupStateRoot::bind(&state).unwrap();
            assert!(root.open_existing("missing").is_err());
            assert!(!state.join("backups").exists());
            let operation = root.reserve("saved").unwrap();
            operation
                .copy_entry(
                    &input,
                    OsStr::new("document"),
                    OsStr::new("snapshot"),
                    limits,
                    &CancellationToken::default(),
                )
                .unwrap()
        };
        let root = BackupStateRoot::bind(&state).unwrap();
        for invalid in ["", "..", "../saved", "/saved", "missing"] {
            assert!(root.open_existing(invalid).is_err());
        }
        assert_eq!(fs::read_dir(state.join("backups")).unwrap().count(), 1);
        let existing = root.open_existing("saved").unwrap();
        assert_eq!(
            existing
                .verify_entry(
                    OsStr::new("snapshot"),
                    &report.tree_identity,
                    limits,
                    &CancellationToken::default()
                )
                .unwrap(),
            report
        );
        fs::write(state.join("backups/saved/snapshot"), "changed content").unwrap();
        assert!(existing
            .verify_entry(
                OsStr::new("snapshot"),
                &report.tree_identity,
                limits,
                &CancellationToken::default()
            )
            .is_err());
    }

    #[test]
    fn reopened_backup_refuses_links_and_replaced_bindings() {
        for level in ["state", "backups", "operation"] {
            let temp = tempfile::tempdir().unwrap();
            let state = temp.path().join("state");
            fs::create_dir_all(state.join("backups/saved")).unwrap();
            let root = BackupStateRoot::bind(&state).unwrap();
            let existing = root.open_existing("saved").unwrap();
            let path = match level {
                "state" => state.clone(),
                "backups" => state.join("backups"),
                _ => state.join("backups/saved"),
            };
            let moved = temp.path().join("moved");
            fs::rename(&path, &moved).unwrap();
            std::os::unix::fs::symlink(&moved, &path).unwrap();
            assert!(existing.revalidate().is_err(), "{level}");
            assert!(root.open_existing("saved").is_err(), "{level}");
            fs::remove_file(&path).unwrap();
            fs::create_dir(&path).unwrap();
            assert!(existing.revalidate().is_err(), "{level}");
            assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
        }
    }

    #[test]
    fn writes_new_files_and_refuses_existing_or_escaping_names() {
        let temp = tempfile::tempdir().unwrap();
        let root = BackupStateRoot::bind(temp.path()).unwrap();
        for id in ["", "..", "../outside", "/outside"] {
            assert!(root.reserve(id).is_err());
        }
        assert!(!temp.path().join("backups").exists());
        let operation = root.reserve("fixture").unwrap();
        operation.write_new_file("document", b"saved").unwrap();
        assert!(operation.write_new_file("document", b"overwrite").is_err());
        for name in ["", "..", "../outside", "/outside", "nested/file"] {
            assert!(operation.write_new_file(name, b"escape").is_err());
        }
        assert_eq!(
            fs::read(temp.path().join("backups/fixture/document")).unwrap(),
            b"saved"
        );
        assert!(root.reserve("fixture").is_err());
    }

    #[test]
    fn directory_swap_during_write_cannot_redirect_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let root = BackupStateRoot::bind(temp.path()).unwrap();
        let operation = root.reserve("fixture").unwrap();
        let path = temp.path().join("backups/fixture");
        let retained = temp.path().join("backups/retained");
        let result = operation.write_new_file_with("document", b"saved", || {
            fs::rename(&path, &retained).unwrap();
            fs::create_dir(&path).unwrap();
        });
        assert!(result.is_err());
        assert!(!path.join("document").exists());
        assert_eq!(fs::read(retained.join("document")).unwrap(), b"saved");
    }

    #[test]
    fn outside_links_and_replaced_state_root_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        fs::create_dir(&state).unwrap();
        let root = BackupStateRoot::bind(&state).unwrap();
        std::os::unix::fs::symlink(outside.path(), state.join("backups")).unwrap();
        assert!(root.reserve("fixture").is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        fs::remove_file(state.join("backups")).unwrap();
        let operation = root.reserve("fixture").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("target"),
            state.join("backups/fixture/document"),
        )
        .unwrap();
        assert!(operation.write_new_file("document", b"escape").is_err());
        assert!(!outside.path().join("target").exists());
        fs::rename(&state, temp.path().join("retained-state")).unwrap();
        fs::create_dir(&state).unwrap();
        assert!(operation.write_new_file("another", b"escape").is_err());
        assert_eq!(fs::read_dir(&state).unwrap().count(), 0);
    }
}
