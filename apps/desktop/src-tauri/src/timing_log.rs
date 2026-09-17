// ============================================================================
// Skill Studio - timing_log
// Appends one JSON line per Tauri command call to `timing.jsonl` in the app
// data dir, rotating to `timing.prev.jsonl` at a size threshold. Action-map
// perf work (`docs/action-map/performance.md`) reads this file to see which
// commands are slow on a real install, the same way core's `--time` flag
// reports per-step timing for the CLI.
// ============================================================================

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use skill_studio_core::timing::StepTiming;
use tauri::{AppHandle, Manager};

/// File size, in bytes, past which [`record_command`] rotates
/// `timing.jsonl` before appending. `record_command` itself always passes
/// this constant; [`append_record`] takes it as a parameter so tests can
/// exercise rotation without writing 5 MB of fixture data.
pub const ROTATE_AT_BYTES: u64 = 5 * 1024 * 1024;

/// One `timing.jsonl` line.
#[derive(Debug, Serialize)]
struct TimingRecord<'a> {
    ts: String,
    command: &'a str,
    elapsed_ms: u64,
    steps: &'a [StepTiming],
    thread: &'a str,
}

/// Times a synchronous `#[tauri::command]` body and appends one record for
/// it, whether `f` returns `Ok` or `Err` - a slow error path is exactly the
/// kind of thing this log exists to surface.
pub fn time_command<T>(app: &AppHandle, command: &str, f: impl FnOnce() -> T) -> T {
    let start = std::time::Instant::now();
    let result = f();
    record_command(
        app,
        command,
        start.elapsed().as_millis() as u64,
        &[],
        "main",
    );
    result
}

/// As [`time_command`], for an async `#[tauri::command]` body: `fut` runs on
/// Tauri's async runtime, off the main thread, so this records `"worker"`.
pub async fn time_command_async<T>(
    app: &AppHandle,
    command: &str,
    fut: impl std::future::Future<Output = T>,
) -> T {
    let start = std::time::Instant::now();
    let result = fut.await;
    record_command(
        app,
        command,
        start.elapsed().as_millis() as u64,
        &[],
        "worker",
    );
    result
}

/// As [`time_command`], for a sync command body that does file, process,
/// network, or SQLite work: runs `f` on a blocking-pool thread via
/// `tauri::async_runtime::spawn_blocking` so it never stalls the main thread
/// or a Tokio worker, and records `"worker"`. `f` returns `Result<T, String>`
/// (the convention every command already follows) so a panic inside the
/// blocking closure - e.g. a poisoned mutex - becomes an `Err` carrying the
/// panic message instead of propagating as an unwind across the task
/// boundary.
pub async fn time_command_blocking<T: Send + 'static>(
    app: &AppHandle,
    command: &str,
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let start = std::time::Instant::now();
    let command_owned = command.to_string();
    let result = join_result_to_err(command_owned, tauri::async_runtime::spawn_blocking(f).await);
    record_command(
        app,
        command,
        start.elapsed().as_millis() as u64,
        &[],
        "worker",
    );
    result
}

/// The panic-to-`Err` conversion [`time_command_blocking`] applies to a
/// `spawn_blocking` join result: an `Ok(result)` passes through unchanged, a
/// `JoinError` (the task panicked or was cancelled) becomes an `Err`
/// carrying the panic message instead of a `spawn_blocking(..).await.unwrap()`
/// that would itself panic on the calling thread.
fn join_result_to_err<T>(
    command: String,
    joined: Result<Result<T, String>, tauri::Error>,
) -> Result<T, String> {
    joined.unwrap_or_else(|join_error| Err(format!("{command} panicked: {join_error}")))
}

/// Appends one record to `<app_data_dir>/timing.jsonl`, rotating first if
/// the file has grown past [`ROTATE_AT_BYTES`]. Best-effort: a failure to
/// resolve the app data dir or to write is logged to stderr and otherwise
/// ignored, matching `open_event_store`'s "never abort the command over a
/// logging failure" rule.
pub fn record_command(
    app: &AppHandle,
    command: &str,
    elapsed_ms: u64,
    steps: &[StepTiming],
    thread: &str,
) {
    let Ok(app_data) = app.path().app_data_dir() else {
        return;
    };
    if let Err(error) = append_record(
        &app_data,
        command,
        elapsed_ms,
        steps,
        thread,
        ROTATE_AT_BYTES,
    ) {
        eprintln!("[timing_log] failed to record {command}: {error}");
    }
}

