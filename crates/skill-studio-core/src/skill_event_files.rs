//! Directory-relative SQLite file operations for a dedicated event worker.
//!
//! The caller supplies retained directory authority. Only the four event database
//! names are accepted. These handles must not be separately opened and closed in
//! a process with a live SQLite connection: doing so can release POSIX locks.
//! Native SQLite hooks use these operations as SQLite's own file opens.
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, OpenOptions};
use std::{fs::File, io, os::unix::fs::MetadataExt};

#[derive(Clone, Copy)]
pub enum EventFile {
    Database,
    Wal,
    SharedMemory,
    Journal,
}
impl EventFile {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Database => "events.sqlite3",
            Self::Wal => "events.sqlite3-wal",
            Self::SharedMemory => "events.sqlite3-shm",
            Self::Journal => "events.sqlite3-journal",
        }
    }
}

#[derive(Clone, Copy)]
pub enum OpenMode {
    Existing,
    CreateNew,
}

#[derive(Debug)]
pub enum OpenFailure {
    NotCreated(io::Error),
    CreationAttempted(io::Error),
}

pub fn open_event_file(
    directory: &Dir,
    role: EventFile,
    mode: OpenMode,
) -> Result<File, OpenFailure> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .follow(FollowSymlinks::No)
        .nonblock(true);
    if matches!(mode, OpenMode::CreateNew) {
        options.create_new(true);
    }
    let failure = |error| match mode {
        OpenMode::Existing => OpenFailure::NotCreated(error),
        OpenMode::CreateNew => OpenFailure::CreationAttempted(error),
    };
    let file = directory
        .open_with(role.name(), &options)
        .map_err(failure)?
        .into_std();
    validate_event_file(directory, role, &file).map_err(failure)?;
    Ok(file)
}

pub(crate) fn validate_event_file(directory: &Dir, role: EventFile, file: &File) -> io::Result<()> {
    use cap_std::fs::MetadataExt as _;
    let opened = file.metadata()?;
    let entry = directory.symlink_metadata(role.name())?;
    if !opened.is_file()
        || opened.nlink() != 1
        || !entry.is_file()
        || entry.nlink() != 1
        || entry.dev() != opened.dev()
        || entry.ino() != opened.ino()
    {
        return Err(io::Error::other(
            "Event file identity changed or is not a single-link regular file",
        ));
    }
    Ok(())
}
