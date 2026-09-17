//! fsops: five primitives for durable, confined writes under one root
//! folder - [`Root`], [`stage`], [`swap`], [`link`], and [`write_file`].
//! See `docs/action-map/primitives-and-call-stack.md` for the guarantee
//! each one makes and `docs/action-map/plan.md` unit 1.1 for the tests
//! that hold them to it.
//!
//! Nothing in [`crate::ops`] calls these yet; adoption starts in a later
//! unit. Every primitive is written against the `fsops_*` methods on
//! [`crate::ports::ScopeFs`], so this module never touches the real
//! filesystem - `skill_studio_host::fs::RealFs` and
//! `crate::testing::FixtureFs` are the two implementations, real disk and
//! in-memory.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ports::{FileKind, ScopeFs};

/// What went wrong running an fsops primitive.
#[derive(Debug, thiserror::Error)]
pub enum FsOpsError {
    /// `name` would resolve outside the root: absolute, a `..` segment, or
    /// a symlink among its existing ancestors whose target lies outside.
    #[error("{name}: escapes the root ({reason})")]
    Escapes {
        /// The name the caller asked for.
        name: PathBuf,
        /// Which check refused it.
        reason: &'static str,
    },
    /// The root's device or inode changed since [`Root::open`] read it, or
    /// since the last step revalidated it.
    #[error("{root}: the root was replaced during the operation")]
    RootMoved {
        /// The root's path.
        root: PathBuf,
    },
    /// [`swap`] found its target already replaced by something other than
    /// the directory it expected to exchange.
    #[error("{path}: was a directory, is now something else; refusing to swap")]
    ReplacedBySymlink {
        /// The path that changed kind between `stage` and `swap`.
        path: PathBuf,
    },
    /// [`write_file`] found the target had changed since the caller's
    /// [`read_stamp`].
    #[error("{path}: changed since it was read; the write was refused")]
    StaleRead {
        /// The path that changed.
        path: PathBuf,
    },
    /// The underlying `ScopeFs` call failed.
    #[error("{path}: {source}")]
    Io {
        /// The path the failing call was about.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}

impl FsOpsError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        FsOpsError::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    fn escapes(name: &Path, reason: &'static str) -> Self {
        FsOpsError::Escapes {
            name: name.to_path_buf(),
            reason,
        }
    }
}

/// Tags an I/O `Result` with the path it was about, collapsing the
/// `.map_err(|e| FsOpsError::io(path, e))` this module would otherwise
/// repeat after every `ScopeFs` call.
trait IoResultExt<T> {
    fn fs_err(self, path: &Path) -> Result<T, FsOpsError>;
}

impl<T> IoResultExt<T> for std::io::Result<T> {
    fn fs_err(self, path: &Path) -> Result<T, FsOpsError> {
        self.map_err(|e| FsOpsError::io(path, e))
    }
}

/// A counter mixed into every temp name this module mints, so two calls in
/// the same process never collide even when they land in the same second.
static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_suffix() -> String {
    let n = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{n}", std::process::id())
}

/// Joins `target` onto `base` (unless `target` is already absolute) and
/// collapses `.`/`..` segments lexically, matching how a real symlink
/// target resolves relative to the directory holding the link.
fn join_lexical(base: &Path, target: &Path) -> PathBuf {
    use std::path::Component;
    let joined = if target.is_absolute() {
        target.to_path_buf()
    } else {
        base.join(target)
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// A handle confined to one root directory.
///
/// Every primitive that takes a `Root` revalidates its device and inode
/// (via [`ScopeFs::fsops_device_inode`]) before and after each on-disk
/// step: a root swapped for a symlink or another directory mid-operation
/// is refused with [`FsOpsError::RootMoved`], never followed.
pub struct Root<'a> {
    fs: &'a dyn ScopeFs,
    path: PathBuf,
    binding: (u64, u64),
}

impl<'a> Root<'a> {
    /// Opens `path` as a root, capturing its device and inode.
    pub fn open(fs: &'a dyn ScopeFs, path: PathBuf) -> Result<Self, FsOpsError> {
        let binding = fs.fsops_device_inode(&path).fs_err(&path)?;
        Ok(Root { fs, path, binding })
    }

    /// The root's own path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Re-reads the root's device and inode; fails with
    /// [`FsOpsError::RootMoved`] when they no longer match what
    /// [`Self::open`] captured.
    pub fn revalidate(&self) -> Result<(), FsOpsError> {
        let now = self.fs.fsops_device_inode(&self.path).fs_err(&self.path)?;
        if now != self.binding {
            return Err(FsOpsError::RootMoved {
                root: self.path.clone(),
            });
        }
        Ok(())
    }

    /// Proves `name` (a path relative to the root) stays inside it and
    /// returns the joined path. Refuses an absolute `name`, a `..`
    /// segment, and a symlink among `name`'s existing ancestors that
    /// resolves outside the root - all before any byte is written.
    ///
    /// The leaf itself is not required to exist: [`stage`], [`link`], and
    /// [`write_file`] all confine a name that is about to be created. When
    /// the leaf already exists as a symlink it is not followed here -
    /// every primitive that finally touches it does so by rename, which
    /// replaces the directory entry without dereferencing it.
    pub fn confine(&self, name: &Path) -> Result<PathBuf, FsOpsError> {
        if name.as_os_str().is_empty() {
            return Ok(self.path.clone());
        }
        if name.is_absolute() {
            return Err(FsOpsError::escapes(name, "absolute path"));
        }
        if name
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(FsOpsError::escapes(name, "`..` segment"));
        }
        let components: Vec<_> = name.components().collect();
        let mut resolved = self.path.clone();
        for (i, component) in components.iter().enumerate() {
            resolved.push(component);
            if i + 1 == components.len() {
                break;
            }
            if let Ok(facts) = self.fs.symlink_metadata(&resolved) {
                if facts.kind == FileKind::Symlink {
                    let target = self.fs.read_link(&resolved).fs_err(&resolved)?;
                    let parent = resolved
                        .parent()
                        .unwrap_or(resolved.as_path())
                        .to_path_buf();
                    resolved = join_lexical(&parent, &target);
                    if resolved != self.path && !resolved.starts_with(&self.path) {
                        return Err(FsOpsError::escapes(
                            name,
                            "a symlink resolves outside the root",
                        ));
                    }
                }
            }
        }
        Ok(self.path.join(name))
    }
}

/// Fsyncs `dir`, then its parent, then on up through the root (inclusive) -
/// the durability order a structural change (a create, a rename, or a
/// removal) needs so the directory entry itself, not just the thing it
/// names, survives a crash.
fn fsync_up_to_root(root: &Root, mut dir: PathBuf) -> Result<(), FsOpsError> {
    loop {
        root.fs.fsops_fsync_dir(&dir).fs_err(&dir)?;
        if dir == root.path {
            return Ok(());
        }
        match dir.parent() {
            Some(parent) if parent == root.path || parent.starts_with(&root.path) => {
                dir = parent.to_path_buf();
            }
            _ => return Ok(()),
        }
    }
}

/// A folder built under a temp name next to its final place, fsynced, and
/// not yet visible under any name a scan would find.
pub struct Staged {
    path: PathBuf,
}

impl Staged {
    /// The staged folder's current (temp) path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Builds a folder under a fresh temp name inside `root`, writes every
/// `(relative path, bytes)` pair from `contents` into it, and fsyncs every
/// file and every directory it created - deepest first, then the staged
/// folder itself, then up to the root - before returning. Nothing outside
/// this call sees the folder yet: it sits under a name a scan never
/// matches.
pub fn stage(root: &Root, contents: &[(PathBuf, Vec<u8>)]) -> Result<Staged, FsOpsError> {
    root.revalidate()?;
    let tmp_name = PathBuf::from(format!(".skill-studio-stage-{}", unique_suffix()));
    let tmp_path = root.confine(&tmp_name)?;
    root.fs.fsops_create_dir(&tmp_path).fs_err(&tmp_path)?;

    let mut created_dirs = vec![tmp_path.clone()];
    for (relative, bytes) in contents {
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(FsOpsError::escapes(
                relative,
                "staged content path escapes the staged folder",
            ));
        }
        let components: Vec<_> = relative.components().collect();
        let mut dir = tmp_path.clone();
        for component in &components[..components.len().saturating_sub(1)] {
            dir.push(component);
            if root.fs.symlink_metadata(&dir).is_err() {
                root.fs.fsops_create_dir(&dir).fs_err(&dir)?;
                created_dirs.push(dir.clone());
            }
        }
        let file_path = tmp_path.join(relative);
        root.fs
            .fsops_write_new_file(&file_path, bytes)
            .fs_err(&file_path)?;
        root.fs.fsops_fsync_file(&file_path).fs_err(&file_path)?;
    }

