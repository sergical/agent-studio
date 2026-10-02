// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Given a home with many skills, when a write begins for one named skill,
//! the lease-time scan reads only that skill's `SKILL.md`; fails if `begin`
//! goes back to scanning every skill on every write.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use skill_studio_core::dto::{ParkRequest, ScanRequest};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{DeploymentId, RootKind, SkillName};
use skill_studio_core::ops;
use skill_studio_core::ports::{
    DirEntryFacts, ExclusiveGuard, FileFacts, MutationSession, Ports, Runtime, ScopeFs, ScopedPath,
};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

const SKILL_COUNT: usize = 25;

/// Wraps another [`ScopeFs`], recording the path of every `SKILL.md` read so
/// a test can name which skills a write looked at.
struct SkillMdReadLog {
    inner: Arc<dyn ScopeFs>,
    reads: Mutex<Vec<PathBuf>>,
}

impl SkillMdReadLog {
    fn wrap(inner: Arc<dyn ScopeFs>) -> Self {
        Self {
            inner,
            reads: Mutex::new(Vec::new()),
        }
    }

    fn note(&self, path: &Path) {
        if path.ends_with("SKILL.md") {
            self.reads.lock().unwrap().push(path.to_path_buf());
        }
    }

    fn clear(&self) {
        self.reads.lock().unwrap().clear();
    }

    /// The folder names whose `SKILL.md` was read, sorted and deduplicated.
    fn skills_read(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .reads
            .lock()
            .unwrap()
            .iter()
            .filter_map(|p| p.parent()?.file_name()?.to_str().map(str::to_string))
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

impl ScopeFs for SkillMdReadLog {
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.inner.canonicalize(path)
    }
    fn symlink_metadata(&self, path: &Path) -> std::io::Result<FileFacts> {
        self.inner.symlink_metadata(path)
    }
    fn read_link(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.inner.read_link(path)
    }
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<DirEntryFacts>> {
        self.inner.read_dir(path)
    }
    fn ancestor_holds(&self, start: &Path, name: &str) -> std::io::Result<bool> {
        self.inner.ancestor_holds(start, name)
    }
    fn read_capped(&self, path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        self.note(path);
        self.inner.read_capped(path, max_bytes)
    }
    fn read_prefix(&self, path: &Path, limit: u64) -> std::io::Result<(Vec<u8>, bool)> {
        self.note(path);
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
    fn fsops_device_inode(&self, path: &Path) -> std::io::Result<(u64, u64)> {
        self.inner.fsops_device_inode(path)
    }
    fn fsops_fsync_file(&self, path: &Path) -> std::io::Result<()> {
        self.inner.fsops_fsync_file(path)
    }
    fn fsops_fsync_dir(&self, path: &Path) -> std::io::Result<()> {
        self.inner.fsops_fsync_dir(path)
    }
    fn fsops_create_dir(&self, path: &Path) -> std::io::Result<()> {
        self.inner.fsops_create_dir(path)
    }
    fn fsops_write_new_file(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        self.inner.fsops_write_new_file(path, bytes)
    }
    fn fsops_write_new_file_with_mode(
        &self,
        path: &Path,
        bytes: &[u8],
        mode: u32,
    ) -> std::io::Result<()> {
        self.inner.fsops_write_new_file_with_mode(path, bytes, mode)
    }
    fn fsops_rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        self.inner.fsops_rename(from, to)
    }
    fn fsops_symlink(&self, target: &Path, link: &Path) -> std::io::Result<()> {
        self.inner.fsops_symlink(target, link)
    }
    fn fsops_remove_dir(&self, path: &Path) -> std::io::Result<()> {
        self.inner.fsops_remove_dir(path)
    }
    fn fsops_remove_file(&self, path: &Path) -> std::io::Result<()> {
        self.inner.fsops_remove_file(path)
    }
    fn fsops_exchange(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        self.inner.fsops_exchange(a, b)
    }
}

fn skill_name(index: usize) -> String {
    format!("skill-{index:02}")
}

/// `SKILL_COUNT` universal skills, each linked from Claude Code's per-skill
/// root, so a full scan has many folders and links to read.
fn crowded_home(home: &Path) {
    let claude_skills = home.join(".claude/skills");
    std::fs::create_dir_all(&claude_skills).unwrap();
    for index in 0..SKILL_COUNT {
        let name = skill_name(index);
        let dir = home.join(".agents/skills").join(&name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: skill number {index}\n---\nBody.\n"),
        )
        .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dir, claude_skills.join(&name)).unwrap();
    }
}

