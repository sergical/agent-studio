// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so the
// same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A folder in `~/.agents/skills` that no ledger names is unmanaged: dotagents
//! prunes only folders its `agents.lock` names (`install/skills.js`) and
//! `sync` adopts every other folder as a `path:` entry without changing it.
//! The source badge must say so, and a per-skill link back into the universal
//! root must not claim a source of its own.

use std::path::Path;
use std::sync::Arc;

use skill_studio_core::dto::{DeploymentDto, InstalledSkillDto, ScanRequest};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{LifecycleOwnerKind, SourceKind};
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime, ScopeFs};
use skill_studio_core::testing::golden::{ctx, scope_for};
use skill_studio_core::testing::{FakeClock, FakeIds, FakeLease, FixtureBuilder, NoHistory};

const HOME: &str = "/home";

const OTHER_MANIFEST: &[u8] =
    b"[[skills]]\nname = \"other\"\nsource = \"o/other\"\npath = \"skills/other\"\n";
const OTHER_LOCK: &[u8] =
    b"[skills.other]\nsource = \"o/other\"\nresolved_path = \"skills/other\"\n";
const PLAIN_LOCK: &[u8] =
    b"[skills.plain]\nsource = \"o/plain\"\nresolved_path = \"skills/plain\"\n";

fn skill_md(name: &str) -> Vec<u8> {
    format!(
        "---\nname: {name}\ndescription: fixture skill for the source kind check.\n---\nBody.\n"
    )
    .into_bytes()
}

fn universal_skill(name: &str) -> FixtureBuilder {
    let dir = format!("{HOME}/.agents/skills/{name}");
    FixtureBuilder::new()
        .dir(&dir)
        .file(&format!("{dir}/SKILL.md"), &skill_md(name))
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
    let scope = scope_for("unmanaged-universal-source-kind", Path::new(HOME));
    Runtime::new(&scope, ports).expect("runtime")
}

fn scan_skill(fs: impl ScopeFs + 'static, name: &str) -> InstalledSkillDto {
    let inventory =
        ops::scan(&runtime(Arc::new(fs)), &ctx(), &ScanRequest::default()).expect("scan");
    inventory
        .skills
        .into_iter()
        .find(|skill| skill.name.0 == name)
        .expect("the scan lists the skill")
}

/// The skill-level badge the desktop assembly shows: the lowest kind over all
/// deployments (`skill_assembly.rs`).
fn skill_source_kind(skill: &InstalledSkillDto) -> SourceKind {
    skill
        .deployments
        .iter()
        .map(|d| d.source_kind)
        .min()
        .expect("the skill has a deployment")
}

fn deployment_under<'a>(skill: &'a InstalledSkillDto, prefix: &str) -> &'a DeploymentDto {
    skill
        .deployments
        .iter()
        .find(|d| d.path.starts_with(prefix))
        .expect("the skill has a deployment under the prefix")
}

/// Flow: a global home has `agents.toml` and `agents.lock` that name only
/// `other`, and a plain folder `~/.agents/skills/plain` that no ledger names.
/// Expectation: the universal deployment is `Manual` and so is the skill's
/// source kind.
/// A failure here means an unnamed folder beside dotagents files still reads
/// as dotagents-managed, so editing it forks first and the fork refuses it.
#[test]
fn a_folder_no_dotagents_ledger_names_is_manual_or_it_reads_as_dotagents() {
    let fs = universal_skill("plain")
        .file("/home/.agents/agents.toml", OTHER_MANIFEST)
        .file("/home/.agents/agents.lock", OTHER_LOCK)
        .build_fs();
    let skill = scan_skill(fs, "plain");
    let deployment = deployment_under(&skill, "/home/.agents/skills");
    assert_eq!(deployment.owner_kind, LifecycleOwnerKind::Manual);
    assert_eq!(skill_source_kind(&skill), SourceKind::Manual);
}

/// Flow: the same home, but `agents.lock` names `plain`.
/// Expectation: the deployment stays dotagents-owned and the skill's source
/// kind stays `Dotagents`.
/// A failure here means the ledger row no longer claims its own folder.
#[test]
fn a_folder_the_dotagents_lock_names_stays_dotagents() {
    let fs = universal_skill("plain")
        .file("/home/.agents/agents.toml", OTHER_MANIFEST)
        .file("/home/.agents/agents.lock", PLAIN_LOCK)
        .build_fs();
    let skill = scan_skill(fs, "plain");
    let deployment = deployment_under(&skill, "/home/.agents/skills");
    assert!(matches!(
        deployment.owner_kind,
        LifecycleOwnerKind::Dotagents | LifecycleOwnerKind::WildcardDotagents
    ));
    assert_eq!(skill_source_kind(&skill), SourceKind::Dotagents);
}

/// Flow: a manual folder `~/.agents/skills/real`, no dotagents files at
/// all, and a symlink `~/.agents/skills/alias` pointing at it (the only link
/// shape `classify_owner` treats as ambiguous: a per-harness link is `Manual`
/// before the ledger rules run).
/// Expectation: the link deployment keeps owner kind `Ambiguous` (the link
/// end owns nothing) but its source kind is `Manual`, and so is the skill's.
/// A failure here means the link end supplies a `Dotagents` source of its
/// own and drags the whole skill's badge to dotagents.
#[test]
fn a_link_into_a_manual_folder_stays_ambiguous_but_never_reads_as_dotagents() {
    let fs = universal_skill("real")
        .alias("/home/.agents/skills/alias", "/home/.agents/skills/real")
        .build_fs();
    let skill = scan_skill(fs, "alias");
    let link = deployment_under(&skill, "/home/.agents/skills/alias");
    assert_eq!(link.owner_kind, LifecycleOwnerKind::Ambiguous);
    assert_eq!(link.source_kind, SourceKind::Manual);
    assert_eq!(skill_source_kind(&skill), SourceKind::Manual);
}
