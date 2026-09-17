//! Skill-use index: parses each enabled harness's own session history for
//! skill uses and keeps a per-source cache so a refresh only re-reads what
//! changed. Two kinds of source exist: append-only JSONL transcripts,
//! resumed from a byte offset (Claude Code, Codex), and SQLite databases,
//! re-queried from a `time_updated` watermark (OpenCode). Read discipline
//! mirrors `discovery.rs`: only regular files are opened, each transcript
//! line is capped so a pathological line can't be buffered in full, and a
//! file/run byte budget bounds worst-case I/O per refresh.
//!
//! A transcript source's `parse` function takes a
//! [`TranscriptContext`](skill_studio_core::skill_uses::TranscriptContext),
//! carried across lines and (for a resumed, not reparsed, file) across
//! refreshes - Codex states its session id and project path once, in a
//! header line, rather than repeating them on every line the way Claude Code
//! does.
//!
//! `SOURCES` is the table of harnesses this index reads from: Claude Code,
//! pi, and Cursor each watch one transcript root; Codex watches two
//! (`sessions`, `archived_sessions`); OpenCode reads its SQLite databases
//! instead of JSONL; Grok Build watches `.grok/sessions` and only lists a
//! session's `updates.jsonl` once its `summary.json` also exists. Adding a
//! harness later means adding a row, not reworking `refresh`.

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
    parse_codex_uses, parse_cursor_uses, parse_grok_uses, parse_pi_uses, skill_heatmap,
    skill_stats, InvocationHeatmap, SkillInvocation, SkillInvocationStats, SkillUseFilter,
    TranscriptContext,
};

use crate::discovery::{cursor_workspace_folders, grok_session_cwd, MAX_GROK_SESSION_DIRS};
use crate::opencode_db::{opencode_databases, OPENCODE_DATA_ROOT};

mod opencode;

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
    /// State the transcript's `parse` function carries across lines (see the
    /// module doc). Resumed alongside `parsed_bytes` when a refresh appends;
    /// reset to [`TranscriptContext::default`] whenever the file is
    /// reparsed from byte 0, so a rewritten file's uses never carry a stale
    /// session or project path.
    #[serde(default)]
    context: TranscriptContext,
}

/// How many bytes of a transcript's already-parsed tail are kept for
/// rewrite detection (see `IndexedTranscript::tail_sample`).
const TAIL_SAMPLE_BYTES: u64 = 64;

/// Size and mtime of a database file and of its `-wal` file, used to tell a
/// changed database from an unchanged one without reopening it. `0` /
/// `UNIX_EPOCH` when a file (typically the `-wal`) is absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct DatabaseStamp {
    db_size: u64,
    db_modified: SystemTime,
    wal_size: u64,
    wal_modified: SystemTime,
}

impl Default for DatabaseStamp {
    fn default() -> Self {
        Self {
            db_size: 0,
            db_modified: SystemTime::UNIX_EPOCH,
            wal_size: 0,
            wal_modified: SystemTime::UNIX_EPOCH,
        }
    }
}

fn file_size_and_mtime(path: &Path) -> (u64, SystemTime) {
    fs::metadata(path)
        .map(|meta| {
            (
                meta.len(),
                meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            )
        })
        .unwrap_or((0, SystemTime::UNIX_EPOCH))
}

fn database_stamp(path: &Path) -> DatabaseStamp {
    let (db_size, db_modified) = file_size_and_mtime(path);
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    let (wal_size, wal_modified) = file_size_and_mtime(Path::new(&wal));
    DatabaseStamp {
        db_size,
        db_modified,
        wal_size,
        wal_modified,
    }
}

/// A database's cached parse result, keyed by [`DatabaseStamp`] so a refresh
/// can tell whether it needs to be re-queried.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct IndexedDatabase {
    stamp: DatabaseStamp,
    /// Highest `time_updated` seen per table (`session_message`, `part`), so
    /// a refresh only re-queries rows that changed since the last pass.
    watermarks: BTreeMap<String, i64>,
    /// Uses per row id (keyed `"<table>:<id>"`), only for rows that gave at
    /// least one use.
    rows: BTreeMap<String, Vec<SkillInvocation>>,
}

/// Outcome of one `SkillInvocationIndex::refresh` call.
#[derive(Debug, Clone, Default)]
pub struct SkillUseRefreshReport {
    /// Number of transcript files re-parsed (in full or in part) this call.
    pub files_reparsed: usize,
    /// Number of cached files or databases removed because they no longer
    /// exist.
    pub files_dropped: usize,
    /// Total bytes read from transcripts this call.
    pub bytes_read: u64,
    /// Number of databases successfully re-queried this call (their stamp
    /// had changed since the last refresh).
    pub databases_read: usize,
    /// Set when a per-file or per-run budget stopped a file short of EOF, or
    /// a source's own listing failed; a short file is left with
    /// `parsed_bytes < size` so a later refresh resumes and finishes
    /// draining the backlog.
    pub incomplete: bool,
}

/// What a transcript parser may need to know besides the text: where the
/// file lives, and when it was last written (used by Cursor, whose lines
/// carry no timestamp of their own).
struct TranscriptFile<'a> {
    home: &'a Path,
    path: &'a Path,
    /// The file's modification time, or now when the platform can't tell.
    modified: DateTime<Utc>,
}

/// How one [`UseSource`] reads its uses: an append-only transcript, resumed
/// from a byte offset, or a SQLite database, re-queried from a watermark.
enum UseReader {
    /// Append-only JSONL transcripts, parsed incrementally from a byte
    /// offset.
    Transcripts {
        list: fn(&Path) -> SourceListing,
        parse: fn(&TranscriptFile, &str, &mut TranscriptContext) -> Vec<SkillInvocation>,
    },
    /// SQLite databases, re-queried from a `time_updated` watermark.
    Databases {
        list: fn(&Path) -> Vec<PathBuf>,
        read: fn(&Path, &mut IndexedDatabase) -> bool,
    },
}

/// One harness's session history this index reads uses from: where to find
/// it under `home` (`root`) and how to read it (`reader`).
struct UseSource {
    /// `AgentId` wire name, e.g. `AgentId::CLAUDE_CODE`.
    harness: &'static str,
    /// The root directory this source reads under `home`, used to scope the
    /// drop rule to files this source (when enabled) actually owns, so a
    /// switched-off source's cached files are left untouched even if their
    /// directory is later removed.
    root: fn(&Path) -> PathBuf,
    reader: UseReader,
    /// Where the app should watch on disk for changes that can add, change,
    /// or remove this source's uses - see [`skill_use_watch_paths`].
    watch: &'static [SourceWatch],
}

/// A directory under `home` whose changes can add, change, or remove this
/// source's uses.
struct SourceWatch {
    /// Relative to `home`.
    dir: &'static str,
    recursive: bool,
    /// Which changed paths under `dir` matter, given relative to `dir`.
    accepts: fn(&Path) -> bool,
}

/// Matches any path - Claude Code's and pi's transcript directories have no
/// path-based filter, since every file under them can hold uses.
fn any_path(_: &Path) -> bool {
    true
}

/// True when `rel` (a single file name: this watch is non-recursive) is an
/// OpenCode database file, or that database's `-wal` sidecar. `-shm` is
/// excluded on purpose: a read-only reader (ours included) can touch `-shm`
/// just by opening the database, so treating it as a use-changing event
/// would make our own reads queue another refresh.
fn is_opencode_database_or_wal(rel: &Path) -> bool {
    let Some(name) = rel.to_str() else {
        return false;
    };
    match name.strip_suffix("-wal") {
        Some(db_name) => crate::opencode_db::is_opencode_database_name(db_name),
        None => crate::opencode_db::is_opencode_database_name(name),
    }
}

/// True when the second component of `rel` (`<project>/agent-transcripts/
/// ...`) is `agent-transcripts`. A recursive watch of `.cursor/projects`
/// also sees Cursor's terminal logs and tool files under other project
/// subdirectories: this keeps them out.
fn is_cursor_transcript_path(rel: &Path) -> bool {
    let mut components = rel.components();
    components.next(); // <project>
    components
        .next()
        .is_some_and(|c| c.as_os_str() == "agent-transcripts")
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
/// missing `projects` root is normal (Claude Code was never installed) and
/// yields an empty, complete listing; any other failure to list it, or a
/// failed listing of a `<project>` directory, marks the listing incomplete.
/// A missing `subagents` directory is normal (most sessions have no
/// subagents) and does not.
fn list_claude_code_transcripts(home: &Path) -> SourceListing {
    let mut files = Vec::new();
    let mut listed_dirs = BTreeSet::new();
    let mut incomplete = false;

    let projects_dir = claude_code_root(home);
    let project_dirs = match fs::read_dir(&projects_dir) {
        Ok(dirs) => dirs,
        // No Claude Code on this machine: an empty listing, not a failure,
        // so the desktop refresh loop (which re-runs while `incomplete` is
        // set) doesn't spin forever for a user who never installed it.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return SourceListing {
                files,
                listed_dirs,
                incomplete: false,
            };
        }
        Err(_) => {
            return SourceListing {
                files,
                listed_dirs,
                incomplete: true,
            };
        }
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

/// Adapts [`parse_claude_code_uses`] to the [`UseReader::Transcripts`]
/// `parse` signature. Claude Code repeats its session id and cwd on every
/// record, so unlike Codex it needs no [`TranscriptContext`], and it never
/// needs the file itself.
fn parse_claude_code_uses_with_context(
    _file: &TranscriptFile,
    text: &str,
    _context: &mut TranscriptContext,
) -> Vec<SkillInvocation> {
    parse_claude_code_uses(text)
}

/// Adapts [`parse_codex_uses`] to the [`UseReader::Transcripts`] `parse`
/// signature; Codex never needs the file itself.
fn parse_codex_uses_with_file(
    _file: &TranscriptFile,
    text: &str,
    context: &mut TranscriptContext,
) -> Vec<SkillInvocation> {
    parse_codex_uses(text, context)
}

fn opencode_root(home: &Path) -> PathBuf {
    home.join(OPENCODE_DATA_ROOT)
}

/// Codex's live rollouts under the default `<home>/.codex`; also used
/// (stripped of its `.codex/` prefix) as the sub-path under an overridden
/// [`codex_root`].
const CODEX_SESSIONS_DIR: &str = ".codex/sessions";
/// Rollouts Codex has moved aside (still readable, never appended to again),
/// under the default `<home>/.codex`; see [`CODEX_SESSIONS_DIR`].
const CODEX_ARCHIVED_SESSIONS_DIR: &str = ".codex/archived_sessions";

/// Codex's own directory: `$CODEX_HOME`, or `<home>/.codex` when unset (see
/// [`crate::discovery::codex_home`]). [`SourceWatch`] entries still watch
/// the default `.codex/sessions` and `.codex/archived_sessions` under
/// `home` - watching a `CODEX_HOME` override too is a follow-up.
fn codex_root(home: &Path) -> PathBuf {
    crate::discovery::codex_home(home)
}

/// How many directory levels [`list_codex_rollouts`] descends below each of
/// `sessions` and `archived_sessions`: enough for the dated `YYYY/MM/DD`
/// layout with room to spare, without walking the rest of [`codex_root`]
/// (plugin caches, logs, state databases - all churn constantly and hold no
/// skill-use signal) should a rollout ever nest deeper than expected.
const CODEX_WALK_DEPTH: u32 = 4;

/// Lists Codex's rollout transcripts: `<codex_root>/sessions/**/*.jsonl` and
/// `<codex_root>/archived_sessions/**/*.jsonl`, walked to
/// [`CODEX_WALK_DEPTH`] levels below each. A missing top dir is normal
/// (Codex was never installed, or has archived nothing yet); any other
/// failure to list a directory marks the listing incomplete.
fn list_codex_rollouts(home: &Path) -> SourceListing {
    let mut files = Vec::new();
    let mut listed_dirs = BTreeSet::new();
    let mut incomplete = false;

    let root = codex_root(home);
    for top in ["sessions", "archived_sessions"] {
        walk_jsonl_files(
            &root.join(top),
            CODEX_WALK_DEPTH,
            &mut files,
            &mut listed_dirs,
            &mut incomplete,
        );
    }

    SourceListing {
        files,
        listed_dirs,
        incomplete,
    }
}

/// One directory's share of a source's listing: lists `dir`, collects its
/// `.jsonl` files, and (while `depth_remaining` allows) recurses into its
/// real (non-symlink) subdirectories. Shared by Codex, pi and Cursor. A
/// missing `dir` is normal (nothing has been written there yet) and not an
/// error; any other failure to list it marks `incomplete`.
fn walk_jsonl_files(
    dir: &Path,
    depth_remaining: u32,
    files: &mut Vec<PathBuf>,
    listed_dirs: &mut BTreeSet<PathBuf>,
    incomplete: &mut bool,
) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(_) => {
            *incomplete = true;
            return;
        }
    };
    listed_dirs.insert(dir.to_path_buf());

    for entry in entries.flatten() {
        let path = entry.path();
        if is_jsonl(&path) {
            files.push(path);
            continue;
        }
        if depth_remaining == 0 {
            continue;
        }
        let is_real_dir = fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_dir());
        if is_real_dir {
            walk_jsonl_files(&path, depth_remaining - 1, files, listed_dirs, incomplete);
        }
    }
}

