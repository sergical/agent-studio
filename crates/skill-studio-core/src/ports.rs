//! Ports: the traits adapters implement so the core can run anywhere.
//!
//! A port never decides policy. It moves bytes, tells the time, hands out
//! ids, holds leases, opens the history store, and reports notices.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::dto::{DeploymentDto, Inventory};
use crate::error::{CoreError, ErrorCode};
use crate::events::{EventDraft, EventFilter, EventRecord, EventStatus};
use crate::harness::HarnessCatalog;
use crate::identity::{CorrelationId, DeploymentId, EventId, Fingerprint, SkillName};
use crate::scope::{NormalizedScope, RuntimeScope};
use crate::snapshot::Revision;

/// Kind of a directory entry, without following links.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    /// Regular file.
    File,
    /// Directory.
    Dir,
    /// Symbolic link (target may be missing).
    Symlink,
    /// Anything else (socket, device).
    Other,
}

/// `lstat` facts about one path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileFacts {
    /// Kind without following links.
    pub kind: FileKind,
    /// Size in bytes for files; `0` otherwise.
    pub len: u64,
    /// Last modification time, when the platform reports one.
    pub modified: Option<DateTime<Utc>>,
    /// Unix mode bits, when the platform reports them.
    pub mode: Option<u32>,
}

/// One entry of a directory listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DirEntryFacts {
    /// File name (last component only).
    pub name: String,
    /// Kind without following links.
    pub kind: FileKind,
}

/// A path proven to lie inside the scope.
///
/// Invariant: only [`confine`] builds one. Every write helper takes a
/// `ScopedPath`, so the core cannot write outside the home and the projects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedPath {
    path: PathBuf,
}

impl ScopedPath {
    /// The confined path.
    pub fn as_path(&self) -> &Path {
        &self.path
    }
}

/// Proves that `path` lies under the scope home or one of its projects.
///
/// The check is lexical on the given path and on its parent's canonical
/// form, so a `..` segment or a link that escapes the scope is refused with
/// [`ErrorCode::InvalidRequest`].
pub fn confine(
    scope: &NormalizedScope,
    fs: &dyn ScopeFs,
    path: &Path,
) -> Result<ScopedPath, CoreError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "path must be absolute without `..`",
        )
        .at(path));
    }
    let parent = path.parent().unwrap_or(path);
    let parent_canonical = fs
        .canonicalize(parent)
        .map_err(|e| CoreError::io(parent, e))?;
    if scope.contains(path) && scope.contains(&parent_canonical) {
        Ok(ScopedPath {
            path: path.to_path_buf(),
        })
    } else {
        Err(CoreError::new(ErrorCode::InvalidRequest, "path lies outside the scope").at(path))
    }
}

/// Shared body for every [`ScopeFs::ancestor_holds`] implementation: walk
/// `start` and its ancestors via `fs.symlink_metadata`, one directory at a
/// time, stopping as soon as `dir.join(name)` resolves or the walk runs out
/// of parents. Never propagates a read error - a missing or unreadable
/// ancestor is the same as "does not hold `name`" - so the result is always
/// `Ok`. Bounding the walk to a scope is the caller's job: wrap `fs` in
/// [`ScopedReads`] first so a directory outside the scope reads as missing
/// rather than reaching the real adapter.
pub fn ancestor_holds(fs: &dyn ScopeFs, start: &Path, name: &str) -> std::io::Result<bool> {
    let mut dir = start.to_path_buf();
    loop {
        if fs.symlink_metadata(&dir.join(name)).is_ok() {
            return Ok(true);
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent.to_path_buf(),
            _ => return Ok(false),
        }
    }
}

/// A [`ScopeFs`] view over `inner` whose reads outside `scope` are reported
/// as missing.
///
/// [`ScopeFs`]'s read methods otherwise take plain, unchecked paths - see the
/// trait's own doc comment - so nothing stops a walk that climbs a path's
/// ancestors, like [`ScopeFs::ancestor_holds`], from reading real directories
/// above the scope's home or projects. Wrapping the adapter in `ScopedReads`
/// before such a walk is what makes it stop at the scope root instead of the
/// filesystem root.
pub struct ScopedReads<'a> {
    inner: &'a dyn ScopeFs,
    scope: &'a NormalizedScope,
}

