// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so the
// same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! `dotagents sync` adopts an undeclared folder as `source = "path:skills/<name>"`.
//! That row only says "the folder is the only copy", so it must not claim
//! ownership over a skill that skills.sh also claims, and on its own it has
//! nothing upstream to update from.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use skill_studio_core::dto::ScanRequest;
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::UNIVERSAL_ROOT_RELATIVE;
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime, ScopeFs};
use skill_studio_core::skill_update_check::{
    CommitInfo, CommitLookup, Currency, OutdatedRecord, PluginManifestLookup, SourceTreeLookup,
};
use skill_studio_core::testing::golden::{ctx, scope_for};
use skill_studio_core::testing::{FakeClock, FakeIds, FakeLease, FixtureBuilder, NoHistory};
use skill_studio_core::CoreError;

const HOME: &str = "/home";

/// Panics if reached: a `path:` row has no upstream repo to ask.
struct UnusedCommitLookup;

impl CommitLookup for UnusedCommitLookup {
    fn latest_commit(&self, repo: &str, _path: &str) -> Result<Option<CommitInfo>, CoreError> {
        panic!("a path: entry must not reach the commit lookup, got one for {repo}");
    }
}

/// Answers the skills.sh tree lookup with one moved folder hash.
struct MovedTreeLookup;

impl SourceTreeLookup for MovedTreeLookup {
    fn tree_shas_at_head(&self, _repo: &str) -> Result<HashMap<String, String>, CoreError> {
        Ok(HashMap::from([(
            "skills/find-bugs".to_string(),
            "new-hash".to_string(),
        )]))
    }
}

struct PanicTreeLookup;

impl SourceTreeLookup for PanicTreeLookup {
    fn tree_shas_at_head(&self, repo: &str) -> Result<HashMap<String, String>, CoreError> {
        panic!("a path: entry must not reach the tree lookup, got one for {repo}");
    }
}

struct UnusedPluginLookup;

impl PluginManifestLookup for UnusedPluginLookup {
    fn marketplace_version(&self, m: &str, p: &str) -> Result<Option<String>, CoreError> {
        panic!("no plugin deployment expected a lookup, got one for {m}/{p}");
    }
}

fn runtime(fs: Arc<dyn ScopeFs>) -> Runtime {
    let ports = Ports {
        fs,
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FakeLease::default()),
        history: Arc::new(NoHistory),
        sink: Arc::new(skill_studio_core::testing::RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
        telemetry: std::sync::Arc::new(skill_studio_core::ports::NoopTelemetry),
    };
    let scope = scope_for("dotagents-path-entry-ownership", Path::new(HOME));
    Runtime::new(&scope, ports).expect("runtime")
}

const PATH_LOCK: &[u8] = br#"
[skills.find-bugs]
source = "path:skills/find-bugs"
resolved_path = "skills/find-bugs"
"#;

const SKILLS_SH_LOCK: &[u8] = br#"{"version":3,"skills":{"find-bugs":{"source":"getsentry/find-bugs","sourceType":"github","sourceUrl":"https://github.com/getsentry/find-bugs","skillPath":"skills/find-bugs/SKILL.md","skillFolderHash":"old-hash","installedAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z"}}}"#;

fn fixture(with_skills_sh_row: bool) -> Arc<dyn ScopeFs> {
    let dir = format!("{HOME}/{UNIVERSAL_ROOT_RELATIVE}/find-bugs");
    let mut b = FixtureBuilder::new()
        .dir(&dir)
        .file(
            &format!("{dir}/SKILL.md"),
            b"---\nname: find-bugs\ndescription: fixture skill for the path entry ownership check.\n---\nBody.\n",
        )
        .file(&format!("{HOME}/.agents/agents.lock"), PATH_LOCK);
    if with_skills_sh_row {
        b = b.file(&format!("{HOME}/.agents/.skill-lock.json"), SKILLS_SH_LOCK);
    }
    Arc::new(b.build_fs())
}

fn find_bugs_record(fs: Arc<dyn ScopeFs>, tree: &dyn SourceTreeLookup) -> OutdatedRecord {
    let rt = runtime(fs);
    let req = ScanRequest {
        skills: Vec::new(),
        timings: false,
    };
    let mut result = ops::outdated(
        &rt,
        &ctx(),
        &req,
        tree,
        &UnusedCommitLookup,
        &UnusedPluginLookup,
    )
    .expect("outdated");
    result
        .remove("find-bugs")
        .expect("find-bugs is in the result")
}

/// Flow: `dotagents sync` adopted a folder (`path:` row in `agents.lock`)
/// that nothing else claims, then `ops::outdated` runs.
/// Expectation: `Currency::NotTracked`, with no upstream lookup at all.
/// A failure here means a local folder is offered an update it cannot
/// have, or the currency check calls out for a repo that does not exist.
#[test]
fn a_lone_path_entry_is_not_tracked_and_never_asks_upstream_or_offers_an_update() {
    let record = find_bugs_record(fixture(false), &PanicTreeLookup);
    assert_eq!(record.currency, Currency::NotTracked);
}

/// Flow: the same folder is also a skills.sh install (`.skill-lock.json`
/// has a row), and the adopted `path:` row sits beside it.
/// Expectation: skills.sh owns the skill, so the check reads the skills.sh
/// hash and reports `UpdateAvailable` for the moved folder hash.
/// A failure here means the `path:` row still claims the skill (ownership
/// reads `Ambiguous`/`Dotagents`), so a skills.sh update never shows.
#[test]
fn a_path_entry_beside_a_skills_sh_row_leaves_the_skill_to_skills_sh_or_hides_its_update() {
    let record = find_bugs_record(fixture(true), &MovedTreeLookup);
    assert_eq!(
        record.currency,
        Currency::UpdateAvailable,
        "record: {record:?}"
    );
    assert_eq!(record.installed_commit.as_deref(), Some("old-hash"));
}
