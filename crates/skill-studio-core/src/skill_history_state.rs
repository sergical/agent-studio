//! Metadata-only history state observations. No database file is opened, and
//! presence is not permission to query SQLite or execute a stored inverse.
use crate::skill_scope::{ScopedReadError, SkillReadScope};
use cap_std::fs::{Metadata, MetadataExt};
use std::{
    ffi::OsStr,
    os::fd::{AsFd, BorrowedFd, OwnedFd},
    path::{Path, PathBuf},
};

const FILES: [&str; 4] = [
    "events.sqlite3",
    "events.sqlite3-wal",
    "events.sqlite3-shm",
    "events.sqlite3-journal",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryStateError {
    InvalidRoot,
    Unavailable,
    UnsupportedFile,
    OrphanedSidecars,
    Changed,
}
impl HistoryStateError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidRoot => "invalid_history_state_root",
            Self::Unavailable => "history_state_unavailable",
            Self::UnsupportedFile => "unsupported_history_state_file",
            Self::OrphanedSidecars => "orphaned_history_sidecars",
            Self::Changed => "history_state_changed",
        }
    }
}
impl std::fmt::Display for HistoryStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for HistoryStateError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryStoreState {
    Absent,
    Present {
        wal: bool,
        shared_memory: bool,
        journal: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileIdentity {
    device: u64,
    inode: u64,
    links: u64,
}
impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            links: metadata.nlink(),
        }
    }
}
/// Expected identities for the four fixed history files. This carries no path
/// authority: the worker must still open through its received directory.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryFileBinding {
    files: [Option<FileIdentity>; 4],
}
impl HistoryFileBinding {
    fn state(&self) -> HistoryStoreState {
        if self.files[0].is_none() {
            HistoryStoreState::Absent
        } else {
            HistoryStoreState::Present {
                wal: self.files[1].is_some(),
                shared_memory: self.files[2].is_some(),
                journal: self.files[3].is_some(),
            }
        }
    }

    /// Compare no-follow metadata through the received directory. This opens no
    /// database descriptors and must be followed by checks on actual opens.
    pub fn validate_directory(
        &self,
        directory: &cap_std::fs::Dir,
    ) -> Result<HistoryStoreState, HistoryStateError> {
        for (index, name) in FILES.iter().enumerate() {
            match directory.symlink_metadata(name) {
                Ok(metadata) => self.validate_opened(name, &metadata)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if self.files[index].is_some() {
                        return Err(HistoryStateError::Changed);
                    }
                }
                Err(_) => return Err(HistoryStateError::Unavailable),
            }
        }
        if self.files[0].is_none() && self.files[1..].iter().any(Option::is_some) {
            return Err(HistoryStateError::OrphanedSidecars);
        }
        Ok(self.state())
    }

    /// Inspect metadata from the opened descriptor, not another filename lookup.
    /// A successful comparison does not establish SQLite content consistency.
    pub fn validate_opened(
        &self,
        name: &str,
        metadata: &Metadata,
    ) -> Result<(), HistoryStateError> {
        let index = FILES
            .iter()
            .position(|candidate| *candidate == name)
            .ok_or(HistoryStateError::UnsupportedFile)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(HistoryStateError::UnsupportedFile);
        }
        if self.files[index] == Some(FileIdentity::from_metadata(metadata)) {
            Ok(())
        } else {
            Err(HistoryStateError::Changed)
        }
    }
}

#[derive(PartialEq, Eq)]
struct DirectoryStamp {
    identity: FileIdentity,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl DirectoryStamp {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            identity: FileIdentity::from_metadata(metadata),
            length: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

pub struct HistoryStateRoot {
    scope: SkillReadScope,
    root: PathBuf,
}
impl HistoryStateRoot {
    /// The explicit root must already exist. This never creates a state directory.
    pub fn bind(root: &Path) -> Result<Self, HistoryStateError> {
        if !root.is_absolute() {
            return Err(HistoryStateError::InvalidRoot);
        }
        let scope = SkillReadScope::bind(&[root.to_path_buf()])
            .map_err(|_| HistoryStateError::Unavailable)?;
        scope
            .revalidate_roots()
            .map_err(|_| HistoryStateError::Changed)?;
        Ok(Self {
            scope,
            root: root.to_path_buf(),
        })
    }

    /// Duplicate only the retained directory, never a database or sidecar file.
    pub fn prepare_directory_transfer(
        &self,
    ) -> Result<HistoryDirectoryTransfer<'_>, HistoryStateError> {
        self.scope
            .revalidate_roots()
            .map_err(|_| HistoryStateError::Changed)?;
        let descriptor = self
            .scope
            .clone_bound_directory(&self.root)
            .map_err(|_| HistoryStateError::Unavailable)?;
        let transfer = HistoryDirectoryTransfer {
            root: self,
            descriptor,
        };
        transfer.revalidate()?;
        Ok(transfer)
    }

    pub fn observe(&self) -> Result<HistoryStoreObservation<'_>, HistoryStateError> {
        let (files, directory) = self.observe_files()?;
        Ok(HistoryStoreObservation {
            root: self,
            files,
            directory,
        })
    }

