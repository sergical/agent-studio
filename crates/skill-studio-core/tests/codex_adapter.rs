//! Real-disk integration tests for the Codex adapter: the `[[skills.config]]`
//! disable row, `ops::park`'s fix for the stale-row bug it leaves behind, and
//! `CODEX_HOME` support. Like `park_and_unpark.rs`, these run against
//! `skill-studio-host`'s real adapters rather than the in-memory `FixtureFs`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use skill_studio_core::discovery_sources::DiscoverySources;
use skill_studio_core::dto::{ParkRequest, UnparkRequest};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::RootKind;
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SkillInvocationIndex, SqliteHistoryOpener};

const UNIVERSAL_ROOT_RELATIVE: &str = ".agents/skills";
const PARKED_ROOT_RELATIVE: &str = ".agents/skills-parked";

/// Serializes the one test below that mutates the process-wide `CODEX_HOME`
/// env var - `skill_studio_host::codex_home` is the one place allowed to
/// read it, and cargo runs every `#[test]` in this binary on shared threads.
static CODEX_HOME_ENV_LOCK: Mutex<()> = Mutex::new(());

fn runtime_for(home: &Path, codex_home: Option<&Path>) -> Runtime {
    let history_root = home.join(".history");
    let db_path = history_root.join("events.sqlite3");
    let mut scope = RuntimeScope::fixture(home);
    if let Some(codex_home) = codex_home {
        scope = scope.with_codex_home(codex_home);
    }
    let ports = Ports {
        fs: Arc::new(RealFs::new()),
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history: Arc::new(SqliteHistoryOpener::new(db_path)),
        sink: Arc::new(RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
    };
    Runtime::new(&scope, ports).unwrap()
}

/// codex_disable_writes_the_skills_config_row_and_keeps_other_tables_and_comments_or_names_the_dropped_content:
/// disabling a skill adds a `[[skills.config]]` row, but every other byte of
/// an existing `config.toml` - an unrelated table, and the comment above it -
/// survives untouched, since `set_codex_skill_disabled` edits the parsed
/// document in place rather than re-serializing a plain value.
#[test]
fn codex_disable_writes_the_skills_config_row_and_keeps_other_tables_and_comments_or_names_the_dropped_content(
) {
    let home = unique_temp_dir("codex_disable_preserves");
    let codex_home = home.join(".codex");
    std::fs::create_dir_all(&codex_home).unwrap();
    let original =
        "# a user comment\nmodel = \"o3\"\n\n[projects.\"/my-project\"]\ntrusted = true\n";
    std::fs::write(codex_home.join("config.toml"), original).unwrap();
    let rt = runtime_for(&home, None);
    let skill_md = home
        .join(UNIVERSAL_ROOT_RELATIVE)
        .join("gamma")
        .join("SKILL.md");

    ops::set_codex_skill_disabled(&rt, &ctx(), &skill_md, true).unwrap();

    let written = std::fs::read_to_string(codex_home.join("config.toml")).unwrap();
    assert!(
        written.contains("# a user comment"),
        "dropped content: the leading comment did not survive: {written}"
    );
    assert!(
        written.contains("[projects.\"/my-project\"]") && written.contains("trusted = true"),
        "dropped content: the unrelated projects table did not survive: {written}"
    );
    assert!(
        written.contains(&skill_md.to_string_lossy().to_string())
            && written.contains("enabled = false"),
        "the new skills.config row was not written: {written}"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// codex_park_updates_the_skills_config_row_path_or_names_the_stale_row: a
/// skill disabled through Codex's config, then parked, keeps its disabled
/// row pointing at the directory it actually lives in now - not the one
/// `park` just moved it out of (`docs/action-map/harnesses/codex.md`).
#[test]
fn codex_park_updates_the_skills_config_row_path_or_names_the_stale_row() {
    let home = unique_temp_dir("codex_park_row");
    let dir = home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        b"---\nname: gamma\ndescription: a parkable skill\n---\nBody.\n",
    )
    .unwrap();
    let rt = runtime_for(&home, None);
    let old_skill_md = dir.join("SKILL.md");
    ops::set_codex_skill_disabled(&rt, &ctx(), &old_skill_md, true).unwrap();

    let inventory = ops::scan(&rt, &ctx(), &Default::default()).unwrap();
    let deployment_id = inventory
        .skills
        .iter()
        .find(|s| s.name.0 == "gamma")
        .unwrap()
        .deployments
        .iter()
        .find(|d| d.root.kind == RootKind::Universal)
        .unwrap()
        .id
        .clone();

    let outcome = ops::park(&rt, &ctx(), &ParkRequest { deployment_id }).unwrap();
    let new_skill_md = outcome.parked_path.join("SKILL.md");
    assert_eq!(
        new_skill_md,
        home.join(PARKED_ROOT_RELATIVE)
            .join("gamma")
            .join("SKILL.md")
    );

    let config = std::fs::read_to_string(home.join(".codex").join("config.toml")).unwrap();
    assert!(
        !config.contains(&old_skill_md.to_string_lossy().to_string()),
        "the stale row: config.toml still names the pre-park path: {config}"
    );
    assert!(
        config.contains(&new_skill_md.to_string_lossy().to_string()),
        "config.toml never picked up the parked path: {config}"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// codex_unpark_rewrites_the_disable_row_back_to_the_live_path_or_names_the_stale_parked_path:
/// disable, park, then unpark - the row `park` rewrote to the parked
/// `SKILL.md` path must come back to naming the live path, or Codex still
/// treats the restored skill as disabled at a directory that no longer
/// exists.
#[test]
fn codex_unpark_rewrites_the_disable_row_back_to_the_live_path_or_names_the_stale_parked_path() {
    let home = unique_temp_dir("codex_unpark_row");
    let dir = home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        b"---\nname: gamma\ndescription: a parkable skill\n---\nBody.\n",
    )
    .unwrap();
    let rt = runtime_for(&home, None);
    let live_skill_md = dir.join("SKILL.md");
    ops::set_codex_skill_disabled(&rt, &ctx(), &live_skill_md, true).unwrap();

    let inventory = ops::scan(&rt, &ctx(), &Default::default()).unwrap();
    let deployment_id = inventory
        .skills
        .iter()
        .find(|s| s.name.0 == "gamma")
        .unwrap()
        .deployments
        .iter()
        .find(|d| d.root.kind == RootKind::Universal)
        .unwrap()
        .id
        .clone();

    let park_outcome = ops::park(
        &rt,
        &ctx(),
        &ParkRequest {
            deployment_id: deployment_id.clone(),
        },
    )
    .unwrap();
    let parked_skill_md = park_outcome.parked_path.join("SKILL.md");

    let inventory = ops::scan(&rt, &ctx(), &Default::default()).unwrap();
    let parked_deployment_id = inventory
        .skills
        .iter()
        .find(|s| s.name.0 == "gamma")
        .unwrap()
        .deployments
        .iter()
        .find(|d| d.root.kind == RootKind::Parked)
        .unwrap()
        .id
        .clone();

    ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: parked_deployment_id,
        },
    )
    .unwrap();

    let config = std::fs::read_to_string(home.join(".codex").join("config.toml")).unwrap();
    assert!(
        !config.contains(&parked_skill_md.to_string_lossy().to_string()),
        "the stale parked path: config.toml still names it after unpark: {config}"
    );
    assert!(
        config.contains(&live_skill_md.to_string_lossy().to_string()),
        "config.toml never picked up the live path after unpark: {config}"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// codex_honours_codex_home_for_config_and_rollouts_or_names_the_path_read_from_the_default:
/// with `CODEX_HOME` pointed somewhere other than `<home>/.codex`, both the
/// disable-config writer and the rollout reader follow it - neither one
/// falls back to reading or writing under the default path.
#[test]
fn codex_honours_codex_home_for_config_and_rollouts_or_names_the_path_read_from_the_default() {
    let _guard = CODEX_HOME_ENV_LOCK.lock().unwrap();
    let home = unique_temp_dir("codex_home_override");
    std::fs::create_dir_all(&home).unwrap();
    // Nested under `home` (as a real `CODEX_HOME` override normally is,
    // e.g. `~/.codex2`) rather than an unrelated directory: `confine`
    // requires every path it confines, including `codex_home` itself, to
    // have a parent already inside the scope, and `home` is that scope's
    // only unconditional root.
    let custom_codex_home = home.join("custom-codex-home");
    std::fs::create_dir_all(&custom_codex_home).unwrap();
    std::env::set_var("CODEX_HOME", &custom_codex_home);

    // Config: the writer must land under the override, not `<home>/.codex`.
    let rt = runtime_for(&home, Some(&skill_studio_host::codex_home(&home)));
    let skill_md = home
        .join(UNIVERSAL_ROOT_RELATIVE)
        .join("gamma")
        .join("SKILL.md");
    ops::set_codex_skill_disabled(&rt, &ctx(), &skill_md, true).unwrap();
    assert!(
        custom_codex_home.join("config.toml").exists(),
        "the path read from the default: config.toml was not written under CODEX_HOME"
    );
    assert!(
        !home.join(".codex").join("config.toml").exists(),
        "the path read from the default: config.toml leaked into <home>/.codex despite CODEX_HOME"
    );

    // Rollouts: the reader must find a session file under the override.
    let session_dir = custom_codex_home.join("sessions").join("2026/09/17");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("a.jsonl"),
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"sess-a\",\"cwd\":\"/proj-a\"}}\n",
    )
    .unwrap();
    let mut index = SkillInvocationIndex::default();
    let report = index.refresh(&home, &DiscoverySources::default());
    assert!(
        report.bytes_read > 0,
        "the path read from the default: no bytes were read from CODEX_HOME/sessions"
    );

    std::env::remove_var("CODEX_HOME");
    std::fs::remove_dir_all(&home).ok();
    std::fs::remove_dir_all(&custom_codex_home).ok();
}
