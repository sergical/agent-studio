//! End-to-end tests: runs the built `skill-studio` binary against
//! materialized fixtures with `--fixture --json`, and checks the printed
//! `ResultEnvelope` and the process exit status.
//!
//! `scan`/`diagnose` golden files live at `apps/cli/tests/golden/
//! <fixture>.<op>.json`, with the temp fixture root normalized to `$HOME`
//! and `correlation_id` blanked (it is fresh per run). Regenerate with
//! `UPDATE_GOLDENS=1 cargo test -p skill-studio-cli --test envelope`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use skill_studio_core::ports::{LeaseKey, LeaseMode, LeaseProvider};
use skill_studio_core::testing::fixtures;
use skill_studio_host::FileLease;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_skill-studio")
}

/// Materializes the named fixture to a fresh temp directory and returns its
/// canonical path.
fn materialized_fixture(name: &str) -> PathBuf {
    let dir = tempfile::tempdir().unwrap().keep();
    let (_, builder) = fixtures::all()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap();
    builder
        .materialize(&dir)
        .unwrap_or_else(|e| panic!("materialize {name}: {e}"));
    dir.canonicalize().unwrap()
}

struct Run {
    status: i32,
    json: serde_json::Value,
}

fn run(args: &[&str]) -> Run {
    let output = Command::new(bin())
        .args(args)
        .output()
        .expect("run skill-studio");
    let status = output.status.code().expect("no signal");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not one JSON document: {e}\n{stdout}"));
    Run { status, json }
}

/// Replaces every occurrence of `from` inside any string value in `value`,
/// including a string that only partly matches (a `DeploymentDto.id` embeds
/// the home path percent-encoded partway through a larger opaque string).
fn replace_everywhere(value: &mut serde_json::Value, from: &str, to: &str) {
    match value {
        serde_json::Value::String(s) => {
            if s.contains(from) {
                *s = s.replace(from, to);
            }
        }
        serde_json::Value::Array(items) => items
            .iter_mut()
            .for_each(|v| replace_everywhere(v, from, to)),
        serde_json::Value::Object(map) => map
            .values_mut()
            .for_each(|v| replace_everywhere(v, from, to)),
        _ => {}
    }
}

/// Blanks every `key`'s value anywhere in `value`, however deeply nested -
/// used for fields that are fresh per run/materialization (a deployment's
/// `modified_at` is the fixture's real file mtime, set the instant the
/// fixture is materialized to a temp directory).
fn blank_field(value: &mut serde_json::Value, key: &str) {
    match value {
        serde_json::Value::Object(map) => {
            if map.contains_key(key) {
                map.insert(key.to_string(), serde_json::Value::String("-".into()));
            }
            for v in map.values_mut() {
                blank_field(v, key);
            }
        }
        serde_json::Value::Array(items) => {
            items.iter_mut().for_each(|v| blank_field(v, key));
        }
        _ => {}
    }
}

/// Normalizes the fixture's temp root to `$HOME` and blanks the
/// per-invocation `correlation_id`, so the envelope is deterministic enough
/// to compare against a golden file.
fn normalize(home: &Path, mut json: serde_json::Value) -> serde_json::Value {
    replace_everywhere(&mut json, &home.to_string_lossy(), "$HOME");
    let encoded_home = home
        .to_string_lossy()
        .replace('%', "%25")
        .replace('/', "%2F");
    replace_everywhere(&mut json, &encoded_home, "%24HOME");
    if let Some(obj) = json.as_object_mut() {
        obj.insert(
            "correlation_id".into(),
            serde_json::Value::String("-".into()),
        );
        // `scope.id` hashes the *un-normalized* canonical home path (see
        // `ScopeId::for_canonical_home`), so it differs per temp directory
        // even after the `$HOME` substring rewrite above.
        if let Some(scope) = obj.get_mut("scope").and_then(|s| s.as_object_mut()) {
            scope.insert("id".into(), serde_json::Value::String("-".into()));
        }
    }
    // A deployment's `modified_at` is the fixture's real mtime at
    // materialization time, not reproducible run to run.
    blank_field(&mut json, "modified_at");
    json
}

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

