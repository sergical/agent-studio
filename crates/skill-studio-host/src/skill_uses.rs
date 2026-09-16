//! Skill-use index: parses each enabled harness's own session history for
//! skill uses and keeps a per-file cache so a refresh only re-parses
//! transcripts whose size or mtime changed. Read discipline mirrors
//! `discovery.rs`: only regular files are opened, each line is capped so a
//! pathological line can't be buffered in full, and a file/run byte budget
//! bounds worst-case I/O per refresh.
//!
//! `SOURCES` is the table of harnesses this index reads from; today it holds
//! one entry (Claude Code). Adding a harness later means adding a row, not
//! reworking `refresh`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, BufRead, BufReader, Read as _, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use skill_studio_core::discovery_sources::DiscoverySources;
use skill_studio_core::identity::AgentId;
use skill_studio_core::skill_uses::parse_claude_code_uses;
use skill_studio_core::skill_uses::{
    skill_heatmap, skill_stats, InvocationHeatmap, SkillInvocation, SkillInvocationStats,
    SkillUseFilter,
};

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

/// Claude Code keeps one directory per project here, and (for subagent
/// sessions) one `<session>/subagents/*.jsonl` per parent session inside it.
const CLAUDE_PROJECTS_ROOT: &str = ".claude/projects";

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
    uses: Vec<SkillInvocation>,
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
pub struct SkillUseRefreshReport {
    /// Number of transcript files re-parsed (in full or in part) this call.
    pub files_reparsed: usize,
    /// Number of cached files removed because they no longer exist.
    pub files_dropped: usize,
    /// Total bytes read from transcripts this call.
    pub bytes_read: u64,
    /// Set when a per-file or per-run budget stopped a file short of EOF, or
    /// a source's own listing failed; the file (if any) is left with
    /// `parsed_bytes < size` so a later refresh resumes and finishes
    /// draining the backlog.
    pub incomplete: bool,
}

/// One harness's session history this index reads uses from: where to find
/// its transcripts (`list`) and how to parse one transcript's text (`parse`).
struct UseSource {
    /// `AgentId` wire name, e.g. `AgentId::CLAUDE_CODE`.
    harness: &'static str,
    /// The root directory this source reads under `home`, used to scope the
    /// drop rule to files this source (when enabled) actually owns, so a
    /// switched-off source's cached files are left untouched even if their
    /// directory is later removed.
    root: fn(&Path) -> PathBuf,
    list: fn(&Path) -> SourceListing,
    parse: fn(&str) -> Vec<SkillInvocation>,
}

/// One source's listing of the transcript files it found under `home`.
struct SourceListing {
    files: Vec<PathBuf>,
    /// Directories listed successfully (parent dirs of `files`, plus any
    /// intermediate directory checked along the way), used by the drop rule.
    listed_dirs: BTreeSet<PathBuf>,
    /// Set when a directory this source needed to list could not be listed,
    /// for a reason other than "the directory doesn't exist" where that's
    /// expected (see `list_claude_code_transcripts`).
    incomplete: bool,
}

/// Opening a FIFO blocks, and a symlink can point anywhere, so transcripts
/// are read only when they are regular files.
fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

fn is_jsonl(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl") && is_regular_file(path)
}

fn claude_code_root(home: &Path) -> PathBuf {
    home.join(CLAUDE_PROJECTS_ROOT)
}

