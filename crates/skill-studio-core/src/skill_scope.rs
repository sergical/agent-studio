//! Reads through declared directory handles. Binding is provisional until the
//! service acquires coordination and revalidates roots; this is not a scanner.
use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[cfg(test)]
use std::cell::RefCell;

use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, File, Metadata, MetadataExt, OpenOptions};

const MAX_LINKS: usize = 40;

struct BoundRoot {
    requested: PathBuf,
    physical: PathBuf,
    directory: Dir,
}

/// A set of explicitly supplied roots, with no process-home or cwd fallback.
/// The caller retains lexical deployment identity separately from these handles.
pub struct SkillReadScope {
    roots: Vec<BoundRoot>,
}

/// The result of binding a requested root without discarding other readable
/// roots. `bind` remains all-or-nothing for coordination callers.
pub struct PartialSkillReadScope {
    pub scope: SkillReadScope,
    pub outcomes: Vec<RootBindOutcome>,
}

#[derive(Debug)]
pub enum RootBindOutcome {
    Bound {
        requested: PathBuf,
        physical: PathBuf,
    },
    Missing {
        requested: PathBuf,
        source: io::Error,
    },
    Failed {
        requested: PathBuf,
        source: io::Error,
    },
}

/// A regular-file identity observed through this scope's retained roots.
///
/// This stays crate-private because a coordination adapter must not construct a
/// file lock claim from an ambient path.
#[derive(Clone, Debug)]
pub(crate) struct ScopedFileObservation {
    requested: PathBuf,
    resolved: PathBuf,
    device: u64,
    inode: u64,
    link_count: u64,
    metadata: Metadata,
    file: Arc<File>,
}

/// A short-lived regular-file observation for bounded content reads. Unlike
/// `ScopedFileObservation`, this retains no descriptor and therefore cannot
/// accumulate one open file per resource during a folder walk.
#[derive(Clone, Debug)]
pub(crate) struct ScopedContentObservation {
    pub(crate) requested: PathBuf,
    resolved: PathBuf,
    device: u64,
    inode: u64,
    link_count: u64,
    metadata: Metadata,
}

pub(crate) struct ScopedPrefixRead {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

#[derive(Debug)]
pub(crate) enum ScopedContentFoldError {
    Cancelled(String),
    Read(ScopedReadError),
    Changed,
}

impl PartialEq for ScopedFileObservation {
    fn eq(&self, other: &Self) -> bool {
        self.requested == other.requested
            && self.resolved == other.resolved
            && self.device == other.device
            && self.inode == other.inode
            && self.link_count == other.link_count
            && unchanged(&self.metadata, &other.metadata)
    }
}

impl Eq for ScopedFileObservation {}

struct OpenedSource {
    file: File,
    metadata: Metadata,
    resolved: PathBuf,
}

#[derive(Debug)]
pub struct ScopedDirectoryEntry {
    pub name: OsString,
    pub metadata: Metadata,
    /// The link text observed without following an entry link.  It is kept
    /// with the lexical listing so a failed later resolution has useful,
    /// non-ambient diagnostics.
    pub raw_link_target: Result<Option<PathBuf>, io::Error>,
    requested: PathBuf,
    parent_resolved: PathBuf,
    parent_metadata: Metadata,
}

/// A no-follow observation of one lexical child.  The entry can later be
/// resolved through the same retained scope; callers keep this record so a
/// replacement or link retarget cannot be mistaken for the listed entry.
pub(crate) struct ScopedEntryObservation {
    requested: PathBuf,
    parent_resolved: PathBuf,
    parent_metadata: Metadata,
    pub metadata: Metadata,
    pub raw_link_target: Result<Option<PathBuf>, io::Error>,
}

impl ScopedDirectoryEntry {
    pub(crate) fn into_observation(self) -> ScopedEntryObservation {
        ScopedEntryObservation {
            requested: self.requested,
            parent_resolved: self.parent_resolved,
            parent_metadata: self.parent_metadata,
            metadata: self.metadata,
            raw_link_target: self.raw_link_target,
        }
    }
}

#[derive(Debug)]
pub enum DirectoryReadIssue {
    Entry {
        name: Option<OsString>,
        source: io::Error,
    },
    LimitReached,
    Changed,
    DirectoryMetadata(io::Error),
}

#[derive(Debug)]
pub struct ScopedDirectoryRead {
    pub(crate) observation: ScopedDirectoryObservation,
    pub entries: Vec<ScopedDirectoryEntry>,
    pub issues: Vec<DirectoryReadIssue>,
}

#[derive(Debug, Clone)]
pub(crate) struct ScopedDirectoryObservation {
    requested: PathBuf,
    resolved: PathBuf,
    metadata: Metadata,
}

impl ScopedDirectoryObservation {
    pub(crate) fn revalidate(&self, scope: &SkillReadScope) -> bool {
        matches!(scope.resolve(&self.requested), Ok(current)
            if current.metadata.is_dir()
                && current.resolved == self.resolved
                && unchanged(&self.metadata, &current.metadata))
    }
}

/// Only `Missing` establishes an absent entry. An I/O error after observing an
/// entry, or while resolving a link target, must remain a failed source read.
#[derive(Debug)]
pub enum ScopedReadError {
    Missing {
        path: PathBuf,
        source: io::Error,
    },
    LinkTarget {
        link: PathBuf,
        target: PathBuf,
        source: io::Error,
    },
    Io(io::Error),
}

pub(crate) enum ScopedLockOpenError {
    Changed,
    Unavailable(ScopedReadError),
}

enum RetainedObservationError {
    Changed,
    Unavailable(io::Error),
}

#[cfg(test)]
thread_local! {
    static OBSERVED_LOCK_OPEN_FAILURE: RefCell<Option<(usize, i32)>> = const { RefCell::new(None) };
    static OBSERVED_LINK_REPLACEMENT: RefCell<bool> = const { RefCell::new(false) };
    static OBSERVED_ENTRY_LINK_READ_FAILURE: RefCell<Option<i32>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub(crate) struct InjectedObservedLockOpenFailure;

#[cfg(test)]
impl Drop for InjectedObservedLockOpenFailure {
    fn drop(&mut self) {
        OBSERVED_LOCK_OPEN_FAILURE.with(|failure| *failure.borrow_mut() = None);
    }
}

#[cfg(test)]
pub(crate) struct InjectedObservedLinkReplacement;

#[cfg(test)]
impl Drop for InjectedObservedLinkReplacement {
    fn drop(&mut self) {
        OBSERVED_LINK_REPLACEMENT.with(|replacement| *replacement.borrow_mut() = false);
    }
}

#[cfg(test)]
pub(crate) struct InjectedEntryLinkReadFailure;

#[cfg(test)]
impl Drop for InjectedEntryLinkReadFailure {
    fn drop(&mut self) {
        OBSERVED_ENTRY_LINK_READ_FAILURE.with(|failure| *failure.borrow_mut() = None);
    }
}

impl std::fmt::Display for ScopedReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { path, source } => {
                write!(formatter, "missing entry {}: {source}", path.display())
            }
            Self::LinkTarget {
                link,
                target,
                source,
            } => write!(
                formatter,
                "cannot resolve link {} to {}: {source}",
                link.display(),
                target.display()
            ),
            Self::Io(source) => source.fmt(formatter),
        }
    }
}

impl std::error::Error for ScopedReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::Missing { source, .. } | Self::LinkTarget { source, .. } | Self::Io(source) => {
                source
            }
        })
    }
}

impl From<io::Error> for ScopedReadError {
    fn from(source: io::Error) -> Self {
        Self::Io(source)
    }
}

fn scoped_error_is_binding_change(error: &ScopedReadError) -> bool {
    match error {
        ScopedReadError::Missing { .. } => true,
        ScopedReadError::LinkTarget { source, .. } | ScopedReadError::Io(source) => {
            is_binding_changed(source)
                || source.kind() == io::ErrorKind::NotFound
                || matches!(source.raw_os_error(), Some(code) if code == libc::ELOOP || code == libc::ENOTDIR)
        }
    }
}

struct LinkOrigin {
    link: PathBuf,
    target: PathBuf,
}

#[derive(Debug)]
struct ScopedBindingChanged(&'static str);

impl std::fmt::Display for ScopedBindingChanged {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for ScopedBindingChanged {}

impl LinkOrigin {
    fn failure(&self, source: io::Error) -> ScopedReadError {
        ScopedReadError::LinkTarget {
            link: self.link.clone(),
            target: self.target.clone(),
            source,
        }
    }
}

fn resolution_failure(origin: Option<&LinkOrigin>, source: io::Error) -> ScopedReadError {
    match origin {
        Some(origin) => origin.failure(source),
        None => ScopedReadError::Io(source),
    }
}

struct LookupStep {
    name: OsString,
    origin: Option<Arc<LinkOrigin>>,
}

fn lookup_steps(
    names: VecDeque<OsString>,
    origin: Option<Arc<LinkOrigin>>,
) -> VecDeque<LookupStep> {
    names
        .into_iter()
        .map(|name| LookupStep {
            name,
            origin: origin.clone(),
        })
        .collect()
}

// Relocation strips a known root prefix and can add already-resolved ancestor
// names. The remaining pending names are a suffix, with their link origins intact.
fn relocated_steps(
    names: VecDeque<OsString>,
    pending: &VecDeque<LookupStep>,
) -> Result<VecDeque<LookupStep>, ScopedReadError> {
    let mut origins = pending.iter().rev().filter(|step| step.name != ".");
    let mut result = VecDeque::new();
    for name in names.into_iter().rev() {
        let origin = if name == "." {
            None
        } else if let Some(previous) = origins.next() {
            if name != previous.name {
                return Err(changed().into());
            }
            previous.origin.clone()
        } else {
            None
        };
        result.push_front(LookupStep { name, origin });
    }
    Ok(result)
}

#[derive(Debug)]
pub enum RootRevalidationError {
    Changed { root: PathBuf },
    Unavailable { root: PathBuf, source: io::Error },
}

impl std::fmt::Display for RootRevalidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Changed { root } => {
                write!(formatter, "declared root changed: {}", root.display())
            }
            Self::Unavailable { root, source } => write!(
                formatter,
                "cannot revalidate root {}: {source}",
                root.display()
            ),
        }
    }
}

impl std::error::Error for RootRevalidationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Changed { .. } => None,
            Self::Unavailable { source, .. } => Some(source),
        }
    }
}

fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "path leaves declared roots",
    )
}
fn changed() -> io::Error {
    io::Error::other(ScopedBindingChanged("source changed during scoped read"))
}
fn changed_with_kind(kind: io::ErrorKind, message: &'static str) -> io::Error {
    io::Error::new(kind, ScopedBindingChanged(message))
}
fn is_binding_changed(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|source| source.is::<ScopedBindingChanged>())
}
fn same_object(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}
fn unchanged(left: &Metadata, right: &Metadata) -> bool {
    same_object(left, right)
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}
fn parts(path: &Path) -> VecDeque<OsString> {
    let mut parts: VecDeque<_> = path
        .components()
        .map(|part| part.as_os_str().to_owned())
        .collect();
    let bytes = path.as_os_str().as_bytes();
    if bytes.ends_with(b"/") || bytes.ends_with(b"/.") {
        parts.push_back(OsString::from("."));
    }
    parts
}