impl<'a> ScopedReads<'a> {
    /// Wraps `inner`, confining every read to `scope`.
    pub fn new(inner: &'a dyn ScopeFs, scope: &'a NormalizedScope) -> Self {
        ScopedReads { inner, scope }
    }

    fn out_of_scope(_path: &Path) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::NotFound, "path lies outside the scope")
    }
}

impl ScopeFs for ScopedReads<'_> {
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.inner.canonicalize(path)
    }
    fn symlink_metadata(&self, path: &Path) -> std::io::Result<FileFacts> {
        if !self.scope.contains(path) {
            return Err(Self::out_of_scope(path));
        }
        self.inner.symlink_metadata(path)
    }
    fn read_link(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.inner.read_link(path)
    }
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<DirEntryFacts>> {
        self.inner.read_dir(path)
    }
    fn ancestor_holds(&self, start: &Path, name: &str) -> std::io::Result<bool> {
        ancestor_holds(self, start, name)
    }
    fn read_capped(&self, path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        self.inner.read_capped(path, max_bytes)
    }
    fn read_prefix(&self, path: &Path, limit: u64) -> std::io::Result<(Vec<u8>, bool)> {
        self.inner.read_prefix(path, limit)
    }
    fn write_atomic(
        &self,
        guard: &ExclusiveGuard,
        path: &ScopedPath,
        bytes: &[u8],
    ) -> std::io::Result<()> {
        self.inner.write_atomic(guard, path, bytes)
    }
    fn rename(
        &self,
        guard: &ExclusiveGuard,
        from: &ScopedPath,
        to: &ScopedPath,
    ) -> std::io::Result<()> {
        self.inner.rename(guard, from, to)
    }
    fn remove_file(&self, guard: &ExclusiveGuard, path: &ScopedPath) -> std::io::Result<()> {
        self.inner.remove_file(guard, path)
    }
    fn create_dir_all(&self, guard: &ExclusiveGuard, path: &ScopedPath) -> std::io::Result<()> {
        self.inner.create_dir_all(guard, path)
    }
    fn symlink(
        &self,
        guard: &ExclusiveGuard,
        target: &ScopedPath,
        link: &ScopedPath,
    ) -> std::io::Result<()> {
        self.inner.symlink(guard, target, link)
    }
}

