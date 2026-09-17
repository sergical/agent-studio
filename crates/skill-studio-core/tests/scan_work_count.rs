//! Given a 400-skill estate across the harness roots with 20% linked from
//! the shared root, when the app rescans, the scan reads each `SKILL.md`
//! once and lists each directory once; fails if either count grows with
//! links or harnesses.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use skill_studio_core::bench_estate::estate;
use skill_studio_core::dto::ScanRequest;
use skill_studio_core::harness::{HarnessCatalog, RootRole, ScopeLevel};
use skill_studio_core::identity::{MOVE_ASIDE_DIR_NAME, PARKED_ROOT_RELATIVE};
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
/// calls whose path ends in `SKILL.md`. `read_dir` calls are recorded in
/// full, not just counted, so a test can assert no path was listed twice.
#[derive(Default)]
struct CountingFs {
    inner: Option<Arc<dyn ScopeFs>>,
    canonicalize: AtomicU64,
    symlink_metadata: AtomicU64,
    read_link: AtomicU64,
    read_dir_calls: Mutex<Vec<PathBuf>>,
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
        self.read_dir_calls
            .lock()
            .expect("read_dir_calls lock")
            .push(path.to_path_buf());
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
    fn fsops_device_inode(&self, path: &Path) -> std::io::Result<(u64, u64)> {
        self.inner().fsops_device_inode(path)
    }
    fn fsops_fsync_file(&self, path: &Path) -> std::io::Result<()> {
        self.inner().fsops_fsync_file(path)
    }
    fn fsops_fsync_dir(&self, path: &Path) -> std::io::Result<()> {
        self.inner().fsops_fsync_dir(path)
    }
    fn fsops_create_dir(&self, path: &Path) -> std::io::Result<()> {
        self.inner().fsops_create_dir(path)
    }
    fn fsops_write_new_file(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        self.inner().fsops_write_new_file(path, bytes)
    }
    fn fsops_rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        self.inner().fsops_rename(from, to)
    }
    fn fsops_symlink(&self, target: &Path, link: &Path) -> std::io::Result<()> {
        self.inner().fsops_symlink(target, link)
    }
    fn fsops_remove_dir(&self, path: &Path) -> std::io::Result<()> {
        self.inner().fsops_remove_dir(path)
    }
    fn fsops_remove_file(&self, path: &Path) -> std::io::Result<()> {
        self.inner().fsops_remove_file(path)
    }
    fn fsops_exchange(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        self.inner().fsops_exchange(a, b)
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

/// Every directory a correct scan must call `read_dir` on for this estate, by
/// canonical identity, derived from the catalog's own root declarations and
/// a plain (uncounted) walk of the materialized fixture: each harness root,
/// tracked-project root, and parked/move-aside root that actually exists on
/// disk, plus each directory entry those roots hold, resolved to its
/// canonical path so a symlinked skill's one real directory appears once,
/// however many roots link it. A scan's actual `read_dir` calls must be
/// canonicalized the same way before comparing against this set, since a
/// deduplicated symlinked skill is read through whichever root's literal
/// entry path the scan reaches it by first, not through its canonical path.
fn expected_dirs(home: &Path, project_dirs: &[PathBuf]) -> BTreeSet<PathBuf> {
    let catalog = HarnessCatalog::builtin();
    let mut candidate_roots: BTreeSet<PathBuf> = BTreeSet::new();
    candidate_roots.insert(home.join(PARKED_ROOT_RELATIVE));
    for facts in &catalog.facts {
        for root in &facts.roots {
            let tracked = matches!(
                root.role,
                RootRole::Own | RootRole::Universal | RootRole::Legacy | RootRole::PluginCache
            );
            if !tracked {
                continue;
            }
            match root.level {
                ScopeLevel::Global => {
                    candidate_roots.insert(home.join(&root.relative_path));
                }
                ScopeLevel::Project => {
                    for project in project_dirs {
                        candidate_roots.insert(home.join(project).join(&root.relative_path));
                    }
                }
            }
        }
    }

    // A root only gets `read_dir`'d when it (or its move-aside sibling)
    // really exists; `Path::exists` follows a symlinked root the same way
    // `read_dir` would, and reports false for a broken one the way a
    // resolved-not-found root would never be listed either.
    let mut existing_roots: Vec<PathBuf> = Vec::new();
    for root in &candidate_roots {
        if root.exists() {
            existing_roots.push(root.clone());
        }
        let move_aside = root.join(MOVE_ASIDE_DIR_NAME);
        if move_aside.exists() {
            existing_roots.push(move_aside);
        }
    }

    let mut expected = BTreeSet::new();
    for root in &existing_roots {
        expected.insert(root.canonicalize().unwrap_or_else(|_| root.clone()));
        let Ok(entries) = root.read_dir() else {
            continue;
        };
        for entry in entries.flatten() {
            let is_dir = entry
                .file_type()
                .map(|t| t.is_dir() || t.is_symlink())
                .unwrap_or(false);
            if !is_dir {
                continue;
            }
            // A broken link resolves to neither a file nor a directory;
            // the scan never lists it, so it is not added here either.
            if let Ok(canonical) = entry.path().canonicalize() {
                if canonical.is_dir() {
                    expected.insert(canonical);
                }
            }
        }
    }
    expected
}

/// Given a 400-skill estate (63% global, 20% universal/linked, four
/// harnesses), when `ops::scan` runs once, it reads each skill's `SKILL.md`
/// exactly once (a linked skill included, since it has exactly one real
/// directory) and calls `read_dir` on exactly the set of directories the
/// fixture itself holds, each exactly once.
#[test]
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

    let read_dir_calls = counting_fs
        .read_dir_calls
        .lock()
        .expect("read_dir_calls lock")
        .clone();

    let mut seen: BTreeSet<&PathBuf> = BTreeSet::new();
    for path in &read_dir_calls {
        assert!(
            seen.insert(path),
            "expected every directory to be listed once; {} was listed twice",
            path.display()
        );
    }

    // Compare by canonical identity: a deduplicated symlinked skill is
    // listed through whichever root's literal entry path the scan reaches
    // it by first, which `expected_dirs` cannot predict, but both sides
    // must still resolve to the same real directories.
    let read_dir_paths: BTreeSet<PathBuf> = read_dir_calls
        .iter()
        .map(|path| path.canonicalize().unwrap_or_else(|_| path.clone()))
        .collect();

    let expected = expected_dirs(&home, &generated.project_dirs);
    if read_dir_paths != expected {
        let missing: Vec<&PathBuf> = expected.difference(&read_dir_paths).collect();
        let extra: Vec<&PathBuf> = read_dir_paths.difference(&expected).collect();
        panic!(
            "scan's read_dir calls don't match the fixture's directories: \
             missing (expected but not listed) = {missing:#?}, \
             extra (listed but not expected) = {extra:#?}"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}
