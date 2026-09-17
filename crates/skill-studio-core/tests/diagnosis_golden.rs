//! Golden snapshots for `ops::diagnose`, one per named fixture.
//!
//! Same harness as `scan_golden.rs`: every fixture in
//! [`skill_studio_core::testing::fixtures::all`] is materialized to a real
//! temp directory through `skill-studio-host`'s `RealFs`, diagnosed, and
//! normalized (temp root -> `$HOME`), then compared against a committed
//! golden file. `UPDATE_GOLDENS=1 cargo test -p skill-studio-core --test
//! diagnosis_golden` regenerates them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skill_studio_core::dto::ScanRequest;
use skill_studio_core::ops::diagnose;
use skill_studio_core::ports::Runtime;
use skill_studio_core::testing::fixtures;
use skill_studio_core::testing::golden::{
    assert_json_eq, ctx, materialized_ports, normalize, scope_for, unique_temp_dir,
};

use skill_studio_host::{FileLease, RealFs};

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.diagnosis.json"))
}

fn run_materialized(name: &str, home: &Path) -> serde_json::Value {
    let ports = materialized_ports(
        Arc::new(RealFs::new()),
        Arc::new(FileLease::new(home.join(".leases"))),
    );
    let scope = scope_for(name, home);
    let rt = Runtime::new(&scope, ports).expect("runtime");
    let diagnosis = diagnose(&rt, &ctx(), &ScanRequest::default()).expect("diagnose");
    normalize(home, serde_json::to_value(&diagnosis).unwrap())
}

#[test]
fn diagnose_matches_the_golden_snapshot_for_every_fixture() {
    let update = std::env::var("UPDATE_GOLDENS").is_ok();

    for (name, builder) in fixtures::all() {
        let dir = unique_temp_dir(name);
        std::fs::create_dir_all(&dir).unwrap();
        let home = dir.canonicalize().unwrap();

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

        std::fs::remove_dir_all(&dir).ok();
    }
}