/// Filesystem access. Read calls take plain paths; write calls take a
/// [`ScopedPath`] and an [`ExclusiveGuard`].
pub trait ScopeFs: Send + Sync {
    /// Resolves every symlink. Fails when the path does not exist.
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf>;
    /// `lstat` facts. Fails when the path does not exist.
    fn symlink_metadata(&self, path: &Path) -> std::io::Result<FileFacts>;
    /// Reads a symlink target without resolving it.
    fn read_link(&self, path: &Path) -> std::io::Result<PathBuf>;
    /// Lists a directory. Order is unspecified; callers sort.
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<DirEntryFacts>>;
    /// True when `start` or any ancestor of it inside the scope holds an entry
    /// named `name`. Walks up from `start` and stops at the scope root, so a
    /// repository outside the scope is not visible.
    fn ancestor_holds(&self, start: &Path, name: &str) -> std::io::Result<bool>;
    /// Reads at most `max_bytes`. A larger file is an error, not a truncation.
    fn read_capped(&self, path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>>;
    /// Reads at most `limit` bytes and reports whether more bytes followed.
    /// Unlike [`Self::read_capped`], a file over the limit is not an error:
    /// the caller gets the first `limit` bytes and `true`.
    fn read_prefix(&self, path: &Path, limit: u64) -> std::io::Result<(Vec<u8>, bool)>;
    /// Writes `bytes` through a temp file and rename, keeping permissions.
    fn write_atomic(
        &self,
        guard: &ExclusiveGuard,
        path: &ScopedPath,
        bytes: &[u8],
    ) -> std::io::Result<()>;
    /// Renames inside one filesystem.
    fn rename(
        &self,
        guard: &ExclusiveGuard,
        from: &ScopedPath,
        to: &ScopedPath,
    ) -> std::io::Result<()>;
    /// Removes a file or a symlink, never a directory.
    fn remove_file(&self, guard: &ExclusiveGuard, path: &ScopedPath) -> std::io::Result<()>;
    /// Creates a directory and its parents.
    fn create_dir_all(&self, guard: &ExclusiveGuard, path: &ScopedPath) -> std::io::Result<()>;
    /// Creates a symlink at `link` pointing to `target`.
    ///
    /// `target` is confined too: a link that points outside the scope would
    /// be a scope escape on the next scan, so the core never creates one.
    fn symlink(
        &self,
        guard: &ExclusiveGuard,
        target: &ScopedPath,
        link: &ScopedPath,
    ) -> std::io::Result<()>;
}

/// Finds the projects a scope covers under [`ProjectSelection::Discover`].
///
/// The desktop reads harness session stores (Claude Code `~/.claude/projects`
/// transcripts) to find them; the CLI may read a preference file instead.
/// The port returns candidate paths only. [`NormalizedScope::normalize_with_discovery`]
/// canonicalizes them, drops the ones that no longer exist, removes the
/// excluded ones, and refuses the home itself.
///
/// [`ProjectSelection::Discover`]: crate::scope::ProjectSelection::Discover
pub trait ProjectDiscovery: Send + Sync {
    /// Absolute candidate project paths for the home at `home_root`. Order
    /// does not matter; the scope sorts them.
    fn discover_projects(&self, home_root: &Path) -> Result<Vec<PathBuf>, CoreError>;
}

/// Resolves an executable name on the adapter's `PATH`.
///
/// The core never reads `PATH` itself. `capabilities` uses this port for
/// harness runner binaries and for the tools an installer needs (`npx`,
/// `dotagents`, `gh`).
pub trait ToolLookup: Send + Sync {
    /// Absolute path of `name`, or `None` when it is not on `PATH`.
    fn find_binary(&self, name: &str) -> Option<PathBuf>;
}

/// Wall clock and monotonic time.
pub trait Clock: Send + Sync {
    /// Current UTC time.
    fn now(&self) -> DateTime<Utc>;
    /// Time since an arbitrary fixed point; used for timings and budgets.
    fn monotonic(&self) -> Duration;
}

/// Source of fresh ids.
pub trait IdSource: Send + Sync {
    /// A new event id. Must sort after every id returned before.
    fn next_event_id(&self) -> EventId;
}

/// One lease key: a canonical physical root.
///
/// Invariant: the lease file lives under the adapter-supplied lease root,
/// never inside a user project. Keys are acquired in sorted order.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct LeaseKey {
    /// Canonical path of the root.
    pub canonical_root: PathBuf,
}

/// Lease mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LeaseMode {
    /// Many readers.
    Shared,
    /// One writer.
    Exclusive,
}

/// A held lease. Dropping it releases the lease.
pub trait LeaseHandle: Send {
    /// Keys this handle holds.
    fn keys(&self) -> &[LeaseKey];
    /// Mode this handle holds.
    fn mode(&self) -> LeaseMode;
}

/// Proof that a shared lease is held for the whole scope.
pub struct SharedGuard(Box<dyn LeaseHandle>);

impl SharedGuard {
    /// Keys the guard holds.
    pub fn keys(&self) -> &[LeaseKey] {
        self.0.keys()
    }
}

/// Proof that an exclusive lease is held for the whole scope.
///
/// Invariant: every write helper and every history append takes a reference
/// to one of these, so no write happens without the lease.
pub struct ExclusiveGuard(Box<dyn LeaseHandle>);

impl ExclusiveGuard {
    /// Keys the guard holds.
    pub fn keys(&self) -> &[LeaseKey] {
        self.0.keys()
    }
}

/// Acquires and releases leases.
pub trait LeaseProvider: Send + Sync {
    /// Acquires `keys` in the given order, waiting at most `wait`.
    /// Fails with [`ErrorCode::ScopeBusy`] when the budget runs out.
    fn acquire(
        &self,
        keys: &[LeaseKey],
        mode: LeaseMode,
        wait: Duration,
    ) -> Result<Box<dyn LeaseHandle>, CoreError>;
}

