//! Compares the medians `cargo bench` just recorded against a committed
//! baseline (`bench/baseline.json` by default, or the file named by
//! `BENCH_BASELINE`), failing when any bench is more than 20% above its
//! baseline. `#[ignore]` by default: it reads
//! `target/criterion/<bench>/new/estimates.json`, which only exists after a
//! bench run finished in this checkout, so `npm run bench:check` always runs
//! `cargo bench` first.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Baseline {
    benches: Vec<BaselineEntry>,
    /// Bench names allowed to have `target/criterion` results with no
    /// baseline entry, e.g. `scan_400_skills_in_memory`, which is excluded
    /// from the gate until `FixtureFs` lookups are fixed (see
    /// `benches/scan.rs`) but must still run so its number stays visible.
    #[serde(default)]
    ungated: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct BaselineEntry {
    name: String,
    median_ns: f64,
}

#[derive(Debug, Deserialize)]
struct Estimates {
    median: PointEstimate,
}

#[derive(Debug, Deserialize)]
struct PointEstimate {
    point_estimate: f64,
}

/// `crates/skill-studio-core` is two levels under the workspace root.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("core crate is two levels under the workspace root")
        .to_path_buf()
}

fn baseline_path() -> PathBuf {
    let relative =
        std::env::var("BENCH_BASELINE").unwrap_or_else(|_| "bench/baseline.json".to_string());
    workspace_root().join(relative)
}

/// `cargo bench` writes `criterion/` under `CARGO_TARGET_DIR` when set,
/// falling back to the workspace's own `target/` otherwise.
fn criterion_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir).join("criterion"),
        None => workspace_root().join("target/criterion"),
    }
}

fn criterion_median_ns(bench_name: &str) -> f64 {
    let path = criterion_dir().join(bench_name).join("new/estimates.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}; run `cargo bench` first", path.display()));
    let estimates: Estimates =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    estimates.median.point_estimate
}

/// Guards: a regression that slips into `ops::scan` (or, once units 3.1 and
/// 3.5 land, `park` or the install plan) would otherwise only surface as a
/// slow app, never a failed check.
#[test]
#[ignore]
fn bench_medians_are_within_20_percent_of_the_baseline() {
    let baseline_path = baseline_path();
    let text = std::fs::read_to_string(&baseline_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", baseline_path.display()));
    let baseline: Baseline = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse {}: {e}", baseline_path.display()));

    let mut failures = Vec::new();
    for entry in &baseline.benches {
        let actual_ns = criterion_median_ns(&entry.name);
        let allowed_ns = entry.median_ns * 1.2;
        if actual_ns > allowed_ns {
            failures.push(format!(
                "{}: {actual_ns:.0} ns is more than 20% above the baseline {:.0} ns (allowed up to {allowed_ns:.0} ns)",
                entry.name, entry.median_ns
            ));
        }
    }

    // A bench with results but no baseline entry and no `ungated` exemption
    // would otherwise run every time and never be checked against anything.
    let gated: std::collections::HashSet<&str> =
        baseline.benches.iter().map(|e| e.name.as_str()).collect();
    let ungated: std::collections::HashSet<&str> =
        baseline.ungated.iter().map(|s| s.as_str()).collect();
    if let Ok(entries) = std::fs::read_dir(criterion_dir()) {
        for entry in entries.filter_map(Result::ok) {
            let bench_name = entry.file_name();
            let bench_name = bench_name.to_string_lossy();
            let has_estimates = entry.path().join("new/estimates.json").is_file();
            if has_estimates
                && !gated.contains(bench_name.as_ref())
                && !ungated.contains(bench_name.as_ref())
            {
                failures.push(format!(
                    "{bench_name}: has criterion results but no baseline entry and no `ungated` exemption"
                ));
            }
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
