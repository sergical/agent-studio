//! Moving a skill folder for park and unpark: a rename, or a verified copy
//! when the parked root is on another volume.

use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{CoreError, ErrorCode};
use crate::ports::{confine, FileKind, MutationSession, Runtime, ScopeFs};

/// Largest single file the cross-volume copy reads into memory.
const COPY_FILE_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// `EXDEV`: the same number on macOS and Linux.
const EXDEV: i32 = 18;

/// One entry under a folder being copied, relative to that folder.
#[derive(Debug)]
struct TreeEntry {
    relative: PathBuf,
    kind: FileKind,
    mode: Option<u32>,
}

/// True when `err` is a rename refused because source and destination are on
/// different volumes.
pub(crate) fn crosses_devices(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::CrossesDevices || err.raw_os_error() == Some(EXDEV)
}

/// Moves the folder `from` to `to`. A rename when both sit on one volume.
/// Across volumes: copies the folder, checks the copy (file count and a hash
/// of every file), then deletes the source. Any failure before the delete
/// removes the partial copy and leaves the source as it was.
pub(crate) fn move_dir(
    rt: &Runtime,
    session: &MutationSession,
    from: &Path,
    to: &Path,
) -> Result<(), CoreError> {
    let fs = rt.ports.fs.as_ref();
    let scoped_from = confine(&rt.scope, fs, from)?;
    let scoped_to = confine(&rt.scope, fs, to)?;
    match fs.rename(&session.guard, &scoped_from, &scoped_to) {
        Ok(()) => Ok(()),
        Err(e) if crosses_devices(&e) => copy_verify_remove(rt, session, from, to),
        Err(e) => Err(CoreError::io(from, e)),
    }
}

fn copy_verify_remove(
    rt: &Runtime,
    session: &MutationSession,
    from: &Path,
    to: &Path,
) -> Result<(), CoreError> {
    let fs = rt.ports.fs.as_ref();
    let tree = walk(fs, from).map_err(|e| CoreError::io(from, e))?;
    if let Some(entry) = tree
        .iter()
        .find(|e| !matches!(e.kind, FileKind::Dir | FileKind::File))
    {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "this folder holds a link or special file, so it cannot move to another disk",
        )
        .at(from.join(&entry.relative)));
    }

    let copied = copy_tree(rt, session, from, to, &tree)
        .and_then(|()| verify_copy(fs, from, to, &tree).map_err(|e| CoreError::io(to, e)));
    if let Err(e) = copied {
        // Best effort: whatever landed at `to` is this attempt's own partial
        // copy, because park refuses a destination that already exists.
        let _ = remove_tree(rt, session, to);
        return Err(e);
    }

    remove_tree(rt, session, from).map_err(|e| {
        CoreError::new(
            ErrorCode::Io,
            format!(
                "the copy on the other disk is complete, but the original could not be removed: {e}"
            ),
        )
        .at(from)
    })
}

/// Every entry under `root`, directories before their contents.
fn walk(fs: &dyn ScopeFs, root: &Path) -> io::Result<Vec<TreeEntry>> {
    let mut out = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative_dir) = pending.pop() {
        let mut entries = fs.read_dir(&root.join(&relative_dir))?;
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        for entry in entries {
            let relative = relative_dir.join(&entry.name);
            let facts = fs.symlink_metadata(&root.join(&relative))?;
            if entry.kind == FileKind::Dir {
                pending.push(relative.clone());
            }
            out.push(TreeEntry {
                relative,
                kind: entry.kind,
                mode: facts.mode,
            });
        }
    }
    out.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(out)
}

fn copy_tree(
    rt: &Runtime,
    session: &MutationSession,
    from: &Path,
    to: &Path,
    tree: &[TreeEntry],
) -> Result<(), CoreError> {
    let fs = rt.ports.fs.as_ref();
    let scoped_root = confine(&rt.scope, fs, to)?;
    fs.create_dir_all(&session.guard, &scoped_root)
        .map_err(|e| CoreError::io(to, e))?;
    for entry in tree {
        let source = from.join(&entry.relative);
        let dest = to.join(&entry.relative);
        // Confine each destination before the unscoped write below.
        let scoped = confine(&rt.scope, fs, &dest)?;
        if entry.kind == FileKind::Dir {
            fs.create_dir_all(&session.guard, &scoped)
                .map_err(|e| CoreError::io(&dest, e))?;
        } else {
            let bytes = fs
                .read_capped(&source, COPY_FILE_MAX_BYTES)
                .map_err(|e| CoreError::io(&source, e))?;
            let written = match entry.mode {
                Some(mode) => fs.fsops_write_new_file_with_mode(&dest, &bytes, mode & 0o7777),
                None => fs.fsops_write_new_file(&dest, &bytes),
            };
            written.map_err(|e| CoreError::io(&dest, e))?;
        }
    }
    Ok(())
}

/// Fails unless `to` holds the same entries as `tree`, and every file hashes
/// the same as its source.
fn verify_copy(fs: &dyn ScopeFs, from: &Path, to: &Path, tree: &[TreeEntry]) -> io::Result<()> {
    let copied = walk(fs, to)?;
    let same_shape = copied.len() == tree.len()
        && copied
            .iter()
            .zip(tree)
            .all(|(a, b)| a.relative == b.relative && a.kind == b.kind);
    if !same_shape {
        return Err(io::Error::other("the copy does not list the same files"));
    }
    for entry in tree.iter().filter(|e| e.kind == FileKind::File) {
        let hash = |root: &Path| -> io::Result<Vec<u8>> {
            let bytes = fs.read_capped(&root.join(&entry.relative), COPY_FILE_MAX_BYTES)?;
            Ok(Sha256::digest(bytes).to_vec())
        };
        if hash(from)? != hash(to)? {
            return Err(io::Error::other(format!(
                "{} differs in the copy",
                entry.relative.display()
            )));
        }
    }
    Ok(())
}

/// Deletes `root` and everything under it (files and directories only).
fn remove_tree(rt: &Runtime, session: &MutationSession, root: &Path) -> Result<(), CoreError> {
    let fs = rt.ports.fs.as_ref();
    let tree = walk(fs, root).map_err(|e| CoreError::io(root, e))?;
    for entry in tree.iter().rev() {
        let path = root.join(&entry.relative);
        if entry.kind == FileKind::Dir {
            confine(&rt.scope, fs, &path)?;
            fs.fsops_remove_dir(&path)
                .map_err(|e| CoreError::io(&path, e))?;
        } else {
            let scoped = confine(&rt.scope, fs, &path)?;
            fs.remove_file(&session.guard, &scoped)
                .map_err(|e| CoreError::io(&path, e))?;
        }
    }
    confine(&rt.scope, fs, root)?;
    fs.fsops_remove_dir(root)
        .map_err(|e| CoreError::io(root, e))
}