/// Acquires a shared lease over every root of the scope.
pub fn acquire_shared(
    leases: &dyn LeaseProvider,
    scope: &NormalizedScope,
) -> Result<SharedGuard, CoreError> {
    let keys = scope.lease_keys();
    let handle = leases.acquire(&keys, LeaseMode::Shared, scope.raw.read_timeout())?;
    Ok(SharedGuard(handle))
}

/// Acquires an exclusive lease over every root of the scope.
pub fn acquire_exclusive(
    leases: &dyn LeaseProvider,
    scope: &NormalizedScope,
) -> Result<ExclusiveGuard, CoreError> {
    let keys = scope.lease_keys();
    let handle = leases.acquire(&keys, LeaseMode::Exclusive, scope.raw.write_timeout())?;
    Ok(ExclusiveGuard(handle))
}

/// How an operation wants the history store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistoryAccess {
    /// Open only when the database already exists. Never creates it.
    ReadIfExists,
    /// Open or create. Requires an exclusive guard at the call site.
    ReadWrite,
}

/// Opens the history store for a scope.
pub trait HistoryOpener: Send + Sync {
    /// Returns `None` for [`HistoryAccess::ReadIfExists`] when no store exists.
    fn open(
        &self,
        scope: &NormalizedScope,
        access: HistoryAccess,
    ) -> Result<Option<Box<dyn HistoryStore>>, CoreError>;
}

/// The append-only event log and its backups.
///
/// Invariant: the store does not decide recovery. Startup recovery lives in
/// [`crate::events::recover_interrupted`] and takes the exclusive guard.
pub trait HistoryStore: Send {
    /// Lists events, newest first, honoring the filter.
    fn list(&self, filter: &EventFilter) -> Result<Vec<EventRecord>, CoreError>;
    /// Reads one event.
    fn get(&self, id: &EventId) -> Result<Option<EventRecord>, CoreError>;
    /// Copies `paths` into the backup directory for `id` and returns the
    /// manifest. Runs before the row exists.
    fn backup_paths(
        &mut self,
        guard: &ExclusiveGuard,
        id: &EventId,
        paths: &[PathBuf],
    ) -> Result<crate::events::BackupManifest, CoreError>;
    /// Inserts a `pending` row.
    fn record(
        &mut self,
        guard: &ExclusiveGuard,
        id: &EventId,
        draft: &EventDraft,
    ) -> Result<(), CoreError>;
    /// Sets the final status and the post-mutation fingerprint.
    fn finish(
        &mut self,
        guard: &ExclusiveGuard,
        id: &EventId,
        status: EventStatus,
        post_fingerprint: Option<Fingerprint>,
    ) -> Result<(), CoreError>;
    /// Compare-and-set claim of `target.reverted_by`. Returns `false` when
    /// another restore already claimed it.
    fn claim_revert(
        &mut self,
        guard: &ExclusiveGuard,
        target: &EventId,
        by: &EventId,
    ) -> Result<bool, CoreError>;
    /// Compare-and-set release of `target.reverted_by`, clearing it only when
    /// it still holds `restore`. Returns `false` when it holds anything else.
    /// A restore that claims the target and then fails before mutating calls
    /// this so the event stays revertible.
    fn release_revert(
        &mut self,
        guard: &ExclusiveGuard,
        target: &EventId,
        restore: &EventId,
    ) -> Result<bool, CoreError>;
    /// Rows still `pending`; only a crash leaves one behind.
    fn pending(&self) -> Result<Vec<EventRecord>, CoreError>;
    /// Reads back the manifest a prior [`Self::backup_paths`] call wrote for
    /// `backup_dir` (an [`EventRecord::backup_dir`] value). A restore uses
    /// this to find, for the path it is putting back, whether the backup
    /// holds bytes or recorded the path as absent, and the relative name to
    /// pass to [`Self::read_backup_bytes`].
    fn read_manifest(&self, backup_dir: &str) -> Result<crate::events::BackupManifest, CoreError>;
    /// Reads the bytes stored at `relative` inside `backup_dir`, as named by
    /// a present [`crate::events::BackupEntry::relative`] from
    /// [`Self::read_manifest`]. Never called for an absent entry.
    fn read_backup_bytes(&self, backup_dir: &str, relative: &str) -> Result<Vec<u8>, CoreError>;
}

