//! Fixture builder and fakes for adapter and core tests.
//!
//! Enabled with the `testing` feature or under `cfg(test)`. Nothing here
//! touches the real filesystem.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};

use crate::error::{CoreError, ErrorCode};
use crate::identity::EventId;
use crate::ports::{
    CancelToken, Clock, CoreNotice, DirEntryFacts, EventSink, ExclusiveGuard, FileFacts, FileKind,
    HistoryAccess, HistoryOpener, HistoryStore, IdSource, LeaseHandle, LeaseKey, LeaseMode,
    LeaseProvider, ProjectDiscovery, ScopeFs, ScopedPath, ToolLookup,
};
use crate::scope::NormalizedScope;

/// A discovery port that returns a fixed list of candidate projects.
#[derive(Debug, Default, Clone)]
pub struct FakeProjectDiscovery {
    /// Paths returned for every home.
    pub projects: Vec<PathBuf>,
}

impl ProjectDiscovery for FakeProjectDiscovery {
    fn discover_projects(&self, _home_root: &Path) -> Result<Vec<PathBuf>, CoreError> {
        Ok(self.projects.clone())
    }
}

/// A `PATH` lookup over a fixed name-to-path table.
#[derive(Debug, Default, Clone)]
pub struct FakeToolLookup {
    /// Binaries that "exist", by name.
    pub binaries: BTreeMap<String, PathBuf>,
}

impl ToolLookup for FakeToolLookup {
    fn find_binary(&self, name: &str) -> Option<PathBuf> {
        self.binaries.get(name).cloned()
    }
}

/// Describes an in-memory tree: directories, files, and aliases (symlinks).
#[derive(Debug, Default, Clone)]
pub struct FixtureBuilder {
    dirs: Vec<PathBuf>,
    files: BTreeMap<PathBuf, Vec<u8>>,
    aliases: BTreeMap<PathBuf, PathBuf>,
}

impl FixtureBuilder {
    /// An empty fixture.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a directory and its parents.
    pub fn dir(mut self, path: &str) -> Self {
        self.dirs.push(PathBuf::from(path));
        self
    }

    /// Adds a file with bytes; parents are implied.
    pub fn file(mut self, path: &str, bytes: &[u8]) -> Self {
        self.files.insert(PathBuf::from(path), bytes.to_vec());
        self
    }

    /// Adds a symlink `link -> target`.
    pub fn alias(mut self, link: &str, target: &str) -> Self {
        self.aliases
            .insert(PathBuf::from(link), PathBuf::from(target));
        self
    }

    /// Builds the in-memory filesystem.
    ///
    /// Every file, alias, and declared dir implies its ancestor
    /// directories exist (matching [`Self::file`]'s "parents are implied"
    /// and what [`Self::materialize`]'s `create_dir_all` does on real
    /// disk), so those ancestors are filled in here even when the caller
    /// only declared the leaf: [`FixtureFs::read_dir`] lists only the
    /// direct children it was explicitly told about, and a root's scan
    /// walks one directory level at a time, so an unlisted intermediate
    /// directory would otherwise never be found.
    pub fn build_fs(mut self) -> FixtureFs {
        let mut ancestors = Vec::new();
        for leaf in self
            .dirs
            .iter()
            .chain(self.files.keys())
            .chain(self.aliases.keys())
        {
            let mut current = leaf.as_path();
            while let Some(parent) = current.parent() {
                if parent.as_os_str().is_empty() {
                    break;
                }
                ancestors.push(parent.to_path_buf());
                current = parent;
            }
        }
        self.dirs.extend(ancestors);
        FixtureFs {
            dirs: self.dirs,
            files: self.files,
            aliases: self.aliases,
        }
    }

    /// Like [`Self::build_fs`], but first substitutes [`FIXTURE_HOME_TOKEN`]
    /// in every file's bytes with `home`, the same substitution
    /// [`Self::materialize`] does for a real directory. Lets an in-memory
    /// fixture that embeds a materialize-time absolute path (e.g. a Codex
    /// `config.toml` row naming a skill by its canonical `SKILL.md` path) be
    /// read correctly by a `ScopeFs` test that never touches the real
    /// filesystem.
    pub fn build_fs_with_home(mut self, home: &Path) -> FixtureFs {
        let home = home.to_string_lossy().into_owned();
        for bytes in self.files.values_mut() {
            if bytes
                .windows(FIXTURE_HOME_TOKEN.len())
                .any(|w| w == FIXTURE_HOME_TOKEN.as_bytes())
            {
                *bytes = String::from_utf8_lossy(bytes)
                    .replace(FIXTURE_HOME_TOKEN, &home)
                    .into_bytes();
            }
        }
        self.build_fs()
    }

