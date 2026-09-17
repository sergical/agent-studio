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
/// network, or `SQLite` work: runs `f` on a blocking-pool thread via
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
    let result = join_result_to_err(command, tauri::async_runtime::spawn_blocking(f).await);
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
    command: &str,
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
        let result = join_result_to_err("cmd", joined);
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

    /// Names allowed to stay a sync `pub fn` command, with the reason each
    /// one never blocks the main thread for long.
    const SYNC_ALLOWLIST: &[(&str, &str)] = &[
        ("get_skill_snapshot", "in-memory state only"),
        ("request_skill_rescan", "in-memory state only"),
        ("get_agent_targets", "in-memory state only"),
        ("cancel_skill_agent_run", "in-memory state only"),
        ("start_add_skill_operation", "in-memory state only"),
        ("start_add_skills_operation", "in-memory state only"),
        ("get_add_skill_operation", "in-memory state only"),
        ("cancel_add_skill_operation", "in-memory state only"),
        (
            "open_skill_path",
            "#[tauri::command(async)], Tauri dispatches it off the main thread",
        ),
        (
            "set_preferred_editor",
            "#[tauri::command(async)], Tauri dispatches it off the main thread",
        ),
    ];

    /// Every `.rs` file under `dir`, recursively.
    fn rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                rs_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// The function name out of a `pub [async] fn <name>(` line, or `None`
    /// when the line isn't a function signature at all (e.g. a doc comment
    /// sitting between the attribute and the fn).
    fn fn_name(line: &str) -> Option<&str> {
        let after_fn = line
            .strip_prefix("pub async fn ")
            .or_else(|| line.strip_prefix("pub fn "))?;
        after_fn.split(['(', '<', ' ']).next()
    }

    /// A new command that does file, process, or database work on the main
    /// thread fails this test.
    #[test]
    fn every_tauri_command_is_async_unless_allowlisted() {
        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rs_files(&src_dir, &mut files);

        let mut failures = Vec::new();
        for file in &files {
            let source = std::fs::read_to_string(file)
                .unwrap_or_else(|e| panic!("could not read {}: {e}", file.display()));
            let lines: Vec<&str> = source.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let trimmed = line.trim();
                let is_async_attr = trimmed == "#[tauri::command(async)]";
                if trimmed != "#[tauri::command]" && !is_async_attr {
                    continue;
                }
                let Some(fn_line) = lines[i + 1..]
                    .iter()
                    .map(|l| l.trim())
                    .find(|l| !l.is_empty())
                else {
                    continue;
                };
                let Some(name) = fn_name(fn_line) else {
                    continue;
                };
                if fn_line.starts_with("pub async fn ") {
                    continue;
                }
                match SYNC_ALLOWLIST.iter().find(|(n, _)| *n == name) {
                    Some((_, reason)) if *reason != "in-memory state only" && !is_async_attr => {
                        failures.push(format!(
                            "{}: {name} is allowlisted as {reason} but is not `#[tauri::command(async)]`",
                            file.display()
                        ));
                    }
                    Some(_) => {}
                    None => failures.push(format!(
                        "{}: {name} is a sync `#[tauri::command]` and not in SYNC_ALLOWLIST; \
                         file/process/database work must be `pub async fn`",
                        file.display()
                    )),
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
