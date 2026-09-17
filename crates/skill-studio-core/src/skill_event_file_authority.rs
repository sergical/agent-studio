//! Test-only file authority for the future isolated SQLite writer. No SQLite
//! connection may share a process with this probe's separately opened handles.
use crate::skill_event_files::{
    open_event_file, validate_event_file, EventFile, OpenFailure, OpenMode,
};
use crate::{skill_coordination::FinalizedWriteLease, skill_scope::SkillReadScope};
use cap_std::fs::Dir;
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};

pub(crate) struct EventFileAuthority<'lease, 'scope> {
    lease: &'lease FinalizedWriteLease<'scope>,
    scope: SkillReadScope,
    path: PathBuf,
    directory: Dir,
}
struct OpenedEventFile<'root, 'lease, 'scope> {
    root: &'root EventFileAuthority<'lease, 'scope>,
    role: EventFile,
    file: File,
}

impl<'lease, 'scope> EventFileAuthority<'lease, 'scope> {
    pub(crate) fn bind(
        lease: &'lease FinalizedWriteLease<'scope>,
        path: &Path,
    ) -> io::Result<Self> {
        lease.validate_state_tree(path).map_err(io::Error::other)?;
        let scope = SkillReadScope::bind(&[path.to_path_buf()]).map_err(io::Error::other)?;
        let directory = Dir::from_std_file(scope.clone_bound_directory(path)?.into());
        let root = Self {
            lease,
            scope,
            path: path.to_path_buf(),
            directory,
        };
        root.revalidate()?;
        Ok(root)
    }

    pub(crate) fn revalidate(&self) -> io::Result<()> {
        self.lease
            .validate_state_tree(&self.path)
            .map_err(io::Error::other)?;
        self.scope.revalidate_roots().map_err(io::Error::other)
    }

    fn open(
        &self,
        role: EventFile,
        mode: OpenMode,
    ) -> Result<OpenedEventFile<'_, 'lease, 'scope>, OpenFailure> {
        self.revalidate().map_err(OpenFailure::NotCreated)?;
        let failure = |error| match mode {
            OpenMode::Existing => OpenFailure::NotCreated(error),
            OpenMode::CreateNew => OpenFailure::CreationAttempted(error),
        };
        let file = open_event_file(&self.directory, role, mode)?;
        let opened = OpenedEventFile {
            root: self,
            role,
            file,
        };
        opened.revalidate().map_err(failure)?;
        Ok(opened)
    }
}

impl OpenedEventFile<'_, '_, '_> {
    pub(crate) fn revalidate(&self) -> io::Result<()> {
        self.root.revalidate()?;
        validate_event_file(&self.root.directory, self.role, &self.file)?;
        self.root.revalidate()
    }
}

#[test]
fn event_file_authority_confines_opens_and_detects_replacement() {
    use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
    use std::fs;
    use std::os::unix::fs::symlink;

    for role in [
        EventFile::Database,
        EventFile::Wal,
        EventFile::SharedMemory,
        EventFile::Journal,
    ] {
        for case in [
            "create",
            "existing",
            "symlink",
            "hardlink",
            "directory",
            "entry-replaced",
            "root-replaced",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(temp.path()).unwrap();
            let state = root.join("state");
            fs::create_dir(&state).unwrap();
            let outside = root.join("outside");
            fs::write(&outside, b"outside").unwrap();
            let path = state.join(role.name());
            match case {
                "symlink" => symlink(&outside, &path).unwrap(),
                "hardlink" => fs::hard_link(&outside, &path).unwrap(),
                "directory" => fs::create_dir(&path).unwrap(),
                "create" => {}
                _ => fs::write(&path, b"existing").unwrap(),
            }
            let scope = SkillReadScope::bind(std::slice::from_ref(&state)).unwrap();
            let lease = CoordinationPlan::new_fixture(
                vec![DirectoryEffect::tree(&state, CoordinationMode::Exclusive)],
                &root,
                None,
            )
            .unwrap()
            .acquire()
            .unwrap()
            .finalize_write(&scope, &[])
            .unwrap();
            let authority = EventFileAuthority::bind(&lease, &state).unwrap();
            let mode = if case == "create" {
                OpenMode::CreateNew
            } else {
                OpenMode::Existing
            };
            match authority.open(role, mode) {
                Ok(opened) => {
                    assert!(!matches!(case, "symlink" | "hardlink" | "directory"));
                    opened.revalidate().unwrap();
                    if case == "entry-replaced" {
                        fs::rename(&path, state.join("previous")).unwrap();
                        fs::write(&path, b"replacement").unwrap();
                        assert!(opened.revalidate().is_err());
                    } else if case == "root-replaced" {
                        fs::rename(&state, root.join("previous-state")).unwrap();
                        fs::create_dir(&state).unwrap();
                        assert!(opened.revalidate().is_err());
                        assert!(matches!(
                            authority.open(role, OpenMode::CreateNew),
                            Err(OpenFailure::NotCreated(_))
                        ));
                        assert_eq!(fs::read_dir(&state).unwrap().count(), 0);
                    } else {
                        let error = match authority.open(role, OpenMode::CreateNew) {
                            Err(error) => error,
                            Ok(_) => panic!("exclusive creation replaced an existing file"),
                        };
                        assert!(
                            matches!(error, OpenFailure::CreationAttempted(ref source) if source.kind() == io::ErrorKind::AlreadyExists)
                        );
                    }
                }
                Err(OpenFailure::NotCreated(error)) => {
                    assert!(
                        matches!(case, "symlink" | "hardlink" | "directory"),
                        "{error}"
                    );
                }
                Err(OpenFailure::CreationAttempted(error)) => {
                    panic!("unexpected creation failure: {error}")
                }
            }
            assert_eq!(fs::read(&outside).unwrap(), b"outside");
        }
    }
}

