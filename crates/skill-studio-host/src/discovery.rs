//! [`ProjectDiscovery`] over the project history each harness keeps under the
//! home directory.
//!
//! The union of Codex's `~/.codex/config.toml` recent projects, the working
//! directories in Claude Code and pi session transcripts, the folders in
//! Cursor's workspace storage, the project worktrees `OpenCode` records, and
//! the working directories Grok Build names its session folders after,
//! filtered to directories that hold a skill dir for one of the first-class
//! agents. A harness switched off in the `discovery` section of
//! `~/.agents/skill-studio.json` is not read.

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use skill_studio_core::discovery_sources::DiscoverySources;
use skill_studio_core::error::CoreError;
use skill_studio_core::identity::AgentId;
use skill_studio_core::ports::ProjectDiscovery;
use skill_studio_core::tracked_projects::SKILL_DIR_MARKERS;

use crate::fs::RealFs;
use crate::opencode_db::{open_opencode_database, opencode_databases, OPENCODE_DATA_ROOT};

/// Codex's own directory: `$CODEX_HOME` when set to a non-empty value, else
/// `<home>/.codex`. This is the one place allowed to read `CODEX_HOME` - the
/// core crate never does (`docs/action-map/harnesses/codex.md`, "Resolved by
/// the docs on 2026-09-16": "`CODEX_HOME` overrides `~/.codex` for config,
/// sessions, and the `SQLite` state; every Codex path in the app must honour
/// it").
pub fn codex_home(home: &Path) -> PathBuf {
    match std::env::var_os("CODEX_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home.join(".codex"),
    }
}

/// Project paths recorded in Codex's `[projects."/abs/path"]` config
/// sections (`~/.codex/config.toml`, or `$CODEX_HOME/config.toml`).
fn codex_project_paths(home: &Path) -> Vec<PathBuf> {
    let Ok(content) = fs::read_to_string(codex_home(home).join("config.toml")) else {
        return Vec::new();
    };
    let Ok(value) = content.parse::<toml::Table>() else {
        return Vec::new();
    };
    value
        .get("projects")
        .and_then(|v| v.as_table())
        .map(|table| table.keys().map(PathBuf::from).collect())
        .unwrap_or_default()
}

/// Lines examined per transcript file, and the max size of a single line,
/// before giving up on that file and falling back to the next-newest one.
const MAX_TRANSCRIPT_LINES: usize = 200;
const MAX_TRANSCRIPT_LINE_BYTES: usize = 64 * 1024;

/// Total bytes read from a single transcript file before giving up on it and
/// falling back to the next-newest one. Bounds worst-case read work per file
/// independently of `MAX_TRANSCRIPT_LINES`, since a file made of many small
/// lines could otherwise still cost an unbounded amount of I/O.
const MAX_TRANSCRIPT_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Total bytes one discovery run may read under one transcript root. This
/// bounds one refresh even when no transcript carries a recognizable `cwd`
/// (e.g. after a transcript schema change).
const MAX_TRANSCRIPT_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

/// Transcript files one discovery run may try to open under one transcript
/// root. This independently bounds empty and unreadable files, which do not
/// consume the byte budget.
const MAX_TRANSCRIPT_ATTEMPTS: usize = 10_000;

/// Claude Code keeps one directory per project here. The directory name
/// encodes the path lossily, so the `cwd` inside the transcripts is read.
const CLAUDE_TRANSCRIPT_ROOT: &str = ".claude/projects";

/// pi keeps one `--<cwd with / \ : as ->--` directory per project here. That
/// name cannot be decoded for folders whose own names contain `-`, so the
/// `cwd` in each session's header record is read instead.
const PI_TRANSCRIPT_ROOT: &str = ".pi/agent/sessions";

/// Depth [`nested_pi_project_roots`] descends below a discovered project
/// root while collecting nested `.pi/skills` folders. pi's own skills doc
/// says project roots are walked recursively to the git root (unlike the
/// one-level project root every other first-class agent uses); this caps
/// how deep a monorepo is walked in the opposite direction (down from an
/// already-discovered root, not up from a `cwd`) while still reaching a
/// realistic nesting depth.
const PI_NESTED_ROOT_WALK_DEPTH: u32 = 6;

/// Directory entries [`walk_for_pi_roots`] may examine below one pi `cwd`
/// before it gives up on that `cwd`, bounding worst-case I/O against a
/// monorepo with enormous fan-out at any depth, including the last one
/// [`PI_NESTED_ROOT_WALK_DEPTH`] allows.
const PI_NESTED_ROOT_WALK_BUDGET: usize = 2_000;

/// High-fanout folder names skipped in addition to hidden ones, so the walk
/// does not spend its budget descending into dependency or build-output
/// trees that never hold a nested pi project.
const PI_NESTED_ROOT_WALK_SKIP_NAMES: &[&str] = &["node_modules", "target", "Library"];

/// Every directory under `root` (not `root` itself) that holds a `.pi/skills`
/// folder, found by walking down up to [`PI_NESTED_ROOT_WALK_DEPTH`] levels
/// and [`PI_NESTED_ROOT_WALK_BUDGET`] directories. Hidden directories
/// (`.git`, `.pi`, ...) and [`PI_NESTED_ROOT_WALK_SKIP_NAMES`] are never
/// descended into, so a project's own `.pi/skills` is found by the marker
/// check but never mistaken for a nested project root.
fn nested_pi_project_roots(home: &Path, root: &Path) -> Vec<PathBuf> {
    nested_pi_project_roots_with_budget(home, root, PI_NESTED_ROOT_WALK_BUDGET)
}

/// [`nested_pi_project_roots`] with an explicit directory budget, so a test
/// can bound the walk without building thousands of fixture directories.
fn nested_pi_project_roots_with_budget(home: &Path, root: &Path, budget: usize) -> Vec<PathBuf> {
    // A `cwd` this shallow (e.g. `/`, pi's own sentinel for a session
    // outside any project), a filesystem root (also true of a Windows drive
    // root like `C:\`, whose `parent()` is `None`), the home directory
    // itself, or Skill Studio's own scratch root is never a real project
    // root; walking any of these would mean reading a huge, unrelated part
    // of the filesystem.
    if root.components().count() < 2
        || root.parent().is_none()
        || is_home_root(home, root)
        || is_studio_scratch_path(home, root)
    {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut remaining_budget = budget;
    walk_for_pi_roots(
        root,
        PI_NESTED_ROOT_WALK_DEPTH,
        &mut found,
        &mut remaining_budget,
    );
    found
}

fn walk_for_pi_roots(
    dir: &Path,
    depth_remaining: u32,
    found: &mut Vec<PathBuf>,
    remaining_budget: &mut usize,
) {
    if depth_remaining == 0 || *remaining_budget == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || PI_NESTED_ROOT_WALK_SKIP_NAMES.contains(&name.as_ref()) {
            continue;
        }
        if *remaining_budget == 0 {
            break;
        }
        *remaining_budget -= 1;
        let path = entry.path();
        let has_pi_skills = path.join(".pi/skills").exists();
        walk_for_pi_roots(&path, depth_remaining - 1, found, remaining_budget);
        if has_pi_skills {
            found.push(path);
        }
    }
}