    created_dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for dir in &created_dirs {
        root.fs.fsops_fsync_dir(dir).fs_err(dir)?;
    }
    fsync_up_to_root(root, tmp_path.clone())?;
    root.revalidate()?;
    Ok(Staged { path: tmp_path })
}

/// Replaces the folder at `final_name` with `staged`. If one already sits
/// at `final_name`, it is moved into `quarantine_dir` (never deleted in
/// place); `staged` becomes `final_name`.
///
/// The one crash-critical step is [`ScopeFs::fsops_exchange`] (an old
/// folder existed - a single atomic swap of the two directory entries) or
/// a single rename (nothing was there before). Either way a process killed
/// at any point during `swap` leaves `final_name` showing the old folder
/// or the new one, never neither and never a mix of both; moving the
/// exchanged-out old folder into `quarantine_dir` afterward is cleanup, not
/// part of that guarantee.
///
/// Refuses with [`FsOpsError::ReplacedBySymlink`], touching nothing, when
/// `final_name` exists but is not a directory - the shape a directory
/// replaced by a symlink between an earlier [`stage`] and this call would
/// take.
pub fn swap(
    root: &Root,
    final_name: &Path,
    staged: Staged,
    quarantine_dir: &Path,
) -> Result<(), FsOpsError> {
    root.revalidate()?;
    let final_path = root.confine(final_name)?;
    let old_facts = root.fs.symlink_metadata(&final_path).ok();

    match old_facts {
        None => {
            root.fs
                .fsops_rename(&staged.path, &final_path)
                .fs_err(&final_path)?;
        }
        Some(facts) if facts.kind == FileKind::Dir => {
            root.fs
                .fsops_exchange(&staged.path, &final_path)
                .fs_err(&final_path)?;
            // `final_path` already shows the new content: the exchange
            // above is the commit point. The old content now sits at
            // `staged.path`, under its temp name; moving it into
            // quarantine is durability for the *old* copy, not for this
            // operation's own correctness.
            let quarantine_root = root.confine(quarantine_dir)?;
            if root.fs.symlink_metadata(&quarantine_root).is_err() {
                root.fs
                    .fsops_create_dir(&quarantine_root)
                    .fs_err(&quarantine_root)?;
            }
            let leaf = final_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("quarantined");
            let quarantine_target = quarantine_root.join(format!("{leaf}-{}", unique_suffix()));
            root.fs
                .fsops_rename(&staged.path, &quarantine_target)
                .fs_err(&quarantine_target)?;
        }
        Some(_) => {
            return Err(FsOpsError::ReplacedBySymlink { path: final_path });
        }
    }

    let parent = final_path.parent().unwrap_or(root.path()).to_path_buf();
    fsync_up_to_root(root, parent)?;
    root.revalidate()?;
    Ok(())
}

/// Creates a symlink at `name` pointing at `target`, under a temp name
/// first and then renamed into place, so the link only ever appears fully
/// formed.
pub fn link(root: &Root, name: &Path, target: &Path) -> Result<(), FsOpsError> {
    root.revalidate()?;
    let link_path = root.confine(name)?;
    let parent = link_path.parent().unwrap_or(root.path()).to_path_buf();
    let leaf = link_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("link");
    let tmp_path = parent.join(format!(".{leaf}-{}", unique_suffix()));

    root.fs.fsops_symlink(target, &tmp_path).fs_err(&tmp_path)?;
    root.fs
        .fsops_rename(&tmp_path, &link_path)
        .fs_err(&link_path)?;
    fsync_up_to_root(root, parent)?;
    root.revalidate()?;
    Ok(())
}

/// What [`read_stamp`] captured about a target before a [`write_file`]
/// call, so the write can be refused if the target changed underneath the
/// caller.
///
/// The stamp is the target's SHA-256 content hash (or that it was absent),
/// not its length plus mtime: a length-plus-mtime stamp can miss a same-
/// second edit that keeps the length unchanged, which a content hash
/// cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadStamp {
    /// Nothing was at the path when it was read.
    Absent,
    /// The path held a file with this SHA-256 hash.
    Present([u8; 32]),
}

