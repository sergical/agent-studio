// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
            for v in items.iter_mut() {
                blank_field(v, key);
            }
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
    // `envelope.timings.elapsed_ms` (whole call and per step) is wall-clock
    // time, not reproducible run to run; the step names and shape are.
    blank_field(&mut json, "elapsed_ms");
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
fn scan_time_prints_step_lines_to_stderr() {
    let home = materialized_fixture("basic");
    let output = Command::new(bin())
        .args([
            "scan",
            "--fixture",
            home.to_str().unwrap(),
            "--json",
            "--time",
        ])
        .output()
        .expect("run skill-studio");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(
        stderr
            .lines()
            .any(|line| line.trim_start().starts_with("scan ")),
        "expected an op line, got:\n{stderr}"
    );
    let step_lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("  ") && line.contains("....") && line.ends_with("ms"))
        .collect();
    assert!(
        !step_lines.is_empty(),
        "expected at least one indented step line, got:\n{stderr}"
    );
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

/// `--home` picks up the folders the user added or stopped tracking from
/// `<home>/.agents/skill-studio.json` - the same file the desktop and the MCP
/// server read.
#[test]
fn home_flag_applies_the_saved_project_list() {
    let home = tempfile::tempdir().unwrap().keep();
    std::fs::create_dir_all(home.join("kept")).unwrap();
    std::fs::create_dir_all(home.join("dropped-by-exclude")).unwrap();
    // "deleted" is named in `added` but never created on disk.
    std::fs::create_dir_all(home.join(".agents")).unwrap();
    let registry = serde_json::json!({
        "projects": {
            "added": [
                home.join("kept"),
                home.join("dropped-by-exclude"),
                home.join("deleted"),
                home.clone(),
            ],
            "excluded": [home.join("dropped-by-exclude")],
        }
    });
    std::fs::write(
        home.join(".agents").join("skill-studio.json"),
        serde_json::to_vec(&registry).unwrap(),
    )
    .unwrap();

    let run = run(&["scan", "--home", home.to_str().unwrap(), "--json"]);
    assert_eq!(run.json["status"], "ok", "{:?}", run.json);
    let projects: Vec<PathBuf> = run.json["scope"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| Path::new(p.as_str().unwrap()).canonicalize().unwrap())
        .collect();
    assert_eq!(
        projects,
        [home.join("kept").canonicalize().unwrap()],
        "dropped-by-exclude is excluded, deleted is missing, and home is never a project"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `--home` also honours `<home>/.agents/skill-studio.json`'s `discovery`
/// key, which switches a harness's own project history off for discovery -
/// the same file and key the desktop and the MCP server read.
#[test]
fn home_flag_applies_the_saved_discovery_switches() {
    let home = tempfile::tempdir().unwrap().keep();
    let codex_only = home.join("codex-only");
    let claude_only = home.join("claude-only");
    for project in [&codex_only, &claude_only] {
        std::fs::create_dir_all(project.join(".agents/skills")).unwrap();
    }
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(
        home.join(".codex/config.toml"),
        format!("[projects.\"{}\"]\n", codex_only.display()),
    )
    .unwrap();
    std::fs::create_dir_all(home.join(".claude/projects/-a")).unwrap();
    std::fs::write(
        home.join(".claude/projects/-a/session.jsonl"),
        format!(r#"{{"cwd":"{}"}}"#, claude_only.display()),
    )
    .unwrap();

    let projects = |home: &Path| -> Vec<PathBuf> {
        let run = run(&["scan", "--home", home.to_str().unwrap(), "--json"]);
        assert_eq!(run.json["status"], "ok", "{:?}", run.json);
        run.json["scope"]["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| Path::new(p.as_str().unwrap()).canonicalize().unwrap())
            .collect()
    };

    // Before the settings file exists, both harnesses' folders are found -
    // proving the switch below is what removes codex-only, not something
    // else about the fixture.
    let before = projects(&home);
    assert!(before.contains(&codex_only.canonicalize().unwrap()));
    assert!(before.contains(&claude_only.canonicalize().unwrap()));

    std::fs::create_dir_all(home.join(".agents")).unwrap();
    std::fs::write(
        home.join(".agents/skill-studio.json"),
        serde_json::json!({ "discovery": { "codex": false } }).to_string(),
    )
    .unwrap();

    let after = projects(&home);
    assert!(!after.contains(&codex_only.canonicalize().unwrap()));
    assert!(after.contains(&claude_only.canonicalize().unwrap()));

    std::fs::remove_dir_all(&home).ok();
}

/// A malformed registry file downgrades to "nothing tracked", not a scan
/// failure - matching [`skill_studio_core::tracked_projects::TrackedProjects::read`].
#[test]
fn a_malformed_project_registry_still_scans_with_no_projects() {
    let home = tempfile::tempdir().unwrap().keep();
    std::fs::create_dir_all(home.join(".agents")).unwrap();
    std::fs::write(home.join(".agents").join("skill-studio.json"), b"not json").unwrap();

    let run = run(&["scan", "--home", home.to_str().unwrap(), "--json"]);
    assert_eq!(run.json["status"], "ok", "{:?}", run.json);
    assert_eq!(run.json["scope"]["projects"], serde_json::json!([]));

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
fn harnesses_with_no_binaries_on_path_prints_unknown_for_every_row() {
    let home = materialized_fixture("basic");
    // Override the child's PATH to an empty directory so no harness
    // executable resolves; the test process's own PATH and HOME are left
    // untouched.
    let empty_path_dir = tempfile::tempdir().unwrap();
    let output = Command::new(bin())
        .args(["harnesses", "--fixture", home.to_str().unwrap()])
        .env("PATH", empty_path_dir.path())
        .output()
        .expect("run skill-studio harnesses");
    assert!(output.status.success(), "expected exit 0");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let rows: Vec<&str> = stdout.lines().collect();
    assert_eq!(rows.len(), 6, "one row per first-class harness:\n{stdout}");
    for row in &rows {
        assert!(
            row.contains("Unknown"),
            "row named no version/install-method evidence as Unknown:\n{row}"
        );
    }
    std::fs::remove_dir_all(&home).ok();
}

/// A universal, canonical `alpha` skill with no Claude Code link yet, built
/// directly (not via `--fixture`) so `set-harness-enabled`'s live-scope
/// write - creating `<home>/.claude/skills/alpha` - has somewhere real to
/// land, and a second, unrelated `zeta-bad` skill with the same malformed
/// frontmatter `repairable_live_home` uses, so one `--home` scope carries
/// two independent write kinds to undo in turn.
fn undoable_live_home() -> PathBuf {
    let home = tempfile::tempdir().unwrap().keep();
    let alpha_dir = home.join(".agents/skills/alpha");
    std::fs::create_dir_all(&alpha_dir).unwrap();
    std::fs::write(
        alpha_dir.join("SKILL.md"),
        b"---\nname: alpha\ndescription: A universal skill.\n---\nBody.\n",
    )
    .unwrap();
    let zeta_dir = home.join(".claude/skills/zeta-bad");
    std::fs::create_dir_all(&zeta_dir).unwrap();
    std::fs::write(
        zeta_dir.join("SKILL.md"),
        b"---\nname: zeta-bad\ndescription: Use this: when needed\n---\nBody.\n",
    )
    .unwrap();
    home.canonicalize().unwrap()
}

/// `undo` reverts the newest journal entry this CLI slice writes, whichever
/// write kind it came from: a `set-harness-enabled` link toggle, then an
/// `apply-repair` frontmatter fix, each undone in turn without touching the
/// other's already-reverted change.
#[test]
fn skill_studio_undo_reverses_the_last_journal_entry_for_every_write_kind_in_this_slices_scope() {
    let home = undoable_live_home();
    let link_path = home.join(".claude/skills/alpha");
    let zeta_skill_md = home.join(".claude/skills/zeta-bad/SKILL.md");
    let original_zeta_content = std::fs::read(&zeta_skill_md).unwrap();

    // Write kind 1: `set-harness-enabled` links Claude Code to `alpha`.
    assert!(
        !link_path.exists(),
        "alpha should start with no Claude Code link"
    );
    let enable = run(&[
        "set-harness-enabled",
        "--home",
        home.to_str().unwrap(),
        "--skill",
        "alpha",
        "--harness",
        "claude-code",
        "--enabled",
        "--json",
    ]);
    assert_eq!(enable.json["status"], "ok", "{:?}", enable.json);
    assert!(
        link_path.symlink_metadata().is_ok(),
        "set-harness-enabled should have created the Claude Code link"
    );

    // `undo` reverts that link toggle - the only unreverted event so far.
    let undo_link = run(&["undo", "--home", home.to_str().unwrap(), "--json"]);
    assert_eq!(undo_link.json["status"], "ok", "{:?}", undo_link.json);
    assert!(
        link_path.symlink_metadata().is_err(),
        "undo should have removed the Claude Code link set-harness-enabled created"
    );

    // Write kind 2: `apply-repair` fixes `zeta-bad`'s frontmatter.
    let scan = run(&["scan", "--home", home.to_str().unwrap(), "--json"]);
    let zeta = scan.json["data"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "zeta-bad")
        .unwrap_or_else(|| panic!("zeta-bad missing from scan: {:?}", scan.json));
    let deployment_id = zeta["deployments"][0]["id"].as_str().unwrap().to_string();
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
    let repaired_content = std::fs::read(&zeta_skill_md).unwrap();
    assert_ne!(
        repaired_content, original_zeta_content,
        "apply-repair should have changed zeta-bad's SKILL.md"
    );

    // `undo` now reverts the repair - the link toggle it already reverted
    // above stays reverted, proving each `undo` call only touches the
    // newest still-unreverted entry.
    let undo_repair = run(&["undo", "--home", home.to_str().unwrap(), "--json"]);
    assert_eq!(undo_repair.json["status"], "ok", "{:?}", undo_repair.json);
    assert_eq!(
        std::fs::read(&zeta_skill_md).unwrap(),
        original_zeta_content,
        "undo should have put zeta-bad's original malformed frontmatter back"
    );
    assert!(
        link_path.symlink_metadata().is_err(),
        "undoing the repair must not resurrect the already-undone Claude Code link"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// A live home with one universal `gamma` skill, linked from Claude Code -
/// the shape `park` looks for - plus a second `manual-only` skill that no
/// install ledger claims. `gamma` is listed in `.agents/.skill-lock.json`,
/// so `outdated` classifies it as skills.sh-owned and `manual-only` as
/// untracked.
/// The `TempDir` comes back with the path: hold it for the test's
/// lifetime and the tree goes away even when an assertion panics.
fn parkable_live_home() -> (tempfile::TempDir, PathBuf) {
    // Canonical from the start: on macOS a temp dir is reached
    // through the `/var` -> `/private/var` symlink, and a link
    // written under the uncanonical path lies outside the scope the
    // runtime roots at the canonical one.
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let gamma_dir = home.join(".agents/skills/gamma");
    std::fs::create_dir_all(&gamma_dir).unwrap();
    std::fs::write(
        gamma_dir.join("SKILL.md"),
        b"---\nname: gamma\ndescription: A parkable skill.\n---\nBody.\n",
    )
    .unwrap();
    let claude_skills = home.join(".claude/skills");
    std::fs::create_dir_all(&claude_skills).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&gamma_dir, claude_skills.join("gamma")).unwrap();

    let manual_dir = claude_skills.join("manual-only");
    std::fs::create_dir_all(&manual_dir).unwrap();
    std::fs::write(
        manual_dir.join("SKILL.md"),
        b"---\nname: manual-only\ndescription: A hand-written skill.\n---\nBody.\n",
    )
    .unwrap();

    let agents_dir = home.join(".agents");
    std::fs::write(
        agents_dir.join(".skill-lock.json"),
        serde_json::to_vec(&serde_json::json!({
            "version": 3,
            "skills": {
                "gamma": {
                    "source": "owner/gamma",
                    "sourceType": "github",
                    "sourceUrl": "https://github.com/owner/gamma",
                    "skillFolderHash": "deadbeef",
                    "installedAt": "2024-01-01T00:00:00Z",
                    "updatedAt": "2024-01-01T00:00:00Z",
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    (dir, home)
}

/// The deployment id `scan` prints for `skill` in the root of `kind`
/// (`universal`, `parked`), under `home`.
fn deployment_id_in_root(home: &Path, skill: &str, kind: &str) -> String {
    let scan = run(&["scan", "--home", home.to_str().unwrap(), "--json"]);
    let found = scan.json["data"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == skill)
        .unwrap_or_else(|| panic!("{skill} missing from scan: {:?}", scan.json));
    found["deployments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["root"]["kind"]["kind"] == kind)
        .unwrap_or_else(|| panic!("{skill} has no {kind} deployment: {found:?}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Flow: `park` the universal `gamma` deployment through the CLI, then hand
/// the parked deployment id `scan` now prints to `unpark`.
/// Expectation: an `ok` envelope naming `gamma`, the directory back under
/// `.agents/skills`, and the Claude Code link recreated.
/// A failure here means the parked half of the park/unpark pair has no
/// working CLI surface, so a parked skill can only be brought back by hand.
#[test]
fn unpark_puts_a_parked_skill_and_its_link_back_or_names_the_envelope() {
    let (_home_dir, home) = parkable_live_home();
    let park = run(&[
        "park",
        "--home",
        home.to_str().unwrap(),
        "--deployment-id",
        &deployment_id_in_root(&home, "gamma", "universal"),
        "--json",
    ]);
    assert_eq!(park.status, 0, "{:?}", park.json);
    assert_eq!(park.json["status"], "ok", "{:?}", park.json);
    assert!(!home.join(".agents/skills/gamma").exists());

    let unpark = run(&[
        "unpark",
        "--home",
        home.to_str().unwrap(),
        "--deployment-id",
        &deployment_id_in_root(&home, "gamma", "parked"),
        "--json",
    ]);

    assert_eq!(unpark.status, 0, "{:?}", unpark.json);
    assert_eq!(unpark.json["status"], "ok", "{:?}", unpark.json);
    assert_eq!(unpark.json["operation"], "unpark", "{:?}", unpark.json);
    assert_eq!(
        unpark.json["data"]["restored_path"],
        serde_json::Value::from(home.join(".agents/skills/gamma").to_str().unwrap()),
        "{:?}",
        unpark.json
    );
    assert!(
        home.join(".agents/skills/gamma/SKILL.md").is_file(),
        "unpark left gamma in the parked root"
    );
    assert!(
        home.join(".claude/skills/gamma").symlink_metadata().is_ok(),
        "unpark did not recreate the Claude Code link park removed"
    );
}

/// Flow: `outdated` over a home holding one skills.sh-owned skill and one
/// skill no ledger claims, with `PATH` emptied on the child so the CLI
/// finds no `gh` and falls back to its `NoGhLookup`.
/// Expectation: an `ok` envelope whose map says `unknown` for the tracked
/// skill - the check could not run - and `not_tracked` for the other.
/// Collapsing those two into one value is the failure this guards: it would
/// either invent a currency for a skill nothing tracks, or report a lookup
/// that never ran as "nothing to know".
#[test]
fn outdated_separates_an_unknown_check_from_an_untracked_skill_or_names_the_currency() {
    let (_home_dir, home) = parkable_live_home();
    let output = Command::new(bin())
        .args(["outdated", "--home", home.to_str().unwrap(), "--json"])
        .env("PATH", "")
        .output()
        .expect("run skill-studio outdated");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not one JSON document: {e}\n{stdout}"));

    assert_eq!(output.status.code(), Some(0), "{json:?}");
    assert_eq!(json["status"], "ok", "{json:?}");
    assert_eq!(json["operation"], "outdated", "{json:?}");
    assert_eq!(
        json["data"]["gamma"]["currency"], "unknown",
        "a skills.sh skill whose lookup could not run is not up to date or behind: {json:?}"
    );
    assert_eq!(
        json["data"]["manual-only"]["currency"], "not_tracked",
        "a skill no install method claims has nothing to check: {json:?}"
    );
}

/// Builds the same `Runtime` `skill-studio --home <home>` would build for
/// itself (matching `ScopeArgs::resolve`'s `--home` branch in
/// `apps/cli/src/scope.rs`: history under `<home>/.skill-studio/history`,
/// leases under `<home>/.skill-studio/leases`), so a test can seed history
/// rows in-process - one direct call per row is far cheaper than spawning
/// the CLI binary `DEFAULT_EVENT_LIMIT + 1` times - while still reading the
/// same on-disk database the CLI subprocess opens for the `undo` under test.
fn cli_runtime_for(home: &Path) -> skill_studio_core::ports::Runtime {
    use skill_studio_core::harness::HarnessCatalog;
    use skill_studio_core::scope::{ProjectSelection, RuntimeScope};
    use std::sync::Arc;

    let data_root = home.join(".skill-studio");
    let history_root = data_root.join("history");
    let db_path = history_root.join("events.sqlite3");
    let mut scope = RuntimeScope::live(home.to_path_buf(), history_root);
    // No projects in this fixture; explicit-empty skips the discovery port
    // this bare runtime never wires up.
    scope.projects = ProjectSelection::Explicit { paths: Vec::new() };
    let catalog = Arc::new(HarnessCatalog::builtin());
    let ports =
        skill_studio_host::default_ports_with_history(data_root.join("leases"), catalog, db_path);
    skill_studio_core::ports::Runtime::new(&scope, ports).unwrap()
}

/// `undo` pages past a default-sized page of unrestorable history to find
/// the one restorable row underneath it: `DEFAULT_EVENT_LIMIT + 1` no-op
/// Claude Code toggles (each recording no inverse, since the link is
/// already in the state asked for) sit on top of the one real toggle that
/// actually moved the link. A single-page read of `list_events` never sees
/// that real toggle, so `undo` must keep paging with `after` until it does.
#[test]
fn undo_finds_the_last_restorable_event_past_the_default_page_or_names_the_event_it_missed() {
    use skill_studio_core::dto::SetHarnessEnabledRequest;
    use skill_studio_core::identity::{AgentId, SkillName};
    use skill_studio_core::ops;
    use skill_studio_core::testing::golden::ctx;

    let home = tempfile::tempdir().unwrap().keep();
    let home = home.canonicalize().unwrap();
    let alpha_dir = home.join(".agents/skills/alpha");
    std::fs::create_dir_all(&alpha_dir).unwrap();
    std::fs::write(
        alpha_dir.join("SKILL.md"),
        b"---\nname: alpha\ndescription: A universal skill.\n---\nBody.\n",
    )
    .unwrap();
    let claude_skills = home.join(".claude/skills");
    std::fs::create_dir_all(&claude_skills).unwrap();
    let link_path = claude_skills.join("alpha");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&alpha_dir, &link_path).unwrap();

    let rt = cli_runtime_for(&home);
    let req = SetHarnessEnabledRequest {
        skill: SkillName("alpha".into()),
        harness: AgentId::from(AgentId::CLAUDE_CODE),
        enabled: false,
        project_path: None,
    };

    // The one restorable row: this toggle actually removes the link.
    ops::set_harness_enabled(&rt, &ctx(), &req).unwrap();
    assert!(
        link_path.symlink_metadata().is_err(),
        "fixture setup: the first disable should have removed the link"
    );

    // Bury it under `DEFAULT_EVENT_LIMIT + 1` no-op repeats: the link is
    // already gone, so each of these records no inverse.
    for _ in 0..=ops::DEFAULT_EVENT_LIMIT {
        ops::set_harness_enabled(&rt, &ctx(), &req).unwrap();
    }
    drop(rt);

    let undo = run(&["undo", "--home", home.to_str().unwrap(), "--json"]);
    assert_eq!(undo.json["status"], "ok", "{:?}", undo.json);
    assert!(
        link_path.symlink_metadata().is_ok(),
        "undo should have paged past the no-op rows and restored the buried link toggle, got {:?}",
        undo.json
    );

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

/// Flow: `--fixture <dir>` scans the `disabled` fixture (epsilon is denied
/// through `.config/opencode/opencode.json` under the fixture) while
/// `XDG_CONFIG_HOME` and `OPENCODE_CONFIG_DIR` are both set on the child
/// process to unrelated real-looking directories, the way a Linux desktop
/// or CI runner (`ubuntu-latest` exports `XDG_CONFIG_HOME`) commonly does.
/// Expectation: the scan still resolves `opencode_config_root` under the
/// fixture (`ScopeArgs::resolve`'s `--fixture` arm now uses
/// `opencode_config_dir_under`, not the env-aware `opencode_config_dir`),
/// so epsilon's `OpenCode` deployment still shows disabled.
/// Failure here would mean a `--fixture` scan on a machine with either
/// variable set reads the real user's `OpenCode` config instead of the
/// fixture's, exactly the bug this test guards against.
#[test]
fn a_fixture_scan_reads_opencode_config_under_the_fixture_even_with_xdg_config_home_set() {
    let home = materialized_fixture("disabled");
    let unrelated_xdg = tempfile::tempdir().unwrap();
    let unrelated_opencode_config_dir = tempfile::tempdir().unwrap();

    let output = Command::new(bin())
        .args(["scan", "--fixture", home.to_str().unwrap(), "--json"])
        .env("XDG_CONFIG_HOME", unrelated_xdg.path())
        .env("OPENCODE_CONFIG_DIR", unrelated_opencode_config_dir.path())
        .output()
        .expect("run skill-studio");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not one JSON document: {e}\n{stdout}"));

    let deployments = json["data"]["skills"]
        .as_array()
        .expect("skills array")
        .iter()
        .flat_map(|skill| skill["deployments"].as_array().unwrap())
        .filter(|deployment| deployment["harness"] == "open-code")
        .collect::<Vec<_>>();
    assert!(
        !deployments.is_empty(),
        "expected an OpenCode deployment in the scan: {json}"
    );
    assert!(
        deployments
            .iter()
            .all(|deployment| deployment["disabled_by"] == "opencode-permission"),
        "expected every OpenCode deployment disabled by the fixture's opencode.json deny rule: {json}"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `add_with_project_path_alone_installs_under_that_project_or_names_the_scope_error`
/// (R1): `--project-path` on its own, with no `--project`, must still resolve
/// the write inside the runtime scope. Before the fix, the runtime scope's
/// `projects` list only ever came from `--project`, so `ScopeFs` rejected the
/// write as "path lies outside the scope" even though `--project-path` named
/// exactly where to write it.
#[test]
fn add_with_project_path_alone_installs_under_that_project_or_names_the_scope_error() {
    let home = materialized_fixture("empty_home");
    let project = tempfile::tempdir().unwrap().keep();
    let source = tempfile::tempdir().unwrap().keep();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: r1-project-path\ndescription: a copy source for the R1 regression\n---\nBody.\n",
    )
    .unwrap();

    let run = run(&[
        "add",
        "--fixture",
        home.to_str().unwrap(),
        "--method",
        "copy",
        "--project-path",
        project.to_str().unwrap(),
        "--name",
        "r1-project-path",
        "--json",
        source.to_str().unwrap(),
    ]);
    assert_eq!(run.status, 0, "expected add to succeed: {:?}", run.json);
    assert_eq!(run.json["status"], "ok", "{:?}", run.json);
    assert!(
        project
            .join(".agents/skills/r1-project-path/SKILL.md")
            .exists(),
        "expected the skill under the project's .agents/skills, got: {:?}",
        run.json
    );

    std::fs::remove_dir_all(&home).ok();
    std::fs::remove_dir_all(&project).ok();
    std::fs::remove_dir_all(&source).ok();
}

/// `add_skills_sh_without_a_name_returns_invalid_request_naming_the_flag`
/// (R2): `skills-sh`/`dotagents` shell out to a CLI that always installs
/// under the given `--skill`/`--name`; a name derived from `source`'s last
/// path segment names the wrong folder for a multi-skill or differently
/// named repo, so the CLI must refuse to guess and name the missing flag.
#[test]
fn add_skills_sh_without_a_name_returns_invalid_request_naming_the_flag() {
    let home = materialized_fixture("empty_home");
    let run = run(&[
        "add",
        "--fixture",
        home.to_str().unwrap(),
        "--method",
        "skills-sh",
        "--json",
        "owner/repo",
    ]);
    assert_eq!(run.status, 2, "{:?}", run.json);
    assert_eq!(run.json["status"], "error", "{:?}", run.json);
    let errors = run.json["errors"].as_array().expect("errors array");
    assert!(
        errors
            .iter()
            .any(|e| e["code"] == "invalid_request"
                && e["message"].as_str().unwrap().contains("--name")),
        "expected an invalid_request error naming --name: {errors:?}"
    );
    std::fs::remove_dir_all(&home).ok();
}

/// `add_with_an_unknown_harness_returns_invalid_request_naming_the_value`
/// (R5): a harness id that is syntactically kebab-case but not in
/// `HarnessCatalog` (a typo, or a harness this build never shipped) must be
/// rejected before `ops::install` runs, not silently ignored.
#[test]
fn add_with_an_unknown_harness_returns_invalid_request_naming_the_value() {
    let home = materialized_fixture("empty_home");
    let source = tempfile::tempdir().unwrap().keep();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: r5-unknown-harness\ndescription: a copy source for the R5 regression\n---\nBody.\n",
    )
    .unwrap();

    let run = run(&[
        "add",
        "--fixture",
        home.to_str().unwrap(),
        "--method",
        "copy",
        "--harness",
        "not-a-real-harness",
        "--name",
        "r5-unknown-harness",
        "--json",
        source.to_str().unwrap(),
    ]);
    assert_eq!(run.status, 2, "{:?}", run.json);
    assert_eq!(run.json["status"], "error", "{:?}", run.json);
    let errors = run.json["errors"].as_array().expect("errors array");
    assert!(
        errors.iter().any(|e| e["code"] == "invalid_request"
            && e["message"]
                .as_str()
                .unwrap()
                .contains("not-a-real-harness")),
        "expected an invalid_request error naming the unknown harness: {errors:?}"
    );
    assert!(
        !home.join(".agents/skills/r5-unknown-harness").exists(),
        "an unknown harness must fail before any write"
    );
    std::fs::remove_dir_all(&home).ok();
    std::fs::remove_dir_all(&source).ok();
}

/// `add_copy_with_a_symlinked_file_copies_it_as_a_file_or_names_the_missing_path`
/// (R6): `read_skill_files` used `DirEntry::file_type`, which is
/// `lstat`-based and reports a symlink as neither a file nor a directory, so
/// a symlinked file inside the source folder silently dropped out of the
/// copy. `std::fs::metadata` follows the link and reports what it points at.
#[test]
#[cfg(unix)]
fn add_copy_with_a_symlinked_file_copies_it_as_a_file_or_names_the_missing_path() {
    let home = materialized_fixture("empty_home");
    let source = tempfile::tempdir().unwrap().keep();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: r6-symlink\ndescription: a copy source for the R6 regression\n---\nBody.\n",
    )
    .unwrap();
    std::fs::write(source.join("reference.md"), "Reference body.\n").unwrap();
    std::os::unix::fs::symlink(
        source.join("reference.md"),
        source.join("reference-link.md"),
    )
    .unwrap();

    let run = run(&[
        "add",
        "--fixture",
        home.to_str().unwrap(),
        "--method",
        "copy",
        "--name",
        "r6-symlink",
        "--json",
        source.to_str().unwrap(),
    ]);
    assert_eq!(run.status, 0, "expected add to succeed: {:?}", run.json);
    let copied = home.join(".agents/skills/r6-symlink/reference-link.md");
    assert!(
        copied.is_file(),
        "expected the symlinked file to be copied as a file, got: {:?}",
        run.json
    );
    assert_eq!(
        std::fs::read_to_string(&copied).unwrap(),
        "Reference body.\n",
        "the symlinked file's bytes must match its target"
    );

    std::fs::remove_dir_all(&home).ok();
    std::fs::remove_dir_all(&source).ok();
}