struct TranscriptScanLimits {
    remaining_bytes: u64,
    remaining_attempts: usize,
}

impl TranscriptScanLimits {
    fn new(remaining_bytes: u64, remaining_attempts: usize) -> Self {
        Self {
            remaining_bytes,
            remaining_attempts,
        }
    }

    fn can_attempt_transcript(&self) -> bool {
        self.remaining_bytes > 0 && self.remaining_attempts > 0
    }

    fn begin_transcript_attempt(&mut self) -> Option<u64> {
        if !self.can_attempt_transcript() {
            return None;
        }
        self.remaining_attempts -= 1;
        Some(MAX_TRANSCRIPT_FILE_BYTES.min(self.remaining_bytes))
    }

    fn consume_bytes(&mut self, bytes: u64) {
        self.remaining_bytes = self.remaining_bytes.saturating_sub(bytes);
    }
}

/// The `cwd` recorded in a single transcript file: the first line (of up to
/// `MAX_TRANSCRIPT_LINES`, each capped at `MAX_TRANSCRIPT_LINE_BYTES`, within
/// a `MAX_TRANSCRIPT_FILE_BYTES` total budget) that mentions `"cwd"` and
/// parses as JSON with an absolute-path `cwd` string. A line that overruns
/// the per-line cap abandons the whole file rather than draining and
/// continuing, so a single pathological line can't be used to keep reading
/// past the file's budget one bounded chunk at a time.
fn cwd_from_transcript(path: &Path, limits: &mut TranscriptScanLimits) -> Option<PathBuf> {
    let mut budget = limits.begin_transcript_attempt()?;
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    // Reused across iterations and cleared each time, so memory use is
    // bounded by one line's worth of bytes rather than growing with the
    // number of lines scanned.
    let mut buf: Vec<u8> = Vec::new();
    for _ in 0..MAX_TRANSCRIPT_LINES {
        if budget == 0 {
            break;
        }
        buf.clear();
        // Cap the read at the limit (+1, to distinguish "found the newline
        // right at the cap" from "no newline within the cap") so a
        // pathologically long line is never buffered in full. Also capped by
        // the file's remaining byte budget.
        let line_cap = (MAX_TRANSCRIPT_LINE_BYTES as u64 + 1).min(budget);
        let read = reader.by_ref().take(line_cap).read_until(b'\n', &mut buf);
        match read {
            Ok(0) | Err(_) => break, // EOF, or a read error treated the same way
            Ok(n) => {
                budget = budget.saturating_sub(n as u64);
                limits.consume_bytes(n as u64);
            }
        }
        let oversized =
            buf.len() as u64 > MAX_TRANSCRIPT_LINE_BYTES as u64 && buf.last() != Some(&b'\n');
        if oversized {
            // No newline within the limit: abandon this file entirely rather
            // than draining the rest of the offending line, so a
            // pathological line can't be used to keep reading past budget.
            return None;
        }
        let Ok(trimmed) = std::str::from_utf8(&buf) else {
            continue;
        };
        let trimmed = trimmed.trim_end_matches(['\n', '\r']);
        if !trimmed.contains("\"cwd\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let Some(cwd) = value.get("cwd").and_then(|v| v.as_str()) else {
            continue;
        };
        let path = PathBuf::from(cwd);
        if path.is_absolute() {
            return Some(path);
        }
    }
    None
}

/// The distinct `cwd` values recorded in the transcripts under `root`, which
/// holds one directory of `*.jsonl` files per project. Each encoded project
/// directory can represent more than one real path, so every `*.jsonl` file
/// is scanned newest-first within the total byte budget.
fn transcript_cwds(root: &Path) -> Vec<PathBuf> {
    transcript_cwds_within(
        root,
        TranscriptScanLimits::new(MAX_TRANSCRIPT_TOTAL_BYTES, MAX_TRANSCRIPT_ATTEMPTS),
    )
}

/// `transcript_cwds` with explicit limits. Stops scanning and returns what it
/// found when either limit is spent.
fn transcript_cwds_within(root: &Path, mut limits: TranscriptScanLimits) -> Vec<PathBuf> {
    let mut out = BTreeSet::new();

    let Ok(project_dirs) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut project_dirs: Vec<_> = project_dirs.flatten().collect();
    project_dirs.sort_by_key(std::fs::DirEntry::path);
    for project_dir in project_dirs {
        if !limits.can_attempt_transcript() {
            break;
        }
        let dir = project_dir.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut transcripts: Vec<_> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
            .filter(|e| is_regular_file(&e.path()))
            .collect();
        transcripts.sort_by(|left, right| {
            let left_modified = left
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let right_modified = right
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            right_modified
                .cmp(&left_modified)
                .then_with(|| left.path().cmp(&right.path()))
        });

        for transcript in transcripts {
            if !limits.can_attempt_transcript() {
                break;
            }
            if let Some(cwd) = cwd_from_transcript(&transcript.path(), &mut limits) {
                out.insert(cwd);
            }
        }
    }
    out.into_iter().collect()
}

/// Cursor inherits VS Code's per-workspace storage: one
/// `<hash>/workspace.json` per opened folder. Cursor does not document the
/// location, so every OS's home-relative VS Code path is tried (macOS, Linux,
/// Windows).
const CURSOR_WORKSPACE_STORAGE_ROOTS: &[&str] = &[
    "Library/Application Support/Cursor/User/workspaceStorage",
    ".config/Cursor/User/workspaceStorage",
    "AppData/Roaming/Cursor/User/workspaceStorage",
];

/// `workspace.json` files one discovery run may try to open.
const MAX_CURSOR_WORKSPACES: usize = 10_000;

/// Local folders Cursor has opened. Multi-root workspaces (`workspace`) and
/// remote folders (`vscode-remote://`) name no local project root and are
/// skipped.
pub(crate) fn cursor_workspace_folders(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut remaining = MAX_CURSOR_WORKSPACES;
    for root in CURSOR_WORKSPACE_STORAGE_ROOTS {
        let Ok(entries) = fs::read_dir(home.join(root)) else {
            continue;
        };
        for entry in entries.flatten() {
            if remaining == 0 {
                return out;
            }
            remaining -= 1;
            out.extend(cursor_workspace_folder(
                &entry.path().join("workspace.json"),
            ));
        }
    }
    out
}

fn cursor_workspace_folder(path: &Path) -> Option<PathBuf> {
    let value = read_small_json(path)?;
    let folder = value.get("folder")?.as_str()?;
    url::Url::parse(folder).ok()?.to_file_path().ok()
}