    /// Rebases every declared dir, file, and alias *link* (not alias
    /// targets, which stay relative to the link the same way a real symlink
    /// would) under `root`. Lets a fixture written with home-relative paths
    /// (as every [`fixtures`] builder is, for reuse with
    /// [`Self::materialize`]) be scanned in memory too, where
    /// [`Self::build_fs`] has no directory of its own to join against and a
    /// [`crate::scope::RuntimeScope`] needs an absolute home.
    pub fn rooted_at(self, root: &str) -> Self {
        let root = Path::new(root);
        FixtureBuilder {
            dirs: self.dirs.into_iter().map(|d| root.join(d)).collect(),
            files: self
                .files
                .into_iter()
                .map(|(path, bytes)| (root.join(path), bytes))
                .collect(),
            aliases: self
                .aliases
                .into_iter()
                .map(|(link, target)| (root.join(link), target))
                .collect(),
        }
    }

    /// Writes this fixture to a real directory: every declared dir is
    /// created, every file is written (parents implied), and every alias
    /// becomes a real symlink. `dir` itself must already exist. Order
    /// matters only for aliases, which are created last so a symlink whose
    /// target is a fixture dir or file always has something to point at.
    ///
    /// A file's bytes may contain the literal token [`FIXTURE_HOME_TOKEN`];
    /// it is replaced with `dir`'s path before the file is written. Fixtures
    /// that need to embed an absolute path only known at materialize time
    /// (e.g. a Codex `config.toml` row naming a skill by its canonical
    /// `SKILL.md` path) use this instead of hardcoding a path that would be
    /// wrong for every run but the one that happened to pick it.
    pub fn materialize(&self, dir: &Path) -> std::io::Result<()> {
        for d in &self.dirs {
            std::fs::create_dir_all(dir.join(d))?;
        }
        let home = dir.to_string_lossy();
        for (path, bytes) in &self.files {
            let full = dir.join(path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if bytes
                .windows(FIXTURE_HOME_TOKEN.len())
                .any(|w| w == FIXTURE_HOME_TOKEN.as_bytes())
            {
                let text = String::from_utf8_lossy(bytes).replace(FIXTURE_HOME_TOKEN, &home);
                std::fs::write(full, text)?;
            } else {
                std::fs::write(full, bytes)?;
            }
        }
        for (link, target) in &self.aliases {
            let full = dir.join(link);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)?;
            }
            symlink(target, &full)?;
        }
        Ok(())
    }
}

/// Placeholder a fixture's file bytes can contain in place of the real
/// materialize-time absolute path; see [`FixtureBuilder::materialize`].
pub const FIXTURE_HOME_TOKEN: &str = "{{FIXTURE_HOME}}";

/// Creates a symlink at `link` pointing at `target`. Unix only, matching the
/// rest of Skill Studio's symlink handling (`skill_park.rs`,
/// `skill_harness_disable.rs`).
#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "FixtureBuilder::materialize needs symlinks, which this platform doesn't support",
    ))
}

/// In-memory [`ScopeFs`]. Aliases resolve lexically, component by component.
#[derive(Debug, Clone)]
pub struct FixtureFs {
    dirs: Vec<PathBuf>,
    files: BTreeMap<PathBuf, Vec<u8>>,
    aliases: BTreeMap<PathBuf, PathBuf>,
}

impl FixtureFs {
    fn exists(&self, path: &Path) -> bool {
        self.dirs.iter().any(|d| d == path || d.starts_with(path))
            || self.files.keys().any(|f| f == path || f.starts_with(path))
    }

    /// Walks `path` one component at a time. An alias may be registered on
    /// the lexical prefix (as a user typed it) or on the resolved prefix, so
    /// both are checked at every step. A relative alias target (the common
    /// case: `../../.agents/skills/gamma`) is resolved the way a real
    /// symlink is - relative to the directory containing the link, not to
    /// the process's working directory - and any `..`/`.` segments the
    /// target or the substitution introduces are collapsed lexically, the
    /// same normalization `std::fs::canonicalize` performs on real disk.
    fn resolve(&self, path: &Path) -> Option<PathBuf> {
        let mut lexical = PathBuf::new();
        let mut out = PathBuf::new();
        for component in path.components() {
            lexical.push(component);
            let parent = out.clone();
            out.push(component);
            if let Some(target) = self.aliases.get(&lexical) {
                out = Self::join_normalized(&parent, target);
            }
            let mut hops = 0;
            while let Some(target) = self.aliases.get(&out) {
                hops += 1;
                if hops > 32 {
                    return None;
                }
                let out_parent = out.parent().unwrap_or(Path::new("")).to_path_buf();
                out = Self::join_normalized(&out_parent, target);
            }
        }
        Some(out)
    }

