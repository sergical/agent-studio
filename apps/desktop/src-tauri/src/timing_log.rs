// ============================================================================
// Skill Studio - timing_log
// Appends one JSON line per Tauri command call to `timing.jsonl` in the app
// data dir, rotating to `timing.prev.jsonl` at a size threshold. Action-map
// perf work (`docs/action-map/performance.md`) reads this file to see which
// commands are slow on a real install, the same way core's `--time` flag
// reports per-step timing for the CLI.
// ============================================================================

use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use skill_studio_core::health::{Outcome, TimingRow};
use skill_studio_core::timing::StepTiming;
use tauri::{AppHandle, Manager};

use crate::skills::agents::AgentTarget;
use crate::skills::skill_refresh::SkillSnapshot;

/// File size, in bytes, past which [`record_command`] rotates
/// `timing.jsonl` before appending. `record_command` itself always passes
/// this constant; [`append_record`] takes it as a parameter so tests can
/// exercise rotation without writing 5 MB of fixture data.
pub const ROTATE_AT_BYTES: u64 = 5 * 1024 * 1024;

/// One `timing.jsonl` line.
#[derive(Debug, Serialize, Deserialize)]
struct TimingRecord<'a> {
    ts: String,
    #[serde(borrow)]
    command: std::borrow::Cow<'a, str>,
    elapsed_ms: u64,
    #[serde(default)]
    steps: Vec<StepTiming>,
    #[serde(borrow)]
    thread: std::borrow::Cow<'a, str>,
    /// `"ok"` or `"error"` - see [`CommandOutcome`].
    #[serde(borrow)]
    outcome: std::borrow::Cow<'a, str>,
    /// The error's first line, capped at 120 chars, when `outcome` is `"error"`.
    #[serde(default)]
    error: Option<String>,
}

