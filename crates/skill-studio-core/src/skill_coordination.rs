//! Cooperative directory coordination for validated directory effects.
//!
//! This module observes directory identities itself. It does not authorize filesystem
//! mutations. The operation service must derive complete effects before it creates a
//! plan, including file domains for hard-link-sensitive operations.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs::{self, File, Metadata};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::skill_scope::{
    ScopedContentFoldError, ScopedContentObservation, ScopedFileObservation, ScopedLockOpenError,
    ScopedPrefixRead, ScopedReadError, SkillReadScope,
};

#[cfg(test)]
use std::sync::atomic::AtomicU64;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_DOMAINS: usize = 256;

static PROCESS_GATE: OnceLock<RwLock<()>> = OnceLock::new();

/// The strongest access needed by a merged directory domain.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CoordinationMode {
    Shared,
    Exclusive,
}

/// A directory effect already derived from a validated operation.
///
/// `Tree` protects the resolved directory and its existing ancestors. `Entry`
/// protects the lexical parent of an entry, including a missing entry's nearest
/// existing parent. A directory rename uses two `Entry` effects and one `Tree`
/// effect for the moved directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectoryEffect {
    Tree {
        path: PathBuf,
        mode: CoordinationMode,
    },
    Entry {
        path: PathBuf,
        mode: CoordinationMode,
    },
}

impl DirectoryEffect {
    pub fn tree(path: impl Into<PathBuf>, mode: CoordinationMode) -> Self {
        Self::Tree {
            path: path.into(),
            mode,
        }
    }

    pub fn entry(path: impl Into<PathBuf>, mode: CoordinationMode) -> Self {
        Self::Entry {
            path: path.into(),
            mode,
        }
    }

    fn path(&self) -> &Path {
        match self {
            Self::Tree { path, .. } | Self::Entry { path, .. } => path,
        }
    }

    fn mode(&self) -> CoordinationMode {
        match self {
            Self::Tree { mode, .. } | Self::Entry { mode, .. } => *mode,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PhysicalObjectId {
    device: u64,
    inode: u64,
}

impl Ord for PhysicalObjectId {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.device, self.inode).cmp(&(other.device, other.inode))
    }
}

impl PartialOrd for PhysicalObjectId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug)]
struct ObservedDomain {
    id: PhysicalObjectId,
    mode: CoordinationMode,
    path: PathBuf,
    kind: DomainKind,
    file: Option<ScopedFileObservation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DomainKind {
    Directory,
    File { link_count: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EffectBinding {
    requested: PathBuf,
    existing_lexical: PathBuf,
    resolved: PathBuf,
    resolved_id: PhysicalObjectId,
    missing_suffix: PathBuf,
}

#[derive(Clone, Debug)]
struct ObservedEffect {
    binding: EffectBinding,
    domains: Vec<ObservedDomain>,
}

#[derive(Clone, Debug)]
struct Observation {
    domains: Vec<ObservedDomain>,
    bindings: Vec<EffectBinding>,
    files: Vec<ScopedFileObservation>,
}

/// Clones cancel the same operation. A cancelled token cannot be reset or reused.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    flag: Arc<AtomicBool>,
    deadline: Option<Instant>,
}

impl CancellationToken {
    pub fn from_shared_deadline(flag: Arc<AtomicBool>, deadline: Instant) -> Self {
        Self {
            flag,
            deadline: Some(deadline),
        }
    }

    pub fn cancel(&self) {
        self.flag.store(true, AtomicOrdering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(AtomicOrdering::Acquire)
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
    }
}

#[derive(Clone, Debug)]
struct CoordinationDeadline {
    at: Instant,
    cancellation: Option<CancellationToken>,
    #[cfg(test)]
    manual: Option<(Arc<AtomicU64>, u64)>,
}

impl CoordinationDeadline {
    fn new(timeout: Option<Duration>) -> Result<Self, CoordinationFailure> {
        let timeout = timeout.unwrap_or(DEFAULT_TIMEOUT);
        if timeout.is_zero() {
            return Err(CoordinationFailure::InvalidTimeout);
        }
        let at = Instant::now()
            .checked_add(timeout)
            .ok_or(CoordinationFailure::InvalidTimeout)?;
        Ok(Self {
            at,
            cancellation: None,
            #[cfg(test)]
            manual: None,
        })
    }

    #[cfg(test)]
    fn manual(clock: Arc<AtomicU64>, budget: u64) -> Self {
        Self {
            at: Instant::now() + DEFAULT_TIMEOUT,
            cancellation: None,
            manual: Some((clock, budget)),
        }
    }

    fn expired(&self) -> bool {
        #[cfg(test)]
        if let Some((clock, budget)) = &self.manual {
            return clock.load(std::sync::atomic::Ordering::SeqCst) >= *budget;
        }
        Instant::now() >= self.at
    }

    fn check_cancelled(&self) -> Result<(), CoordinationFailure> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            Err(CoordinationFailure::Cancelled)
        } else {
            Ok(())
        }
    }

    fn remaining(&self) -> Duration {
        #[cfg(test)]
        if let Some((clock, budget)) = &self.manual {
            return Duration::from_nanos(
                budget.saturating_sub(clock.load(std::sync::atomic::Ordering::SeqCst)),
            );
        }
        self.at.saturating_duration_since(Instant::now())
    }
}

#[derive(Clone, Debug)]
enum ObservationScope {
    Production,
    #[cfg(test)]
    Fixture {
        lexical_root: PathBuf,
        root: PathBuf,
    },
}

/// A validated and observed directory coordination plan.
#[derive(Debug)]
pub struct CoordinationPlan {
    effects: Vec<DirectoryEffect>,
    observation: Observation,
    scope: ObservationScope,
    deadline: CoordinationDeadline,
}

/// Owns each native lease and the process-wide gate. Drop releases all leases.
pub struct CoordinationGuard {
    _leases: Vec<File>,
    _gate: ProcessGateGuard,
    effects: Vec<DirectoryEffect>,
    observation: Observation,
    scope: ObservationScope,
    deadline: CoordinationDeadline,
}

/// A complete directory-and-file lease for scoped reads.
pub(crate) struct CoordinatedReadGuard {
    guard: CoordinationGuard,
    file_mode: CoordinationMode,
    files: Vec<ScopedFileObservation>,
    directory_files: Vec<ScopedContentObservation>,
}

/// Retains the completed read plan and its scope without an extension path.
/// This lease permits only planned reads; it does not authorize writes.
#[cfg(any(test, feature = "event-store"))]
pub(crate) struct FinalizedReadLease<'scope> {
    guard: CoordinatedReadGuard,
    scope: &'scope SkillReadScope,
}

#[cfg(any(test, feature = "event-store"))]
impl FinalizedReadLease<'_> {
    pub(crate) fn read(&self, path: &Path, limit: usize) -> Result<Vec<u8>, ScopedReadError> {
        self.guard.read(self.scope, path, limit)
    }

    pub(crate) fn revalidate(&self) -> Result<(), CoordinationFailure> {
        self.guard.revalidate(self.scope)
    }
}

#[derive(Debug)]
pub enum PreparedContentError {
    Coordination(CoordinationFailure),
    Read(ScopedReadError),
    Io(io::Error),
    Invalid(String),
}

impl PreparedContentError {
    pub fn is_cancelled(&self) -> bool {
        match self {
            Self::Coordination(CoordinationFailure::Cancelled) => true,
            Self::Read(ScopedReadError::Io(error)) | Self::Io(error) => error
                .get_ref()
                .and_then(|source| source.downcast_ref::<CoordinationFailure>())
                .is_some_and(|cause| matches!(cause, CoordinationFailure::Cancelled)),
            _ => false,
        }
    }
}
impl std::fmt::Display for PreparedContentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Coordination(error) => error.fmt(f),
            Self::Read(error) => error.fmt(f),
            Self::Io(error) => error.fmt(f),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for PreparedContentError {}
impl From<CoordinationFailure> for PreparedContentError {
    fn from(error: CoordinationFailure) -> Self {
        Self::Coordination(error)
    }
}
impl From<ScopedReadError> for PreparedContentError {
    fn from(error: ScopedReadError) -> Self {
        Self::Read(error)
    }
}
impl From<io::Error> for PreparedContentError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<String> for PreparedContentError {
    fn from(message: String) -> Self {
        Self::Invalid(message)
    }
}
impl From<&str> for PreparedContentError {
    fn from(message: &str) -> Self {
        Self::Invalid(message.into())
    }
}

struct AbsentInvocationSidecar {
    path: PathBuf,
    parent: PathBuf,
    resolved_parent: PathBuf,
    identity: (u64, u64),
}

impl AbsentInvocationSidecar {
    fn capture(scope: &SkillReadScope, path: &Path) -> Result<Self, String> {
        use cap_std::fs::MetadataExt;
        let mut parent = path.parent().ok_or("Invocation sidecar has no parent")?;
        loop {
            match scope.resolved_path_metadata(parent) {
                Ok((resolved_parent, metadata)) if metadata.is_dir() => {
                    return Ok(Self {
                        path: path.to_path_buf(),
                        parent: parent.to_path_buf(),
                        resolved_parent,
                        identity: (metadata.dev(), metadata.ino()),
                    })
                }
                Err(ScopedReadError::Missing { .. }) => {
                    parent = parent.parent().ok_or("Invocation parent is unavailable")?
                }
                _ => return Err("Invocation parent is not a scoped directory".into()),
            }
        }
    }

    fn revalidate_parent(&self, scope: &SkillReadScope) -> Result<(), CoordinationFailure> {
        use cap_std::fs::MetadataExt;
        let skill = self
            .path
            .parent()
            .and_then(Path::parent)
            .ok_or(CoordinationFailure::Changed)?;
        match scope.observe_entry(skill, std::ffi::OsStr::new("agents")) {
            Ok(entry) if entry.metadata.is_dir() && !entry.metadata.file_type().is_symlink() => {}
            Err(ScopedReadError::Missing { .. }) if self.parent == skill => {}
            _ => return Err(CoordinationFailure::Changed),
        }
        match scope.resolved_path_metadata(&self.parent) {
            Ok((path, metadata))
                if path == self.resolved_parent
                    && metadata.is_dir()
                    && (metadata.dev(), metadata.ino()) == self.identity =>
            {
                Ok(())
            }
            _ => Err(CoordinationFailure::Changed),
        }
    }
}

/// Frozen exclusive directory and file domains. The caller still owns effect
/// completeness, ownership validation, intent persistence and recovery.
pub struct FinalizedWriteLease<'scope> {
    guard: CoordinatedReadGuard,
    scope: &'scope SkillReadScope,
    published: BTreeMap<PathBuf, crate::skill_document_target::DocumentReceipt>,
    failed_after_replace: bool,
    absent_invocation_sidecar: Option<AbsentInvocationSidecar>,
    ownership: Option<crate::skill_ownership::PreparedOwnershipRead>,
    discovery_membership: Option<crate::skill_discovery::DiscoveryMembershipProof>,
}

impl FinalizedWriteLease<'_> {
    pub(crate) fn retain_absent_invocation_sidecar(mut self, path: &Path) -> Result<Self, String> {
        self.revalidate().map_err(|error| error.to_string())?;
        if self.absent_invocation_sidecar.is_some()
            || path.file_name() != Some(std::ffi::OsStr::new("openai.yaml"))
            || path.parent().and_then(Path::file_name) != Some(std::ffi::OsStr::new("agents"))
            || !self.guard.guard.effects.iter().any(|effect| matches!(effect,
                DirectoryEffect::Tree { path: root, mode: CoordinationMode::Exclusive } if path.starts_with(root)))
            || !matches!(self.scope.resolved_path_metadata(path), Err(ScopedReadError::Missing { .. })) {
            return Err("Invocation sidecar absence is not covered by this lease".into());
        }
        self.absent_invocation_sidecar = Some(AbsentInvocationSidecar::capture(self.scope, path)?);
        self.revalidate().map_err(|error| error.to_string())?;
        Ok(self)
    }

    pub(crate) fn invocation_parent_was_absent(&self, path: &Path) -> bool {
        self.absent_invocation_sidecar
            .as_ref()
            .is_some_and(|proof| {
                proof.path == path && path.parent() != Some(proof.parent.as_path())
            })
    }

    pub(crate) fn validate_invocation_creation(&self, path: &Path) -> Result<(), String> {
        self.revalidate().map_err(|error| error.to_string())?;
        if self
            .absent_invocation_sidecar
            .as_ref()
            .map(|proof| proof.path.as_path())
            != Some(path)
            || self.published.contains_key(path)
        {
            return Err("Invocation creation requires the retained original absence".into());
        }
        Ok(())
    }

    #[cfg(feature = "event-store")]
    pub(crate) fn validate_invocation_output(
        &self,
        path: &Path,
        expected: Option<&[u8]>,
    ) -> Result<(), String> {
        self.revalidate().map_err(|error| error.to_string())?;
        if let Some(receipt) = self.published.get(path) {
            match expected {
                Some(bytes) => receipt.verify_content(bytes)?,
                None => receipt.verify_absence()?,
            }
        } else {
            match expected {
                Some(bytes) => {
                    let current = self
                        .read(
                            path,
                            crate::skill_copy_document_edit::MAX_COPY_DOCUMENT_EDIT_BYTES,
                        )
                        .map_err(|error| error.to_string())?;
                    if current != bytes {
                        return Err("Invocation output does not match its intent".into());
                    }
                }
                None if self
                    .absent_invocation_sidecar
                    .as_ref()
                    .map(|proof| proof.path.as_path())
                    == Some(path) => {}
                None => return Err("Invocation absence has no retained proof".into()),
            }
        }
        self.revalidate().map_err(|error| error.to_string())
    }

    pub(crate) fn retain_membership(
        mut self,
        plugins: crate::skill_discovery::DiscoveryMembershipProof,
    ) -> Result<Self, CoordinationFailure> {
        self.discovery_membership = Some(plugins);
        self.revalidate()?;
        Ok(self)
    }

    pub(crate) fn retain_ownership(
        mut self,
        ownership: crate::skill_ownership::PreparedOwnershipRead,
    ) -> Result<Self, CoordinationFailure> {
        ownership
            .revalidate(self.scope)
            .map_err(|_| CoordinationFailure::Changed)?;
        self.ownership = Some(ownership);
        self.revalidate()?;
        Ok(self)
    }

    pub fn read(&self, path: &Path, limit: usize) -> Result<Vec<u8>, ScopedReadError> {
        self.guard.read(self.scope, path, limit)
    }

    pub(crate) fn read_retained(&self, path: &Path, limit: usize) -> Result<Vec<u8>, String> {
        match self.published.get(path) {
            Some(receipt) => receipt.read(limit),
            None => self.read(path, limit).map_err(|error| error.to_string()),
        }
    }

    pub(crate) fn read_ownership_registry(
        &self,
        path: &Path,
        limit: usize,
    ) -> Result<Option<Vec<u8>>, String> {
        self.revalidate().map_err(|error| error.to_string())?;
        let absent = self
            .ownership
            .as_ref()
            .ok_or("Lease has no retained ownership inputs")?
            .registry_was_absent(path)?;
        let bytes = if absent {
            None
        } else {
            Some(self.read(path, limit).map_err(|error| error.to_string())?)
        };
        self.revalidate().map_err(|error| error.to_string())?;
        Ok(bytes)
    }

    pub(crate) fn read_current_ownership_registry(
        &self,
        path: &Path,
        limit: usize,
    ) -> Result<Option<Vec<u8>>, String> {
        self.revalidate().map_err(|error| error.to_string())?;
        let bytes = if let Some(receipt) = self.published.get(path) {
            Some(receipt.read(limit)?)
        } else if self
            .ownership
            .as_ref()
            .ok_or("Lease has no retained ownership inputs")?
            .registry_was_absent(path)?
        {
            None
        } else {
            Some(self.read(path, limit).map_err(|error| error.to_string())?)
        };
        self.revalidate().map_err(|error| error.to_string())?;
        Ok(bytes)
    }

    pub(crate) fn fold_resource(
        &self,
        path: &Path,
        limit: u64,
        fold: &mut dyn FnMut(&[u8]),
    ) -> Result<u64, PreparedContentError> {
        self.validate_document(path)
            .map_err(PreparedContentError::from)?;
        let count = self
            .guard
            .fold(self.scope, path, limit, &mut || Ok(()), fold)
            .map_err(|error| -> PreparedContentError {
                match error {
                    ScopedContentFoldError::Read(error) => error.into(),
                    ScopedContentFoldError::Cancelled(_) => CoordinationFailure::Cancelled.into(),
                    ScopedContentFoldError::Changed => {
                        "Resource changed during streaming read".into()
                    }
                }
            })?;
        self.revalidate().map_err(PreparedContentError::from)?;
        Ok(count)
    }

    pub(crate) fn validate_published_document(
        &self,
        path: &Path,
        expected: &[u8],
    ) -> Result<(), String> {
        self.revalidate().map_err(|error| error.to_string())?;
        self.published
            .get(path)
            .ok_or("Document has no publication receipt in this lease")?
            .verify_content(expected)?;
        self.revalidate().map_err(|error| error.to_string())
    }

    pub(crate) fn validate_registry_creation(&self, path: &Path) -> Result<(), String> {
        self.revalidate().map_err(|error| error.to_string())?;
        if self.published.contains_key(path)
            || !self.ownership.as_ref().ok_or("Missing registry ownership proof")?.registry_was_absent(path)?
            || !self.guard.guard.effects.iter().any(|effect| matches!(effect,
                DirectoryEffect::Tree { path: root, mode: CoordinationMode::Exclusive } if path.starts_with(root))) {
            return Err("Registry creation is not covered by the original absent ownership proof".into());
        }
        Ok(())
    }

    pub(crate) fn validate_document(&self, path: &Path) -> Result<(), CoordinationFailure> {
        self.guard.check_cancelled()?;
        self.guard
            .planned_file(path)
            .map_err(|_| CoordinationFailure::Changed)?;
        if self.published.contains_key(path) {
            return Err(CoordinationFailure::Changed);
        }
        self.revalidate()
    }

    pub(crate) fn validate_registry_replacement(
        &self,
        path: &Path,
        expected: &[u8],
    ) -> Result<(), String> {
        self.revalidate().map_err(|error| error.to_string())?;
        if let Some(receipt) = self.published.get(path) {
            return receipt.verify_content(expected);
        }
        self.guard
            .planned_file(path)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Validate remaining inputs and completed replacements without reacquiring
    /// locks. Call before subsequent effects and before recording completion.
    pub fn revalidate(&self) -> Result<(), CoordinationFailure> {
        self.guard.check_cancelled()?;
        if self.failed_after_replace {
            return Err(CoordinationFailure::Changed);
        }
        self.guard.guard.revalidate()?;
        if let Some(proof) = &self.absent_invocation_sidecar {
            proof.revalidate_parent(self.scope)?;
            let path = &proof.path;
            if !self.published.contains_key(path)
                && !matches!(
                    self.scope.resolved_path_metadata(path),
                    Err(ScopedReadError::Missing { .. })
                )
            {
                return Err(CoordinationFailure::Changed);
            }
        }

        for receipt in self.published.values() {
            receipt
                .revalidate()
                .map_err(|_| CoordinationFailure::Changed)?;
        }
        if let Some(ownership) = &self.ownership {
            ownership
                .revalidate_remaining(self.scope, |path| self.published.contains_key(path))
                .map_err(|_| CoordinationFailure::Changed)?;
        }
        if let Some(plugins) = &self.discovery_membership {
            plugins.revalidate(self.scope, |path| self.published.contains_key(path))?;
        }
        let published_targets = self
            .guard
            .files
            .iter()
            .filter_map(|file| {
                self.published
                    .get(SkillReadScope::observed_requested(file))
                    .map(|receipt| (SkillReadScope::observed_resolved(file), receipt))
            })
            .collect::<BTreeMap<_, _>>();
        let mut remaining = Vec::new();
        for file in &self.guard.files {
            let requested = SkillReadScope::observed_requested(file);
            if self.published.contains_key(requested) {
                continue;
            }
            if let Some(receipt) = published_targets.get(SkillReadScope::observed_resolved(file)) {
                receipt
                    .revalidate_alias(self.scope, requested)
                    .map_err(|_| CoordinationFailure::Changed)?;
            } else {
                remaining.push(file.clone());
            }
        }
        let current = observe_scoped_files(
            self.scope,
            &remaining
                .iter()
                .map(|file| SkillReadScope::observed_requested(file).to_path_buf())
                .collect::<Vec<_>>(),
            None,
        )?;
        self.guard.check_cancelled()?;
        if current == remaining {
            Ok(())
        } else {
            Err(CoordinationFailure::Changed)
        }
    }

    #[cfg(any(test, feature = "event-store"))]
    pub(crate) fn validate_tree_move(
        &self,
        source: &Path,
        destination: &Path,
    ) -> Result<(), String> {
        self.validate_state_tree(source)?;
        self.validate_entry_move(source, destination)
    }

    #[cfg(any(test, feature = "event-store"))]
    pub(crate) fn validate_entry_move(
        &self,
        source: &Path,
        destination: &Path,
    ) -> Result<(), String> {
        self.revalidate().map_err(|error| error.to_string())?;
        for path in [source, destination] {
            if !self
                .guard
                .guard
                .effects
                .contains(&DirectoryEffect::entry(path, CoordinationMode::Exclusive))
            {
                return Err(
                    "Tree move needs exclusive source and destination entry effects".into(),
                );
            }
        }
        Ok(())
    }

    #[cfg(any(test, feature = "event-store"))]
    pub(crate) fn validate_state_tree(&self, path: &Path) -> Result<(), String> {
        self.validate_state_tree_prepared(path)
            .map_err(|error| error.to_string())
    }

    #[cfg(any(test, feature = "event-store"))]
    pub(crate) fn validate_state_tree_prepared(
        &self,
        path: &Path,
    ) -> Result<(), PreparedContentError> {
        self.revalidate()?;
        if self.guard.guard.effects.iter().any(|effect| matches!(effect,
            DirectoryEffect::Tree { path: planned, mode: CoordinationMode::Exclusive } if planned == path)) {
            Ok(())
        } else {
            Err("State root is not an explicit exclusive tree in the write plan".into())
        }
    }

    pub(crate) fn record_document(
        &mut self,
        result: Result<
            crate::skill_document_target::DocumentReceipt,
            crate::skill_document_write::DocumentWriteFailure,
        >,
    ) -> Result<(), crate::skill_document_write::DocumentWriteFailure> {
        match result {
            Ok(receipt) => {
                self.published.insert(receipt.path.clone(), receipt);
                Ok(())
            }
            Err(error) => {
                if matches!(
                    error,
                    crate::skill_document_write::DocumentWriteFailure::AfterReplace(_)
                ) {
                    self.failed_after_replace = true;
                }
                Err(error)
            }
        }
    }
}