    /// Joins `target` onto `base` (unless `target` is already absolute) and
    /// collapses `.`/`..` segments lexically, without touching the real
    /// filesystem.
    fn join_normalized(base: &Path, target: &Path) -> PathBuf {
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

    fn not_found(path: &Path) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::NotFound, path.display().to_string())
    }

    /// Resolves every ancestor of `path` (as [`Self::resolve`] does for a
    /// full path) but leaves the final component untouched, matching real
    /// `lstat` semantics: a path reached through a resolved directory
    /// symlink is looked up correctly, but the leaf itself is reported
    /// without being followed. Without this, a caller that joins a
    /// resolved directory's *lexical* path (as real `lstat`-based code
    /// does; see `ops::walk_content_files`) with an entry name would look
    /// up a path this in-memory filesystem never declared.
    fn resolve_leaf(&self, path: &Path) -> PathBuf {
        match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => {
                let resolved_parent = self.resolve(parent).unwrap_or_else(|| parent.to_path_buf());
                resolved_parent.join(name)
            }
            _ => path.to_path_buf(),
        }
    }
}

impl ScopeFs for FixtureFs {
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        match self.resolve(path) {
            Some(resolved) if self.exists(&resolved) => Ok(resolved),
            _ => Err(Self::not_found(path)),
        }
    }

    fn symlink_metadata(&self, path: &Path) -> std::io::Result<FileFacts> {
        let path = &self.resolve_leaf(path);
        let kind = if self.aliases.contains_key(path) {
            FileKind::Symlink
        } else if self.files.contains_key(path) {
            FileKind::File
        } else if self.exists(path) {
            FileKind::Dir
        } else {
            return Err(Self::not_found(path));
        };
        Ok(FileFacts {
            kind,
            len: self.files.get(path).map(|b| b.len() as u64).unwrap_or(0),
            modified: None,
            mode: None,
        })
    }

    fn read_link(&self, path: &Path) -> std::io::Result<PathBuf> {
        let path = self.resolve_leaf(path);
        self.aliases
            .get(&path)
            .cloned()
            .ok_or_else(|| Self::not_found(&path))
    }

    fn ancestor_holds(&self, start: &Path, name: &str) -> std::io::Result<bool> {
        crate::ports::ancestor_holds(self, start, name)
    }

    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<DirEntryFacts>> {
        let dir = self.canonicalize(path)?;
        let mut names: BTreeMap<String, FileKind> = BTreeMap::new();
        for d in &self.dirs {
            if d.parent() == Some(&dir) {
                names.insert(crate::identity::leaf_name(d), FileKind::Dir);
            }
        }
        for f in self.files.keys() {
            if f.parent() == Some(&dir) {
                names.insert(crate::identity::leaf_name(f), FileKind::File);
            }
        }
        for a in self.aliases.keys() {
            if a.parent() == Some(&dir) {
                names.insert(crate::identity::leaf_name(a), FileKind::Symlink);
            }
        }
        Ok(names
            .into_iter()
            .map(|(name, kind)| DirEntryFacts { name, kind })
            .collect())
    }

    fn read_capped(&self, path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        let resolved = self.canonicalize(path)?;
        let bytes = self
            .files
            .get(&resolved)
            .ok_or_else(|| Self::not_found(path))?;
        let len = bytes.len() as u64;
        if len > max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{} is {len} bytes, over the {max_bytes} byte cap",
                    path.display()
                ),
            ));
        }
        Ok(bytes.clone())
    }

    fn read_prefix(&self, path: &Path, limit: u64) -> std::io::Result<(Vec<u8>, bool)> {
        let resolved = self.canonicalize(path)?;
        let bytes = self
            .files
            .get(&resolved)
            .ok_or_else(|| Self::not_found(path))?;
        let truncated = bytes.len() as u64 > limit;
        let prefix = if truncated {
            bytes[..limit as usize].to_vec()
        } else {
            bytes.clone()
        };
        Ok((prefix, truncated))
    }

    fn write_atomic(&self, _: &ExclusiveGuard, _: &ScopedPath, _: &[u8]) -> std::io::Result<()> {
        Err(std::io::Error::other("FixtureFs is read-only"))
    }

    fn rename(&self, _: &ExclusiveGuard, _: &ScopedPath, _: &ScopedPath) -> std::io::Result<()> {
        Err(std::io::Error::other("FixtureFs is read-only"))
    }

    fn remove_file(&self, _: &ExclusiveGuard, _: &ScopedPath) -> std::io::Result<()> {
        Err(std::io::Error::other("FixtureFs is read-only"))
    }

    fn create_dir_all(&self, _: &ExclusiveGuard, _: &ScopedPath) -> std::io::Result<()> {
        Err(std::io::Error::other("FixtureFs is read-only"))
    }

    fn symlink(&self, _: &ExclusiveGuard, _: &ScopedPath, _: &ScopedPath) -> std::io::Result<()> {
        Err(std::io::Error::other("FixtureFs is read-only"))
    }
}

/// Wraps another [`ScopeFs`], failing exactly one `write_atomic` call and
/// delegating everything else. Lets a test exercise a mutation that fails
/// partway through without a second fake filesystem.
pub struct FailingFs {
    inner: Arc<dyn ScopeFs>,
    fail_next_write_atomic: AtomicBool,
}