impl SkillReadScope {
    fn bind_root(requested: &Path) -> io::Result<BoundRoot> {
        if !requested.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "root must be absolute",
            ));
        }
        let physical = std::fs::canonicalize(requested)?;
        let directory = Dir::open_ambient_dir(&physical, ambient_authority())?;
        Ok(BoundRoot {
            requested: requested.to_path_buf(),
            physical,
            directory,
        })
    }

    pub fn bind(roots: &[PathBuf]) -> io::Result<Self> {
        let bound = roots
            .iter()
            .map(|path| Self::bind_root(path))
            .collect::<io::Result<Vec<_>>>()?;
        let scope = Self { roots: bound };
        scope.revalidate_roots().map_err(|error| {
            let kind = match &error {
                RootRevalidationError::Changed { .. } => io::ErrorKind::Other,
                RootRevalidationError::Unavailable { source, .. } => source.kind(),
            };
            io::Error::new(kind, error)
        })?;
        Ok(scope)
    }

    /// Bind every readable declared root once. This deliberately has no
    /// existence preflight and never binds a missing root's parent.
    pub fn bind_partial(roots: &[PathBuf]) -> PartialSkillReadScope {
        let mut bound = Vec::new();
        let mut outcomes = Vec::with_capacity(roots.len());
        for requested in roots {
            match Self::bind_root(requested) {
                Ok(root) => {
                    let physical = root.physical.clone();
                    bound.push(root);
                    outcomes.push(RootBindOutcome::Bound {
                        requested: requested.clone(),
                        physical,
                    });
                }
                Err(source) if source.kind() == io::ErrorKind::NotFound => {
                    outcomes.push(RootBindOutcome::Missing {
                        requested: requested.clone(),
                        source,
                    });
                }
                Err(source) => outcomes.push(RootBindOutcome::Failed {
                    requested: requested.clone(),
                    source,
                }),
            }
        }
        PartialSkillReadScope {
            scope: SkillReadScope { roots: bound },
            outcomes,
        }
    }

    /// Checks each declared name against its captured physical path and handle.
    /// Call under coordination before and after an authoritative read sequence.
    /// This point-in-time check does not exclude concurrent external changes.
    pub fn revalidate_roots(&self) -> Result<(), RootRevalidationError> {
        for root in &self.roots {
            let unavailable = |source| RootRevalidationError::Unavailable {
                root: root.requested.clone(),
                source,
            };
            let physical = std::fs::canonicalize(&root.requested).map_err(unavailable)?;
            if physical != root.physical {
                return Err(RootRevalidationError::Changed {
                    root: root.requested.clone(),
                });
            }
            let current =
                Dir::open_ambient_dir(&root.requested, ambient_authority()).map_err(unavailable)?;
            let current_metadata = current.dir_metadata().map_err(unavailable)?;
            let bound_metadata = root.directory.dir_metadata().map_err(unavailable)?;
            if !same_object(&bound_metadata, &current_metadata) {
                return Err(RootRevalidationError::Changed {
                    root: root.requested.clone(),
                });
            }
        }
        Ok(())
    }

    pub(crate) fn clone_bound_directory(
        &self,
        requested: &Path,
    ) -> io::Result<std::os::fd::OwnedFd> {
        use std::os::fd::AsFd;
        self.roots
            .iter()
            .find(|root| root.requested == requested)
            .ok_or_else(denied)?
            .directory
            .as_fd()
            .try_clone_to_owned()
    }

    /// Diagnostic classification only; this does not resolve links or authorize a read.
    pub(crate) fn has_declared_prefix(&self, path: &Path) -> bool {
        self.locate(path).is_ok()
    }

    fn locate(&self, path: &Path) -> io::Result<(usize, VecDeque<OsString>)> {
        if !path.is_absolute() {
            return Err(denied());
        }
        self.roots
            .iter()
            .enumerate()
            .flat_map(|(index, root)| {
                [&root.requested, &root.physical]
                    .into_iter()
                    .filter_map(move |alias| {
                        path.strip_prefix(alias).ok().map(|rest| {
                            let mut remaining = parts(rest);
                            let bytes = path.as_os_str().as_bytes();
                            if bytes.ends_with(b"/") || bytes.ends_with(b"/.") {
                                remaining.push_back(OsString::from("."));
                            }
                            (alias.components().count(), index, remaining)
                        })
                    })
            })
            .max_by_key(|(length, _, _)| *length)
            .map(|(_, index, rest)| (index, rest))
            .ok_or_else(denied)
    }

    /// Reads one regular file with a caller-supplied byte limit. Missing files,
    /// denied paths, link loops, type changes, and oversized files remain errors.
    pub fn read(&self, path: &Path, limit: usize) -> Result<Vec<u8>, ScopedReadError> {
        let cap = limit
            .checked_add(1)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid byte limit"))?;
        let source = self.resolve(path)?;
        read_regular(source.file, source.metadata, cap, limit).map_err(Into::into)
    }

    pub(crate) fn read_prefix_checked(
        &self,
        path: &Path,
        limit: usize,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<ScopedPrefixRead, ScopedContentFoldError> {
        check().map_err(ScopedContentFoldError::Cancelled)?;
        let cap = limit.checked_add(1).ok_or_else(|| {
            ScopedContentFoldError::Read(ScopedReadError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid byte limit",
            )))
        })?;
        let source = self.resolve(path).map_err(ScopedContentFoldError::Read)?;
        if !source.metadata.is_file() {
            return Err(ScopedContentFoldError::Read(ScopedReadError::Io(
                io::Error::new(io::ErrorKind::InvalidInput, "source is not a regular file"),
            )));
        }
        let mut file = source.file;
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 64 * 1024];
        while bytes.len() < cap {
            check().map_err(ScopedContentFoldError::Cancelled)?;
            let chunk_len = (cap - bytes.len()).min(64 * 1024);
            let count = file
                .read(&mut buffer[..chunk_len])
                .map_err(|error| ScopedContentFoldError::Read(ScopedReadError::Io(error)))?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        let after = file
            .metadata()
            .map_err(|error| ScopedContentFoldError::Read(ScopedReadError::Io(error)))?;
        if !unchanged(&source.metadata, &after) {
            return Err(ScopedContentFoldError::Changed);
        }
        let reopened = self.resolve(path).map_err(ScopedContentFoldError::Read)?;
        if reopened.resolved != source.resolved || !unchanged(&source.metadata, &reopened.metadata)
        {
            return Err(ScopedContentFoldError::Changed);
        }
        let truncated = bytes.len() > limit;
        if truncated {
            bytes.truncate(limit);
        }
        Ok(ScopedPrefixRead { bytes, truncated })
    }

    pub(crate) fn read_observed_prefix(
        &self,
        observation: &ScopedFileObservation,
        limit: usize,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<ScopedPrefixRead, ScopedContentFoldError> {
        let cap = limit.checked_add(1).ok_or_else(|| {
            ScopedContentFoldError::Read(ScopedReadError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid byte limit",
            )))
        })?;
        let mut bytes = Vec::new();
        self.fold_observed_file(observation, cap as u64, check, &mut |chunk| {
            bytes.extend_from_slice(chunk);
        })?;
        let truncated = bytes.len() > limit;
        bytes.truncate(limit);
        Ok(ScopedPrefixRead { bytes, truncated })
    }

    /// Streams against the observation retained by a file coordination lease.
    /// Both the retained descriptor and the scoped name must remain unchanged.
    pub(crate) fn fold_observed_file(
        &self,
        observation: &ScopedFileObservation,
        limit: u64,
        check: &mut dyn FnMut() -> Result<(), String>,
        fold: &mut dyn FnMut(&[u8]),
    ) -> Result<u64, ScopedContentFoldError> {
        let validate = || {
            observation
                .validate_retained()
                .map_err(|error| match error {
                    RetainedObservationError::Changed => ScopedContentFoldError::Changed,
                    RetainedObservationError::Unavailable(source) => {
                        ScopedContentFoldError::Read(ScopedReadError::Io(source))
                    }
                })
        };
        check().map_err(ScopedContentFoldError::Cancelled)?;
        validate()?;
        let content = ScopedContentObservation {
            requested: observation.requested.clone(),
            resolved: observation.resolved.clone(),
            device: observation.device,
            inode: observation.inode,
            link_count: observation.link_count,
            metadata: observation.metadata.clone(),
        };
        let total = self.fold_observed_content(&content, limit, check, fold)?;
        validate()?;
        Ok(total)
    }

    /// Records a regular file's identity and metadata without retaining its
    /// descriptor. `fold_observed` must reopen it through this scope.
    pub(crate) fn observe_content_regular(
        &self,
        path: &Path,
    ) -> Result<ScopedContentObservation, ScopedReadError> {
        let source = self.resolve(path)?;
        if !source.metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source is not a regular file",
            )
            .into());
        }
        Ok(ScopedContentObservation {
            requested: path.to_path_buf(),
            resolved: source.resolved,
            device: source.metadata.dev(),
            inode: source.metadata.ino(),
            link_count: source.metadata.nlink(),
            metadata: source.metadata,
        })
    }

    /// Reopens a lightweight observation through the bound scope and streams
    /// it in 64 KiB chunks. The identity and metadata must match before and
    /// after the fold.
    pub(crate) fn fold_observed_content(
        &self,
        observation: &ScopedContentObservation,
        limit: u64,
        check: &mut dyn FnMut() -> Result<(), String>,
        fold: &mut dyn FnMut(&[u8]),
    ) -> Result<u64, ScopedContentFoldError> {
        check().map_err(ScopedContentFoldError::Cancelled)?;
        let source = self
            .resolve(&observation.requested)
            .map_err(ScopedContentFoldError::Read)?;
        if !source.metadata.is_file() {
            return Err(ScopedContentFoldError::Read(ScopedReadError::Io(
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "source is no longer a regular file",
                ),
            )));
        }
        if !observation.matches_source(&source) {
            return Err(ScopedContentFoldError::Changed);
        }
        let mut file = source.file;
        let mut total = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            check().map_err(ScopedContentFoldError::Cancelled)?;
            let remaining = limit.saturating_sub(total);
            if remaining == 0 {
                break;
            }
            let chunk_len = remaining.min(64 * 1024) as usize;
            let count = file
                .read(&mut buffer[..chunk_len])
                .map_err(|error| ScopedContentFoldError::Read(ScopedReadError::Io(error)))?;
            if count == 0 {
                break;
            }
            fold(&buffer[..count]);
            total += count as u64;
        }
        let after = file
            .metadata()
            .map_err(|error| ScopedContentFoldError::Read(ScopedReadError::Io(error)))?;
        if !observation.matches_metadata(&after) {
            return Err(ScopedContentFoldError::Changed);
        }
        let reopened = self
            .resolve(&observation.requested)
            .map_err(ScopedContentFoldError::Read)?;
        if !observation.matches_source(&reopened) {
            return Err(ScopedContentFoldError::Changed);
        }
        Ok(total)
    }

    pub(crate) fn read_content_prefix(
        &self,
        observation: &ScopedContentObservation,
        limit: usize,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<ScopedPrefixRead, ScopedContentFoldError> {
        let cap = limit.checked_add(1).ok_or_else(|| {
            ScopedContentFoldError::Read(ScopedReadError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid byte limit",
            )))
        })?;
        let mut bytes = Vec::new();
        self.fold_observed_content(observation, cap as u64, check, &mut |chunk| {
            bytes.extend_from_slice(chunk)
        })?;
        let truncated = bytes.len() > limit;
        bytes.truncate(limit);
        Ok(ScopedPrefixRead { bytes, truncated })
    }

    pub(crate) fn read_content_observed(
        &self,
        observation: &ScopedContentObservation,
        limit: usize,
    ) -> Result<Vec<u8>, ScopedReadError> {
        let prefix = self
            .read_content_prefix(observation, limit, &mut || Ok(()))
            .map_err(|error| match error {
                ScopedContentFoldError::Read(error) => error,
                ScopedContentFoldError::Changed => ScopedReadError::Io(changed()),
                ScopedContentFoldError::Cancelled(message) => {
                    ScopedReadError::Io(io::Error::new(io::ErrorKind::Interrupted, message))
                }
            })?;
        if prefix.truncated {
            return Err(
                io::Error::new(io::ErrorKind::InvalidData, "source exceeds byte limit").into(),
            );
        }
        Ok(prefix.bytes)
    }

    /// Observes a regular file using only declared roots. The result can be
    /// retained by the coordinator and later supplied to `read_observed`.
    pub(crate) fn observe_regular(
        &self,
        path: &Path,
    ) -> Result<ScopedFileObservation, ScopedReadError> {
        let source = self.resolve(path)?;
        if !source.metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source is not a regular file",
            )
            .into());
        }
        Ok(ScopedFileObservation {
            requested: path.to_path_buf(),
            resolved: source.resolved,
            device: source.metadata.dev(),
            inode: source.metadata.ino(),
            link_count: source.metadata.nlink(),
            metadata: source.metadata,
            file: Arc::new(source.file),
        })
    }

    /// Reads an observation previously issued by this scope. Both the name and
    /// the opened object must still match before and after byte collection.
    pub(crate) fn read_observed(
        &self,
        observation: &ScopedFileObservation,
        limit: usize,
    ) -> Result<Vec<u8>, ScopedReadError> {
        let cap = limit
            .checked_add(1)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid byte limit"))?;
        observation
            .validate_retained()
            .map_err(|error| match error {
                RetainedObservationError::Changed => ScopedReadError::Io(changed()),
                RetainedObservationError::Unavailable(source) => ScopedReadError::Io(source),
            })?;
        let source = self.resolve(&observation.requested)?;
        if !observation.matches_source(&source) {
            return Err(changed().into());
        }
        read_regular_observed(source.file, source.metadata, observation, cap, limit)
            .map_err(Into::into)
    }

    pub(crate) fn observed_requested(observation: &ScopedFileObservation) -> &Path {
        &observation.requested
    }

    pub(crate) fn observed_resolved(observation: &ScopedFileObservation) -> &Path {
        &observation.resolved
    }

    pub(crate) fn observed_identity(observation: &ScopedFileObservation) -> (u64, u64, u64) {
        (
            observation.device,
            observation.inode,
            observation.link_count,
        )
    }

    /// Opens a fresh, scope-validated handle for a planned file lock. This is
    /// deliberately separate from `read_observed`: each native lease gets an
    /// independent descriptor and never uses an ambient path reopen.
    pub(crate) fn open_observed_lock(
        &self,
        observation: &ScopedFileObservation,
    ) -> Result<std::fs::File, ScopedLockOpenError> {
        #[cfg(test)]
        if let Some(error) = OBSERVED_LOCK_OPEN_FAILURE.with(|failure| {
            let mut failure = failure.borrow_mut();
            match failure.as_mut() {
                Some((remaining, _)) if *remaining > 0 => {
                    *remaining -= 1;
                    None
                }
                Some(_) => failure.take().map(|(_, error)| error),
                None => None,
            }
        }) {
            return Err(ScopedLockOpenError::Unavailable(ScopedReadError::Io(
                io::Error::from_raw_os_error(error),
            )));
        }
        observation
            .validate_retained()
            .map_err(|error| match error {
                RetainedObservationError::Changed => ScopedLockOpenError::Changed,
                RetainedObservationError::Unavailable(source) => {
                    ScopedLockOpenError::Unavailable(ScopedReadError::Io(source))
                }
            })?;
        let source = self
            .resolve(&observation.requested)
            .map_err(|error| match error {
                error if scoped_error_is_binding_change(&error) => ScopedLockOpenError::Changed,
                error => ScopedLockOpenError::Unavailable(error),
            })?;
        if !observation.matches_source(&source) {
            return Err(ScopedLockOpenError::Changed);
        }
        Ok(source.file.into_std())
    }

    #[cfg(test)]
    pub(crate) fn inject_observed_lock_open_failure(
        successful_opens_before_failure: usize,
        raw_os_error: i32,
    ) -> InjectedObservedLockOpenFailure {
        OBSERVED_LOCK_OPEN_FAILURE.with(|failure| {
            *failure.borrow_mut() = Some((successful_opens_before_failure, raw_os_error));
        });
        InjectedObservedLockOpenFailure
    }

    #[cfg(test)]
    pub(crate) fn inject_observed_link_replacement() -> InjectedObservedLinkReplacement {
        OBSERVED_LINK_REPLACEMENT.with(|replacement| *replacement.borrow_mut() = true);
        InjectedObservedLinkReplacement
    }

    #[cfg(test)]
    pub(crate) fn inject_entry_link_read_failure(
        raw_os_error: i32,
    ) -> InjectedEntryLinkReadFailure {
        OBSERVED_ENTRY_LINK_READ_FAILURE.with(|failure| *failure.borrow_mut() = Some(raw_os_error));
        InjectedEntryLinkReadFailure
    }

    /// Resolves a declared directory through this scope's retained handles.
    /// The returned path is the resolved identity used for the open, without
    /// ambient canonicalization of the caller's path.
    pub fn resolved_dir_path(&self, path: &Path) -> Result<PathBuf, ScopedReadError> {
        let source = self.resolve(path)?;
        if !source.metadata.is_dir() {
            return Err(
                io::Error::new(io::ErrorKind::InvalidInput, "source is not a directory").into(),
            );
        }
        Ok(source.resolved)
    }

    pub(crate) fn resolved_path_metadata(
        &self,
        path: &Path,
    ) -> Result<(PathBuf, Metadata), ScopedReadError> {
        let source = self.resolve(path)?;
        Ok((source.resolved, source.metadata))
    }

    pub(crate) fn observe_entry(
        &self,
        parent: &Path,
        name: &std::ffi::OsStr,
    ) -> Result<ScopedEntryObservation, ScopedReadError> {
        let source = self.resolve(parent)?;
        if !source.metadata.is_dir() {
            return Err(
                io::Error::new(io::ErrorKind::InvalidInput, "parent is not a directory").into(),
            );
        }
        let directory = Dir::from_std_file(source.file.into_std());
        let metadata = directory.symlink_metadata(name).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                ScopedReadError::Missing {
                    path: parent.join(name),
                    source,
                }
            } else {
                ScopedReadError::Io(source)
            }
        })?;
        let raw_link_target = if metadata.file_type().is_symlink() {
            read_observed_link(&directory, name).map(Some)
        } else {
            Ok(None)
        };
        Ok(ScopedEntryObservation {
            requested: parent.join(name),
            parent_resolved: source.resolved,
            parent_metadata: source.metadata,
            metadata,
            raw_link_target,
        })
    }

    /// Resolves a directory from the exact lexical entry that was observed.
    /// The no-follow entry identity and raw link target must remain stable
    /// before and after resolution.
    pub(crate) fn resolve_observed_dir(
        &self,
        observation: &ScopedEntryObservation,
    ) -> Result<PathBuf, ScopedReadError> {
        self.resolve_observed_dir_with_parent_change(observation, false)
    }

    pub(crate) fn resolve_observed_dir_after_sibling_replace(
        &self,
        observation: &ScopedEntryObservation,
        sibling: &Path,
    ) -> Result<PathBuf, ScopedReadError> {
        if sibling == observation.requested || sibling.parent() != observation.requested.parent() {
            return Err(changed().into());
        }
        self.resolve_observed_dir_with_parent_change(observation, true)
    }

    fn resolve_observed_dir_with_parent_change(
        &self,
        observation: &ScopedEntryObservation,
        parent_membership_changed: bool,
    ) -> Result<PathBuf, ScopedReadError> {
        if let Err(source) = &observation.raw_link_target {
            let source = source.raw_os_error().map_or_else(
                || io::Error::new(source.kind(), source.to_string()),
                io::Error::from_raw_os_error,
            );
            return Err(ScopedReadError::Io(source));
        }
        self.validate_entry_observation_with_parent_change(observation, parent_membership_changed)?;
        let source = self.resolve(&observation.requested)?;
        if !source.metadata.is_dir() {
            return Err(
                io::Error::new(io::ErrorKind::InvalidInput, "source is not a directory").into(),
            );
        }
        self.validate_entry_observation_with_parent_change(observation, parent_membership_changed)?;
        Ok(source.resolved)
    }

    fn validate_entry_observation_with_parent_change(
        &self,
        observation: &ScopedEntryObservation,
        parent_membership_changed: bool,
    ) -> Result<(), ScopedReadError> {
        let parent =
            self.resolve(observation.requested.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "entry has no parent")
            })?)?;
        let parent_unchanged = if parent_membership_changed {
            parent.metadata.is_dir()
                && observation.parent_metadata.is_dir()
                && parent.metadata.dev() == observation.parent_metadata.dev()
                && parent.metadata.ino() == observation.parent_metadata.ino()
        } else {
            unchanged(&parent.metadata, &observation.parent_metadata)
        };
        if parent.resolved != observation.parent_resolved || !parent_unchanged {
            return Err(changed().into());
        }
        let directory = Dir::from_std_file(parent.file.into_std());
        let name = observation
            .requested
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "entry has no name"))?;
        let metadata = directory.symlink_metadata(name)?;
        if !unchanged(&observation.metadata, &metadata)
            || observation.metadata.file_type().is_symlink() != metadata.file_type().is_symlink()
        {
            return Err(changed().into());
        }
        if let Ok(expected) = &observation.raw_link_target {
            let actual = if metadata.file_type().is_symlink() {
                read_observed_link(&directory, name).map(Some)?
            } else {
                None
            };
            if &actual != expected {
                return Err(changed().into());
            }
        }
        Ok(())
    }

    /// Lists a bounded number of entries without following entry links.
    /// Issues retain partial results; a capped listing is never complete.
    pub fn read_dir(
        &self,
        path: &Path,
        limit: usize,
    ) -> Result<ScopedDirectoryRead, ScopedReadError> {
        let source = self.resolve(path)?;
        if !source.metadata.is_dir() {
            return Err(
                io::Error::new(io::ErrorKind::InvalidInput, "source is not a directory").into(),
            );
        }
        let directory = Dir::from_std_file(source.file.into_std());
        let entries = directory.entries()?;
        let resolved = source.resolved.clone();
        let metadata = source.metadata.clone();
        let mut result = collect_directory(
            &directory,
            path,
            &source.resolved,
            source.metadata,
            limit,
            entries,
        );
        match self.resolve(path) {
            Ok(reopened)
                if reopened.resolved == resolved && unchanged(&metadata, &reopened.metadata) => {}
            Ok(_) | Err(_) => {
                if !result
                    .issues
                    .iter()
                    .any(|issue| matches!(issue, DirectoryReadIssue::Changed))
                {
                    result.issues.push(DirectoryReadIssue::Changed);
                }
            }
        }
        Ok(result)
    }

    fn resolve(&self, path: &Path) -> Result<OpenedSource, ScopedReadError> {
        let (mut root_index, names) = self.locate(path)?;
        let mut pending = lookup_steps(names, None);
        let mut resolved = self.roots[root_index].physical.clone();
        let mut directories = vec![self.roots[root_index].directory.try_clone()?];
        let mut links = 0;
        while let Some(step) = pending.pop_front() {
            let name = step.name;
            if name == "." {
                continue;
            }
            if name == ".." {
                if directories.len() > 1 {
                    directories.pop();
                    resolved.pop();
                } else {
                    // Only already-resolved parents are collapsed. Pending
                    // names retain their order, including `link/..`.
                    let mut parent = self.roots[root_index].physical.clone();
                    let mut origin = step.origin;
                    if !parent.pop() {
                        return Err(resolution_failure(origin.as_deref(), denied()));
                    }
                    while pending.front().is_some_and(|part| part.name == "..") {
                        origin = pending.pop_front().unwrap().origin;
                        if !parent.pop() {
                            return Err(resolution_failure(origin.as_deref(), denied()));
                        }
                    }
                    for part in &pending {
                        parent.push(&part.name);
                    }
                    let (next_root, names) = self
                        .locate(&parent)
                        .map_err(|source| resolution_failure(origin.as_deref(), source))?;
                    pending = relocated_steps(names, &pending)?;
                    root_index = next_root;
                    resolved = self.roots[root_index].physical.clone();
                    directories = vec![self.roots[root_index].directory.try_clone()?];
                }
                continue;
            }
            if Path::new(&name)
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            {
                return Err(denied().into());
            }
            let directory = directories.last().unwrap();
            let before = directory.symlink_metadata(&name).map_err(|source| {
                if let Some(origin) = &step.origin {
                    origin.failure(source)
                } else if source.kind() == io::ErrorKind::NotFound {
                    ScopedReadError::Missing {
                        path: resolved.join(&name),
                        source,
                    }
                } else {
                    ScopedReadError::Io(source)
                }
            })?;
            if before.file_type().is_symlink() {
                links += 1;
                if links > MAX_LINKS {
                    return Err(changed_with_kind(
                        io::ErrorKind::InvalidData,
                        "too many symbolic links",
                    )
                    .into());
                }
                #[cfg(test)]
                if OBSERVED_LINK_REPLACEMENT.with(|replacement| replacement.take()) {
                    let path = resolved.join(&name);
                    std::fs::remove_file(&path)?;
                    std::fs::write(path, b"replaced")?;
                }
                let target = directory.read_link_contents(&name).map_err(|source| {
                    if source.raw_os_error() == Some(libc::EINVAL) {
                        ScopedReadError::Io(changed_with_kind(
                            source.kind(),
                            "symbolic link changed during scoped read",
                        ))
                    } else {
                        ScopedReadError::Io(source)
                    }
                })?;
                if !unchanged(&before, &directory.symlink_metadata(&name)?) {
                    return Err(changed().into());
                }
                let origin = Arc::new(LinkOrigin {
                    link: resolved.join(&name),
                    target: target.clone(),
                });
                if target.is_absolute() {
                    let (next_root, names) = self
                        .locate(&target)
                        .map_err(|source| origin.failure(source))?;
                    let mut next = lookup_steps(names, Some(origin));
                    next.append(&mut pending);
                    pending = next;
                    root_index = next_root;
                    resolved = self.roots[root_index].physical.clone();
                    directories = vec![self.roots[root_index].directory.try_clone()?];
                } else {
                    let mut next = lookup_steps(parts(&target), Some(origin));
                    next.append(&mut pending);
                    pending = next;
                }
                continue;
            }
            if (!before.is_file() && !before.is_dir()) || (!pending.is_empty() && !before.is_dir())
            {
                return Err(changed_with_kind(
                    io::ErrorKind::InvalidInput,
                    "unexpected source type",
                )
                .into());
            }
            let mut options = OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No).nonblock(true);
            let file = directory.open_with(&name, &options)?;
            let opened = file.metadata()?;
            if !unchanged(&before, &opened) {
                return Err(changed().into());
            }
            if pending.is_empty() {
                return Ok(OpenedSource {
                    file,
                    metadata: opened,
                    resolved: resolved.join(&name),
                });
            }
            directories.push(Dir::from_std_file(file.into_std()));
            resolved.push(name);
        }
        let directory = directories.pop().unwrap();
        let metadata = directory.dir_metadata()?;
        let file = File::from_std(directory.into_std_file());
        Ok(OpenedSource {
            file,
            metadata,
            resolved,
        })
    }
}

