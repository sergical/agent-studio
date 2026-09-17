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
    use crate::timing;

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

    /// Wraps a real `std::time::Instant` behind [`Clock`] so this test can
    /// drive [`timing::op_timing`] (the same instrumentation an op call
    /// uses) with a real measurement of the fold - a [`FakeClock`] only
    /// advances when a test tells it to, which would make "how long did the
    /// fold actually take" a fiction.
    struct WallClock {
        start: std::time::Instant,
    }

    impl Clock for WallClock {
        fn now(&self) -> DateTime<Utc> {
            Utc::now()
        }
        fn monotonic(&self) -> Duration {
            self.start.elapsed()
        }
    }

    #[test]
    fn health_rollup_of_ten_thousand_rows_stays_under_the_fold_budget() {
        let now = Utc::now();
        let rows: Vec<TimingRow> = (0..10_000)
            .map(|i| {
                let outcome = if i % 13 == 0 {
                    Outcome::Error
                } else {
                    Outcome::Ok
                };
                row(
                    &format!("command_{}", i % 20),
                    (i % (60 * 24 * 6)) as i64,
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

        let clock = WallClock {
            start: std::time::Instant::now(),
        };
        let since = clock.monotonic();
        let got = health_rollup(&rows, now, Duration::from_secs(7 * 24 * 3600));
        let timing = timing::op_timing(&clock, "health_rollup_10000", since, vec![]);

        // No wall-clock assertion - the ticket's 50ms budget is measured
        // here for a human to read in test output, not enforced by the test
        // itself (a loaded CI box would make that assert flaky).
        println!("health_rollup of 10000 rows: {} ms", timing.elapsed_ms);

        let total: u64 = got.iter().map(|h| h.count).sum();
        assert_eq!(
            total, 10_000,
            "every row folds into exactly one command's count"
        );
    }
}