#[test]
fn event_file_authority_requires_exact_live_state_lease() {
    use crate::skill_coordination::{
        CancellationToken, CoordinationMode, CoordinationPlan, DirectoryEffect,
    };
    use std::fs;
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let state = root.join("state");
    let other = root.join("other");
    fs::create_dir(&state).unwrap();
    fs::create_dir(&other).unwrap();
    let scope = SkillReadScope::bind(&[state.clone(), other.clone()]).unwrap();
    let cancellation = CancellationToken::default();
    let lease = CoordinationPlan::new_cancellable(
        vec![DirectoryEffect::tree(&state, CoordinationMode::Exclusive)],
        None,
        cancellation.clone(),
    )
    .unwrap()
    .acquire()
    .unwrap()
    .finalize_write(&scope, &[])
    .unwrap();
    assert!(EventFileAuthority::bind(&lease, &other).is_err());
    let authority = EventFileAuthority::bind(&lease, &state).unwrap();
    cancellation.cancel();
    assert!(matches!(
        authority.open(EventFile::Database, OpenMode::CreateNew),
        Err(OpenFailure::NotCreated(_))
    ));
    assert_eq!(fs::read_dir(&state).unwrap().count(), 0);
    assert_eq!(fs::read_dir(&other).unwrap().count(), 0);
}

pub(crate) struct EventDirectoryTransfer<'root, 'lease, 'scope> {
    root: &'root EventFileAuthority<'lease, 'scope>,
    descriptor: std::os::fd::OwnedFd,
}
impl EventFileAuthority<'_, '_> {
    pub(crate) fn prepare_transfer(&self) -> io::Result<EventDirectoryTransfer<'_, '_, '_>> {
        self.revalidate()?;
        let descriptor = self.scope.clone_bound_directory(&self.path)?;
        self.revalidate()?;
        Ok(EventDirectoryTransfer {
            root: self,
            descriptor,
        })
    }
}
impl EventDirectoryTransfer<'_, '_, '_> {
    pub(crate) fn send(&self, socket: &std::os::unix::net::UnixStream) -> io::Result<()> {
        use std::os::fd::AsFd;
        self.root.revalidate()?;
        crate::skill_history_worker_bootstrap::send_directory_descriptor(
            socket,
            self.descriptor.as_fd(),
        )
    }
}

#[test]
#[ignore = "private subprocess fixture"]
fn event_directory_receiver_child() {
    use cap_std::fs::MetadataExt as _;
    use std::{
        io::{Read, Write},
        os::{fd::AsFd, unix::net::UnixStream},
    };
    let mut socket = UnixStream::from(std::io::stdin().as_fd().try_clone_to_owned().unwrap());
    let directory =
        crate::skill_history_worker_bootstrap::receive_history_directory_or_exit(&socket);
    let identity = directory.dir_metadata().unwrap();
    socket.write_all(&identity.dev().to_le_bytes()).unwrap();
    socket.write_all(&identity.ino().to_le_bytes()).unwrap();
    let mut stop = [0u8; 1];
    socket.read_exact(&mut stop).unwrap();
    match stop {
        [1] => {}
        [2] => {
            use std::os::unix::fs::FileExt;
            let file = match open_event_file(&directory, EventFile::Wal, OpenMode::CreateNew) {
                Ok(file) => file,
                Err(_) => {
                    socket.write_all(b"F").unwrap();
                    return;
                }
            };
            file.write_all_at(b"writer fixture", 0).unwrap();
            file.sync_all().unwrap();
            validate_event_file(&directory, EventFile::Wal, &file).unwrap();
            drop(file);
            directory
                .try_clone()
                .unwrap()
                .into_std_file()
                .sync_all()
                .unwrap();
            socket.write_all(b"D").unwrap();
        }
        _ => panic!("invalid private fixture action"),
    }
}