/// pi keeps one directory per session here:
/// `.pi/agent/sessions/<dir>/<file>.jsonl`.
const PI_SESSIONS_ROOT: &str = ".pi/agent/sessions";

/// How many directory levels [`list_pi_sessions`] descends below
/// `.pi/agent/sessions` - one level covers the known layout, with the same
/// room to spare as [`CODEX_WALK_DEPTH`].
const PI_WALK_DEPTH: u32 = 4;

fn pi_root(home: &Path) -> PathBuf {
    home.join(PI_SESSIONS_ROOT)
}

/// Lists pi's session transcripts: `<home>/.pi/agent/sessions/**/*.jsonl`. A
/// missing top dir is normal (pi was never installed); any other failure to
/// list a directory marks the listing incomplete.
fn list_pi_sessions(home: &Path) -> SourceListing {
    let mut files = Vec::new();
    let mut listed_dirs = BTreeSet::new();
    let mut incomplete = false;

    walk_jsonl_files(
        &home.join(PI_SESSIONS_ROOT),
        PI_WALK_DEPTH,
        &mut files,
        &mut listed_dirs,
        &mut incomplete,
    );

    SourceListing {
        files,
        listed_dirs,
        incomplete,
    }
}

/// Adapts [`parse_pi_uses`] to the [`UseReader::Transcripts`] `parse`
/// signature; pi never needs the file itself.
fn parse_pi_uses_with_file(
    _file: &TranscriptFile,
    text: &str,
    context: &mut TranscriptContext,
) -> Vec<SkillInvocation> {
    parse_pi_uses(text, context)
}

/// Cursor keeps one directory per opened project here, each holding its own
/// `agent-transcripts` directory of session (and subagent) transcripts.
const CURSOR_PROJECTS_ROOT: &str = ".cursor/projects";

/// How many directory levels [`list_cursor_transcripts`] descends below each
/// project's `agent-transcripts`: covers `<session>/<session>.jsonl` and
/// `<session>/subagents/<id>.jsonl`, with a level of room to spare.
const CURSOR_TRANSCRIPTS_WALK_DEPTH: u32 = 3;

fn cursor_root(home: &Path) -> PathBuf {
    home.join(CURSOR_PROJECTS_ROOT)
}

/// Lists Cursor's transcripts: for each real (non-symlink) directory under
/// `<home>/.cursor/projects`, `<project>/agent-transcripts/**/*.jsonl`. A
/// missing `.cursor/projects` is normal (Cursor was never installed); a
/// missing `agent-transcripts` under a given project is normal too (most
/// project dirs hold none). Any other failure to list a directory marks the
/// listing incomplete.
fn list_cursor_transcripts(home: &Path) -> SourceListing {
    let mut files = Vec::new();
    let mut listed_dirs = BTreeSet::new();
    let mut incomplete = false;

    let projects_dir = home.join(CURSOR_PROJECTS_ROOT);
    let project_dirs = match fs::read_dir(&projects_dir) {
        Ok(dirs) => dirs,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return SourceListing {
                files,
                listed_dirs,
                incomplete: false,
            };
        }
        Err(_) => {
            return SourceListing {
                files,
                listed_dirs,
                incomplete: true,
            };
        }
    };
    for project_dir in project_dirs.flatten() {
        let dir = project_dir.path();
        let is_real_dir = fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_dir());
        if !is_real_dir {
            continue;
        }
        walk_jsonl_files(
            &dir.join("agent-transcripts"),
            CURSOR_TRANSCRIPTS_WALK_DEPTH,
            &mut files,
            &mut listed_dirs,
            &mut incomplete,
        );
    }

    SourceListing {
        files,
        listed_dirs,
        incomplete,
    }
}

/// The directory directly under `agent-transcripts` in `path` - the
/// session id, shared by a session's own transcript and its
/// `subagents/<id>.jsonl` siblings.
fn cursor_session_from_path(path: &Path) -> Option<String> {
    let mut components = path.components();
    loop {
        let component = components.next()?;
        if component.as_os_str() == "agent-transcripts" {
            return components.next()?.as_os_str().to_str().map(str::to_string);
        }
    }
}

/// The directory name directly under `.cursor/projects` in `path` - the
/// encoded project name [`cursor_project_dir_name`] produces.
fn cursor_project_dir_from_path(home: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(home.join(CURSOR_PROJECTS_ROOT)).ok()?;
    rel.components()
        .next()?
        .as_os_str()
        .to_str()
        .map(str::to_string)
}

/// Cursor's `projects/<name>` encoding of a workspace folder's absolute
/// path: the path without its leading `/`, with every character that isn't
/// an ASCII letter or digit replaced by `-`.
fn cursor_project_dir_name(folder: &Path) -> String {
    let s = folder.to_string_lossy();
    let s = s.strip_prefix('/').unwrap_or(&s);
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The one Cursor workspace folder under `home` whose encoded name
/// ([`cursor_project_dir_name`]) equals `project_dir_name`; `None` when no
/// folder matches, or more than one does.
fn cursor_project_path(home: &Path, project_dir_name: &str) -> Option<String> {
    let mut matches = cursor_workspace_folders(home)
        .into_iter()
        .filter(|folder| cursor_project_dir_name(folder) == project_dir_name);
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first.to_string_lossy().into_owned())
}

/// Adapts [`parse_cursor_uses`] to the [`UseReader::Transcripts`] `parse`
/// signature. Cursor transcript lines carry no session id, project path, or
/// timestamp of their own (see the core module doc), so the first call for
/// a file fills the context from `file.path`/`file.home` before parsing;
/// later calls (a resumed, appended file) reuse the context already stored
/// alongside it, so the folder lookup only runs once per file.
fn parse_cursor_uses_with_file(
    file: &TranscriptFile,
    text: &str,
    context: &mut TranscriptContext,
) -> Vec<SkillInvocation> {
    if context.session.is_none() {
        context.session = cursor_session_from_path(file.path);
        context.project_path = cursor_project_dir_from_path(file.home, file.path)
            .and_then(|name| cursor_project_path(file.home, &name));
    }
    parse_cursor_uses(text, context, file.modified)
}

/// Grok Build keeps one directory per working folder here, each holding one
/// subdirectory per session (`<encoded cwd>/<session id>/{updates.jsonl,
/// summary.json}`); a session only counts once it has a `summary.json` (see
/// `list_grok_sessions`).
const GROK_SESSIONS_ROOT: &str = ".grok/sessions";

/// `summary.json` is read only to check for `forked_at`; a file bigger than
/// this is treated as not forked rather than read in full.
const MAX_GROK_SUMMARY_BYTES: u64 = 1024 * 1024;

fn grok_root(home: &Path) -> PathBuf {
    home.join(GROK_SESSIONS_ROOT)
}

/// True when `rel` (relative to `.grok/sessions`) is a session's
/// `updates.jsonl` or `summary.json`: `<cwd dir>/<session>/<file>`, exactly
/// three components. A new `summary.json` must pass this watch, because
/// `list_grok_sessions` otherwise skips the session until one exists.
fn is_grok_session_file(rel: &Path) -> bool {
    rel.components().count() == 3
        && matches!(
            rel.file_name().and_then(|n| n.to_str()),
            Some("updates.jsonl") | Some("summary.json")
        )
}

/// Lists Grok Build's session transcripts: for each real (non-symlink)
/// working-folder directory under `<home>/.grok/sessions`, each real
/// (non-symlink) session directory whose `updates.jsonl` and `summary.json`
/// are both regular files. A session with no `summary.json` yet is skipped,
/// not counted incomplete - it isn't a session until Grok Build finishes
/// writing one. A missing `.grok/sessions` is normal (Grok Build was never
/// installed); any other failure to list a directory marks the listing
/// incomplete.
fn list_grok_sessions(home: &Path) -> SourceListing {
    let mut files = Vec::new();
    let mut listed_dirs = BTreeSet::new();
    let mut incomplete = false;

    let sessions_dir = home.join(GROK_SESSIONS_ROOT);
    let cwd_dirs = match fs::read_dir(&sessions_dir) {
        Ok(dirs) => dirs,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return SourceListing {
                files,
                listed_dirs,
                incomplete: false,
            };
        }
        Err(_) => {
            return SourceListing {
                files,
                listed_dirs,
                incomplete: true,
            };
        }
    };
    listed_dirs.insert(sessions_dir.clone());

    for cwd_entry in cwd_dirs.flatten().take(MAX_GROK_SESSION_DIRS) {
        let cwd_dir = cwd_entry.path();
        let is_real_dir = fs::symlink_metadata(&cwd_dir).is_ok_and(|m| m.file_type().is_dir());
        if !is_real_dir {
            continue;
        }
        let session_entries = match fs::read_dir(&cwd_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                incomplete = true;
                continue;
            }
        };
        listed_dirs.insert(cwd_dir.clone());

        for session_entry in session_entries.flatten() {
            let session_dir = session_entry.path();
            let is_real_session_dir =
                fs::symlink_metadata(&session_dir).is_ok_and(|m| m.file_type().is_dir());
            if !is_real_session_dir {
                continue;
            }
            listed_dirs.insert(session_dir.clone());

            let updates = session_dir.join("updates.jsonl");
            let summary = session_dir.join("summary.json");
            if is_regular_file(&updates) && is_regular_file(&summary) {
                files.push(updates);
            }
        }
    }

    SourceListing {
        files,
        listed_dirs,
        incomplete,
    }
}

