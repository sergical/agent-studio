//! Bench: `ops::scan` over the generated 400-skill estate
//! ([`skill_studio_core::bench_estate::estate`]), once on real disk and once
//! entirely in memory, so the disk cost and the pure scan cost are separate
//! numbers. Run on demand with `cargo bench -p skill-studio-core --features
//! testing --bench scan`; nothing in CI reads the result.
//!
//! Park and install-plan benches belong in this same file, alongside scan,
//! once units 3.1 and 3.5 add those ops to the core.

// A bench harness has no caller to return an error to; a setup failure here
// should stop the run immediately with the panic message, same as a test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};

use skill_studio_core::bench_estate::estate;
use skill_studio_core::dto::ScanRequest;
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::ops::scan;
use skill_studio_core::ports::{LeaseProvider, Ports, Runtime, ScopeFs};
use skill_studio_core::scope::{ProjectSelection, RuntimeScope};
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FakeClock, FakeIds, FakeLease, NoHistory, RecordingSink};

use skill_studio_host::{FileLease, RealFs};

const SKILL_COUNT: usize = 400;
const SEED: u64 = 1;

fn scope_for(home: &std::path::Path, project_dirs: &[PathBuf]) -> RuntimeScope {
    let mut scope = RuntimeScope::fixture(home);
    scope.read_timeout_ms = 10_000;
    scope.projects = ProjectSelection::Explicit {
        paths: project_dirs.iter().map(|d| home.join(d)).collect(),
    };
    scope
}

fn ports_with(fs: Arc<dyn ScopeFs>, leases: Arc<dyn LeaseProvider>) -> Ports {
    Ports {
        fs,
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases,
        history: Arc::new(NoHistory),
        sink: Arc::new(RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
    }
}

fn bench_scan_on_disk(c: &mut Criterion) {
    let generated = estate(SKILL_COUNT, SEED);
    let dir = unique_temp_dir("bench-scan-disk");
    std::fs::create_dir_all(&dir).expect("create bench home");
    let home = dir.canonicalize().expect("canonicalize bench home");
    generated
        .builder
        .materialize(&home)
        .expect("materialize bench estate");
    let scope = scope_for(&home, &generated.project_dirs);

    // Named for the 100ms budget it's checked against by hand (unit 3.3's
    // "Done when": `cargo bench -p skill-studio-core --features testing
    // --bench scan`, read the printed median - no wall-clock `#[test]` gate,
    // since a debug-mode assertion measures build-mode cost, not a
    // regression; see the deleted `tests/scan_budget.rs`). Last measured at
    // ~86ms median.
    c.bench_function(
        "scan_on_the_bench_estate_finishes_under_the_100ms_budget",
        |b| {
            b.iter(|| {
                let ports = ports_with(
                    Arc::new(RealFs::new()),
                    Arc::new(FileLease::new(home.join(".leases"))),
                );
                let rt = Runtime::new(&scope, ports).expect("runtime");
                scan(&rt, &ctx(), &ScanRequest::default()).expect("scan")
            });
        },
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Until `FixtureFs`'s lookups are fixed to be cheap under contention, this
/// bench measures the `FixtureFs` double, not `ops::scan` itself.
fn bench_scan_in_memory(c: &mut Criterion) {
    let generated = estate(SKILL_COUNT, SEED);
    let home = PathBuf::from("/bench-home");
    let home_str = home.to_string_lossy().into_owned();
    let fs: Arc<dyn ScopeFs> = Arc::new(
        generated
            .builder
            .rooted_at(&home_str)
            .dir(&home_str)
            .build_fs(),
    );
    let scope = scope_for(&home, &generated.project_dirs);

    c.bench_function("scan_400_skills_in_memory", |b| {
        b.iter(|| {
            let ports = ports_with(Arc::clone(&fs), Arc::new(FakeLease::default()));
            let rt = Runtime::new(&scope, ports).expect("runtime");
            scan(&rt, &ctx(), &ScanRequest::default()).expect("scan")
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(30).measurement_time(Duration::from_secs(10));
    targets = bench_scan_on_disk, bench_scan_in_memory
}
criterion_main!(benches);
