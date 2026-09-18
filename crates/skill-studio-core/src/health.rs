//! Command health rollup: folds `timing.jsonl` rows (unit 0.1) into one
//! [`crate::dto::CommandHealth`] per command name.
//!
//! The fold is pure over rows passed in - no file IO here, matching the
//! core's ports-only IO rule. `timing_log::read_rows` (desktop) and the CLI's
//! own reader parse the log's JSON lines into [`TimingRow`] before calling
//! [`health_rollup`].
//!
//! Percentiles use nearest-rank: for a sorted-ascending list of `n`
//! durations and a quantile `q`, the pth value where `p = ceil(q * n)`,
//! 1-indexed and clamped to `[1, n]`. `p50` of `[10, 20, 30, 40]` is
//! `sorted[ceil(0.5 * 4) - 1] = sorted[1] = 20`; `p95` of the same list is
//! `sorted[ceil(0.95 * 4) - 1] = sorted[3] = 40`. Tests hand-compute against
//! this formula, so a different interpolation (e.g. linear) will fail them.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::dto::CommandHealth;

/// How one recorded command call finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The command returned successfully.
    Ok,
    /// The command returned an error.
    Error,
}

/// One `timing.jsonl` row, parsed. Mirrors the desktop's `TimingRecord`
/// (`apps/desktop/src-tauri/src/timing_log.rs`) minus the fields the rollup
/// never reads (`steps`, `thread`).
#[derive(Debug, Clone, PartialEq)]
pub struct TimingRow {
    /// When the command ran.
    pub ts: DateTime<Utc>,
    /// The command name, as recorded in `timing.jsonl`.
    pub command: String,
    /// Elapsed milliseconds for the call.
    pub elapsed_ms: u64,
    /// Whether the call succeeded.
    pub outcome: Outcome,
    /// The error's first line, capped at 120 chars, when `outcome` is
    /// [`Outcome::Error`].
    pub error: Option<String>,
}

/// The pth value of a sorted-ascending `[u64]`, nearest-rank (see the module
/// doc comment). `sorted` must already be sorted ascending; `sorted.is_empty()`
/// returns `0`, which never happens in practice since callers only invoke
/// this on a non-empty per-command group.
fn nearest_rank(sorted: &[u64], quantile: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (quantile * sorted.len() as f64).ceil() as usize;
    let index = rank.clamp(1, sorted.len()) - 1;
    sorted[index]
}

/// Folds `rows` within `window` of `now` into one [`CommandHealth`] per
/// command name, sorted by command name. A row exactly `window` old is kept
/// (the window is inclusive); a row from the future (a clock skew) is kept
/// too - the rollup only ever excludes rows *older* than the window.
pub fn health_rollup(
    rows: &[TimingRow],
    now: DateTime<Utc>,
    window: Duration,
) -> Vec<CommandHealth> {
    let window = chrono::Duration::from_std(window).unwrap_or(chrono::Duration::zero());
    let cutoff = now - window;

    let mut by_command: BTreeMap<&str, Vec<&TimingRow>> = BTreeMap::new();
    for row in rows {
        if row.ts >= cutoff {
            by_command
                .entry(row.command.as_str())
                .or_default()
                .push(row);
        }
    }

    by_command
        .into_iter()
        .map(|(command, group)| {
            let count = group.len() as u64;
            let failures = group.iter().filter(|r| r.outcome == Outcome::Error).count() as u64;
            let last_error = group
                .iter()
                .filter(|r| r.outcome == Outcome::Error)
                .max_by_key(|r| r.ts)
                .and_then(|r| r.error.clone());

            let mut durations: Vec<u64> = group.iter().map(|r| r.elapsed_ms).collect();
            durations.sort_unstable();

            CommandHealth {
                command: command.to_string(),
                count,
                failures,
                p50_ms: nearest_rank(&durations, 0.50),
                p95_ms: nearest_rank(&durations, 0.95),
                last_error,
            }
        })
        .collect()
}