/// `summary.json`'s only key this reader needs; unknown keys are ignored.
#[derive(Deserialize)]
struct GrokSummary {
    forked_at: Option<String>,
}

/// The session directory's `summary.json` `forked_at`, parsed as RFC 3339 -
/// `None` when there is no fork, the file is missing or oversized
/// (see [`MAX_GROK_SUMMARY_BYTES`]), or its `forked_at` doesn't parse.
fn grok_forked_at(session_dir: &Path) -> Option<DateTime<Utc>> {
    let path = session_dir.join("summary.json");
    let meta = fs::metadata(&path).ok()?;
    if meta.len() > MAX_GROK_SUMMARY_BYTES {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    let summary: GrokSummary = serde_json::from_str(&content).ok()?;
    DateTime::parse_from_rfc3339(&summary.forked_at?)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// Adapts [`parse_grok_uses`] to the [`UseReader::Transcripts`] `parse`
/// signature. Grok Build's `updates.jsonl` lines carry no session id or
/// project path of their own, so the first call for a file fills the context
/// from the file's path (the session directory name, and its parent decoded
/// by `discovery.rs`'s `grok_session_cwd`) and from `summary.json`'s
/// `forked_at`, before parsing; later calls (a resumed, appended file) reuse
/// the context already stored alongside it.
fn parse_grok_uses_with_file(
    file: &TranscriptFile,
    text: &str,
    context: &mut TranscriptContext,
) -> Vec<SkillInvocation> {
    if context.session.is_none() {
        let session_dir = file.path.parent();
        context.session = session_dir
            .and_then(|dir| dir.file_name())
            .and_then(|name| name.to_str())
            .map(str::to_string);
        context.project_path = session_dir
            .and_then(|dir| dir.parent())
            .and_then(grok_session_cwd)
            .map(|cwd| cwd.to_string_lossy().into_owned());
        context.forked_at = session_dir.and_then(grok_forked_at);
    }
    parse_grok_uses(text, context, file.modified)
}

/// Every harness this index reads uses from, in the order they're processed.
const SOURCES: &[UseSource] = &[
    UseSource {
        harness: AgentId::CLAUDE_CODE,
        root: claude_code_root,
        reader: UseReader::Transcripts {
            list: list_claude_code_transcripts,
            parse: parse_claude_code_uses_with_context,
        },
        watch: &[SourceWatch {
            dir: CLAUDE_PROJECTS_ROOT,
            recursive: true,
            accepts: any_path,
        }],
    },
    UseSource {
        harness: AgentId::CODEX,
        root: codex_root,
        reader: UseReader::Transcripts {
            list: list_codex_rollouts,
            parse: parse_codex_uses_with_file,
        },
        watch: &[
            SourceWatch {
                dir: CODEX_SESSIONS_DIR,
                recursive: true,
                accepts: any_path,
            },
            SourceWatch {
                dir: CODEX_ARCHIVED_SESSIONS_DIR,
                recursive: true,
                accepts: any_path,
            },
        ],
    },
    UseSource {
        harness: AgentId::OPEN_CODE,
        root: opencode_root,
        reader: UseReader::Databases {
            list: opencode_databases,
            read: opencode::read_database,
        },
        watch: &[SourceWatch {
            dir: OPENCODE_DATA_ROOT,
            recursive: false,
            accepts: is_opencode_database_or_wal,
        }],
    },
    UseSource {
        harness: AgentId::PI,
        root: pi_root,
        reader: UseReader::Transcripts {
            list: list_pi_sessions,
            parse: parse_pi_uses_with_file,
        },
        watch: &[SourceWatch {
            dir: PI_SESSIONS_ROOT,
            recursive: true,
            accepts: any_path,
        }],
    },
    UseSource {
        harness: AgentId::CURSOR,
        root: cursor_root,
        reader: UseReader::Transcripts {
            list: list_cursor_transcripts,
            parse: parse_cursor_uses_with_file,
        },
        watch: &[SourceWatch {
            dir: CURSOR_PROJECTS_ROOT,
            recursive: true,
            accepts: is_cursor_transcript_path,
        }],
    },
    UseSource {
        harness: AgentId::GROK_BUILD,
        root: grok_root,
        reader: UseReader::Transcripts {
            list: list_grok_sessions,
            parse: parse_grok_uses_with_file,
        },
        watch: &[SourceWatch {
            dir: GROK_SESSIONS_ROOT,
            recursive: true,
            accepts: is_grok_session_file,
        }],
    },
];

/// A directory the app should watch so session-history changes refresh
/// skill uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillUseWatchPath {
    /// The directory to watch, absolute (`home` joined with the source's
    /// relative watch dir).
    pub path: PathBuf,
    /// Whether the watch should recurse into subdirectories.
    pub recursive: bool,
}

/// Every directory to watch for skill-use changes, for every source
/// (switched-off harnesses included: a refresh skips them anyway, and the
/// watch set then doesn't depend on settings).
pub fn skill_use_watch_paths(home: &Path) -> Vec<SkillUseWatchPath> {
    SOURCES
        .iter()
        .flat_map(|source| source.watch)
        .map(|watch| SkillUseWatchPath {
            path: home.join(watch.dir),
            recursive: watch.recursive,
        })
        .collect()
}

/// True when a change at `path` can change skill uses: `path` is under a
/// recursive watch dir, or directly inside a non-recursive one, and the
/// path relative to that dir passes the watch's `accepts`.
pub fn is_skill_use_change(home: &Path, path: &Path) -> bool {
    SOURCES.iter().flat_map(|source| source.watch).any(|watch| {
        let dir = home.join(watch.dir);
        let Ok(rel) = path.strip_prefix(&dir) else {
            return false;
        };
        if !watch.recursive && rel.components().count() != 1 {
            return false;
        }
        (watch.accepts)(rel)
    })
}

/// Index of skill uses parsed from local harness session history, cached per
/// file or database so unchanged sources are never re-read. `file_budget`/
/// `run_budget` are not persisted (see `with_budgets` for the test-only
/// override).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInvocationIndex {
    files: BTreeMap<PathBuf, IndexedTranscript>,
    #[serde(default)]
    databases: BTreeMap<PathBuf, IndexedDatabase>,
    #[serde(skip, default = "default_file_budget")]
    file_budget: u64,
    #[serde(skip, default = "default_run_budget")]
    run_budget: u64,
}

impl Default for SkillInvocationIndex {
    fn default() -> Self {
        Self {
            files: BTreeMap::new(),
            databases: BTreeMap::new(),
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
            databases: BTreeMap::new(),
            file_budget: file_bytes,
            run_budget: run_bytes,
        }
    }

    /// Test-only: forces the next refresh to treat `path` as changed
    /// regardless of its actual stamp, sidestepping filesystem mtime
    /// granularity that a fast rewrite-and-refresh test can otherwise race.
    #[cfg(test)]
    fn clear_database_stamp(&mut self, path: &Path) {
        if let Some(entry) = self.databases.get_mut(path) {
            entry.stamp = DatabaseStamp::default();
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

    /// Re-reads every enabled source in `SOURCES` that changed since the
    /// last refresh - transcript files by size/mtime, databases by
    /// [`DatabaseStamp`] - and drops cached entries that no longer exist. A
    /// source whose harness is switched off in `sources` is skipped
    /// entirely: it is neither listed nor does it drop any of its previously
    /// cached entries. Never panics: unreadable dirs/files/databases are
    /// skipped.
    pub fn refresh(&mut self, home: &Path, sources: &DiscoverySources) -> SkillUseRefreshReport {
        let mut report = SkillUseRefreshReport::default();
        let mut run_budget = self.run_budget;

        for source in SOURCES {
            if !sources.is_enabled(source.harness) {
                continue;
            }
            match &source.reader {
                UseReader::Transcripts { list, parse } => {
                    self.refresh_transcripts(
                        home,
                        source,
                        *list,
                        *parse,
                        &mut run_budget,
                        &mut report,
                    );
                }
                UseReader::Databases { list, read } => {
                    self.refresh_databases(home, source, *list, *read, &mut report);
                }
            }
        }

        report
    }

    /// One [`UseReader::Transcripts`] source's share of `refresh`: lists its
    /// files, re-parses the ones that changed (resuming from their cached
    /// byte offset), and drops cached files under this source's root that
    /// are no longer listed and no longer exist.
    fn refresh_transcripts(
        &mut self,
        home: &Path,
        source: &UseSource,
        list: fn(&Path) -> SourceListing,
        parse: fn(&TranscriptFile, &str, &mut TranscriptContext) -> Vec<SkillInvocation>,
        run_budget: &mut u64,
        report: &mut SkillUseRefreshReport,
    ) {
        let owned_root = (source.root)(home);
        let listing = list(home);
        if listing.incomplete {
            report.incomplete = true;
        }
        let listed_dirs = listing.listed_dirs;
        let mut seen: BTreeSet<PathBuf> = BTreeSet::new();

        for path in listing.files {
            seen.insert(path.clone());

            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            let size = meta.len();
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);

            let (start_offset, mut uses, skip_to_newline, mut context) = match self.files.get(&path)
            {
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
                    (0, Vec::new(), false, TranscriptContext::default())
                }
                Some(existing) if size < existing.parsed_bytes => {
                    (0, Vec::new(), false, TranscriptContext::default())
                }
                Some(existing) => {
                    let current_tail = read_tail_sample(&path, existing.parsed_bytes);
                    if current_tail != existing.tail_sample {
                        // The bytes just before our resume point no
                        // longer match what we parsed last time: this
                        // wasn't a plain append, so the cached uses may
                        // be stale.
                        (0, Vec::new(), false, TranscriptContext::default())
                    } else {
                        (
                            existing.parsed_bytes,
                            existing.uses.clone(),
                            existing.skipping_line,
                            existing.context.clone(),
                        )
                    }
                }
                None => (0, Vec::new(), false, TranscriptContext::default()),
            };

            if *run_budget == 0 {
                report.incomplete = true;
                continue;
            }

            let file_budget = self.file_budget.min(*run_budget);
            let Some((text, consumed, skipping_line)) = read_transcript_from_offset(
                &path,
                start_offset,
                file_budget,
                run_budget,
                skip_to_newline,
            ) else {
                continue;
            };
            let parsed_bytes = start_offset + consumed;
            let modified_utc = meta
                .modified()
                .map(DateTime::<Utc>::from)
                .unwrap_or_else(|_| Utc::now());
            let file = TranscriptFile {
                home,
                path: &path,
                modified: modified_utc,
            };
            uses.extend(parse(&file, &text, &mut context));
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
                    context,
                },
            );
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
            if !path.starts_with(&owned_root) {
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
        report.files_dropped += before - self.files.len();
    }