enum ProcessGateGuard {
    Shared {
        _guard: RwLockReadGuard<'static, ()>,
    },
    Exclusive {
        _guard: RwLockWriteGuard<'static, ()>,
    },
}

#[derive(Debug)]
pub enum CoordinationFailure {
    Busy,
    DeadlineExceeded,
    Cancelled,
    InvalidTimeout,
    FilesystemRootRequired,
    ExclusiveEffectsRequired,
    Unavailable { path: PathBuf, source: io::Error },
    Changed,
    CapacityExceeded { required: usize, limit: usize },
}

impl std::fmt::Display for CoordinationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => write!(formatter, "directory coordination is busy"),
            Self::DeadlineExceeded => write!(
                formatter,
                "directory coordination exceeded its acquisition deadline"
            ),
            Self::Cancelled => write!(formatter, "directory coordination was cancelled"),
            Self::InvalidTimeout => write!(formatter, "directory coordination timeout is invalid"),
            Self::ExclusiveEffectsRequired => write!(
                formatter,
                "write lease requires exclusive directory effects"
            ),
            Self::FilesystemRootRequired => {
                write!(formatter, "operation requires filesystem-root coordination")
            }
            Self::Unavailable { path, source } => write!(
                formatter,
                "directory coordination is unavailable at {}: {source}",
                path.display()
            ),
            Self::Changed => write!(formatter, "directory bindings changed during coordination"),
            Self::CapacityExceeded { required, limit } => write!(
                formatter,
                "directory coordination needs {required} domains, limit is {limit}"
            ),
        }
    }
}

impl std::error::Error for CoordinationFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unavailable { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl CoordinationPlan {
    /// Observes all absolute effects and always derives their complete ancestors.
    pub fn new(
        effects: Vec<DirectoryEffect>,
        timeout: Option<Duration>,
    ) -> Result<Self, CoordinationFailure> {
        let deadline = CoordinationDeadline::new(timeout)?;
        Self::new_in_scope(effects, ObservationScope::Production, deadline)
    }

    pub fn new_cancellable(
        effects: Vec<DirectoryEffect>,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<Self, CoordinationFailure> {
        let mut deadline = CoordinationDeadline::new(timeout)?;
        deadline.cancellation = Some(cancellation);
        Self::new_in_scope(effects, ObservationScope::Production, deadline)
    }

    fn new_in_scope(
        effects: Vec<DirectoryEffect>,
        scope: ObservationScope,
        deadline: CoordinationDeadline,
    ) -> Result<Self, CoordinationFailure> {
        deadline.check_cancelled()?;
        let observation = observe_effects(&effects, &scope, Some(&deadline))?;
        deadline.check_cancelled()?;
        Ok(Self {
            effects,
            observation,
            scope,
            deadline,
        })
    }

    #[cfg(test)]
    fn new_in_scope_with<F>(
        effects: Vec<DirectoryEffect>,
        scope: ObservationScope,
        deadline: CoordinationDeadline,
        observer: F,
    ) -> Result<Self, CoordinationFailure>
    where
        F: FnMut(
            &DirectoryEffect,
            &ObservationScope,
            Option<&CoordinationDeadline>,
        ) -> Result<ObservedEffect, CoordinationFailure>,
    {
        let observation = observe_effects_with(&effects, &scope, Some(&deadline), observer)?;
        Ok(Self {
            effects,
            observation,
            scope,
            deadline,
        })
    }

    #[cfg(test)]
    pub(crate) fn new_fixture(
        effects: Vec<DirectoryEffect>,
        root: &Path,
        timeout: Option<Duration>,
    ) -> Result<Self, CoordinationFailure> {
        let deadline = CoordinationDeadline::new(timeout)?;
        let lexical_root = root.to_path_buf();
        let root = existing_directory(root, Some(&deadline))?.0;
        Self::new_in_scope(
            effects,
            ObservationScope::Fixture { lexical_root, root },
            deadline,
        )
    }

    pub fn acquire(self) -> Result<CoordinationGuard, CoordinationFailure> {
        let deadline = self.deadline;
        let mut expected = self.observation;
        let mut saw_contention = false;
        let mut saw_change = false;
        loop {
            deadline.check_cancelled()?;
            if deadline.expired() {
                return Err(expired_failure(saw_contention, saw_change));
            }
            let gate = match acquire_gate(strongest_mode(&expected.domains), &deadline)? {
                Some(gate) => gate,
                None => return Err(CoordinationFailure::Busy),
            };
            match acquire_native(&expected.domains, &deadline, None) {
                Ok(leases) => {
                    let current = match observe_effects(&self.effects, &self.scope, Some(&deadline))
                    {
                        Ok(current) => current,
                        Err(CoordinationFailure::DeadlineExceeded) if deadline.expired() => {
                            drop(leases);
                            drop(gate);
                            return Err(expired_failure(saw_contention, saw_change));
                        }
                        Err(error) if observation_may_have_changed(&error) => {
                            drop(leases);
                            drop(gate);
                            saw_change = true;
                            backoff(&deadline, saw_contention, saw_change)?;
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    deadline.check_cancelled()?;
                    if deadline.expired() {
                        drop(leases);
                        drop(gate);
                        return Err(expired_failure(saw_contention, saw_change));
                    }
                    if same_observation(&expected, &current) {
                        return Ok(CoordinationGuard {
                            _gate: gate,
                            _leases: leases,
                            effects: self.effects,
                            observation: current,
                            scope: self.scope,
                            deadline,
                        });
                    }
                    drop(leases);
                    drop(gate);
                    expected = current;
                    saw_change = true;
                }
                Err(NativeAcquire::Busy) => {
                    drop(gate);
                    saw_contention = true;
                    backoff(&deadline, saw_contention, saw_change)?;
                    match observe_effects(&self.effects, &self.scope, Some(&deadline)) {
                        Ok(current) => expected = current,
                        Err(CoordinationFailure::DeadlineExceeded) if deadline.expired() => {
                            return Err(expired_failure(saw_contention, saw_change));
                        }
                        Err(error) if observation_may_have_changed(&error) => saw_change = true,
                        Err(error) => return Err(error),
                    }
                }
                Err(NativeAcquire::Changed) => {
                    drop(gate);
                    saw_change = true;
                    backoff(&deadline, saw_contention, saw_change)?;
                    match observe_effects(&self.effects, &self.scope, Some(&deadline)) {
                        Ok(current) => expected = current,
                        Err(CoordinationFailure::DeadlineExceeded) if deadline.expired() => {
                            return Err(expired_failure(saw_contention, saw_change));
                        }
                        Err(error) if observation_may_have_changed(&error) => {}
                        Err(error) => return Err(error),
                    }
                }
                Err(NativeAcquire::Deadline) => {
                    drop(gate);
                    return Err(expired_failure(saw_contention, saw_change));
                }
                Err(NativeAcquire::Unavailable(error)) => {
                    drop(gate);
                    return Err(error);
                }
            }
        }
    }
}

impl CoordinationGuard {
    /// Re-observes the effects that produced this lease.
    pub fn revalidate(&self) -> Result<(), CoordinationFailure> {
        let current = observe_effects(&self.effects, &self.scope, None)?;
        if same_directory_observation(&self.observation, &current) {
            Ok(())
        } else {
            Err(CoordinationFailure::Changed)
        }
    }

    /// Completes file-domain planning before intent. This step may release and
    /// reacquire the sorted plan. The returned lease has no extension API.
    pub fn finalize_write<'scope>(
        self,
        scope: &'scope SkillReadScope,
        files: &[PathBuf],
    ) -> Result<FinalizedWriteLease<'scope>, CoordinationFailure> {
        if self.effects.is_empty()
            || self
                .effects
                .iter()
                .any(|effect| effect.mode() != CoordinationMode::Exclusive)
        {
            return Err(CoordinationFailure::ExclusiveEffectsRequired);
        }
        let guard = self.continue_with_files(scope, files, CoordinationMode::Exclusive)?;
        guard.finalize_write(scope)
    }

    /// Continues a held directory lease into a complete file-aware read lease.
    /// Covered single-link reads keep the directory lease. Other reads release
    /// it before acquiring the complete sorted directory-and-file set.
    pub(crate) fn continue_with_files(
        self,
        read_scope: &SkillReadScope,
        paths: &[PathBuf],
        mode: CoordinationMode,
    ) -> Result<CoordinatedReadGuard, CoordinationFailure> {
        let mut effects = self.effects.clone();
        let scope = self.scope.clone();
        let deadline = self.deadline.clone();
        let covered = shared_tree_roots(&self.effects, &self.observation.bindings);
        for path in paths {
            check_observation_deadline(Some(&deadline))?;
            let parent = path
                .parent()
                .ok_or(CoordinationFailure::FilesystemRootRequired)?;
            let already_covered = mode == CoordinationMode::Shared
                && covered
                    .iter()
                    .any(|(lexical, _)| parent.starts_with(lexical))
                && {
                    let resolved = read_scope
                        .resolved_dir_path(parent)
                        .map_err(scoped_failure)?;
                    covered.iter().any(|(lexical, physical)| {
                        parent.starts_with(lexical) && resolved.starts_with(physical)
                    })
                };
            if !already_covered {
                effects.push(DirectoryEffect::entry(path.clone(), mode));
            }
        }
        if mode == CoordinationMode::Shared && effects == self.effects {
            let mut directory_files = Vec::with_capacity(paths.len());
            for path in paths {
                check_observation_deadline(Some(&deadline))?;
                let file = read_scope
                    .observe_content_regular(path)
                    .map_err(scoped_failure)?;
                let parent = file
                    .resolved_path()
                    .parent()
                    .ok_or(CoordinationFailure::FilesystemRootRequired)?;
                if !file.single_link() || !covered.iter().any(|(_, root)| parent.starts_with(root))
                {
                    break;
                }
                directory_files.push(file);
            }
            if directory_files.len() == paths.len() {
                directory_files.sort_by(|left, right| left.requested.cmp(&right.requested));
                self.revalidate()?;
                revalidate_directory_files(read_scope, &directory_files, Some(&deadline))?;
                return Ok(CoordinatedReadGuard {
                    file_mode: mode,
                    guard: self,
                    files: Vec::new(),
                    directory_files,
                });
            }
        }
        drop(self);
        let (effects, first) =
            plan_covered_files(effects, &scope, &deadline, read_scope, paths, mode)?;
        acquire_complete_read(effects, scope, deadline, read_scope, paths, mode, first)
    }
}

impl CoordinatedReadGuard {
    pub(crate) fn finalize_write(
        self,
        scope: &SkillReadScope,
    ) -> Result<FinalizedWriteLease<'_>, CoordinationFailure> {
        if self.file_mode != CoordinationMode::Exclusive
            || self.guard.effects.is_empty()
            || self
                .guard
                .effects
                .iter()
                .any(|effect| effect.mode() != CoordinationMode::Exclusive)
        {
            return Err(CoordinationFailure::ExclusiveEffectsRequired);
        }
        self.check_cancelled()?;
        self.revalidate(scope)?;
        Ok(FinalizedWriteLease {
            guard: self,
            scope,
            published: BTreeMap::new(),
            failed_after_replace: false,
            absent_invocation_sidecar: None,
            ownership: None,
            discovery_membership: None,
        })
    }

    #[cfg(any(test, feature = "event-store"))]
    pub(crate) fn finalize(
        self,
        scope: &SkillReadScope,
    ) -> Result<FinalizedReadLease<'_>, CoordinationFailure> {
        self.revalidate(scope)?;
        Ok(FinalizedReadLease { guard: self, scope })
    }

    pub(crate) fn check_cancelled(&self) -> Result<(), CoordinationFailure> {
        self.guard.deadline.check_cancelled()
    }

    #[cfg(test)]
    pub(crate) fn check_identity_admission(
        &self,
        scope: &SkillReadScope,
    ) -> Result<(), CoordinationFailure> {
        check_observation_deadline(Some(&self.guard.deadline))?;
        self.revalidate(scope)?;
        check_observation_deadline(Some(&self.guard.deadline))
    }

    #[cfg(test)]
    pub(crate) fn covers_identity_directory(
        &self,
        scope: &SkillReadScope,
        path: &Path,
    ) -> Result<bool, CoordinationFailure> {
        self.check_identity_admission(scope)?;
        if self.file_mode != CoordinationMode::Shared {
            return Ok(false);
        }
        let parent = path
            .parent()
            .ok_or(CoordinationFailure::FilesystemRootRequired)?;
        let resolved_parent = scope.resolved_dir_path(parent).map_err(scoped_failure)?;
        let resolved = scope.resolved_dir_path(path).map_err(scoped_failure)?;
        let covered = shared_tree_roots(&self.guard.effects, &self.guard.observation.bindings)
            .iter()
            .any(|(lexical, physical)| {
                parent.starts_with(lexical)
                    && resolved_parent.starts_with(physical)
                    && resolved.starts_with(physical)
            });
        self.check_identity_admission(scope)?;
        Ok(covered)
    }