    fn directory_stamp(&self) -> Result<DirectoryStamp, HistoryStateError> {
        self.scope
            .revalidate_roots()
            .map_err(|_| HistoryStateError::Changed)?;
        let (_, metadata) = self
            .scope
            .resolved_path_metadata(&self.root)
            .map_err(|_| HistoryStateError::Unavailable)?;
        Ok(DirectoryStamp::from_metadata(&metadata))
    }

    fn observe_files(
        &self,
    ) -> Result<([Option<FileIdentity>; 4], DirectoryStamp), HistoryStateError> {
        let before = self.directory_stamp()?;
        let mut files = [None; 4];
        for (index, name) in FILES.iter().enumerate() {
            files[index] = match self.scope.observe_entry(&self.root, OsStr::new(name)) {
                Ok(entry)
                    if entry.metadata.is_file() && !entry.metadata.file_type().is_symlink() =>
                {
                    Some(FileIdentity::from_metadata(&entry.metadata))
                }
                Ok(_) => return Err(HistoryStateError::UnsupportedFile),
                Err(ScopedReadError::Missing { .. }) => None,
                Err(_) => return Err(HistoryStateError::Unavailable),
            };
        }
        if self.directory_stamp()? != before {
            return Err(HistoryStateError::Changed);
        }
        if files[0].is_none() && files[1..].iter().any(Option::is_some) {
            return Err(HistoryStateError::OrphanedSidecars);
        }
        Ok((files, before))
    }
}

/// A descriptor prepared for a trusted worker transport. Borrowing keeps the
/// parent root alive. This is not a read-only OS descriptor capability: the
/// worker must restrict every operation performed relative to this directory.
pub struct HistoryDirectoryTransfer<'a> {
    root: &'a HistoryStateRoot,
    descriptor: OwnedFd,
}
impl AsFd for HistoryDirectoryTransfer<'_> {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.descriptor.as_fd()
    }
}
impl HistoryDirectoryTransfer<'_> {
    /// Recheck before transfer and retain parent-side validation at publication.
    /// Changes after this check cannot redirect the duplicated descriptor.
    pub fn revalidate(&self) -> Result<(), HistoryStateError> {
        self.root
            .scope
            .revalidate_roots()
            .map_err(|_| HistoryStateError::Changed)
    }
}

