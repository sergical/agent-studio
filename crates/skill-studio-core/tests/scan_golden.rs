//! Golden snapshots for `ops::scan`, one per named fixture.
//!
//! Every fixture in [`skill_studio_core::testing::fixtures::all`] is run two
//! ways: materialized to a real temp directory through `skill-studio-host`'s
//! `RealFs`, and scanned in memory through `FixtureFs`. Both are normalized
//! (temp root -> `$HOME`) and must produce byte-identical JSON, so the two
//! `ScopeFs` implementations agree. The materialized run is compared against
//! a committed golden file; `UPDATE_GOLDENS=1 cargo test -p skill-studio-core
//! --test scan_golden` regenerates them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skill_studio_core::dto::ScanRequest;
use skill_studio_core::ops::scan;
use skill_studio_core::ports::{Ports, Runtime};
use skill_studio_core::testing::golden::{
    assert_json_eq, ctx, materialized_ports, normalize, scope_for, unique_temp_dir,
};
use skill_studio_core::testing::{fixtures, FakeClock, FakeIds, FakeLease, NoHistory};

use skill_studio_host::{FileLease, RealFs};

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.inventory.json"))
}

fn run_materialized(name: &str, home: &Path) -> serde_json::Value {
    let ports = materialized_ports(
        Arc::new(RealFs::new()),
        Arc::new(FileLease::new(home.join(".leases"))),
    );
    let scope = scope_for(name, home);
    let rt = Runtime::new(&scope, ports).expect("runtime");
    let inv = scan(&rt, &ctx(), &ScanRequest::default()).expect("scan");
    normalize(home, serde_json::to_value(&inv).unwrap())
}

fn run_in_memory(name: &str, home: &Path) -> serde_json::Value {
    use skill_studio_core::harness::HarnessCatalog;
    use skill_studio_core::testing::FixtureBuilder;

    let home_str = home.to_string_lossy().into_owned();
    let builder: FixtureBuilder = fixtures::all()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no fixture named {name}"))
        .1
        .rooted_at(&home_str)
        .dir(&home_str);
    let fs = if name == "disabled" {
        builder.build_fs_with_home(home)
    } else {
        builder.build_fs()
    };

    let ports = Ports {
        fs: Arc::new(fs),
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FakeLease::default()),
        history: Arc::new(NoHistory),
        sink: Arc::new(skill_studio_core::testing::RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
    };
    let scope = scope_for(name, home);
    let rt = Runtime::new(&scope, ports).expect("runtime");
    let inv = scan(&rt, &ctx(), &ScanRequest::default()).expect("scan");
    normalize(home, serde_json::to_value(&inv).unwrap())
}

#[test]
fn scan_matches_the_golden_snapshot_for_every_fixture() {
    let update = std::env::var("UPDATE_GOLDENS").is_ok();

    for (name, _builder) in fixtures::all() {
        let dir = unique_temp_dir(name);
        std::fs::create_dir_all(&dir).unwrap();
        let home = dir.canonicalize().unwrap();

        let (_, builder) = fixtures::all()
            .into_iter()
            .find(|(n, _)| *n == name)
            .unwrap();
        builder
            .materialize(&home)
            .unwrap_or_else(|e| panic!("materialize {name}: {e}"));

        let materialized = run_materialized(name, &home);

        let path = golden_path(name);
        if update {
            std::fs::write(
                &path,
                serde_json::to_string_pretty(&materialized).unwrap() + "\n",
            )
            .unwrap_or_else(|e| panic!("write golden {name}: {e}"));
        } else {
            let golden_text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!(
                    "missing golden for {name} at {}: {e}. Run with UPDATE_GOLDENS=1 to create it.",
                    path.display()
                )
            });
            let golden: serde_json::Value = serde_json::from_str(&golden_text).unwrap();
            assert_json_eq(
                &format!("{name} (materialized vs golden)"),
                &golden,
                &materialized,
            );
        }

        // The `basic` and `whole_dir_link` fixtures declare aliases whose
        // relative targets climb out of the harness root
        // (`../../.agents/skills/...`); `FixtureFs`'s alias resolver now
        // collapses those `..` segments the same way `RealFs::canonicalize`
        // does (see `testing::FixtureFs::resolve`), so every fixture,
        // including these two, is asserted here rather than excluded.
        let in_memory = run_in_memory(name, &home);
        assert_json_eq(
            &format!("{name} (materialized vs in-memory)"),
            &materialized,
            &in_memory,
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
