// ============================================================================
// Skills Module - Project Discovery
// Finds project directories the user has worked in, so `get_installed_skills`
// can find their project-scoped skills without the caller listing every
// project by hand. Union of Codex's recent-projects config and Claude Code
// transcript working directories, filtered to dirs that actually have a
// skill dir for one of the first-class agents.
// ============================================================================

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Skill directories (relative to a project root) whose presence marks a
/// directory as a real skills project, not just any directory a session
/// happened to run in.
const SKILL_DIR_MARKERS: &[&str] = &[
    ".claude/skills",
    ".codex/skills",
    ".opencode/skills",
    ".opencode/skill",
    ".pi/skills",
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

/// Newest transcript files tried per project dir before giving up on that
/// project, and the total bytes one discovery run may read across every
/// project. Together they bound one refresh even when no transcript carries
/// a recognizable `cwd` (e.g. after a transcript schema change).
const MAX_TRANSCRIPTS_PER_PROJECT: usize = 5;
const MAX_TRANSCRIPT_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

/// The `cwd` recorded in a single transcript file: the first line (of up to
/// `MAX_TRANSCRIPT_LINES`, each capped at `MAX_TRANSCRIPT_LINE_BYTES`, within
/// a `MAX_TRANSCRIPT_FILE_BYTES` total budget) that mentions `"cwd"` and
/// parses as JSON with an absolute-path `cwd` string. A line that overruns
/// the per-line cap abandons the whole file rather than draining and
/// continuing, so a single pathological line can't be used to keep reading
/// past the file's budget one bounded chunk at a time.
fn cwd_from_transcript(path: &Path, total_budget: &mut u64) -> Option<PathBuf> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    // Reused across iterations and cleared each time, so memory use is
    // bounded by one line's worth of bytes rather than growing with the
    // number of lines scanned.
    let mut buf: Vec<u8> = Vec::new();
    let mut budget = MAX_TRANSCRIPT_FILE_BYTES.min(*total_budget);
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
                *total_budget = total_budget.saturating_sub(n as u64);
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

/// The `cwd` recorded in each Claude Code project's transcripts: for each
/// dir in `~/.claude/projects/*`, scan its `*.jsonl` files newest-first and
/// use the first one with a valid `cwd`.
fn claude_transcript_cwds(home: &Path) -> Vec<PathBuf> {
    claude_transcript_cwds_within(home, MAX_TRANSCRIPT_TOTAL_BYTES)
}

/// `claude_transcript_cwds` with an explicit operation-wide byte budget;
/// stops scanning (returning what it found so far) once the budget is spent.
fn claude_transcript_cwds_within(home: &Path, mut total_budget: u64) -> Vec<PathBuf> {
    let mut out = Vec::new();

    let Ok(project_dirs) = fs::read_dir(home.join(".claude/projects")) else {
        return out;
    };
    for project_dir in project_dirs.flatten() {
        if total_budget == 0 {
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
        transcripts.sort_by_key(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH)
        });
        transcripts.reverse(); // newest first
        transcripts.truncate(MAX_TRANSCRIPTS_PER_PROJECT);

        if let Some(cwd) = transcripts
            .iter()
            .find_map(|e| cwd_from_transcript(&e.path(), &mut total_budget))
        {
            out.push(cwd);
        }
    }
    out
}

/// Union of every project directory discoverable from Codex config and
/// Claude Code transcripts, filtered to directories that exist and have at
/// least one first-class agent's skill dir. Sorted and deduped.
pub fn discover_skill_projects(home: &Path) -> Vec<PathBuf> {
    let mut paths: BTreeSet<PathBuf> = BTreeSet::new();
    paths.extend(codex_project_paths(home));
    paths.extend(claude_transcript_cwds(home));

    paths
        .into_iter()
        .filter(|p| {
            p.exists()
                && SKILL_DIR_MARKERS
                    .iter()
                    .any(|marker| p.join(marker).exists())
        })
        .collect()
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

        let unbounded = claude_transcript_cwds_within(home, u64::MAX);
        assert_eq!(unbounded.len(), 10);

        let bounded = claude_transcript_cwds_within(home, 2_500);
        assert!(
            bounded.len() <= 3,
            "budget should stop the scan early: {bounded:?}"
        );
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
}