fn assert_matches_golden(golden_name: &str, actual: &serde_json::Value) {
    let update = std::env::var("UPDATE_GOLDENS").is_ok();
    let path = golden_path(golden_name);
    if update {
        std::fs::write(&path, serde_json::to_string_pretty(actual).unwrap() + "\n")
            .unwrap_or_else(|e| panic!("write golden {golden_name}: {e}"));
        return;
    }
    let golden_text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden at {}: {e}. Run with UPDATE_GOLDENS=1 to create it.",
            path.display()
        )
    });
    let golden: serde_json::Value = serde_json::from_str(&golden_text).unwrap();
    if &golden != actual {
        let expected_text = serde_json::to_string_pretty(&golden).unwrap();
        let actual_text = serde_json::to_string_pretty(actual).unwrap();
        let diff = similar::TextDiff::from_lines(&expected_text, &actual_text)
            .unified_diff()
            .header("expected", "actual")
            .to_string();
        panic!("{golden_name} mismatch:\n{diff}");
    }
}

#[test]
fn scan_on_a_clean_fixture_exits_0() {
    let home = materialized_fixture("basic");
    let run = run(&["scan", "--fixture", home.to_str().unwrap(), "--json"]);
    assert_eq!(run.status, 0);
    assert_eq!(run.json["status"], "ok");
    assert_matches_golden("basic.scan.json", &normalize(&home, run.json));
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn diagnose_with_warnings_exits_1() {
    let home = materialized_fixture("broken_link");
    let run = run(&["diagnose", "--fixture", home.to_str().unwrap(), "--json"]);
    assert_eq!(run.status, 1);
    assert_eq!(run.json["status"], "ok");
    assert_matches_golden("broken_link.diagnose.json", &normalize(&home, run.json));
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn a_zero_read_timeout_forces_a_partial_scan_that_exits_4() {
    let home = materialized_fixture("basic");
    let run = run(&[
        "scan",
        "--fixture",
        home.to_str().unwrap(),
        "--read-timeout-ms",
        "0",
        "--json",
    ]);
    assert_eq!(run.status, 4);
    assert_eq!(run.json["status"], "partial");
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn a_nonexistent_fixture_exits_2() {
    let run = run(&[
        "scan",
        "--fixture",
        "/nonexistent/path/skill-studio-cli-test",
        "--json",
    ]);
    assert_eq!(run.status, 2);
    assert_eq!(run.json["status"], "error");
    assert_eq!(run.json["data"], serde_json::Value::Null);
}

#[test]
fn a_lease_held_by_another_process_exits_3() {
    let home = materialized_fixture("basic");
    let lease_root = home.join(".history").join("leases");
    let lease = FileLease::new(lease_root);
    let key = LeaseKey {
        canonical_root: home.clone(),
    };
    // Hold the exclusive lease for the whole call, so the CLI's shared
    // acquire for `scan` times out against it.
    let _guard = lease
        .acquire(&[key], LeaseMode::Exclusive, Duration::from_secs(5))
        .expect("acquire exclusive lease");

    let run = run(&[
        "scan",
        "--fixture",
        home.to_str().unwrap(),
        "--read-timeout-ms",
        "100",
        "--json",
    ]);
    assert_eq!(run.status, 3);
    assert_eq!(run.json["status"], "error");
    assert_eq!(run.json["errors"][0]["code"], "scope_busy");
    std::fs::remove_dir_all(&home).ok();
}

/// Lists every path under `dir`, relative to it, sorted - `None` when `dir`
/// doesn't exist. Used to snapshot the real ambient data root before and
/// after a `--home <tempdir>` run.
fn snapshot_tree(dir: &Path) -> Option<Vec<PathBuf>> {
    if !dir.exists() {
        return None;
    }
    fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    Some(out)
}

fn real_ambient_data_root() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("skill-studio");
        }
    }
    dirs::home_dir().unwrap().join(".local/share/skill-studio")
}

/// A universal, skills.sh-owned skill with the unquoted `description: a: b`
/// shape `preview-repair`/`apply-repair` know how to fix - built directly
/// (not through `materialized_fixture`/`--fixture`) because this test needs
/// a `--home` live scope, and a live scope's mutability rule only makes a
/// universal, lock-file-owned skill writable.
fn repairable_live_home() -> PathBuf {
    let home = tempfile::tempdir().unwrap().keep();
    // A per-harness root with no `.skill-lock.json` entry, so the
    // deployment is `Manual`-owned. The core's repair gate
    // (`desktop_repair_apply_modes`) only writes `ApplyFix` for
    // `Manual`/`Copy`/`Fork` owners; a `SkillsSh`-owned universal-canonical
    // skill instead gets `ForkAndFix`/`FixInstalledCopy`, which this core
    // build has no writer for yet, so it would refuse here.
    let dir = home.join(".claude/skills/zeta-bad");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        b"---\nname: zeta-bad\ndescription: Use this: when needed\n---\nBody.\n",
    )
    .unwrap();
    home.canonicalize().unwrap()
}