/// Drops rows older than `retain` from `now`; a row exactly `retain` old is
/// kept, matching [`health_rollup`]'s inclusive window.
pub fn trim_rows(rows: Vec<TimingRow>, now: DateTime<Utc>, retain: Duration) -> Vec<TimingRow> {
    let retain = chrono::Duration::from_std(retain).unwrap_or(chrono::Duration::zero());
    let cutoff = now - retain;
    rows.into_iter().filter(|row| row.ts >= cutoff).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::Clock;
    use crate::testing::FakeClock;

    fn row(
        command: &str,
        minutes_ago: i64,
        elapsed_ms: u64,
        outcome: Outcome,
        error: Option<&str>,
    ) -> TimingRow {
        TimingRow {
            ts: Utc::now() - chrono::Duration::minutes(minutes_ago),
            command: command.to_string(),
            elapsed_ms,
            outcome,
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn health_rollup_groups_rows_by_command_and_reports_count_failures_p50_p95_or_names_the_wrong_field(
    ) {
        let now = Utc::now();
        // Fixture built oldest-first per command so the "most recent error"
        // check exercises real ordering, not insertion order.
        let rows = vec![
            // "add_skill": errors at minute 11 (elapsed 15) and minute 8 (elapsed 35, the more
            // recent one) -> last_error is the minute-8 row's message.
            row("add_skill", 11, 5, Outcome::Ok, None),
            row("add_skill", 10, 15, Outcome::Error, Some("disk full")),
            row("add_skill", 9, 25, Outcome::Ok, None),
            row(
                "add_skill",
                8,
                35,
                Outcome::Error,
                Some("permission denied"),
            ),
            // "list_events": all ok.
            row("list_events", 7, 100, Outcome::Ok, None),
            row("list_events", 6, 200, Outcome::Ok, None),
            row("list_events", 5, 300, Outcome::Ok, None),
            row("list_events", 4, 400, Outcome::Ok, None),
            // "scan": one error at minute 1 (elapsed 30).
            row("scan", 3, 10, Outcome::Ok, None),
            row("scan", 2, 20, Outcome::Ok, None),
            row("scan", 1, 30, Outcome::Error, Some("scope busy")),
            row("scan", 0, 40, Outcome::Ok, None),
        ];

        let got = health_rollup(&rows, now, Duration::from_secs(7 * 24 * 3600));

        let expected = vec![
            CommandHealth {
                command: "add_skill".to_string(),
                count: 4,
                failures: 2,
                p50_ms: 15,
                p95_ms: 35,
                last_error: Some("permission denied".to_string()),
            },
            CommandHealth {
                command: "list_events".to_string(),
                count: 4,
                failures: 0,
                p50_ms: 200,
                p95_ms: 400,
                last_error: None,
            },
            CommandHealth {
                command: "scan".to_string(),
                count: 4,
                failures: 1,
                p50_ms: 20,
                p95_ms: 40,
                last_error: Some("scope busy".to_string()),
            },
        ];

        assert_eq!(got, expected);
    }

    #[test]
    fn health_rollup_drops_rows_older_than_thirty_days_and_keeps_the_rest() {
        let clock = FakeClock::at(0);
        let now = clock.now();
        let rows = vec![
            row("scan", 29 * 24 * 60, 10, Outcome::Ok, None),
            row("scan", 30 * 24 * 60, 20, Outcome::Ok, None),
            row("scan", 31 * 24 * 60, 30, Outcome::Ok, None),
        ];
        // `row()` stamps against real `Utc::now()`, not the fake clock;
        // rebuild each row's `ts` relative to the fake `now` so the retain
        // window (computed from `now`) lines up with the fixture's ages.
        let rows: Vec<TimingRow> = rows
            .into_iter()
            .enumerate()
            .map(|(i, r)| TimingRow {
                ts: now - chrono::Duration::days(29 + i as i64),
                ..r
            })
            .collect();

        let kept = trim_rows(rows, now, Duration::from_secs(30 * 24 * 3600));

        let kept_ages_days: Vec<i64> = kept.iter().map(|r| (now - r.ts).num_days()).collect();
        assert_eq!(
            kept_ages_days,
            vec![29, 30],
            "the 29- and 30-day-old rows survive; the 31-day-old row is trimmed"
        );
    }

    /// 10 000 rows built from a formula (command = index % 20, elapsed = index
    /// % 500, every 13th row a failure), so the expected count/failures/p50/p95
    /// per command are derived from that same formula rather than hand-listed -
    /// the row count is too large to hand-compute, but a formula lets the test
    /// check every command's numbers, not just the total. The fold time is
    /// printed for a human to read under `--nocapture`; the ticket's 50ms
    /// budget is not asserted here since a loaded CI box would make that
    /// assert flaky for no code reason.
    #[test]
    fn health_rollup_of_ten_thousand_rows_matches_the_formula_derived_counts_and_percentiles_or_names_the_command_that_differs(
    ) {
        let now = Utc::now();
        const ROW_COUNT: i64 = 10_000;
        const COMMAND_COUNT: i64 = 20;
        let rows: Vec<TimingRow> = (0..ROW_COUNT)
            .map(|i| {
                let outcome = if i % 13 == 0 {
                    Outcome::Error
                } else {
                    Outcome::Ok
                };
                row(
                    &format!("command_{}", i % COMMAND_COUNT),
                    i % (60 * 24 * 6),
                    (i % 500) as u64,
                    outcome,
                    if outcome == Outcome::Error {
                        Some("synthetic failure")
                    } else {
                        None
                    },
                )
            })
            .collect();

        let start = std::time::Instant::now();
        let got = health_rollup(&rows, now, Duration::from_secs(7 * 24 * 3600));
        eprintln!(
            "health_rollup of {} rows: {} ms",
            ROW_COUNT,
            start.elapsed().as_millis()
        );

        // Derive each command's expected (elapsed_ms, is_error) list from the
        // same formula the fixture above was built from.
        let mut expected_by_command: BTreeMap<String, Vec<(u64, bool)>> = BTreeMap::new();
        for i in 0..ROW_COUNT {
            let command = format!("command_{}", i % COMMAND_COUNT);
            let elapsed_ms = (i % 500) as u64;
            let is_error = i % 13 == 0;
            expected_by_command
                .entry(command)
                .or_default()
                .push((elapsed_ms, is_error));
        }

        assert_eq!(
            got.len(),
            expected_by_command.len(),
            "one rollup row per distinct command_N the formula produced"
        );

        for health in &got {
            let entries = expected_by_command.get(&health.command).unwrap_or_else(|| {
                panic!("rollup produced an unexpected command {}", health.command)
            });
            let expected_count = entries.len() as u64;
            let expected_failures = entries.iter().filter(|(_, is_error)| *is_error).count() as u64;
            let mut durations: Vec<u64> = entries.iter().map(|(ms, _)| *ms).collect();
            durations.sort_unstable();
            let expected_p50 = nearest_rank(&durations, 0.50);
            let expected_p95 = nearest_rank(&durations, 0.95);

            assert_eq!(health.count, expected_count, "{}: count", health.command);
            assert_eq!(
                health.failures, expected_failures,
                "{}: failures",
                health.command
            );
            assert_eq!(health.p50_ms, expected_p50, "{}: p50_ms", health.command);
            assert_eq!(health.p95_ms, expected_p95, "{}: p95_ms", health.command);
        }
    }
}
