// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so the
// same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! `dotagents --project` keeps `agents.toml` and `agents.lock` in the project
//! root (`resolveScope`), while the skills themselves land in
//! `<project>/.agents/skills`. Ownership and the currency check must read the
//! root files, not `<project>/.agents/`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use skill_studio_core::dto::ScanRequest;
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::LifecycleOwnerKind;
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime, ScopeFs};
use skill_studio_core::skill_update_check::{
    CommitInfo, CommitLookup, Currency, PluginManifestLookup, SourceTreeLookup,
};
use skill_studio_core::testing::golden::{ctx, scope_for};
use skill_studio_core::testing::{FakeClock, FakeIds, FakeLease, FixtureBuilder, NoHistory};
use skill_studio_core::CoreError;

const HOME: &str = "/home";
const PROJECT_SKILL_DIR: &str = "/home/proj/.agents/skills/alpha";

const MANIFEST: &[u8] = b"[[skills]]\nname = \"alpha\"\nsource = \"o/r\"\n";

fn lock_pinned_at(commit: &str) -> Vec<u8> {
    format!(
        "[skills.alpha]\nsource = \"o/r\"\nresolved_path = \"skills/alpha\"\nresolved_commit = \"{commit}\"\n"
    )
    .into_bytes()
}

/// A project with one `alpha` skill folder, so only the ledger files differ
/// between the tests.
fn project_with_alpha() -> FixtureBuilder {
    FixtureBuilder::new().dir(PROJECT_SKILL_DIR).file(
        &format!("{PROJECT_SKILL_DIR}/SKILL.md"),
        b"---\nname: alpha\ndescription: fixture skill for the project scope ownership check.\n---\nBody.\n",
    )
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
        telemetry: Arc::new(skill_studio_core::ports::NoopTelemetry),
    };
    let scope = scope_for("project", Path::new(HOME));
    Runtime::new(&scope, ports).expect("runtime")
}

fn alpha_owner(fs: Arc<dyn ScopeFs>) -> LifecycleOwnerKind {
    let inventory = ops::scan(&runtime(fs), &ctx(), &ScanRequest::default()).expect("scan");
    let skill = inventory
        .skills
        .iter()
        .find(|skill| skill.name.0 == "alpha")
        .expect("the scan lists alpha");
    skill
        .deployments
        .first()
        .expect("alpha has a deployment")
        .owner_kind
}

/// Flow: `dotagents --project add` wrote `<project>/agents.toml` and
/// `<project>/agents.lock` for a skill in `<project>/.agents/skills`, then a
/// scan runs.
/// Expectation: the deployment is owned by dotagents.
/// A failure here means the scan still looks in `<project>/.agents/` for the
/// ledger, so every project dotagents skill reads as manual and gets no
/// update or remove path.
#[test]
fn scan_classifies_a_project_skill_declared_in_the_project_root_agents_toml_as_dotagents_or_names_the_owner_it_got(
) {
    let fs = project_with_alpha()
        .file("/home/proj/agents.toml", MANIFEST)
        .file("/home/proj/agents.lock", &lock_pinned_at("aaa"))
        .build_fs();
    assert_eq!(alpha_owner(Arc::new(fs)), LifecycleOwnerKind::Dotagents);
}

/// Flow: only a stale `<project>/.agents/agents.toml` names the skill; the
/// project root has no dotagents files.
/// Expectation: the skill is not dotagents-owned, since dotagents itself
/// never reads that file.
/// A failure here means the old, wrong location still claims ownership.
#[test]
fn scan_ignores_a_stale_agents_toml_inside_the_project_agents_folder_or_names_the_owner_it_got() {
    let fs = project_with_alpha()
        .file("/home/proj/.agents/agents.toml", MANIFEST)
        .file("/home/proj/.agents/agents.lock", &lock_pinned_at("aaa"))
        .build_fs();
    assert_ne!(alpha_owner(Arc::new(fs)), LifecycleOwnerKind::Dotagents);
}

/// Answers every commit lookup with one newer commit and records the ask.
struct NewerCommitLookup;

impl CommitLookup for NewerCommitLookup {
    fn latest_commit(&self, repo: &str, path: &str) -> Result<Option<CommitInfo>, CoreError> {
        assert_eq!((repo, path), ("o/r", "skills/alpha"));
        Ok(Some(CommitInfo {
            sha: "new".to_string(),
            committed_at: None,
        }))
    }
}

struct UnusedTreeLookup;

impl SourceTreeLookup for UnusedTreeLookup {
    fn tree_shas_at_head(&self, repo: &str) -> Result<HashMap<String, String>, CoreError> {
        panic!("no skills.sh deployment expected a tree lookup, got one for {repo}");
    }
}

struct UnusedPluginLookup;

impl PluginManifestLookup for UnusedPluginLookup {
    fn marketplace_version(&self, m: &str, p: &str) -> Result<Option<String>, CoreError> {
        panic!("no plugin deployment expected a lookup, got one for {m}/{p}");
    }
}

/// Flow: the project's root `agents.lock` pins `alpha` to `old`; the home
/// ledger also has an `alpha` row, pinned to `new` (already current). The
/// commit lookup says `new` is the latest.
/// Expectation: the project skill is `UpdateAvailable` from `old`.
/// A failure here means the check answered from the home ledger (`UpToDate`)
/// or from no ledger (`Unknown`) instead of the project's own.
#[test]
fn outdated_resolves_a_project_dotagents_skill_against_its_own_root_lock_or_reports_the_home_answer(
) {
    let fs = project_with_alpha()
        .file("/home/proj/agents.toml", MANIFEST)
        .file("/home/proj/agents.lock", &lock_pinned_at("old"))
        .file("/home/.agents/agents.toml", MANIFEST)
        .file("/home/.agents/agents.lock", &lock_pinned_at("new"))
        .build_fs();
    let rt = runtime(Arc::new(fs));
    let result = ops::outdated(
        &rt,
        &ctx(),
        &ScanRequest::default(),
        &UnusedTreeLookup,
        &NewerCommitLookup,
        &UnusedPluginLookup,
    )
    .expect("outdated");
    let record = result.get("alpha").expect("alpha is in the result");
    assert_eq!(
        record.currency,
        Currency::UpdateAvailable,
        "record: {record:?}"
    );
    assert_eq!(record.installed_commit.as_deref(), Some("old"));
}