/// How a `#[tauri::command]` body finished, for the `outcome`/`error`
/// fields [`record_command`] writes. `Result<T, E>` reports its own
/// `Ok`/`Err`; the handful of commands with no failure path (a bare `Vec`,
/// `Option`, or `()` return) are always `"ok"` - listed explicitly here
/// rather than as a blanket `impl<T> CommandOutcome for T`, so a future
/// command that starts returning a type with a real failure path doesn't
/// silently inherit an "always ok" impl instead of using `Result`.
pub trait CommandOutcome {
    /// `("ok", None)`, or `("error", Some(first line of the error, capped
    /// at 120 chars))`.
    fn outcome(&self) -> (&'static str, Option<String>);
}

impl<T, E: std::fmt::Display> CommandOutcome for Result<T, E> {
    fn outcome(&self) -> (&'static str, Option<String>) {
        match self {
            Ok(_) => ("ok", None),
            Err(error) => ("error", Some(first_line_capped(&error.to_string()))),
        }
    }
}

impl CommandOutcome for Vec<AgentTarget> {
    fn outcome(&self) -> (&'static str, Option<String>) {
        ("ok", None)
    }
}

impl CommandOutcome for Option<SkillSnapshot> {
    fn outcome(&self) -> (&'static str, Option<String>) {
        ("ok", None)
    }
}

impl CommandOutcome for () {
    fn outcome(&self) -> (&'static str, Option<String>) {
        ("ok", None)
    }
}

/// The first line of `text`, capped at 120 `char`s - `timing.jsonl` is one
/// JSON object per line, so a multi-line error (a stack trace, a long
/// diagnostic) would otherwise break that invariant.
fn first_line_capped(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or("");
    first_line.chars().take(120).collect()
}

/// Times a synchronous `#[tauri::command]` body and appends one record for
/// it, whether `f` returns `Ok` or `Err` - a slow error path is exactly the
/// kind of thing this log exists to surface.
pub fn time_command<T: CommandOutcome>(app: &AppHandle, command: &str, f: impl FnOnce() -> T) -> T {
    let start = std::time::Instant::now();
    let result = f();
    let (outcome, error) = result.outcome();
    record_command(
        app,
        command,
        start.elapsed().as_millis() as u64,
        &[],
        "main",
        outcome,
        error,
    );
    result
}

/// As [`time_command`], for an async `#[tauri::command]` body: `fut` runs on
/// Tauri's async runtime, off the main thread, so this records `"worker"`.
pub async fn time_command_async<T: CommandOutcome>(
    app: &AppHandle,
    command: &str,
    fut: impl std::future::Future<Output = T>,
) -> T {
    let start = std::time::Instant::now();
    let result = fut.await;
    let (outcome, error) = result.outcome();
    record_command(
        app,
        command,
        start.elapsed().as_millis() as u64,
        &[],
        "worker",
        outcome,
        error,
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
    let (outcome, error) = result.outcome();
    record_command(
        app,
        command,
        start.elapsed().as_millis() as u64,
        &[],
        "worker",
        outcome,
        error,
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
    outcome: &str,
    error: Option<String>,
) {
    let Ok(app_data) = app.path().app_data_dir() else {
        return;
    };
    if let Err(io_error) = append_record(
        &app_data,
        command,
        elapsed_ms,
        steps,
        thread,
        outcome,
        error,
        ROTATE_AT_BYTES,
    ) {
        eprintln!("[timing_log] failed to record {command}: {io_error}");
    }
}

/// The rotation/append logic [`record_command`] drives, with the app data
/// dir and rotation threshold as plain parameters so tests can point it at
/// a temp dir and a small threshold.
#[allow(clippy::too_many_arguments)]
fn append_record(
    app_data: &Path,
    command: &str,
    elapsed_ms: u64,
    steps: &[StepTiming],
    thread: &str,
    outcome: &str,
    error: Option<String>,
    rotate_at_bytes: u64,
) -> std::io::Result<()> {
    std::fs::create_dir_all(app_data)?;
    let log_path = app_data.join("timing.jsonl");
    rotate_if_needed(&log_path, app_data, rotate_at_bytes)?;

    let record = TimingRecord {
        ts: chrono::Utc::now().to_rfc3339(),
        command: command.into(),
        elapsed_ms,
        steps: steps.to_vec(),
        thread: thread.into(),
        outcome: outcome.into(),
        error,
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

/// Rows older than this are dropped from `timing.jsonl` on each process
/// start ([`trim_on_open`]) - matches unit 6.5's 30-day retention.
const RETAIN: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);

/// Reads every parsable line of `<app_data_dir>/timing.jsonl` into a
/// [`TimingRow`], for [`skill_studio_core::health::health_rollup`]. A line
/// that fails to parse (a partial write, an older log schema before this
/// unit added `outcome`/`error`) is skipped rather than aborting the whole
/// read - `command_health` should never fail just because one old line is
/// unreadable.
pub fn read_rows(app: &AppHandle) -> Vec<TimingRow> {
    let Ok(app_data) = app.path().app_data_dir() else {
        return Vec::new();
    };
    read_rows_from(&app_data.join("timing.jsonl"))
}

fn read_rows_from(log_path: &Path) -> Vec<TimingRow> {
    let Ok(file) = std::fs::File::open(log_path) else {
        return Vec::new();
    };
    std::io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| record_to_row(&line))
        .collect()
}

fn record_to_row(line: &str) -> Option<TimingRow> {
    let record: TimingRecord = serde_json::from_str(line).ok()?;
    let ts = chrono::DateTime::parse_from_rfc3339(&record.ts)
        .ok()?
        .with_timezone(&chrono::Utc);
    let outcome = if record.outcome.as_ref() == "error" {
        Outcome::Error
    } else {
        Outcome::Ok
    };
    Some(TimingRow {
        ts,
        command: record.command.into_owned(),
        elapsed_ms: record.elapsed_ms,
        outcome,
        error: record.error,
    })
}

/// Rewrites `timing.jsonl` with rows older than [`RETAIN`] dropped, via
/// [`skill_studio_core::health::trim_rows`], so the log stays bounded across
/// a long-running install. Called once from `setup()` (see `lib.rs`), off
/// the main thread. A line that doesn't parse into a [`TimingRow`] is kept
/// unconditionally - there's no dated row to hand `trim_rows` a decision
/// about, so it's left for a future line to overwrite through the normal
/// [`ROTATE_AT_BYTES`] rotation instead of guessed away here.
pub fn trim_on_open(app: &AppHandle) {
    let Ok(app_data) = app.path().app_data_dir() else {
        return;
    };
    if let Err(error) = trim_log_file(&app_data.join("timing.jsonl"), chrono::Utc::now(), RETAIN) {
        eprintln!("[timing_log] failed to trim: {error}");
    }
}

fn trim_log_file(
    log_path: &Path,
    now: chrono::DateTime<chrono::Utc>,
    retain: std::time::Duration,
) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(log_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let lines: Vec<&str> = text.lines().collect();
    // One-row-at-a-time so the same `trim_rows` a caller passes a whole log
    // to decides each line's fate individually, while this rewrite keeps
    // every field of the original line (`steps`, `thread`) that `TimingRow`
    // itself doesn't carry.
    let kept: Vec<&str> = lines
        .iter()
        .filter(|line| match record_to_row(line) {
            Some(row) => !skill_studio_core::health::trim_rows(vec![row], now, retain).is_empty(),
            None => true,
        })
        .copied()
        .collect();
    if kept.len() == lines.len() {
        return Ok(()); // nothing dated old enough to drop
    }
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_path)?;
    for line in kept {
        writeln!(file, "{line}")?;
    }
    Ok(())
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
        append_record(
            temp.path(),
            "scan",
            12,
            &[],
            "main",
            "ok",
            None,
            ROTATE_AT_BYTES,
        )
        .unwrap();
        append_record(
            temp.path(),
            "add_skill",
            34,
            &[StepTiming {
                name: "write".into(),
                elapsed_ms: 20,
            }],
            "worker",
            "error",
            Some("disk full".to_string()),
            ROTATE_AT_BYTES,
        )
        .unwrap();

        let rows = read_lines(&temp.path().join("timing.jsonl"));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["command"], "scan");
        assert_eq!(rows[0]["elapsed_ms"], 12);
        assert_eq!(rows[0]["thread"], "main");
        assert_eq!(rows[0]["outcome"], "ok");
        assert!(rows[0]["error"].is_null());
        assert!(rows[0]["steps"].as_array().unwrap().is_empty());
        assert_eq!(rows[1]["command"], "add_skill");
        assert_eq!(rows[1]["steps"][0]["name"], "write");
        assert_eq!(rows[1]["outcome"], "error");
        assert_eq!(rows[1]["error"], "disk full");
        assert!(rows[0]["ts"].as_str().unwrap().contains('T'));
    }