/// The coordinator's hazard fix: `--home <tempdir>` (no `--fixture`) must
/// derive its history and lease roots from that temp directory, never from
/// the real machine's ambient XDG data root - checked end to end through
/// `apply-repair`, the write command that exercises both the lease and the
/// history store.
#[test]
fn home_flag_writes_nothing_outside_the_given_directory() {
    let real_root = real_ambient_data_root();
    let before = snapshot_tree(&real_root);

    let home = repairable_live_home();
    // The deployment id is opaque (embeds the percent-encoded home path);
    // read it back from a scan instead of hardcoding it.
    let scan = run(&["scan", "--home", home.to_str().unwrap(), "--json"]);
    let deployment_id = scan.json["data"]["skills"][0]["deployments"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let preview = run(&[
        "preview-repair",
        "--home",
        home.to_str().unwrap(),
        "--deployment-id",
        &deployment_id,
        "--json",
    ]);
    assert_eq!(preview.json["status"], "ok", "{:?}", preview.json);

    let preview_path = home.join("preview.json");
    std::fs::write(
        &preview_path,
        serde_json::to_string(&preview.json["data"]).unwrap(),
    )
    .unwrap();
    let apply = run(&[
        "apply-repair",
        "--home",
        home.to_str().unwrap(),
        "--preview-json",
        preview_path.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(apply.json["status"], "ok", "{:?}", apply.json);
    let event_id = apply.json["data"]["event_id"].as_str().unwrap().to_string();

    // The event landed under the temp home, not the real data root.
    let history_root = apply.json["scope"]["history_root"].as_str().unwrap();
    assert!(
        Path::new(history_root).starts_with(&home),
        "history_root {history_root} is not under {}",
        home.display()
    );
    let events = run(&["events", "--home", home.to_str().unwrap(), "--json"]);
    assert_eq!(events.json["status"], "ok", "{:?}", events.json);
    assert!(events.json["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["id"] == event_id));

    let after = snapshot_tree(&real_root);
    assert_eq!(
        before,
        after,
        "--home run touched the real ambient data root {}",
        real_root.display()
    );

    std::fs::remove_dir_all(&home).ok();
}

/// A write command (`restore`, here) never blocks on a held lease: it
/// surfaces the ordinary `scope_busy` envelope at exit 3, the same as a
/// read command would.
#[test]
fn a_write_command_under_a_held_lease_exits_3_instead_of_blocking() {
    let home = materialized_fixture("basic");
    let lease_root = home.join(".history").join("leases");
    let lease = FileLease::new(lease_root);
    let key = LeaseKey {
        canonical_root: home.clone(),
    };
    let _guard = lease
        .acquire(&[key], LeaseMode::Exclusive, Duration::from_secs(5))
        .expect("acquire exclusive lease");

    let run = run(&[
        "restore",
        "--fixture",
        home.to_str().unwrap(),
        "--event-id",
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "--write-timeout-ms",
        "100",
        "--json",
    ]);
    assert_eq!(run.status, 3);
    assert_eq!(run.json["status"], "error");
    assert_eq!(run.json["errors"][0]["code"], "scope_busy");
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn schema_regenerates_the_checked_in_snapshot() {
    let out = tempfile::tempdir().unwrap();
    let status = Command::new(bin())
        .args(["schema", "--out"])
        .arg(out.path())
        .status()
        .expect("run skill-studio schema");
    assert!(status.success());

    let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("crates/skill-studio-core/schema");
    let mut names: Vec<_> = std::fs::read_dir(&checked_in)
        .unwrap_or_else(|e| {
            panic!(
                "missing checked-in schema dir {}: {e}",
                checked_in.display()
            )
        })
        .map(|e| e.unwrap().file_name())
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no checked-in schema files to compare");

    for name in names {
        let checked_in_text = std::fs::read_to_string(checked_in.join(&name)).unwrap();
        let regenerated_text = std::fs::read_to_string(out.path().join(&name))
            .unwrap_or_else(|e| panic!("schema regeneration did not write {name:?}: {e}"));
        assert_eq!(
            checked_in_text, regenerated_text,
            "{name:?} is stale; run `cargo run -p skill-studio-cli -- schema --out crates/skill-studio-core/schema` and check it in"
        );
    }
}