/// Lifecycle state of one operation, reported through the sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpState {
    /// The request passed validation.
    Accepted,
    /// The lease is held and work started.
    Running,
    /// The mutation is durable.
    Committed,
    /// The operation failed; history holds a `failed` row when one was recorded.
    Failed,
    /// The caller cancelled before commit.
    Cancelled,
}

/// A notice the core sends to the adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "notice")]
pub enum CoreNotice {
    /// An operation changed state.
    OpState {
        /// Request the notice belongs to.
        correlation_id: CorrelationId,
        /// New state.
        state: OpState,
    },
    /// Progress inside one operation.
    Progress {
        /// Request the notice belongs to.
        correlation_id: CorrelationId,
        /// Short message for a person.
        message: String,
        /// Units done.
        done: u32,
        /// Units expected, when known.
        total: Option<u32>,
    },
    /// These skills and projects changed on disk; re-read them.
    Invalidated {
        /// Skill names affected.
        skills: Vec<SkillName>,
        /// Projects affected.
        projects: Vec<PathBuf>,
    },
    /// A new snapshot revision is available.
    Revision(Revision),
    /// Startup recovery marked these events interrupted.
    Recovered {
        /// Ids now marked `interrupted`.
        events: Vec<EventId>,
    },
}

/// Receives notices. Must not block.
pub trait EventSink: Send + Sync {
    /// Delivers one notice.
    fn notify(&self, notice: CoreNotice);
}

/// A child process to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProcessSpec {
    /// Program name or path.
    pub program: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Working directory.
    pub cwd: Option<PathBuf>,
    /// Extra environment; the adapter decides what else is inherited.
    pub env: Vec<(String, String)>,
    /// Hard deadline in milliseconds.
    pub timeout_ms: u64,
}

/// What a child process produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProcessOutput {
    /// Exit status, `None` when killed by a signal.
    pub status: Option<i32>,
    /// Captured stdout, capped by the adapter.
    pub stdout: String,
    /// Captured stderr tail, capped by the adapter.
    pub stderr: String,
    /// True when the deadline killed the process.
    pub timed_out: bool,
}

/// Runs child processes with cancellation.
pub trait ProcessSpawner: Send + Sync {
    /// Runs to completion, deadline, or cancellation.
    fn run(&self, spec: &ProcessSpec, cancel: &dyn CancelToken)
        -> Result<ProcessOutput, CoreError>;
}

/// Cooperative cancellation.
pub trait CancelToken: Send + Sync {
    /// True once the caller asked to stop.
    fn is_cancelled(&self) -> bool;
}

/// A token that never cancels; the default for synchronous CLI calls.
#[derive(Debug, Default, Clone, Copy)]
pub struct NeverCancel;

impl CancelToken for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// Everything the core needs from the outside world.
///
/// Invariant: `catalog` is the only source of harness facts. Adapters may
/// swap it in fixture mode to test unknown-capability paths.
#[derive(Clone)]
pub struct Ports {
    /// Filesystem.
    pub fs: Arc<dyn ScopeFs>,
    /// Time.
    pub clock: Arc<dyn Clock>,
    /// Ids.
    pub ids: Arc<dyn IdSource>,
    /// Leases.
    pub leases: Arc<dyn LeaseProvider>,
    /// History store opener.
    pub history: Arc<dyn HistoryOpener>,
    /// Notice sink.
    pub sink: Arc<dyn EventSink>,
    /// Process runner; `None` disables runner-backed operations.
    pub spawner: Option<Arc<dyn ProcessSpawner>>,
    /// Project discovery; `None` makes [`ProjectSelection::Discover`] an
    /// [`ErrorCode::InvalidScope`].
    ///
    /// [`ProjectSelection::Discover`]: crate::scope::ProjectSelection::Discover
    pub discovery: Option<Arc<dyn ProjectDiscovery>>,
    /// `PATH` lookup; `None` disables tool and runner observation.
    pub tools: Option<Arc<dyn ToolLookup>>,
    /// Harness facts.
    pub catalog: Arc<HarnessCatalog>,
}

/// Per-call context.
#[derive(Clone)]
pub struct OpContext {
    /// Id the adapter uses to match notices to this request.
    pub correlation_id: CorrelationId,
    /// Cancellation for this request only.
    pub cancel: Arc<dyn CancelToken>,
}