    pub(crate) fn extend_with_files(
        self,
        scope: &SkillReadScope,
        additional: &[PathBuf],
    ) -> Result<Self, CoordinationFailure> {
        let deadline = self.guard.deadline.clone();
        check_observation_deadline(Some(&deadline))?;
        self.revalidate(scope)?;
        let mut previous = self.directory_files.clone();
        for file in &self.files {
            check_observation_deadline(Some(&deadline))?;
            let observation = scope
                .observe_content_regular(SkillReadScope::observed_requested(file))
                .map_err(scoped_failure)?;
            if !file.matches_content(&observation) {
                return Err(CoordinationFailure::Changed);
            }
            previous.push(observation);
        }
        let paths = previous
            .iter()
            .map(|file| file.requested.clone())
            .chain(additional.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let Self {
            guard,
            files,
            file_mode,
            ..
        } = self;
        drop(files);
        let next = guard.continue_with_files(scope, &paths, file_mode)?;
        for observation in &previous {
            check_observation_deadline(Some(&deadline))?;
            next.check_content_observation(observation)
                .map_err(|_| CoordinationFailure::Changed)?;
        }
        next.revalidate(scope)?;
        check_observation_deadline(Some(&deadline))?;
        Ok(next)
    }

    pub(crate) fn read(
        &self,
        scope: &SkillReadScope,
        path: &Path,
        limit: usize,
    ) -> Result<Vec<u8>, ScopedReadError> {
        self.check_cancelled().map_err(|error| {
            ScopedReadError::Io(io::Error::new(io::ErrorKind::Interrupted, error))
        })?;
        if let Some(file) = self.directory_file(path) {
            return scope.read_content_observed(file, limit);
        }
        scope.read_observed(self.planned_file(path)?, limit)
    }

    pub(crate) fn read_prefix(
        &self,
        scope: &SkillReadScope,
        path: &Path,
        limit: usize,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<ScopedPrefixRead, ScopedContentFoldError> {
        let mut check = || {
            self.check_cancelled().map_err(|error| error.to_string())?;
            check()
        };

        if let Some(file) = self.directory_file(path) {
            return scope.read_content_prefix(file, limit, &mut check);
        }
        let file = self
            .planned_file(path)
            .map_err(ScopedContentFoldError::Read)?;
        scope.read_observed_prefix(file, limit, &mut check)
    }

    pub(crate) fn fold(
        &self,
        scope: &SkillReadScope,
        path: &Path,
        limit: u64,
        check: &mut dyn FnMut() -> Result<(), String>,
        fold: &mut dyn FnMut(&[u8]),
    ) -> Result<u64, ScopedContentFoldError> {
        let mut check = || {
            self.check_cancelled().map_err(|error| error.to_string())?;
            check()
        };

        if let Some(file) = self.directory_file(path) {
            return scope.fold_observed_content(file, limit, &mut check, fold);
        }
        let file = self
            .planned_file(path)
            .map_err(ScopedContentFoldError::Read)?;
        scope.fold_observed_file(file, limit, &mut check, fold)
    }

    pub(crate) fn read_content(
        &self,
        scope: &SkillReadScope,
        observation: &ScopedContentObservation,
        limit: usize,
    ) -> Result<Vec<u8>, ScopedContentFoldError> {
        self.check_content_observation(observation)?;
        self.read(scope, &observation.requested, limit)
            .map_err(ScopedContentFoldError::Read)
    }

    pub(crate) fn check_content_observation(
        &self,
        observation: &ScopedContentObservation,
    ) -> Result<(), ScopedContentFoldError> {
        self.check_cancelled()
            .map_err(|error| ScopedContentFoldError::Cancelled(error.to_string()))?;
        let matches = if let Some(file) = self.directory_file(&observation.requested) {
            file == observation
        } else {
            self.planned_file(&observation.requested)
                .map_err(ScopedContentFoldError::Read)?
                .matches_content(observation)
        };
        if matches {
            Ok(())
        } else {
            Err(ScopedContentFoldError::Changed)
        }
    }

    fn directory_file(&self, path: &Path) -> Option<&ScopedContentObservation> {
        self.directory_files
            .binary_search_by(|file| file.requested.as_path().cmp(path))
            .ok()
            .map(|index| &self.directory_files[index])
    }

    fn planned_file(&self, path: &Path) -> Result<&ScopedFileObservation, ScopedReadError> {
        self.files
            .iter()
            .find(|file| SkillReadScope::observed_requested(file) == path)
            .ok_or_else(|| {
                ScopedReadError::Io(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "file was not planned under the coordination guard",
                ))
            })
    }

    pub(crate) fn revalidate(&self, scope: &SkillReadScope) -> Result<(), CoordinationFailure> {
        self.check_cancelled()?;
        self.guard.revalidate()?;
        revalidate_directory_files(scope, &self.directory_files, None)?;
        let current = observe_scoped_files(
            scope,
            &self
                .files
                .iter()
                .map(|file| SkillReadScope::observed_requested(file).to_path_buf())
                .collect::<Vec<_>>(),
            None,
        )?;
        self.check_cancelled()?;
        if current == self.files {
            Ok(())
        } else {
            Err(CoordinationFailure::Changed)
        }
    }
}

fn expired_failure(saw_contention: bool, saw_change: bool) -> CoordinationFailure {
    if saw_contention {
        CoordinationFailure::Busy
    } else if saw_change {
        CoordinationFailure::Changed
    } else {
        CoordinationFailure::DeadlineExceeded
    }
}

fn preserve_deadline_evidence(
    error: CoordinationFailure,
    saw_contention: bool,
    saw_change: bool,
) -> CoordinationFailure {
    match error {
        CoordinationFailure::DeadlineExceeded => expired_failure(saw_contention, saw_change),
        error => error,
    }
}

fn strongest_mode(domains: &[ObservedDomain]) -> CoordinationMode {
    domains
        .iter()
        .map(|domain| domain.mode)
        .max()
        .unwrap_or(CoordinationMode::Shared)
}

fn metadata_id(metadata: &Metadata) -> PhysicalObjectId {
    PhysicalObjectId {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

fn unavailable(path: &Path, source: io::Error) -> CoordinationFailure {
    CoordinationFailure::Unavailable {
        path: path.to_path_buf(),
        source,
    }
}

fn observation_may_have_changed(error: &CoordinationFailure) -> bool {
    matches!(error, CoordinationFailure::Unavailable { source, .. } if source.kind() == io::ErrorKind::NotFound)
}

fn existing_directory(
    path: &Path,
    deadline: Option<&CoordinationDeadline>,
) -> Result<(PathBuf, PhysicalObjectId), CoordinationFailure> {
    check_observation_deadline(deadline)?;
    let resolved = fs::canonicalize(path).map_err(|error| unavailable(path, error))?;
    check_observation_deadline(deadline)?;
    let metadata = fs::metadata(&resolved).map_err(|error| unavailable(path, error))?;
    check_observation_deadline(deadline)?;
    if !metadata.is_dir() {
        return Err(unavailable(
            path,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "coordination domain is not a directory",
            ),
        ));
    }
    Ok((resolved, metadata_id(&metadata)))
}

fn nearest_existing_parent(
    path: &Path,
    deadline: Option<&CoordinationDeadline>,
) -> Result<(PathBuf, PathBuf, PhysicalObjectId, PathBuf), CoordinationFailure> {
    let mut candidate = path
        .parent()
        .ok_or(CoordinationFailure::FilesystemRootRequired)?
        .to_path_buf();
    loop {
        check_observation_deadline(deadline)?;
        match fs::symlink_metadata(&candidate) {
            Ok(_) => {
                check_observation_deadline(deadline)?;
                let (resolved, id) = existing_directory(&candidate, deadline)?;
                let suffix = path
                    .strip_prefix(&candidate)
                    .unwrap_or(Path::new(""))
                    .to_path_buf();
                return Ok((candidate, resolved, id, suffix));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                check_observation_deadline(deadline)?;
                candidate = candidate
                    .parent()
                    .ok_or(CoordinationFailure::FilesystemRootRequired)?
                    .to_path_buf();
            }
            Err(error) => return Err(unavailable(&candidate, error)),
        }
    }
}

fn observe_domain(
    path: &Path,
    mode: CoordinationMode,
) -> Result<ObservedDomain, CoordinationFailure> {
    let metadata = fs::metadata(path).map_err(|error| unavailable(path, error))?;
    if !metadata.is_dir() {
        return Err(unavailable(
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "ancestor is not a directory"),
        ));
    }
    Ok(ObservedDomain {
        id: metadata_id(&metadata),
        mode,
        path: path.to_path_buf(),
        kind: DomainKind::Directory,
        file: None,
    })
}

fn observe_ancestors(
    start: &Path,
    mode: CoordinationMode,
    scope: &ObservationScope,
    deadline: Option<&CoordinationDeadline>,
) -> Result<Vec<ObservedDomain>, CoordinationFailure> {
    let mut domains = Vec::new();
    let mut current = start.to_path_buf();
    match scope {
        ObservationScope::Production => loop {
            if !production_ancestor_is_domain(&current) {
                break;
            }
            check_observation_deadline(deadline)?;
            domains.push(observe_domain(&current, mode)?);
            check_observation_deadline(deadline)?;
            if domains.len() > MAX_DOMAINS {
                return Err(CoordinationFailure::CapacityExceeded {
                    required: domains.len(),
                    limit: MAX_DOMAINS,
                });
            }
            current = current
                .parent()
                .ok_or(CoordinationFailure::FilesystemRootRequired)?
                .to_path_buf();
        },
        #[cfg(test)]
        ObservationScope::Fixture { root, .. } => {
            if !current.starts_with(root) {
                return Err(unavailable(
                    &current,
                    io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "fixture observation escaped its namespace",
                    ),
                ));
            }
            loop {
                check_observation_deadline(deadline)?;
                domains.push(observe_domain(&current, mode)?);
                check_observation_deadline(deadline)?;
                if domains.len() > MAX_DOMAINS {
                    return Err(CoordinationFailure::CapacityExceeded {
                        required: domains.len(),
                        limit: MAX_DOMAINS,
                    });
                }
                if current == *root {
                    break;
                }
                current = current
                    .parent()
                    .filter(|parent| parent.starts_with(root))
                    .ok_or_else(|| {
                        unavailable(
                            &current,
                            io::Error::new(
                                io::ErrorKind::PermissionDenied,
                                "fixture ancestor escaped its namespace",
                            ),
                        )
                    })?
                    .to_path_buf();
            }
        }
    }
    Ok(domains)
}

fn production_ancestor_is_domain(path: &Path) -> bool {
    path != Path::new("/")
}

fn observe_effect(
    effect: &DirectoryEffect,
    scope: &ObservationScope,
    deadline: Option<&CoordinationDeadline>,
) -> Result<ObservedEffect, CoordinationFailure> {
    check_observation_deadline(deadline)?;
    if !effect.path().is_absolute() {
        return Err(unavailable(
            effect.path(),
            io::Error::new(io::ErrorKind::InvalidInput, "effect path must be absolute"),
        ));
    }
    #[cfg(test)]
    if let ObservationScope::Fixture {
        lexical_root, root, ..
    } = scope
    {
        if !effect.path().starts_with(lexical_root) && !effect.path().starts_with(root) {
            return Err(unavailable(
                effect.path(),
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "fixture effect escaped its namespace",
                ),
            ));
        }
    }
    let (existing_lexical, resolved, resolved_id, missing_suffix) = match effect {
        DirectoryEffect::Tree { path, .. } => {
            let (resolved, id) = existing_directory(path, deadline)?;
            (path.clone(), resolved, id, PathBuf::new())
        }
        DirectoryEffect::Entry { path, .. } => nearest_existing_parent(path, deadline)?,
    };
    check_observation_deadline(deadline)?;
    let domains = observe_ancestors(&resolved, effect.mode(), scope, deadline)?;
    Ok(ObservedEffect {
        binding: EffectBinding {
            requested: effect.path().to_path_buf(),
            existing_lexical,
            resolved,
            resolved_id,
            missing_suffix,
        },
        domains,
    })
}

#[cfg(test)]
fn merge_observed_effects(
    observed: Vec<ObservedEffect>,
) -> Result<Observation, CoordinationFailure> {
    let mut merged: BTreeMap<PhysicalObjectId, ObservedDomain> = BTreeMap::new();
    let mut bindings = Vec::with_capacity(observed.len());
    for effect in observed {
        merge_observed_effect(&mut merged, &mut bindings, effect)?;
    }
    finish_observation(merged, bindings)
}

fn merge_observed_effect(
    merged: &mut BTreeMap<PhysicalObjectId, ObservedDomain>,
    bindings: &mut Vec<EffectBinding>,
    effect: ObservedEffect,
) -> Result<(), CoordinationFailure> {
    if effect.domains.is_empty() {
        return Err(CoordinationFailure::FilesystemRootRequired);
    }
    bindings.push(effect.binding);
    for domain in effect.domains {
        merged
            .entry(domain.id)
            .and_modify(|current| current.mode = current.mode.max(domain.mode))
            .or_insert(domain);
        if merged.len() > MAX_DOMAINS {
            return Err(CoordinationFailure::CapacityExceeded {
                required: merged.len(),
                limit: MAX_DOMAINS,
            });
        }
    }
    Ok(())
}

fn finish_observation(
    merged: BTreeMap<PhysicalObjectId, ObservedDomain>,
    bindings: Vec<EffectBinding>,
) -> Result<Observation, CoordinationFailure> {
    if merged.is_empty() {
        return Err(CoordinationFailure::FilesystemRootRequired);
    }
    Ok(Observation {
        domains: merged.into_values().collect(),
        bindings,
        files: Vec::new(),
    })
}

fn observe_effects(
    effects: &[DirectoryEffect],
    scope: &ObservationScope,
    deadline: Option<&CoordinationDeadline>,
) -> Result<Observation, CoordinationFailure> {
    observe_effects_with(effects, scope, deadline, observe_effect)
}

fn observe_effects_with<F>(
    effects: &[DirectoryEffect],
    scope: &ObservationScope,
    deadline: Option<&CoordinationDeadline>,
    mut observer: F,
) -> Result<Observation, CoordinationFailure>
where
    F: FnMut(
        &DirectoryEffect,
        &ObservationScope,
        Option<&CoordinationDeadline>,
    ) -> Result<ObservedEffect, CoordinationFailure>,
{
    let mut merged = BTreeMap::new();
    let mut bindings = Vec::with_capacity(effects.len().min(MAX_DOMAINS));
    for effect in effects {
        check_observation_deadline(deadline)?;
        let observed = observer(effect, scope, deadline)?;
        check_observation_deadline(deadline)?;
        merge_observed_effect(&mut merged, &mut bindings, observed)?;
    }
    finish_observation(merged, bindings)
}

fn check_observation_deadline(
    deadline: Option<&CoordinationDeadline>,
) -> Result<(), CoordinationFailure> {
    if let Some(deadline) = deadline {
        deadline.check_cancelled()?;
    }
    if deadline.is_some_and(CoordinationDeadline::expired) {
        Err(CoordinationFailure::DeadlineExceeded)
    } else {
        Ok(())
    }
}

fn same_observation(expected: &Observation, current: &Observation) -> bool {
    expected.bindings == current.bindings
        && expected.files == current.files
        && expected.domains.len() == current.domains.len()
        && expected
            .domains
            .iter()
            .zip(&current.domains)
            .all(|(left, right)| {
                left.id == right.id
                    && left.mode == right.mode
                    && left.path == right.path
                    && left.kind == right.kind
            })
}

fn same_directory_observation(expected: &Observation, current: &Observation) -> bool {
    let expected_domains: Vec<_> = expected
        .domains
        .iter()
        .filter(|domain| domain.kind == DomainKind::Directory)
        .collect();
    expected.bindings == current.bindings
        && expected_domains.len() == current.domains.len()
        && expected_domains
            .iter()
            .zip(&current.domains)
            .all(|(left, right)| {
                left.id == right.id
                    && left.mode == right.mode
                    && left.path == right.path
                    && right.kind == DomainKind::Directory
            })
}

fn scoped_failure(error: ScopedReadError) -> CoordinationFailure {
    match error {
        ScopedReadError::Io(source) => unavailable(Path::new("<scoped-file>"), source),
        ScopedReadError::Missing { path, source } => unavailable(&path, source),
        ScopedReadError::LinkTarget { link, source, .. } => unavailable(&link, source),
    }
}

fn revalidate_directory_files(
    scope: &SkillReadScope,
    files: &[ScopedContentObservation],
    deadline: Option<&CoordinationDeadline>,
) -> Result<(), CoordinationFailure> {
    for file in files {
        check_observation_deadline(deadline)?;
        let current = scope
            .observe_content_regular(&file.requested)
            .map_err(scoped_failure)?;
        if &current != file {
            return Err(CoordinationFailure::Changed);
        }
        check_observation_deadline(deadline)?;
    }
    Ok(())
}

fn observe_scoped_files(
    scope: &SkillReadScope,
    paths: &[PathBuf],
    deadline: Option<&CoordinationDeadline>,
) -> Result<Vec<ScopedFileObservation>, CoordinationFailure> {
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        check_observation_deadline(deadline)?;
        files.push(scope.observe_regular(path).map_err(scoped_failure)?);
        check_observation_deadline(deadline)?;
    }
    files.sort_by(|left, right| {
        SkillReadScope::observed_requested(left).cmp(SkillReadScope::observed_requested(right))
    });
    Ok(files)
}

fn merge_file_observation(
    mut directory: Observation,
    files: Vec<ScopedFileObservation>,
    mode: CoordinationMode,
) -> Result<Observation, CoordinationFailure> {
    let mut merged: BTreeMap<PhysicalObjectId, ObservedDomain> = directory
        .domains
        .into_iter()
        .map(|domain| (domain.id, domain))
        .collect();
    for file in &files {
        let (device, inode, link_count) = SkillReadScope::observed_identity(file);
        if mode == CoordinationMode::Exclusive || link_count > 1 {
            let id = PhysicalObjectId { device, inode };
            let domain = ObservedDomain {
                id,
                mode,
                path: SkillReadScope::observed_resolved(file).to_path_buf(),
                kind: DomainKind::File { link_count },
                file: Some(file.clone()),
            };
            merged
                .entry(id)
                .and_modify(|current| current.mode = current.mode.max(mode))
                .or_insert(domain);
        }
        if merged.len() > MAX_DOMAINS {
            return Err(CoordinationFailure::CapacityExceeded {
                required: merged.len(),
                limit: MAX_DOMAINS,
            });
        }
    }
    directory.domains = merged.into_values().collect();
    directory.files = files;
    Ok(directory)
}

fn shared_tree_roots(
    effects: &[DirectoryEffect],
    bindings: &[EffectBinding],
) -> Vec<(PathBuf, PathBuf)> {
    effects
        .iter()
        .zip(bindings)
        .filter(|(effect, _)| {
            matches!(
                effect,
                DirectoryEffect::Tree {
                    mode: CoordinationMode::Shared,
                    ..
                }
            )
        })
        .map(|(effect, binding)| (effect.path().to_path_buf(), binding.resolved.clone()))
        .collect()
}

fn effects_with_resolved_parents(
    mut effects: Vec<DirectoryEffect>,
    bindings: &[EffectBinding],
    files: &[ScopedFileObservation],
    mode: CoordinationMode,
) -> Result<Vec<DirectoryEffect>, CoordinationFailure> {
    let covered = shared_tree_roots(&effects, bindings);
    for file in files {
        let resolved = SkillReadScope::observed_resolved(file);
        let parent = resolved.parent().ok_or_else(|| {
            unavailable(
                resolved,
                io::Error::new(io::ErrorKind::InvalidInput, "file has no parent directory"),
            )
        })?;
        if mode == CoordinationMode::Shared
            && covered
                .iter()
                .any(|(_, physical)| parent.starts_with(physical))
        {
            continue;
        }
        let effect = DirectoryEffect::tree(parent, mode);
        if !effects.contains(&effect) {
            effects.push(effect);
        }
    }
    Ok(effects)
}

fn plan_covered_files(
    effects: Vec<DirectoryEffect>,
    scope: &ObservationScope,
    deadline: &CoordinationDeadline,
    read_scope: &SkillReadScope,
    paths: &[PathBuf],
    mode: CoordinationMode,
) -> Result<(Vec<DirectoryEffect>, Observation), CoordinationFailure> {
    plan_covered_files_with(effects, scope, deadline, read_scope, paths, mode, || {})
}

fn plan_covered_files_with<F>(
    mut effects: Vec<DirectoryEffect>,
    scope: &ObservationScope,
    deadline: &CoordinationDeadline,
    read_scope: &SkillReadScope,
    paths: &[PathBuf],
    mode: CoordinationMode,
    mut before_replan: F,
) -> Result<(Vec<DirectoryEffect>, Observation), CoordinationFailure>
where
    F: FnMut(),
{
    loop {
        let directory_guard =
            CoordinationPlan::new_in_scope(effects.clone(), scope.clone(), deadline.clone())?
                .acquire()?;
        let files = observe_scoped_files(read_scope, paths, Some(deadline))?;
        let next_effects = effects_with_resolved_parents(
            effects.clone(),
            &directory_guard.observation.bindings,
            &files,
            mode,
        )?;
        if next_effects != effects {
            drop(directory_guard);
            effects = next_effects;
            before_replan();
            continue;
        }
        let observation = merge_file_observation(directory_guard.observation.clone(), files, mode)?;
        drop(directory_guard);
        return Ok((effects, observation));
    }
}