/// Lists Claude Code's transcripts: `<home>/.claude/projects/<project>/*.jsonl`
/// and `<home>/.claude/projects/<project>/<session>/subagents/*.jsonl`. A
/// failed listing of the `projects` root or of a `<project>` directory marks
/// the listing incomplete; a missing `subagents` directory is normal (most
/// sessions have no subagents) and does not.
fn list_claude_code_transcripts(home: &Path) -> SourceListing {
    let mut files = Vec::new();
    let mut listed_dirs = BTreeSet::new();
    let mut incomplete = false;

    let projects_dir = claude_code_root(home);
    let Ok(project_dirs) = fs::read_dir(&projects_dir) else {
        return SourceListing {
            files,
            listed_dirs,
            incomplete: true,
        };
    };
    for project_dir in project_dirs.flatten() {
        let dir = project_dir.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            incomplete = true;
            continue;
        };
        listed_dirs.insert(dir.clone());
        for entry in entries.flatten() {
            let path = entry.path();
            if is_jsonl(&path) {
                files.push(path);
                continue;
            }
            if !path.is_dir() {
                continue;
            }
            let subagents = path.join("subagents");
            match fs::read_dir(&subagents) {
                Ok(sub_entries) => {
                    listed_dirs.insert(subagents.clone());
                    for sub_entry in sub_entries.flatten() {
                        let sub_path = sub_entry.path();
                        if is_jsonl(&sub_path) {
                            files.push(sub_path);
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(_) => incomplete = true,
            }
        }
    }

    SourceListing {
        files,
        listed_dirs,
        incomplete,
    }
}

/// Every harness this index reads uses from, in the order they're processed.
const SOURCES: &[UseSource] = &[UseSource {
    harness: AgentId::CLAUDE_CODE,
    root: claude_code_root,
    list: list_claude_code_transcripts,
    parse: parse_claude_code_uses,
}];

/// Index of skill uses parsed from local harness session history, cached per
/// file so unchanged files are never re-parsed. `file_budget`/`run_budget`
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
                    "skill uses: cache is {} bytes, refusing to load",
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
                eprintln!("skill uses: cache corrupt");
                let mut corrupt_path = cache_path.as_os_str().to_owned();
                corrupt_path.push(".corrupt");
                if let Err(e) = fs::rename(cache_path, &corrupt_path) {
                    eprintln!("skill uses: failed to rename corrupt cache: {e}");
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

    /// Re-parses only the transcript files (under every enabled source in
    /// `SOURCES`) whose size or mtime changed since the last refresh, and
    /// drops files that no longer exist. A source whose harness is switched
    /// off in `sources` is skipped entirely: it is neither listed nor does
    /// it drop any of its previously cached files. Never panics: unreadable
    /// dirs/files/lines are skipped.
    pub fn refresh(&mut self, home: &Path, sources: &DiscoverySources) -> SkillUseRefreshReport {
        let mut report = SkillUseRefreshReport::default();
        let mut run_budget = self.run_budget;
        let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
        let mut listed_dirs: BTreeSet<PathBuf> = BTreeSet::new();
        let mut owned_roots: Vec<PathBuf> = Vec::new();

        for source in SOURCES {
            if !sources.is_enabled(source.harness) {
                continue;
            }
            owned_roots.push((source.root)(home));
            let listing = (source.list)(home);
            if listing.incomplete {
                report.incomplete = true;
            }
            listed_dirs.extend(listing.listed_dirs);

            for path in listing.files {
                seen.insert(path.clone());

                let Ok(meta) = fs::metadata(&path) else {
                    continue;
                };
                let size = meta.len();
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);

                let (start_offset, mut uses, skip_to_newline) = match self.files.get(&path) {
                    Some(existing)
                        if existing.size == size
                            && existing.modified == modified
                            && existing.parsed_bytes >= size =>
                    {
                        continue;
                    }
                    Some(existing) if existing.size == size && existing.parsed_bytes >= size => {
                        // Same size, but the mtime moved: rewritten in place
                        // at exactly the old length. Reparse from scratch
                        // rather than trusting a byte-for-byte-identical-
                        // looking cache entry.
                        (0, Vec::new(), false)
                    }
                    Some(existing) if size < existing.parsed_bytes => (0, Vec::new(), false),
                    Some(existing) => {
                        let current_tail = read_tail_sample(&path, existing.parsed_bytes);
                        if current_tail != existing.tail_sample {
                            // The bytes just before our resume point no
                            // longer match what we parsed last time: this
                            // wasn't a plain append, so the cached uses may
                            // be stale.
                            (0, Vec::new(), false)
                        } else {
                            (
                                existing.parsed_bytes,
                                existing.uses.clone(),
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
                uses.extend((source.parse)(&text));
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
                        uses,
                        skipping_line,
                        tail_sample,
                    },
                );
            }
        }

        let before = self.files.len();
        self.files.retain(|path, _| {
            if seen.contains(path) {
                return true;
            }
            let Some(parent) = path.parent() else {
                return true;
            };
            if listed_dirs.contains(parent) {
                return false;
            }
            let owned = owned_roots.iter().any(|root| path.starts_with(root));
            if !owned {
                return true;
            }
            // The parent wasn't listed successfully this refresh, but its
            // owning source ran: drop only if the parent directory itself
            // is now gone, not on a merely transient read failure.
            !matches!(
                fs::symlink_metadata(parent),
                Err(e) if e.kind() == io::ErrorKind::NotFound
            )
        });
        report.files_dropped = before - self.files.len();
        report
    }

    /// Per-skill use totals across every cached transcript, with the rolling
    /// windows computed relative to `now`, over uses `filter` counts.
    pub fn stats_at(
        &self,
        now: DateTime<Utc>,
        filter: &SkillUseFilter,
    ) -> Vec<SkillInvocationStats> {
        skill_stats(self.all_uses(), filter, now)
    }

    /// Per-day use counts over the last `days` days, relative to `now`, over
    /// uses `filter` counts.
    pub fn heatmap_at(
        &self,
        days: u32,
        now: DateTime<Utc>,
        filter: &SkillUseFilter,
    ) -> InvocationHeatmap {
        skill_heatmap(self.all_uses(), filter, days, now)
    }

    fn all_uses(&self) -> impl Iterator<Item = &SkillInvocation> {
        self.files.values().flat_map(|t| t.uses.iter())
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
/// too big to fit this pass's budget (see below). An ordinary (in-budget)
/// final line at true EOF with no trailing `\n` is never committed - it may
/// still be being written - so the next refresh re-reads it from the same
/// offset (and it will be parseable once complete). A line that overruns
/// `MAX_LINE_BYTES`, or that alone can't fit `file_budget`, is drained and
/// skipped rather than buffered or parsed - including an oversized line
/// still unterminated at EOF: since an oversized line is unparseable even
/// once complete, re-reading it would only loop on the same bytes forever,
/// so it is committed (drained) and `skip_to_newline` resumes an in-progress
/// drain left over from a previous, budget-truncated pass, so a persistently
/// oversized line still makes progress instead of being re-read from
/// scratch (and truncated at the same point) forever.
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
            // Final line at true EOF with no trailing `\n`. An ordinary
            // (in-budget) line isn't committed so the next refresh re-reads
            // it from this same offset once it's complete - and it *will* be
            // parseable then. An oversized line, however, is never parsed
            // even once complete (it's drained-and-skipped below), so
            // reverting it would only re-read the same unparseable bytes
            // forever and keep `incomplete` wedged. Mirror the budget branch:
            // commit the drained bytes and resume draining next pass.
            if line_bytes > MAX_LINE_BYTES as u64 {
                skipping = true;
            } else {
                consumed -= line_bytes;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use skill_studio_core::skill_uses::SkillTrigger;
    use std::collections::BTreeSet as StdBTreeSet;
    use std::time::Duration;

    fn skill_line(skill: &str, timestamp: &str, cwd: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{timestamp}","cwd":"{cwd}","message":{{"content":[{{"type":"tool_use","name":"Skill","input":{{"skill":"{skill}"}}}}]}}}}"#
        )
    }

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

    fn known(skills: &[&str]) -> StdBTreeSet<String> {
        skills.iter().map(|s| s.to_string()).collect()
    }

    fn filter<'a>(
        known_skills: &'a StdBTreeSet<String>,
        sources: &'a DiscoverySources,
    ) -> SkillUseFilter<'a> {
        SkillUseFilter {
            known_skills,
            sources,
        }
    }

    fn stats(
        index: &SkillInvocationIndex,
        known_skills: &StdBTreeSet<String>,
        sources: &DiscoverySources,
    ) -> Vec<SkillInvocationStats> {
        index.stats_at(Utc::now(), &filter(known_skills, sources))
    }

    #[test]
    fn refresh_skips_unchanged_files() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let session_dir = home.join(CLAUDE_PROJECTS_ROOT).join("-my-project");
        write_transcript(
            &session_dir,
            "session.jsonl",
            "write-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );

        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        let first = index.refresh(home, &sources);
        assert_eq!(first.files_reparsed, 1);
        assert_eq!(stats(&index, &known_skills, &sources).len(), 1);

        let second = index.refresh(home, &sources);
        assert_eq!(second.files_reparsed, 0);
    }

    #[test]
    fn refresh_drops_deleted_files() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let session_dir = home.join(CLAUDE_PROJECTS_ROOT).join("-my-project");
        let path = write_transcript(
            &session_dir,
            "session.jsonl",
            "write-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );

        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        index.refresh(home, &sources);
        assert_eq!(stats(&index, &known_skills, &sources).len(), 1);

        fs::remove_file(&path).unwrap();
        let report = index.refresh(home, &sources);
        assert_eq!(report.files_dropped, 1);
        assert!(stats(&index, &known_skills, &sources).is_empty());
    }

    #[test]
    fn unreadable_projects_dir_keeps_cached_files_and_reports_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_transcript(
            &home.join(CLAUDE_PROJECTS_ROOT).join("-my-project"),
            "session.jsonl",
            "write-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );
        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        index.refresh(home, &sources);
        assert_eq!(stats(&index, &known_skills, &sources).len(), 1);

        // Point at a home whose `.claude/projects` doesn't exist: the
        // listing of the root itself fails.
        let missing_home = tmp.path().join("missing-home");
        fs::create_dir_all(&missing_home).unwrap();
        let report = index.refresh(&missing_home, &sources);
        assert!(report.incomplete);
        assert_eq!(report.files_dropped, 0);
        assert_eq!(stats(&index, &known_skills, &sources).len(), 1);
    }

    #[test]
    fn stats_totals_last_30_days_and_by_project_30_days() {
        let mut index = SkillInvocationIndex::default();
        let recent = Utc::now().to_rfc3339();
        let old = (Utc::now() - chrono::Duration::days(60)).to_rfc3339();
        let session_dir = tempfile::tempdir().unwrap();
        let home = session_dir.path();
        let dir = home.join(CLAUDE_PROJECTS_ROOT).join("-p");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("session.jsonl"),
            format!(
                "{}\n{}\n{}\n",
                skill_line("write-tests", &recent, "/proj-a"),
                skill_line("write-tests", &recent, "/proj-b"),
                skill_line("write-tests", &old, "/proj-a"),
            ),
        )
        .unwrap();

        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        index.refresh(home, &sources);
        let stats = stats(&index, &known_skills, &sources);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].total, 3);
        assert_eq!(stats[0].last_24_hours, 2);
        assert_eq!(stats[0].last_7_days, 2);
        assert_eq!(stats[0].last_14_days, 2);
        assert_eq!(stats[0].last_30_days, 2);
        assert_eq!(stats[0].by_project_30_days.get("/proj-a"), Some(&1));
        assert_eq!(stats[0].by_project_30_days.get("/proj-b"), Some(&1));
        let today = Utc::now().format("%Y-%m-%d").to_string();
        assert_eq!(stats[0].by_day.get(&today), Some(&2));
    }

    #[test]
    fn stats_at_windows_are_relative_to_the_given_now() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = Utc::now();
        let twenty_five_hours_ago = (now - chrono::Duration::hours(25)).to_rfc3339();
        let dir = home.join(CLAUDE_PROJECTS_ROOT).join("-p");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("session.jsonl"),
            format!(
                "{}\n",
                skill_line("write-tests", &twenty_five_hours_ago, "/proj-a")
            ),
        )
        .unwrap();

        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        index.refresh(home, &sources);
        let stats = index.stats_at(now, &filter(&known_skills, &sources));
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].last_24_hours, 0, "25h-old use counted in 24h");
        assert_eq!(stats[0].last_7_days, 1, "25h-old use missing from 7d");
    }

    #[test]
    fn heatmap_buckets_by_day() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let today = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let dir = home.join(CLAUDE_PROJECTS_ROOT).join("-p");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("session.jsonl"),
            format!(
                "{}\n{}\n",
                skill_line("write-tests", &today, "/proj-a"),
                skill_line("lint-code", &today, "/proj-a"),
            ),
        )
        .unwrap();

        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["write-tests", "lint-code"]);
        let sources = DiscoverySources::default();
        index.refresh(home, &sources);
        let heatmap = index.heatmap_at(30, Utc::now(), &filter(&known_skills, &sources));
        assert_eq!(heatmap.days.len(), 1);
        assert_eq!(*heatmap.days.values().next().unwrap(), 2);
    }

    #[test]
    fn oversized_line_is_skipped_and_later_lines_still_count() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let dir = home.join(CLAUDE_PROJECTS_ROOT).join("-huge-line");
        fs::create_dir_all(&dir).unwrap();
        let mut content = vec![b'x'; MAX_LINE_BYTES + 1024];
        content.push(b'\n');
        content.extend(skill_line("write-tests", "2026-08-01T12:00:00Z", "/proj").into_bytes());
        content.push(b'\n');
        fs::write(dir.join("session.jsonl"), content).unwrap();

        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        index.refresh(home, &sources);
        let stats = stats(&index, &known_skills, &sources);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].skill, "write-tests");
        assert_eq!(stats[0].total, 1);
    }

    #[test]
    fn append_to_transcript_only_parses_the_new_line() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let session_dir = home.join(CLAUDE_PROJECTS_ROOT).join("-my-project");
        let path = write_transcript(
            &session_dir,
            "session.jsonl",
            "write-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );

        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["write-tests", "lint-code"]);
        let sources = DiscoverySources::default();
        index.refresh(home, &sources);
        let parsed_bytes_after_first = index.files.get(&path).unwrap().parsed_bytes;
        assert_eq!(parsed_bytes_after_first, fs::metadata(&path).unwrap().len());

        let mut content = fs::read(&path).unwrap();
        content.extend(skill_line("lint-code", "2026-08-02T12:00:00Z", "/my-project").into_bytes());
        content.push(b'\n');
        fs::write(&path, content).unwrap();

        let report = index.refresh(home, &sources);
        assert_eq!(report.files_reparsed, 1);
        let stats = stats(&index, &known_skills, &sources);
        assert_eq!(stats.len(), 2);
        assert_eq!(
            index.files.get(&path).unwrap().parsed_bytes,
            fs::metadata(&path).unwrap().len()
        );
    }

    #[test]
    fn truncated_transcript_is_reparsed_from_scratch() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let session_dir = home.join(CLAUDE_PROJECTS_ROOT).join("-my-project");
        let path = write_transcript(
            &session_dir,
            "session.jsonl",
            "write-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );

        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["write-tests", "lint-code"]);
        let sources = DiscoverySources::default();
        index.refresh(home, &sources);
        assert_eq!(stats(&index, &known_skills, &sources)[0].total, 1);

        fs::write(
            &path,
            format!(
                "{}\n",
                skill_line("lint-code", "2026-08-02T12:00:00Z", "/other")
            ),
        )
        .unwrap();
        index.refresh(home, &sources);
        let stats = stats(&index, &known_skills, &sources);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].skill, "lint-code");
    }

    #[test]
    fn file_larger_than_budget_finishes_over_multiple_refreshes() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let session_dir = home.join(CLAUDE_PROJECTS_ROOT).join("-my-project");
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
        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        let first = index.refresh(home, &sources);
        assert!(first.incomplete);
        assert!(stats(&index, &known_skills, &sources)[0].total < 20);

        let mut passes = 0;
        loop {
            let report = index.refresh(home, &sources);
            passes += 1;
            if !report.incomplete {
                break;
            }
            assert!(passes < 50, "backlog never drained");
        }
        assert_eq!(stats(&index, &known_skills, &sources)[0].total, 20);
    }

    #[test]
    fn partial_final_line_is_not_committed_until_terminated() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let session_dir = home.join(CLAUDE_PROJECTS_ROOT).join("-my-project");
        fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("session.jsonl");
        let full = skill_line("write-tests", "2026-08-01T12:00:00Z", "/my-project");
        let half = &full[..full.len() / 2];
        fs::write(&path, half).unwrap();

        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        index.refresh(home, &sources);
        assert!(
            stats(&index, &known_skills, &sources).is_empty(),
            "half-written record was committed"
        );
        assert_eq!(index.files.get(&path).unwrap().parsed_bytes, 0);

        fs::write(&path, format!("{full}\n")).unwrap();
        index.refresh(home, &sources);
        let stats = stats(&index, &known_skills, &sources);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].skill, "write-tests");
    }

    #[test]
    fn line_larger_than_the_file_budget_is_skipped_over_multiple_passes() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let session_dir = home.join(CLAUDE_PROJECTS_ROOT).join("-my-project");
        fs::create_dir_all(&session_dir).unwrap();
        let mut content = vec![b'x'; 2000];
        content.push(b'\n');
        content.extend(skill_line("write-tests", "2026-08-01T12:00:00Z", "/proj").into_bytes());
        content.push(b'\n');
        fs::write(session_dir.join("session.jsonl"), &content).unwrap();

        let mut index = SkillInvocationIndex::with_budgets(200, 200);
        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        let first = index.refresh(home, &sources);
        assert!(first.incomplete);
        assert!(stats(&index, &known_skills, &sources).is_empty());

        let mut passes = 1;
        loop {
            let report = index.refresh(home, &sources);
            passes += 1;
            if !report.incomplete {
                break;
            }
            assert!(passes < 50, "oversized line never drained");
        }
        assert!(passes > 1, "expected the skip to span multiple passes");
        let stats = stats(&index, &known_skills, &sources);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].skill, "write-tests");
    }

    #[test]
    fn oversized_unterminated_final_line_does_not_stall() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let session_dir = home.join(CLAUDE_PROJECTS_ROOT).join("-my-project");
        fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("session.jsonl");
        let content = vec![b'x'; MAX_LINE_BYTES + 1024];
        fs::write(&path, &content).unwrap();

        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        let first = index.refresh(home, &sources);
        assert!(!first.incomplete);
        let entry = index.files.get(&path).unwrap();
        assert_eq!(entry.parsed_bytes as usize, content.len());
        assert!(entry.skipping_line);
        assert!(stats(&index, &known_skills, &sources).is_empty());

        let second = index.refresh(home, &sources);
        assert_eq!(second.files_reparsed, 0);
        assert!(!second.incomplete);
        assert_eq!(
            index.files.get(&path).unwrap().parsed_bytes as usize,
            content.len()
        );
        assert!(stats(&index, &known_skills, &sources).is_empty());
    }

    #[test]
    fn same_size_rewrite_is_detected_via_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let session_dir = home.join(CLAUDE_PROJECTS_ROOT).join("-my-project");
        let path = write_transcript(
            &session_dir,
            "session.jsonl",
            "run-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );

        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["run-tests", "lint-code"]);
        let sources = DiscoverySources::default();
        index.refresh(home, &sources);
        assert_eq!(stats(&index, &known_skills, &sources)[0].skill, "run-tests");

        let original_size = fs::metadata(&path).unwrap().len();
        let new_content = format!(
            "{}\n",
            skill_line("lint-code", "2026-08-01T12:00:00Z", "/my-project")
        );
        assert_eq!(new_content.len() as u64, original_size);
        fs::write(&path, &new_content).unwrap();
        let bumped_mtime =
            fs::metadata(&path).unwrap().modified().unwrap() + Duration::from_secs(1);
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(bumped_mtime)
            .unwrap();

        index.refresh(home, &sources);
        let stats = stats(&index, &known_skills, &sources);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].skill, "lint-code");
    }

    #[test]
    fn truncate_and_regrow_clears_stale_uses_via_tail_sample() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let session_dir = home.join(CLAUDE_PROJECTS_ROOT).join("-my-project");
        let path = write_transcript(
            &session_dir,
            "session.jsonl",
            "write-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );

        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["write-tests", "lint-code"]);
        let sources = DiscoverySources::default();
        index.refresh(home, &sources);
        assert_eq!(
            stats(&index, &known_skills, &sources)[0].skill,
            "write-tests"
        );

        let mut new_content = String::from("short\n");
        new_content.push_str(&skill_line("lint-code", "2026-08-02T12:00:00Z", "/other"));
        new_content.push('\n');
        fs::write(&path, &new_content).unwrap();

        index.refresh(home, &sources);
        let stats = stats(&index, &known_skills, &sources);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].skill, "lint-code");
    }

    #[test]
    fn corrupt_cache_yields_empty_index_and_leaves_a_corrupt_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_path = tmp.path().join("skill-uses.json");
        fs::write(&cache_path, "not valid json").unwrap();

        let index = SkillInvocationIndex::load_or_empty(&cache_path);
        assert!(index.files.is_empty());
        assert!(!cache_path.exists());
        assert!(cache_path
            .with_file_name("skill-uses.json.corrupt")
            .exists());
    }

    #[test]
    fn save_writes_via_tmp_then_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_path = tmp.path().join("nested/skill-uses.json");
        let index = SkillInvocationIndex::default();
        index.save(&cache_path).unwrap();
        assert!(cache_path.exists());
        assert!(!cache_path.with_file_name("skill-uses.json.tmp").exists());
    }

    #[test]
    fn subagent_transcript_use_is_counted() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let subagents_dir = home.join(CLAUDE_PROJECTS_ROOT).join("p/s1/subagents");
        write_transcript(
            &subagents_dir,
            "a.jsonl",
            "write-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );

        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        index.refresh(home, &sources);
        let stats = stats(&index, &known_skills, &sources);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].total, 1);
    }

    #[test]
    fn typed_command_and_its_is_meta_copy_give_one_user_use() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let dir = home.join(CLAUDE_PROJECTS_ROOT).join("-p");
        fs::create_dir_all(&dir).unwrap();
        let typed_at = Utc::now().to_rfc3339();
        let meta_at = (Utc::now() + chrono::Duration::seconds(1)).to_rfc3339();
        let typed = format!(
            r#"{{"type":"user","timestamp":"{typed_at}","message":{{"content":"<command-message>deploy</command-message>\n<command-name>/deploy</command-name>"}}}}"#
        );
        let meta_copy = format!(
            r#"{{"type":"user","timestamp":"{meta_at}","isMeta":true,"message":{{"content":[{{"type":"text","text":"<command-name>/deploy</command-name>"}}]}}}}"#
        );
        fs::write(dir.join("session.jsonl"), format!("{typed}\n{meta_copy}\n")).unwrap();

        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["deploy"]);
        let sources = DiscoverySources::default();
        index.refresh(home, &sources);
        let stats = stats(&index, &known_skills, &sources);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].skill, "deploy");
        assert_eq!(stats[0].total, 1);
        assert_eq!(stats[0].by_trigger_30_days.user, 1);
        let _ = SkillTrigger::User; // referenced for readability of the assertion above
    }

    #[test]
    fn switched_off_harness_reads_nothing_and_keeps_cached_uses() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let session_dir = home.join(CLAUDE_PROJECTS_ROOT).join("-my-project");
        write_transcript(
            &session_dir,
            "session.jsonl",
            "write-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );

        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["write-tests"]);
        let enabled = DiscoverySources::default();
        index.refresh(home, &enabled);
        assert_eq!(stats(&index, &known_skills, &enabled).len(), 1);

        let mut off = DiscoverySources::default();
        off.set("claude-code", false);
        let report = index.refresh(home, &off);
        assert_eq!(report.files_reparsed, 0);
        assert_eq!(index.files.len(), 1, "cached entries are kept");
        assert!(stats(&index, &known_skills, &off).is_empty());
    }

    #[test]
    fn deleting_a_session_directory_drops_its_subagent_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let session_dir = home.join(CLAUDE_PROJECTS_ROOT).join("p/s1");
        write_transcript(
            &session_dir.join("subagents"),
            "a.jsonl",
            "write-tests",
            "2026-08-01T12:00:00Z",
            "/my-project",
        );

        let mut index = SkillInvocationIndex::default();
        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        index.refresh(home, &sources);
        assert_eq!(stats(&index, &known_skills, &sources).len(), 1);

        fs::remove_dir_all(&session_dir).unwrap();
        let report = index.refresh(home, &sources);
        assert_eq!(report.files_dropped, 1);
        assert!(stats(&index, &known_skills, &sources).is_empty());
    }
}