impl OpContext {
    /// A context that cannot be cancelled.
    pub fn uncancellable(correlation_id: CorrelationId) -> Self {
        OpContext {
            correlation_id,
            cancel: Arc::new(NeverCancel),
        }
    }

    /// Fails with [`ErrorCode::Cancelled`] once the token is set.
    pub fn checkpoint(&self) -> Result<(), CoreError> {
        if self.cancel.is_cancelled() {
            Err(CoreError::new(ErrorCode::Cancelled, "operation cancelled"))
        } else {
            Ok(())
        }
    }
}

/// A normalized scope bound to its ports.
#[derive(Clone)]
pub struct Runtime {
    /// The scope every operation uses.
    pub scope: NormalizedScope,
    /// The ports every operation uses.
    pub ports: Ports,
}

impl Runtime {
    /// Normalizes `scope` through `ports.fs` and `ports.discovery`, then
    /// binds them. Discovered projects are part of the lease keys from here
    /// on; a later `scan` never widens the scope.
    pub fn new(scope: &RuntimeScope, ports: Ports) -> Result<Self, CoreError> {
        let scope = NormalizedScope::normalize_with_discovery(
            scope,
            ports.fs.as_ref(),
            ports.discovery.as_deref(),
        )?;
        Ok(Runtime { scope, ports })
    }
}

/// The state a mutation holds from lease to commit.
///
/// Invariant: `fresh` was built after the exclusive lease was taken, so a
/// target resolved against it cannot be stale.
pub struct MutationSession {
    /// The exclusive lease.
    pub guard: ExclusiveGuard,
    /// The writable history store.
    pub store: Box<dyn HistoryStore>,
    /// Inventory scanned under the lease.
    pub fresh: Inventory,
}

impl MutationSession {
    /// Takes the exclusive lease, opens history, recovers interrupted rows,
    /// and scans a fresh inventory.
    pub fn begin(rt: &Runtime, ctx: &OpContext) -> Result<Self, CoreError> {
        ctx.checkpoint()?;
        let guard = acquire_exclusive(rt.ports.leases.as_ref(), &rt.scope)?;
        let Some(mut store) = rt.ports.history.open(&rt.scope, HistoryAccess::ReadWrite)? else {
            return Err(CoreError::new(
                ErrorCode::Unsupported,
                "this host build has no history store; mutations are not available",
            ));
        };
        crate::events::recover_interrupted(
            &guard,
            store.as_mut(),
            rt.ports.fs.as_ref(),
            rt.ports.sink.as_ref(),
        )?;
        // Scan under the exclusive lease already held: `crate::ops::scan`
        // would try to acquire a second (shared) lease over the same keys,
        // and an advisory file lock does not nest within one process.
        let fresh = crate::ops::scan_inner(
            rt,
            ctx,
            &crate::dto::ScanRequest {
                skills: Vec::new(),
                timings: false,
            },
        )?;
        Ok(MutationSession {
            guard,
            store,
            fresh,
        })
    }

    /// Finds exactly one deployment by id in the fresh inventory.
    ///
    /// Fails with [`ErrorCode::AmbiguousTarget`] when zero or more than one
    /// deployment carries the id.
    pub fn resolve_exact(&self, id: &DeploymentId) -> Result<&DeploymentDto, CoreError> {
        let mut found = self
            .fresh
            .skills
            .iter()
            .flat_map(|s| s.deployments.iter())
            .filter(|d| &d.id == id);
        match (found.next(), found.next()) {
            (Some(one), None) => Ok(one),
            (None, _) => Err(CoreError::new(
                ErrorCode::AmbiguousTarget,
                format!("no deployment matches {}", id.as_str()),
            )),
            (Some(_), Some(_)) => Err(CoreError::new(
                ErrorCode::AmbiguousTarget,
                format!("more than one deployment matches {}", id.as_str()),
            )),
        }
    }

    /// Releases the lease and reports `Committed`.
    pub fn finish(self, rt: &Runtime, ctx: &OpContext) {
        drop(self.store);
        drop(self.guard);
        rt.ports.sink.notify(CoreNotice::OpState {
            correlation_id: ctx.correlation_id.clone(),
            state: OpState::Committed,
        });
    }
}