fn acquire_complete_read(
    mut effects: Vec<DirectoryEffect>,
    scope: ObservationScope,
    deadline: CoordinationDeadline,
    read_scope: &SkillReadScope,
    paths: &[PathBuf],
    mode: CoordinationMode,
    mut expected: Observation,
) -> Result<CoordinatedReadGuard, CoordinationFailure> {
    let mut saw_contention = false;
    let mut saw_change = false;
    loop {
        deadline.check_cancelled()?;
        if deadline.expired() {
            return Err(expired_failure(saw_contention, saw_change));
        }
        let gate = match acquire_gate(strongest_mode(&expected.domains), &deadline)? {
            Some(gate) => gate,
            None => return Err(CoordinationFailure::Busy),
        };
        match acquire_native(&expected.domains, &deadline, Some(read_scope)) {
            Ok(leases) => {
                let directories = observe_effects(&effects, &scope, Some(&deadline));
                let current = directories.and_then(|directories| {
                    let files = observe_scoped_files(read_scope, paths, Some(&deadline))?;
                    let next_effects = effects_with_resolved_parents(
                        effects.clone(),
                        &directories.bindings,
                        &files,
                        mode,
                    )?;
                    if next_effects != effects {
                        return Ok((next_effects, None));
                    }
                    merge_file_observation(directories, files, mode)
                        .map(|observation| (effects.clone(), Some(observation)))
                });
                deadline.check_cancelled()?;
                match current {
                    Ok((_, Some(current)))
                        if same_observation(&expected, &current) && !deadline.expired() =>
                    {
                        return Ok(CoordinatedReadGuard {
                            file_mode: mode,
                            files: current.files.clone(),
                            directory_files: Vec::new(),
                            guard: CoordinationGuard {
                                _leases: leases,
                                _gate: gate,
                                effects,
                                observation: current,
                                scope,
                                deadline,
                            },
                        });
                    }
                    Ok((_, Some(current))) if same_observation(&expected, &current) => {
                        return Err(expired_failure(saw_contention, saw_change));
                    }
                    Err(CoordinationFailure::DeadlineExceeded) => {
                        return Err(expired_failure(saw_contention, saw_change));
                    }
                    Ok((next_effects, current)) => {
                        drop(leases);
                        drop(gate);
                        saw_change = true;
                        if let Some(current) = current {
                            expected = current;
                        } else {
                            effects = next_effects;
                            let planned = plan_covered_files(
                                effects, &scope, &deadline, read_scope, paths, mode,
                            )
                            .map_err(|error| {
                                preserve_deadline_evidence(error, saw_contention, saw_change)
                            })?;
                            effects = planned.0;
                            expected = planned.1;
                        }
                    }
                    Err(error) if observation_may_have_changed(&error) => {
                        drop(leases);
                        drop(gate);
                        saw_change = true;
                        let planned =
                            plan_covered_files(effects, &scope, &deadline, read_scope, paths, mode)
                                .map_err(|error| {
                                    preserve_deadline_evidence(error, saw_contention, saw_change)
                                })?;
                        effects = planned.0;
                        expected = planned.1;
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(NativeAcquire::Busy) => {
                drop(gate);
                saw_contention = true;
                backoff(&deadline, saw_contention, saw_change)?;
                let planned =
                    plan_covered_files(effects, &scope, &deadline, read_scope, paths, mode)
                        .map_err(|error| {
                            preserve_deadline_evidence(error, saw_contention, saw_change)
                        })?;
                effects = planned.0;
                expected = planned.1;
                continue;
            }
            Err(NativeAcquire::Changed) => {
                drop(gate);
                saw_change = true;
                backoff(&deadline, saw_contention, saw_change)?;
                let planned =
                    plan_covered_files(effects, &scope, &deadline, read_scope, paths, mode)
                        .map_err(|error| {
                            preserve_deadline_evidence(error, saw_contention, saw_change)
                        })?;
                effects = planned.0;
                expected = planned.1;
                continue;
            }
            Err(NativeAcquire::Deadline) => {
                drop(gate);
                return Err(expired_failure(saw_contention, saw_change));
            }
            Err(NativeAcquire::Unavailable(error)) => {
                drop(gate);
                return Err(error);
            }
        }
        backoff(&deadline, saw_contention, saw_change)?;
    }
}

fn acquire_gate(
    mode: CoordinationMode,
    deadline: &CoordinationDeadline,
) -> Result<Option<ProcessGateGuard>, CoordinationFailure> {
    let gate = PROCESS_GATE.get_or_init(|| RwLock::new(()));
    acquire_gate_on(gate, mode, deadline)
}

fn acquire_gate_on(
    gate: &'static RwLock<()>,
    mode: CoordinationMode,
    deadline: &CoordinationDeadline,
) -> Result<Option<ProcessGateGuard>, CoordinationFailure> {
    loop {
        deadline.check_cancelled()?;
        let acquired = match mode {
            CoordinationMode::Shared => match gate.try_read() {
                Ok(guard) => Some(ProcessGateGuard::Shared { _guard: guard }),
                Err(std::sync::TryLockError::WouldBlock) => None,
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err(unavailable(
                        Path::new("<process-gate>"),
                        io::Error::other("process coordination gate is poisoned"),
                    ));
                }
            },
            CoordinationMode::Exclusive => match gate.try_write() {
                Ok(guard) => Some(ProcessGateGuard::Exclusive { _guard: guard }),
                Err(std::sync::TryLockError::WouldBlock) => None,
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err(unavailable(
                        Path::new("<process-gate>"),
                        io::Error::other("process coordination gate is poisoned"),
                    ));
                }
            },
        };
        if acquired.is_some() || deadline.expired() {
            return Ok(acquired);
        }
        thread::sleep(Duration::from_millis(2));
    }
}

#[derive(Debug)]
enum NativeAcquire {
    Busy,
    Changed,
    Deadline,
    Unavailable(CoordinationFailure),
}

fn acquire_native(
    domains: &[ObservedDomain],
    deadline: &CoordinationDeadline,
    read_scope: Option<&SkillReadScope>,
) -> Result<Vec<File>, NativeAcquire> {
    let mut leases = Vec::with_capacity(domains.len());
    for domain in domains {
        deadline
            .check_cancelled()
            .map_err(NativeAcquire::Unavailable)?;
        if deadline.expired() {
            return Err(NativeAcquire::Deadline);
        }
        let file = match (&domain.kind, &domain.file) {
            (DomainKind::Directory, None) => fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NONBLOCK | libc::O_NOFOLLOW)
                .open(&domain.path)
                .map_err(|error| {
                    if binding_open_may_have_changed(&error) {
                        NativeAcquire::Changed
                    } else {
                        NativeAcquire::Unavailable(unavailable(&domain.path, error))
                    }
                })?,
            (DomainKind::File { .. }, Some(observation)) => read_scope
                .ok_or_else(|| {
                    NativeAcquire::Unavailable(unavailable(
                        &domain.path,
                        io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "missing scoped file lock handle",
                        ),
                    ))
                })?
                .open_observed_lock(observation)
                .map_err(|error| match error {
                    ScopedLockOpenError::Changed => NativeAcquire::Changed,
                    ScopedLockOpenError::Unavailable(error) => {
                        NativeAcquire::Unavailable(scoped_lock_failure(&domain.path, error))
                    }
                })?,
            _ => {
                return Err(NativeAcquire::Unavailable(unavailable(
                    &domain.path,
                    io::Error::new(io::ErrorKind::InvalidInput, "invalid coordination domain"),
                )));
            }
        };
        let metadata = file.metadata().map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                NativeAcquire::Changed
            } else {
                NativeAcquire::Unavailable(unavailable(&domain.path, error))
            }
        })?;
        let correct_type = match domain.kind {
            DomainKind::Directory => metadata.is_dir(),
            DomainKind::File { link_count } => metadata.is_file() && metadata.nlink() == link_count,
        };
        if !correct_type || metadata_id(&metadata) != domain.id {
            return Err(NativeAcquire::Changed);
        }
        let result = match domain.mode {
            CoordinationMode::Shared => file.try_lock_shared(),
            CoordinationMode::Exclusive => file.try_lock(),
        };
        match result {
            Ok(()) => leases.push(file),
            Err(std::fs::TryLockError::WouldBlock) => return Err(NativeAcquire::Busy),
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(NativeAcquire::Unavailable(unavailable(&domain.path, error)));
            }
        }
        deadline
            .check_cancelled()
            .map_err(NativeAcquire::Unavailable)?;
        if deadline.expired() {
            return Err(NativeAcquire::Deadline);
        }
    }
    Ok(leases)
}

fn binding_open_may_have_changed(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
        || matches!(error.raw_os_error(), Some(code) if code == libc::ELOOP || code == libc::ENOTDIR)
}

fn scoped_lock_failure(path: &Path, error: ScopedReadError) -> CoordinationFailure {
    let source = match error {
        ScopedReadError::Io(source)
        | ScopedReadError::Missing { source, .. }
        | ScopedReadError::LinkTarget { source, .. } => source,
    };
    unavailable(path, source)
}

fn backoff(
    deadline: &CoordinationDeadline,
    saw_contention: bool,
    saw_change: bool,
) -> Result<(), CoordinationFailure> {
    deadline.check_cancelled()?;
    if deadline.expired() {
        return Err(expired_failure(saw_contention, saw_change));
    }
    thread::sleep(deadline.remaining().min(Duration::from_millis(2)));
    Ok(())
}

#[cfg(all(unix, feature = "event-store"))]
impl FinalizedWriteLease<'_> {
    pub fn backup_documents(
        &self,
        root: &crate::skill_backup_reservation::BackupStateRoot,
        id: &str,
        sources: Vec<crate::skill_backup_source::BackupSource>,
        limits: crate::skill_backup_reservation::BackupCopyLimits,
    ) -> Result<crate::skill_event::BackupManifest, String> {
        self.backup_documents_prepared(root, id, sources, limits)
            .map_err(|error| error.to_string())
    }
    pub fn backup_documents_prepared(
        &self,
        root: &crate::skill_backup_reservation::BackupStateRoot,
        id: &str,
        sources: Vec<crate::skill_backup_source::BackupSource>,
        limits: crate::skill_backup_reservation::BackupCopyLimits,
    ) -> Result<crate::skill_event::BackupManifest, PreparedContentError> {
        self.backup_documents_with_absent_sidecar_prepared_retained(
            root, id, sources, limits, None, false,
        )
        .map(|(manifest, _reservation)| manifest)
    }

    pub(crate) fn backup_documents_retained_prepared<'root>(
        &self,
        root: &'root crate::skill_backup_reservation::BackupStateRoot,
        id: &str,
        sources: Vec<crate::skill_backup_source::BackupSource>,
        limits: crate::skill_backup_reservation::BackupCopyLimits,
    ) -> Result<
        (
            crate::skill_event::BackupManifest,
            crate::skill_backup_reservation::ReservedBackup<'root>,
        ),
        PreparedContentError,
    > {
        self.backup_documents_with_absent_sidecar_prepared_retained(
            root, id, sources, limits, None, true,
        )
    }

    pub(crate) fn backup_documents_with_absent_sidecar_prepared(
        &self,
        root: &crate::skill_backup_reservation::BackupStateRoot,
        id: &str,
        sources: Vec<crate::skill_backup_source::BackupSource>,
        limits: crate::skill_backup_reservation::BackupCopyLimits,
        absent_sidecar: Option<&Path>,
    ) -> Result<crate::skill_event::BackupManifest, PreparedContentError> {
        self.backup_documents_with_absent_sidecar_prepared_retained(
            root,
            id,
            sources,
            limits,
            absent_sidecar,
            false,
        )
        .map(|(manifest, _reservation)| manifest)
    }

    fn backup_documents_with_absent_sidecar_prepared_retained<'root>(
        &self,
        root: &'root crate::skill_backup_reservation::BackupStateRoot,
        id: &str,
        sources: Vec<crate::skill_backup_source::BackupSource>,
        limits: crate::skill_backup_reservation::BackupCopyLimits,
        absent_sidecar: Option<&Path>,
        discard_unrecorded: bool,
    ) -> Result<
        (
            crate::skill_event::BackupManifest,
            crate::skill_backup_reservation::ReservedBackup<'root>,
        ),
        PreparedContentError,
    > {
        if let Some(path) = absent_sidecar {
            self.validate_invocation_creation(path)
                .map_err(PreparedContentError::from)?;
        }
        self.validate_state_tree_prepared(&root.path)?;
        if sources.is_empty() || sources.len() > self.guard.files.len() {
            return Err("Backup documents must be a nonempty subset of planned files".into());
        }
        let mut unique = std::collections::BTreeSet::new();
        for source in &sources {
            if !unique.insert(source.original_path.clone()) {
                return Err("Backup documents must be unique".into());
            }
            source.revalidate().map_err(PreparedContentError::from)?;
            if !source
                .directory
                .symlink_metadata(&source.name)
                .map_err(PreparedContentError::from)?
                .is_file()
            {
                return Err("Backup document must be a regular file entry".into());
            }
            self.validate_document(&source.original_path)
                .map_err(PreparedContentError::from)?;
        }
        let cancellation = self
            .guard
            .guard
            .deadline
            .cancellation
            .clone()
            .unwrap_or_default();
        let mut builder = crate::skill_backup_manifest::BackupManifestBuilder::new(
            root,
            id,
            limits,
            cancellation,
        )
        .map_err(PreparedContentError::from)?;
        if discard_unrecorded {
            builder = builder.discard_unrecorded_on_failure();
        }
        for source in sources {
            builder = builder
                .add_source(source)
                .map_err(PreparedContentError::from)?;
        }
        if let Some(path) = absent_sidecar {
            builder = builder
                .add_absent_invocation_sidecar(self, path)
                .map_err(PreparedContentError::from)?;
        }
        if let Err(error) = self.revalidate() {
            return Err(PreparedContentError::from(
                builder.discard_after_failure(std::io::Error::other(error)),
            ));
        }
        let (manifest, reservation) = builder
            .finish_retained()
            .map_err(PreparedContentError::from)?;
        if let Err(error) = self.revalidate() {
            if !discard_unrecorded {
                return Err(PreparedContentError::from(error));
            }
            return match reservation.discard() {
                Ok(()) => Err(PreparedContentError::from(error)),
                Err(cleanup) => Err(PreparedContentError::from(format!(
                    "{error}; backup cleanup refused: {cleanup}"
                ))),
            };
        }
        Ok((manifest, reservation))
    }
}