/// The rotation/append logic [`record_command`] drives, with the app data
/// dir and rotation threshold as plain parameters so tests can point it at
/// a temp dir and a small threshold.
fn append_record(
    app_data: &Path,
    command: &str,
    elapsed_ms: u64,
    steps: &[StepTiming],
    thread: &str,
    rotate_at_bytes: u64,
) -> std::io::Result<()> {
    std::fs::create_dir_all(app_data)?;
    let log_path = app_data.join("timing.jsonl");
    rotate_if_needed(&log_path, app_data, rotate_at_bytes)?;

    let record = TimingRecord {
        ts: chrono::Utc::now().to_rfc3339(),
        command,
        elapsed_ms,
        steps,
        thread,
    };
    let line = serde_json::to_string(&record).unwrap_or_default();

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Renames `timing.jsonl` to `timing.prev.jsonl` (overwriting any earlier
/// `timing.prev.jsonl`, so only one previous file is ever kept) when it has
/// grown past `rotate_at_bytes`, leaving the next [`append_record`] call to
/// create a fresh `timing.jsonl`.
fn rotate_if_needed(log_path: &Path, app_data: &Path, rotate_at_bytes: u64) -> std::io::Result<()> {
    let size = match std::fs::metadata(log_path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if size < rotate_at_bytes {
        return Ok(());
    }
    let prev_path = app_data.join("timing.prev.jsonl");
    match std::fs::rename(log_path, prev_path) {
        Ok(()) => Ok(()),
        // A concurrent rotation already moved it.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::BufRead;

    fn read_lines(path: &Path) -> Vec<serde_json::Value> {
        let file = File::open(path).unwrap();
        std::io::BufReader::new(file)
            .lines()
            .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn join_result_to_err_converts_a_panic_join_error_into_err_carrying_the_panic_message() {
        let joined: Result<Result<(), String>, tauri::Error> =
            tauri::async_runtime::spawn_blocking(|| -> Result<(), String> { panic!("boom") }).await;
        let result = join_result_to_err("cmd".to_string(), joined);
        let error = result.unwrap_err();
        assert!(error.contains("cmd panicked"));
        assert!(
            error.contains("boom"),
            "expected the panic payload text in the error, got: {error}"
        );
    }

    #[test]
    fn writes_and_reads_back_two_records() {
        let temp = tempfile::tempdir().unwrap();
        append_record(temp.path(), "scan", 12, &[], "main", ROTATE_AT_BYTES).unwrap();
        append_record(
            temp.path(),
            "add_skill",
            34,
            &[StepTiming {
                name: "write".into(),
                elapsed_ms: 20,
            }],
            "worker",
            ROTATE_AT_BYTES,
        )
        .unwrap();

        let rows = read_lines(&temp.path().join("timing.jsonl"));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["command"], "scan");
        assert_eq!(rows[0]["elapsed_ms"], 12);
        assert_eq!(rows[0]["thread"], "main");
        assert!(rows[0]["steps"].as_array().unwrap().is_empty());
        assert_eq!(rows[1]["command"], "add_skill");
        assert_eq!(rows[1]["steps"][0]["name"], "write");
        assert!(rows[0]["ts"].as_str().unwrap().contains('T'));
    }

    #[test]
    fn rotates_at_threshold_and_keeps_one_previous_file() {
        let temp = tempfile::tempdir().unwrap();
        // A threshold small enough that the very first record trips it on
        // the *second* append (rotation runs before the write, so the
        // first record establishes the file that then rotates away).
        let threshold = 10;
        append_record(temp.path(), "one", 1, &[], "main", threshold).unwrap();
        let first_size = std::fs::metadata(temp.path().join("timing.jsonl"))
            .unwrap()
            .len();
        assert!(
            first_size >= threshold,
            "fixture record must trip the threshold"
        );

        append_record(temp.path(), "two", 2, &[], "main", threshold).unwrap();

        let prev_path = temp.path().join("timing.prev.jsonl");
        let current_path = temp.path().join("timing.jsonl");
        assert!(prev_path.exists());
        assert!(current_path.exists());

        let prev_rows = read_lines(&prev_path);
        assert_eq!(prev_rows.len(), 1);
        assert_eq!(prev_rows[0]["command"], "one");

        let current_rows = read_lines(&current_path);
        assert_eq!(current_rows.len(), 1);
        assert_eq!(current_rows[0]["command"], "two");

        // A second rotation must not pile up more than one previous file.
        append_record(temp.path(), "three", 3, &[], "main", threshold).unwrap();
        let prev_rows = read_lines(&prev_path);
        assert_eq!(prev_rows.len(), 1);
        assert_eq!(prev_rows[0]["command"], "two");
    }

    /// Unit 0.3's "no command over 16 ms on the main thread" acceptance check
    /// only holds if every `#[tauri::command]` that does file/process/
    /// network/SQLite work stayed `async fn` (and so routes through
    /// `time_command_blocking`/`time_command_async`, never `time_command`).
    /// A true compile-time registry isn't practical here without duplicating
    /// every command's argument list, so this checks that the source has
    /// `#[tauri::command]` on the line immediately before `pub async fn
    /// <name>(` for each name below, and fails if any of them regresses to
    /// sync (or is renamed/removed without updating this list). This list
    /// must cover every async command, not just the ones converted in one
    /// pass, so a future regression on any of them is caught here too.
    #[test]
    fn every_async_command_still_has_tauri_command_and_async_fn() {
        let must_be_async = [
            ("add_method_defaults.rs", "get_add_method_defaults"),
            ("commands.rs", "get_skills_sh_access"),
            ("commands.rs", "set_skills_sh_api_key"),
            ("commands.rs", "search_skills"),
            ("commands.rs", "get_popular_skills"),
            ("commands.rs", "get_skill_details"),
            ("commands.rs", "get_installed_skills"),
            ("commands.rs", "list_skill_projects"),
            ("commands.rs", "is_skill_installed"),
            ("commands.rs", "remove_skill"),
            ("commands.rs", "read_installed_skill_md"),
            ("commands.rs", "write_installed_skill_md"),
            ("commands.rs", "write_installed_skill_md_if_unchanged"),
            ("commands.rs", "get_editor_choices"),
            ("commands.rs", "update_skill"),
            ("commands.rs", "set_plugin_enabled"),
            ("commands.rs", "uninstall_plugin"),
            ("github_skill_listing.rs", "list_github_skills"),
            ("skill_agent_runner.rs", "start_skill_agent_run"),
            ("skill_agent_runner.rs", "create_skill_scratch_dir"),
            ("skill_agent_runner.rs", "remove_skill_scratch_dir"),
            ("skill_add_operation.rs", "confirm_add_skill_trust"),
            ("skill_add.rs", "add_skill"),
            ("skill_add.rs", "add_skills"),
            ("skill_harness_disable.rs", "set_harness_enabled"),
            ("skill_harness_disable.rs", "set_deployment_enabled"),
            ("skill_invocation.rs", "set_skill_invocation"),
            ("skill_fork.rs", "fork_skill"),
            ("skill_fork.rs", "pull_fork_upstream"),
            ("skill_fork.rs", "unfork_skill"),
            (
                "skill_frontmatter_repair.rs",
                "preview_skill_frontmatter_repair",
            ),
            (
                "skill_frontmatter_repair.rs",
                "apply_skill_frontmatter_repair",
            ),
            ("skill_pack.rs", "create_skill_pack"),
            ("skill_pack.rs", "update_skill_pack"),
            ("skill_pack.rs", "publish_skill_pack"),
            ("skill_pack.rs", "delete_skill_pack"),
            ("skill_pack.rs", "import_skill_pack"),
            ("skill_pack.rs", "confirm_skill_pack_trust"),
            ("skill_pack.rs", "abandon_pack_import_trust"),
            ("skill_pack.rs", "list_skill_packs"),
            ("skill_run_target.rs", "prepare_skill_run_target"),
            ("skill_run_target.rs", "reveal_skill_run_target"),
            ("skill_run_target.rs", "skill_run_target_diff"),
            ("skill_run_target.rs", "apply_skill_run_target_diff"),
            ("skill_run_target.rs", "discard_skill_run_target"),
            ("skill_project_folders.rs", "list_project_folders"),
            ("skill_park.rs", "park_skill"),
            ("skill_park.rs", "unpark_skill"),
            ("skill_trial.rs", "keep_skill_trial"),
            ("skill_trial.rs", "restore_trashed_skill"),
            ("skill_run_history.rs", "record_skill_run"),
            ("skill_run_history.rs", "list_skill_runs"),
            ("skill_run_history.rs", "read_skill_run_events"),
            ("skill_refresh.rs", "get_tracked_projects"),
            ("skill_refresh.rs", "register_skill_projects"),
            ("skill_refresh.rs", "unregister_skill_project"),
            ("skill_refresh.rs", "remove_skill_project"),
            ("skill_refresh.rs", "import_tracked_projects"),
            ("skill_refresh.rs", "get_discovery_sources"),
            ("skill_refresh.rs", "set_discovery_source"),
            ("skill_update_check.rs", "check_skill_updates_now"),
            ("event_commands.rs", "list_skill_events"),
            ("event_commands.rs", "restore_skill_event"),
            ("event_commands.rs", "make_skill_independent_copy"),
            ("event_commands.rs", "set_shared_harness_skill_enabled"),
            ("event_commands.rs", "materialize_harness_root"),
            ("event_commands.rs", "materialize_harness_root_then_disable"),
            ("event_commands.rs", "repair_skill_link"),
        ];

        let skills_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/skills");
        for (file, name) in must_be_async {
            let source = std::fs::read_to_string(skills_dir.join(file))
                .unwrap_or_else(|e| panic!("could not read {file}: {e}"));
            let attributed_async_marker = format!("#[tauri::command]\npub async fn {name}(");
            assert!(
                source.contains(&attributed_async_marker),
                "{file}::{name} must be a `#[tauri::command]` immediately followed by `pub async fn {name}(` (does file/process/network/SQLite work)"
            );
        }
    }
}