impl ScopedFileObservation {
    pub(crate) fn matches_content(&self, content: &ScopedContentObservation) -> bool {
        self.requested == content.requested
            && self.resolved == content.resolved
            && self.matches_metadata(&content.metadata)
    }

    fn matches_source(&self, source: &OpenedSource) -> bool {
        source.resolved == self.resolved && self.matches_metadata(&source.metadata)
    }

    fn matches_metadata(&self, metadata: &Metadata) -> bool {
        metadata.is_file()
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
            && metadata.nlink() == self.link_count
            && unchanged(&self.metadata, metadata)
    }

    fn validate_retained(&self) -> Result<(), RetainedObservationError> {
        let metadata = self
            .file
            .metadata()
            .map_err(RetainedObservationError::Unavailable)?;
        if self.matches_metadata(&metadata) {
            Ok(())
        } else {
            Err(RetainedObservationError::Changed)
        }
    }
}

impl PartialEq for ScopedContentObservation {
    fn eq(&self, other: &Self) -> bool {
        self.requested == other.requested
            && self.resolved == other.resolved
            && self.link_count == other.link_count
            && unchanged(&self.metadata, &other.metadata)
    }
}

impl Eq for ScopedContentObservation {}

impl ScopedContentObservation {
    pub(crate) fn len(&self) -> u64 {
        self.metadata.len()
    }