#[cfg(any(test, feature = "event-store"))]
impl FinalizedReadLease<'_> {
    pub(crate) fn check_cancelled(&self) -> Result<(), CoordinationFailure> {
        self.guard.check_cancelled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::os::unix::fs::symlink;
    use std::process::{Child, Command, Stdio};
    use std::sync::{mpsc, Mutex, MutexGuard};
    use tempfile::TempDir;

    static TEST_SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> MutexGuard<'static, ()> {
        TEST_SERIAL
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn fixture() -> TempDir {
        tempfile::tempdir().expect("fixture")
    }

    fn fixture_plan(
        root: &TempDir,
        effects: Vec<DirectoryEffect>,
    ) -> Result<CoordinationPlan, CoordinationFailure> {
        CoordinationPlan::new_fixture(effects, root.path(), None)
    }

    fn fixture_plan_with_timeout(
        root: &TempDir,
        effects: Vec<DirectoryEffect>,
        timeout: Duration,
    ) -> Result<CoordinationPlan, CoordinationFailure> {
        CoordinationPlan::new_fixture(effects, root.path(), Some(timeout))
    }

    fn tree_plan(root: &TempDir, path: &Path, mode: CoordinationMode) -> CoordinationPlan {
        fixture_plan(root, vec![DirectoryEffect::tree(path, mode)]).expect("fixture plan")
    }

    fn tree_plan_with_timeout(
        root: &TempDir,
        path: &Path,
        mode: CoordinationMode,
        timeout: Duration,
    ) -> CoordinationPlan {
        fixture_plan_with_timeout(root, vec![DirectoryEffect::tree(path, mode)], timeout)
            .expect("fixture plan")
    }

    #[derive(Debug, Eq, PartialEq)]
    enum FixtureNode {
        Directory,
        File(Vec<u8>),
        Symlink(PathBuf),
    }

    fn fixture_snapshot(root: &Path) -> BTreeMap<PathBuf, FixtureNode> {
        fn visit(root: &Path, directory: &Path, snapshot: &mut BTreeMap<PathBuf, FixtureNode>) {
            for entry in fs::read_dir(directory).expect("fixture directory") {
                let entry = entry.expect("fixture entry");
                let path = entry.path();
                let relative = path
                    .strip_prefix(root)
                    .expect("fixture relative")
                    .to_path_buf();
                let metadata = fs::symlink_metadata(&path).expect("fixture metadata");
                if metadata.file_type().is_symlink() {
                    snapshot.insert(
                        relative,
                        FixtureNode::Symlink(fs::read_link(&path).expect("fixture link")),
                    );
                } else if metadata.is_dir() {
                    snapshot.insert(relative, FixtureNode::Directory);
                    visit(root, &path, snapshot);
                } else {
                    snapshot.insert(
                        relative,
                        FixtureNode::File(fs::read(&path).expect("fixture file")),
                    );
                }
            }
        }

        let mut snapshot = BTreeMap::new();
        visit(root, root, &mut snapshot);
        snapshot
    }

    struct HeldChild {
        child: Option<Child>,
    }

    impl HeldChild {
        fn start(root: &Path, path: &Path, mode: CoordinationMode) -> Self {
            Self::spawn(root, path, mode, false, false).unwrap_or_else(|error| panic!("{error}"))
        }

        fn raw(path: &Path, mode: CoordinationMode) -> Self {
            Self::spawn(path, path, mode, true, false).unwrap_or_else(|error| panic!("{error}"))
        }

        fn file(root: &Path, path: &Path, mode: CoordinationMode) -> Self {
            Self::spawn(root, path, mode, false, true).unwrap_or_else(|error| panic!("{error}"))
        }

        fn expect_busy(root: &Path, path: &Path, mode: CoordinationMode) {
            match Self::spawn(root, path, mode, false, false) {
                Err(error) if error.contains("Busy") || error.contains("WouldBlock") => {}
                Err(error) => panic!("child failed for an unexpected reason: {error}"),
                Ok(mut child) => {
                    child.finish(false);
                    panic!("child unexpectedly acquired its lease");
                }
            }
        }

        fn expect_raw_busy(path: &Path, mode: CoordinationMode) {
            match Self::spawn(path, path, mode, true, false) {
                Err(error) if error.contains("Busy") || error.contains("WouldBlock") => {}
                Err(error) => panic!("raw child failed for an unexpected reason: {error}"),
                Ok(mut child) => {
                    child.finish(false);
                    panic!("raw child unexpectedly acquired its lease");
                }
            }
        }

        fn spawn(
            root: &Path,
            path: &Path,
            mode: CoordinationMode,
            raw: bool,
            file: bool,
        ) -> Result<Self, String> {
            let mode = match mode {
                CoordinationMode::Shared => "shared",
                CoordinationMode::Exclusive => "exclusive",
            };
            let mut child =
                Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
                    .arg("--exact")
                    .arg("skill_coordination::tests::lock_child")
                    .arg("--ignored")
                    .arg("--nocapture")
                    .env("SKILL_STUDIO_COORDINATION_CHILD_ROOT", root)
                    .env("SKILL_STUDIO_COORDINATION_CHILD_PATH", path)
                    .env("SKILL_STUDIO_COORDINATION_CHILD_MODE", mode)
                    .env("SKILL_STUDIO_COORDINATION_CHILD_RAW", raw.to_string())
                    .env("SKILL_STUDIO_COORDINATION_CHILD_FILE", file.to_string())
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .map_err(|error| error.to_string())?;
            let stdout = child.stdout.take().ok_or("child stdout was not captured")?;
            let (sender, receiver) = mpsc::channel();
            thread::spawn(move || {
                let mut reported = false;
                for line in BufReader::new(stdout).lines() {
                    match line {
                        Ok(line) if line == "READY" && !reported => {
                            let _ = sender.send(Ok(()));
                            reported = true;
                        }
                        Ok(line) if line.starts_with("ACQUIRE_ERROR:") && !reported => {
                            let _ = sender.send(Err(line));
                            reported = true;
                        }
                        Ok(_) => {}
                        Err(error) => {
                            if !reported {
                                let _ = sender.send(Err(format!("child output failed: {error}")));
                            }
                            return;
                        }
                    }
                }
                if !reported {
                    let _ = sender.send(Err("child exited before readiness".to_owned()));
                }
            });
            match receiver.recv_timeout(Duration::from_secs(3)) {
                Ok(Ok(())) => Ok(Self { child: Some(child) }),
                Ok(Err(error)) => {
                    let _ = stop_child(&mut child, true);
                    Err(error)
                }
                Err(error) => {
                    let _ = stop_child(&mut child, true);
                    Err(format!("child readiness timed out: {error}"))
                }
            }
        }

        fn finish(&mut self, crash: bool) {
            if let Some(mut child) = self.child.take() {
                if !crash {
                    drop(child.stdin.take());
                }
                let status = stop_child(&mut child, crash).expect("child must terminate");
                if !crash {
                    assert!(status.success(), "child normal exit failed: {status}");
                }
            }
        }
    }

    impl Drop for HeldChild {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = stop_child(&mut child, true);
            }
        }
    }

    fn stop_child(child: &mut Child, kill_first: bool) -> Result<std::process::ExitStatus, String> {
        if kill_first {
            let _ = child.kill();
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Ok(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("child did not exit before its deadline".to_owned());
                }
                Err(error) => {
                    let _ = child.kill();
                    let wait_result = child.wait();
                    return Err(match wait_result {
                        Ok(status) => {
                            format!("child status failed before reap ({status}): {error}")
                        }
                        Err(wait_error) => {
                            format!("child status and reap failed: {error}; {wait_error}")
                        }
                    });
                }
            }
        }
    }

    #[test]
    #[ignore = "subprocess helper"]
    fn lock_child() {
        let Ok(root) = std::env::var("SKILL_STUDIO_COORDINATION_CHILD_ROOT") else {
            return;
        };
        let path = PathBuf::from(
            std::env::var("SKILL_STUDIO_COORDINATION_CHILD_PATH").expect("child path"),
        );
        let mode = match std::env::var("SKILL_STUDIO_COORDINATION_CHILD_MODE").as_deref() {
            Ok("shared") => CoordinationMode::Shared,
            Ok("exclusive") => CoordinationMode::Exclusive,
            _ => panic!("child mode"),
        };
        let raw = std::env::var("SKILL_STUDIO_COORDINATION_CHILD_RAW").as_deref() == Ok("true");
        let file = std::env::var("SKILL_STUDIO_COORDINATION_CHILD_FILE").as_deref() == Ok("true");
        let lease = if raw {
            match fs::OpenOptions::new().read(true).open(&path) {
                Ok(file) => match mode {
                    CoordinationMode::Shared => file.try_lock_shared(),
                    CoordinationMode::Exclusive => file.try_lock(),
                }
                .map(|()| ChildLease::Native(vec![file]))
                .map_err(|error| format!("{error:?}")),
                Err(error) => Err(error.to_string()),
            }
        } else if file {
            let root_path = PathBuf::from(&root);
            SkillReadScope::bind(std::slice::from_ref(&root_path))
                .map_err(|error| error.to_string())
                .and_then(|scope| {
                    CoordinationPlan::new_fixture(
                        vec![DirectoryEffect::tree(&root_path, mode)],
                        &root_path,
                        Some(Duration::from_millis(250)),
                    )
                    .and_then(CoordinationPlan::acquire)
                    .and_then(|guard| guard.continue_with_files(&scope, &[path], mode))
                    .map(ChildLease::Read)
                    .map_err(|error| error.to_string())
                })
        } else {
            CoordinationPlan::new_fixture(
                vec![DirectoryEffect::tree(path, mode)],
                Path::new(&root),
                Some(Duration::from_millis(250)),
            )
            .and_then(CoordinationPlan::acquire)
            .map(ChildLease::Guard)
            .map_err(|error| format!("{error:?}"))
        };
        let lease = match lease {
            Ok(lease) => lease,
            Err(error) => {
                println!("\nACQUIRE_ERROR:{error}");
                return;
            }
        };
        assert!(lease.domain_count() > 0);
        println!("\nREADY");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).expect("child stdin");
        drop(lease);
    }

    enum ChildLease {
        Guard(CoordinationGuard),
        Read(CoordinatedReadGuard),
        Native(Vec<File>),
    }

    impl ChildLease {
        fn domain_count(&self) -> usize {
            match self {
                Self::Guard(guard) => guard._leases.len(),
                Self::Read(guard) => guard.guard._leases.len(),
                Self::Native(leases) => leases.len(),
            }
        }
    }

    #[test]
    fn synthetic_merge_order_modes_root_and_capacity() {
        let id = |inode| PhysicalObjectId { device: 7, inode };
        let binding = |name: &str, inode| EffectBinding {
            requested: PathBuf::from(name),
            existing_lexical: PathBuf::from(name),
            resolved: PathBuf::from(name),
            resolved_id: id(inode),
            missing_suffix: PathBuf::new(),
        };
        let domain = |inode, mode| ObservedDomain {
            id: id(inode),
            mode,
            path: PathBuf::from(format!("/synthetic/{inode}")),
            kind: DomainKind::Directory,
            file: None,
        };
        let observation = merge_observed_effects(vec![
            ObservedEffect {
                binding: binding("/synthetic/alias-a", 2),
                domains: vec![
                    domain(2, CoordinationMode::Shared),
                    domain(1, CoordinationMode::Shared),
                ],
            },
            ObservedEffect {
                binding: binding("/synthetic/alias-b", 2),
                domains: vec![
                    domain(2, CoordinationMode::Exclusive),
                    domain(1, CoordinationMode::Exclusive),
                ],
            },
        ])
        .expect("synthetic observation");
        assert_eq!(observation.domains.len(), 2);
        assert!(observation
            .domains
            .windows(2)
            .all(|pair| pair[0].id < pair[1].id));
        assert!(observation
            .domains
            .iter()
            .all(|domain| domain.mode == CoordinationMode::Exclusive));
        assert_eq!(observation.bindings.len(), 2);
        for root_required in ["/", "/new-entry"] {
            assert!(matches!(
                merge_observed_effects(vec![ObservedEffect {
                    binding: binding(root_required, 0),
                    domains: Vec::new(),
                }]),
                Err(CoordinationFailure::FilesystemRootRequired)
            ));
        }
        let direct_child_tree = merge_observed_effects(vec![ObservedEffect {
            binding: binding("/direct-child", 8),
            domains: vec![domain(8, CoordinationMode::Shared)],
        }])
        .expect("an existing direct-child tree uses its own domain");
        assert_eq!(direct_child_tree.domains.len(), 1);
        assert!(!production_ancestor_is_domain(Path::new("/")));
        assert!(production_ancestor_is_domain(Path::new("/Users")));
        assert!(production_ancestor_is_domain(Path::new(
            "/Users/example/project"
        )));
        let too_many = (0..=MAX_DOMAINS)
            .map(|inode| domain(inode as u64, CoordinationMode::Shared))
            .collect();
        assert!(matches!(
            merge_observed_effects(vec![ObservedEffect {
                binding: binding("/synthetic/deep", 1),
                domains: too_many,
            }]),
            Err(CoordinationFailure::CapacityExceeded {
                required,
                limit: MAX_DOMAINS
            }) if required == MAX_DOMAINS + 1
        ));
    }

    #[test]
    fn fixture_observation_is_confined_and_retains_bindings() {
        let root = fixture();
        let tree = root.path().join("one/two");
        let left = root.path().join("left");
        let right = root.path().join("right");
        fs::create_dir_all(&tree).expect("tree");
        fs::create_dir_all(&left).expect("left");
        fs::create_dir_all(&right).expect("right");
        let alias = root.path().join("alias");
        symlink(&tree, &alias).expect("alias");
        let plan = fixture_plan(
            &root,
            vec![
                DirectoryEffect::tree(&tree, CoordinationMode::Shared),
                DirectoryEffect::tree(&alias, CoordinationMode::Exclusive),
                DirectoryEffect::entry(left.join("moved"), CoordinationMode::Exclusive),
                DirectoryEffect::entry(right.join("moved"), CoordinationMode::Exclusive),
            ],
        )
        .expect("plan");
        let canonical_root = fs::canonicalize(root.path()).expect("canonical root");
        assert!(plan
            .observation
            .domains
            .iter()
            .all(|domain| domain.path.starts_with(&canonical_root)));
        assert!(plan
            .observation
            .domains
            .iter()
            .any(|domain| domain.path == canonical_root));
        assert_eq!(plan.observation.bindings.len(), 4);
        assert_eq!(
            plan.observation
                .domains
                .iter()
                .filter(|domain| domain.path.ends_with("two"))
                .count(),
            1
        );
    }

    #[test]
    fn fixture_scope_rejects_escapes_and_dangling_parents() {
        let root = fixture();
        assert!(matches!(
            CoordinationPlan::new_fixture(
                vec![DirectoryEffect::tree(
                    root.path().parent().expect("parent"),
                    CoordinationMode::Shared
                )],
                root.path(),
                None
            ),
            Err(CoordinationFailure::Unavailable { .. })
        ));
        let dangling = root.path().join("dangling");
        symlink(root.path().join("absent"), &dangling).expect("dangling alias");
        assert!(matches!(
            CoordinationPlan::new_fixture(
                vec![DirectoryEffect::entry(
                    dangling.join("entry"),
                    CoordinationMode::Exclusive
                )],
                root.path(),
                None
            ),
            Err(CoordinationFailure::Unavailable { .. })
        ));
    }

    #[test]
    fn invalid_and_expired_deadlines_are_bounded() {
        let _serial = serial();
        let root = fixture();
        let tree = root.path().join("tree");
        fs::create_dir(&tree).expect("tree");
        let inaccessible = vec![DirectoryEffect::tree(
            "/deadline-must-fail-before-observation",
            CoordinationMode::Shared,
        )];
        assert!(matches!(
            CoordinationPlan::new(inaccessible.clone(), Some(Duration::ZERO)),
            Err(CoordinationFailure::InvalidTimeout)
        ));
        assert!(matches!(
            CoordinationPlan::new(inaccessible, Some(Duration::MAX)),
            Err(CoordinationFailure::InvalidTimeout)
        ));
        let started = Instant::now();
        assert!(CoordinationPlan::new_fixture(
            vec![DirectoryEffect::tree(&tree, CoordinationMode::Shared)],
            root.path(),
            Some(Duration::from_nanos(1))
        )
        .is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn cancelled_plan_fails_before_observing_paths_and_token_cannot_reset() {
        let token = CancellationToken::default();
        let caller = token.clone();
        caller.cancel();
        assert!(token.is_cancelled());
        assert!(matches!(
            CoordinationPlan::new_cancellable(
                vec![DirectoryEffect::tree(
                    "/must-not-observe",
                    CoordinationMode::Shared
                )],
                None,
                token.clone(),
            ),
            Err(CoordinationFailure::Cancelled)
        ));
        assert!(matches!(
            CoordinationPlan::new_cancellable(vec![], None, token),
            Err(CoordinationFailure::Cancelled)
        ));
        assert!(!CancellationToken::default().is_cancelled());
    }

    #[test]
    fn cancellation_stops_a_process_gate_wait_before_its_deadline() {
        let gate: &'static RwLock<()> = Box::leak(Box::new(RwLock::new(())));
        let held = gate.write().unwrap();
        let token = CancellationToken::default();
        let mut deadline = CoordinationDeadline::new(Some(Duration::from_secs(30))).unwrap();
        deadline.cancellation = Some(token.clone());
        let (started, ready) = mpsc::channel();
        let (sent, received) = mpsc::channel();
        let worker = thread::spawn(move || {
            started.send(()).unwrap();
            let cancelled = matches!(
                acquire_gate_on(gate, CoordinationMode::Shared, &deadline),
                Err(CoordinationFailure::Cancelled)
            );
            sent.send(cancelled).unwrap();
        });
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        token.cancel();
        assert!(received.recv_timeout(Duration::from_secs(1)).unwrap());
        drop(held);
        worker.join().unwrap();
    }

    #[test]
    fn cancelling_staged_file_planning_releases_held_directory_leases() {
        let _serial = serial();
        let root = fixture();
        let document = root.path().join("SKILL.md");
        fs::write(&document, "fixture").unwrap();
        let token = CancellationToken::default();
        let mut plan = tree_plan(&root, root.path(), CoordinationMode::Shared);
        plan.deadline.cancellation = Some(token.clone());
        let guard = plan.acquire().unwrap();
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
        token.cancel();
        assert!(matches!(
            guard.continue_with_files(&scope, &[document], CoordinationMode::Shared),
            Err(CoordinationFailure::Cancelled)
        ));
        let mut writer = HeldChild::start(root.path(), root.path(), CoordinationMode::Exclusive);
        writer.finish(false);
        tree_plan(&root, root.path(), CoordinationMode::Shared)
            .acquire()
            .unwrap();
    }

    #[test]
    fn deadline_classification_requires_observed_contention_or_change() {
        for (contention, changed) in [(false, false), (false, true), (true, false), (true, true)] {
            let error = preserve_deadline_evidence(
                CoordinationFailure::DeadlineExceeded,
                contention,
                changed,
            );
            match (contention, changed) {
                (true, _) => assert!(matches!(error, CoordinationFailure::Busy)),
                (false, true) => assert!(matches!(error, CoordinationFailure::Changed)),
                (false, false) => assert!(matches!(error, CoordinationFailure::DeadlineExceeded)),
            }
        }
        assert!(matches!(
            preserve_deadline_evidence(CoordinationFailure::InvalidTimeout, true, true),
            CoordinationFailure::InvalidTimeout
        ));
    }

    #[test]
    fn initial_observation_consumes_the_acquisition_budget() {
        let clock = Arc::new(AtomicU64::new(0));
        let calls = Arc::new(AtomicU64::new(0));
        let observer_clock = Arc::clone(&clock);
        let observer_calls = Arc::clone(&calls);
        let deadline = CoordinationDeadline::manual(clock, 2);
        let effects = vec![
            DirectoryEffect::tree("/synthetic/one", CoordinationMode::Shared),
            DirectoryEffect::tree("/synthetic/two", CoordinationMode::Shared),
        ];
        let result = CoordinationPlan::new_in_scope_with(
            effects,
            ObservationScope::Production,
            deadline,
            move |effect, _, _| {
                let call = observer_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                observer_clock.fetch_add(2, std::sync::atomic::Ordering::SeqCst);
                let id = PhysicalObjectId {
                    device: 9,
                    inode: call + 1,
                };
                Ok(ObservedEffect {
                    binding: EffectBinding {
                        requested: effect.path().to_path_buf(),
                        existing_lexical: effect.path().to_path_buf(),
                        resolved: effect.path().to_path_buf(),
                        resolved_id: id,
                        missing_suffix: PathBuf::new(),
                    },
                    domains: vec![ObservedDomain {
                        id,
                        mode: effect.mode(),
                        path: effect.path().to_path_buf(),
                        kind: DomainKind::Directory,
                        file: None,
                    }],
                })
            },
        );
        assert!(matches!(result, Err(CoordinationFailure::DeadlineExceeded)));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn acquire_uses_the_budget_remaining_after_initial_observation() {
        let clock = Arc::new(AtomicU64::new(0));
        let observer_clock = Arc::clone(&clock);
        let deadline = CoordinationDeadline::manual(Arc::clone(&clock), 3);
        let effect = DirectoryEffect::tree("/synthetic/no-native-open", CoordinationMode::Shared);
        let plan = CoordinationPlan::new_in_scope_with(
            vec![effect.clone()],
            ObservationScope::Production,
            deadline,
            move |effect, _, _| {
                observer_clock.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let id = PhysicalObjectId {
                    device: 10,
                    inode: 1,
                };
                Ok(ObservedEffect {
                    binding: EffectBinding {
                        requested: effect.path().to_path_buf(),
                        existing_lexical: effect.path().to_path_buf(),
                        resolved: effect.path().to_path_buf(),
                        resolved_id: id,
                        missing_suffix: PathBuf::new(),
                    },
                    domains: vec![ObservedDomain {
                        id,
                        mode: effect.mode(),
                        path: effect.path().to_path_buf(),
                        kind: DomainKind::Directory,
                        file: None,
                    }],
                })
            },
        )
        .expect("initial observation fits budget");
        clock.store(3, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(
            plan.acquire(),
            Err(CoordinationFailure::DeadlineExceeded)
        ));
    }

    #[test]
    fn same_process_instances_allow_readers_and_exclude_writers() {
        let _serial = serial();
        let root = fixture();
        let tree = root.path().join("tree");
        fs::create_dir(&tree).expect("tree");
        let first = tree_plan_with_timeout(
            &root,
            &tree,
            CoordinationMode::Shared,
            Duration::from_millis(100),
        )
        .acquire()
        .expect("first reader");
        let second = tree_plan_with_timeout(
            &root,
            &tree,
            CoordinationMode::Shared,
            Duration::from_millis(100),
        )
        .acquire()
        .expect("second reader");
        assert!(matches!(
            tree_plan_with_timeout(
                &root,
                &tree,
                CoordinationMode::Exclusive,
                Duration::from_millis(30)
            )
            .acquire(),
            Err(CoordinationFailure::Busy)
        ));
        drop(first);
        drop(second);
    }

    #[test]
    fn hard_link_reader_uses_a_file_domain_across_disjoint_fixture_roots() {
        let _serial = serial();
        let fixture = fixture();
        let left = fixture.path().join("left");
        let right = fixture.path().join("right");
        fs::create_dir(&left).expect("left");
        fs::create_dir(&right).expect("right");
        let source = left.join("SKILL.md");
        let alias = right.join("SKILL.md");
        fs::write(&source, "bound bytes").expect("source");
        fs::hard_link(&source, &alias).expect("hard link");
        let other = right.join("other.md");
        fs::write(&other, "other root").expect("other file");
        let scope = SkillReadScope::bind(&[left.clone(), right.clone()]).expect("scope");
        let before = fixture_snapshot(fixture.path());

        let directory_guard = CoordinationPlan::new_fixture(
            vec![DirectoryEffect::tree(&left, CoordinationMode::Shared)],
            fixture.path(),
            Some(Duration::from_millis(100)),
        )
        .expect("left plan")
        .acquire()
        .expect("left directory guard");
        let right_plan = CoordinationPlan::new_fixture(
            vec![DirectoryEffect::tree(&right, CoordinationMode::Exclusive)],
            &right,
            Some(Duration::from_millis(100)),
        )
        .expect("right plan");
        assert!(directory_guard
            .observation
            .domains
            .iter()
            .all(|left_domain| {
                right_plan
                    .observation
                    .domains
                    .iter()
                    .all(|right_domain| left_domain.id != right_domain.id)
            }));
        let reader = directory_guard
            .continue_with_files(
                &scope,
                &[source.clone(), other.clone()],
                CoordinationMode::Shared,
            )
            .expect("complete reader");
        assert_eq!(
            reader.read(&scope, &source, 64).expect("read"),
            b"bound bytes"
        );
        assert_eq!(
            reader.read(&scope, &other, 64).expect("other read"),
            b"other root"
        );
        let right_id = metadata_id(&fs::metadata(&right).expect("right metadata"));
        assert!(reader
            .guard
            .observation
            .domains
            .iter()
            .any(|domain| domain.id == right_id && domain.kind == DomainKind::Directory));
        HeldChild::expect_raw_busy(&alias, CoordinationMode::Exclusive);
        drop(reader);
        let mut writer = HeldChild::raw(&alias, CoordinationMode::Exclusive);
        writer.finish(false);
        assert_eq!(before, fixture_snapshot(fixture.path()));
    }

    #[test]
    fn final_write_lease_retains_domains_through_document_replacement() {
        let _serial = serial();
        let root = fixture();
        let source = root.path().join("SKILL.md");
        let alias = root.path().join("alias");
        fs::write(&source, b"original").unwrap();
        fs::hard_link(&source, &alias).unwrap();
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
        let target = crate::skill_document_target::SkillDocumentTarget::bind(root.path()).unwrap();
        let mut lease = tree_plan(&root, root.path(), CoordinationMode::Exclusive)
            .acquire()
            .unwrap()
            .finalize_write(&scope, std::slice::from_ref(&source))
            .unwrap();
        assert_eq!(lease.read(&source, 64).unwrap(), b"original");
        HeldChild::expect_raw_busy(&alias, CoordinationMode::Shared);
        target
            .replace(&mut lease, b"original", b"replacement")
            .unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"replacement");
        assert_eq!(fs::read(&alias).unwrap(), b"original");
        HeldChild::expect_raw_busy(&alias, CoordinationMode::Shared);
        HeldChild::expect_raw_busy(root.path(), CoordinationMode::Shared);
        assert!(target
            .replace(&mut lease, b"replacement", b"again")
            .is_err());
        drop(lease);
        let mut child = HeldChild::raw(&alias, CoordinationMode::Shared);
        child.finish(false);
    }

    #[test]
    fn final_write_lease_tracks_two_replacements_and_detects_published_drift() {
        let _serial = serial();
        for drift in [false, true] {
            let root = fixture();
            let other = root.path().join("other");
            fs::create_dir(&other).unwrap();
            let first = root.path().join("SKILL.md");
            let second = other.join("SKILL.md");
            let old_alias = root.path().join("old-alias");
            fs::write(&first, b"first").unwrap();
            fs::write(&second, b"second").unwrap();
            fs::hard_link(&first, &old_alias).unwrap();
            let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
            let mut lease = tree_plan(&root, root.path(), CoordinationMode::Exclusive)
                .acquire()
                .unwrap()
                .finalize_write(&scope, &[first.clone(), second.clone()])
                .unwrap();
            let first_target =
                crate::skill_document_target::SkillDocumentTarget::bind(root.path()).unwrap();
            let second_target =
                crate::skill_document_target::SkillDocumentTarget::bind(&other).unwrap();
            first_target
                .replace(&mut lease, b"first", b"new-first")
                .unwrap();
            lease.revalidate().unwrap();
            assert_eq!(lease.read(&second, 64).unwrap(), b"second");
            if drift {
                fs::write(&first, b"external").unwrap();
            }
            let result = second_target.replace(&mut lease, b"second", b"new-second");
            assert_eq!(result.is_err(), drift);
            assert_eq!(
                fs::read(&second).unwrap(),
                if drift {
                    b"second".as_slice()
                } else {
                    b"new-second".as_slice()
                }
            );
            assert_eq!(lease.revalidate().is_err(), drift);
            HeldChild::expect_raw_busy(&old_alias, CoordinationMode::Shared);
            HeldChild::expect_raw_busy(root.path(), CoordinationMode::Shared);
        }
    }

    #[test]
    fn final_write_lease_stops_after_uncertain_replacement() {
        let _serial = serial();
        let root = fixture();
        let source = root.path().join("SKILL.md");
        fs::write(&source, b"original").unwrap();
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
        let mut lease = tree_plan(&root, root.path(), CoordinationMode::Exclusive)
            .acquire()
            .unwrap()
            .finalize_write(&scope, std::slice::from_ref(&source))
            .unwrap();
        assert!(lease
            .record_document(Err(
                crate::skill_document_write::DocumentWriteFailure::AfterReplace(
                    "injected uncertain outcome".into()
                )
            ))
            .is_err());
        assert!(lease.revalidate().is_err());
        let target = crate::skill_document_target::SkillDocumentTarget::bind(root.path()).unwrap();
        assert!(target.replace(&mut lease, b"original", b"bad").is_err());
        assert_eq!(fs::read(source).unwrap(), b"original");
        HeldChild::expect_raw_busy(root.path(), CoordinationMode::Shared);
    }

    #[test]
    fn final_write_lease_rejects_shared_plan_unplanned_target_and_cancellation() {
        let _serial = serial();
        let root = fixture();
        let source = root.path().join("SKILL.md");
        let other = root.path().join("other");
        fs::create_dir(&other).unwrap();
        fs::write(&source, b"original").unwrap();
        fs::write(other.join("SKILL.md"), b"other").unwrap();
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
        assert!(matches!(
            tree_plan(&root, root.path(), CoordinationMode::Shared)
                .acquire()
                .unwrap()
                .finalize_write(&scope, std::slice::from_ref(&source)),
            Err(CoordinationFailure::ExclusiveEffectsRequired)
        ));
        let cancellation = CancellationToken::default();
        let mut plan = tree_plan(&root, root.path(), CoordinationMode::Exclusive);
        plan.deadline.cancellation = Some(cancellation.clone());
        let mut lease = plan
            .acquire()
            .unwrap()
            .finalize_write(&scope, std::slice::from_ref(&source))
            .unwrap();
        let other_target = crate::skill_document_target::SkillDocumentTarget::bind(&other).unwrap();
        assert!(other_target.replace(&mut lease, b"other", b"bad").is_err());
        cancellation.cancel();
        let target = crate::skill_document_target::SkillDocumentTarget::bind(root.path()).unwrap();
        assert!(target.replace(&mut lease, b"original", b"bad").is_err());
        assert_eq!(fs::read(source).unwrap(), b"original");
        assert_eq!(fs::read(other.join("SKILL.md")).unwrap(), b"other");
    }

    #[test]
    fn extending_exclusive_file_plan_keeps_both_file_domains_exclusive() {
        let _serial = serial();
        let root = fixture();
        let source = root.path().join("source");
        let alias = root.path().join("alias");
        let additional = root.path().join("additional");
        let additional_alias = root.path().join("additional-alias");
        fs::write(&source, "original").unwrap();
        fs::hard_link(&source, &alias).unwrap();
        fs::write(&additional, "additional").unwrap();
        fs::hard_link(&additional, &additional_alias).unwrap();
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
        let before = fixture_snapshot(root.path());
        let guard = tree_plan(&root, root.path(), CoordinationMode::Exclusive)
            .acquire()
            .unwrap()
            .continue_with_files(
                &scope,
                std::slice::from_ref(&source),
                CoordinationMode::Exclusive,
            )
            .unwrap();
        HeldChild::expect_raw_busy(&alias, CoordinationMode::Shared);
        let extended = guard.extend_with_files(&scope, &[additional]).unwrap();
        HeldChild::expect_raw_busy(&alias, CoordinationMode::Shared);
        HeldChild::expect_raw_busy(&additional_alias, CoordinationMode::Shared);
        let finalized = extended.finalize(&scope).unwrap();
        HeldChild::expect_raw_busy(&alias, CoordinationMode::Shared);
        HeldChild::expect_raw_busy(&additional_alias, CoordinationMode::Shared);
        assert_eq!(finalized.read(&source, 64).unwrap(), b"original");
        assert!(finalized.read(&alias, 64).is_err());
        finalized.revalidate().unwrap();
        drop(finalized);
        let mut first = HeldChild::raw(&alias, CoordinationMode::Exclusive);
        first.finish(false);
        let mut second = HeldChild::raw(&additional_alias, CoordinationMode::Exclusive);
        second.finish(false);
        assert_eq!(before, fixture_snapshot(root.path()));
    }

    #[test]
    fn finalizing_a_changed_read_plan_refuses_and_releases_its_lease() {
        let _serial = serial();
        let root = fixture();
        let source = root.path().join("source");
        fs::write(&source, "before").unwrap();
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
        let guard = tree_plan(&root, root.path(), CoordinationMode::Shared)
            .acquire()
            .unwrap()
            .continue_with_files(
                &scope,
                std::slice::from_ref(&source),
                CoordinationMode::Shared,
            )
            .unwrap();
        fs::write(&source, "external edit with different size").unwrap();
        assert!(matches!(
            guard.finalize(&scope),
            Err(CoordinationFailure::Changed)
        ));
        let mut writer = HeldChild::raw(root.path(), CoordinationMode::Exclusive);
        writer.finish(false);
        assert_eq!(
            fs::read(&source).unwrap(),
            b"external edit with different size"
        );
    }

    #[test]
    fn file_domains_merge_by_identity_with_the_strongest_mode() {
        let root = fixture();
        let source = root.path().join("source");
        let alias = root.path().join("alias");
        fs::write(&source, "content").expect("source");
        fs::hard_link(&source, &alias).expect("alias");
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).expect("scope");
        let directory = tree_plan(&root, root.path(), CoordinationMode::Shared).observation;
        let shared = merge_file_observation(
            directory,
            vec![scope.observe_regular(&source).expect("source observation")],
            CoordinationMode::Shared,
        )
        .expect("shared merge");
        let merged = merge_file_observation(
            shared,
            vec![scope.observe_regular(&alias).expect("alias observation")],
            CoordinationMode::Exclusive,
        )
        .expect("exclusive merge");
        let file_domains: Vec<_> = merged
            .domains
            .iter()
            .filter(|domain| matches!(domain.kind, DomainKind::File { .. }))
            .collect();
        assert_eq!(file_domains.len(), 1);
        assert_eq!(file_domains[0].mode, CoordinationMode::Exclusive);
    }

    #[test]
    fn reversed_multi_file_requests_acquire_the_same_sorted_domains() {
        let _serial = serial();
        let root = fixture();
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::write(&first, "first").expect("first");
        fs::write(&second, "second").expect("second");
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).expect("scope");
        let acquire = |paths: Vec<PathBuf>| {
            tree_plan(&root, root.path(), CoordinationMode::Exclusive)
                .acquire()
                .expect("directory guard")
                .continue_with_files(&scope, &paths, CoordinationMode::Exclusive)
                .expect("file guard")
        };
        let reverse = acquire(vec![second.clone(), first.clone()]);
        let reverse_ids: Vec<_> = reverse
            .guard
            .observation
            .domains
            .iter()
            .filter(|domain| matches!(domain.kind, DomainKind::File { .. }))
            .map(|domain| domain.id)
            .collect();
        assert_eq!(reverse_ids.len(), 2);
        assert!(reverse_ids.windows(2).all(|pair| pair[0] < pair[1]));
        drop(reverse);

        let forward = acquire(vec![first, second]);
        let forward_ids: Vec<_> = forward
            .guard
            .observation
            .domains
            .iter()
            .filter(|domain| matches!(domain.kind, DomainKind::File { .. }))
            .map(|domain| domain.id)
            .collect();
        assert_eq!(reverse_ids, forward_ids);
    }

    #[test]
    fn guarded_prefix_and_stream_are_bounded_and_reject_unplanned_files() {
        let _serial = serial();
        let root = fixture();
        let source = root.path().join("source");
        let other = root.path().join("other");
        let bytes = vec![42_u8; 160_000];
        fs::write(&source, &bytes).unwrap();
        fs::write(&other, "unplanned").unwrap();
        fs::hard_link(&source, root.path().join("source-alias")).unwrap();
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
        let guard = tree_plan(&root, root.path(), CoordinationMode::Shared)
            .acquire()
            .unwrap()
            .continue_with_files(
                &scope,
                std::slice::from_ref(&source),
                CoordinationMode::Shared,
            )
            .unwrap();
        let prefix = guard
            .read_prefix(&scope, &source, 17, &mut || Ok(()))
            .unwrap();
        assert_eq!(prefix.bytes, bytes[..17]);
        assert!(prefix.truncated);
        let mut chunks = Vec::new();
        let count = guard
            .fold(&scope, &source, 100_000, &mut || Ok(()), &mut |chunk| {
                assert!(chunk.len() <= 64 * 1024);
                chunks.extend_from_slice(chunk);
            })
            .unwrap();
        assert_eq!(count, 100_000);
        assert_eq!(chunks, bytes[..100_000]);
        assert!(matches!(
            guard.read_prefix(&scope, &other, 8, &mut || Ok(())),
            Err(ScopedContentFoldError::Read(_))
        ));
        assert!(matches!(
            guard.fold(&scope, &other, 8, &mut || Ok(()), &mut |_| panic!(
                "unplanned bytes"
            )),
            Err(ScopedContentFoldError::Read(_))
        ));
        HeldChild::expect_raw_busy(&source, CoordinationMode::Exclusive);
        guard.revalidate(&scope).unwrap();
    }

    #[test]
    fn guarded_stream_rejects_replaced_identity_before_delivering_bytes() {
        let _serial = serial();
        let root = fixture();
        let source = root.path().join("source");
        fs::write(&source, "original").unwrap();
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
        let guard = tree_plan(&root, root.path(), CoordinationMode::Shared)
            .acquire()
            .unwrap()
            .continue_with_files(
                &scope,
                std::slice::from_ref(&source),
                CoordinationMode::Shared,
            )
            .unwrap();
        fs::rename(&source, root.path().join("old")).unwrap();
        fs::write(&source, "replacement").unwrap();
        assert!(matches!(
            guard.fold(&scope, &source, 32, &mut || Ok(()), &mut |_| panic!(
                "replacement bytes"
            )),
            Err(ScopedContentFoldError::Changed)
        ));
        assert!(matches!(
            guard.read_prefix(&scope, &source, 32, &mut || Ok(())),
            Err(ScopedContentFoldError::Changed)
        ));
    }

    #[test]
    fn operation_token_stops_guarded_streams_and_revalidation() {
        let _serial = serial();
        for hard_linked in [false, true] {
            let root = fixture();
            let source = root.path().join("source");
            fs::write(&source, vec![7_u8; 160_000]).unwrap();
            if hard_linked {
                fs::hard_link(&source, root.path().join("alias")).unwrap();
            }
            let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
            let token = CancellationToken::default();
            let mut plan = tree_plan(&root, root.path(), CoordinationMode::Shared);
            plan.deadline.cancellation = Some(token.clone());
            let guard = plan
                .acquire()
                .unwrap()
                .continue_with_files(
                    &scope,
                    std::slice::from_ref(&source),
                    CoordinationMode::Shared,
                )
                .unwrap();
            let mut delivered = 0;
            let result = guard.fold(&scope, &source, 200_000, &mut || Ok(()), &mut |chunk| {
                delivered += chunk.len();
                token.cancel();
            });
            assert!(matches!(result, Err(ScopedContentFoldError::Cancelled(_))));
            assert_eq!(delivered, 64 * 1024);
            assert!(matches!(
                guard.read_prefix(&scope, &source, 8, &mut || Ok(())),
                Err(ScopedContentFoldError::Cancelled(_))
            ));
            assert!(guard.read(&scope, &source, 200_000).is_err());
            assert!(matches!(
                guard.revalidate(&scope),
                Err(CoordinationFailure::Cancelled)
            ));
            drop(guard);
            let mut writer =
                HeldChild::start(root.path(), root.path(), CoordinationMode::Exclusive);
            writer.finish(false);
        }
    }

    #[test]
    fn guarded_stream_cancels_between_chunks_and_detects_content_drift() {
        let _serial = serial();
        for cancel in [true, false] {
            let root = fixture();
            let source = root.path().join("source");
            fs::write(&source, vec![7_u8; 160_000]).unwrap();
            let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
            let guard = tree_plan(&root, root.path(), CoordinationMode::Shared)
                .acquire()
                .unwrap()
                .continue_with_files(
                    &scope,
                    std::slice::from_ref(&source),
                    CoordinationMode::Shared,
                )
                .unwrap();
            let delivered = std::cell::Cell::new(0);
            let result = guard.fold(
                &scope,
                &source,
                200_000,
                &mut || {
                    if cancel && delivered.get() > 0 {
                        Err("cancelled".into())
                    } else {
                        Ok(())
                    }
                },
                &mut |chunk| {
                    delivered.set(delivered.get() + chunk.len());
                    if !cancel {
                        fs::write(&source, "changed").unwrap();
                    }
                },
            );
            if cancel {
                assert!(matches!(result, Err(ScopedContentFoldError::Cancelled(_))));
                assert_eq!(delivered.get(), 64 * 1024);
            } else {
                assert!(matches!(result, Err(ScopedContentFoldError::Changed)));
            }
        }
    }

    #[test]
    fn independent_file_readers_keep_independent_native_leases() {
        let _serial = serial();
        let root = fixture();
        let source = root.path().join("source");
        let alias = root.path().join("alias");
        fs::write(&source, "content").expect("source");
        fs::hard_link(&source, &alias).expect("alias");
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).expect("scope");
        let acquire = || {
            tree_plan(&root, root.path(), CoordinationMode::Shared)
                .acquire()
                .expect("directory guard")
                .continue_with_files(
                    &scope,
                    std::slice::from_ref(&source),
                    CoordinationMode::Shared,
                )
                .expect("file reader")
        };
        let first = acquire();
        let second = acquire();
        drop(first);
        HeldChild::expect_raw_busy(&alias, CoordinationMode::Exclusive);
        drop(second);
        let mut writer = HeldChild::raw(&alias, CoordinationMode::Exclusive);
        writer.finish(false);
    }

    #[test]
    fn coordinated_read_revalidation_detects_in_place_content_changes() {
        let _serial = serial();
        for replacement in ["after!", "longer replacement"] {
            let root = fixture();
            let source = root.path().join("source");
            fs::write(&source, "before").expect("source");
            File::open(&source)
                .expect("source handle")
                .set_modified(std::time::UNIX_EPOCH)
                .expect("distinct initial modification time");
            let original = metadata_id(&fs::metadata(&source).expect("original metadata"));
            let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).expect("scope");
            let guard = tree_plan(&root, root.path(), CoordinationMode::Shared)
                .acquire()
                .expect("directory guard")
                .continue_with_files(
                    &scope,
                    std::slice::from_ref(&source),
                    CoordinationMode::Shared,
                )
                .expect("file guard");
            assert_eq!(
                guard.read(&scope, &source, 64).expect("initial read"),
                b"before"
            );
            guard.revalidate(&scope).expect("unchanged read");

            fs::write(&source, replacement).expect("in-place external edit");
            assert_eq!(
                metadata_id(&fs::metadata(&source).expect("edited metadata")),
                original
            );
            assert!(matches!(
                guard.revalidate(&scope),
                Err(CoordinationFailure::Changed)
            ));
            assert!(guard.read(&scope, &source, 64).is_err());
        }
    }

    #[test]
    fn link_count_transition_replans_the_complete_set() {
        let _serial = serial();
        let root = fixture();
        let source = root.path().join("source");
        let alias = root.path().join("alias");
        fs::write(&source, "content").expect("source");
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).expect("scope");
        let coordination_scope = ObservationScope::Fixture {
            lexical_root: root.path().to_path_buf(),
            root: fs::canonicalize(root.path()).expect("canonical root"),
        };
        let deadline =
            CoordinationDeadline::new(Some(Duration::from_secs(1))).expect("coordination deadline");
        let effects = vec![
            DirectoryEffect::tree(root.path(), CoordinationMode::Shared),
            DirectoryEffect::entry(&source, CoordinationMode::Shared),
        ];
        let (effects, expected) = plan_covered_files(
            effects,
            &coordination_scope,
            &deadline,
            &scope,
            std::slice::from_ref(&source),
            CoordinationMode::Shared,
        )
        .expect("single-link observation");
        assert!(expected
            .domains
            .iter()
            .all(|domain| !matches!(domain.kind, DomainKind::File { .. })));

        fs::hard_link(&source, &alias).expect("new hard link");
        let guard = acquire_complete_read(
            effects,
            coordination_scope,
            deadline,
            &scope,
            std::slice::from_ref(&source),
            CoordinationMode::Shared,
            expected,
        )
        .expect("replanned file guard");
        assert!(guard
            .guard
            .observation
            .domains
            .iter()
            .any(|domain| { matches!(domain.kind, DomainKind::File { link_count: 2 }) }));
        HeldChild::expect_raw_busy(&alias, CoordinationMode::Exclusive);
    }

    #[test]
    fn file_replacement_discards_the_stale_observation() {
        let _serial = serial();
        let root = fixture();
        let source = root.path().join("source");
        fs::write(&source, "old").expect("source");
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).expect("scope");
        let coordination_scope = ObservationScope::Fixture {
            lexical_root: root.path().to_path_buf(),
            root: fs::canonicalize(root.path()).expect("canonical root"),
        };
        let deadline =
            CoordinationDeadline::new(Some(Duration::from_secs(1))).expect("coordination deadline");
        let effects = vec![
            DirectoryEffect::tree(root.path(), CoordinationMode::Exclusive),
            DirectoryEffect::entry(&source, CoordinationMode::Exclusive),
        ];
        let (effects, expected) = plan_covered_files(
            effects,
            &coordination_scope,
            &deadline,
            &scope,
            std::slice::from_ref(&source),
            CoordinationMode::Exclusive,
        )
        .expect("old observation");
        let old_id = SkillReadScope::observed_identity(&expected.files[0]);
        fs::rename(&source, root.path().join("old-source")).expect("retain old file");
        fs::write(&source, "new").expect("replacement");

        let guard = acquire_complete_read(
            effects,
            coordination_scope,
            deadline,
            &scope,
            std::slice::from_ref(&source),
            CoordinationMode::Exclusive,
            expected,
        )
        .expect("replacement guard");
        assert_ne!(old_id, SkillReadScope::observed_identity(&guard.files[0]));
        assert_eq!(guard.read(&scope, &source, 16).expect("new bytes"), b"new");
    }

    #[test]
    fn cross_root_symlink_retarget_between_planner_observations_expands_coverage() {
        let _serial = serial();
        let root = fixture();
        let left = root.path().join("left");
        let right = root.path().join("right");
        let left_backing = left.join("backing");
        let right_backing = right.join("backing");
        fs::create_dir_all(&left_backing).expect("left backing");
        fs::create_dir_all(&right_backing).expect("right backing");
        let left_file = left_backing.join("SKILL.md");
        let right_file = right_backing.join("SKILL.md");
        let alias = left.join("selected");
        fs::write(&left_file, "left").expect("left file");
        fs::write(&right_file, "right").expect("right file");
        symlink(&left_file, &alias).expect("initial alias");
        let scope = SkillReadScope::bind(&[left.clone(), right.clone()]).expect("scope");
        let coordination_scope = ObservationScope::Fixture {
            lexical_root: root.path().to_path_buf(),
            root: fs::canonicalize(root.path()).expect("canonical root"),
        };
        let deadline =
            CoordinationDeadline::new(Some(Duration::from_secs(1))).expect("coordination deadline");
        let effects = vec![DirectoryEffect::entry(&alias, CoordinationMode::Shared)];
        let replans = Arc::new(AtomicU64::new(0));
        let replan_count = Arc::clone(&replans);
        let retarget_alias = alias.clone();
        let retarget_file = right_file.clone();
        let (effects, expected) = plan_covered_files_with(
            effects,
            &coordination_scope,
            &deadline,
            &scope,
            std::slice::from_ref(&alias),
            CoordinationMode::Shared,
            move || {
                if replan_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    fs::remove_file(&retarget_alias).expect("remove initial alias");
                    symlink(&retarget_file, &retarget_alias).expect("retarget alias");
                }
            },
        )
        .expect("retargeted observation");
        let right_resolved = fs::canonicalize(&right_backing).expect("resolved right backing");
        let right_id = metadata_id(&fs::metadata(&right_backing).expect("right metadata"));
        assert_eq!(replans.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(effects.contains(&DirectoryEffect::tree(
            &right_resolved,
            CoordinationMode::Shared,
        )));
        assert!(expected
            .domains
            .iter()
            .any(|domain| { domain.id == right_id && domain.kind == DomainKind::Directory }));

        let guard = acquire_complete_read(
            effects,
            coordination_scope,
            deadline,
            &scope,
            std::slice::from_ref(&alias),
            CoordinationMode::Shared,
            expected,
        )
        .expect("retargeted guard");
        assert!(guard
            .guard
            .observation
            .domains
            .iter()
            .any(|domain| { domain.id == right_id && domain.kind == DomainKind::Directory }));
        assert_eq!(
            guard.read(&scope, &alias, 16).expect("right bytes"),
            b"right"
        );
    }

    #[test]
    fn file_replanning_uses_the_original_deadline() {
        let _serial = serial();
        let root = fixture();
        let left = root.path().join("left");
        let right = root.path().join("right");
        fs::create_dir(&left).expect("left");
        fs::create_dir(&right).expect("right");
        let source = right.join("source");
        let alias = left.join("alias");
        fs::write(&source, "content").expect("source");
        symlink(&source, &alias).expect("alias");
        let scope = SkillReadScope::bind(&[left.clone(), right]).expect("scope");
        let coordination_scope = ObservationScope::Fixture {
            lexical_root: root.path().to_path_buf(),
            root: fs::canonicalize(root.path()).expect("canonical root"),
        };
        let clock = Arc::new(AtomicU64::new(0));
        let deadline = CoordinationDeadline::manual(Arc::clone(&clock), 1);
        let replans = Arc::new(AtomicU64::new(0));
        let replan_count = Arc::clone(&replans);
        let replan_clock = Arc::clone(&clock);
        let result = plan_covered_files_with(
            vec![
                DirectoryEffect::tree(&left, CoordinationMode::Shared),
                DirectoryEffect::entry(&alias, CoordinationMode::Shared),
            ],
            &coordination_scope,
            &deadline,
            &scope,
            std::slice::from_ref(&alias),
            CoordinationMode::Shared,
            move || {
                replan_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                replan_clock.store(1, std::sync::atomic::Ordering::SeqCst);
            },
        );
        assert!(matches!(result, Err(CoordinationFailure::DeadlineExceeded)));
        assert_eq!(replans.load(std::sync::atomic::Ordering::SeqCst), 1);
        let mut released = HeldChild::start(root.path(), &left, CoordinationMode::Exclusive);
        released.finish(false);
    }

    #[test]
    fn staged_read_guard_preserves_old_files_and_deadline() {
        let _serial = serial();
        for hard_linked in [false, true] {
            for change in ["stable", "changed", "expired"] {
                let root = fixture();
                let manifest = root.path().join("plugin.json");
                let document = root.path().join("SKILL.md");
                fs::write(&manifest, "manifest").unwrap();
                fs::write(&document, "document").unwrap();
                if hard_linked {
                    fs::hard_link(&document, root.path().join("document-alias")).unwrap();
                }
                let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
                let mut guard = tree_plan(&root, root.path(), CoordinationMode::Shared)
                    .acquire()
                    .unwrap()
                    .continue_with_files(
                        &scope,
                        std::slice::from_ref(&manifest),
                        CoordinationMode::Shared,
                    )
                    .unwrap();
                let old_deadline = guard.guard.deadline.at;
                if change == "changed" {
                    fs::write(&manifest, "different").unwrap();
                }
                if change == "expired" {
                    guard.guard.deadline.at = Instant::now();
                }
                let result = guard.extend_with_files(&scope, &[document.clone(), manifest.clone()]);
                match change {
                    "changed" => assert!(matches!(result, Err(CoordinationFailure::Changed))),
                    "expired" => {
                        assert!(matches!(result, Err(CoordinationFailure::DeadlineExceeded)))
                    }
                    _ => {
                        let guard = result.unwrap();
                        assert_eq!(guard.guard.deadline.at, old_deadline);
                        assert_eq!(guard.read(&scope, &manifest, 64).unwrap(), b"manifest");
                        assert_eq!(guard.read(&scope, &document, 64).unwrap(), b"document");
                        assert_eq!(guard.files.len() + guard.directory_files.len(), 2);
                        assert_eq!(guard.files.is_empty(), !hard_linked);
                        guard.revalidate(&scope).unwrap();
                        let guard = guard.extend_with_files(&scope, &[]).unwrap();
                        assert_eq!(guard.guard.deadline.at, old_deadline);
                        assert_eq!(guard.read(&scope, &manifest, 64).unwrap(), b"manifest");
                        assert_eq!(guard.read(&scope, &document, 64).unwrap(), b"document");
                    }
                }
            }
        }
    }

    #[test]
    fn shared_tree_covers_many_skill_directories_and_excludes_subtree_writers() {
        let _serial = serial();
        let root = fixture();
        let mut paths = Vec::new();
        for i in 0..300 {
            let directory = root.path().join(format!("skill-{i}"));
            fs::create_dir(&directory).unwrap();
            let path = directory.join("SKILL.md");
            fs::write(&path, "skill").unwrap();
            paths.push(path);
        }
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
        let guard = tree_plan(&root, root.path(), CoordinationMode::Shared)
            .acquire()
            .unwrap()
            .continue_with_files(&scope, &paths, CoordinationMode::Shared)
            .unwrap();
        assert_eq!(guard.guard.observation.domains.len(), 1);
        assert!(guard.files.is_empty());
        assert_eq!(guard.directory_files.len(), 300);
        let prefix = guard
            .read_prefix(&scope, &paths[299], 2, &mut || Ok(()))
            .unwrap();
        assert_eq!(prefix.bytes, b"sk");
        assert!(prefix.truncated);
        assert!(guard.read(&scope, &paths[299], 2).is_err());
        assert_eq!(guard.read(&scope, &paths[299], 16).unwrap(), b"skill");
        HeldChild::expect_busy(
            root.path(),
            paths[299].parent().unwrap(),
            CoordinationMode::Exclusive,
        );
        guard.revalidate(&scope).unwrap();
        drop(guard);
        HeldChild::start(
            root.path(),
            paths[299].parent().unwrap(),
            CoordinationMode::Exclusive,
        )
        .finish(false);
    }

    #[test]
    fn sibling_files_keep_individual_target_coverage() {
        let _serial = serial();
        for declared_target in [false, true] {
            let root = fixture();
            let tree = root.path().join("tree");
            let external = root.path().join("external");
            fs::create_dir(&tree).unwrap();
            fs::create_dir(&external).unwrap();
            let document = tree.join("SKILL.md");
            let resource = tree.join("resource.txt");
            let target = external.join("resource.txt");
            fs::write(&document, "document").unwrap();
            fs::write(&target, "resource").unwrap();
            std::os::unix::fs::symlink(&target, &resource).unwrap();
            let mut roots = vec![tree.clone()];
            if declared_target {
                roots.push(external.clone());
            }
            let scope = SkillReadScope::bind(&roots).unwrap();
            let result = tree_plan(&root, &tree, CoordinationMode::Shared)
                .acquire()
                .unwrap()
                .continue_with_files(
                    &scope,
                    &[document.clone(), resource.clone()],
                    CoordinationMode::Shared,
                );
            if declared_target {
                let guard = result.unwrap();
                assert_eq!(guard.files.len(), 2);
                assert!(guard.directory_files.is_empty());
                assert!(guard.guard.effects.contains(&DirectoryEffect::tree(
                    fs::canonicalize(&external).unwrap(),
                    CoordinationMode::Shared,
                )));
                assert_eq!(guard.read(&scope, &document, 64).unwrap(), b"document");
                assert_eq!(guard.read(&scope, &resource, 64).unwrap(), b"resource");
                fs::write(&target, "changed").unwrap();
                assert!(guard.read(&scope, &resource, 64).is_err());
                assert!(guard.revalidate(&scope).is_err());
            } else {
                assert!(matches!(
                    result,
                    Err(CoordinationFailure::Unavailable { .. })
                ));
            }
        }
    }

    #[test]
    fn shared_read_keeps_coverage_for_an_external_parent_pointing_back_inside() {
        let _serial = serial();
        let root = fixture();
        let tree = root.path().join("tree");
        let outside = root.path().join("outside");
        fs::create_dir(&tree).unwrap();
        fs::create_dir(&outside).unwrap();
        let target = tree.join("target.md");
        fs::write(&target, "document").unwrap();
        std::os::unix::fs::symlink(&outside, tree.join("alias")).unwrap();
        std::os::unix::fs::symlink(&target, outside.join("SKILL.md")).unwrap();
        let document = tree.join("alias/SKILL.md");
        let scope = SkillReadScope::bind(&[tree.clone(), outside.clone()]).unwrap();
        let guard = tree_plan(&root, &tree, CoordinationMode::Shared)
            .acquire()
            .unwrap()
            .continue_with_files(
                &scope,
                std::slice::from_ref(&document),
                CoordinationMode::Shared,
            )
            .unwrap();
        assert!(guard.directory_files.is_empty());
        assert_eq!(guard.files.len(), 1);
        HeldChild::expect_busy(root.path(), &outside, CoordinationMode::Exclusive);
        assert_eq!(guard.read(&scope, &document, 64).unwrap(), b"document");
    }

    #[test]
    fn shared_read_parent_alias_requires_matching_tree_coverage() {
        let _serial = serial();
        for external in [false, true] {
            let root = fixture();
            let tree = root.path().join("tree");
            let outside = root.path().join("outside");
            let local = tree.join("local");
            fs::create_dir_all(&local).unwrap();
            fs::create_dir(&outside).unwrap();
            fs::write(local.join("SKILL.md"), "local").unwrap();
            fs::write(outside.join("SKILL.md"), "outside").unwrap();
            let alias = tree.join("alias");
            let target = if external { &outside } else { &local };
            std::os::unix::fs::symlink(target, &alias).unwrap();
            let document = alias.join("SKILL.md");
            let scope = SkillReadScope::bind(&[tree.clone(), outside.clone()]).unwrap();
            let guard = tree_plan(&root, &tree, CoordinationMode::Shared)
                .acquire()
                .unwrap()
                .continue_with_files(
                    &scope,
                    std::slice::from_ref(&document),
                    CoordinationMode::Shared,
                )
                .unwrap();
            assert_eq!(guard.directory_files.len(), usize::from(!external));
            assert_eq!(guard.files.len(), usize::from(external));
            assert_eq!(
                guard.read(&scope, &document, 64).unwrap(),
                if external {
                    b"outside".as_slice()
                } else {
                    b"local".as_slice()
                }
            );
            fs::remove_file(&alias).unwrap();
            std::os::unix::fs::symlink(if external { &local } else { &outside }, &alias).unwrap();
            assert!(guard.read(&scope, &document, 64).is_err());
            assert!(guard.revalidate(&scope).is_err());
        }
    }

    #[test]
    fn directory_protected_read_rejects_a_new_hard_link() {
        let _serial = serial();
        let root = fixture();
        let source = root.path().join("source");
        fs::write(&source, "original").unwrap();
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
        let guard = tree_plan(&root, root.path(), CoordinationMode::Shared)
            .acquire()
            .unwrap()
            .continue_with_files(
                &scope,
                std::slice::from_ref(&source),
                CoordinationMode::Shared,
            )
            .unwrap();
        assert!(guard.files.is_empty());
        assert_eq!(guard.directory_files.len(), 1);
        fs::hard_link(&source, root.path().join("alias")).unwrap();
        assert!(guard.read(&scope, &source, 16).is_err());
        assert!(matches!(
            guard.revalidate(&scope),
            Err(CoordinationFailure::Changed)
        ));
    }

    #[test]
    fn shared_tree_does_not_drop_an_external_lexical_alias_parent() {
        let _serial = serial();
        let root = fixture();
        let left = root.path().join("left");
        let right = root.path().join("right");
        fs::create_dir(&left).unwrap();
        fs::create_dir(&right).unwrap();
        fs::write(left.join("SKILL.md"), "skill").unwrap();
        let alias = right.join("linked");
        std::os::unix::fs::symlink(left.join("SKILL.md"), &alias).unwrap();
        let scope = SkillReadScope::bind(&[left.clone(), right.clone()]).unwrap();
        let guard = tree_plan(&root, &left, CoordinationMode::Shared)
            .acquire()
            .unwrap()
            .continue_with_files(
                &scope,
                std::slice::from_ref(&alias),
                CoordinationMode::Shared,
            )
            .unwrap();
        assert!(guard
            .guard
            .effects
            .contains(&DirectoryEffect::entry(&alias, CoordinationMode::Shared)));
        assert!(guard
            .guard
            .observation
            .domains
            .iter()
            .any(|domain| domain.path == fs::canonicalize(&right).unwrap()));
        assert_eq!(guard.read(&scope, &alias, 16).unwrap(), b"skill");
    }

    #[test]
    #[ignore = "release measurement; run alone with --nocapture --test-threads=1"]
    fn measure_inventory_coordination_scale() {
        let _serial = serial();
        let open_files = || fs::read_dir("/dev/fd").unwrap().count();
        for layout in ["one-directory", "skill-directories", "hard-linked-files"] {
            for count in [10, 100, 1_000] {
                let root = fixture();
                let mut paths = Vec::with_capacity(count);
                for i in 0..count {
                    let path = if layout == "skill-directories" {
                        let dir = root.path().join(format!("skill-{i}"));
                        fs::create_dir(&dir).unwrap();
                        dir.join("SKILL.md")
                    } else {
                        root.path().join(format!("file-{i}"))
                    };
                    fs::write(&path, "fixture content").unwrap();
                    if layout == "hard-linked-files" {
                        fs::hard_link(&path, root.path().join(format!("alias-{i}"))).unwrap();
                    }
                    paths.push(path);
                }
                let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
                let before = open_files();
                let started = Instant::now();
                let result = tree_plan_with_timeout(
                    &root,
                    root.path(),
                    CoordinationMode::Shared,
                    Duration::from_secs(60),
                )
                .acquire()
                .and_then(|guard| {
                    guard.continue_with_files(&scope, &paths, CoordinationMode::Shared)
                });
                let acquire_us = started.elapsed().as_micros();
                let held = open_files();
                let (outcome, domains, revalidate_us) = match &result {
                    Ok(guard) => {
                        let started = Instant::now();
                        guard.revalidate(&scope).unwrap();
                        (
                            "ready".to_string(),
                            Some(guard.guard.observation.domains.len()),
                            Some(started.elapsed().as_micros()),
                        )
                    }
                    Err(error) => (error.to_string(), None, None),
                };
                drop(result);
                let after = open_files();
                assert_eq!(after, before, "coordination leaked descriptors");
                println!(
                    "{}",
                    serde_json::json!({
                        "measurement": "inventory-coordination-scale",
                        "layout": layout, "files": count, "acquire_us": acquire_us,
                        "revalidate_us": revalidate_us, "outcome": outcome,
                        "held_domains": domains, "fds_before": before,
                        "fds_held": held, "fds_after": after,
                        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
                        "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
                    })
                );
            }
        }
    }

    #[test]
    fn file_domains_count_toward_capacity() {
        let _serial = serial();
        let root = fixture();
        let mut paths = Vec::with_capacity(MAX_DOMAINS);
        for index in 0..MAX_DOMAINS {
            let path = root.path().join(format!("file-{index}"));
            fs::write(&path, index.to_string()).expect("file");
            paths.push(path);
        }
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).expect("scope");
        let guard = fixture_plan_with_timeout(
            &root,
            vec![DirectoryEffect::tree(
                root.path(),
                CoordinationMode::Exclusive,
            )],
            Duration::from_secs(10),
        )
        .expect("directory plan")
        .acquire()
        .expect("directory guard");
        assert!(matches!(
            guard.continue_with_files(&scope, &paths, CoordinationMode::Exclusive),
            Err(CoordinationFailure::CapacityExceeded {
                required,
                limit: MAX_DOMAINS
            }) if required == MAX_DOMAINS + 1
        ));
    }

    #[test]
    fn partial_file_acquisition_releases_prior_file_leases() {
        let _serial = serial();
        let root = fixture();
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::write(&first, "first").expect("first");
        fs::write(&second, "second").expect("second");
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).expect("scope");
        let mut domains: Vec<_> = [&first, &second]
            .into_iter()
            .map(|path| {
                let observation = scope.observe_regular(path).expect("file observation");
                let (device, inode, link_count) = SkillReadScope::observed_identity(&observation);
                ObservedDomain {
                    id: PhysicalObjectId { device, inode },
                    mode: CoordinationMode::Exclusive,
                    path: SkillReadScope::observed_resolved(&observation).to_path_buf(),
                    kind: DomainKind::File { link_count },
                    file: Some(observation),
                }
            })
            .collect();
        domains.sort_by_key(|domain| domain.id);
        let released_path = domains[0].path.clone();
        let blocked_path = domains[1].path.clone();
        let mut blocker = HeldChild::raw(&blocked_path, CoordinationMode::Exclusive);
        let deadline = CoordinationDeadline::new(Some(Duration::from_millis(100)))
            .expect("acquisition deadline");
        assert!(matches!(
            acquire_native(&domains, &deadline, Some(&scope)),
            Err(NativeAcquire::Busy)
        ));
        let mut proves_release = HeldChild::raw(&released_path, CoordinationMode::Exclusive);
        proves_release.finish(false);
        blocker.finish(false);
    }

    #[test]
    fn scoped_file_open_failure_is_unavailable_and_releases_leases_and_gate() {
        let _serial = serial();
        let root = fixture();
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::write(&first, "first").expect("first");
        fs::write(&second, "second").expect("second");
        let read_scope = SkillReadScope::bind(&[root.path().to_path_buf()]).expect("scope");
        let scope = ObservationScope::Fixture {
            lexical_root: root.path().to_path_buf(),
            root: fs::canonicalize(root.path()).expect("canonical root"),
        };
        let deadline =
            CoordinationDeadline::new(Some(Duration::from_millis(100))).expect("deadline");
        let (effects, expected) = plan_covered_files(
            vec![DirectoryEffect::tree(
                root.path(),
                CoordinationMode::Exclusive,
            )],
            &scope,
            &deadline,
            &read_scope,
            &[first, second],
            CoordinationMode::Exclusive,
        )
        .expect("file plan");
        let released_path = expected
            .domains
            .iter()
            .find(|domain| matches!(domain.kind, DomainKind::File { .. }))
            .expect("first file domain")
            .path
            .clone();
        let _failure = SkillReadScope::inject_observed_lock_open_failure(1, libc::EMFILE);
        let error = match acquire_complete_read(
            effects,
            scope,
            deadline,
            &read_scope,
            &[],
            CoordinationMode::Exclusive,
            expected,
        ) {
            Ok(_) => panic!("injected file open failure"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            CoordinationFailure::Unavailable { ref source, .. }
                if source.raw_os_error() == Some(libc::EMFILE)
        ));
        let mut proves_lease_release = HeldChild::raw(&released_path, CoordinationMode::Exclusive);
        proves_lease_release.finish(false);
        let gate_deadline =
            CoordinationDeadline::new(Some(Duration::from_millis(10))).expect("gate deadline");
        let gate = acquire_gate(CoordinationMode::Exclusive, &gate_deadline)
            .expect("gate result")
            .expect("released gate");
        drop(gate);
    }

    #[test]
    fn planned_file_alias_retargets_are_changed_during_native_acquisition() {
        let _serial = serial();
        for target in ["missing", "not-a-directory/child", "alias"] {
            let root = fixture();
            let source = root.path().join("source");
            let alias = root.path().join("alias");
            fs::write(&source, "content").expect("source");
            fs::write(root.path().join("not-a-directory"), "content").expect("not a directory");
            symlink(&source, &alias).expect("initial alias");
            let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).expect("scope");
            let observation = scope.observe_regular(&alias).expect("file observation");
            let (device, inode, link_count) = SkillReadScope::observed_identity(&observation);
            let domain = ObservedDomain {
                id: PhysicalObjectId { device, inode },
                mode: CoordinationMode::Exclusive,
                path: SkillReadScope::observed_resolved(&observation).to_path_buf(),
                kind: DomainKind::File { link_count },
                file: Some(observation),
            };
            fs::remove_file(&alias).expect("remove alias");
            symlink(target, &alias).expect("retarget alias");
            let deadline =
                CoordinationDeadline::new(Some(Duration::from_millis(100))).expect("deadline");
            assert!(matches!(
                acquire_native(&[domain], &deadline, Some(&scope)),
                Err(NativeAcquire::Changed)
            ));
        }
    }

    #[test]
    fn observed_link_replacement_during_readlink_is_changed_during_native_acquisition() {
        let _serial = serial();
        let root = fixture();
        let source = root.path().join("source");
        let alias = root.path().join("alias");
        fs::write(&source, "content").expect("source");
        symlink(&source, &alias).expect("alias");
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).expect("scope");
        let observation = scope.observe_regular(&alias).expect("file observation");
        let (device, inode, link_count) = SkillReadScope::observed_identity(&observation);
        let domain = ObservedDomain {
            id: PhysicalObjectId { device, inode },
            mode: CoordinationMode::Exclusive,
            path: SkillReadScope::observed_resolved(&observation).to_path_buf(),
            kind: DomainKind::File { link_count },
            file: Some(observation),
        };
        let _replacement = SkillReadScope::inject_observed_link_replacement();
        let deadline =
            CoordinationDeadline::new(Some(Duration::from_millis(100))).expect("deadline");
        assert!(matches!(
            acquire_native(&[domain], &deadline, Some(&scope)),
            Err(NativeAcquire::Changed)
        ));
    }

    #[test]
    fn file_lock_is_released_when_the_coordinator_process_dies() {
        let _serial = serial();
        let root = fixture();
        let source = root.path().join("source");
        let alias = root.path().join("alias");
        fs::write(&source, "content").expect("source");
        fs::hard_link(&source, &alias).expect("alias");
        let mut child = HeldChild::file(root.path(), &source, CoordinationMode::Shared);
        HeldChild::expect_raw_busy(&alias, CoordinationMode::Exclusive);
        child.finish(true);
        let mut writer = HeldChild::raw(&alias, CoordinationMode::Exclusive);
        writer.finish(false);
    }

    #[test]
    fn native_parent_child_conflicts_work_in_both_directions() {
        let _serial = serial();
        let root = fixture();
        let parent = root.path().join("parent");
        let child_path = parent.join("child");
        fs::create_dir_all(&child_path).expect("tree");

        let mut writer = HeldChild::start(root.path(), &child_path, CoordinationMode::Exclusive);
        assert!(matches!(
            tree_plan_with_timeout(
                &root,
                &parent,
                CoordinationMode::Shared,
                Duration::from_millis(40)
            )
            .acquire(),
            Err(CoordinationFailure::Busy)
        ));
        writer.finish(false);

        let mut reader = HeldChild::start(root.path(), &parent, CoordinationMode::Shared);
        assert!(matches!(
            tree_plan_with_timeout(
                &root,
                &child_path,
                CoordinationMode::Exclusive,
                Duration::from_millis(40)
            )
            .acquire(),
            Err(CoordinationFailure::Busy)
        ));
        reader.finish(false);
    }

    #[test]
    fn native_shared_aliases_and_independent_reader_handles_work() {
        let _serial = serial();
        let root = fixture();
        let tree = root.path().join("tree");
        let alias = root.path().join("alias");
        fs::create_dir(&tree).expect("tree");
        symlink(&tree, &alias).expect("alias");

        let mut child_reader = HeldChild::start(root.path(), &tree, CoordinationMode::Shared);
        let first = tree_plan_with_timeout(
            &root,
            &alias,
            CoordinationMode::Shared,
            Duration::from_millis(100),
        )
        .acquire()
        .expect("alias reader");
        child_reader.finish(false);

        let second = tree_plan_with_timeout(
            &root,
            &tree,
            CoordinationMode::Shared,
            Duration::from_millis(100),
        )
        .acquire()
        .expect("second reader");
        drop(first);
        HeldChild::expect_busy(root.path(), &alias, CoordinationMode::Exclusive);
        drop(second);
        let mut writer = HeldChild::start(root.path(), &alias, CoordinationMode::Exclusive);
        writer.finish(false);
    }

    #[test]
    fn native_partial_busy_and_error_release_prior_domains() {
        let _serial = serial();
        let root = fixture();
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::create_dir(&first).expect("first");
        fs::create_dir(&second).expect("second");
        let mut domains = vec![
            observe_domain(&first, CoordinationMode::Exclusive).expect("first domain"),
            observe_domain(&second, CoordinationMode::Exclusive).expect("second domain"),
        ];
        domains.sort_by_key(|domain| domain.id);
        let released_path = domains[0].path.clone();
        let blocked_path = domains[1].path.clone();
        let mut held_second = HeldChild::raw(&blocked_path, CoordinationMode::Exclusive);
        let busy_deadline =
            CoordinationDeadline::new(Some(Duration::from_millis(40))).expect("busy deadline");
        assert!(matches!(
            acquire_native(&domains, &busy_deadline, None),
            Err(NativeAcquire::Busy)
        ));
        let mut proves_busy_release = HeldChild::raw(&released_path, CoordinationMode::Exclusive);
        proves_busy_release.finish(false);
        held_second.finish(false);

        let bad_domain = ObservedDomain {
            id: PhysicalObjectId {
                device: u64::MAX,
                inode: u64::MAX,
            },
            mode: CoordinationMode::Exclusive,
            path: root.path().join("x".repeat(5_000)),
            kind: DomainKind::Directory,
            file: None,
        };
        let first_domain =
            observe_domain(&released_path, CoordinationMode::Exclusive).expect("released domain");
        assert!(first_domain.id < bad_domain.id);
        let error_deadline =
            CoordinationDeadline::new(Some(Duration::from_millis(100))).expect("error deadline");
        assert!(matches!(
            acquire_native(&[first_domain, bad_domain], &error_deadline, None),
            Err(NativeAcquire::Unavailable(_))
        ));
        let mut proves_error_release = HeldChild::raw(&released_path, CoordinationMode::Exclusive);
        proves_error_release.finish(false);
    }

    #[test]
    fn alias_and_missing_bindings_reobserve_and_reacquire() {
        let _serial = serial();
        let root = fixture();
        let first = root.path().join("first");
        let second = root.path().join("second");
        let alias = root.path().join("alias");
        fs::create_dir(&first).expect("first");
        fs::create_dir(&second).expect("second");
        symlink(&first, &alias).expect("alias");
        let alias_plan = fixture_plan(
            &root,
            vec![
                DirectoryEffect::tree(&first, CoordinationMode::Shared),
                DirectoryEffect::tree(&second, CoordinationMode::Shared),
                DirectoryEffect::tree(&alias, CoordinationMode::Shared),
            ],
        )
        .expect("alias plan with unchanged merged union");
        fs::remove_file(&alias).expect("remove alias");
        symlink(&second, &alias).expect("retarget alias");
        let alias_guard = alias_plan.acquire().expect("retargeted alias reacquired");
        alias_guard.revalidate().expect("alias revalidation");
        drop(alias_guard);

        let missing = root.path().join("present/new-parent/entry");
        fs::create_dir(root.path().join("present")).expect("present");
        let missing_plan = fixture_plan(
            &root,
            vec![DirectoryEffect::entry(
                &missing,
                CoordinationMode::Exclusive,
            )],
        )
        .expect("missing plan");
        fs::create_dir(root.path().join("present/new-parent")).expect("new parent");
        let missing_guard = missing_plan.acquire().expect("missing binding reacquired");
        missing_guard.revalidate().expect("missing revalidation");
    }

    #[test]
    fn changed_identity_reacquires_and_guard_detects_later_change() {
        let _serial = serial();
        let root = fixture();
        let tree = root.path().join("tree");
        fs::create_dir(&tree).expect("tree");
        let plan = tree_plan(&root, &tree, CoordinationMode::Shared);
        fs::rename(&tree, root.path().join("old")).expect("rename old identity");
        fs::create_dir(&tree).expect("replacement identity");
        let guard = plan.acquire().expect("replacement reacquired");
        fs::rename(&tree, root.path().join("new-old")).expect("rename acquired identity");
        fs::create_dir(&tree).expect("second replacement");
        assert!(matches!(
            guard.revalidate(),
            Err(CoordinationFailure::Changed)
        ));
    }

    #[test]
    fn directory_replaced_by_symlink_reobserves_and_reacquires() {
        let _serial = serial();
        let root = fixture();
        let tree = root.path().join("tree");
        let target = root.path().join("target");
        fs::create_dir(&tree).expect("tree");
        fs::create_dir(&target).expect("target");
        let plan = tree_plan(&root, &tree, CoordinationMode::Shared);
        fs::rename(&tree, root.path().join("old-tree")).expect("old tree");
        symlink(&target, &tree).expect("replacement symlink");
        let guard = plan.acquire().expect("symlink target reacquired");
        guard.revalidate().expect("symlink target revalidation");
    }

    #[test]
    fn native_normal_and_crash_exit_release_leases() {
        let _serial = serial();
        let root = fixture();
        let tree = root.path().join("tree");
        fs::create_dir(&tree).expect("tree");
        let mut normal = HeldChild::start(root.path(), &tree, CoordinationMode::Exclusive);
        normal.finish(false);
        let normal_guard = tree_plan_with_timeout(
            &root,
            &tree,
            CoordinationMode::Exclusive,
            Duration::from_millis(200),
        )
        .acquire()
        .expect("normal release");
        drop(normal_guard);

        let mut crash = HeldChild::start(root.path(), &tree, CoordinationMode::Exclusive);
        crash.finish(true);
        let crash_guard = tree_plan_with_timeout(
            &root,
            &tree,
            CoordinationMode::Exclusive,
            Duration::from_millis(200),
        )
        .acquire()
        .expect("crash release");
        drop(crash_guard);
    }

    #[test]
    fn read_only_coordination_creates_no_persistent_state() {
        let _serial = serial();
        let root = fixture();
        let tree = root.path().join("tree");
        fs::create_dir(&tree).expect("tree");
        fs::write(tree.join("content"), b"unchanged").expect("content");
        symlink(tree.join("content"), root.path().join("content-link")).expect("content link");
        let before = fixture_snapshot(root.path());
        let guard = tree_plan_with_timeout(
            &root,
            &tree,
            CoordinationMode::Shared,
            Duration::from_millis(100),
        )
        .acquire()
        .expect("reader");
        guard.revalidate().expect("reader revalidation");
        drop(guard);
        let after = fixture_snapshot(root.path());
        assert_eq!(before, after);
    }

    #[test]
    fn poisoned_process_gate_is_unavailable() {
        let gate: &'static RwLock<()> = Box::leak(Box::new(RwLock::new(())));
        let _ = std::panic::catch_unwind(|| {
            let _guard = gate.write().expect("test gate");
            panic!("poison test gate");
        });
        let deadline =
            CoordinationDeadline::new(Some(Duration::from_millis(10))).expect("gate deadline");
        assert!(matches!(
            acquire_gate_on(gate, CoordinationMode::Shared, &deadline),
            Err(CoordinationFailure::Unavailable { .. })
        ));
    }
}
