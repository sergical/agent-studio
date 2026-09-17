// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Given a 400-skill estate across the harness roots with 20% linked from
//! the shared root, when the app rescans, the scan reads each `SKILL.md`
//! once and lists each directory once; fails if either count grows with
//! links or harnesses.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use skill_studio_core::bench_estate::estate;
use skill_studio_core::dto::ScanRequest;
use skill_studio_core::harness::{HarnessCatalog, RootRole};
use skill_studio_core::ops::scan;
use skill_studio_core::ports::{
    DirEntryFacts, ExclusiveGuard, FileFacts, Ports, Runtime, ScopeFs, ScopedPath,
};
use skill_studio_core::scope::{ProjectSelection, RuntimeScope};
use skill_studio_core::testing::golden::{ctx, materialized_ports, unique_temp_dir};

use skill_studio_host::{FileLease, RealFs};

/// Wraps another [`ScopeFs`], counting every call per method so a test can
/// assert on the number of filesystem operations a scan performed, not just
/// its result. `skill_md_reads` counts only `read_capped`/`read_prefix`
/// calls whose path ends in `SKILL.md`.
#[derive(Default)]
struct CountingFs {
    inner: Option<Arc<dyn ScopeFs>>,
    canonicalize: AtomicU64,
    symlink_metadata: AtomicU64,
    read_link: AtomicU64,
    read_dir: AtomicU64,
    ancestor_holds: AtomicU64,
    read_capped: AtomicU64,
    read_prefix: AtomicU64,
    write_atomic: AtomicU64,
    rename: AtomicU64,
    skill_md_reads: AtomicU64,
}

impl CountingFs {
    fn wrap(inner: Arc<dyn ScopeFs>) -> Self {
        CountingFs {
            inner: Some(inner),
            ..Default::default()
        }
    }

    fn inner(&self) -> &dyn ScopeFs {
        self.inner.as_deref().expect("CountingFs::wrap sets inner")
    }

    fn count(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::SeqCst);
    }

    fn get(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::SeqCst)
    }
}

impl ScopeFs for CountingFs {
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        Self::count(&self.canonicalize);
        self.inner().canonicalize(path)
    }
    fn symlink_metadata(&self, path: &Path) -> std::io::Result<FileFacts> {
        Self::count(&self.symlink_metadata);
        self.inner().symlink_metadata(path)
    }
    fn read_link(&self, path: &Path) -> std::io::Result<PathBuf> {
        Self::count(&self.read_link);
        self.inner().read_link(path)
    }
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<DirEntryFacts>> {
        Self::count(&self.read_dir);
        self.inner().read_dir(path)
    }
    fn ancestor_holds(&self, start: &Path, name: &str) -> std::io::Result<bool> {
        Self::count(&self.ancestor_holds);
        self.inner().ancestor_holds(start, name)
    }
    fn read_capped(&self, path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        Self::count(&self.read_capped);
        if path.ends_with("SKILL.md") {
            Self::count(&self.skill_md_reads);
        }
        self.inner().read_capped(path, max_bytes)
    }
    fn read_prefix(&self, path: &Path, limit: u64) -> std::io::Result<(Vec<u8>, bool)> {
        Self::count(&self.read_prefix);
        if path.ends_with("SKILL.md") {
            Self::count(&self.skill_md_reads);
        }
        self.inner().read_prefix(path, limit)
    }
    fn write_atomic(
        &self,
        guard: &ExclusiveGuard,
        path: &ScopedPath,
        bytes: &[u8],
    ) -> std::io::Result<()> {
        Self::count(&self.write_atomic);
        self.inner().write_atomic(guard, path, bytes)
    }
    fn rename(
        &self,
        guard: &ExclusiveGuard,
        from: &ScopedPath,
        to: &ScopedPath,
    ) -> std::io::Result<()> {
        Self::count(&self.rename);
        self.inner().rename(guard, from, to)
    }
    fn remove_file(&self, guard: &ExclusiveGuard, path: &ScopedPath) -> std::io::Result<()> {
        self.inner().remove_file(guard, path)
    }
    fn create_dir_all(&self, guard: &ExclusiveGuard, path: &ScopedPath) -> std::io::Result<()> {
        self.inner().create_dir_all(guard, path)
    }
    fn symlink(
        &self,
        guard: &ExclusiveGuard,
        target: &ScopedPath,
        link: &ScopedPath,
    ) -> std::io::Result<()> {
        self.inner().symlink(guard, target, link)
    }
}