    pub(crate) fn single_link(&self) -> bool {
        self.link_count == 1
    }

    pub(crate) fn resolved_path(&self) -> &Path {
        &self.resolved
    }

    fn matches_source(&self, source: &OpenedSource) -> bool {
        source.resolved == self.resolved && self.matches_metadata(&source.metadata)
    }

    pub(crate) fn matches_metadata(&self, metadata: &Metadata) -> bool {
        metadata.is_file()
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
            && metadata.nlink() == self.link_count
            && unchanged(&self.metadata, metadata)
    }
}

fn read_observed_link(directory: &Dir, name: &std::ffi::OsStr) -> io::Result<PathBuf> {
    #[cfg(test)]
    if let Some(raw_os_error) =
        OBSERVED_ENTRY_LINK_READ_FAILURE.with(|failure| failure.borrow_mut().take())
    {
        return Err(io::Error::from_raw_os_error(raw_os_error));
    }
    directory.read_link_contents(name)
}

fn collect_directory(
    directory: &Dir,
    requested: &Path,
    resolved: &Path,
    before: Metadata,
    limit: usize,
    mut entries: impl Iterator<Item = io::Result<cap_std::fs::DirEntry>>,
) -> ScopedDirectoryRead {
    let mut result = ScopedDirectoryRead {
        observation: ScopedDirectoryObservation {
            requested: requested.to_path_buf(),
            resolved: resolved.to_path_buf(),
            metadata: before.clone(),
        },
        entries: Vec::new(),
        issues: Vec::new(),
    };
    for entry in entries.by_ref().take(limit) {
        match entry {
            Ok(entry) => {
                let name = entry.file_name();
                match directory.symlink_metadata(&name) {
                    Ok(metadata) => {
                        let raw_link_target = if metadata.file_type().is_symlink() {
                            read_observed_link(directory, &name).map(Some)
                        } else {
                            Ok(None)
                        };
                        result.entries.push(ScopedDirectoryEntry {
                            requested: requested.join(&name),
                            parent_resolved: resolved.to_path_buf(),
                            parent_metadata: before.clone(),
                            name,
                            metadata,
                            raw_link_target,
                        });
                    }
                    Err(source) => result.issues.push(DirectoryReadIssue::Entry {
                        name: Some(name),
                        source,
                    }),
                }
            }
            Err(source) => result
                .issues
                .push(DirectoryReadIssue::Entry { name: None, source }),
        }
    }
    if let Some(extra) = entries.next() {
        result.issues.push(DirectoryReadIssue::LimitReached);
        if let Err(source) = extra {
            result
                .issues
                .push(DirectoryReadIssue::Entry { name: None, source });
        }
    }
    match directory.dir_metadata() {
        Ok(after) if unchanged(&before, &after) => {}
        Ok(_) => result.issues.push(DirectoryReadIssue::Changed),
        Err(source) => result
            .issues
            .push(DirectoryReadIssue::DirectoryMetadata(source)),
    }
    result
        .entries
        .sort_by(|left, right| left.name.cmp(&right.name));
    result
}

fn read_regular(file: File, before: Metadata, cap: usize, limit: usize) -> io::Result<Vec<u8>> {
    if !before.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source is not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    (&file).take(cap as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source exceeds byte limit",
        ));
    }
    if !unchanged(&before, &file.metadata()?) {
        return Err(changed());
    }
    Ok(bytes)
}