/// An observation belongs to its retained root and cannot be constructed from
/// a status enum or SQLite error code. It is not a database-content snapshot.
pub struct HistoryStoreObservation<'a> {
    root: &'a HistoryStateRoot,
    files: [Option<FileIdentity>; 4],
    directory: DirectoryStamp,
}
impl HistoryStoreObservation<'_> {
    pub fn file_binding(&self) -> HistoryFileBinding {
        HistoryFileBinding { files: self.files }
    }

    pub fn state(&self) -> HistoryStoreState {
        self.file_binding().state()
    }

    /// Validate immediately before acting on absence or publishing a read.
    /// Directory churn, including unrelated entries, invalidates the read.
    /// SQLite remains responsible for content consistency of present files.
    pub fn revalidate(&self) -> Result<(), HistoryStateError> {
        let (files, directory) = self.root.observe_files()?;
        if files == self.files && directory == self.directory {
            Ok(())
        } else {
            Err(HistoryStateError::Changed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    #[test]
    fn opened_descriptor_identity_rejects_replacement_after_names_are_restored() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(FILES[0]);
        fs::write(&path, "original").unwrap();
        let root = HistoryStateRoot::bind(temp.path()).unwrap();
        let binding = root.observe().unwrap().file_binding();
        let directory =
            cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        let original = directory.open(FILES[0]).unwrap();
        binding
            .validate_opened(FILES[0], &original.metadata().unwrap())
            .unwrap();
        fs::rename(&path, temp.path().join("held")).unwrap();
        fs::write(&path, "replacement").unwrap();
        let replacement = directory.open(FILES[0]).unwrap();
        fs::remove_file(&path).unwrap();
        fs::rename(temp.path().join("held"), &path).unwrap();
        assert_eq!(
            binding.validate_opened(FILES[0], &replacement.metadata().unwrap()),
            Err(HistoryStateError::Changed)
        );
        binding
            .validate_opened(FILES[0], &original.metadata().unwrap())
            .unwrap();
        assert_eq!(
            binding.validate_opened(FILES[1], &original.metadata().unwrap()),
            Err(HistoryStateError::Changed)
        );
        assert_eq!(
            binding.validate_opened("../events.sqlite3", &original.metadata().unwrap()),
            Err(HistoryStateError::UnsupportedFile)
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    }

    #[test]
    fn received_directory_must_match_expected_presence_and_identity() {
        let temp = tempfile::tempdir().unwrap();
        let root = HistoryStateRoot::bind(temp.path()).unwrap();
        let directory =
            cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        let absent = root.observe().unwrap().file_binding();
        absent.validate_directory(&directory).unwrap();
        for name in FILES {
            fs::write(temp.path().join(name), "fixture").unwrap();
            assert_eq!(
                absent.validate_directory(&directory),
                Err(HistoryStateError::Changed)
            );
            fs::remove_file(temp.path().join(name)).unwrap();
        }
        fs::write(temp.path().join(FILES[0]), "database").unwrap();
        fs::write(temp.path().join(FILES[1]), "wal").unwrap();
        let present = root.observe().unwrap().file_binding();
        present.validate_directory(&directory).unwrap();
        fs::write(temp.path().join(FILES[1]), "updated WAL contents").unwrap();
        present.validate_directory(&directory).unwrap();
        fs::rename(temp.path().join(FILES[1]), temp.path().join("held")).unwrap();
        assert_eq!(
            present.validate_directory(&directory),
            Err(HistoryStateError::Changed)
        );
        fs::write(temp.path().join(FILES[1]), "replacement WAL").unwrap();
        assert_eq!(
            present.validate_directory(&directory),
            Err(HistoryStateError::Changed)
        );
        fs::remove_file(temp.path().join(FILES[1])).unwrap();
        symlink("held", temp.path().join(FILES[1])).unwrap();
        assert_eq!(
            present.validate_directory(&directory),
            Err(HistoryStateError::UnsupportedFile)
        );
    }

    #[test]
    fn file_binding_wire_is_fixed_size_and_strict() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(FILES[0]), "fixture").unwrap();
        let root = HistoryStateRoot::bind(temp.path()).unwrap();
        let binding = root.observe().unwrap().file_binding();
        let wire = serde_json::to_value(&binding).unwrap();
        assert_eq!(
            serde_json::from_value::<HistoryFileBinding>(wire.clone()).unwrap(),
            binding
        );
        for change in 0..4 {
            let mut invalid = wire.clone();
            match change {
                0 => invalid["root"] = serde_json::json!("/tmp"),
                1 => invalid["files"][0]["path"] = serde_json::json!("/tmp"),
                2 => {
                    invalid["files"].as_array_mut().unwrap().pop();
                }
                3 => invalid["files"][0]["inode"] = serde_json::json!(-1),
                _ => unreachable!(),
            }
            assert!(serde_json::from_value::<HistoryFileBinding>(invalid).is_err());
        }
    }

    #[test]
    fn prepared_transfer_keeps_original_directory_after_replacement() {
        use std::os::unix::fs::MetadataExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("marker"), "original").unwrap();
        let original = fs::metadata(&path).unwrap();
        let root = HistoryStateRoot::bind(&path).unwrap();
        let transfer = root.prepare_directory_transfer().unwrap();
        let file = std::fs::File::from(transfer.as_fd().try_clone_to_owned().unwrap());
        assert_eq!(file.metadata().unwrap().ino(), original.ino());
        assert_eq!(file.metadata().unwrap().dev(), original.dev());
        let directory = cap_std::fs::Dir::from_std_file(file);
        fs::rename(&path, temp.path().join("old")).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(path.join("marker"), "replacement").unwrap();
        assert_eq!(directory.read_to_string("marker").unwrap(), "original");
        assert_eq!(transfer.revalidate(), Err(HistoryStateError::Changed));
        assert!(matches!(
            root.prepare_directory_transfer(),
            Err(HistoryStateError::Changed)
        ));
    }

    #[test]
    fn aliased_transfer_refuses_retargeting_and_closes_on_exec() {
        use std::os::fd::AsRawFd;
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let alias = temp.path().join("alias");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        symlink(&first, &alias).unwrap();
        let root = HistoryStateRoot::bind(&alias).unwrap();
        let transfer = root.prepare_directory_transfer().unwrap();
        // F_GETFD only inspects the live borrowed descriptor; it does not change it.
        let flags = unsafe { libc::fcntl(transfer.as_fd().as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0);
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
        transfer.revalidate().unwrap();
        fs::remove_file(&alias).unwrap();
        symlink(&second, &alias).unwrap();
        assert_eq!(transfer.revalidate(), Err(HistoryStateError::Changed));
        assert!(matches!(
            root.prepare_directory_transfer(),
            Err(HistoryStateError::Changed)
        ));
    }

    #[test]
    fn absence_requires_no_database_or_sidecars_and_is_revalidated() {
        let temp = tempfile::tempdir().unwrap();
        let root = HistoryStateRoot::bind(temp.path()).unwrap();
        let observation = root.observe().unwrap();
        assert_eq!(observation.state(), HistoryStoreState::Absent);
        observation.revalidate().unwrap();
        fs::write(temp.path().join(FILES[0]), "fixture").unwrap();
        assert_eq!(observation.revalidate(), Err(HistoryStateError::Changed));
        assert_eq!(
            root.observe().unwrap().state(),
            HistoryStoreState::Present {
                wal: false,
                shared_memory: false,
                journal: false
            }
        );
        fs::remove_file(temp.path().join(FILES[0])).unwrap();
        for name in &FILES[1..] {
            fs::write(temp.path().join(name), "orphan").unwrap();
            assert!(matches!(
                root.observe(),
                Err(HistoryStateError::OrphanedSidecars)
            ));
            fs::remove_file(temp.path().join(name)).unwrap();
        }
    }

    #[test]
    fn present_file_identity_is_separate_from_sqlite_content_consistency() {
        let temp = tempfile::tempdir().unwrap();
        for name in FILES {
            fs::write(temp.path().join(name), "before").unwrap();
        }
        let root = HistoryStateRoot::bind(temp.path()).unwrap();
        let observation = root.observe().unwrap();
        assert_eq!(
            observation.state(),
            HistoryStoreState::Present {
                wal: true,
                shared_memory: true,
                journal: true
            }
        );
        fs::write(temp.path().join(FILES[1]), "a longer WAL update").unwrap();
        observation.revalidate().unwrap();
        fs::rename(temp.path().join(FILES[0]), temp.path().join("old-database")).unwrap();
        fs::write(temp.path().join(FILES[0]), "before").unwrap();
        assert_eq!(observation.revalidate(), Err(HistoryStateError::Changed));
    }

    #[test]
    fn links_and_nonregular_entries_are_never_absence() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        fs::create_dir(&state).unwrap();
        let outside = temp.path().join("outside");
        fs::write(&outside, "private fixture").unwrap();
        let root = HistoryStateRoot::bind(&state).unwrap();
        for name in FILES {
            let path = state.join(name);
            symlink(&outside, &path).unwrap();
            assert!(matches!(
                root.observe(),
                Err(HistoryStateError::UnsupportedFile)
            ));
            fs::remove_file(&path).unwrap();
            fs::create_dir(&path).unwrap();
            assert!(matches!(
                root.observe(),
                Err(HistoryStateError::UnsupportedFile)
            ));
            fs::remove_dir(&path).unwrap();
        }
        assert_eq!(fs::read_to_string(outside).unwrap(), "private fixture");
    }

    #[test]
    fn root_replacement_and_alias_retargeting_invalidate_observations() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        let alias = temp.path().join("alias");
        symlink(&first, &alias).unwrap();
        let root = HistoryStateRoot::bind(&alias).unwrap();
        let observation = root.observe().unwrap();
        fs::remove_file(&alias).unwrap();
        symlink(&second, &alias).unwrap();
        assert_eq!(observation.revalidate(), Err(HistoryStateError::Changed));
        let direct = HistoryStateRoot::bind(&first).unwrap();
        let observation = direct.observe().unwrap();
        fs::rename(&first, temp.path().join("retained")).unwrap();
        fs::create_dir(&first).unwrap();
        assert_eq!(observation.revalidate(), Err(HistoryStateError::Changed));
    }

    #[test]
    fn invalid_or_missing_root_is_not_created_or_reported_empty() {
        assert!(matches!(
            HistoryStateRoot::bind(Path::new("relative")),
            Err(HistoryStateError::InvalidRoot)
        ));
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        assert!(matches!(
            HistoryStateRoot::bind(&missing),
            Err(HistoryStateError::Unavailable)
        ));
        assert!(!missing.exists());
    }
}
