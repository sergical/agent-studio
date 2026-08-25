// ============================================================================
// Skills Module - Invocation Index
// Parses Claude Code transcripts (~/.claude/projects/<dir>/*.jsonl) for
// Skill tool_use invocations and keeps a per-file cache so a refresh only
// re-parses transcripts whose size or mtime changed. Read discipline mirrors
// project_discovery.rs: only regular files are opened, each line is capped
// so a pathological line can't be buffered in full, and a file/run byte
// budget bounds worst-case I/O per refresh.
// ============================================================================

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Read as _, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One recorded skill invocation from an agent transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInvocation {
    pub skill: String,
    /// Which agent recorded this invocation, e.g. "Claude Code".
    pub agent: String,
    pub at: DateTime<Utc>,
    pub project_path: Option<String>,
}

/// Per-skill invocation summary sent to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInvocationStats {
    pub skill: String,
    pub total: u32,
    pub last_24_hours: u32,
    pub last_7_days: u32,
    pub last_14_days: u32,
    pub last_30_days: u32,
    pub last_used: Option<String>,
    /// Invocation counts by full project path, over the last 30 days only.
    pub by_project_30_days: BTreeMap<String, u32>,
    /// Per-day invocation counts, "YYYY-MM-DD" (UTC), over the last 365 days.
    pub by_day: BTreeMap<String, u32>,
}

/// Per-day invocation counts for the heatmap (date "YYYY-MM-DD" -> count).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InvocationHeatmap {
    pub days: BTreeMap<String, u32>,
}

/// A single line examined while parsing a transcript is capped at this many
/// bytes; a line that overruns the cap is drained and skipped (not parsed,
/// not buffered in full) rather than abandoning the whole file, so one
/// pathological line can't stop the rest of the file from being indexed.
const MAX_LINE_BYTES: usize = 256 * 1024;

/// Total bytes read from a single transcript file in one `refresh` call
/// before moving on; a file bigger than this needs further passes to finish.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Total bytes one `refresh` call may read across every changed file, so a
/// burst of large transcripts can't make one refresh unbounded.
const MAX_RUN_BYTES: u64 = 128 * 1024 * 1024;

/// A cache file larger than this is treated as unreadable rather than being
/// loaded, so a runaway cache can't blow up memory on startup.
const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;

fn default_file_budget() -> u64 {
    MAX_FILE_BYTES
}

fn default_run_budget() -> u64 {
    MAX_RUN_BYTES
}

/// A transcript file's cached parse result, keyed by size/mtime so a refresh
/// can tell whether it needs to be re-parsed. `parsed_bytes` is the offset of
/// the end of the last fully-parsed line, so an append-only transcript can be
/// resumed from where the previous refresh left off instead of reparsed from
/// scratch.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexedTranscript {
    size: u64,
    modified: SystemTime,
    parsed_bytes: u64,
    invocations: Vec<SkillInvocation>,
    /// Set when `parsed_bytes` stopped mid-line because that line alone
    /// couldn't fit the remaining budget (whether or not it's over
    /// `MAX_LINE_BYTES`). The next pass resumes by draining - not
    /// re-parsing - up to the next `\n` before returning to normal parsing,
    /// so a line that will never fit one pass's budget still makes progress.
    #[serde(default)]
    skipping_line: bool,
    /// The last <=64 bytes of the file immediately before `parsed_bytes`,
    /// captured when this entry was written. Lets a later refresh tell a
    /// plain append (those bytes are unchanged) from a same-size or
    /// still-growing rewrite (those bytes differ), even when size/mtime
    /// alone can't tell the difference.
    #[serde(default)]
    tail_sample: Vec<u8>,
}

/// How many bytes of a transcript's already-parsed tail are kept for
/// rewrite detection (see `IndexedTranscript::tail_sample`).
const TAIL_SAMPLE_BYTES: u64 = 64;

/// Outcome of one `SkillInvocationIndex::refresh` call.
#[derive(Debug, Clone, Default)]
pub struct RefreshReport {
    pub files_reparsed: usize,
    pub files_dropped: usize,
    pub bytes_read: u64,
    /// Set when a per-file or per-run budget stopped a file short of EOF;
    /// the file is left with `parsed_bytes < size` so a later refresh
    /// resumes and finishes draining the backlog.
    pub incomplete: bool,
}

/// Index of skill invocations parsed from local agent transcripts, cached
/// per file so unchanged files are never re-parsed. `file_budget`/`run_budget`
/// are not persisted (see `with_budgets` for the test-only override).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInvocationIndex {
    files: BTreeMap<PathBuf, IndexedTranscript>,
    #[serde(skip, default = "default_file_budget")]
    file_budget: u64,
    #[serde(skip, default = "default_run_budget")]
    run_budget: u64,
}