fn read_regular_observed(
    file: File,
    before: Metadata,
    observation: &ScopedFileObservation,
    cap: usize,
    limit: usize,
) -> io::Result<Vec<u8>> {
    if !observation.matches_metadata(&before) {
        return Err(changed());
    }
    let mut bytes = Vec::new();
    (&file).take(cap as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source exceeds byte limit",
        ));
    }
    let after = file.metadata()?;
    if !unchanged(&before, &after) || !observation.matches_metadata(&after) {
        return Err(changed());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    impl ScopedReadError {
        fn io_kind(&self) -> io::ErrorKind {
            match self {
                Self::Missing { source, .. }
                | Self::LinkTarget { source, .. }
                | Self::Io(source) => source.kind(),
            }
        }
    }

    struct Fixture {
        temp: TempDir,
        home: PathBuf,
        backing: PathBuf,
        outside: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let backing = temp.path().join("backing");
            let outside = temp.path().join("outside");
            for root in [&home, &backing, &outside] {
                fs::create_dir_all(root.join("skill/nested")).unwrap();
            }
            fs::write(home.join("skill/SKILL.md"), "home").unwrap();
            fs::write(backing.join("skill/SKILL.md"), "backing").unwrap();
            fs::write(outside.join("skill/SKILL.md"), "secret").unwrap();
            Self {
                temp,
                home,
                backing,
                outside,
            }
        }
        fn scope(&self) -> SkillReadScope {
            SkillReadScope::bind(&[self.home.clone(), self.backing.clone()]).unwrap()
        }
    }

    #[test]
    fn sibling_replacement_allows_parent_membership_change_but_not_identity_change() {
        for replace_parent in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let parent = temp.path().join("agents");
            let root = parent.join("skills");
            std::fs::create_dir_all(&root).unwrap();
            let registry = parent.join("skill-studio.json");
            std::fs::write(&registry, b"old").unwrap();
            let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
            let observation = scope
                .observe_entry(&parent, std::ffi::OsStr::new("skills"))
                .unwrap();
            std::fs::write(parent.join("stage"), b"new").unwrap();
            std::fs::rename(parent.join("stage"), &registry).unwrap();
            assert!(scope.resolve_observed_dir(&observation).is_err());
            scope
                .resolve_observed_dir_after_sibling_replace(&observation, &registry)
                .unwrap();
            assert!(scope
                .resolve_observed_dir_after_sibling_replace(
                    &observation,
                    &temp.path().join("other")
                )
                .is_err());
            let changed = if replace_parent { &parent } else { &root };
            std::fs::rename(changed, temp.path().join("moved")).unwrap();
            std::fs::create_dir_all(&root).unwrap();
            assert!(scope
                .resolve_observed_dir_after_sibling_replace(&observation, &registry)
                .is_err());
        }
    }

    #[test]
    fn absolute_and_relative_links_reach_only_declared_backing() {
        let fixture = Fixture::new();
        for (name, target, expected) in [
            ("local", PathBuf::from("skill"), b"home".as_slice()),
            (
                "absolute-local",
                fixture.home.join("skill"),
                b"home".as_slice(),
            ),
            (
                "absolute-backing",
                fixture.backing.join("skill"),
                b"backing".as_slice(),
            ),
            (
                "relative-backing",
                PathBuf::from("../backing/skill"),
                b"backing".as_slice(),
            ),
        ] {
            symlink(target, fixture.home.join(name)).unwrap();
            assert_eq!(
                fixture
                    .scope()
                    .read(&fixture.home.join(name).join("SKILL.md"), 20)
                    .unwrap(),
                expected
            );
        }
        for (name, target) in [
            ("escape", fixture.outside.join("skill")),
            ("relative-escape", PathBuf::from("../outside/skill")),
        ] {
            let link = fixture.home.join(name);
            let physical_link = fs::canonicalize(&fixture.home).unwrap().join(name);
            symlink(&target, &link).unwrap();
            let scope = fixture.scope();
            for error in [
                scope.read(&link.join("SKILL.md"), 20).unwrap_err(),
                scope.read_dir(&link, 10).unwrap_err(),
            ] {
                assert!(
                    matches!(&error, ScopedReadError::LinkTarget {
                    link: observed_link, target: observed_target, source
                } if observed_link == &physical_link && observed_target == &target
                    && source.kind() == io::ErrorKind::PermissionDenied),
                    "{error:?}"
                );
            }
        }
        let nested = fixture.home.join("skill");
        symlink("..", nested.join("parent-link")).unwrap();
        let scope = SkillReadScope::bind(&[fixture.home.clone(), nested.clone()]).unwrap();
        assert!(matches!(
            scope.read(&nested.join("parent-link/../outside/skill/SKILL.md"), 20),
            Err(ScopedReadError::Io(source)) if source.kind() == io::ErrorKind::PermissionDenied
        ));
    }

    #[test]
    fn parent_components_follow_link_resolution_order() {
        let fixture = Fixture::new();
        symlink(
            fixture.backing.join("skill/nested"),
            fixture.home.join("link"),
        )
        .unwrap();
        assert_eq!(
            fixture
                .scope()
                .read(&fixture.home.join("link/../SKILL.md"), 20)
                .unwrap(),
            b"backing"
        );
        symlink(&fixture.outside, fixture.home.join("outside-link")).unwrap();
        assert_eq!(
            fixture
                .scope()
                .read(
                    &fixture.home.join("outside-link/../home/skill/SKILL.md"),
                    20
                )
                .unwrap_err()
                .io_kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn loops_missing_files_and_limits_are_distinct_errors() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        symlink("loop", fixture.home.join("loop")).unwrap();
        assert_eq!(
            scope
                .read(&fixture.home.join("loop/SKILL.md"), 20)
                .unwrap_err()
                .io_kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            scope
                .read(&fixture.home.join("missing"), 20)
                .unwrap_err()
                .io_kind(),
            io::ErrorKind::NotFound
        );
        let document = fixture.home.join("skill/SKILL.md");
        assert_eq!(scope.read(&document, 4).unwrap(), b"home");
        assert_eq!(
            scope.read(&document, 3).unwrap_err().io_kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            scope.read(&document, usize::MAX).unwrap_err().io_kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn bound_handles_do_not_follow_replaced_root_names() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        fs::rename(&fixture.home, fixture.temp.path().join("moved-home")).unwrap();
        symlink(&fixture.outside, &fixture.home).unwrap();
        assert_eq!(
            scope
                .read(&fixture.home.join("skill/SKILL.md"), 20)
                .unwrap(),
            b"home"
        );
    }

    #[test]
    fn explicit_scopes_do_not_share_ambient_home_or_contents() {
        let first = Fixture::new();
        let second = Fixture::new();
        fs::write(second.home.join("skill/SKILL.md"), "second").unwrap();
        let first_scope = first.scope();
        let second_scope = second.scope();
        assert_eq!(
            first_scope
                .read(&second.home.join("skill/SKILL.md"), 20)
                .unwrap_err()
                .io_kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            first_scope
                .read(&first.home.join("skill/SKILL.md"), 20)
                .unwrap(),
            b"home"
        );
        assert_eq!(
            second_scope
                .read(&second.home.join("skill/SKILL.md"), 20)
                .unwrap(),
            b"second"
        );
        assert_eq!(
            fs::read(first.outside.join("skill/SKILL.md")).unwrap(),
            b"secret"
        );
        assert_eq!(
            SkillReadScope::bind(&[PathBuf::from("relative")])
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn observed_file_rejects_link_count_and_path_replacement() {
        let fixture = Fixture::new();
        let document = fixture.home.join("document.md");
        fs::write(&document, "first").expect("document");
        let scope = SkillReadScope::bind(std::slice::from_ref(&fixture.home)).expect("scope");
        let observation = scope.observe_regular(&document).expect("observation");
        let link = fixture.home.join("alias.md");
        fs::hard_link(&document, &link).expect("link");
        assert!(matches!(
            scope.read_observed(&observation, 32),
            Err(ScopedReadError::Io(_))
        ));
        fs::remove_file(&link).expect("unlink");
        fs::remove_file(&document).expect("remove");
        fs::write(&document, "replacement").expect("replacement");
        let replacement = scope
            .observe_regular(&document)
            .expect("replacement observation");
        assert_ne!(
            (observation.device, observation.inode),
            (replacement.device, replacement.inode)
        );
        assert!(matches!(
            scope.read_observed(&observation, 32),
            Err(ScopedReadError::Io(_))
        ));
    }
    #[test]
    fn directory_suffixes_and_special_files_are_not_documents() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        for suffix in ["skill/SKILL.md/", "skill/SKILL.md/."] {
            assert!(
                scope.read(&fixture.home.join(suffix), 20).is_err(),
                "{suffix}"
            );
        }
        symlink("skill/SKILL.md/", fixture.home.join("file-link")).unwrap();
        assert!(scope.read(&fixture.home.join("file-link"), 20).is_err());
        let fifo = fixture.home.join("fifo");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        assert_eq!(
            scope.read(&fifo, 20).unwrap_err().io_kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn nested_and_alias_roots_preserve_reads_through_either_entry() {
        let fixture = Fixture::new();
        let alias = fixture.temp.path().join("home-alias");
        symlink(&fixture.home, &alias).unwrap();
        let scope = SkillReadScope::bind(&[
            alias.clone(),
            fixture.home.clone(),
            fixture.home.join("skill/nested"),
        ])
        .unwrap();
        assert_eq!(
            scope.read(&alias.join("skill/SKILL.md"), 20).unwrap(),
            b"home"
        );
        assert_eq!(
            scope
                .read(&fixture.home.join("skill/nested/../SKILL.md"), 20)
                .unwrap(),
            b"home"
        );
        assert_eq!(
            scope
                .read(&fixture.home.join("skill/SKILL.md"), 20)
                .unwrap(),
            b"home"
        );
    }
    #[test]
    fn root_revalidation_accepts_stable_roots_and_content_updates() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        scope.revalidate_roots().unwrap();
        fs::write(fixture.home.join("skill/SKILL.md"), "updated").unwrap();
        scope.revalidate_roots().unwrap();
        assert_eq!(
            scope
                .read(&fixture.home.join("skill/SKILL.md"), 20)
                .unwrap(),
            b"updated"
        );
    }

    #[test]
    fn root_revalidation_detects_replacement_at_the_same_canonical_path() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        fs::rename(&fixture.home, fixture.temp.path().join("old-home")).unwrap();
        fs::create_dir(&fixture.home).unwrap();
        assert!(
            matches!(scope.revalidate_roots(), Err(RootRevalidationError::Changed { root }) if root == fixture.home)
        );
    }

    #[test]
    fn root_revalidation_detects_retargeted_alias_and_missing_root() {
        let fixture = Fixture::new();
        let alias = fixture.temp.path().join("alias");
        symlink(&fixture.home, &alias).unwrap();
        let scope = SkillReadScope::bind(std::slice::from_ref(&alias)).unwrap();
        fs::remove_file(&alias).unwrap();
        symlink(&fixture.backing, &alias).unwrap();
        assert!(
            matches!(scope.revalidate_roots(), Err(RootRevalidationError::Changed { root }) if root == alias)
        );
        fs::remove_file(&alias).unwrap();
        assert!(
            matches!(scope.revalidate_roots(), Err(RootRevalidationError::Unavailable { root, source }) if root == alias && source.kind() == io::ErrorKind::NotFound)
        );
    }

    #[test]
    fn root_revalidation_checks_backing_roots_too() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        fs::rename(&fixture.backing, fixture.temp.path().join("old-backing")).unwrap();
        symlink(&fixture.outside, &fixture.backing).unwrap();
        assert!(
            matches!(scope.revalidate_roots(), Err(RootRevalidationError::Changed { root }) if root == fixture.backing)
        );
        assert_eq!(
            fs::read(fixture.outside.join("skill/SKILL.md")).unwrap(),
            b"secret"
        );
    }
    #[test]
    fn directory_listing_keeps_link_metadata_without_reading_its_target() {
        let fixture = Fixture::new();
        symlink(&fixture.outside, fixture.home.join("outside-link")).unwrap();
        let scope = fixture.scope();
        let result = scope.read_dir(&fixture.home, 10).unwrap();
        assert!(result.issues.is_empty());
        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| entry.name.to_str().unwrap())
                .collect::<Vec<_>>(),
            ["outside-link", "skill"]
        );
        assert!(result.entries[0].metadata.file_type().is_symlink());
        assert!(result.entries[1].metadata.is_dir());
        assert_eq!(
            scope
                .read_dir(&fixture.home.join("outside-link"), 10)
                .unwrap_err()
                .io_kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn directory_listing_follows_only_declared_backing_and_preserves_parent_order() {
        let fixture = Fixture::new();
        symlink(
            fixture.backing.join("skill/nested"),
            fixture.home.join("link"),
        )
        .unwrap();
        let result = fixture
            .scope()
            .read_dir(&fixture.home.join("link/.."), 10)
            .unwrap();
        assert!(result.issues.is_empty());
        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| entry.name.to_str().unwrap())
                .collect::<Vec<_>>(),
            ["SKILL.md", "nested"]
        );
        assert!(result.entries[0].metadata.is_file());
    }

    #[test]
    fn directory_listing_reports_limits_and_sorts_retained_entries() {
        let fixture = Fixture::new();
        let path = fixture.home.join("listing");
        fs::create_dir(&path).unwrap();
        let scope = fixture.scope();
        assert!(scope.read_dir(&path, 0).unwrap().issues.is_empty());
        for name in ["c", "a", "b"] {
            fs::write(path.join(name), name).unwrap();
        }
        for limit in [0, 2] {
            let result = scope.read_dir(&path, limit).unwrap();
            assert_eq!(result.entries.len(), limit);
            assert!(matches!(
                result.issues.as_slice(),
                [DirectoryReadIssue::LimitReached]
            ));
        }
        let result = scope.read_dir(&path, 3).unwrap();
        assert!(result.issues.is_empty());
        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| entry.name.to_str().unwrap())
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(scope.read_dir(&path, 3).unwrap().entries.len(), 3);
    }

    #[test]
    fn directory_listing_preserves_missing_type_and_loop_errors() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        assert_eq!(
            scope
                .read_dir(&fixture.home.join("missing"), 10)
                .unwrap_err()
                .io_kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            scope
                .read_dir(&fixture.home.join("skill/SKILL.md"), 10)
                .unwrap_err()
                .io_kind(),
            io::ErrorKind::InvalidInput
        );
        symlink("loop", fixture.home.join("loop")).unwrap();
        assert_eq!(
            scope
                .read_dir(&fixture.home.join("loop"), 10)
                .unwrap_err()
                .io_kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn directory_listing_reports_observed_drift_without_discarding_entries() {
        let fixture = Fixture::new();
        let directory = Dir::open_ambient_dir(&fixture.home, ambient_authority()).unwrap();
        fs::File::open(&fixture.home)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
            .unwrap();
        let before = directory.dir_metadata().unwrap();
        fs::create_dir(fixture.home.join("added")).unwrap();
        let result = collect_directory(
            &directory,
            &fixture.home,
            &fixture.home,
            before,
            10,
            directory.entries().unwrap(),
        );
        assert!(result.entries.iter().any(|entry| entry.name == "added"));
        assert!(result
            .issues
            .iter()
            .any(|issue| matches!(issue, DirectoryReadIssue::Changed)));
    }

    #[test]
    fn directory_listing_keeps_the_bound_directory_after_name_replacement() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        fs::write(fixture.outside.join("outside-only"), "secret").unwrap();
        fs::rename(&fixture.home, fixture.temp.path().join("old-home")).unwrap();
        symlink(&fixture.outside, &fixture.home).unwrap();
        let result = scope.read_dir(&fixture.home, 10).unwrap();
        assert!(result.issues.is_empty());
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].name, "skill");
        assert!(scope.revalidate_roots().is_err());
    }
    #[test]
    fn directory_entry_and_iterator_failures_preserve_readable_results() {
        let fixture = Fixture::new();
        let path = fixture.home.join("faults");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("good"), "readable").unwrap();
        fs::write(path.join("removed"), "gone before metadata").unwrap();
        let directory = Dir::open_ambient_dir(&path, ambient_authority()).unwrap();
        let before = directory.dir_metadata().unwrap();
        let entries = directory
            .entries()
            .unwrap()
            .inspect(|entry| {
                if entry
                    .as_ref()
                    .is_ok_and(|entry| entry.file_name() == "removed")
                {
                    fs::remove_file(path.join("removed")).unwrap();
                }
            })
            .chain(std::iter::once(Err(io::Error::other(
                "injected iterator failure",
            ))));
        let result = collect_directory(&directory, &path, &path, before, 10, entries);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].name, "good");
        assert!(result.issues.iter().any(|issue| matches!(issue,
            DirectoryReadIssue::Entry { name: Some(name), source }
            if name == "removed" && source.kind() == io::ErrorKind::NotFound
        )));
        assert!(result.issues.iter().any(|issue| matches!(issue,
            DirectoryReadIssue::Entry { name: None, source }
            if source.to_string() == "injected iterator failure"
        )));
        assert!(!result
            .issues
            .iter()
            .any(|issue| matches!(issue, DirectoryReadIssue::LimitReached)));
    }
    #[test]
    fn missing_entries_have_typed_absence_for_file_and_directory_reads() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        for error in [
            scope.read(&fixture.home.join("missing"), 20).unwrap_err(),
            scope
                .read_dir(&fixture.home.join("missing"), 10)
                .unwrap_err(),
        ] {
            assert!(
                matches!(error, ScopedReadError::Missing { path, source } if path.ends_with("missing") && source.kind() == io::ErrorKind::NotFound)
            );
        }
    }

    #[test]
    fn dangling_links_are_failed_targets_even_when_followed_by_a_child() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        for (name, target) in [
            ("local", PathBuf::from("missing")),
            ("absolute-local", fixture.home.join("missing")),
            ("relative-backing", PathBuf::from("../backing/missing")),
            ("absolute-backing", fixture.backing.join("missing")),
        ] {
            symlink(&target, fixture.home.join(name)).unwrap();
            for suffix in ["", "SKILL.md"] {
                let path = if suffix.is_empty() {
                    fixture.home.join(name)
                } else {
                    fixture.home.join(name).join(suffix)
                };
                for error in [
                    scope.read(&path, 20).unwrap_err(),
                    scope.read_dir(&path, 10).unwrap_err(),
                ] {
                    assert!(
                        matches!(error, ScopedReadError::LinkTarget { link, target: actual, source }
                        if link.file_name().unwrap() == name && actual == target && source.kind() == io::ErrorKind::NotFound),
                        "{name}/{suffix}"
                    );
                }
            }
        }
    }

    #[test]
    fn missing_child_after_a_valid_link_is_not_a_failed_link_target() {
        let fixture = Fixture::new();
        symlink("../backing/skill", fixture.home.join("valid")).unwrap();
        let scope = fixture.scope();
        let missing = fixture.home.join("valid/missing");
        assert!(matches!(
            scope.read(&missing, 20),
            Err(ScopedReadError::Missing { .. })
        ));
        assert!(matches!(
            scope.read_dir(&missing, 10),
            Err(ScopedReadError::Missing { .. })
        ));
    }

    #[test]
    fn nested_links_keep_the_origin_of_each_target_component() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        symlink("../backing/skill", fixture.home.join("inner")).unwrap();
        symlink("inner/missing", fixture.home.join("outer")).unwrap();
        assert!(
            matches!(scope.read(&fixture.home.join("outer"), 20), Err(ScopedReadError::LinkTarget { link, .. }) if link.file_name().unwrap() == "outer")
        );
        fs::remove_file(fixture.home.join("outer")).unwrap();
        symlink("inner/nested", fixture.home.join("outer")).unwrap();
        assert!(matches!(
            scope.read_dir(&fixture.home.join("outer/missing"), 10),
            Err(ScopedReadError::Missing { .. })
        ));
        fs::remove_file(fixture.home.join("inner")).unwrap();
        symlink("../backing/missing", fixture.home.join("inner")).unwrap();
        assert!(
            matches!(scope.read(&fixture.home.join("outer/SKILL.md"), 20), Err(ScopedReadError::LinkTarget { link, .. }) if link.file_name().unwrap() == "inner")
        );
    }

    #[test]
    fn target_origin_survives_declared_root_relocation_and_parent_components() {
        let fixture = Fixture::new();
        let nested = fixture.backing.join("skill/nested");
        let scope = SkillReadScope::bind(&[
            fixture.home.clone(),
            fixture.backing.clone(),
            nested.clone(),
        ])
        .unwrap();
        symlink(nested.join("../missing"), fixture.home.join("bad")).unwrap();
        assert!(
            matches!(scope.read_dir(&fixture.home.join("bad"), 10), Err(ScopedReadError::LinkTarget { link, .. }) if link.file_name().unwrap() == "bad")
        );
        symlink("../backing/skill/nested", fixture.home.join("valid")).unwrap();
        assert!(matches!(
            scope.read(&fixture.home.join("valid/../missing"), 20),
            Err(ScopedReadError::Missing { .. })
        ));
        symlink(
            "../backing/skill/nested/../missing",
            fixture.home.join("relative-bad"),
        )
        .unwrap();
        assert!(
            matches!(scope.read(&fixture.home.join("relative-bad"), 20), Err(ScopedReadError::LinkTarget { link, .. }) if link.file_name().unwrap() == "relative-bad")
        );
    }
    #[test]
    fn relative_target_variants_preserve_reads_and_absence_classification() {
        let fixture = Fixture::new();
        let scope = SkillReadScope::bind(&[
            fixture.home.clone(),
            fixture.backing.clone(),
            fixture.backing.join("skill/nested"),
            fixture.home.join("skill/nested"),
        ])
        .unwrap();
        for (index, target) in [
            "../backing/skill/nested/../",
            "../backing/skill/nested/..",
            "../backing/skill/nested/./../",
            "../backing/skill/nested/../nested/..",
            "../backing/skill/nested/../../skill",
            ".././backing/skill",
            "../backing/skill/",
            "../backing/skill/.",
            "../backing/skill/nested/../../skill/.",
        ]
        .into_iter()
        .enumerate()
        {
            let name = format!("variant-{index}");
            let link = fixture.home.join(&name);
            symlink(target, &link).unwrap();
            assert_eq!(
                scope.read(&link.join("SKILL.md"), 20).unwrap(),
                b"backing",
                "{target}"
            );
            assert!(
                scope.read_dir(&link, 10).unwrap().issues.is_empty(),
                "{target}"
            );
            assert!(
                matches!(
                    scope.read(&link.join("missing"), 20),
                    Err(ScopedReadError::Missing { .. })
                ),
                "{target}"
            );
            assert!(
                matches!(
                    scope.read_dir(&link.join("missing"), 10),
                    Err(ScopedReadError::Missing { .. })
                ),
                "{target}"
            );
            fs::remove_file(&link).unwrap();
            symlink(Path::new(target).join("missing"), &link).unwrap();
            assert!(
                matches!(
                    scope.read(&link, 20),
                    Err(ScopedReadError::LinkTarget { .. })
                ),
                "{target}"
            );
            assert!(
                matches!(
                    scope.read_dir(&link, 10),
                    Err(ScopedReadError::LinkTarget { .. })
                ),
                "{target}"
            );
        }
    }

    #[test]
    fn checked_prefix_rejects_a_retargeted_name() {
        let fixture = Fixture::new();
        let link = fixture.home.join("prefix-link");
        let first = fixture.home.join("skill/SKILL.md");
        let second = fixture.home.join("skill/other.md");
        fs::write(&second, "other").unwrap();
        symlink(&first, &link).unwrap();
        let scope = fixture.scope();
        let mut checks = 0;
        let result = scope.read_prefix_checked(&link, 64, &mut || {
            checks += 1;
            if checks == 2 {
                fs::remove_file(&link).unwrap();
                symlink(&second, &link).unwrap();
            }
            Ok(())
        });
        assert!(matches!(result, Err(ScopedContentFoldError::Changed)));
    }

    #[test]
    fn observed_directory_resolution_keeps_lexical_identity() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        let relative = fixture.home.join("relative-dir");
        let absolute = fixture.home.join("absolute-dir");
        let file = fixture.home.join("file-link");
        let broken = fixture.home.join("broken-dir");
        let looped = fixture.home.join("loop-dir");
        symlink("../backing/skill", &relative).unwrap();
        symlink(fixture.backing.join("skill"), &absolute).unwrap();
        symlink(fixture.backing.join("skill/SKILL.md"), &file).unwrap();
        symlink(fixture.backing.join("missing"), &broken).unwrap();
        symlink("loop-dir", &looped).unwrap();
        let resolved_skill = fs::canonicalize(fixture.backing.join("skill")).unwrap();

        for path in [&relative, &absolute] {
            let observation = scope
                .observe_entry(path.parent().unwrap(), path.file_name().unwrap())
                .unwrap();
            assert_eq!(
                scope.resolve_observed_dir(&observation).unwrap(),
                resolved_skill
            );
        }
        let file_observation = scope
            .observe_entry(file.parent().unwrap(), file.file_name().unwrap())
            .unwrap();
        assert_eq!(
            scope
                .resolve_observed_dir(&file_observation)
                .unwrap_err()
                .io_kind(),
            io::ErrorKind::InvalidInput
        );
        let broken_observation = scope
            .observe_entry(broken.parent().unwrap(), broken.file_name().unwrap())
            .unwrap();
        assert!(matches!(
            scope.resolve_observed_dir(&broken_observation),
            Err(ScopedReadError::LinkTarget { .. })
        ));
        let loop_observation = scope
            .observe_entry(looped.parent().unwrap(), looped.file_name().unwrap())
            .unwrap();
        assert_eq!(
            scope
                .resolve_observed_dir(&loop_observation)
                .unwrap_err()
                .io_kind(),
            io::ErrorKind::InvalidData
        );

        let retarget = fixture.home.join("retarget-dir");
        symlink(fixture.backing.join("skill"), &retarget).unwrap();
        let observation = scope
            .observe_entry(retarget.parent().unwrap(), retarget.file_name().unwrap())
            .unwrap();
        fs::remove_file(&retarget).unwrap();
        symlink(fixture.home.join("skill"), &retarget).unwrap();
        let error = scope.resolve_observed_dir(&observation).unwrap_err();
        assert!(scoped_error_is_binding_change(&error));
    }

    #[test]
    fn directory_listing_retains_a_link_target_read_failure() {
        let fixture = Fixture::new();
        let link = fixture.home.join("unreadable-link-target");
        symlink(fixture.backing.join("skill"), &link).unwrap();
        let scope = fixture.scope();
        let _failure = SkillReadScope::inject_entry_link_read_failure(libc::EIO);

        let mut listing = scope.read_dir(&fixture.home, 10).unwrap();
        let entry = listing
            .entries
            .drain(..)
            .find(|entry| entry.name == "unreadable-link-target")
            .unwrap();
        assert_eq!(
            entry.raw_link_target.as_ref().unwrap_err().raw_os_error(),
            Some(libc::EIO)
        );
        let error = scope
            .resolve_observed_dir(&entry.into_observation())
            .unwrap_err();
        assert!(matches!(
            error,
            ScopedReadError::Io(source) if source.raw_os_error() == Some(libc::EIO)
        ));
    }
}