/// Reads `path`'s current stamp through `fs`, for a caller to hold and pass
/// back to [`write_file`] once it has decided what to write.
pub fn read_stamp(fs: &dyn ScopeFs, path: &Path) -> Result<ReadStamp, FsOpsError> {
    if fs.symlink_metadata(path).is_err() {
        return Ok(ReadStamp::Absent);
    }
    let bytes = fs.read_capped(path, u64::MAX).fs_err(path)?;
    Ok(ReadStamp::Present(sha256(&bytes)))
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Writes `bytes` to `name` through a temp file, fsync, and rename.
///
/// Refuses with [`FsOpsError::StaleRead`], leaving the current target
/// untouched, when the target's content no longer matches `expected` - the
/// [`ReadStamp`] the caller captured with [`read_stamp`] before deciding
/// what `bytes` should be.
pub fn write_file(
    root: &Root,
    name: &Path,
    bytes: &[u8],
    expected: &ReadStamp,
) -> Result<(), FsOpsError> {
    root.revalidate()?;
    let target = root.confine(name)?;
    let current = read_stamp(root.fs, &target)?;
    if &current != expected {
        return Err(FsOpsError::StaleRead { path: target });
    }

    let parent = target.parent().unwrap_or(root.path()).to_path_buf();
    let leaf = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("write");
    let tmp_path = parent.join(format!(".{leaf}-{}.tmp", unique_suffix()));

    root.fs
        .fsops_write_new_file(&tmp_path, bytes)
        .fs_err(&tmp_path)?;
    root.fs.fsops_fsync_file(&tmp_path).fs_err(&tmp_path)?;
    root.fs.fsops_rename(&tmp_path, &target).fs_err(&target)?;
    fsync_up_to_root(root, parent)?;
    root.revalidate()?;
    Ok(())
}