impl Default for SkillInvocationIndex {
    fn default() -> Self {
        Self {
            files: BTreeMap::new(),
            file_budget: MAX_FILE_BYTES,
            run_budget: MAX_RUN_BYTES,
        }
    }
}

impl SkillInvocationIndex {
    /// A test-only index with small per-file/per-run budgets, so a backlog
    /// that spans multiple `refresh` passes can be exercised without
    /// generating megabytes of fixture data.
    #[cfg(test)]
    fn with_budgets(file_bytes: u64, run_bytes: u64) -> Self {
        Self {
            files: BTreeMap::new(),
            file_budget: file_bytes,
            run_budget: run_bytes,
        }
    }

    /// Load the cache from `cache_path`. A missing, oversized, or
    /// unparseable cache yields an empty index rather than an error, so a
    /// corrupt cache file never blocks startup. A cache that fails to parse
    /// is renamed to `<path>.corrupt` so it doesn't keep failing on every
    /// startup and the raw bytes are still around to inspect.
    pub fn load_or_empty(cache_path: &Path) -> Self {
        if let Ok(meta) = fs::metadata(cache_path) {
            if meta.len() > MAX_CACHE_BYTES {
                eprintln!(
                    "skill invocations: cache is {} bytes, refusing to load",
                    meta.len()
                );
                return Self::default();
            }
        }
        let Ok(content) = fs::read_to_string(cache_path) else {
            return Self::default();
        };
        match serde_json::from_str(&content) {
            Ok(index) => index,
            Err(_) => {
                eprintln!("skill invocations: cache corrupt");
                let mut corrupt_path = cache_path.as_os_str().to_owned();
                corrupt_path.push(".corrupt");
                if let Err(e) = fs::rename(cache_path, &corrupt_path) {
                    eprintln!("skill invocations: failed to rename corrupt cache: {e}");
                }
                Self::default()
            }
        }
    }

    /// Persist the cache to `cache_path`, creating its parent directory if
    /// needed. Writes to a sibling `<path>.tmp` file and renames it into
    /// place, so a crash mid-write never leaves a half-written cache file.
    pub fn save(&self, cache_path: &Path) -> Result<(), String> {
        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        let mut tmp_path = cache_path.as_os_str().to_owned();
        tmp_path.push(".tmp");
        let tmp_path = PathBuf::from(tmp_path);
        fs::write(&tmp_path, json).map_err(|e| e.to_string())?;
        fs::rename(&tmp_path, cache_path).map_err(|e| e.to_string())
    }