#[test]
fn event_directory_transfer_keeps_lease_in_parent_until_worker_exit() {
    use crate::{
        skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect},
        skill_history_worker_process::HistoryWorkerProcess,
    };
    use cap_std::fs::MetadataExt as _;
    use std::{
        fs,
        io::{Read, Write},
        process::Command,
        sync::atomic::AtomicBool,
        time::{Duration, Instant},
    };
    for case in ["write", "root-replaced", "wal-symlink", "wal-hardlink"] {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let state = root.join("state");
        fs::create_dir(&state).unwrap();
        let outside = root.join("outside");
        fs::write(&outside, b"outside").unwrap();
        let wal = state.join(EventFile::Wal.name());
        if case == "wal-symlink" {
            std::os::unix::fs::symlink(&outside, &wal).unwrap();
        } else if case == "wal-hardlink" {
            fs::hard_link(&outside, &wal).unwrap();
        }
        let scope = SkillReadScope::bind(std::slice::from_ref(&state)).unwrap();
        let lease = CoordinationPlan::new_fixture(
            vec![DirectoryEffect::tree(&state, CoordinationMode::Exclusive)],
            &root,
            None,
        )
        .unwrap()
        .acquire()
        .unwrap()
        .finalize_write(&scope, &[])
        .unwrap();
        {
            let authority = EventFileAuthority::bind(&lease, &state).unwrap();
            let transfer = authority.prepare_transfer().unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args([
                "--exact",
                "skill_event_file_authority::event_directory_receiver_child",
                "--ignored",
                "--nocapture",
            ]);
            let mut child = HistoryWorkerProcess::spawn(&mut command).unwrap();
            child
                .socket()
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            transfer.send(child.socket()).unwrap();
            let mut identity = [0u8; 16];
            child.socket().read_exact(&mut identity).unwrap();
            let metadata = authority.directory.dir_metadata().unwrap();
            assert_eq!(
                u64::from_le_bytes(identity[..8].try_into().unwrap()),
                metadata.dev()
            );
            assert_eq!(
                u64::from_le_bytes(identity[8..].try_into().unwrap()),
                metadata.ino()
            );
            assert!(CoordinationPlan::new_fixture(
                vec![DirectoryEffect::tree(&state, CoordinationMode::Exclusive)],
                &root,
                Some(Duration::from_millis(30)),
            )
            .unwrap()
            .acquire()
            .is_err());
            if case == "root-replaced" {
                fs::rename(&state, root.join("previous")).unwrap();
                fs::create_dir(&state).unwrap();
                assert!(transfer.send(child.socket()).is_err());
                assert_eq!(fs::read_dir(&state).unwrap().count(), 0);
            }
            if case == "root-replaced" {
                child.socket().write_all(&[1]).unwrap();
            } else {
                authority.revalidate().unwrap();
                child.socket().write_all(&[2]).unwrap();
                let mut receipt = [0u8; 1];
                child.socket().read_exact(&mut receipt).unwrap();
                assert_eq!(receipt, [if case == "write" { b'D' } else { b'F' }]);
            }
            child
                .wait_for_exit(
                    &AtomicBool::new(false),
                    Instant::now() + Duration::from_secs(3),
                )
                .unwrap();
            assert_eq!(fs::read(&outside).unwrap(), b"outside");
            match case {
                "write" => assert_eq!(fs::read(&wal).unwrap(), b"writer fixture"),
                "root-replaced" => {
                    assert_eq!(fs::read_dir(&state).unwrap().count(), 0);
                    assert_eq!(fs::read_dir(root.join("previous")).unwrap().count(), 0);
                }
                _ => assert_eq!(fs::read_dir(&state).unwrap().count(), 1),
            }
        }
        drop(lease);
        assert!(CoordinationPlan::new_fixture(
            vec![DirectoryEffect::tree(&state, CoordinationMode::Exclusive)],
            &root,
            Some(Duration::from_secs(1)),
        )
        .unwrap()
        .acquire()
        .is_ok());
    }
}