/// Project rows read per database, and legacy project files read in total.
const MAX_OPENCODE_PROJECTS: usize = 10_000;

/// Worktrees of the projects `OpenCode` has opened, from the database of every
/// channel and from the `storage/project/<id>.json` records that `OpenCode`
/// wrote before it moved to `SQLite`.
fn opencode_worktrees(home: &Path) -> Vec<PathBuf> {
    let root = home.join(OPENCODE_DATA_ROOT);
    let mut out = opencode_legacy_worktrees(&root.join("storage/project"));
    for database in opencode_databases(home) {
        out.extend(opencode_database_worktrees(&database));
    }
    out
}

/// `project.worktree` values from one `OpenCode` database.
fn opencode_database_worktrees(database: &Path) -> Vec<PathBuf> {
    let Some(conn) = open_opencode_database(database) else {
        return Vec::new();
    };
    let Ok(mut statement) = conn.prepare("SELECT worktree FROM project LIMIT ?1") else {
        return Vec::new();
    };
    let limit = i64::try_from(MAX_OPENCODE_PROJECTS).unwrap_or(i64::MAX);
    let Ok(rows) = statement.query_map([limit], |row| row.get::<_, String>(0)) else {
        return Vec::new();
    };
    let worktrees: Vec<PathBuf> = rows
        .flatten()
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .collect();
    worktrees
}

fn opencode_legacy_worktrees(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .take(MAX_OPENCODE_PROJECTS)
        .filter_map(|path| {
            let value = read_small_json(&path)?;
            value.get("worktree")?.as_str().map(PathBuf::from)
        })
        .filter(|path| path.is_absolute())
        .collect()
}

/// Grok Build's session store at its default `$GROK_HOME` location. Each
/// child directory is named for the working directory its sessions ran in,
/// by `encode_cwd_dirname` in Grok's `xai-grok-config/src/paths.rs`.
const GROK_SESSIONS_ROOT: &str = ".grok/sessions";

/// Session store entries one discovery run may look at. Shared with
/// `skill_uses.rs`'s `list_grok_sessions`, which walks the same tree.
pub(crate) const MAX_GROK_SESSION_DIRS: usize = 10_000;

fn grok_session_cwds(home: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(home.join(GROK_SESSIONS_ROOT)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .take(MAX_GROK_SESSION_DIRS)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| grok_session_cwd(&entry.path()))
        .filter(|path| path.is_absolute())
        .collect()
}

/// Grok names the directory by the percent-encoded cwd while that fits in
/// 255 bytes. A longer cwd gets a `<slug>-<hash>` name and a `.cwd` file
/// that holds the path. A slug never decodes to an absolute path, which is
/// how Grok's own `decode_cwd_from_dirname` tells the two apart.
pub(crate) fn grok_session_cwd(dir: &Path) -> Option<PathBuf> {
    let name = dir.file_name()?.to_str()?;
    if let Ok(decoded) = percent_encoding::percent_decode_str(name).decode_utf8() {
        let path = PathBuf::from(decoded.into_owned());
        if path.is_absolute() {
            return Some(path);
        }
    }
    read_small_file(&dir.join(".cwd")).map(|cwd| PathBuf::from(cwd.trim()))
}

/// Cursor workspace records, `OpenCode` project records, and Grok `.cwd` files
/// each hold one path and a few fields.
const MAX_SMALL_FILE_BYTES: u64 = 64 * 1024;

fn read_small_file(path: &Path) -> Option<String> {
    if !is_regular_file(path) {
        return None;
    }
    let mut content = String::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_SMALL_FILE_BYTES)
        .read_to_string(&mut content)
        .ok()?;
    Some(content)
}

fn read_small_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&read_small_file(path)?).ok()
}

/// Opening a FIFO blocks, and a symlink can point anywhere, so history files
/// are read only when they are regular files.
fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

/// True when `path` is inside Skill Studio's own scratch root - the
/// assistant's Test and Audit runs create a throwaway project there and
/// Claude Code records a transcript for it, which would otherwise be adopted
/// as one of the user's projects.
fn is_studio_scratch_path(home: &Path, path: &Path) -> bool {
    path.starts_with(home.join("Library/Caches/com.skillstudio.app"))
        || path.starts_with(home.join(".cache/com.skillstudio.app"))
}

/// True when `path` is the home root itself. A session run from the home
/// directory records `cwd` as the home, and the home has a skill dir by
/// definition, so it would otherwise be adopted as one of the user's own
/// projects. `NormalizedScope` rejects that scope outright, which would
/// leave the default `scan` failing on any machine where Claude Code has
/// ever been started from the home directory.
fn is_home_root(home: &Path, path: &Path) -> bool {
    let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    canonical(path) == canonical(home)
}

fn claude_transcript_cwds(home: &Path) -> Vec<PathBuf> {
    transcript_cwds(&home.join(CLAUDE_TRANSCRIPT_ROOT))
}

fn pi_transcript_cwds(home: &Path) -> Vec<PathBuf> {
    transcript_cwds(&home.join(PI_TRANSCRIPT_ROOT))
}

type HistorySource = fn(&Path) -> Vec<PathBuf>;

/// Each harness whose history discovery reads, in the order a settings
/// screen lists them, with the reader for that history.
const HISTORY_SOURCES: &[(&str, HistorySource)] = &[
    (AgentId::CLAUDE_CODE, claude_transcript_cwds),
    (AgentId::CODEX, codex_project_paths),
    (AgentId::OPEN_CODE, opencode_worktrees),
    (AgentId::PI, pi_transcript_cwds),
    (AgentId::CURSOR, cursor_workspace_folders),
    (AgentId::GROK_BUILD, grok_session_cwds),
];

/// Ids of the harnesses whose history discovery can read, which are the
/// keys of [`DiscoverySources`] that have an effect.
pub fn discovery_harnesses() -> impl Iterator<Item = &'static str> {
    HISTORY_SOURCES.iter().map(|(harness, _)| *harness)
}

/// [`discover_skill_projects_from`] with the switches saved under `home`.
pub fn discover_skill_projects(home: &Path) -> Vec<PathBuf> {
    discover_skill_projects_from(home, &DiscoverySources::read(&RealFs, home))
}