/// A fixture-rooted scope tracking `project_dirs` as explicit projects, the
/// same shape `benches/scan.rs`'s `scope_for` uses, with the same generous
/// read budget `tests/scan_timing.rs` and `tests/repair_and_restore.rs` run
/// under so a real-disk 400-skill scan never trips the budget mid-test.
fn scope_for(home: &Path, project_dirs: &[PathBuf]) -> RuntimeScope {
    let mut scope = RuntimeScope::fixture(home);
    scope.read_timeout_ms = 10_000;
    scope.projects = ProjectSelection::Explicit {
        paths: project_dirs.iter().map(|d| home.join(d)).collect(),
    };
    scope
}

/// Given a 400-skill estate (63% global, 20% universal/linked, four
/// harnesses), when `ops::scan` runs once, it reads each skill's `SKILL.md`
/// exactly once (a linked skill included, since it has exactly one real
/// directory) and lists no more directories than the estate's known roots,
/// tracked projects, skill folders, and plugin caches account for.
#[test]
#[ignore = "fails against the current scan: measured skill_md_reads=1440 vs \
            expected 400 (compute_content_facts re-reads SKILL.md for \
            content_fingerprint and content_hash_from_walk on top of \
            read_skill_md's own read_prefix); measured read_dir=1284 vs a \
            bound of 428 (7 estate roots + 19 tracked projects + 400 skill \
            folders + 2 plugin cache roots)"]
fn scan_reads_each_skill_md_once_and_lists_each_directory_once() {
    let generated = estate(400, 1);
    let dir = unique_temp_dir("scan-work-count");
    std::fs::create_dir_all(&dir).expect("create scan-work-count home");
    let home = dir.canonicalize().expect("canonicalize home");
    generated
        .builder
        .materialize(&home)
        .expect("materialize 400-skill estate");

    let counting_fs = Arc::new(CountingFs::wrap(Arc::new(RealFs::new())));
    let ports: Ports = materialized_ports(
        counting_fs.clone(),
        Arc::new(FileLease::new(home.join(".leases"))),
    );
    let scope = scope_for(&home, &generated.project_dirs);
    let rt = Runtime::new(&scope, ports).expect("runtime");

    scan(&rt, &ctx(), &ScanRequest::default()).expect("scan");

    let skill_md_reads = CountingFs::get(&counting_fs.skill_md_reads);
    assert_eq!(
        skill_md_reads, generated.stats.skill_count as u64,
        "expected one SKILL.md read per distinct skill folder ({}); a linked \
         skill must be read once, not once per link",
        generated.stats.skill_count
    );

    let catalog = HarnessCatalog::builtin();
    let plugin_cache_roots = catalog
        .facts
        .iter()
        .flat_map(|facts| facts.roots.iter())
        .filter(|root| root.role == RootRole::PluginCache)
        .count() as u64;
    let read_dir_bound = generated.root_count() as u64
        + generated.project_dirs.len() as u64
        + generated.stats.skill_count as u64
        + plugin_cache_roots;
    let read_dir_calls = CountingFs::get(&counting_fs.read_dir);
    assert!(
        read_dir_calls <= read_dir_bound,
        "expected at most {read_dir_bound} read_dir calls (harness roots \
         {} + tracked projects {} + skill folders {} + plugin cache roots \
         {plugin_cache_roots}), got {read_dir_calls}",
        generated.root_count(),
        generated.project_dirs.len(),
        generated.stats.skill_count,
    );

    std::fs::remove_dir_all(&dir).ok();
}