    /// Re-parse only the transcript files under `claude_projects_dir` whose
    /// size or mtime changed since the last refresh, and drop files that no
    /// longer exist. Never panics: unreadable dirs/files/lines are skipped.
    /// An unchanged file (same size/mtime and already fully parsed) is
    /// skipped; a file that grew is read starting from `parsed_bytes` (the
    /// append-only fast path); a file that shrank below `parsed_bytes` is
    /// assumed truncated or rewritten and reparsed from scratch.
    pub fn refresh(&mut self, claude_projects_dir: &Path) -> RefreshReport {
        let mut report = RefreshReport::default();
        let mut run_budget = self.run_budget;
        let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
        // Cached entries are only dropped for directories that were listed
        // successfully; a transient read failure must not erase history.
        let mut listed_dirs: BTreeSet<PathBuf> = BTreeSet::new();

        match fs::read_dir(claude_projects_dir) {
            Err(_) => {
                report.incomplete = true;
                return report;
            }
            Ok(project_dirs) => {
                for project_dir in project_dirs.flatten() {
                    let dir = project_dir.path();
                    if !dir.is_dir() {
                        continue;
                    }
                    let Ok(entries) = fs::read_dir(&dir) else {
                        report.incomplete = true;
                        continue;
                    };
                    listed_dirs.insert(dir.clone());
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().is_none_or(|ext| ext != "jsonl") {
                            continue;
                        }
                        // Only regular files: a symlink, FIFO, or directory named
                        // `*.jsonl` is never opened as a transcript.
                        if !fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
                            continue;
                        }
                        let Ok(meta) = entry.metadata() else {
                            continue;
                        };
                        let size = meta.len();
                        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                        seen.insert(path.clone());

                        let (start_offset, mut invocations, skip_to_newline) =
                            match self.files.get(&path) {
                                Some(existing)
                                    if existing.size == size
                                        && existing.modified == modified
                                        && existing.parsed_bytes >= size =>
                                {
                                    continue;
                                }
                                Some(existing)
                                    if existing.size == size && existing.parsed_bytes >= size =>
                                {
                                    // Same size, but the mtime moved: rewritten in
                                    // place at exactly the old length. Reparse from
                                    // scratch rather than trusting a
                                    // byte-for-byte-identical-looking cache entry.
                                    (0, Vec::new(), false)
                                }
                                Some(existing) if size < existing.parsed_bytes => {
                                    (0, Vec::new(), false)
                                }
                                Some(existing) => {
                                    let current_tail =
                                        read_tail_sample(&path, existing.parsed_bytes);
                                    if current_tail != existing.tail_sample {
                                        // The bytes just before our resume point no
                                        // longer match what we parsed last time:
                                        // this wasn't a plain append, so the cached
                                        // invocations may be stale.
                                        (0, Vec::new(), false)
                                    } else {
                                        (
                                            existing.parsed_bytes,
                                            existing.invocations.clone(),
                                            existing.skipping_line,
                                        )
                                    }
                                }
                                None => (0, Vec::new(), false),
                            };

                        if run_budget == 0 {
                            report.incomplete = true;
                            continue;
                        }

                        let file_budget = self.file_budget.min(run_budget);
                        let Some((text, consumed, skipping_line)) = read_transcript_from_offset(
                            &path,
                            start_offset,
                            file_budget,
                            &mut run_budget,
                            skip_to_newline,
                        ) else {
                            continue;
                        };
                        let parsed_bytes = start_offset + consumed;
                        invocations.extend(parse_transcript_invocations(&text));
                        if parsed_bytes < size {
                            report.incomplete = true;
                        }

                        report.files_reparsed += 1;
                        report.bytes_read += consumed;
                        let tail_sample = read_tail_sample(&path, parsed_bytes);
                        self.files.insert(
                            path,
                            IndexedTranscript {
                                size,
                                modified,
                                parsed_bytes,
                                invocations,
                                skipping_line,
                                tail_sample,
                            },
                        );
                    }
                }
            }
        }

        let before = self.files.len();
        self.files.retain(|path, _| {
            seen.contains(path)
                || !path
                    .parent()
                    .is_some_and(|parent| listed_dirs.contains(parent))
        });
        report.files_dropped = before - self.files.len();
        report
    }

    /// Whether `path` is already a tracked transcript file. Used by the
    /// background watcher to tell a brand-new transcript (needs a full
    /// rebuild, since it can also mean a new project) from a change to one
    /// already indexed (an invocations-only refresh suffices).
    pub fn knows_file(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    /// Per-skill invocation totals across every cached transcript, as of now.
    /// Thin wrapper around `stats_at` so callers don't have to thread `now`
    /// through for the common case.
    pub fn stats(&self) -> Vec<SkillInvocationStats> {
        self.stats_at(Utc::now())
    }

    /// Per-skill invocation totals across every cached transcript, with the
    /// rolling windows (24h/7d/14d/30d, by_project_30_days, by_day) computed
    /// relative to `now` rather than the wall clock. Pulled out of `stats` so
    /// window-boundary behavior (e.g. a 25h-old invocation dropping out of
    /// the 24h bucket) is testable without waiting on real time.
    pub fn stats_at(&self, now: DateTime<Utc>) -> Vec<SkillInvocationStats> {
        struct Acc {
            total: u32,
            last_24_hours: u32,
            last_7_days: u32,
            last_14_days: u32,
            last_30_days: u32,
            last_used: Option<DateTime<Utc>>,
            by_project_30_days: BTreeMap<String, u32>,
            by_day: BTreeMap<String, u32>,
        }

        let cutoff_24h = now - chrono::Duration::hours(24);
        let cutoff_7 = now - chrono::Duration::days(7);
        let cutoff_14 = now - chrono::Duration::days(14);
        let cutoff_30 = now - chrono::Duration::days(30);
        let cutoff_365 = now - chrono::Duration::days(365);
        let mut by_skill: BTreeMap<String, Acc> = BTreeMap::new();

        for transcript in self.files.values() {
            for invocation in &transcript.invocations {
                let acc = by_skill.entry(invocation.skill.clone()).or_insert(Acc {
                    total: 0,
                    last_24_hours: 0,
                    last_7_days: 0,
                    last_14_days: 0,
                    last_30_days: 0,
                    last_used: None,
                    by_project_30_days: BTreeMap::new(),
                    by_day: BTreeMap::new(),
                });
                acc.total += 1;
                if invocation.at >= cutoff_24h {
                    acc.last_24_hours += 1;
                }
                if invocation.at >= cutoff_7 {
                    acc.last_7_days += 1;
                }
                if invocation.at >= cutoff_14 {
                    acc.last_14_days += 1;
                }
                if invocation.at >= cutoff_30 {
                    acc.last_30_days += 1;
                }
                if acc.last_used.is_none_or(|last| invocation.at > last) {
                    acc.last_used = Some(invocation.at);
                }
                if invocation.at >= cutoff_30 {
                    if let Some(project) = &invocation.project_path {
                        *acc.by_project_30_days.entry(project.clone()).or_insert(0) += 1;
                    }
                }
                if invocation.at >= cutoff_365 {
                    let day = invocation.at.format("%Y-%m-%d").to_string();
                    *acc.by_day.entry(day).or_insert(0) += 1;
                }
            }
        }

        by_skill
            .into_iter()
            .map(|(skill, acc)| SkillInvocationStats {
                skill,
                total: acc.total,
                last_24_hours: acc.last_24_hours,
                last_7_days: acc.last_7_days,
                last_14_days: acc.last_14_days,
                last_30_days: acc.last_30_days,
                last_used: acc.last_used.map(|at| at.to_rfc3339()),
                by_project_30_days: acc.by_project_30_days,
                by_day: acc.by_day,
            })
            .collect()
    }

    /// Per-day invocation counts over the last `days` days.
    pub fn heatmap(&self, days: u32) -> InvocationHeatmap {
        let cutoff = Utc::now() - chrono::Duration::days(days as i64);
        let mut result = BTreeMap::new();
        for transcript in self.files.values() {
            for invocation in &transcript.invocations {
                if invocation.at < cutoff {
                    continue;
                }
                let day = invocation.at.format("%Y-%m-%d").to_string();
                *result.entry(day).or_insert(0) += 1;
            }
        }
        InvocationHeatmap { days: result }
    }
}