/// Union of every project directory nominated by an enabled harness's
/// history (Codex config, Claude Code and pi transcripts, Cursor workspace
/// storage, `OpenCode`'s project records, and Grok Build's session folders),
/// filtered to directories that exist and have at least one first-class
/// agent's skill dir. Sorted and deduped.
fn discover_skill_projects_from(home: &Path, sources: &DiscoverySources) -> Vec<PathBuf> {
    let mut paths: BTreeSet<PathBuf> = BTreeSet::new();
    for (harness, read_history) in HISTORY_SOURCES {
        if !sources.is_enabled(harness) {
            continue;
        }
        let discovered = read_history(home);
        // pi walks project roots recursively to the git root
        // (docs/action-map/harnesses/pi.md), so a nested `.pi/skills`
        // several levels below a pi transcript's own `cwd` is still a
        // project root, not just the `cwd` itself. Scoped to pi's own
        // history only: another harness's sentinel value (e.g. OpenCode's
        // "/" for sessions outside any project) is never a real project
        // root to walk.
        if *harness == AgentId::PI {
            for p in &discovered {
                paths.extend(nested_pi_project_roots(home, p));
            }
        }
        paths.extend(discovered);
    }

    paths
        .into_iter()
        .filter(|p| {
            p.exists()
                && !is_studio_scratch_path(home, p)
                && !is_home_root(home, p)
                && SKILL_DIR_MARKERS
                    .iter()
                    .any(|marker| p.join(marker).exists())
        })
        .collect()
}

/// `ProjectDiscovery` backed by the harness project histories under the home
/// directory ([`ports::ProjectDiscovery`](ProjectDiscovery)).
pub struct HostProjectDiscovery;

impl HostProjectDiscovery {
    /// Builds a discovery adapter. Holds no state; every call re-reads the
    /// discovery switches and the harness histories under the given home, so
    /// a long-running server sees a changed switch on its next call.
    pub fn new() -> Self {
        HostProjectDiscovery
    }
}

impl Default for HostProjectDiscovery {
    fn default() -> Self {
        HostProjectDiscovery::new()
    }
}

impl ProjectDiscovery for HostProjectDiscovery {
    fn discover_projects(&self, home_root: &Path) -> Result<Vec<PathBuf>, CoreError> {
        Ok(discover_skill_projects(home_root))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn codex_config_toml_projects_are_parsed_and_filtered() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("my-project");
        fs::create_dir_all(project.join(".codex/skills")).unwrap();

        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::write(
            home.join(".codex/config.toml"),
            format!(
                "[projects.\"{}\"]\ntrusted = true\n",
                project.to_string_lossy()
            ),
        )
        .unwrap();

        let found = discover_skill_projects(home);
        assert_eq!(found, vec![project]);
    }

    #[test]
    fn operation_wide_byte_budget_stops_transcript_scanning() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        // Ten project dirs, each with a 1 KiB transcript that carries no cwd
        // until the very last line; a small total budget must stop the scan
        // before it reaches most of them.
        for i in 0..10 {
            let dir = home.join(format!(".claude/projects/-p{i}"));
            fs::create_dir_all(&dir).unwrap();
            let filler = format!("{{\"type\":\"x\",\"pad\":\"{}\"}}\n", "a".repeat(900));
            let cwd_line = format!(
                "{{\"cwd\":\"{}\"}}\n",
                home.join(format!("proj{i}")).display()
            );
            fs::write(dir.join("s.jsonl"), format!("{filler}{cwd_line}")).unwrap();
            fs::create_dir_all(home.join(format!("proj{i}/.claude/skills"))).unwrap();
        }

        let unbounded = transcript_cwds_within(
            &home.join(CLAUDE_TRANSCRIPT_ROOT),
            TranscriptScanLimits::new(u64::MAX, usize::MAX),
        );
        assert_eq!(unbounded.len(), 10);