    /// One [`UseReader::Databases`] source's share of `refresh`: lists its
    /// databases, re-queries the ones whose [`DatabaseStamp`] changed, and
    /// drops cached databases under this source's root that are no longer
    /// listed and no longer exist.
    fn refresh_databases(
        &mut self,
        home: &Path,
        source: &UseSource,
        list: fn(&Path) -> Vec<PathBuf>,
        read: fn(&Path, &mut IndexedDatabase) -> bool,
        report: &mut SkillUseRefreshReport,
    ) {
        let owned_root = (source.root)(home);
        let mut seen: BTreeSet<PathBuf> = BTreeSet::new();

        for path in list(home) {
            seen.insert(path.clone());
            let stamp = database_stamp(&path);
            if self
                .databases
                .get(&path)
                .is_some_and(|existing| existing.stamp == stamp)
            {
                continue;
            }

            let mut entry = self.databases.get(&path).cloned().unwrap_or_default();
            if read(&path, &mut entry) {
                entry.stamp = stamp;
                self.databases.insert(path, entry);
                report.databases_read += 1;
            }
            // A failed read leaves the old cached stamp in place, so the
            // next refresh retries it on its own - `report.incomplete`
            // wouldn't add anything here, and a persistently unreadable
            // database would otherwise wedge it on forever.
        }

        let before = self.databases.len();
        self.databases.retain(|path, _| {
            if seen.contains(path) {
                return true;
            }
            if !path.starts_with(&owned_root) {
                return true;
            }
            !matches!(
                fs::symlink_metadata(path),
                Err(e) if e.kind() == io::ErrorKind::NotFound
            )
        });
        report.files_dropped += before - self.databases.len();
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

    /// File uses plus database uses. A database row id (`"<table>:<id>"`)
    /// that appears in more than one database (`opencode-next.db` rows get
    /// copied into `opencode.db`) counts once: databases are visited in
    /// path order and the first one to have a row wins.
    fn all_uses(&self) -> impl Iterator<Item = &SkillInvocation> {
        let mut seen_row_ids: BTreeSet<&str> = BTreeSet::new();
        let database_uses: Vec<&SkillInvocation> = self
            .databases
            .values()
            .flat_map(|database| database.rows.iter())
            .filter(move |(row_id, _)| seen_row_ids.insert(row_id.as_str()))
            .flat_map(|(_, uses)| uses.iter())
            .collect();
        self.files
            .values()
            .flat_map(|t| t.uses.iter())
            .chain(database_uses)
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
    fn missing_projects_dir_keeps_cached_files_and_is_not_incomplete() {
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

        // Point at a home whose `.claude/projects` doesn't exist: a user
        // without Claude Code must not be stuck `incomplete` forever (that
        // drives the desktop's 5s rebuild loop).
        let missing_home = tmp.path().join("missing-home");
        fs::create_dir_all(&missing_home).unwrap();
        let report = index.refresh(&missing_home, &sources);
        assert!(!report.incomplete);
        assert_eq!(report.files_dropped, 0);
        assert_eq!(stats(&index, &known_skills, &sources).len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn permission_denied_projects_dir_reports_incomplete() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let projects_dir = home.join(CLAUDE_PROJECTS_ROOT);
        fs::create_dir_all(&projects_dir).unwrap();
        fs::set_permissions(&projects_dir, fs::Permissions::from_mode(0o000)).unwrap();

        let mut index = SkillInvocationIndex::default();
        let sources = DiscoverySources::default();
        let report = index.refresh(home, &sources);

        // Restore permissions so the tempdir can be cleaned up regardless of
        // the assertion outcome.
        fs::set_permissions(&projects_dir, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(report.incomplete);
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

    #[test]
    fn skill_use_watch_paths_covers_claude_projects_and_opencode_data() {
        let home = PathBuf::from("/home/tester");
        let paths = skill_use_watch_paths(&home);
        assert!(paths.contains(&SkillUseWatchPath {
            path: home.join(CLAUDE_PROJECTS_ROOT),
            recursive: true,
        }));
        assert!(paths.contains(&SkillUseWatchPath {
            path: home.join(OPENCODE_DATA_ROOT),
            recursive: false,
        }));
        assert!(paths.contains(&SkillUseWatchPath {
            path: home.join(CODEX_SESSIONS_DIR),
            recursive: true,
        }));
        assert!(paths.contains(&SkillUseWatchPath {
            path: home.join(CODEX_ARCHIVED_SESSIONS_DIR),
            recursive: true,
        }));
        assert!(paths.contains(&SkillUseWatchPath {
            path: home.join(PI_SESSIONS_ROOT),
            recursive: true,
        }));
        assert!(paths.contains(&SkillUseWatchPath {
            path: home.join(CURSOR_PROJECTS_ROOT),
            recursive: true,
        }));
        assert!(paths.contains(&SkillUseWatchPath {
            path: home.join(GROK_SESSIONS_ROOT),
            recursive: true,
        }));
    }

    #[test]
    fn is_skill_use_change_matches_opencode_databases_and_claude_transcripts() {
        let home = PathBuf::from("/home/tester");
        assert!(is_skill_use_change(
            &home,
            &home.join(".local/share/opencode/opencode.db"),
        ));
        assert!(is_skill_use_change(
            &home,
            &home.join(".local/share/opencode/opencode-next.db-wal"),
        ));
        assert!(is_skill_use_change(
            &home,
            &home.join(CLAUDE_PROJECTS_ROOT).join("-p/abc.jsonl"),
        ));

        assert!(!is_skill_use_change(
            &home,
            &home.join(".local/share/opencode/opencode.db-shm"),
        ));
        assert!(!is_skill_use_change(
            &home,
            &home.join(".local/share/opencode/auth.json"),
        ));
        assert!(!is_skill_use_change(
            &home,
            &home.join(".local/share/opencode/storage/session/x.json"),
        ));
        assert!(!is_skill_use_change(
            &home,
            &home.join(".local/share/opencode/log/x.log"),
        ));
        assert!(!is_skill_use_change(
            &home,
            &home.join(".claude/settings.json")
        ));

        assert!(is_skill_use_change(
            &home,
            &home.join(".codex/sessions/2026/09/16/rollout-x.jsonl"),
        ));
        assert!(is_skill_use_change(
            &home,
            &home.join(".codex/archived_sessions/rollout-x.jsonl"),
        ));
        assert!(!is_skill_use_change(
            &home,
            &home.join(".codex/config.toml")
        ));
        assert!(!is_skill_use_change(
            &home,
            &home.join(".codex/log/codex-tui.log"),
        ));

        assert!(is_skill_use_change(
            &home,
            &home.join(".pi/agent/sessions/d/f.jsonl"),
        ));
        assert!(is_skill_use_change(
            &home,
            &home.join(".cursor/projects/p/agent-transcripts/s/s.jsonl"),
        ));
        assert!(!is_skill_use_change(
            &home,
            &home.join(".cursor/projects/p/terminals/1.txt"),
        ));
        assert!(!is_skill_use_change(
            &home,
            &home.join(".cursor/projects/p")
        ));
        assert!(!is_skill_use_change(
            &home,
            &home.join(".pi/agent/settings.json"),
        ));

        assert!(is_skill_use_change(
            &home,
            &home.join(".grok/sessions/p/s/updates.jsonl"),
        ));
        assert!(is_skill_use_change(
            &home,
            &home.join(".grok/sessions/p/s/summary.json"),
        ));
        assert!(!is_skill_use_change(
            &home,
            &home.join(".grok/sessions/p/s/images/a.png"),
        ));
        assert!(!is_skill_use_change(
            &home,
            &home.join(".grok/sessions/p/s")
        ));
        assert!(!is_skill_use_change(
            &home,
            &home.join(".grok/sessions/p/s/.cwd")
        ));
        assert!(!is_skill_use_change(
            &home,
            &home.join(".grok/settings.json")
        ));
    }

    mod codex_rollouts {
        use super::*;

        fn rollout_line(record_type: &str, timestamp: &str, payload: &str) -> String {
            format!(r#"{{"type":"{record_type}","timestamp":"{timestamp}","payload":{payload}}}"#)
        }

        fn session_meta_line(id: &str, cwd: &str) -> String {
            format!(r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"{cwd}"}}}}"#)
        }

        fn skill_block_line(timestamp: &str, name: &str) -> String {
            rollout_line(
                "response_item",
                timestamp,
                &format!(
                    r#"{{"type":"message","role":"user","content":[{{"type":"input_text","text":"<skill>\n<name>{name}</name>\n<path>/u/.codex/skills/{name}/SKILL.md</path>\n"}}]}}"#
                ),
            )
        }

        fn exec_cat_line(timestamp: &str, path: &str) -> String {
            rollout_line(
                "response_item",
                timestamp,
                &format!(
                    r#"{{"type":"custom_tool_call","name":"exec","input":"tools.exec_command({{\"cmd\": \"cat {path}\"}})"}}"#
                ),
            )
        }

        fn write_rollout(dir: &Path, name: &str, lines: &[String]) -> PathBuf {
            fs::create_dir_all(dir).unwrap();
            let path = dir.join(name);
            let mut content = lines.join("\n");
            content.push('\n');
            fs::write(&path, content).unwrap();
            path
        }

        fn known(skills: &[&str]) -> StdBTreeSet<String> {
            skills.iter().map(|s| s.to_string()).collect()
        }

        #[test]
        fn issue_acceptance_user_and_file_read_uses_are_counted() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            write_rollout(
                &home.join(CODEX_SESSIONS_DIR).join("2026/09/16"),
                "a.jsonl",
                &[
                    session_meta_line("sess-a", "/proj-a"),
                    skill_block_line("2026-09-16T12:00:00Z", "foo"),
                    exec_cat_line("2026-09-16T12:00:01Z", "/u/.codex/skills/foo/SKILL.md"),
                ],
            );
            write_rollout(
                &home.join(CODEX_SESSIONS_DIR).join("2026/09/16"),
                "b.jsonl",
                &[
                    session_meta_line("sess-b", "/proj-b"),
                    exec_cat_line("2026-09-16T12:00:00Z", "/u/.codex/skills/bar/SKILL.md"),
                    exec_cat_line("2026-09-16T12:00:01Z", "/x/foo/SKILL.md"),
                ],
            );

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo", "bar"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            let by_skill: BTreeMap<&str, &SkillInvocationStats> =
                stats.iter().map(|s| (s.skill.as_str(), s)).collect();
            assert_eq!(by_skill.len(), 2);
            assert_eq!(by_skill["foo"].total, 1);
            assert_eq!(by_skill["foo"].by_trigger_30_days.user, 1);
            assert_eq!(by_skill["bar"].total, 1);
            assert_eq!(by_skill["bar"].by_trigger_30_days.file_read, 1);
        }

        #[test]
        fn a_flat_archived_session_file_is_read() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            write_rollout(
                &home.join(CODEX_ARCHIVED_SESSIONS_DIR),
                "old.jsonl",
                &[
                    session_meta_line("sess-a", "/proj-a"),
                    skill_block_line("2026-09-16T12:00:00Z", "foo"),
                ],
            );

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            assert_eq!(stats.len(), 1);
            assert_eq!(stats[0].skill, "foo");
        }

        #[test]
        fn append_resumes_and_carries_context_from_the_first_refresh() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            let dir = home.join(CODEX_SESSIONS_DIR).join("2026/09/16");
            let path = write_rollout(&dir, "a.jsonl", &[session_meta_line("sess-a", "/proj-a")]);

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);

            let mut content = fs::read_to_string(&path).unwrap();
            content.push_str(&skill_block_line("2026-09-16T12:05:00Z", "foo"));
            content.push('\n');
            let appended_len = content.len() as u64 - fs::metadata(&path).unwrap().len();
            fs::write(&path, &content).unwrap();

            let report = index.refresh(home, &sources);
            assert_eq!(
                report.bytes_read, appended_len,
                "the file must be resumed, not reparsed from 0"
            );
            let stats = stats(&index, &known_skills, &sources);
            assert_eq!(stats.len(), 1);
            assert_eq!(stats[0].skill, "foo");
            assert_eq!(
                stats[0].by_project_30_days.get("/proj-a"),
                Some(&1),
                "expected the session/project from the first refresh's context"
            );
        }