/// Read the up to `TAIL_SAMPLE_BYTES` bytes of `path` immediately before
/// `offset`, used to detect a same-size (or still-growing) rewrite that
/// size/mtime alone can't distinguish from a plain append. An unreadable
/// path, or `offset == 0`, yields an empty sample.
fn read_tail_sample(path: &Path, offset: u64) -> Vec<u8> {
    let start = offset.saturating_sub(TAIL_SAMPLE_BYTES);
    let len = (offset - start) as usize;
    if len == 0 {
        return Vec::new();
    }
    let Ok(mut file) = fs::File::open(path) else {
        return Vec::new();
    };
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = vec![0u8; len];
    match file.read_exact(&mut buf) {
        Ok(()) => buf,
        Err(_) => Vec::new(),
    }
}

/// Read `path` starting at byte `start_offset`, line by line, bounded per
/// line and by `file_budget`/`run_budget`. Returns the concatenated text of
/// every *complete, `\n`-terminated* line read, the number of bytes consumed
/// for those complete lines, and whether the read ended mid-drain of a line
/// too big to fit this pass's budget (see below). A final line at true EOF
/// with no trailing `\n` is never committed - it may still be being
/// written - so the next refresh re-reads it from the same offset. A line
/// that overruns `MAX_LINE_BYTES`, or that alone can't fit `file_budget`, is
/// drained and skipped rather than buffered or parsed: `skip_to_newline`
/// resumes an in-progress drain left over from a previous, budget-truncated
/// pass, so a persistently oversized line still makes progress instead of
/// being re-read from scratch (and truncated at the same point) forever.
/// `None` when the file can't be opened or seeked to; an empty read (nothing
/// to do) still returns `Some(("", 0, false))`.
fn read_transcript_from_offset(
    path: &Path,
    start_offset: u64,
    file_budget: u64,
    run_budget: &mut u64,
    skip_to_newline: bool,
) -> Option<(String, u64, bool)> {
    let mut file = fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(start_offset)).ok()?;
    let mut reader = BufReader::new(file);
    let mut text = String::new();
    let mut consumed: u64 = 0;
    let mut remaining = file_budget;
    let mut skipping = skip_to_newline;

    if skipping {
        loop {
            if remaining == 0 {
                // Still mid-drain: resume from here next pass.
                return Some((text, consumed, true));
            }
            let chunk_cap = (MAX_LINE_BYTES as u64 + 1).min(remaining);
            let mut chunk: Vec<u8> = Vec::new();
            let read = reader
                .by_ref()
                .take(chunk_cap)
                .read_until(b'\n', &mut chunk);
            let n = match read {
                Ok(0) => {
                    skipping = false; // true EOF: nothing left to drain
                    break;
                }
                Ok(n) => n as u64,
                Err(_) => {
                    skipping = false;
                    break;
                }
            };
            remaining = remaining.saturating_sub(n);
            *run_budget = run_budget.saturating_sub(n);
            consumed += n;
            if chunk.last() == Some(&b'\n') {
                skipping = false;
                break;
            }
        }
    }

    'lines: loop {
        if remaining == 0 {
            break;
        }
        let mut line_buf: Vec<u8> = Vec::new();
        let mut line_bytes: u64 = 0;
        let mut terminated = false;

        // Inner loop only exits via one of the `break`s below (a line is
        // complete at a `\n`, at true EOF, or on a read error) or via
        // `break 'lines` (budget exhausted mid-line, handled there).
        loop {
            if remaining == 0 {
                if line_bytes > MAX_LINE_BYTES as u64 || line_bytes >= file_budget {
                    // Too big to buffer, or too big to ever fit one pass's
                    // budget: commit the bytes already drained and resume
                    // draining (not re-reading from scratch) next pass.
                    skipping = true;
                } else {
                    // An ordinary line cut short by the budget: don't count
                    // it as consumed, so the next pass re-reads it whole.
                    consumed -= line_bytes;
                }
                break 'lines;
            }
            // Cap each chunk at the line limit (+1, to distinguish "found
            // the newline right at the cap" from "no newline within the
            // cap") so a pathologically long line is never buffered in full
            // in one read; a still-unterminated line loops for another chunk.
            let chunk_cap = (MAX_LINE_BYTES as u64 + 1).min(remaining);
            let mut chunk: Vec<u8> = Vec::new();
            let read = reader
                .by_ref()
                .take(chunk_cap)
                .read_until(b'\n', &mut chunk);
            let n = match read {
                Ok(0) => break, // true EOF
                Ok(n) => n as u64,
                Err(_) => break,
            };
            remaining = remaining.saturating_sub(n);
            *run_budget = run_budget.saturating_sub(n);
            consumed += n;
            line_bytes += n;
            if line_buf.len() as u64 <= MAX_LINE_BYTES as u64 {
                line_buf.extend_from_slice(&chunk);
            }
            if chunk.last() == Some(&b'\n') {
                terminated = true;
                break;
            }
        }

        if line_bytes == 0 {
            break; // nothing left to read
        }

        if !terminated {
            // Final line at true EOF with no trailing `\n`: don't commit it,
            // the next refresh re-reads it from this same offset once it's
            // complete.
            consumed -= line_bytes;
            break;
        }

        let oversized = line_buf.len() as u64 > MAX_LINE_BYTES as u64;
        if !oversized {
            if let Ok(line) = std::str::from_utf8(&line_buf) {
                text.push_str(line);
            }
        }
    }

    Some((text, consumed, skipping))
}