fn runtime_over(home: &Path, fs: Arc<dyn ScopeFs>) -> Runtime {
    let ports = Ports {
        fs,
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history: Arc::new(SqliteHistoryOpener::new(
            home.join(".history/events.sqlite3"),
        )),
        sink: Arc::new(RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
        telemetry: Arc::new(skill_studio_core::ports::NoopTelemetry),
    };
    Runtime::new(&RuntimeScope::fixture(home), ports).unwrap()
}

fn universal_deployment_id(rt: &Runtime, name: &str) -> DeploymentId {
    let inventory = ops::scan(rt, &ctx(), &ScanRequest::default()).unwrap();
    inventory
        .skills
        .iter()
        .find(|s| s.name.0 == name)
        .unwrap()
        .deployments
        .iter()
        .find(|d| d.root.kind == RootKind::Universal)
        .unwrap()
        .id
        .clone()
}

/// Given a home with 25 skills, when `park` runs on one deployment, expect
/// only that skill's `SKILL.md` to be read. Fails if the write's lease-time
/// scan walks every skill again, which cost ~2 s on a 378-skill machine.
#[test]
fn park_of_one_deployment_reads_only_that_skills_skill_md() {
    let home = unique_temp_dir("scoped_park");
    crowded_home(&home);
    let log = Arc::new(SkillMdReadLog::wrap(Arc::new(RealFs::new())));
    let rt = runtime_over(&home, log.clone());
    let deployment_id = universal_deployment_id(&rt, "skill-07");
    log.clear();

    ops::park(&rt, &ctx(), &ParkRequest { deployment_id }).unwrap();

    assert_eq!(
        log.skills_read(),
        vec!["skill-07".to_string()],
        "park must not read any other skill's SKILL.md"
    );
    std::fs::remove_dir_all(&home).ok();
}

/// Given a home with 25 skills, when a session begins for one skill name,
/// expect `fresh` to hold that skill and no other. Fails if the scan ignores
/// the names it was given.
#[test]
fn begin_for_a_named_skill_holds_only_that_skill_in_fresh() {
    let home = unique_temp_dir("scoped_begin");
    crowded_home(&home);
    let log = Arc::new(SkillMdReadLog::wrap(Arc::new(RealFs::new())));
    let rt = runtime_over(&home, log.clone());

    let session = MutationSession::begin_for(&rt, &ctx(), &[SkillName(skill_name(3))]).unwrap();

    let held: Vec<&str> = session
        .fresh
        .skills
        .iter()
        .map(|s| s.name.0.as_str())
        .collect();
    assert_eq!(held, vec!["skill-03"]);
    session.finish(&rt, &ctx());
    std::fs::remove_dir_all(&home).ok();
}

/// Given the same home, when a session begins with no names, expect every
/// skill in `fresh`. Fails if the fallback for a caller that cannot name its
/// skills stops scanning everything.
#[test]
fn begin_with_no_skill_names_scans_every_skill() {
    let home = unique_temp_dir("scoped_fallback_unnamed");
    crowded_home(&home);
    let rt = runtime_over(&home, Arc::new(RealFs::new()));

    let session = MutationSession::begin(&rt, &ctx()).unwrap();

    assert_eq!(session.fresh.skills.len(), SKILL_COUNT);
    session.finish(&rt, &ctx());
    std::fs::remove_dir_all(&home).ok();
}

/// Given a history row a crash left pending, when a session begins for one
/// skill, expect the recovery to force a full scan: the repair may have
/// touched skills the caller did not name. Fails if a named begin keeps the
/// narrow scan after repairing something.
#[test]
fn begin_for_a_named_skill_scans_every_skill_after_recovering_an_interrupted_row() {
    let home = unique_temp_dir("scoped_fallback_recovery");
    crowded_home(&home);
    let rt = runtime_over(&home, Arc::new(RealFs::new()));
    {
        let mut store = rt
            .ports
            .history
            .open(
                &rt.scope,
                skill_studio_core::ports::HistoryAccess::ReadWrite,
            )
            .unwrap()
            .unwrap();
        let guard =
            skill_studio_core::ports::acquire_exclusive(rt.ports.leases.as_ref(), &rt.scope)
                .unwrap();
        let id = rt.ports.ids.next_event_id();
        store
            .record(
                &guard,
                &id,
                &skill_studio_core::events::EventDraft {
                    kind: skill_studio_core::events::EventKind::RepairSkillFrontmatter,
                    skill: SkillName(skill_name(1)),
                    harness: None,
                    scope: None,
                    project_path: None,
                    payload: serde_json::json!({}),
                    inverse: None,
                    backup_dir: None,
                },
            )
            .unwrap();
    }

    let session = MutationSession::begin_for(&rt, &ctx(), &[SkillName(skill_name(3))]).unwrap();

    assert_eq!(session.fresh.skills.len(), SKILL_COUNT);
    session.finish(&rt, &ctx());
    std::fs::remove_dir_all(&home).ok();
}

/// Given every deployment the scan reports, expect the skill name read from
/// its id to equal the skill it belongs to. Fails if the id layout and
/// `DeploymentId::skill_name` drift apart; a scoped write would then scan the
/// wrong skill and report its own target as missing.
#[test]
fn deployment_id_names_the_skill_it_belongs_to() {
    let home = unique_temp_dir("scoped_id_name");
    crowded_home(&home);
    let rt = runtime_over(&home, Arc::new(RealFs::new()));

    let inventory = ops::scan(&rt, &ctx(), &ScanRequest::default()).unwrap();

    let mut checked = 0;
    for skill in &inventory.skills {
        for deployment in &skill.deployments {
            assert_eq!(deployment.id.skill_name(), Some(skill.name.clone()));
            checked += 1;
        }
    }
    assert!(
        checked >= SKILL_COUNT * 2,
        "expected linked deployments too"
    );
    std::fs::remove_dir_all(&home).ok();
}