#[cfg(test)]
mod content_directory_identity_prototype {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    struct RetainedDirectory<'scope> {
        scope: &'scope SkillReadScope,
        entry: Arc<ScopedEntryObservation>,
        source: OpenedSource,
    }

    impl<'scope> RetainedDirectory<'scope> {
        fn retain(
            scope: &'scope SkillReadScope,
            entry: Arc<ScopedEntryObservation>,
        ) -> Result<Self, ScopedReadError> {
            if let Err(source) = &entry.raw_link_target {
                let source = source.raw_os_error().map_or_else(
                    || io::Error::new(source.kind(), source.to_string()),
                    io::Error::from_raw_os_error,
                );
                return Err(ScopedReadError::Io(source));
            }
            scope.validate_entry_observation_with_parent_change(&entry, false)?;
            let source = scope.resolve(&entry.requested)?;
            if !source.metadata.is_dir() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a directory").into());
            }
            scope.validate_entry_observation_with_parent_change(&entry, false)?;
            Ok(Self {
                scope,
                entry,
                source,
            })
        }

        fn revalidate(&self) -> Result<(), ScopedReadError> {
            self.scope
                .validate_entry_observation_with_parent_change(&self.entry, false)?;
            let current = self.scope.resolve(&self.entry.requested)?;
            let retained = self.source.file.metadata()?;
            if current.resolved != self.source.resolved
                || !current.metadata.is_dir()
                || retained.dev() != current.metadata.dev()
                || retained.ino() != current.metadata.ino()
                || !unchanged(&self.source.metadata, &current.metadata)
            {
                return Err(changed().into());
            }
            self.scope
                .validate_entry_observation_with_parent_change(&self.entry, false)
        }

        fn same_directory(&self, other: &Self) -> Result<bool, ScopedReadError> {
            self.revalidate()?;
            other.revalidate()?;
            Ok(std::ptr::eq(self.scope, other.scope)
                && self.source.metadata.dev() == other.source.metadata.dev()
                && self.source.metadata.ino() == other.source.metadata.ino())
        }
    }

    #[derive(Clone)]
    struct ContentId {
        operation: Arc<()>,
        index: usize,
    }

    enum Admission {
        Retained(ContentId),
        Independent,
    }

    struct IdentityPool<'scope> {
        scope: &'scope SkillReadScope,
        operation: Arc<()>,
        entries: Vec<RetainedDirectory<'scope>>,
        directory_limit: usize,
        path_byte_limit: usize,
        retained_path_bytes: usize,
    }

    impl<'scope> IdentityPool<'scope> {
        fn new(
            scope: &'scope SkillReadScope,
            directory_limit: usize,
            path_byte_limit: usize,
        ) -> Self {
            Self {
                scope,
                operation: Arc::new(()),
                entries: Vec::new(),
                directory_limit,
                path_byte_limit,
                retained_path_bytes: 0,
            }
        }

        fn admit(
            &mut self,
            entry: Arc<ScopedEntryObservation>,
        ) -> Result<Admission, ScopedReadError> {
            if self.directory_limit == 0 || self.path_byte_limit == 0 {
                return Ok(Admission::Independent);
            }
            let candidate = RetainedDirectory::retain(self.scope, entry)?;
            for (index, retained) in self.entries.iter().enumerate() {
                if candidate.same_directory(retained)? {
                    return Ok(Admission::Retained(ContentId {
                        operation: Arc::clone(&self.operation),
                        index,
                    }));
                }
            }
            let paths = [
                candidate.source.resolved.as_path(),
                candidate.entry.requested.as_path(),
                candidate.entry.parent_resolved.as_path(),
            ];
            let extra = paths
                .iter()
                .try_fold(0usize, |total, path| {
                    total.checked_add(path.as_os_str().len())
                })
                .and_then(|total| {
                    let link_bytes = candidate
                        .entry
                        .raw_link_target
                        .as_ref()
                        .ok()
                        .and_then(Option::as_ref)
                        .map_or(0, |path| path.as_os_str().len());
                    total.checked_add(link_bytes)
                });
            let Some(total) = extra.and_then(|extra| self.retained_path_bytes.checked_add(extra))
            else {
                return Ok(Admission::Independent);
            };
            if self.entries.len() >= self.directory_limit || total > self.path_byte_limit {
                return Ok(Admission::Independent);
            }
            let index = self.entries.len();
            self.entries.push(candidate);
            self.retained_path_bytes = total;
            Ok(Admission::Retained(ContentId {
                operation: Arc::clone(&self.operation),
                index,
            }))
        }

        fn get(&self, id: &ContentId) -> Result<&RetainedDirectory<'scope>, ScopedReadError> {
            if !Arc::ptr_eq(&self.operation, &id.operation) {
                return Err(changed().into());
            }
            let retained = self.entries.get(id.index).ok_or_else(changed)?;
            retained.revalidate()?;
            Ok(retained)
        }
    }

    #[test]
    fn identity_pool_does_not_promote_failed_link_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let target = root.join("target");
        fs::create_dir(&target).unwrap();
        let alias = root.join("alias");
        symlink(&target, &alias).unwrap();
        let scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
        let entry = {
            let _failure = SkillReadScope::inject_entry_link_read_failure(libc::EIO);
            observe(&scope, &alias)
        };
        assert!(entry.raw_link_target.is_err());
        assert!(scope.resolved_dir_path(&alias).is_ok());
        let mut pool = IdentityPool::new(&scope, 1, usize::MAX);
        assert!(pool.admit(entry).is_err());
        assert!(pool.entries.is_empty());
        assert_eq!(pool.retained_path_bytes, 0);
    }

    #[test]
    fn identity_pool_bounds_retention_and_rejects_foreign_or_stale_ids() {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let first = root.join("first");
        let second = root.join("second");
        let alias = root.join("alias");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        symlink(&first, &alias).unwrap();
        let scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
        let mut pool = IdentityPool::new(&scope, 1, usize::MAX);
        let Admission::Retained(id) = pool.admit(observe(&scope, &first)).unwrap() else {
            panic!("first admission");
        };
        let bytes = pool.retained_path_bytes;
        let Admission::Retained(linked) = pool.admit(observe(&scope, &alias)).unwrap() else {
            panic!("alias admission");
        };
        assert_eq!(linked.index, id.index);
        assert_eq!(pool.entries.len(), 1);
        assert_eq!(pool.retained_path_bytes, bytes);
        assert!(matches!(
            pool.admit(observe(&scope, &second)).unwrap(),
            Admission::Independent
        ));
        assert_eq!(pool.entries.len(), 1);
        assert!(pool.get(&id).is_ok());
        let other = IdentityPool::new(&scope, 1, usize::MAX);
        assert!(other.get(&id).is_err());
        let mut bounded = IdentityPool::new(&scope, 2, bytes - 1);
        assert!(matches!(
            bounded.admit(observe(&scope, &first)).unwrap(),
            Admission::Independent
        ));
        assert_eq!(bounded.retained_path_bytes, 0);
        assert!(bounded.entries.is_empty());
        let mut disabled = IdentityPool::new(&scope, 0, usize::MAX);
        assert!(matches!(
            disabled.admit(observe(&scope, &first)).unwrap(),
            Admission::Independent
        ));
        let weak_entry = Arc::downgrade(&pool.entries[0].entry);
        fs::rename(&first, root.join("old")).unwrap();
        fs::create_dir(&first).unwrap();
        assert!(pool.get(&id).is_err());
        assert!(pool.admit(observe(&scope, &first)).is_err());
        drop(pool);
        assert!(weak_entry.upgrade().is_none());
        assert!(other.get(&id).is_err());
    }

    enum IdentityAdmissionFailure {
        Coordination(crate::skill_coordination::CoordinationFailure),
        Source(ScopedReadError),
        Content(ScopedContentFoldError),
        PlanLimit,
        Folder(crate::skill_discovery::content_folder_probe::Failure),
    }

    impl From<crate::skill_coordination::CoordinationFailure> for IdentityAdmissionFailure {
        fn from(error: crate::skill_coordination::CoordinationFailure) -> Self {
            Self::Coordination(error)
        }
    }

    struct PreparedContentFiles {
        id: ContentId,
        files: Vec<ScopedContentObservation>,
        byte_limit: u64,
    }

    struct GuardedContentRead<'scope> {
        guard: crate::skill_coordination::CoordinatedReadGuard,
        pool: IdentityPool<'scope>,
        prepared: PreparedContentFiles,
    }

    impl GuardedContentRead<'_> {
        fn read_all(&self) -> Result<Vec<Vec<u8>>, IdentityAdmissionFailure> {
            self.guard.check_cancelled()?;
            self.guard.revalidate(self.pool.scope)?;
            self.pool
                .get(&self.prepared.id)
                .map_err(IdentityAdmissionFailure::Source)?;
            let mut remaining = self.prepared.byte_limit;
            let mut result = Vec::with_capacity(self.prepared.files.len());
            for file in &self.prepared.files {
                let limit = usize::try_from(file.len().min(remaining))
                    .map_err(|_| IdentityAdmissionFailure::PlanLimit)?;
                let bytes = self
                    .guard
                    .read_content(self.pool.scope, file, limit)
                    .map_err(IdentityAdmissionFailure::Content)?;
                remaining = remaining
                    .checked_sub(bytes.len() as u64)
                    .ok_or(IdentityAdmissionFailure::PlanLimit)?;
                result.push(bytes);
            }
            self.guard.revalidate(self.pool.scope)?;
            self.pool
                .get(&self.prepared.id)
                .map_err(IdentityAdmissionFailure::Source)?;
            self.guard.check_cancelled()?;
            Ok(result)
        }
    }

    #[test]
    fn pooled_content_handoff_preserves_prepared_bytes_and_read_budget() {
        use crate::skill_coordination::{
            CancellationToken, CoordinationFailure, CoordinationMode, CoordinationPlan,
            DirectoryEffect,
        };
        use std::time::Duration;
        for case in [
            "read",
            "drift-before",
            "drift-after",
            "cancel-after",
            "limits",
            "external",
            "hardlink",
            "escape",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(temp.path()).unwrap();
            let home = root.join("home");
            let outside = root.join("outside");
            let skill = home.join("skill");
            fs::create_dir_all(&skill).unwrap();
            fs::create_dir(&outside).unwrap();
            let document = skill.join("SKILL.md");
            let resource = skill.join("resource.txt");
            fs::write(&document, "doc").unwrap();
            fs::write(&resource, "resource").unwrap();
            if matches!(case, "external" | "hardlink" | "escape") {
                let target = outside.join("resource.txt");
                fs::write(&target, "resource").unwrap();
                fs::remove_file(&resource).unwrap();
                if case == "hardlink" {
                    fs::hard_link(&target, &resource).unwrap();
                } else {
                    symlink(&target, &resource).unwrap();
                }
            }
            let mut roots = vec![home.clone()];
            if matches!(case, "external" | "hardlink") {
                roots.push(outside.clone());
            }
            let scope = SkillReadScope::bind(&roots).unwrap();
            let token = CancellationToken::default();
            let guard = CoordinationPlan::new_cancellable(
                vec![DirectoryEffect::tree(
                    home.clone(),
                    CoordinationMode::Shared,
                )],
                Some(Duration::from_secs(10)),
                token.clone(),
            )
            .unwrap()
            .acquire()
            .unwrap()
            .continue_with_files(&scope, &[], CoordinationMode::Shared)
            .unwrap();
            let session = CoordinatedIdentitySession {
                guard,
                pool: IdentityPool::new(&scope, 1, 4096),
            };
            let Ok((session, Admission::Retained(id))) = session.admit(observe(&scope, &skill))
            else {
                panic!("admission");
            };
            if case == "limits" {
                assert!(matches!(
                    session.prepare_files(&id, &[Path::new("SKILL.md")], 2),
                    Err(IdentityAdmissionFailure::PlanLimit)
                ));
                assert!(matches!(
                    session.prepare_files(&id, &[Path::new("../other")], 20),
                    Err(IdentityAdmissionFailure::PlanLimit)
                ));
                assert!(matches!(
                    session.prepare_files(&id, &vec![Path::new("SKILL.md"); 129], 1000),
                    Err(IdentityAdmissionFailure::PlanLimit)
                ));
            }
            if case == "escape" {
                assert!(matches!(
                    session.prepare_files(&id, &[Path::new("resource.txt")], 20),
                    Err(IdentityAdmissionFailure::Source(_))
                ));
                drop(session);
                assert!(CoordinationPlan::new(
                    vec![DirectoryEffect::tree(home, CoordinationMode::Exclusive)],
                    Some(Duration::from_secs(1))
                )
                .unwrap()
                .acquire()
                .is_ok());
                continue;
            }
            let prepared = match session.prepare_files(
                &id,
                &[Path::new("SKILL.md"), Path::new("resource.txt")],
                11,
            ) {
                Ok(value) => value,
                Err(_) => panic!("prepare"),
            };
            if case == "drift-before" {
                fs::write(&document, "changed").unwrap();
            }
            let read = session.finalize_files(prepared);
            if case == "drift-before" {
                assert!(matches!(
                    read,
                    Err(IdentityAdmissionFailure::Content(
                        ScopedContentFoldError::Changed
                    ))
                ));
            } else {
                let read = match read {
                    Ok(value) => value,
                    Err(_) => panic!("finalize"),
                };
                if case == "drift-after" {
                    fs::write(&resource, "changed resource").unwrap();
                }
                if case == "cancel-after" {
                    token.cancel();
                }
                match (case, read.read_all()) {
                    ("read" | "limits" | "external" | "hardlink", Ok(bytes)) => {
                        assert_eq!(bytes, vec![b"doc".to_vec(), b"resource".to_vec()])
                    }
                    (
                        "drift-after",
                        Err(IdentityAdmissionFailure::Coordination(CoordinationFailure::Changed)),
                    ) => {}
                    (
                        "cancel-after",
                        Err(IdentityAdmissionFailure::Coordination(CoordinationFailure::Cancelled)),
                    ) => {}
                    _ => panic!("unexpected read result for {case}"),
                }
            }
            assert!(CoordinationPlan::new(
                vec![DirectoryEffect::tree(home, CoordinationMode::Exclusive)],
                Some(Duration::from_secs(1))
            )
            .unwrap()
            .acquire()
            .is_ok());
        }
    }

    struct ContentDeployment {
        entry: Arc<ScopedEntryObservation>,
        content: ContentId,
    }

    impl ReadFolder<'_> {
        fn deployment_facts(
            &self,
            deployment: &ContentDeployment,
        ) -> Result<&crate::skill_discovery::content_folder_probe::Facts, IdentityAdmissionFailure>
        {
            self.revalidate()?;
            let expected = self
                .pool
                .get(&deployment.content)
                .map_err(IdentityAdmissionFailure::Source)?;
            if deployment.content.index != self.id.index {
                return Err(crate::skill_coordination::CoordinationFailure::Changed.into());
            }
            let current = RetainedDirectory::retain(self.pool.scope, Arc::clone(&deployment.entry))
                .map_err(IdentityAdmissionFailure::Source)?;
            if !current
                .same_directory(expected)
                .map_err(IdentityAdmissionFailure::Source)?
            {
                return Err(crate::skill_coordination::CoordinationFailure::Changed.into());
            }
            self.revalidate()?;
            Ok(&self.content.facts)
        }
    }

    #[test]
    fn shared_folder_keeps_each_deployments_route_evidence() {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        use std::time::Duration;

        for changed in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(temp.path()).unwrap();
            let backing = root.join("backing");
            let other = root.join("other");
            let first = root.join("claude/alpha");
            let second = root.join("codex/alias");
            for directory in [
                &backing,
                &other,
                first.parent().unwrap(),
                second.parent().unwrap(),
            ] {
                fs::create_dir_all(directory).unwrap();
            }
            fs::write(
                backing.join("SKILL.md"),
                "---\nname: alpha\ndescription: shared fixture\n---\nbody\n",
            )
            .unwrap();
            fs::write(backing.join("resource.txt"), "resource").unwrap();
            symlink(&backing, &first).unwrap();
            symlink(&backing, &second).unwrap();
            let scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
            let guard = CoordinationPlan::new(
                vec![DirectoryEffect::tree(
                    root.clone(),
                    CoordinationMode::Shared,
                )],
                Some(Duration::from_secs(10)),
            )
            .unwrap()
            .acquire()
            .unwrap()
            .continue_with_files(&scope, &[], CoordinationMode::Shared)
            .unwrap();
            let session = CoordinatedIdentitySession {
                guard,
                pool: IdentityPool::new(&scope, 1, 4096),
            };
            let first_entry = observe(&scope, &first);
            let second_entry = observe(&scope, &second);
            let Ok((session, Admission::Retained(first_id))) =
                session.admit(Arc::clone(&first_entry))
            else {
                panic!("first admission");
            };
            let Ok((session, Admission::Retained(second_id))) =
                session.admit(Arc::clone(&second_entry))
            else {
                panic!("second admission");
            };
            assert_eq!(first_id.index, second_id.index);
            assert_eq!(session.pool.entries.len(), 1);
            let first_deployment = ContentDeployment {
                entry: first_entry,
                content: first_id,
            };
            let second_deployment = ContentDeployment {
                entry: second_entry,
                content: second_id,
            };
            let Ok(prepared) = session.prepare_folder(&first_deployment.content) else {
                panic!("single folder preparation");
            };
            let Ok(read) = session.finalize_folder(prepared) else {
                panic!("single folder materialization");
            };
            let Ok(first_facts) = read.deployment_facts(&first_deployment) else {
                panic!("first projection");
            };
            let Ok(second_facts) = read.deployment_facts(&second_deployment) else {
                panic!("second projection");
            };
            assert!(std::ptr::eq(first_facts, second_facts));
            assert_ne!(
                first_deployment.entry.requested,
                second_deployment.entry.requested
            );
            if changed {
                fs::remove_file(&second).unwrap();
                symlink(&other, &second).unwrap();
                // The shared backing evidence stays valid; the alias evidence does not.
                assert!(read.revalidate().is_ok());
                assert!(read.deployment_facts(&first_deployment).is_ok());
                assert!(read.deployment_facts(&second_deployment).is_err());
            }
            drop(read);
            assert!(CoordinationPlan::new(
                vec![DirectoryEffect::tree(root, CoordinationMode::Exclusive)],
                Some(Duration::from_secs(1)),
            )
            .unwrap()
            .acquire()
            .is_ok());
        }
    }

    struct PreparedFolder {
        id: ContentId,
        plan: crate::skill_discovery::content_folder_probe::Plan,
    }

    struct ReadFolder<'scope> {
        pool: IdentityPool<'scope>,
        guard: crate::skill_coordination::CoordinatedReadGuard,
        id: ContentId,
        content: crate::skill_discovery::content_folder_probe::Read,
    }

    impl ReadFolder<'_> {
        fn revalidate(&self) -> Result<(), IdentityAdmissionFailure> {
            self.guard.check_cancelled()?;
            self.guard.revalidate(self.pool.scope)?;
            self.pool
                .get(&self.id)
                .map_err(IdentityAdmissionFailure::Source)?;
            self.content.revalidate(self.pool.scope)?;
            Ok(())
        }
    }

    #[test]
    fn full_folder_probe_keeps_resource_membership_and_spec_evidence() {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        use std::time::Duration;
        for case in [
            "stable",
            "new-resource",
            "new-spec",
            "late-resource",
            "incomplete",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(temp.path()).unwrap();
            let skill = root.join("skill");
            fs::create_dir(&skill).unwrap();
            fs::create_dir(skill.join("nested")).unwrap();
            fs::write(
                skill.join("SKILL.md"),
                "---\nname: skill\ndescription: fixture\n---\nbody\n",
            )
            .unwrap();
            fs::write(skill.join("nested/resource.txt"), "resource").unwrap();
            if case == "incomplete" {
                symlink("missing", skill.join("broken")).unwrap();
            }
            let scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
            let guard = CoordinationPlan::new(
                vec![DirectoryEffect::tree(
                    root.clone(),
                    CoordinationMode::Shared,
                )],
                Some(Duration::from_secs(10)),
            )
            .unwrap()
            .acquire()
            .unwrap()
            .continue_with_files(&scope, &[], CoordinationMode::Shared)
            .unwrap();
            let session = CoordinatedIdentitySession {
                guard,
                pool: IdentityPool::new(&scope, 1, 4096),
            };
            let Ok((session, Admission::Retained(id))) = session.admit(observe(&scope, &skill))
            else {
                panic!("admission");
            };
            let prepared = match session.prepare_folder(&id) {
                Ok(value) => value,
                Err(_) => panic!("prepare"),
            };
            if case == "new-resource" {
                fs::write(skill.join("nested/new.txt"), "late").unwrap();
            }
            if case == "new-spec" {
                fs::write(skill.join("spec.md"), "late spec").unwrap();
            }
            let read = session.finalize_folder(prepared);
            match (case, read) {
                ("stable" | "late-resource", Ok(read)) => {
                    assert_eq!(read.content.facts.file_count, 2);
                    assert!(!read.content.facts.has_spec);
                    assert!(!read.content.facts.content_hash.is_empty());
                    assert!(read.content.facts.skill_md_tokens > 0);
                    assert!(read.revalidate().is_ok());
                    if case == "late-resource" {
                        fs::write(skill.join("nested/new.txt"), "late").unwrap();
                        assert!(read.revalidate().is_err());
                    }
                }
                (
                    "new-resource",
                    Err(IdentityAdmissionFailure::Folder(
                        crate::skill_discovery::content_folder_probe::Failure::Coordination(error),
                    )),
                ) => assert!(matches!(
                    error,
                    crate::skill_coordination::CoordinationFailure::Changed
                )),
                ("new-spec", Err(_)) => {}
                (
                    "incomplete",
                    Err(IdentityAdmissionFailure::Folder(
                        crate::skill_discovery::content_folder_probe::Failure::Incomplete(issues),
                    )),
                ) => assert!(!issues.is_empty()),
                _ => panic!("unexpected folder result for {case}"),
            }
            assert!(CoordinationPlan::new(
                vec![DirectoryEffect::tree(root, CoordinationMode::Exclusive)],
                Some(Duration::from_secs(1))
            )
            .unwrap()
            .acquire()
            .is_ok());
        }
    }

    struct CoordinatedIdentitySession<'scope> {
        guard: crate::skill_coordination::CoordinatedReadGuard,
        pool: IdentityPool<'scope>,
    }

    impl<'scope> CoordinatedIdentitySession<'scope> {
        fn prepare_folder(
            &self,
            id: &ContentId,
        ) -> Result<PreparedFolder, IdentityAdmissionFailure> {
            self.guard.check_identity_admission(self.pool.scope)?;
            let directory = self
                .pool
                .get(id)
                .map_err(IdentityAdmissionFailure::Source)?;
            let plan = crate::skill_discovery::content_folder_probe::Plan::enumerate(
                self.pool.scope,
                &directory.source.resolved,
                &self.guard,
            )
            .map_err(IdentityAdmissionFailure::Folder)?;
            self.pool
                .get(id)
                .map_err(IdentityAdmissionFailure::Source)?;
            self.guard.check_identity_admission(self.pool.scope)?;
            Ok(PreparedFolder {
                id: id.clone(),
                plan,
            })
        }

        fn finalize_folder(
            self,
            prepared: PreparedFolder,
        ) -> Result<ReadFolder<'scope>, IdentityAdmissionFailure> {
            self.guard.check_identity_admission(self.pool.scope)?;
            self.pool
                .get(&prepared.id)
                .map_err(IdentityAdmissionFailure::Source)?;
            let paths = prepared.plan.regular_files();
            let Self { guard, pool } = self;
            let guard = guard.extend_with_files(pool.scope, &paths)?;
            let content = prepared
                .plan
                .materialize(pool.scope, &guard)
                .map_err(IdentityAdmissionFailure::Folder)?;
            let read = ReadFolder {
                pool,
                guard,
                id: prepared.id,
                content,
            };
            read.revalidate()?;
            Ok(read)
        }

        fn prepare_files(
            &self,
            id: &ContentId,
            relative: &[&Path],
            byte_limit: u64,
        ) -> Result<PreparedContentFiles, IdentityAdmissionFailure> {
            self.guard.check_identity_admission(self.pool.scope)?;
            if relative.len() > 128 {
                return Err(IdentityAdmissionFailure::PlanLimit);
            }
            let retained = self
                .pool
                .get(id)
                .map_err(IdentityAdmissionFailure::Source)?;
            let mut files = Vec::with_capacity(relative.len());
            let mut total = 0u64;
            for path in relative {
                if path.as_os_str().is_empty()
                    || path
                        .components()
                        .any(|component| !matches!(component, Component::Normal(_)))
                {
                    return Err(IdentityAdmissionFailure::PlanLimit);
                }
                let file = self
                    .pool
                    .scope
                    .observe_content_regular(&retained.source.resolved.join(path))
                    .map_err(IdentityAdmissionFailure::Source)?;
                total = total
                    .checked_add(file.len())
                    .filter(|total| *total <= byte_limit)
                    .ok_or(IdentityAdmissionFailure::PlanLimit)?;
                files.push(file);
            }
            self.pool
                .get(id)
                .map_err(IdentityAdmissionFailure::Source)?;
            self.guard.check_identity_admission(self.pool.scope)?;
            Ok(PreparedContentFiles {
                id: id.clone(),
                files,
                byte_limit,
            })
        }

        fn finalize_files(
            self,
            prepared: PreparedContentFiles,
        ) -> Result<GuardedContentRead<'scope>, IdentityAdmissionFailure> {
            self.guard.check_identity_admission(self.pool.scope)?;
            self.pool
                .get(&prepared.id)
                .map_err(IdentityAdmissionFailure::Source)?;
            let paths = prepared
                .files
                .iter()
                .map(|file| file.requested.clone())
                .collect::<Vec<_>>();
            let Self { guard, pool } = self;
            let guard = guard.extend_with_files(pool.scope, &paths)?;
            for file in &prepared.files {
                guard
                    .check_content_observation(file)
                    .map_err(IdentityAdmissionFailure::Content)?;
            }
            pool.get(&prepared.id)
                .map_err(IdentityAdmissionFailure::Source)?;
            guard.revalidate(pool.scope)?;
            Ok(GuardedContentRead {
                guard,
                pool,
                prepared,
            })
        }

        fn admit(
            mut self,
            entry: Arc<ScopedEntryObservation>,
        ) -> Result<(Self, Admission), IdentityAdmissionFailure> {
            self.guard.check_identity_admission(self.pool.scope)?;
            if !self
                .guard
                .covers_identity_directory(self.pool.scope, &entry.requested)?
            {
                return Ok((self, Admission::Independent));
            }
            let result = self
                .pool
                .admit(entry)
                .map_err(IdentityAdmissionFailure::Source)?;
            self.guard.check_identity_admission(self.pool.scope)?;
            Ok((self, result))
        }
    }

    #[test]
    fn coordinated_identity_admission_preserves_coverage_cancellation_and_deadline() {
        use crate::skill_coordination::{
            CancellationToken, CoordinationFailure, CoordinationMode, CoordinationPlan,
            DirectoryEffect,
        };
        use std::time::Duration;
        for case in [
            "covered",
            "uncovered",
            "cancelled",
            "expired",
            "cancelled-full",
            "failed-link",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(temp.path()).unwrap();
            let tree = root.join("tree");
            let skill = tree.join("skill");
            let outside = root.join("outside");
            fs::create_dir_all(&skill).unwrap();
            fs::create_dir(&outside).unwrap();
            let alias = tree.join("alias");
            symlink(&outside, &alias).unwrap();
            let internal_alias = tree.join("internal-alias");
            symlink(&skill, &internal_alias).unwrap();
            let scope = SkillReadScope::bind(&[tree.clone(), outside.clone()]).unwrap();
            let token = CancellationToken::default();
            let budget = if case == "expired" {
                Duration::from_millis(500)
            } else {
                Duration::from_secs(10)
            };
            let guard = CoordinationPlan::new_cancellable(
                vec![DirectoryEffect::tree(
                    tree.clone(),
                    CoordinationMode::Shared,
                )],
                Some(budget),
                token.clone(),
            )
            .unwrap()
            .acquire()
            .unwrap()
            .continue_with_files(&scope, &[], CoordinationMode::Shared)
            .unwrap();
            let session = CoordinatedIdentitySession {
                guard,
                pool: IdentityPool::new(&scope, usize::from(case != "cancelled-full"), usize::MAX),
            };
            let entry = if case == "failed-link" {
                let _failure = SkillReadScope::inject_entry_link_read_failure(libc::EIO);
                observe(&scope, &internal_alias)
            } else {
                observe(&scope, if case == "uncovered" { &alias } else { &skill })
            };
            if case.starts_with("cancelled") {
                token.cancel();
            }
            if case == "expired" {
                std::thread::sleep(Duration::from_millis(550));
            }
            match (case, session.admit(entry)) {
                ("covered", Ok((session, Admission::Retained(id)))) => {
                    assert!(session.pool.get(&id).is_ok())
                }
                ("uncovered", Ok((session, Admission::Independent))) => {
                    assert!(session.pool.entries.is_empty())
                }
                (
                    "cancelled" | "cancelled-full",
                    Err(IdentityAdmissionFailure::Coordination(CoordinationFailure::Cancelled)),
                ) => {}
                (
                    "expired",
                    Err(IdentityAdmissionFailure::Coordination(
                        CoordinationFailure::DeadlineExceeded,
                    )),
                ) => {}
                (
                    "failed-link",
                    Err(IdentityAdmissionFailure::Source(ScopedReadError::Io(source))),
                ) => assert_eq!(source.raw_os_error(), Some(libc::EIO)),
                _ => panic!("unexpected admission result for {case}"),
            }
            let writer = CoordinationPlan::new(
                vec![DirectoryEffect::tree(tree, CoordinationMode::Exclusive)],
                Some(Duration::from_secs(1)),
            )
            .unwrap()
            .acquire();
            assert!(writer.is_ok(), "session guard was not released for {case}");
        }
    }

    fn observe(scope: &SkillReadScope, path: &Path) -> Arc<ScopedEntryObservation> {
        Arc::new(
            scope
                .observe_entry(path.parent().unwrap(), path.file_name().unwrap())
                .unwrap(),
        )
    }

    #[test]
    fn retained_content_identity_is_scope_local_and_distinguishes_copies() {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let content = root.join("content");
        let copy = root.join("copy");
        fs::create_dir(&content).unwrap();
        fs::create_dir(&copy).unwrap();
        fs::write(content.join("SKILL.md"), "same content").unwrap();
        fs::write(copy.join("SKILL.md"), "same content").unwrap();
        let alias = root.join("alias");
        symlink(&content, &alias).unwrap();
        let scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
        let other_scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
        let direct_entry = observe(&scope, &content);
        let alias_entry = observe(&scope, &alias);
        let copy_entry = observe(&scope, &copy);
        let other_entry = observe(&other_scope, &content);
        let direct = RetainedDirectory::retain(&scope, Arc::clone(&direct_entry)).unwrap();
        let linked = RetainedDirectory::retain(&scope, Arc::clone(&alias_entry)).unwrap();
        let copied = RetainedDirectory::retain(&scope, Arc::clone(&copy_entry)).unwrap();
        let separate = RetainedDirectory::retain(&other_scope, Arc::clone(&other_entry)).unwrap();
        assert!(direct.same_directory(&linked).unwrap());
        assert!(!direct.same_directory(&copied).unwrap());
        assert!(!direct.same_directory(&separate).unwrap());
    }

    #[test]
    fn retained_content_identity_rejects_stale_and_unauthorized_sources() {
        for change in [
            "retarget",
            "replace",
            "resource-change",
            "escape",
            "dangling",
            "file",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(temp.path()).unwrap();
            let home = root.join("home");
            let first = home.join("first");
            let second = home.join("second");
            fs::create_dir_all(&first).unwrap();
            fs::create_dir(&second).unwrap();
            let alias = home.join("alias");
            let outside = root.join("outside");
            fs::create_dir(&outside).unwrap();
            let file = home.join("document");
            fs::write(&file, "document").unwrap();
            symlink(
                match change {
                    "escape" => outside.clone(),
                    "dangling" => home.join("absent"),
                    "file" => file,
                    _ => first.clone(),
                },
                &alias,
            )
            .unwrap();
            let scope = SkillReadScope::bind(std::slice::from_ref(&home)).unwrap();
            let entry = observe(&scope, &alias);
            let retained = RetainedDirectory::retain(&scope, Arc::clone(&entry));
            if matches!(change, "escape" | "dangling" | "file") {
                assert!(retained.is_err(), "{change}");
                continue;
            }
            let retained = retained.unwrap();
            let original_inode = retained.source.file.metadata().unwrap().ino();
            match change {
                "retarget" => {
                    fs::remove_file(&alias).unwrap();
                    symlink(&second, &alias).unwrap();
                }
                "replace" => {
                    fs::rename(&first, home.join("old")).unwrap();
                    fs::create_dir(&first).unwrap();
                    assert_ne!(
                        std::os::unix::fs::MetadataExt::ino(&fs::metadata(&first).unwrap()),
                        original_inode
                    );
                    assert_eq!(
                        retained.source.file.metadata().unwrap().ino(),
                        original_inode
                    );
                }
                "resource-change" => fs::write(first.join("new-resource"), "changed").unwrap(),
                _ => unreachable!(),
            }
            assert!(retained.revalidate().is_err(), "{change}");
        }
    }
}
