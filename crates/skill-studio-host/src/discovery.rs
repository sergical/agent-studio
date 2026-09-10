//! [`ProjectDiscovery`] over Claude Code session transcripts.
//!
//! Ported from `apps/desktop/src-tauri/src/skills/project_discovery.rs`. The
//! desktop version also unions in Codex's `~/.codex/config.toml` recent
//! projects; that half is left out here because it needs a `toml`
//! dependency this crate does not otherwise carry; add it back at the CLI
//! layer if a caller wants that source too.

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use skill_studio_core::error::CoreError;
use skill_studio_core::ports::ProjectDiscovery;

/// Skill directories (relative to a project root) whose presence marks a
/// directory as a real skills project, not just any directory a session
/// happened to run in.
const SKILL_DIR_MARKERS: &[&str] = &[
    ".claude/skills",
    ".codex/skills",
    ".opencode/skills",
    ".opencode/skill",
    ".pi/skills",
    ".cursor/skills",
    ".grok/skills",
    ".agents/skills",
];

/// Lines examined per transcript file, and the max size of a single line,
/// before giving up on that file and falling back to the next-newest one.
const MAX_TRANSCRIPT_LINES: usize = 200;
const MAX_TRANSCRIPT_LINE_BYTES: usize = 64 * 1024;

/// Total bytes read from a single transcript file before giving up on it and
/// falling back to the next-newest one. Bounds worst-case read work per file
/// independently of `MAX_TRANSCRIPT_LINES`, since a file made of many small
/// lines could otherwise still cost an unbounded amount of I/O.
const MAX_TRANSCRIPT_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Total bytes one discovery run may read across every project. This bounds
/// one refresh even when no transcript carries a recognizable `cwd` (e.g.
/// after a transcript schema change).
const MAX_TRANSCRIPT_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

/// Transcript files one discovery run may try to open. This independently
/// bounds empty and unreadable files, which do not consume the byte budget.
const MAX_TRANSCRIPT_ATTEMPTS: usize = 10_000;

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
            Ok(0) => break, // EOF
            Ok(n) => {
                budget = budget.saturating_sub(n as u64);
                limits.consume_bytes(n as u64);
            }
            Err(_) => break,
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

/// The distinct `cwd` values recorded in Claude Code project transcripts.
/// Each encoded project directory can represent more than one real path, so
/// every `*.jsonl` file is scanned newest-first within the total byte budget.
fn claude_transcript_cwds(home: &Path) -> Vec<PathBuf> {
    claude_transcript_cwds_within(
        home,
        TranscriptScanLimits::new(MAX_TRANSCRIPT_TOTAL_BYTES, MAX_TRANSCRIPT_ATTEMPTS),
    )
}

/// `claude_transcript_cwds` with explicit operation-wide limits. Stops
/// scanning and returns what it found when either limit is spent.
fn claude_transcript_cwds_within(home: &Path, mut limits: TranscriptScanLimits) -> Vec<PathBuf> {
    let mut out = BTreeSet::new();

    let Ok(project_dirs) = fs::read_dir(home.join(".claude/projects")) else {
        return Vec::new();
    };
    let mut project_dirs: Vec<_> = project_dirs.flatten().collect();
    project_dirs.sort_by_key(|entry| entry.path());
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
            // Only regular files: a symlink, FIFO, or directory named
            // `*.jsonl` is never opened as a transcript.
            .filter(|e| fs::symlink_metadata(e.path()).is_ok_and(|m| m.file_type().is_file()))
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

/// Every project directory discoverable from Claude Code transcripts under
/// `home`, filtered to directories that exist and have at least one
/// first-class agent's skill dir. Sorted and deduped.
fn discover_transcript_projects(home: &Path) -> Vec<PathBuf> {
    claude_transcript_cwds(home)
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

/// `ProjectDiscovery` backed by Claude Code session transcripts under the
/// home directory ([`ports::ProjectDiscovery`](ProjectDiscovery)).
pub struct TranscriptProjectDiscovery;

impl TranscriptProjectDiscovery {
    /// Builds a discovery adapter. Holds no state; every call re-reads the
    /// transcripts under the given home.
    pub fn new() -> Self {
        TranscriptProjectDiscovery
    }
}

impl Default for TranscriptProjectDiscovery {
    fn default() -> Self {
        TranscriptProjectDiscovery::new()
    }
}

impl ProjectDiscovery for TranscriptProjectDiscovery {
    fn discover_projects(&self, home_root: &Path) -> Result<Vec<PathBuf>, CoreError> {
        Ok(discover_transcript_projects(home_root))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let discovery = TranscriptProjectDiscovery::new();
        let found = discovery.discover_projects(home).unwrap();
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

        let discovery = TranscriptProjectDiscovery::new();
        let found = discovery.discover_projects(home).unwrap();
        assert!(
            found.is_empty(),
            "the home root was adopted as one of its own projects: {found:?}"
        );
    }

    #[test]
    fn projects_without_a_skill_dir_are_filtered_out() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("no-skills-here");
        fs::create_dir_all(&project).unwrap();

        let transcript_dir = home.join(".claude/projects/-no-skills-here");
        fs::create_dir_all(&transcript_dir).unwrap();
        fs::write(
            transcript_dir.join("session.jsonl"),
            format!(r#"{{"type":"user","cwd":"{}"}}"#, project.to_string_lossy()),
        )
        .unwrap();

        let discovery = TranscriptProjectDiscovery::new();
        assert!(discovery.discover_projects(home).unwrap().is_empty());
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

        let transcript_dir = home.join(".claude/projects/-mixed");
        fs::create_dir_all(&transcript_dir).unwrap();
        let scratch_project = scratch.parent().unwrap().parent().unwrap();
        fs::write(
            transcript_dir.join("a-scratch.jsonl"),
            format!(
                r#"{{"type":"user","cwd":"{}"}}"#,
                scratch_project.to_string_lossy()
            ),
        )
        .unwrap();
        fs::write(
            transcript_dir.join("b-real.jsonl"),
            format!(r#"{{"type":"user","cwd":"{}"}}"#, project.to_string_lossy()),
        )
        .unwrap();

        let discovery = TranscriptProjectDiscovery::new();
        let found = discovery.discover_projects(home).unwrap();
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

        let now = SystemTime::now();
        fs::File::open(&older)
            .unwrap()
            .set_modified(now - std::time::Duration::from_secs(3600))
            .unwrap();
        fs::File::open(&newer).unwrap().set_modified(now).unwrap();

        let discovery = TranscriptProjectDiscovery::new();
        let found = discovery.discover_projects(home).unwrap();
        assert_eq!(found, vec![project]);
    }

    #[test]
    fn empty_home_yields_no_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let discovery = TranscriptProjectDiscovery::new();
        assert!(discovery.discover_projects(tmp.path()).unwrap().is_empty());
    }
}
