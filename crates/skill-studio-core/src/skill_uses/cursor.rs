//! Cursor transcript parsing: `~/.cursor/projects/<project>/agent-
//! transcripts/<session>/<session>.jsonl` and its `subagents/<id>.jsonl`
//! siblings. Unlike Claude Code and Codex, a Cursor line carries neither a
//! timestamp nor a session id - both come from outside the file (see the
//! host adapter and `docs/agent-skill-conventions.md` "Skill uses"). No
//! local transcript holds a skill tool call or a typed skill command, so the
//! only use this file finds is a file read.

use chrono::{DateTime, Utc};

use crate::identity::AgentId;
use crate::skill_uses::{
    push_shell_reads, push_use, skill_name_from_read_path, SkillInvocation, SkillTrigger,
    TranscriptContext,
};

/// Fast-path substring a line must contain before it's worth a full JSON
/// parse.
const SKILL_MD_MARKER: &str = "SKILL.md";

/// Parses one Cursor transcript's text (newline-delimited JSON) into
/// `FileRead` uses: an assistant `Read`/`ReadFile` tool_use that reads a
/// skill's `SKILL.md` directly, or a `Shell` tool_use whose command prints
/// one. A line with no `message` (Cursor's own `{"status":..,"type":..}`
/// lines) is skipped, as is any non-assistant line. `at` is used for every
/// use, since Cursor transcript lines carry no time of their own. Never
/// panics: a malformed line is skipped rather than failing the whole file.
pub fn parse_cursor_uses(
    text: &str,
    context: &TranscriptContext,
    at: DateTime<Utc>,
) -> Vec<SkillInvocation> {
    let mut out = Vec::new();

    for line in text.lines() {
        if !line.contains(SKILL_MD_MARKER) {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if record.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let Some(items) = record
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for item in items {
            if item.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
                continue;
            }
            let name = item.get("name").and_then(|v| v.as_str());
            let Some(input) = item.get("input") else {
                continue;
            };
            match name {
                Some("Read") | Some("ReadFile") => {
                    let Some(path) = input.get("path").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    if let Some(skill) =
                        skill_name_from_read_path(path, context.project_path.as_deref())
                    {
                        push_use(
                            &mut out,
                            context,
                            AgentId::CURSOR,
                            &skill,
                            SkillTrigger::FileRead,
                            at,
                        );
                    }
                }
                Some("Shell") => {
                    let Some(command) = input.get("command").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    push_shell_reads(&mut out, context, AgentId::CURSOR, [command], at);
                }
                _ => {}
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(session: &str, project_path: &str) -> TranscriptContext {
        TranscriptContext {
            session: Some(session.to_string()),
            project_path: Some(project_path.to_string()),
            ..Default::default()
        }
    }

    fn read_line(name: &str, path: &str) -> String {
        format!(
            r#"{{"role":"assistant","message":{{"content":[{{"type":"tool_use","name":"{name}","input":{{"path":"{path}"}}}}]}}}}"#
        )
    }

    fn shell_line(command: &str) -> String {
        format!(
            r#"{{"role":"assistant","message":{{"content":[{{"type":"tool_use","name":"Shell","input":{{"command":"{command}"}}}}]}}}}"#
        )
    }

    fn write_line(path: &str) -> String {
        format!(
            r#"{{"role":"assistant","message":{{"content":[{{"type":"tool_use","name":"Write","input":{{"path":"{path}","content":"x"}}}}]}}}}"#
        )
    }

    #[test]
    fn read_gives_a_file_read_with_the_given_at_session_and_project() {
        let ctx = context("s1", "/proj-a");
        let at = "2026-09-16T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let text = read_line("Read", "/x/skills/foo/SKILL.md");
        let uses = parse_cursor_uses(&text, &ctx, at);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
        assert_eq!(uses[0].harness, crate::identity::AgentId::CURSOR);
        assert_eq!(uses[0].at, at);
        assert_eq!(uses[0].session.as_deref(), Some("s1"));
        assert_eq!(uses[0].project_path.as_deref(), Some("/proj-a"));
    }

    #[test]
    fn read_file_gives_a_file_read() {
        let ctx = context("s1", "/proj-a");
        let at = Utc::now();
        let text = read_line("ReadFile", "/x/skills/foo/SKILL.md");
        let uses = parse_cursor_uses(&text, &ctx, at);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
    }

    #[test]
    fn shell_cat_gives_a_file_read() {
        let ctx = context("s1", "/proj-a");
        let at = Utc::now();
        let text = shell_line("cat /x/skills/foo/SKILL.md");
        let uses = parse_cursor_uses(&text, &ctx, at);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
    }

    #[test]
    fn write_of_a_skill_md_gives_nothing() {
        let ctx = context("s1", "/proj-a");
        let at = Utc::now();
        let text = write_line("/x/skills/foo/SKILL.md");
        assert!(parse_cursor_uses(&text, &ctx, at).is_empty());
    }

    #[test]
    fn a_status_line_gives_nothing() {
        let ctx = context("s1", "/proj-a");
        let at = Utc::now();
        let text = r#"{"status":"completed","type":"tool_result_at_SKILL.md"}"#;
        assert!(parse_cursor_uses(text, &ctx, at).is_empty());
    }

    #[test]
    fn a_user_role_line_gives_nothing() {
        let ctx = context("s1", "/proj-a");
        let at = Utc::now();
        let text = r#"{"role":"user","message":{"content":[{"type":"text","text":"read /x/skills/foo/SKILL.md"}]}}"#;
        assert!(parse_cursor_uses(text, &ctx, at).is_empty());
    }

    #[test]
    fn relative_read_path_joined_with_the_project_gives_a_file_read() {
        let ctx = context("s1", "/proj-a");
        let at = Utc::now();
        let text = read_line("Read", "skills/foo/SKILL.md");
        let uses = parse_cursor_uses(&text, &ctx, at);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
    }

    #[test]
    fn read_outside_a_root_gives_nothing() {
        let ctx = context("s1", "/proj-a");
        let at = Utc::now();
        let text = read_line("Read", "notes/SKILL.md");
        assert!(parse_cursor_uses(&text, &ctx, at).is_empty());
    }
}
