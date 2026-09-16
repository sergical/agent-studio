//! [`ProjectDiscovery`] over the project history each harness keeps under the
//! home directory.
//!
//! The union of Codex's `~/.codex/config.toml` recent projects, the working
//! directories in Claude Code and pi session transcripts, and the folders in
//! Cursor's workspace storage, filtered to directories that hold a skill dir
//! for one of the first-class agents.

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

/// Project paths recorded in Codex's `[projects."/abs/path"]` config
/// sections (`~/.codex/config.toml`).
fn codex_project_paths(home: &Path) -> Vec<PathBuf> {
    let Ok(content) = fs::read_to_string(home.join(".codex/config.toml")) else {
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

/// Cursor inherits VS Code's per-workspace storage: one
/// `<hash>/workspace.json` per opened folder. Cursor does not document the
/// location, so every OS's home-relative VS Code path is tried (macOS, Linux,
/// Windows).
const CURSOR_WORKSPACE_STORAGE_ROOTS: &[&str] = &[
    "Library/Application Support/Cursor/User/workspaceStorage",
    ".config/Cursor/User/workspaceStorage",
    "AppData/Roaming/Cursor/User/workspaceStorage",
];

/// A real `workspace.json` is about 100 bytes.
const MAX_WORKSPACE_JSON_BYTES: u64 = 64 * 1024;

/// `workspace.json` files one discovery run may try to open.
const MAX_CURSOR_WORKSPACES: usize = 10_000;

/// Local folders Cursor has opened. Multi-root workspaces (`workspace`) and
/// remote folders (`vscode-remote://`) name no local project root and are
/// skipped.
fn cursor_workspace_folders(home: &Path) -> Vec<PathBuf> {
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
    // Opening a FIFO would block the refresh, so only regular files are read.
    if !fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file()) {
        return None;
    }
    let mut content = String::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_WORKSPACE_JSON_BYTES)
        .read_to_string(&mut content)
        .ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    let folder = value.get("folder")?.as_str()?;
    url::Url::parse(folder).ok()?.to_file_path().ok()
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

/// Union of every project directory discoverable from Codex config, Claude
/// Code and pi transcripts, and Cursor workspace storage, filtered to
/// directories that exist and have at least one first-class agent's skill
/// dir. Sorted and deduped.
pub fn discover_skill_projects(home: &Path) -> Vec<PathBuf> {
    let mut paths: BTreeSet<PathBuf> = BTreeSet::new();
    paths.extend(codex_project_paths(home));
    paths.extend(transcript_cwds(&home.join(CLAUDE_TRANSCRIPT_ROOT)));
    paths.extend(transcript_cwds(&home.join(PI_TRANSCRIPT_ROOT)));
    paths.extend(cursor_workspace_folders(home));

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
    /// harness histories under the given home.
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

    #[test]
    fn empty_home_yields_no_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let discovery = HostProjectDiscovery::new();
        assert!(discovery.discover_projects(tmp.path()).unwrap().is_empty());
    }
}