        let bounded = transcript_cwds_within(
            &home.join(CLAUDE_TRANSCRIPT_ROOT),
            TranscriptScanLimits::new(2_500, usize::MAX),
        );
        assert!(
            bounded.len() <= 3,
            "budget should stop the scan early: {bounded:?}"
        );
    }

    #[test]
    fn empty_and_invalid_transcripts_exhaust_attempt_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("older-valid-project");
        fs::create_dir_all(project.join(".claude/skills")).unwrap();

        let transcript_dir = home.join(".claude/projects/-attempt-limit");
        fs::create_dir_all(&transcript_dir).unwrap();
        let valid = transcript_dir.join("oldest-valid.jsonl");
        fs::write(
            &valid,
            format!(r#"{{"type":"user","cwd":"{}"}}"#, project.display()),
        )
        .unwrap();
        let invalid = transcript_dir.join("middle-invalid.jsonl");
        fs::write(&invalid, "not json\n").unwrap();
        let empty = transcript_dir.join("newest-empty.jsonl");
        fs::write(&empty, "").unwrap();

        let now = SystemTime::now();
        fs::File::open(&valid)
            .unwrap()
            .set_modified(now - std::time::Duration::from_secs(120))
            .unwrap();
        fs::File::open(&invalid)
            .unwrap()
            .set_modified(now - std::time::Duration::from_secs(60))
            .unwrap();
        fs::File::open(&empty).unwrap().set_modified(now).unwrap();

        let limits = TranscriptScanLimits::new(u64::MAX, 2);
        let found = transcript_cwds_within(&home.join(CLAUDE_TRANSCRIPT_ROOT), limits);

        assert!(found.is_empty());
    }

    #[test]
    fn claude_transcript_cwd_is_parsed_and_filtered() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("another-project");
        fs::create_dir_all(project.join(".claude/skills")).unwrap();

        let transcript_dir = home.join(".claude/projects/-some-project");
        fs::create_dir_all(&transcript_dir).unwrap();
        fs::write(
            transcript_dir.join("session.jsonl"),
            format!(
                r#"{{"type":"user","cwd":"{}","message":{{}}}}"#,
                project.to_string_lossy()
            ),
        )
        .unwrap();

        let found = discover_skill_projects(home);
        assert_eq!(found, vec![project]);
    }

    /// A session started from the home directory records `cwd` as the home,
    /// and the home always has a skill dir, so discovery used to return the
    /// home as a project. `NormalizedScope` rejects a scope whose project
    /// equals its home, so the default `skill-studio scan` then failed with
    /// `invalid_scope` on any such machine.
    #[test]
    fn the_home_root_is_never_discovered_as_one_of_its_own_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join(".claude/skills")).unwrap();

        let transcript_dir = home.join(".claude/projects/-home");
        fs::create_dir_all(&transcript_dir).unwrap();
        fs::write(
            transcript_dir.join("session.jsonl"),
            format!(
                r#"{{"type":"user","cwd":"{}","message":{{}}}}"#,
                home.to_string_lossy()
            ),
        )
        .unwrap();

        let discovery = HostProjectDiscovery::new();
        let found = discovery.discover_projects(home).unwrap();
        assert!(
            found.is_empty(),
            "the home root was adopted as one of its own projects: {found:?}"
        );
    }

    #[test]
    fn colliding_claude_project_directory_discovers_every_transcript_cwd() {
        fn claude_project_dir_name(path: &Path) -> String {
            path.to_string_lossy()
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() {
                        character
                    } else {
                        '-'
                    }
                })
                .collect()
        }

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let hyphenated_project = home.join("foo-bar");
        let nested_project = home.join("foo/bar");
        fs::create_dir_all(hyphenated_project.join(".claude/skills")).unwrap();
        fs::create_dir_all(nested_project.join(".claude/skills")).unwrap();

        let encoded_hyphenated = claude_project_dir_name(&hyphenated_project);
        let encoded_nested = claude_project_dir_name(&nested_project);
        assert_eq!(encoded_hyphenated, encoded_nested);

        let transcript_dir = home.join(".claude/projects").join(encoded_hyphenated);
        fs::create_dir_all(&transcript_dir).unwrap();
        let older = transcript_dir.join("older.jsonl");
        fs::write(
            &older,
            format!(
                r#"{{"type":"user","cwd":"{}"}}"#,
                nested_project.to_string_lossy()
            ),
        )
        .unwrap();
        let newer = transcript_dir.join("newer.jsonl");
        fs::write(
            &newer,
            format!(
                r#"{{"type":"user","cwd":"{}"}}"#,
                hyphenated_project.to_string_lossy()
            ),
        )
        .unwrap();

        let now = SystemTime::now();
        fs::File::open(&older)
            .unwrap()
            .set_modified(now - std::time::Duration::from_secs(60))
            .unwrap();
        fs::File::open(&newer).unwrap().set_modified(now).unwrap();

        let found = discover_skill_projects(home);
        assert_eq!(found, vec![nested_project, hyphenated_project]);
    }

    #[test]
    fn whitespace_formatted_cwd_line_is_parsed() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("spaced-project");
        fs::create_dir_all(project.join(".claude/skills")).unwrap();

        let transcript_dir = home.join(".claude/projects/-spaced-project");
        fs::create_dir_all(&transcript_dir).unwrap();
        fs::write(
            transcript_dir.join("session.jsonl"),
            format!(
                r#"{{ "type" : "user" ,   "cwd" :  "{}" , "message": {{}} }}"#,
                project.to_string_lossy()
            ),
        )
        .unwrap();

        let found = discover_skill_projects(home);
        assert_eq!(found, vec![project]);
    }

    #[test]
    fn escaped_cwd_path_is_parsed() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("quo\"ted-project");
        fs::create_dir_all(project.join(".claude/skills")).unwrap();

        let transcript_dir = home.join(".claude/projects/-escaped-project");
        fs::create_dir_all(&transcript_dir).unwrap();
        let escaped = project.to_string_lossy().replace('"', "\\\"");
        fs::write(
            transcript_dir.join("session.jsonl"),
            format!(r#"{{"type":"user","cwd":"{escaped}"}}"#),
        )
        .unwrap();

        let found = discover_skill_projects(home);
        assert_eq!(found, vec![project]);
    }

    #[test]
    fn cwd_falls_back_to_older_file_when_newest_has_none() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("older-file-project");
        fs::create_dir_all(project.join(".claude/skills")).unwrap();

        let transcript_dir = home.join(".claude/projects/-older-file-project");
        fs::create_dir_all(&transcript_dir).unwrap();

        let older = transcript_dir.join("a-older.jsonl");
        fs::write(
            &older,
            format!(r#"{{"type":"user","cwd":"{}"}}"#, project.to_string_lossy()),
        )
        .unwrap();
        let newer = transcript_dir.join("b-newer.jsonl");
        fs::write(
            &newer,
            "{\"type\":\"summary\",\"summary\":\"no cwd here\"}\n",
        )
        .unwrap();

        let now = std::time::SystemTime::now();
        fs::File::open(&older)
            .unwrap()
            .set_modified(now - std::time::Duration::from_secs(3600))
            .unwrap();
        fs::File::open(&newer).unwrap().set_modified(now).unwrap();

        let found = discover_skill_projects(home);
        assert_eq!(found, vec![project]);
    }

    #[test]
    fn oversized_line_abandons_the_file_without_panicking() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("huge-line-project");
        // No skill dir here: the transcript's oversized line means its cwd
        // is never found, so this project must not surface.
        fs::create_dir_all(&project).unwrap();

        let transcript_dir = home.join(".claude/projects/-huge-line-project");
        fs::create_dir_all(&transcript_dir).unwrap();
        let mut content = Vec::new();
        // A 200 KiB line with no cwd, well past MAX_TRANSCRIPT_LINE_BYTES.
        content.extend(vec![b'x'; 200 * 1024]);
        content.push(b'\n');
        // A cwd line follows in the same file, but the file is abandoned as
        // soon as the oversized line is hit, so this must never be reached.
        content.extend(
            format!(r#"{{"type":"user","cwd":"{}"}}"#, project.to_string_lossy()).into_bytes(),
        );
        content.push(b'\n');
        fs::write(transcript_dir.join("session.jsonl"), content).unwrap();

        assert!(discover_skill_projects(home).is_empty());
    }

    #[test]
    fn oversized_line_abandons_file_and_older_file_cwd_is_still_found() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("older-file-project-2");
        fs::create_dir_all(project.join(".claude/skills")).unwrap();

        let transcript_dir = home.join(".claude/projects/-older-file-project-2");
        fs::create_dir_all(&transcript_dir).unwrap();

        let older = transcript_dir.join("a-older.jsonl");
        fs::write(
            &older,
            format!(r#"{{"type":"user","cwd":"{}"}}"#, project.to_string_lossy()),
        )
        .unwrap();

        let newer = transcript_dir.join("b-newer.jsonl");
        let mut content = Vec::new();
        content.extend(vec![b'x'; 200 * 1024]);
        content.push(b'\n');
        content.extend(
            format!(r#"{{"type":"user","cwd":"{}"}}"#, project.to_string_lossy()).into_bytes(),
        );
        content.push(b'\n');
        fs::write(&newer, content).unwrap();

        let now = std::time::SystemTime::now();
        fs::File::open(&older)
            .unwrap()
            .set_modified(now - std::time::Duration::from_secs(3600))
            .unwrap();
        fs::File::open(&newer).unwrap().set_modified(now).unwrap();

        let found = discover_skill_projects(home);
        assert_eq!(found, vec![project]);
    }

    #[test]
    fn non_regular_transcript_entry_is_skipped_without_error() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("dir-named-jsonl-project");
        fs::create_dir_all(project.join(".claude/skills")).unwrap();

        let transcript_dir = home.join(".claude/projects/-dir-named-jsonl-project");
        // A directory named `*.jsonl`: it matches the extension filter but
        // must never be opened as a transcript file.
        fs::create_dir_all(transcript_dir.join("weird.jsonl")).unwrap();
        fs::write(
            transcript_dir.join("session.jsonl"),
            format!(r#"{{"type":"user","cwd":"{}"}}"#, project.to_string_lossy()),
        )
        .unwrap();

        let found = discover_skill_projects(home);
        assert_eq!(found, vec![project]);
    }

    #[test]
    fn cursor_and_grok_skill_dirs_count_as_project_markers() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let cursor_project = home.join("cursor-project");
        fs::create_dir_all(cursor_project.join(".cursor/skills")).unwrap();
        let grok_project = home.join("grok-project");
        fs::create_dir_all(grok_project.join(".grok/skills")).unwrap();

        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::write(
            home.join(".codex/config.toml"),
            format!(
                "[projects.\"{}\"]\ntrusted = true\n[projects.\"{}\"]\ntrusted = true\n",
                cursor_project.to_string_lossy(),
                grok_project.to_string_lossy()
            ),
        )
        .unwrap();

        let projects = discover_skill_projects(home);
        assert!(projects.contains(&cursor_project));
        assert!(projects.contains(&grok_project));
    }

    #[test]
    fn projects_without_a_skill_dir_are_filtered_out() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("no-skills-here");
        fs::create_dir_all(&project).unwrap();

        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::write(
            home.join(".codex/config.toml"),
            format!(
                "[projects.\"{}\"]\ntrusted = true\n",
                project.to_string_lossy()
            ),
        )
        .unwrap();

        assert!(discover_skill_projects(home).is_empty());
    }

    #[test]
    fn studio_scratch_path_is_excluded_but_a_normal_project_with_the_same_marker_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        let scratch = home
            .join("Library/Caches/com.skillstudio.app/skill-studio/scratch/20260827-1")
            .join(".agents/skills");
        fs::create_dir_all(&scratch).unwrap();
        let project = home.join("real-project");
        fs::create_dir_all(project.join(".agents/skills")).unwrap();

        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::write(
            home.join(".codex/config.toml"),
            format!(
                "[projects.\"{}\"]\ntrusted = true\n[projects.\"{}\"]\ntrusted = true\n",
                scratch.ancestors().nth(2).unwrap().to_string_lossy(),
                project.to_string_lossy()
            ),
        )
        .unwrap();

        let found = discover_skill_projects(home);
        assert_eq!(found, vec![project]);
    }

    /// pi's directory name for `<home>/my-pi-project` decodes to
    /// `<home>/my/pi/project`, so this only passes when the header is read.
    #[test]
    fn pi_session_header_cwd_is_discovered() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("my-pi-project");
        fs::create_dir_all(project.join(".pi/skills")).unwrap();

        let encoded: String = project
            .to_string_lossy()
            .chars()
            .map(|c| {
                if matches!(c, '/' | '\\' | ':') {
                    '-'
                } else {
                    c
                }
            })
            .collect();
        let session_dir = home.join(PI_TRANSCRIPT_ROOT).join(format!("--{encoded}--"));
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("2026-09-16T10-00-00-000Z_0192.jsonl"),
            format!(
                "{}\n{}\n",
                serde_json::json!({
                    "type": "session",
                    "version": 3,
                    "id": "0192",
                    "timestamp": "2026-09-16T10:00:00.000Z",
                    "cwd": project,
                }),
                r#"{"type":"message","id":"a1","parentId":null}"#,
            ),
        )
        .unwrap();

        assert_eq!(discover_skill_projects(home), vec![project]);
    }

    /// Only the repo root ever appears in pi's session history; the nested
    /// `.pi/skills` two levels below it is found by walking down from that
    /// root, matching pi's own doc (recursive to the git root) rather than
    /// the one-level Claude-shaped reader the facts table used to carry.
    #[test]
    fn pi_project_roots_are_discovered_recursively_to_the_git_root_or_names_the_skipped_nested_folder(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("my-pi-repo");
        fs::create_dir_all(project.join(".git")).unwrap();
        let nested = project.join("packages/foo");
        fs::create_dir_all(nested.join(".pi/skills")).unwrap();

        let session_dir = home.join(PI_TRANSCRIPT_ROOT).join("--proj--");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("a.jsonl"),
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "session",
                    "version": 3,
                    "id": "0192",
                    "timestamp": "2026-09-16T10:00:00.000Z",
                    "cwd": project,
                }),
            ),
        )
        .unwrap();

        let found = discover_skill_projects(home);
        assert!(
            found.contains(&nested),
            "expected the nested .pi/skills root {nested:?} among {found:?}: a one-level \
             reader would skip it since only the repo root appears in pi's session history"
        );
    }

    #[test]
    fn pi_nested_walk_stops_at_the_directory_budget_or_names_the_unbounded_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("proj");
        for i in 0..20 {
            fs::create_dir_all(project.join(format!("child-{i}/.pi/skills"))).unwrap();
        }

        let budget = 5;
        let found = nested_pi_project_roots_with_budget(home, &project, budget);
        assert!(
            found.len() <= budget,
            "a budget of {budget} directories must bound how many nested roots a 20-way \
             fan-out tree yields, got {}",
            found.len()
        );
    }

    #[test]
    fn pi_nested_walk_budget_bounds_the_last_depth_fan_out_or_names_the_unbounded_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let deepest = home.join("a/b/c/d/e/f");
        for i in 0..20 {
            fs::create_dir_all(deepest.join(format!("child-{i}/.pi/skills"))).unwrap();
        }

        let budget = 8;
        let found = nested_pi_project_roots_with_budget(home, &home.join("a"), budget);
        assert!(
            found.len() <= budget,
            "a budget of {budget} directory entries must bound a 20-way fan-out at the last \
             allowed depth, got {}",
            found.len()
        );
    }

    #[test]
    fn pi_nested_walk_refuses_a_home_or_root_cwd_or_names_the_walked_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join("nested/.pi/skills")).unwrap();

        assert!(
            nested_pi_project_roots(home, home).is_empty(),
            "a home cwd must not be walked even though it holds a nested .pi/skills folder"
        );
        assert!(
            nested_pi_project_roots(home, Path::new("/")).is_empty(),
            "a filesystem root cwd must not be walked"
        );
    }

    #[test]
    fn pi_nested_walk_skips_node_modules_and_target_or_names_the_descended_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("proj");
        fs::create_dir_all(project.join("node_modules/x/.pi/skills")).unwrap();
        fs::create_dir_all(project.join("packages/x/.pi/skills")).unwrap();

        let found = nested_pi_project_roots(home, &project);
        assert!(
            !found
                .iter()
                .any(|p| p.starts_with(project.join("node_modules"))),
            "node_modules must never be descended into, even when it holds a .pi/skills \
             folder: found {found:?}"
        );
        assert!(
            found.contains(&project.join("packages/x")),
            "a normal nested folder must still be discovered: found {found:?}"
        );
    }

    fn write_cursor_workspace(home: &Path, hash: &str, workspace_json: &str) {
        let dir = home.join(CURSOR_WORKSPACE_STORAGE_ROOTS[0]).join(hash);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("workspace.json"), workspace_json).unwrap();
    }

    #[test]
    fn cursor_workspace_folder_uri_is_decoded_and_discovered() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("cursor project");
        fs::create_dir_all(project.join(".cursor/skills")).unwrap();

        let uri = url::Url::from_file_path(&project).unwrap();
        assert!(uri.as_str().contains("%20"));
        write_cursor_workspace(
            home,
            "0a1b2c",
            &serde_json::json!({ "folder": uri.as_str() }).to_string(),
        );

        assert_eq!(discover_skill_projects(home), vec![project]);
    }

    /// Cursor records the home itself when a window is opened on it.
    #[test]
    fn cursor_workspace_at_the_home_directory_discovers_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join(".cursor/skills")).unwrap();

        let uri = url::Url::from_file_path(home).unwrap();
        write_cursor_workspace(
            home,
            "home",
            &serde_json::json!({ "folder": uri.as_str() }).to_string(),
        );

        assert!(discover_skill_projects(home).is_empty());
    }

    #[test]
    fn cursor_workspaces_without_a_local_folder_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("multi-root");
        fs::create_dir_all(project.join(".cursor/skills")).unwrap();
        let workspace_file =
            url::Url::from_file_path(project.join("multi.code-workspace")).unwrap();

        write_cursor_workspace(
            home,
            "remote",
            r#"{"folder":"vscode-remote://ssh-remote%2Bbox/home/me/app"}"#,
        );
        write_cursor_workspace(
            home,
            "multi",
            &serde_json::json!({ "workspace": workspace_file.as_str() }).to_string(),
        );
        write_cursor_workspace(home, "broken", "{not json");

        assert!(cursor_workspace_folders(home).is_empty());
    }

    /// A WAL-mode OpenCode-shaped database that never checkpoints on its own,
    /// so rows stay in the `-wal` file while the connection is open. Dropping
    /// the connection checkpoints and removes the `-wal` and `-shm` files.
    fn opencode_db(path: &Path, worktrees: &[&Path]) -> Connection {
        let conn = Connection::open(path).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        conn.execute_batch(
            "PRAGMA wal_autocheckpoint=0;
             CREATE TABLE project (id TEXT PRIMARY KEY, worktree TEXT NOT NULL);",
        )
        .unwrap();
        for worktree in worktrees {
            insert_opencode_project(&conn, worktree);
        }
        conn
    }

    fn insert_opencode_project(conn: &Connection, worktree: &Path) {
        conn.execute(
            "INSERT INTO project (id, worktree) VALUES (?1, ?2)",
            (worktree.to_str().unwrap(), worktree.to_str().unwrap()),
        )
        .unwrap();
    }

    fn opencode_root(home: &Path) -> PathBuf {
        let root = home.join(OPENCODE_DATA_ROOT);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn opencode_project(home: &Path, name: &str) -> PathBuf {
        let project = home.join(name);
        fs::create_dir_all(project.join(".opencode/skills")).unwrap();
        project
    }

    fn write_legacy_opencode_project(root: &Path, id: &str, worktree: &Path) {
        let dir = root.join("storage/project");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{id}.json")),
            serde_json::json!({ "id": id, "worktree": worktree, "vcs": "git" }).to_string(),
        )
        .unwrap();
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn size_and_mtime(path: &Path) -> (u64, SystemTime) {
        let metadata = fs::metadata(path).unwrap();
        (metadata.len(), metadata.modified().unwrap())
    }

    #[test]
    fn opencode_rows_from_every_channel_database_are_discovered() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let root = opencode_root(home);
        let stable = opencode_project(home, "stable");
        let next = opencode_project(home, "next");

        // OpenCode records sessions outside any project under worktree "/".
        drop(opencode_db(
            &root.join("opencode.db"),
            &[&stable, Path::new("/")],
        ));
        drop(opencode_db(&root.join("opencode-next.db"), &[&next]));

        assert_eq!(discover_skill_projects(home), vec![next, stable]);
    }

    #[test]
    fn legacy_opencode_project_json_is_discovered_without_a_database() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let root = opencode_root(home);
        let project = opencode_project(home, "legacy");
        write_legacy_opencode_project(&root, "abc123", &project);
        write_legacy_opencode_project(&root, "global", Path::new("relative/path"));

        assert_eq!(discover_skill_projects(home), vec![project]);
    }

    #[test]
    fn opencode_project_in_database_and_legacy_json_is_reported_once() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let root = opencode_root(home);
        let project = opencode_project(home, "both");
        drop(opencode_db(&root.join("opencode.db"), &[&project]));
        write_legacy_opencode_project(&root, "abc123", &project);

        assert_eq!(opencode_worktrees(home).len(), 2);
        assert_eq!(discover_skill_projects(home), vec![project]);
    }

    #[test]
    fn unreadable_and_foreign_databases_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let root = opencode_root(home);
        let project = opencode_project(home, "local-build");
        let foreign = opencode_project(home, "foreign");

        fs::write(root.join("opencode.db"), b"").unwrap();
        fs::write(root.join("opencode-garbage.db"), b"this is not a database").unwrap();
        Connection::open(root.join("opencode-old.db"))
            .unwrap()
            .execute_batch("CREATE TABLE project (id TEXT PRIMARY KEY)")
            .unwrap();
        drop(opencode_db(&root.join("opencode-local.db"), &[&project]));
        drop(opencode_db(&root.join("other.db"), &[&foreign]));

        assert_eq!(opencode_worktrees(home), vec![project]);
    }

    #[test]
    fn closed_opencode_database_is_read_without_creating_or_changing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let root = opencode_root(home);
        let project = opencode_project(home, "closed");
        let database = root.join("opencode.db");
        drop(opencode_db(&database, &[&project]));
        let names_before = file_names(&root);
        assert_eq!(names_before, ["opencode.db"]);
        let database_before = size_and_mtime(&database);

        assert_eq!(opencode_worktrees(home), vec![project]);

        assert_eq!(file_names(&root), names_before);
        assert_eq!(size_and_mtime(&database), database_before);
    }

    #[test]
    fn live_opencode_database_rows_in_the_wal_are_read_without_changing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let root = opencode_root(home);
        let first = opencode_project(home, "first");
        let second = opencode_project(home, "second");
        let database = root.join("opencode.db");
        let wal = root.join("opencode.db-wal");
        let writer = opencode_db(&database, &[&first]);
        let names_before = file_names(&root);
        assert_eq!(
            names_before,
            ["opencode.db", "opencode.db-shm", "opencode.db-wal"]
        );
        let database_before = size_and_mtime(&database);
        let wal_before = size_and_mtime(&wal);

        assert_eq!(opencode_worktrees(home), vec![first.clone()]);

        assert_eq!(file_names(&root), names_before);
        assert_eq!(size_and_mtime(&database), database_before);
        assert_eq!(size_and_mtime(&wal), wal_before);
        insert_opencode_project(&writer, &second);
        assert_eq!(opencode_worktrees(home), vec![first, second]);
    }

    /// The state `OpenCode` leaves after a crash: WAL files on disk and no
    /// connection open, so the discovery connection is the last one to close.
    #[test]
    fn leftover_opencode_wal_is_read_without_changing_the_database_or_wal() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let root = opencode_root(home);
        let project = opencode_project(home, "crashed");
        let scratch = tmp.path().join("writer");
        fs::create_dir_all(&scratch).unwrap();
        let writer = opencode_db(&scratch.join("opencode.db"), &[&project]);
        for name in ["opencode.db", "opencode.db-wal", "opencode.db-shm"] {
            fs::copy(scratch.join(name), root.join(name)).unwrap();
        }
        drop(writer);
        let database = root.join("opencode.db");
        let wal = root.join("opencode.db-wal");
        let names_before = file_names(&root);
        let database_before = size_and_mtime(&database);
        let wal_before = size_and_mtime(&wal);

        assert_eq!(opencode_worktrees(home), vec![project]);

        assert_eq!(file_names(&root), names_before);
        assert_eq!(size_and_mtime(&database), database_before);
        assert_eq!(size_and_mtime(&wal), wal_before);
    }

    fn grok_sessions_root(home: &Path) -> PathBuf {
        let root = home.join(GROK_SESSIONS_ROOT);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn grok_project(home: &Path, name: &str) -> PathBuf {
        let project = home.join(name);
        fs::create_dir_all(project.join(".grok/skills")).unwrap();
        project
    }

    /// Grok's `urlencoding::encode` leaves only the RFC 3986 unreserved
    /// characters as they are.
    const GROK_ENCODED: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');

    fn grok_dirname(cwd: &Path) -> String {
        percent_encoding::utf8_percent_encode(cwd.to_str().unwrap(), GROK_ENCODED).to_string()
    }

    #[test]
    fn grok_session_folder_name_is_decoded_to_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let root = grok_sessions_root(home);
        let project = grok_project(home, "my project-名前");
        let session = root.join(grok_dirname(&project)).join("019a-session");
        fs::create_dir_all(&session).unwrap();

        assert!(!root.join(grok_dirname(&project)).join(".cwd").exists());
        assert_eq!(discover_skill_projects(home), vec![project]);
    }

    #[test]
    fn long_grok_cwd_is_read_from_the_cwd_file() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let root = grok_sessions_root(home);
        let project = grok_project(home, &["deep"; 60].join("/"));
        let dir = root.join("deep-0123456789abcdef");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".cwd"), format!("{}\n", project.display())).unwrap();

        assert!(grok_dirname(&project).len() > 255);
        assert_eq!(discover_skill_projects(home), vec![project]);
    }

    #[test]
    fn malformed_grok_session_folders_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let root = grok_sessions_root(home);
        let project = grok_project(home, "kept");
        fs::create_dir_all(root.join(grok_dirname(&project))).unwrap();
        // Invalid UTF-8 once decoded, a relative path, a slug with no `.cwd`
        // file, and a plain file named like an encoded project.
        fs::create_dir_all(root.join("%FF%FE")).unwrap();
        fs::create_dir_all(root.join("relative%2Fpath")).unwrap();
        fs::create_dir_all(root.join("orphan-0123456789abcdef")).unwrap();
        let file_project = grok_project(home, "file");
        fs::write(root.join(grok_dirname(&file_project)), "").unwrap();

        assert_eq!(grok_session_cwds(home), vec![project.clone()]);
        assert_eq!(discover_skill_projects(home), vec![project]);
    }

    #[test]
    fn missing_grok_home_yields_no_grok_projects() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(grok_session_cwds(tmp.path()).is_empty());
        fs::create_dir_all(tmp.path().join(".grok")).unwrap();
        assert!(grok_session_cwds(tmp.path()).is_empty());
    }

    /// A home where Codex nominates `codex-only` and `shared`, and Claude
    /// Code nominates `claude-only` and `shared`.
    fn two_harness_home(home: &Path) -> [PathBuf; 3] {
        let [codex_only, claude_only, shared] =
            ["codex-only", "claude-only", "shared"].map(|name| home.join(name));
        for project in [&codex_only, &claude_only, &shared] {
            fs::create_dir_all(project.join(".agents/skills")).unwrap();
        }
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::write(
            home.join(".codex/config.toml"),
            format!(
                "[projects.\"{}\"]\n[projects.\"{}\"]\n",
                codex_only.display(),
                shared.display()
            ),
        )
        .unwrap();
        for (session, project) in [("a", &claude_only), ("b", &shared)] {
            let dir = home
                .join(CLAUDE_TRANSCRIPT_ROOT)
                .join(format!("-{session}"));
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("session.jsonl"),
                format!(r#"{{"cwd":"{}"}}"#, project.display()),
            )
            .unwrap();
        }
        [codex_only, claude_only, shared]
    }

    fn write_discovery_switches(home: &Path, switches: &serde_json::Value) {
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(
            home.join(".agents/skill-studio.json"),
            serde_json::json!({ "discovery": switches }).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn every_harness_is_read_without_a_settings_file() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let [codex_only, claude_only, shared] = two_harness_home(home);

        assert_eq!(
            discover_skill_projects(home),
            vec![claude_only, codex_only, shared]
        );
    }

    #[test]
    fn a_switched_off_harness_drops_only_the_folders_it_alone_nominates() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let [codex_only, claude_only, shared] = two_harness_home(home);

        write_discovery_switches(home, &serde_json::json!({ "codex": false }));
        assert_eq!(
            discover_skill_projects(home),
            vec![claude_only.clone(), shared.clone()]
        );

        write_discovery_switches(home, &serde_json::json!({ "claude-code": false }));
        assert_eq!(discover_skill_projects(home), vec![codex_only, shared]);

        write_discovery_switches(
            home,
            &serde_json::json!({ "claude-code": false, "codex": false }),
        );
        assert!(discover_skill_projects(home).is_empty());
    }

    #[test]
    fn unknown_discovery_keys_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let [codex_only, claude_only, shared] = two_harness_home(home);
        write_discovery_switches(
            home,
            &serde_json::json!({ "future-harness": false, "codex": true }),
        );

        assert_eq!(
            discover_skill_projects(home),
            vec![claude_only, codex_only, shared]
        );
    }

    #[test]
    fn every_history_source_has_a_distinct_switch() {
        let harnesses: BTreeSet<&str> = discovery_harnesses().collect();
        assert_eq!(harnesses.len(), HISTORY_SOURCES.len());
        let mut off = DiscoverySources::default();
        for harness in discovery_harnesses() {
            off.set(harness, false);
        }
        let tmp = tempfile::tempdir().unwrap();
        two_harness_home(tmp.path());
        assert!(discover_skill_projects_from(tmp.path(), &off).is_empty());
    }

    #[test]
    fn empty_home_yields_no_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let discovery = HostProjectDiscovery::new();
        assert!(discovery.discover_projects(tmp.path()).unwrap().is_empty());
    }
}