        #[test]
        fn a_rewrite_with_a_different_session_meta_carries_the_new_session() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            let dir = home.join(CODEX_SESSIONS_DIR).join("2026/09/16");
            let path = write_rollout(
                &dir,
                "a.jsonl",
                &[
                    session_meta_line("sess-a", "/proj-a"),
                    skill_block_line("2026-09-16T12:00:00Z", "foo"),
                ],
            );

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            assert_eq!(
                stats(&index, &known_skills, &sources)[0]
                    .by_project_30_days
                    .get("/proj-a"),
                Some(&1)
            );

            let mut content = session_meta_line("sess-b", "/proj-b");
            content.push('\n');
            content.push_str(&skill_block_line("2026-09-16T12:00:00Z", "foo"));
            content.push('\n');
            fs::write(&path, content).unwrap();

            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            assert_eq!(stats.len(), 1);
            assert_eq!(stats[0].by_project_30_days.get("/proj-b"), Some(&1));
        }

        #[test]
        fn moving_a_file_from_sessions_to_archived_counts_its_use_once() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            let dir = home.join(CODEX_SESSIONS_DIR).join("2026/09/16");
            let path = write_rollout(
                &dir,
                "a.jsonl",
                &[
                    session_meta_line("sess-a", "/proj-a"),
                    skill_block_line("2026-09-16T12:00:00Z", "foo"),
                ],
            );

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            assert_eq!(stats(&index, &known_skills, &sources)[0].total, 1);

            let archived_dir = home.join(CODEX_ARCHIVED_SESSIONS_DIR);
            fs::create_dir_all(&archived_dir).unwrap();
            fs::rename(&path, archived_dir.join("a.jsonl")).unwrap();

            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            assert_eq!(stats.len(), 1);
            assert_eq!(stats[0].total, 1, "the use must not be doubled");
        }

        #[test]
        fn switching_codex_off_stops_reads_and_on_resumes_counting() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            write_rollout(
                &home.join(CODEX_SESSIONS_DIR).join("2026/09/16"),
                "a.jsonl",
                &[
                    session_meta_line("sess-a", "/proj-a"),
                    skill_block_line("2026-09-16T12:00:00Z", "foo"),
                ],
            );

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo"]);
            let mut off = DiscoverySources::default();
            off.set(AgentId::CODEX, false);
            index.refresh(home, &off);
            assert!(stats(&index, &known_skills, &off).is_empty());

            let enabled = DiscoverySources::default();
            index.refresh(home, &enabled);
            assert_eq!(stats(&index, &known_skills, &enabled).len(), 1);
        }

        /// codex_rollout_reader_resumes_from_a_byte_offset_across_archived_sessions_or_names_the_missed_use:
        /// an archived rollout gets the same resume treatment as a live one -
        /// a second refresh after new lines are appended reads only the new
        /// bytes and counts only the new use, not the whole file again.
        #[test]
        fn codex_rollout_reader_resumes_from_a_byte_offset_across_archived_sessions_or_names_the_missed_use(
        ) {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            let path = write_rollout(
                &home.join(CODEX_ARCHIVED_SESSIONS_DIR),
                "old.jsonl",
                &[
                    session_meta_line("sess-a", "/proj-a"),
                    skill_block_line("2026-09-16T12:00:00Z", "foo"),
                ],
            );

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo"]);
            let sources = DiscoverySources::default();
            let first = index.refresh(home, &sources);
            assert_eq!(stats(&index, &known_skills, &sources)[0].total, 1);

            let mut content = fs::read_to_string(&path).unwrap();
            content.push_str(&skill_block_line("2026-09-16T12:05:00Z", "foo"));
            content.push('\n');
            let appended_len = content.len() as u64 - fs::metadata(&path).unwrap().len();
            fs::write(&path, &content).unwrap();

            let second = index.refresh(home, &sources);
            assert_eq!(
                second.bytes_read, appended_len,
                "the missed use: a full reparse (or no read at all) instead of a byte-offset resume"
            );
            assert_ne!(
                first.bytes_read, 0,
                "sanity: the first pass must have read something"
            );
            let stats = stats(&index, &known_skills, &sources);
            assert_eq!(stats.len(), 1);
            assert_eq!(
                stats[0].total, 2,
                "the missed use: the appended skill use was not counted"
            );
        }

        #[test]
        fn no_codex_and_no_claude_projects_is_not_incomplete() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            let mut index = SkillInvocationIndex::default();
            let sources = DiscoverySources::default();
            let report = index.refresh(home, &sources);
            assert!(!report.incomplete);
        }

        #[test]
        fn a_cache_written_before_context_still_loads() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            write_rollout(
                &home.join(CODEX_SESSIONS_DIR).join("2026/09/16"),
                "a.jsonl",
                &[
                    session_meta_line("sess-a", "/proj-a"),
                    skill_block_line("2026-09-16T12:00:00Z", "foo"),
                ],
            );
            let mut index = SkillInvocationIndex::default();
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let cache_path = tmp.path().join("cache/skill-uses.json");
            index.save(&cache_path).unwrap();

            // Simulate a cache written before this change: drop `context`
            // from every cached file entry.
            let raw = fs::read_to_string(&cache_path).unwrap();
            let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
            if let Some(files) = value.get_mut("files").and_then(|v| v.as_object_mut()) {
                for entry in files.values_mut() {
                    if let Some(obj) = entry.as_object_mut() {
                        obj.remove("context");
                    }
                }
            }
            fs::write(&cache_path, serde_json::to_string(&value).unwrap()).unwrap();

            let loaded = SkillInvocationIndex::load_or_empty(&cache_path);
            let known_skills = known(&["foo"]);
            assert_eq!(stats(&loaded, &known_skills, &sources).len(), 1);
        }
    }

    mod pi_sessions {
        use super::*;

        fn header_line(id: &str, cwd: &str) -> String {
            format!(
                r#"{{"type":"session","id":"{id}","cwd":"{cwd}","timestamp":"2026-09-16T11:00:00Z","version":"1.0.0"}}"#
            )
        }

        fn user_skill_line(timestamp: &str, name: &str, location: &str) -> String {
            format!(
                r#"{{"type":"message","id":"m1","parentId":null,"timestamp":"{timestamp}","message":{{"role":"user","content":[{{"type":"text","text":"<skill name=\"{name}\" location=\"{location}\">"}}]}}}}"#
            )
        }

        fn read_tool_call_line(timestamp: &str, path: &str) -> String {
            format!(
                r#"{{"type":"message","id":"m2","parentId":"m1","timestamp":"{timestamp}","message":{{"role":"assistant","content":[{{"type":"toolCall","name":"read","arguments":{{"path":"{path}"}}}}]}}}}"#
            )
        }

        fn write_session(dir: &Path, name: &str, lines: &[String]) -> PathBuf {
            fs::create_dir_all(dir).unwrap();
            let path = dir.join(name);
            let mut content = lines.join("\n");
            content.push('\n');
            fs::write(&path, content).unwrap();
            path
        }

        fn known(skills: &[&str]) -> StdBTreeSet<String> {
            skills.iter().map(|s| s.to_string()).collect()
        }

        #[test]
        fn issue_acceptance_user_and_file_read_uses_are_counted() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            write_session(
                &home.join(PI_SESSIONS_ROOT).join("d"),
                "a.jsonl",
                &[
                    header_line("sess-a", "/proj-a"),
                    user_skill_line("2026-09-16T12:00:00Z", "foo", "/x/skills/foo/SKILL.md"),
                    read_tool_call_line("2026-09-16T12:00:01Z", "/x/skills/bar/SKILL.md"),
                    read_tool_call_line("2026-09-16T12:00:02Z", "/x/baz/SKILL.md"),
                ],
            );

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo", "bar", "baz"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            let by_skill: BTreeMap<&str, &SkillInvocationStats> =
                stats.iter().map(|s| (s.skill.as_str(), s)).collect();
            assert_eq!(by_skill.len(), 2, "baz's read isn't under a skills root");
            assert_eq!(by_skill["foo"].total, 1);
            assert_eq!(by_skill["foo"].by_trigger_30_days.user, 1);
            assert_eq!(by_skill["foo"].by_project_30_days.get("/proj-a"), Some(&1));
            assert_eq!(by_skill["bar"].total, 1);
            assert_eq!(by_skill["bar"].by_trigger_30_days.file_read, 1);
            assert_eq!(by_skill["bar"].by_project_30_days.get("/proj-a"), Some(&1));
        }

        #[test]
        fn switching_pi_off_stops_reads_and_on_resumes_counting() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            write_session(
                &home.join(PI_SESSIONS_ROOT).join("d"),
                "a.jsonl",
                &[
                    header_line("sess-a", "/proj-a"),
                    user_skill_line("2026-09-16T12:00:00Z", "foo", "/x/skills/foo/SKILL.md"),
                ],
            );

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo"]);
            let mut off = DiscoverySources::default();
            off.set(AgentId::PI, false);
            index.refresh(home, &off);
            assert!(stats(&index, &known_skills, &off).is_empty());

            let enabled = DiscoverySources::default();
            index.refresh(home, &enabled);
            assert_eq!(stats(&index, &known_skills, &enabled).len(), 1);
        }
    }

    mod cursor_transcripts {
        use super::*;

        /// Matches `discovery.rs`'s `CURSOR_WORKSPACE_STORAGE_ROOTS[0]`.
        const CURSOR_WORKSPACE_STORAGE_ROOT: &str =
            "Library/Application Support/Cursor/User/workspaceStorage";

        fn write_cursor_workspace(home: &Path, hash: &str, folder: &Path) {
            let dir = home.join(CURSOR_WORKSPACE_STORAGE_ROOT).join(hash);
            fs::create_dir_all(&dir).unwrap();
            let uri = url::Url::from_file_path(folder).unwrap();
            fs::write(
                dir.join("workspace.json"),
                serde_json::json!({ "folder": uri.as_str() }).to_string(),
            )
            .unwrap();
        }

        fn read_line(path: &str) -> String {
            format!(
                r#"{{"role":"assistant","message":{{"content":[{{"type":"tool_use","name":"Read","input":{{"path":"{path}"}}}}]}}}}"#
            )
        }

        fn write_transcript(dir: &Path, name: &str, lines: &[String]) -> PathBuf {
            fs::create_dir_all(dir).unwrap();
            let path = dir.join(name);
            let mut content = lines.join("\n");
            content.push('\n');
            fs::write(&path, content).unwrap();
            path
        }

        fn known(skills: &[&str]) -> StdBTreeSet<String> {
            skills.iter().map(|s| s.to_string()).collect()
        }

        #[test]
        fn issue_acceptance_a_skill_read_gives_one_file_read_with_session_and_project() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            let project_folder = home.join("proj");
            write_cursor_workspace(home, "hash1", &project_folder);
            let encoded = cursor_project_dir_name(&project_folder);

            write_transcript(
                &home
                    .join(CURSOR_PROJECTS_ROOT)
                    .join(&encoded)
                    .join("agent-transcripts")
                    .join("s1"),
                "s1.jsonl",
                &[
                    read_line("/x/skills/foo/SKILL.md"),
                    read_line("/x/notes/SKILL.md"),
                ],
            );

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            assert_eq!(stats.len(), 1, "the read outside a root must not count");
            assert_eq!(stats[0].skill, "foo");
            assert_eq!(stats[0].total, 1);
            assert_eq!(
                stats[0]
                    .by_project_30_days
                    .get(&project_folder.to_string_lossy().into_owned()),
                Some(&1)
            );
        }

        #[test]
        fn a_subagent_file_shares_its_parents_session_and_dedupes_with_it() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            let project_folder = home.join("proj");
            write_cursor_workspace(home, "hash1", &project_folder);
            let encoded = cursor_project_dir_name(&project_folder);
            let session_dir = home
                .join(CURSOR_PROJECTS_ROOT)
                .join(&encoded)
                .join("agent-transcripts")
                .join("s1");

            write_transcript(
                &session_dir,
                "s1.jsonl",
                &[read_line("/x/skills/foo/SKILL.md")],
            );
            write_transcript(
                &session_dir.join("subagents"),
                "sub1.jsonl",
                &[read_line("/x/skills/foo/SKILL.md")],
            );

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            assert_eq!(stats.len(), 1);
            assert_eq!(
                stats[0].total, 1,
                "the same session's file-read dedupe must collapse the subagent copy"
            );
        }

        #[test]
        fn no_matching_workspace_folder_still_counts_the_use_without_a_project_path() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            write_transcript(
                &home
                    .join(CURSOR_PROJECTS_ROOT)
                    .join("unknown-project")
                    .join("agent-transcripts")
                    .join("s1"),
                "s1.jsonl",
                &[read_line("/x/skills/foo/SKILL.md")],
            );

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            assert_eq!(stats.len(), 1);
            assert_eq!(stats[0].total, 1);
            assert!(stats[0].by_project_30_days.is_empty());
        }

        #[test]
        fn switching_cursor_off_stops_reads_and_on_resumes_counting() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            write_transcript(
                &home
                    .join(CURSOR_PROJECTS_ROOT)
                    .join("unknown-project")
                    .join("agent-transcripts")
                    .join("s1"),
                "s1.jsonl",
                &[read_line("/x/skills/foo/SKILL.md")],
            );

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo"]);
            let mut off = DiscoverySources::default();
            off.set(AgentId::CURSOR, false);
            index.refresh(home, &off);
            assert!(stats(&index, &known_skills, &off).is_empty());

            let enabled = DiscoverySources::default();
            index.refresh(home, &enabled);
            assert_eq!(stats(&index, &known_skills, &enabled).len(), 1);
        }

        #[test]
        fn cursor_project_dir_name_replaces_non_alphanumerics_and_strips_the_leading_slash() {
            assert_eq!(
                cursor_project_dir_name(Path::new("/Users/a/src/agent-studio")),
                "Users-a-src-agent-studio"
            );
            assert_eq!(
                cursor_project_dir_name(Path::new("/Users/a/my.app/x_y")),
                "Users-a-my-app-x-y"
            );
        }
    }

    mod grok_sessions {
        use super::*;

        /// Matches `discovery.rs`'s `grok_dirname` test helper: Grok's own
        /// `urlencoding::encode` leaves only the RFC 3986 unreserved
        /// characters as they are.
        const GROK_ENCODED: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
            .remove(b'-')
            .remove(b'_')
            .remove(b'.')
            .remove(b'~');

        fn grok_dirname(cwd: &Path) -> String {
            percent_encoding::utf8_percent_encode(cwd.to_str().unwrap(), GROK_ENCODED).to_string()
        }

        /// A timestamp a few minutes ago plus `offset_secs`, so tests that
        /// assert on `total`/`by_trigger_30_days` land inside the rolling
        /// windows regardless of when the test runs.
        fn recent_secs(offset_secs: i64) -> i64 {
            (Utc::now() - chrono::Duration::minutes(5)).timestamp() + offset_secs
        }

        fn grok_line(timestamp: i64, update: serde_json::Value) -> String {
            serde_json::json!({
                "timestamp": timestamp,
                "method": "session/update",
                "params": {"update": update},
            })
            .to_string()
        }

        fn read_update(tool_call_id: &str, path: &str) -> serde_json::Value {
            serde_json::json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": tool_call_id,
                "_meta": {"x.ai/tool": {"kind": "read", "input": {"path": path}}},
            })
        }

        fn skill_update(tool_call_id: &str, name: &str) -> serde_json::Value {
            serde_json::json!({
                "sessionUpdate": "tool_call",
                "toolCallId": tool_call_id,
                "title": format!("Skill: {name}"),
                "kind": "other",
            })
        }

        fn user_chunk_update(text: &str) -> serde_json::Value {
            serde_json::json!({
                "sessionUpdate": "user_message_chunk",
                "content": {"type": "text", "text": text},
            })
        }

        fn write_grok_updates(session_dir: &Path, lines: &[String]) -> PathBuf {
            fs::create_dir_all(session_dir).unwrap();
            let path = session_dir.join("updates.jsonl");
            let mut content = lines.join("\n");
            content.push('\n');
            fs::write(&path, content).unwrap();
            path
        }

        fn write_grok_summary(session_dir: &Path, forked_at: Option<&DateTime<Utc>>) {
            fs::create_dir_all(session_dir).unwrap();
            let mut value = serde_json::json!({});
            if let Some(forked_at) = forked_at {
                value["forked_at"] = serde_json::json!(forked_at.to_rfc3339());
            }
            fs::write(session_dir.join("summary.json"), value.to_string()).unwrap();
        }

        fn known(skills: &[&str]) -> StdBTreeSet<String> {
            skills.iter().map(|s| s.to_string()).collect()
        }

        #[test]
        fn issue_acceptance_file_read_agent_and_user_uses_carry_session_and_project() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            let project = home.join("proj");
            let encoded = grok_dirname(&project);
            let session_dir = home.join(GROK_SESSIONS_ROOT).join(&encoded).join("s1");
            write_grok_updates(
                &session_dir,
                &[
                    grok_line(recent_secs(0), read_update("tc1", "/x/skills/foo/SKILL.md")),
                    grok_line(recent_secs(1), skill_update("tc2", "bar")),
                    grok_line(recent_secs(2), user_chunk_update("/baz go")),
                ],
            );
            write_grok_summary(&session_dir, None);

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo", "bar", "baz"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            let by_skill: BTreeMap<&str, &SkillInvocationStats> =
                stats.iter().map(|s| (s.skill.as_str(), s)).collect();
            assert_eq!(by_skill["foo"].total, 1);
            assert_eq!(by_skill["foo"].by_trigger_30_days.file_read, 1);
            assert_eq!(by_skill["bar"].total, 1);
            assert_eq!(by_skill["bar"].by_trigger_30_days.agent, 1);
            assert_eq!(by_skill["baz"].total, 1);
            assert_eq!(by_skill["baz"].by_trigger_30_days.user, 1);
            assert_eq!(
                by_skill["foo"]
                    .by_project_30_days
                    .get(&project.to_string_lossy().into_owned()),
                Some(&1),
                "expected the decoded cwd as the project path"
            );
        }

        #[test]
        fn fork_skips_copied_lines_and_counts_each_sessions_new_uses() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            let project = home.join("proj");
            let encoded = grok_dirname(&project);
            let forked_at = Utc::now() - chrono::Duration::minutes(1);
            let before = forked_at.timestamp() - 5;
            let after = forked_at.timestamp() + 5;

            let s1 = home.join(GROK_SESSIONS_ROOT).join(&encoded).join("s1");
            write_grok_updates(
                &s1,
                &[
                    grok_line(before, read_update("tc1", "/x/skills/foo/SKILL.md")),
                    grok_line(before, skill_update("tc2", "bar")),
                    grok_line(before, user_chunk_update("/baz go")),
                ],
            );
            write_grok_summary(&s1, None);

            let s2 = home.join(GROK_SESSIONS_ROOT).join(&encoded).join("s2");
            write_grok_updates(
                &s2,
                &[
                    grok_line(before, read_update("tc1", "/x/skills/foo/SKILL.md")),
                    grok_line(before, skill_update("tc2", "bar")),
                    grok_line(before, user_chunk_update("/baz go")),
                    grok_line(after, skill_update("tc3", "bar")),
                ],
            );
            write_grok_summary(&s2, Some(&forked_at));

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo", "bar", "baz"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            let by_skill: BTreeMap<&str, &SkillInvocationStats> =
                stats.iter().map(|s| (s.skill.as_str(), s)).collect();
            assert_eq!(
                by_skill["bar"].total, 2,
                "one from s1, one from s2's new line"
            );
            assert_eq!(by_skill["foo"].total, 1);
            assert_eq!(by_skill["baz"].total, 1);
        }

        #[test]
        fn a_session_without_summary_json_is_not_read_until_it_is_written() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            let project = home.join("proj");
            let encoded = grok_dirname(&project);
            let session_dir = home.join(GROK_SESSIONS_ROOT).join(&encoded).join("s1");
            write_grok_updates(
                &session_dir,
                &[grok_line(recent_secs(0), skill_update("tc1", "bar"))],
            );

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["bar"]);
            let sources = DiscoverySources::default();
            let report = index.refresh(home, &sources);
            assert!(!report.incomplete);
            assert!(stats(&index, &known_skills, &sources).is_empty());

            write_grok_summary(&session_dir, None);
            index.refresh(home, &sources);
            assert_eq!(stats(&index, &known_skills, &sources).len(), 1);
        }

        #[test]
        fn switching_grok_build_off_stops_reads_and_on_resumes_counting() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            let project = home.join("proj");
            let encoded = grok_dirname(&project);
            let session_dir = home.join(GROK_SESSIONS_ROOT).join(&encoded).join("s1");
            write_grok_updates(
                &session_dir,
                &[grok_line(recent_secs(0), skill_update("tc1", "bar"))],
            );
            write_grok_summary(&session_dir, None);

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["bar"]);
            let mut off = DiscoverySources::default();
            off.set(AgentId::GROK_BUILD, false);
            index.refresh(home, &off);
            assert!(stats(&index, &known_skills, &off).is_empty());

            let enabled = DiscoverySources::default();
            index.refresh(home, &enabled);
            assert_eq!(stats(&index, &known_skills, &enabled).len(), 1);
        }

        #[test]
        fn no_grok_home_is_not_incomplete() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            let mut index = SkillInvocationIndex::default();
            let sources = DiscoverySources::default();
            let report = index.refresh(home, &sources);
            assert!(!report.incomplete);
        }

        #[test]
        fn resuming_a_file_keeps_counted_calls_and_ignores_a_repeated_id() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            let project = home.join("proj");
            let encoded = grok_dirname(&project);
            let session_dir = home.join(GROK_SESSIONS_ROOT).join(&encoded).join("s1");
            let path = write_grok_updates(
                &session_dir,
                &[grok_line(recent_secs(0), skill_update("tc1", "bar"))],
            );
            write_grok_summary(&session_dir, None);

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["bar"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            assert_eq!(stats(&index, &known_skills, &sources)[0].total, 1);

            let mut content = fs::read_to_string(&path).unwrap();
            content.push_str(&grok_line(recent_secs(1), skill_update("tc1", "bar")));
            content.push('\n');
            fs::write(&path, &content).unwrap();

            index.refresh(home, &sources);
            assert_eq!(stats(&index, &known_skills, &sources)[0].total, 1);
        }

        #[test]
        fn deleting_summary_json_drops_the_sessions_cached_uses() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            let project = home.join("proj");
            let encoded = grok_dirname(&project);
            let session_dir = home.join(GROK_SESSIONS_ROOT).join(&encoded).join("s1");
            write_grok_updates(
                &session_dir,
                &[grok_line(recent_secs(0), skill_update("tc1", "bar"))],
            );
            write_grok_summary(&session_dir, None);

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["bar"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            assert_eq!(stats(&index, &known_skills, &sources).len(), 1);

            fs::remove_file(session_dir.join("summary.json")).unwrap();
            let report = index.refresh(home, &sources);
            assert_eq!(report.files_dropped, 1);
            assert!(stats(&index, &known_skills, &sources).is_empty());
        }
    }

    #[test]
    fn no_pi_and_no_cursor_is_not_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let mut index = SkillInvocationIndex::default();
        let sources = DiscoverySources::default();
        let report = index.refresh(home, &sources);
        assert!(!report.incomplete);
    }

    mod opencode_databases {
        use super::*;
        use rusqlite::{params, Connection};

        fn opencode_db_path(home: &Path, name: &str) -> PathBuf {
            opencode_root(home).join(name)
        }

        /// A temp `home` with the OpenCode data dir created and
        /// `opencode.db`'s path (not yet an actual database file) under it.
        fn temp_opencode_db_home() -> (tempfile::TempDir, PathBuf) {
            let tmp = tempfile::tempdir().unwrap();
            let db_path = opencode_db_path(tmp.path(), "opencode.db");
            fs::create_dir_all(db_path.parent().unwrap()).unwrap();
            (tmp, db_path)
        }

        /// A timestamp within the 30-day rolling window, `offset_ms` after a
        /// fixed point a few minutes ago - tests that assert on
        /// `by_trigger_30_days`/`by_project_30_days` need a real recent
        /// timestamp, not an arbitrary epoch-adjacent one.
        fn recent_ms(offset_ms: i64) -> i64 {
            (Utc::now() - chrono::Duration::minutes(5)).timestamp_millis() + offset_ms
        }

        fn create_message_table(conn: &Connection) {
            conn.execute_batch(
                "CREATE TABLE session_message (
                    id TEXT PRIMARY KEY,
                    session_id TEXT,
                    type TEXT,
                    time_created INTEGER,
                    time_updated INTEGER,
                    data TEXT
                )",
            )
            .unwrap();
        }

        fn create_part_table(conn: &Connection) {
            conn.execute_batch(
                "CREATE TABLE part (
                    id TEXT PRIMARY KEY,
                    session_id TEXT,
                    time_created INTEGER,
                    time_updated INTEGER,
                    data TEXT
                )",
            )
            .unwrap();
        }

        fn create_session_table(conn: &Connection) {
            conn.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT)")
                .unwrap();
        }

        fn create_session_v2_table(conn: &Connection) {
            conn.execute_batch("CREATE TABLE session_v2 (id TEXT PRIMARY KEY, directory TEXT)")
                .unwrap();
        }

        fn insert_session(conn: &Connection, id: &str, directory: &str) {
            conn.execute(
                "INSERT INTO session (id, directory) VALUES (?1, ?2)",
                params![id, directory],
            )
            .unwrap();
        }

        fn insert_session_v2(conn: &Connection, id: &str, directory: &str) {
            conn.execute(
                "INSERT INTO session_v2 (id, directory) VALUES (?1, ?2)",
                params![id, directory],
            )
            .unwrap();
        }

        fn insert_message(
            conn: &Connection,
            id: &str,
            session_id: &str,
            kind: &str,
            time_created: i64,
            time_updated: i64,
            data: &str,
        ) {
            conn.execute(
                "INSERT INTO session_message (id, session_id, type, time_created, time_updated, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, session_id, kind, time_created, time_updated, data],
            )
            .unwrap();
        }

        fn insert_part(
            conn: &Connection,
            id: &str,
            session_id: &str,
            time_created: i64,
            time_updated: i64,
            data: &str,
        ) {
            conn.execute(
                "INSERT INTO part (id, session_id, time_created, time_updated, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, session_id, time_created, time_updated, data],
            )
            .unwrap();
        }

        fn skill_tool_message(id: &str, status: &str, skill: &str) -> String {
            format!(
                r#"{{"content":[{{"type":"tool","id":"{id}","name":"skill","state":{{"status":"{status}","input":{{"id":"{skill}"}}}}}}]}}"#
            )
        }

        fn read_tool_message(id: &str, path: &str) -> String {
            format!(
                r#"{{"content":[{{"type":"tool","id":"{id}","name":"read","state":{{"status":"completed","input":{{"path":"{path}"}}}}}}]}}"#
            )
        }

        #[test]
        fn skill_row_gives_one_user_use_with_project_from_session_v2() {
            let (tmp, db_path) = temp_opencode_db_home();
            let home = tmp.path();
            {
                let conn = Connection::open(&db_path).unwrap();
                create_message_table(&conn);
                create_session_table(&conn);
                create_session_v2_table(&conn);
                insert_session(&conn, "s1", "/legacy-proj");
                insert_session_v2(&conn, "s1", "/proj-v2");
                insert_message(
                    &conn,
                    "m1",
                    "s1",
                    "skill",
                    recent_ms(0),
                    recent_ms(0),
                    r#"{"skill":"deploy","name":"deploy"}"#,
                );
            }

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["deploy"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            assert_eq!(stats.len(), 1);
            assert_eq!(stats[0].skill, "deploy");
            assert_eq!(stats[0].by_trigger_30_days.user, 1);
            assert_eq!(
                stats[0].by_project_30_days.get("/proj-v2"),
                Some(&1),
                "expected the session_v2 directory, not the session one"
            );
        }

        #[test]
        fn skill_row_without_session_v2_uses_session_directory() {
            let (tmp, db_path) = temp_opencode_db_home();
            let home = tmp.path();
            {
                let conn = Connection::open(&db_path).unwrap();
                create_message_table(&conn);
                create_session_table(&conn);
                insert_session(&conn, "s1", "/legacy-proj");
                insert_message(
                    &conn,
                    "m1",
                    "s1",
                    "skill",
                    recent_ms(0),
                    recent_ms(0),
                    r#"{"skill":"deploy"}"#,
                );
            }

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["deploy"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            assert_eq!(stats.len(), 1);
            assert_eq!(stats[0].by_project_30_days.get("/legacy-proj"), Some(&1));
        }

        #[test]
        fn skill_tool_completed_gives_agent_use_and_error_gives_nothing() {
            let (tmp, db_path) = temp_opencode_db_home();
            let home = tmp.path();
            {
                let conn = Connection::open(&db_path).unwrap();
                create_message_table(&conn);
                insert_message(
                    &conn,
                    "m1",
                    "s1",
                    "assistant",
                    recent_ms(0),
                    recent_ms(0),
                    &skill_tool_message("c1", "completed", "deploy"),
                );
                insert_message(
                    &conn,
                    "m2",
                    "s1",
                    "assistant",
                    recent_ms(1),
                    recent_ms(1),
                    &skill_tool_message("c2", "error", "deploy"),
                );
            }

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["deploy"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            assert_eq!(stats.len(), 1);
            assert_eq!(stats[0].total, 1);
            assert_eq!(stats[0].by_trigger_30_days.agent, 1);
        }

        #[test]
        fn read_tool_on_known_skill_md_path_gives_file_read_others_give_nothing() {
            let (tmp, db_path) = temp_opencode_db_home();
            let home = tmp.path();
            {
                let conn = Connection::open(&db_path).unwrap();
                create_message_table(&conn);
                insert_message(
                    &conn,
                    "m1",
                    "s1",
                    "assistant",
                    recent_ms(0),
                    recent_ms(0),
                    &read_tool_message("c1", "/x/.claude/skills/foo/SKILL.md"),
                );
                insert_message(
                    &conn,
                    "m2",
                    "s1",
                    "assistant",
                    recent_ms(1),
                    recent_ms(1),
                    &read_tool_message("c2", "/x/notes.md"),
                );
                insert_message(
                    &conn,
                    "m3",
                    "s1",
                    "assistant",
                    recent_ms(2),
                    recent_ms(2),
                    r#"{"content":[{"type":"tool","id":"c3","name":"shell","state":{"status":"completed","input":{"command":"cat /x/.claude/skills/foo/SKILL.md"}}}]}"#,
                );
            }

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            assert_eq!(stats.len(), 1);
            assert_eq!(stats[0].skill, "foo");
            assert_eq!(stats[0].total, 1);
            assert_eq!(stats[0].by_trigger_30_days.file_read, 1);
        }

        #[test]
        fn part_read_row_with_file_path_uses_session_directory() {
            let (tmp, db_path) = temp_opencode_db_home();
            let home = tmp.path();
            {
                let conn = Connection::open(&db_path).unwrap();
                create_part_table(&conn);
                create_session_table(&conn);
                insert_session(&conn, "s1", "/proj");
                insert_part(
                    &conn,
                    "p1",
                    "s1",
                    recent_ms(0),
                    recent_ms(0),
                    r#"{"type":"tool","tool":"read","callID":"c1","state":{"status":"completed","input":{"filePath":"/x/.claude/skills/foo/SKILL.md"}}}"#,
                );
            }

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["foo"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            assert_eq!(stats.len(), 1);
            assert_eq!(stats[0].skill, "foo");
            assert_eq!(stats[0].by_project_30_days.get("/proj"), Some(&1));
        }

        #[test]
        fn second_refresh_with_no_change_reads_no_databases() {
            let (tmp, db_path) = temp_opencode_db_home();
            let home = tmp.path();
            {
                let conn = Connection::open(&db_path).unwrap();
                create_message_table(&conn);
                insert_message(
                    &conn,
                    "m1",
                    "s1",
                    "skill",
                    1000,
                    1000,
                    r#"{"skill":"deploy"}"#,
                );
            }

            let mut index = SkillInvocationIndex::default();
            let sources = DiscoverySources::default();
            let first = index.refresh(home, &sources);
            assert_eq!(first.databases_read, 1);

            let second = index.refresh(home, &sources);
            assert_eq!(second.databases_read, 0);
        }

        #[test]
        fn a_later_row_is_counted_without_doubling_the_earlier_one() {
            let (tmp, db_path) = temp_opencode_db_home();
            let home = tmp.path();
            {
                let conn = Connection::open(&db_path).unwrap();
                create_message_table(&conn);
                insert_message(
                    &conn,
                    "m1",
                    "s1",
                    "skill",
                    1000,
                    1000,
                    r#"{"skill":"deploy"}"#,
                );
            }

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["deploy", "lint"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            assert_eq!(stats(&index, &known_skills, &sources)[0].total, 1);

            {
                let conn = Connection::open(&db_path).unwrap();
                insert_message(
                    &conn,
                    "m2",
                    "s1",
                    "skill",
                    2000,
                    2000,
                    r#"{"skill":"lint"}"#,
                );
            }
            index.clear_database_stamp(&db_path);
            let report = index.refresh(home, &sources);
            assert_eq!(report.databases_read, 1);
            let stats = stats(&index, &known_skills, &sources);
            let total: u32 = stats.iter().map(|s| s.total).sum();
            assert_eq!(total, 2, "old row must not be doubled");
        }

        #[test]
        fn updating_a_row_in_place_gives_exactly_one_use() {
            let (tmp, db_path) = temp_opencode_db_home();
            let home = tmp.path();
            {
                let conn = Connection::open(&db_path).unwrap();
                create_message_table(&conn);
                insert_message(
                    &conn,
                    "m1",
                    "s1",
                    "assistant",
                    1000,
                    1000,
                    &skill_tool_message("c1", "running", "deploy"),
                );
            }

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["deploy"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            assert_eq!(stats(&index, &known_skills, &sources)[0].total, 1);

            {
                let conn = Connection::open(&db_path).unwrap();
                conn.execute(
                    "UPDATE session_message SET data = ?1, time_updated = ?2 WHERE id = 'm1'",
                    params![skill_tool_message("c1", "completed", "deploy"), 2000],
                )
                .unwrap();
            }
            index.clear_database_stamp(&db_path);
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            assert_eq!(stats.len(), 1);
            assert_eq!(stats[0].total, 1);
        }

        #[test]
        fn deleting_a_row_removes_its_use() {
            let (tmp, db_path) = temp_opencode_db_home();
            let home = tmp.path();
            {
                let conn = Connection::open(&db_path).unwrap();
                create_message_table(&conn);
                insert_message(
                    &conn,
                    "keep",
                    "s1",
                    "skill",
                    500,
                    500,
                    r#"{"skill":"lint"}"#,
                );
                insert_message(
                    &conn,
                    "del",
                    "s1",
                    "skill",
                    1000,
                    1000,
                    r#"{"skill":"deploy"}"#,
                );
            }

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["deploy", "lint"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let total: u32 = stats(&index, &known_skills, &sources)
                .iter()
                .map(|s| s.total)
                .sum();
            assert_eq!(total, 2);

            {
                let conn = Connection::open(&db_path).unwrap();
                conn.execute("DELETE FROM session_message WHERE id = 'del'", [])
                    .unwrap();
            }
            index.clear_database_stamp(&db_path);
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            let names: BTreeSet<&str> = stats.iter().map(|s| s.skill.as_str()).collect();
            assert_eq!(names, BTreeSet::from(["lint"]));
        }

        #[test]
        fn same_row_id_in_two_databases_counts_once() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            for name in ["opencode.db", "opencode-next.db"] {
                let db_path = opencode_db_path(home, name);
                fs::create_dir_all(db_path.parent().unwrap()).unwrap();
                let conn = Connection::open(&db_path).unwrap();
                create_message_table(&conn);
                insert_message(
                    &conn,
                    "m1",
                    "s1",
                    "skill",
                    1000,
                    1000,
                    r#"{"skill":"deploy"}"#,
                );
            }

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["deploy"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            let stats = stats(&index, &known_skills, &sources);
            assert_eq!(stats.len(), 1);
            assert_eq!(stats[0].total, 1);
        }

        #[test]
        fn switched_off_source_reads_nothing_and_keeps_cached_uses() {
            let (tmp, db_path) = temp_opencode_db_home();
            let home = tmp.path();
            {
                let conn = Connection::open(&db_path).unwrap();
                create_message_table(&conn);
                insert_message(
                    &conn,
                    "m1",
                    "s1",
                    "skill",
                    1000,
                    1000,
                    r#"{"skill":"deploy"}"#,
                );
            }

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["deploy"]);
            let enabled = DiscoverySources::default();
            index.refresh(home, &enabled);
            assert_eq!(stats(&index, &known_skills, &enabled).len(), 1);

            let mut off = DiscoverySources::default();
            off.set(AgentId::OPEN_CODE, false);
            let report = index.refresh(home, &off);
            assert_eq!(report.databases_read, 0);
            assert!(stats(&index, &known_skills, &off).is_empty());

            let report = index.refresh(home, &enabled);
            assert_eq!(
                report.databases_read, 0,
                "stamp was never touched while off"
            );
            assert_eq!(stats(&index, &known_skills, &enabled).len(), 1);
        }

        #[test]
        fn deleting_the_database_file_drops_its_entry() {
            let (tmp, db_path) = temp_opencode_db_home();
            let home = tmp.path();
            {
                let conn = Connection::open(&db_path).unwrap();
                create_message_table(&conn);
                insert_message(
                    &conn,
                    "m1",
                    "s1",
                    "skill",
                    1000,
                    1000,
                    r#"{"skill":"deploy"}"#,
                );
            }

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["deploy"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);
            assert_eq!(stats(&index, &known_skills, &sources).len(), 1);

            fs::remove_file(&db_path).unwrap();
            let report = index.refresh(home, &sources);
            assert_eq!(report.files_dropped, 1);
            assert!(stats(&index, &known_skills, &sources).is_empty());
        }

        #[test]
        fn refresh_never_creates_wal_or_shm_sidecars() {
            let (tmp, db_path) = temp_opencode_db_home();
            let home = tmp.path();
            {
                let conn = Connection::open(&db_path).unwrap();
                create_message_table(&conn);
                insert_message(
                    &conn,
                    "m1",
                    "s1",
                    "skill",
                    1000,
                    1000,
                    r#"{"skill":"deploy"}"#,
                );
            }

            let mut index = SkillInvocationIndex::default();
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);

            let mut wal = db_path.as_os_str().to_owned();
            wal.push("-wal");
            let mut shm = db_path.as_os_str().to_owned();
            shm.push("-shm");
            assert!(!Path::new(&wal).exists());
            assert!(!Path::new(&shm).exists());
        }

        #[test]
        fn cache_round_trip_keeps_database_uses_and_a_files_only_cache_still_loads() {
            let (tmp, db_path) = temp_opencode_db_home();
            let home = tmp.path();
            {
                let conn = Connection::open(&db_path).unwrap();
                create_message_table(&conn);
                insert_message(
                    &conn,
                    "m1",
                    "s1",
                    "skill",
                    1000,
                    1000,
                    r#"{"skill":"deploy"}"#,
                );
            }

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["deploy"]);
            let sources = DiscoverySources::default();
            index.refresh(home, &sources);

            let cache_path = tmp.path().join("cache/skill-uses.json");
            index.save(&cache_path).unwrap();
            let loaded = SkillInvocationIndex::load_or_empty(&cache_path);
            let stats = stats(&loaded, &known_skills, &sources);
            assert_eq!(stats.len(), 1);
            assert_eq!(stats[0].skill, "deploy");

            let files_only = tmp.path().join("files-only.json");
            fs::write(&files_only, r#"{"files":{}}"#).unwrap();
            let loaded = SkillInvocationIndex::load_or_empty(&files_only);
            assert!(loaded.files.is_empty());
            assert!(loaded.databases.is_empty());
        }

        #[test]
        fn a_row_with_a_null_time_created_is_skipped_and_the_other_row_still_counts() {
            let (tmp, db_path) = temp_opencode_db_home();
            let home = tmp.path();
            {
                let conn = Connection::open(&db_path).unwrap();
                create_message_table(&conn);
                create_part_table(&conn);
                conn.execute(
                    "INSERT INTO session_message (id, session_id, type, time_created, \
                     time_updated, data) VALUES ('bad', 's1', 'skill', NULL, 1000, ?1)",
                    params![r#"{"skill":"deploy"}"#],
                )
                .unwrap();
                insert_message(
                    &conn,
                    "good",
                    "s1",
                    "skill",
                    2000,
                    2000,
                    r#"{"skill":"lint"}"#,
                );
                conn.execute(
                    "INSERT INTO part (id, session_id, time_created, time_updated, data) \
                     VALUES ('bad-part', 's1', NULL, 1000, ?1)",
                    params![r#"{"type":"tool","tool":"skill","state":{"status":"completed","input":{"name":"deploy"}}}"#],
                )
                .unwrap();
            }

            let mut index = SkillInvocationIndex::default();
            let known_skills = known(&["deploy", "lint"]);
            let sources = DiscoverySources::default();
            let report = index.refresh(home, &sources);
            assert_eq!(report.databases_read, 1);
            let stats = stats(&index, &known_skills, &sources);
            let names: BTreeSet<&str> = stats.iter().map(|s| s.skill.as_str()).collect();
            assert_eq!(
                names,
                BTreeSet::from(["lint"]),
                "the bad row must not fail the read"
            );
        }

        #[test]
        fn an_unreadable_database_file_reads_nothing_and_never_sets_incomplete() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path();
            // An existing (empty) Claude Code projects dir, so this test's
            // `incomplete` assertion is about the OpenCode read failure, not
            // an unrelated missing-directory failure on the other source.
            fs::create_dir_all(home.join(CLAUDE_PROJECTS_ROOT)).unwrap();
            let db_path = opencode_db_path(home, "opencode.db");
            fs::create_dir_all(db_path.parent().unwrap()).unwrap();
            fs::write(&db_path, b"not a database").unwrap();

            let mut index = SkillInvocationIndex::default();
            let sources = DiscoverySources::default();
            let report = index.refresh(home, &sources);
            assert_eq!(report.databases_read, 0);
            assert!(
                !report.incomplete,
                "a failed database read must not force a re-run loop"
            );
            assert!(
                index.databases.is_empty(),
                "a failed read must not cache the file's stamp"
            );
        }
    }
}