/// Parse one transcript file's text into invocations. Fast path: only lines
/// containing the substring `"name":"Skill"` are handed to `serde_json`, so
/// most lines (which don't reference the Skill tool) never get a full JSON
/// parse. A line that's malformed JSON, or missing a `timestamp`, is skipped
/// rather than failing the whole file.
pub fn parse_transcript_invocations(text: &str) -> Vec<SkillInvocation> {
    let mut out = Vec::new();

    for line in text.lines() {
        if !line.contains("\"name\":\"Skill\"") {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(timestamp) = record.get("timestamp").and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(at) = DateTime::parse_from_rfc3339(timestamp) else {
            continue;
        };
        let at = at.with_timezone(&Utc);
        let project_path = record
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let Some(content) = record
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };

        for block in content {
            if block.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
                continue;
            }
            if block.get("name").and_then(|v| v.as_str()) != Some("Skill") {
                continue;
            }
            let Some(skill) = block
                .get("input")
                .and_then(|i| i.get("skill"))
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            out.push(SkillInvocation {
                skill: skill.to_string(),
                agent: "Claude Code".to_string(),
                at,
                project_path: project_path.clone(),
            });
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn skill_line(skill: &str, timestamp: &str, cwd: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{timestamp}","cwd":"{cwd}","message":{{"content":[{{"type":"tool_use","name":"Skill","input":{{"skill":"{skill}"}}}}]}}}}"#
        )
    }

    #[test]
    fn parses_a_skill_tool_use_line() {
        let text = skill_line("write-tests", "2026-08-01T12:00:00Z", "/home/me/project");
        let invocations = parse_transcript_invocations(&text);
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].skill, "write-tests");
        assert_eq!(invocations[0].agent, "Claude Code");
        assert_eq!(
            invocations[0].project_path.as_deref(),
            Some("/home/me/project")
        );
    }

    #[test]
    fn non_skill_line_is_skipped() {
        let text = r#"{"type":"assistant","timestamp":"2026-08-01T12:00:00Z","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#;
        assert!(parse_transcript_invocations(text).is_empty());
    }

    #[test]
    fn malformed_json_line_is_skipped() {
        let text = "{\"name\":\"Skill\" this is not valid json";
        assert!(parse_transcript_invocations(text).is_empty());
    }

    #[test]
    fn missing_timestamp_is_skipped() {
        let text = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":"write-tests"}}]}}"#;
        assert!(parse_transcript_invocations(text).is_empty());
    }

    #[test]
    fn multiple_lines_and_blocks_are_all_found() {
        let mut text = skill_line("write-tests", "2026-08-01T12:00:00Z", "/proj-a");
        text.push('\n');
        text.push_str(&skill_line("lint-code", "2026-08-02T12:00:00Z", "/proj-b"));
        let invocations = parse_transcript_invocations(&text);
        assert_eq!(invocations.len(), 2);
    }

    // Real transcripts are newline-delimited JSON, so every written record
    // ends in `\n`: an unterminated final line is a partial write in
    // progress (see `partial_final_line_is_not_committed_until_terminated`)
    // and is deliberately never committed.
    fn write_transcript(
        dir: &Path,
        name: &str,
        skill: &str,
        timestamp: &str,
        cwd: &str,
    ) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, format!("{}\n", skill_line(skill, timestamp, cwd))).unwrap();
        path
    }

    #[test]
    fn refresh_skips_unchanged_files() {
        let tmp = tempfile::tempdir().unwrap();
        let projects_dir = tmp.path().join("projects");
        let session_dir = projects_dir.join("-my-project");
        write_transcript(
            &session_dir,
            "session.jsonl",
            "write-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );

        let mut index = SkillInvocationIndex::default();
        let first = index.refresh(&projects_dir);
        assert_eq!(first.files_reparsed, 1);
        assert_eq!(index.stats().len(), 1);

        let second = index.refresh(&projects_dir);
        assert_eq!(second.files_reparsed, 0);
    }

    #[test]
    fn refresh_drops_deleted_files() {
        let tmp = tempfile::tempdir().unwrap();
        let projects_dir = tmp.path().join("projects");
        let session_dir = projects_dir.join("-my-project");
        let path = write_transcript(
            &session_dir,
            "session.jsonl",
            "write-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );

        let mut index = SkillInvocationIndex::default();
        index.refresh(&projects_dir);
        assert_eq!(index.stats().len(), 1);

        fs::remove_file(&path).unwrap();
        let report = index.refresh(&projects_dir);
        assert_eq!(report.files_dropped, 1);
        assert!(index.stats().is_empty());
    }

    #[test]
    fn unreadable_projects_dir_keeps_cached_files_and_reports_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let projects_dir = tmp.path().join("projects");
        write_transcript(
            &projects_dir.join("-my-project"),
            "session.jsonl",
            "write-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );
        let mut index = SkillInvocationIndex::default();
        index.refresh(&projects_dir);
        assert_eq!(index.stats().len(), 1);

        let report = index.refresh(&tmp.path().join("missing"));
        assert!(report.incomplete);
        assert_eq!(report.files_dropped, 0);
        assert_eq!(index.stats().len(), 1);
    }

    #[test]
    fn stats_totals_last_30_days_and_by_project_30_days() {
        let mut index = SkillInvocationIndex::default();
        let recent = Utc::now().to_rfc3339();
        let old = (Utc::now() - chrono::Duration::days(60)).to_rfc3339();
        let text = format!(
            "{}\n{}\n{}",
            skill_line("write-tests", &recent, "/proj-a"),
            skill_line("write-tests", &recent, "/proj-b"),
            skill_line("write-tests", &old, "/proj-a"),
        );
        index.files.insert(
            PathBuf::from("fixture.jsonl"),
            IndexedTranscript {
                size: 0,
                modified: SystemTime::UNIX_EPOCH,
                parsed_bytes: 0,
                invocations: parse_transcript_invocations(&text),
                skipping_line: false,
                tail_sample: Vec::new(),
            },
        );

        let stats = index.stats();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].total, 3);
        assert_eq!(stats[0].last_24_hours, 2);
        assert_eq!(stats[0].last_7_days, 2);
        assert_eq!(stats[0].last_14_days, 2);
        assert_eq!(stats[0].last_30_days, 2);
        // The 60-day-old invocation is outside the 30-day window, so it
        // doesn't count toward by_project_30_days even though it counts
        // toward `total`.
        assert_eq!(stats[0].by_project_30_days.get("/proj-a"), Some(&1));
        assert_eq!(stats[0].by_project_30_days.get("/proj-b"), Some(&1));
        let today = Utc::now().format("%Y-%m-%d").to_string();
        assert_eq!(stats[0].by_day.get(&today), Some(&2));
    }

    #[test]
    fn stats_at_windows_are_relative_to_the_given_now() {
        let mut index = SkillInvocationIndex::default();
        let now = Utc::now();
        let twenty_five_hours_ago = (now - chrono::Duration::hours(25)).to_rfc3339();
        let text = skill_line("write-tests", &twenty_five_hours_ago, "/proj-a");
        index.files.insert(
            PathBuf::from("fixture.jsonl"),
            IndexedTranscript {
                size: 0,
                modified: SystemTime::UNIX_EPOCH,
                parsed_bytes: 0,
                invocations: parse_transcript_invocations(&text),
                skipping_line: false,
                tail_sample: Vec::new(),
            },
        );

        let stats = index.stats_at(now);
        assert_eq!(stats.len(), 1);
        assert_eq!(
            stats[0].last_24_hours, 0,
            "25h-old invocation counted in 24h"
        );
        assert_eq!(
            stats[0].last_7_days, 1,
            "25h-old invocation missing from 7d"
        );
    }

    #[test]
    fn heatmap_buckets_by_day() {
        let mut index = SkillInvocationIndex::default();
        let today = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let text = format!(
            "{}\n{}",
            skill_line("write-tests", &today, "/proj-a"),
            skill_line("lint-code", &today, "/proj-a"),
        );
        index.files.insert(
            PathBuf::from("fixture.jsonl"),
            IndexedTranscript {
                size: 0,
                modified: SystemTime::UNIX_EPOCH,
                parsed_bytes: 0,
                invocations: parse_transcript_invocations(&text),
                skipping_line: false,
                tail_sample: Vec::new(),
            },
        );

        let heatmap = index.heatmap(30);
        assert_eq!(heatmap.days.len(), 1);
        assert_eq!(*heatmap.days.values().next().unwrap(), 2);
    }

    #[test]
    fn oversized_line_is_skipped_and_later_lines_still_count() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("projects/-huge-line");
        fs::create_dir_all(&dir).unwrap();
        let mut content = vec![b'x'; MAX_LINE_BYTES + 1024];
        content.push(b'\n');
        content.extend(skill_line("write-tests", "2026-08-01T12:00:00Z", "/proj").into_bytes());
        content.push(b'\n');
        fs::write(dir.join("session.jsonl"), content).unwrap();

        let mut index = SkillInvocationIndex::default();
        index.refresh(&tmp.path().join("projects"));
        let stats = index.stats();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].skill, "write-tests");
        assert_eq!(stats[0].total, 1);
    }

    #[test]
    fn append_to_transcript_only_parses_the_new_line() {
        let tmp = tempfile::tempdir().unwrap();
        let projects_dir = tmp.path().join("projects");
        let session_dir = projects_dir.join("-my-project");
        let path = write_transcript(
            &session_dir,
            "session.jsonl",
            "write-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );

        let mut index = SkillInvocationIndex::default();
        index.refresh(&projects_dir);
        let parsed_bytes_after_first = index.files.get(&path).unwrap().parsed_bytes;
        assert_eq!(parsed_bytes_after_first, fs::metadata(&path).unwrap().len());

        let mut content = fs::read(&path).unwrap();
        content.extend(skill_line("lint-code", "2026-08-02T12:00:00Z", "/my-project").into_bytes());
        content.push(b'\n');
        fs::write(&path, content).unwrap();

        let report = index.refresh(&projects_dir);
        assert_eq!(report.files_reparsed, 1);
        let stats = index.stats();
        assert_eq!(stats.len(), 2);
        assert_eq!(
            index.files.get(&path).unwrap().parsed_bytes,
            fs::metadata(&path).unwrap().len()
        );
    }

    #[test]
    fn truncated_transcript_is_reparsed_from_scratch() {
        let tmp = tempfile::tempdir().unwrap();
        let projects_dir = tmp.path().join("projects");
        let session_dir = projects_dir.join("-my-project");
        let path = write_transcript(
            &session_dir,
            "session.jsonl",
            "write-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );

        let mut index = SkillInvocationIndex::default();
        index.refresh(&projects_dir);
        assert_eq!(index.stats()[0].total, 1);

        fs::write(
            &path,
            format!(
                "{}\n",
                skill_line("lint-code", "2026-08-02T12:00:00Z", "/other")
            ),
        )
        .unwrap();
        index.refresh(&projects_dir);
        let stats = index.stats();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].skill, "lint-code");
    }

    #[test]
    fn file_larger_than_budget_finishes_over_multiple_refreshes() {
        let tmp = tempfile::tempdir().unwrap();
        let projects_dir = tmp.path().join("projects");
        let session_dir = projects_dir.join("-my-project");
        fs::create_dir_all(&session_dir).unwrap();
        let mut content = String::new();
        for i in 0..20 {
            content.push_str(&skill_line(
                "write-tests",
                "2026-08-01T12:00:00Z",
                &format!("/proj-{i}"),
            ));
            content.push('\n');
        }
        fs::write(session_dir.join("session.jsonl"), &content).unwrap();

        let mut index = SkillInvocationIndex::with_budgets(200, 200);
        let first = index.refresh(&projects_dir);
        assert!(first.incomplete);
        assert!(index.stats()[0].total < 20);

        let mut passes = 0;
        loop {
            let report = index.refresh(&projects_dir);
            passes += 1;
            if !report.incomplete {
                break;
            }
            assert!(passes < 50, "backlog never drained");
        }
        assert_eq!(index.stats()[0].total, 20);
    }

    #[test]
    fn partial_final_line_is_not_committed_until_terminated() {
        let tmp = tempfile::tempdir().unwrap();
        let projects_dir = tmp.path().join("projects");
        let session_dir = projects_dir.join("-my-project");
        fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("session.jsonl");
        let full = skill_line("write-tests", "2026-08-01T12:00:00Z", "/my-project");
        let half = &full[..full.len() / 2];
        fs::write(&path, half).unwrap();

        let mut index = SkillInvocationIndex::default();
        index.refresh(&projects_dir);
        assert!(
            index.stats().is_empty(),
            "half-written record was committed"
        );
        assert_eq!(index.files.get(&path).unwrap().parsed_bytes, 0);

        fs::write(&path, format!("{full}\n")).unwrap();
        index.refresh(&projects_dir);
        let stats = index.stats();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].skill, "write-tests");
    }

    #[test]
    fn line_larger_than_the_file_budget_is_skipped_over_multiple_passes() {
        let tmp = tempfile::tempdir().unwrap();
        let projects_dir = tmp.path().join("projects");
        let session_dir = projects_dir.join("-my-project");
        fs::create_dir_all(&session_dir).unwrap();
        let mut content = vec![b'x'; 2000];
        content.push(b'\n');
        content.extend(skill_line("write-tests", "2026-08-01T12:00:00Z", "/proj").into_bytes());
        content.push(b'\n');
        fs::write(session_dir.join("session.jsonl"), &content).unwrap();

        // The oversized line (2001 bytes) is well over the per-pass budget,
        // so a naive "revert an incomplete line" strategy would truncate at
        // the same offset forever.
        let mut index = SkillInvocationIndex::with_budgets(200, 200);
        let first = index.refresh(&projects_dir);
        assert!(first.incomplete);
        assert!(index.stats().is_empty());

        let mut passes = 1;
        loop {
            let report = index.refresh(&projects_dir);
            passes += 1;
            if !report.incomplete {
                break;
            }
            assert!(passes < 50, "oversized line never drained");
        }
        assert!(passes > 1, "expected the skip to span multiple passes");
        let stats = index.stats();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].skill, "write-tests");
    }

    #[test]
    fn same_size_rewrite_is_detected_via_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let projects_dir = tmp.path().join("projects");
        let session_dir = projects_dir.join("-my-project");
        let path = write_transcript(
            &session_dir,
            "session.jsonl",
            "run-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );

        let mut index = SkillInvocationIndex::default();
        index.refresh(&projects_dir);
        assert_eq!(index.stats()[0].skill, "run-tests");

        let original_size = fs::metadata(&path).unwrap().len();
        let new_content = format!(
            "{}\n",
            skill_line("lint-code", "2026-08-01T12:00:00Z", "/my-project")
        );
        assert_eq!(
            new_content.len() as u64,
            original_size,
            "test fixture must keep the same byte length"
        );
        fs::write(&path, &new_content).unwrap();
        let bumped_mtime =
            fs::metadata(&path).unwrap().modified().unwrap() + Duration::from_secs(1);
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(bumped_mtime)
            .unwrap();

        index.refresh(&projects_dir);
        let stats = index.stats();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].skill, "lint-code");
    }

    #[test]
    fn truncate_and_regrow_clears_stale_invocations_via_tail_sample() {
        let tmp = tempfile::tempdir().unwrap();
        let projects_dir = tmp.path().join("projects");
        let session_dir = projects_dir.join("-my-project");
        let path = write_transcript(
            &session_dir,
            "session.jsonl",
            "write-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );

        let mut index = SkillInvocationIndex::default();
        index.refresh(&projects_dir);
        assert_eq!(index.stats()[0].skill, "write-tests");

        // Truncate to unrelated content, then regrow past the old
        // `parsed_bytes` offset: the file is now longer than before (an
        // append would be assumed) but the bytes at the old offset don't
        // match, so this must be detected as a rewrite.
        let mut new_content = String::from("short\n");
        new_content.push_str(&skill_line("lint-code", "2026-08-02T12:00:00Z", "/other"));
        new_content.push('\n');
        fs::write(&path, &new_content).unwrap();

        index.refresh(&projects_dir);
        let stats = index.stats();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].skill, "lint-code");
    }

    #[test]
    fn corrupt_cache_yields_empty_index_and_leaves_a_corrupt_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_path = tmp.path().join("skill-invocations.json");
        fs::write(&cache_path, "not valid json").unwrap();

        let index = SkillInvocationIndex::load_or_empty(&cache_path);
        assert!(index.files.is_empty());
        assert!(!cache_path.exists());
        assert!(cache_path
            .with_file_name("skill-invocations.json.corrupt")
            .exists());
    }

    #[test]
    fn save_writes_via_tmp_then_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_path = tmp.path().join("nested/skill-invocations.json");
        let index = SkillInvocationIndex::default();
        index.save(&cache_path).unwrap();
        assert!(cache_path.exists());
        assert!(!cache_path
            .with_file_name("skill-invocations.json.tmp")
            .exists());
    }
}