    #[test]
    fn rotates_at_threshold_and_keeps_one_previous_file() {
        let temp = tempfile::tempdir().unwrap();
        // A threshold small enough that the very first record trips it on
        // the *second* append (rotation runs before the write, so the
        // first record establishes the file that then rotates away).
        let threshold = 10;
        append_record(temp.path(), "one", 1, &[], "main", "ok", None, threshold).unwrap();
        let first_size = std::fs::metadata(temp.path().join("timing.jsonl"))
            .unwrap()
            .len();
        assert!(
            first_size >= threshold,
            "fixture record must trip the threshold"
        );

        append_record(temp.path(), "two", 2, &[], "main", "ok", None, threshold).unwrap();

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
        append_record(temp.path(), "three", 3, &[], "main", "ok", None, threshold).unwrap();
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

    #[test]
    fn command_outcome_reports_ok_for_ok_and_the_capped_first_error_line_for_err() {
        let ok: Result<u32, String> = Ok(7);
        assert_eq!(ok.outcome(), ("ok", None));

        let multi_line_err: Result<u32, String> = Err("boom\nstack trace line two".to_string());
        assert_eq!(
            multi_line_err.outcome(),
            ("error", Some("boom".to_string()))
        );

        let long = "x".repeat(200);
        let long_err: Result<u32, String> = Err(long.clone());
        let (outcome, error) = long_err.outcome();
        assert_eq!(outcome, "error");
        assert_eq!(error.unwrap().chars().count(), 120);

        assert_eq!(Vec::<AgentTarget>::new().outcome(), ("ok", None));
        assert_eq!(None::<SkillSnapshot>.outcome(), ("ok", None));
        assert_eq!(().outcome(), ("ok", None));
    }

    #[test]
    fn read_rows_skips_unparsable_lines_and_carries_outcome_and_error_through() {
        let temp = tempfile::tempdir().unwrap();
        let log_path = temp.path().join("timing.jsonl");
        append_record(
            temp.path(),
            "scan",
            12,
            &[],
            "main",
            "ok",
            None,
            ROTATE_AT_BYTES,
        )
        .unwrap();
        append_record(
            temp.path(),
            "add_skill",
            34,
            &[],
            "worker",
            "error",
            Some("disk full".to_string()),
            ROTATE_AT_BYTES,
        )
        .unwrap();
        // A line that isn't even JSON, as a partial write would leave.
        let mut file = OpenOptions::new().append(true).open(&log_path).unwrap();
        writeln!(file, "not json").unwrap();

        let rows = read_rows_from(&log_path);
        assert_eq!(rows.len(), 2, "the unparsable third line is skipped");
        assert_eq!(rows[0].command, "scan");
        assert_eq!(rows[0].outcome, Outcome::Ok);
        assert_eq!(rows[0].error, None);
        assert_eq!(rows[1].command, "add_skill");
        assert_eq!(rows[1].outcome, Outcome::Error);
        assert_eq!(rows[1].error, Some("disk full".to_string()));
    }

    #[test]
    fn trim_log_file_drops_rows_older_than_thirty_days_and_keeps_the_rest_and_the_unparsable_line()
    {
        let temp = tempfile::tempdir().unwrap();
        let log_path = temp.path().join("timing.jsonl");
        let now = chrono::Utc::now();
        let old_ts = (now - chrono::Duration::days(45)).to_rfc3339();
        let recent_ts = (now - chrono::Duration::days(1)).to_rfc3339();
        std::fs::write(
            &log_path,
            format!(
                "{{\"ts\":\"{old_ts}\",\"command\":\"scan\",\"elapsed_ms\":1,\"steps\":[],\"thread\":\"main\",\"outcome\":\"ok\",\"error\":null}}\n\
                 {{\"ts\":\"{recent_ts}\",\"command\":\"scan\",\"elapsed_ms\":2,\"steps\":[],\"thread\":\"main\",\"outcome\":\"ok\",\"error\":null}}\n\
                 not json\n"
            ),
        )
        .unwrap();

        trim_log_file(&log_path, now, RETAIN).unwrap();

        let text = std::fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "the 45-day-old row is dropped");
        assert!(lines[0].contains(&recent_ts));
        assert_eq!(
            lines[1], "not json",
            "an unparsable line survives - there's no dated row to judge it by"
        );
    }
}