impl FailingFs {
    /// Wraps `inner`, initially failing nothing.
    pub fn wrap(inner: Arc<dyn ScopeFs>) -> Self {
        FailingFs {
            inner,
            fail_next_write_atomic: AtomicBool::new(false),
        }
    }

    /// The next `write_atomic` call returns an error instead of reaching
    /// `inner`; later calls delegate normally again.
    pub fn fail_next_write_atomic(&self) {
        self.fail_next_write_atomic.store(true, Ordering::SeqCst);
    }
}

impl ScopeFs for FailingFs {
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.inner.canonicalize(path)
    }
    fn symlink_metadata(&self, path: &Path) -> std::io::Result<FileFacts> {
        self.inner.symlink_metadata(path)
    }
    fn read_link(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.inner.read_link(path)
    }
    fn ancestor_holds(&self, start: &Path, name: &str) -> std::io::Result<bool> {
        self.inner.ancestor_holds(start, name)
    }
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<DirEntryFacts>> {
        self.inner.read_dir(path)
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
        if self.fail_next_write_atomic.swap(false, Ordering::SeqCst) {
            return Err(std::io::Error::other(
                "FailingFs: injected write_atomic failure",
            ));
        }
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

/// A clock that only moves when a test advances it.
#[derive(Debug)]
pub struct FakeClock {
    now_ms: AtomicU64,
}

impl FakeClock {
    /// Starts at the given epoch milliseconds.
    pub fn at(epoch_ms: u64) -> Self {
        FakeClock {
            now_ms: AtomicU64::new(epoch_ms),
        }
    }

    /// Advances the clock.
    pub fn advance(&self, by: Duration) {
        self.now_ms
            .fetch_add(by.as_millis() as u64, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        Utc.timestamp_millis_opt(self.now_ms.load(Ordering::SeqCst) as i64)
            .single()
            .unwrap_or_else(Utc::now)
    }

    fn monotonic(&self) -> Duration {
        Duration::from_millis(self.now_ms.load(Ordering::SeqCst))
    }
}

/// Sequential, sortable fake ids (`01FAKE...000001`).
#[derive(Debug, Default)]
pub struct FakeIds {
    next: AtomicU64,
}

impl IdSource for FakeIds {
    fn next_event_id(&self) -> EventId {
        let n = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        EventId(format!("01FAKE{n:020}"))
    }
}

/// A lease provider whose exclusive lock a test can hold from outside.
#[derive(Debug, Default)]
pub struct FakeLease {
    held_exclusive: Mutex<Vec<LeaseKey>>,
}

impl FakeLease {
    /// Simulates another process holding an exclusive lease on `keys`.
    pub fn hold_exclusive(&self, keys: &[LeaseKey]) {
        self.held_exclusive
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(keys);
    }

    /// Releases every simulated lease.
    pub fn release_all(&self) {
        self.held_exclusive
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

struct FakeHandle {
    keys: Vec<LeaseKey>,
    mode: LeaseMode,
}

impl LeaseHandle for FakeHandle {
    fn keys(&self) -> &[LeaseKey] {
        &self.keys
    }

    fn mode(&self) -> LeaseMode {
        self.mode
    }
}

impl LeaseProvider for FakeLease {
    fn acquire(
        &self,
        keys: &[LeaseKey],
        mode: LeaseMode,
        _wait: Duration,
    ) -> Result<Box<dyn LeaseHandle>, CoreError> {
        let held = self
            .held_exclusive
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(busy) = keys.iter().find(|k| held.contains(k)) {
            return Err(
                CoreError::new(ErrorCode::ScopeBusy, "another process holds the lease")
                    .at(&busy.canonical_root),
            );
        }
        Ok(Box::new(FakeHandle {
            keys: keys.to_vec(),
            mode,
        }))
    }
}

/// A history opener for scopes that have no event log.
///
/// `ReadIfExists` answers `None`; `ReadWrite` fails with `io`, so a test
/// notices when a read path tries to create the store.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoHistory;

impl HistoryOpener for NoHistory {
    fn open(
        &self,
        scope: &NormalizedScope,
        access: HistoryAccess,
    ) -> Result<Option<Box<dyn HistoryStore>>, CoreError> {
        match access {
            HistoryAccess::ReadIfExists => Ok(None),
            HistoryAccess::ReadWrite => Err(CoreError::new(
                ErrorCode::Io,
                "NoHistory cannot create an event store",
            )
            .at(&scope.history_root)),
        }
    }
}

/// A token that reports cancelled from the start, so an [`crate::ports::OpContext`]
/// built with it fails its very first `checkpoint()`.
#[derive(Debug, Default, Clone, Copy)]
pub struct AlwaysCancel;

impl CancelToken for AlwaysCancel {
    fn is_cancelled(&self) -> bool {
        true
    }
}

/// Collects notices for assertions.
#[derive(Debug, Default)]
pub struct RecordingSink {
    notices: Mutex<Vec<CoreNotice>>,
}

impl RecordingSink {
    /// Everything received so far.
    pub fn notices(&self) -> Vec<CoreNotice> {
        self.notices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl EventSink for RecordingSink {
    fn notify(&self, notice: CoreNotice) {
        self.notices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(notice);
    }
}

/// Rewrites absolute fixture paths and ids so two runs compare equal.
///
/// Invariant: array order is preserved; only string values that start with
/// a registered root are rewritten.
#[derive(Debug, Default, Clone)]
pub struct Normalizer {
    roots: Vec<(PathBuf, String)>,
}

impl Normalizer {
    /// Replaces `root` with `token` in every string value.
    pub fn root(mut self, root: &str, token: &str) -> Self {
        self.roots.push((PathBuf::from(root), token.to_string()));
        self
    }

    /// Applies the rewrite to a JSON value.
    pub fn apply(&self, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(s) => {
                for (root, token) in &self.roots {
                    let prefix = root.to_string_lossy();
                    if let Some(rest) = s.strip_prefix(prefix.as_ref()) {
                        *s = format!("{token}{rest}");
                        break;
                    }
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(|v| self.apply(v)),
            serde_json::Value::Object(map) => map.values_mut().for_each(|v| self.apply(v)),
            _ => {}
        }
    }
}

/// Named [`FixtureBuilder`]s covering the scanner scenarios the core and its
/// adapters test against: a clean home, ordinary per-harness and shared
/// deployments, broken and whole-directory symlinks, spec violations,
/// legacy/parked/plugin/project roots, and the disable mechanisms each
/// harness uses. Every SKILL.md is spec-valid unless the fixture's own point
/// is a violation.
pub mod fixtures {
    use super::{FixtureBuilder, FIXTURE_HOME_TOKEN};
    use crate::identity::{MOVE_ASIDE_DIR_NAME, PARKED_ROOT_RELATIVE, UNIVERSAL_ROOT_RELATIVE};

    /// A minimal spec-valid `SKILL.md`: frontmatter `name` matches `name`,
    /// and `description` is a non-empty, sub-1024-char sentence.
    fn skill_md(name: &str) -> String {
        format!("---\nname: {name}\ndescription: Helps with {name} for fixture-driven tests.\n---\nBody text for {name}.\n")
    }

    fn skill(builder: FixtureBuilder, dir: &str, name: &str) -> FixtureBuilder {
        // The skill directory itself must be a declared dir, not just
        // implied by its `SKILL.md` file: `FixtureFs::read_dir` lists only
        // the direct children it was explicitly told about, so a root's
        // scan would otherwise never see this skill directory as an entry
        // to walk into.
        builder
            .dir(dir)
            .file(&format!("{dir}/SKILL.md"), skill_md(name).as_bytes())
    }

    /// `empty_home`: a home with none of the roots populated.
    fn empty_home() -> FixtureBuilder {
        FixtureBuilder::new()
    }

    /// `basic`: one skill per harness plus a universal skill symlinked
    /// into two harnesses, and a lock file naming the universal skill as
    /// installed via skills.sh.
    fn basic() -> FixtureBuilder {
        let mut b = FixtureBuilder::new();
        b = skill(b, ".claude/skills/alpha", "alpha");
        b = skill(b, ".codex/skills/beta", "beta");
        b = skill(b, &format!("{UNIVERSAL_ROOT_RELATIVE}/gamma"), "gamma");
        b = b
            .alias(
                ".claude/skills/gamma",
                &format!("../../{UNIVERSAL_ROOT_RELATIVE}/gamma"),
            )
            .alias(
                ".codex/skills/gamma",
                &format!("../../{UNIVERSAL_ROOT_RELATIVE}/gamma"),
            );
        b.file(
            ".agents/.skill-lock.json",
            br#"{"version":3,"skills":{"gamma":{"source":"owner/gamma","sourceType":"github","sourceUrl":"https://github.com/owner/gamma","skillFolderHash":"deadbeef","installedAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}}}"#,
        )
    }

    /// `broken_link`: a per-skill Claude Code symlink whose target doesn't exist.
    fn broken_link() -> FixtureBuilder {
        FixtureBuilder::new().alias(
            ".claude/skills/ghost",
            &format!("../../{UNIVERSAL_ROOT_RELATIVE}/missing"),
        )
    }

    /// `malformed_frontmatter`: an unquoted `description: ...: ...` that
    /// breaks YAML parsing, plus a second skill whose frontmatter `name` has
    /// uppercase letters (a spec violation, not a YAML error).
    fn malformed_frontmatter() -> FixtureBuilder {
        FixtureBuilder::new()
            .file(
                ".claude/skills/zeta-bad/SKILL.md",
                b"---\nname: zeta-bad\ndescription: Use this: when needed\n---\nBody.\n",
            )
            .file(
                ".claude/skills/UpperCase/SKILL.md",
                b"---\nname: UpperCase\ndescription: Has an uppercase name, which the spec forbids.\n---\nBody.\n",
            )
    }

    /// `manual_repairable_frontmatter`: a hand-made skill in a per-harness
    /// root with no lock entry (so it is `Manual`-owned, per
    /// `classify_owner`'s default), with the same unquoted `description: a:
    /// b` shape as `malformed_frontmatter`'s `zeta-bad`. `Manual` ownership
    /// makes the deployment `ReadOnly` (`LifecycleOwnerKind::is_mutable`
    /// excludes it), and the desktop's repair gate carves out exactly this
    /// case (`owner_kind == Manual`) to still offer `ApplyFix` - the
    /// regression this fixture exists to catch had the core's gate miss
    /// that carve-out and refuse every hand-made skill.
    fn manual_repairable_frontmatter() -> FixtureBuilder {
        FixtureBuilder::new().file(
            ".claude/skills/badfm/SKILL.md",
            b"---\nname: badfm\ndescription: Use this: when the frontmatter is broken\n---\nBody.\n",
        )
    }

    /// `legacy_opencode`: OpenCode's older singular `skill/` root
    /// alongside its current `skills/` root.
    fn legacy_opencode() -> FixtureBuilder {
        let mut b = FixtureBuilder::new();
        b = skill(b, ".config/opencode/skill/delta", "delta");
        skill(b, ".config/opencode/skills/epsilon", "epsilon")
    }

    /// `parked`: a skill parked (globally disabled) in the shared parked root.
    fn parked() -> FixtureBuilder {
        skill(
            FixtureBuilder::new(),
            &format!("{PARKED_ROOT_RELATIVE}/zeta"),
            "zeta",
        )
    }

    /// `plugin_cache`: one plugin-shipped skill each for Claude Code and
    /// Codex, in the exact layout `plugins.rs` walks:
    /// `cache/<marketplace>/<plugin>/<version>/`.
    fn plugin_cache() -> FixtureBuilder {
        let mut b = FixtureBuilder::new();
        let claude_root = ".claude/plugins/cache/some-marketplace/sentry-toolkit/1.0.0";
        b = b.file(
            &format!("{claude_root}/.claude-plugin/plugin.json"),
            br#"{"name": "sentry-toolkit", "version": "1.0.0"}"#,
        );
        b = skill(b, &format!("{claude_root}/skills/lint-code"), "lint-code");

        let codex_root = ".codex/plugins/cache/some-marketplace/codex-toolkit/2.0.0";
        b = b.file(
            &format!("{codex_root}/.codex-plugin/plugin.json"),
            br#"{"name": "codex-toolkit", "version": "2.0.0"}"#,
        );
        skill(b, &format!("{codex_root}/skills/audit-code"), "audit-code")
    }

    /// `project`: a registered project with a `.git` marker and three
    /// roots: Claude Code, the project's shared `.agents/skills`, and
    /// OpenCode's legacy `skill/` root.
    fn project() -> FixtureBuilder {
        let mut b = FixtureBuilder::new().dir("proj/.git");
        b = skill(b, "proj/.claude/skills/eta", "eta");
        b = skill(b, &format!("proj/{UNIVERSAL_ROOT_RELATIVE}/theta"), "theta");
        skill(b, "proj/.opencode/skill/iota", "iota")
    }

    /// `disabled`: every disable mechanism the core knows about: Codex's
    /// own `config.toml` row (keyed by `beta`'s canonical `SKILL.md` path,
    /// filled in at materialize time), OpenCode's `permission.skill` deny,
    /// and Skill Studio's own move-aside directory.
    fn disabled() -> FixtureBuilder {
        let mut b = FixtureBuilder::new();
        b = skill(b, ".codex/skills/beta", "beta");
        b = skill(b, ".config/opencode/skills/epsilon", "epsilon");
        b = skill(
            b,
            &format!(".claude/skills/{MOVE_ASIDE_DIR_NAME}/kappa"),
            "kappa",
        );
        let codex_config = format!(
            "[[skills.config]]\npath = \"{FIXTURE_HOME_TOKEN}/.codex/skills/beta/SKILL.md\"\nenabled = false\n"
        );
        b = b.file(".codex/config.toml", codex_config.as_bytes());
        b.file(
            ".config/opencode/opencode.json",
            br#"{"$schema": "https://opencode.ai/config.json", "permission": {"skill": {"epsilon": "deny"}}}"#,
        )
    }

    /// `whole_dir_link`: Claude Code's own skills root is itself a
    /// symlink to the shared root, so every skill under it is reachable
    /// through a whole-directory link rather than a per-skill one.
    fn whole_dir_link() -> FixtureBuilder {
        let b = skill(
            FixtureBuilder::new(),
            &format!("{UNIVERSAL_ROOT_RELATIVE}/omega"),
            "omega",
        );
        b.alias(".claude/skills", &format!("../{UNIVERSAL_ROOT_RELATIVE}"))
    }

    /// `oversize_skill_md`: a `SKILL.md` bigger than the 2 MiB read cap.
    fn oversize_skill_md() -> FixtureBuilder {
        let mut content = skill_md("giant");
        content.push_str(&"x".repeat(2 * 1024 * 1024 + 1));
        FixtureBuilder::new().file(".claude/skills/giant/SKILL.md", content.as_bytes())
    }

    /// Every named fixture, in the order documented above.
    pub fn all() -> Vec<(&'static str, FixtureBuilder)> {
        vec![
            ("empty_home", empty_home()),
            ("basic", basic()),
            ("broken_link", broken_link()),
            ("malformed_frontmatter", malformed_frontmatter()),
            (
                "manual_repairable_frontmatter",
                manual_repairable_frontmatter(),
            ),
            ("legacy_opencode", legacy_opencode()),
            ("parked", parked()),
            ("plugin_cache", plugin_cache()),
            ("project", project()),
            ("disabled", disabled()),
            ("whole_dir_link", whole_dir_link()),
            ("oversize_skill_md", oversize_skill_md()),
        ]
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::collections::BTreeSet;

        #[test]
        fn every_fixture_has_a_unique_name() {
            let names: BTreeSet<&str> = all().iter().map(|(name, _)| *name).collect();
            assert_eq!(names.len(), all().len());
            assert_eq!(names.len(), 12);
        }
    }
}

/// Scaffolding shared by the crate's golden-snapshot and agreement tests
/// (`tests/scan_golden.rs`, `tests/diagnosis_golden.rs`,
/// `tests/diagnose_next_action_agreement.rs`, `tests/repair_and_restore.rs`)
/// and by the desktop's parity tests. `materialized_ports` takes its
/// filesystem and lease provider from the caller rather than constructing
/// `skill-studio-host`'s `RealFs`/`FileLease` itself, since `skill-studio-
/// host` depends on this crate and a dependency the other way would cycle.
pub mod golden {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use crate::harness::HarnessCatalog;
    use crate::identity::CorrelationId;
    use crate::ports::{LeaseProvider, OpContext, Ports, ScopeFs};
    use crate::scope::{ProjectSelection, RuntimeScope};
    use crate::testing::{FakeClock, FakeIds, NoHistory, RecordingSink};

    /// A directory under `std::env::temp_dir()` unique to this process and
    /// `name`, so parallel test runs never collide.
    pub fn unique_temp_dir(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "skill-studio-core-golden-{name}-{}-{n}",
            std::process::id()
        ))
    }

    /// `Ports` wired to real adapters for a materialized fixture: caller
    /// supplies the filesystem and lease provider (typically `RealFs` and
    /// `FileLease` from `skill-studio-host`), everything else is a fake.
    pub fn materialized_ports(fs: Arc<dyn ScopeFs>, leases: Arc<dyn LeaseProvider>) -> Ports {
        Ports {
            fs,
            clock: Arc::new(FakeClock::at(0)),
            ids: Arc::new(FakeIds::default()),
            leases,
            history: Arc::new(NoHistory),
            sink: Arc::new(RecordingSink::default()),
            spawner: None,
            discovery: None,
            tools: None,
            catalog: Arc::new(HarnessCatalog::builtin()),
        }
    }

    /// A fixture-rooted scope with a budget generous enough for golden runs
    /// reading real disk under CI load. `project` alone registers `home/proj`
    /// as an explicit project, since fixture scans never run project
    /// discovery.
    pub fn scope_for(name: &str, home: &Path) -> RuntimeScope {
        let mut scope = RuntimeScope::fixture(home);
        scope.read_timeout_ms = 10_000;
        if name == "project" {
            scope.projects = ProjectSelection::Explicit {
                paths: vec![home.join("proj")],
            };
        }
        scope
    }

    /// An uncancellable context with a fresh correlation id.
    pub fn ctx() -> OpContext {
        OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()))
    }

    /// `Normalizer::apply` only rewrites a string that starts with the home
    /// root; `DeploymentDto.id`/`OwnerId` embed the home path percent-encoded
    /// (`/` -> `%2F`) partway through a larger opaque string, so it needs a
    /// substring rewrite too.
    pub fn replace_everywhere(value: &mut serde_json::Value, from: &str, to: &str) {
        match value {
            serde_json::Value::String(s) => {
                if s.contains(from) {
                    *s = s.replace(from, to);
                }
            }
            serde_json::Value::Array(items) => items
                .iter_mut()
                .for_each(|v| replace_everywhere(v, from, to)),
            serde_json::Value::Object(map) => map
                .values_mut()
                .for_each(|v| replace_everywhere(v, from, to)),
            _ => {}
        }
    }

    /// Blanks every `key`'s value anywhere in `value`, however deeply nested -
    /// used for fields that are fresh per run/materialization (a deployment's
    /// `modified_at` is the fixture's real file mtime, set the instant the
    /// fixture is materialized to a temp directory).
    pub fn blank_field(value: &mut serde_json::Value, key: &str) {
        match value {
            serde_json::Value::Object(map) => {
                if map.contains_key(key) {
                    map.insert(key.to_string(), serde_json::Value::String("-".into()));
                }
                for v in map.values_mut() {
                    blank_field(v, key);
                }
            }
            serde_json::Value::Array(items) => {
                items.iter_mut().for_each(|v| blank_field(v, key));
            }
            _ => {}
        }
    }

    /// Rewrites `home` (and its percent-encoded form) to `$HOME`/`%24HOME`
    /// and blanks `modified_at`, so a materialized run's JSON is comparable
    /// across machines and runs.
    pub fn normalize(home: &Path, mut value: serde_json::Value) -> serde_json::Value {
        replace_everywhere(&mut value, &home.to_string_lossy(), "$HOME");
        let encoded_home = home
            .to_string_lossy()
            .replace('%', "%25")
            .replace('/', "%2F");
        replace_everywhere(&mut value, &encoded_home, "%24HOME");
        blank_field(&mut value, "modified_at");
        value
    }

    /// Panics with a unified diff of `expected` vs `actual` if they differ.
    pub fn assert_json_eq(label: &str, expected: &serde_json::Value, actual: &serde_json::Value) {
        if expected == actual {
            return;
        }
        let expected_text = serde_json::to_string_pretty(expected).unwrap();
        let actual_text = serde_json::to_string_pretty(actual).unwrap();
        let diff = similar::TextDiff::from_lines(&expected_text, &actual_text)
            .unified_diff()
            .header("expected", "actual")
            .to_string();
        panic!("{label} mismatch:\n{diff}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_lease_reports_busy_with_the_root_path() {
        let lease = FakeLease::default();
        let key = LeaseKey {
            canonical_root: PathBuf::from("/h"),
        };
        lease.hold_exclusive(std::slice::from_ref(&key));
        let err = lease
            .acquire(
                std::slice::from_ref(&key),
                LeaseMode::Shared,
                Duration::ZERO,
            )
            .err()
            .unwrap();
        assert_eq!(err.code, ErrorCode::ScopeBusy);
        lease.release_all();
        assert!(lease
            .acquire(&[key], LeaseMode::Exclusive, Duration::ZERO)
            .is_ok());
    }

    #[test]
    fn fixture_fs_read_prefix_agrees_with_real_fs_truncation_semantics() {
        let fs = FixtureBuilder::new()
            .file("/h/big.txt", &[b'x'; 16])
            .file("/h/small.txt", b"hello")
            .build_fs();
        let (bytes, truncated) = fs.read_prefix(Path::new("/h/big.txt"), 4).unwrap();
        assert_eq!(bytes, vec![b'x'; 4]);
        assert!(truncated);
        let (bytes, truncated) = fs.read_prefix(Path::new("/h/small.txt"), 5).unwrap();
        assert_eq!(bytes, b"hello");
        assert!(!truncated);
    }

    #[test]
    fn normalizer_keeps_array_order() {
        let n = Normalizer::default().root("/h", "$HOME");
        let mut v = serde_json::json!({"paths": ["/h/b", "/h/a", "/other"]});
        n.apply(&mut v);
        assert_eq!(
            v["paths"],
            serde_json::json!(["$HOME/b", "$HOME/a", "/other"])
        );
    }

    /// A directory under `std::env::temp_dir()` this test process alone will
    /// use, so parallel test runs (and parallel invocations of this test
    /// across the crate) never collide.
    fn unique_temp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "skill-studio-core-{label}-{}-{n}",
            std::process::id()
        ))
    }

    #[test]
    #[cfg(unix)]
    fn materialize_round_trips_dirs_files_and_symlinks() {
        let dir = unique_temp_dir("materialize-round-trip");
        std::fs::create_dir_all(&dir).unwrap();

        let builder = FixtureBuilder::new()
            .dir("empty-dir")
            .file("a/b.txt", b"hello")
            .alias("link", "a/b.txt");
        builder.materialize(&dir).unwrap();

        assert!(dir.join("empty-dir").is_dir());
        assert_eq!(std::fs::read(dir.join("a/b.txt")).unwrap(), b"hello");
        let link_meta = std::fs::symlink_metadata(dir.join("link")).unwrap();
        assert!(link_meta.file_type().is_symlink());
        assert_eq!(
            std::fs::read_link(dir.join("link")).unwrap(),
            PathBuf::from("a/b.txt")
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn materialize_substitutes_the_fixture_home_token() {
        let dir = unique_temp_dir("materialize-home-token");
        std::fs::create_dir_all(&dir).unwrap();

        let builder = FixtureBuilder::new().file(
            "config.toml",
            format!("path = \"{FIXTURE_HOME_TOKEN}/skill/SKILL.md\"").as_bytes(),
        );
        builder.materialize(&dir).unwrap();

        let content = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert_eq!(
            content,
            format!("path = \"{}/skill/SKILL.md\"", dir.display())
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
