//! Claude Code transcript parsing: `~/.claude/projects/<project>/*.jsonl`
//! and `~/.claude/projects/<project>/<session>/subagents/*.jsonl`. Unlike
//! Codex, a Claude Code line repeats its `cwd` and `sessionId` on every
//! record, so no [`TranscriptContext`] is threaded across lines - each line
//! is self-contained (see `docs/agent-skill-conventions.md` "Skill uses" for
//! the record shapes).

use chrono::{DateTime, Utc};
use serde_json;

use crate::skill_uses::{skill_name_from_skill_md_path, skill_names_read_by_shell};
use crate::skill_uses::{SkillInvocation, SkillTrigger};

/// Fast-path substrings a line must contain before it's worth a full JSON
/// parse: a `Skill` tool_use, a typed command block, or a `SKILL.md` path
/// (a `Read` or `Bash` file read).
const SKILL_TOOL_MARKER: &str = "\"name\":\"Skill\"";
const COMMAND_MARKER: &str = "<command-name>";
const SKILL_MD_MARKER: &str = "SKILL.md";

/// Parses one Claude Code transcript's text (newline-delimited JSON) into
/// skill uses: an `Agent` use per `Skill` tool_use block, a `User` use per
/// typed slash command line, and a `FileRead` use per `Read` or `Bash`
/// tool_use block that reads a skill's `SKILL.md` directly. Never panics: a
/// malformed line, a missing timestamp, or an unrecognized shape is skipped
/// rather than failing the whole file.
pub fn parse_claude_code_uses(text: &str) -> Vec<SkillInvocation> {
    let mut out = Vec::new();

    for line in text.lines() {
        if !line.contains(SKILL_TOOL_MARKER)
            && !line.contains(COMMAND_MARKER)
            && !line.contains(SKILL_MD_MARKER)
        {
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
        let session = record
            .get("sessionId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let record_type = record.get("type").and_then(|v| v.as_str());
        let mut push = |skill: &str, trigger: SkillTrigger| {
            out.push(SkillInvocation {
                skill: skill.to_string(),
                harness: crate::identity::AgentId::CLAUDE_CODE.to_string(),
                trigger,
                at,
                project_path: project_path.clone(),
                session: session.clone(),
            });
        };

        if record_type == Some("assistant") {
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
                let tool_name = block.get("name").and_then(|v| v.as_str());
                let input = block.get("input");
                match tool_name {
                    Some("Skill") => {
                        let Some(skill) =
                            input.and_then(|i| i.get("skill")).and_then(|v| v.as_str())
                        else {
                            continue;
                        };
                        push(skill, SkillTrigger::Agent);
                    }
                    Some("Read") => {
                        let Some(file_path) = input
                            .and_then(|i| i.get("file_path"))
                            .and_then(|v| v.as_str())
                        else {
                            continue;
                        };
                        let Some(name) = skill_name_from_skill_md_path(file_path) else {
                            continue;
                        };
                        push(name, SkillTrigger::FileRead);
                    }
                    Some("Bash") => {
                        let Some(command) = input
                            .and_then(|i| i.get("command"))
                            .and_then(|v| v.as_str())
                        else {
                            continue;
                        };
                        for name in skill_names_read_by_shell(command) {
                            push(name, SkillTrigger::FileRead);
                        }
                    }
                    _ => {}
                }
            }
        } else if record_type == Some("user") {
            if record.get("isMeta").and_then(|v| v.as_bool()) == Some(true) {
                continue;
            }
            let Some(content) = record
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
            else {
                continue;
            };
            let trimmed = content.trim_start();
            if !trimmed.starts_with("<command-message>") && !trimmed.starts_with("<command-name>") {
                continue;
            }
            let Some(start) = content.find("<command-name>") else {
                continue;
            };
            let after_open = &content[start + "<command-name>".len()..];
            let Some(end) = after_open.find("</command-name>") else {
                continue;
            };
            let name = after_open[..end]
                .strip_prefix('/')
                .unwrap_or(&after_open[..end]);
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            push(name, SkillTrigger::User);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_tool_use_line(skill: &str, timestamp: &str, cwd: &str, session: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{timestamp}","cwd":"{cwd}","sessionId":"{session}","message":{{"content":[{{"type":"tool_use","name":"Skill","input":{{"skill":"{skill}"}}}}]}}}}"#
        )
    }

    fn command_line(command_message: &str, command_name: &str, timestamp: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"{timestamp}","message":{{"content":"<command-message>{command_message}</command-message>\n<command-name>{command_name}</command-name>"}}}}"#
        )
    }

    #[test]
    fn skill_tool_use_gives_one_agent_use() {
        let text = skill_tool_use_line("write-tests", "2026-08-01T12:00:00Z", "/proj", "sess-1");
        let uses = parse_claude_code_uses(&text);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "write-tests");
        assert_eq!(uses[0].trigger, SkillTrigger::Agent);
        assert_eq!(uses[0].harness, crate::identity::AgentId::CLAUDE_CODE);
        assert_eq!(uses[0].project_path.as_deref(), Some("/proj"));
        assert_eq!(uses[0].session.as_deref(), Some("sess-1"));
    }

    #[test]
    fn typed_command_gives_one_user_use() {
        let text = command_line("deploy", "/deploy", "2026-08-01T12:00:00Z");
        let uses = parse_claude_code_uses(&text);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "deploy");
        assert_eq!(uses[0].trigger, SkillTrigger::User);
    }

    #[test]
    fn leading_whitespace_and_command_name_first_also_parse() {
        let padded = r#"{"type":"user","timestamp":"2026-08-01T12:00:00Z","message":{"content":"   <command-message>deploy</command-message>\n<command-name>/deploy</command-name>"}}"#;
        assert_eq!(parse_claude_code_uses(padded).len(), 1);

        let name_first = r#"{"type":"user","timestamp":"2026-08-01T12:00:00Z","message":{"content":"<command-name>/deploy</command-name>"}}"#;
        assert_eq!(parse_claude_code_uses(name_first).len(), 1);
    }

    #[test]
    fn is_meta_command_line_gives_nothing() {
        let text = r#"{"type":"user","timestamp":"2026-08-01T12:00:00Z","isMeta":true,"message":{"content":"<command-name>/deploy</command-name>"}}"#;
        assert!(parse_claude_code_uses(text).is_empty());
    }

    #[test]
    fn array_content_command_line_gives_nothing() {
        let text = r#"{"type":"user","timestamp":"2026-08-01T12:00:00Z","message":{"content":[{"type":"text","text":"<command-name>/deploy</command-name>"}]}}"#;
        assert!(parse_claude_code_uses(text).is_empty());
    }

    #[test]
    fn command_name_mentioned_mid_text_gives_nothing() {
        let text = r#"{"type":"user","timestamp":"2026-08-01T12:00:00Z","message":{"content":"just chatting about <command-name>/deploy</command-name> today"}}"#;
        assert!(parse_claude_code_uses(text).is_empty());
    }

    fn read_tool_use_line(file_path: &str, timestamp: &str, session: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{timestamp}","sessionId":"{session}","message":{{"content":[{{"type":"tool_use","name":"Read","input":{{"file_path":"{file_path}"}}}}]}}}}"#
        )
    }

    fn bash_tool_use_line(command: &str, timestamp: &str, session: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{timestamp}","sessionId":"{session}","message":{{"content":[{{"type":"tool_use","name":"Bash","input":{{"command":"{command}"}}}}]}}}}"#
        )
    }

    #[test]
    fn read_tool_use_under_a_skills_root_gives_a_file_read() {
        let text = read_tool_use_line(
            "/Users/me/.claude/skills/foo/SKILL.md",
            "2026-08-01T12:00:00Z",
            "sess-1",
        );
        let uses = parse_claude_code_uses(&text);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
        assert_eq!(uses[0].session.as_deref(), Some("sess-1"));
    }

    #[test]
    fn read_tool_use_outside_a_skills_root_gives_nothing() {
        let text = read_tool_use_line("/Users/me/notes/SKILL.md", "2026-08-01T12:00:00Z", "s1");
        assert!(parse_claude_code_uses(&text).is_empty());
    }

    #[test]
    fn bash_cat_of_a_skill_md_gives_a_file_read() {
        let text = bash_tool_use_line(
            "cat /Users/me/.claude/skills/foo/SKILL.md",
            "2026-08-01T12:00:00Z",
            "sess-1",
        );
        let uses = parse_claude_code_uses(&text);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
    }

    #[test]
    fn bash_wc_of_a_skill_md_gives_nothing() {
        let text = bash_tool_use_line(
            "wc -l /Users/me/.claude/skills/foo/SKILL.md",
            "2026-08-01T12:00:00Z",
            "s1",
        );
        assert!(parse_claude_code_uses(&text).is_empty());
    }

    #[test]
    fn malformed_json_line_is_skipped() {
        let text = "{\"name\":\"Skill\" this is not valid json";
        assert!(parse_claude_code_uses(text).is_empty());
    }

    #[test]
    fn missing_timestamp_is_skipped() {
        let text = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":"write-tests"}}]}}"#;
        assert!(parse_claude_code_uses(text).is_empty());
    }

    #[test]
    fn non_skill_line_is_skipped() {
        let text = r#"{"type":"assistant","timestamp":"2026-08-01T12:00:00Z","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#;
        assert!(parse_claude_code_uses(text).is_empty());
    }

    #[test]
    fn multiple_lines_and_blocks_are_all_found() {
        let mut text = skill_tool_use_line("write-tests", "2026-08-01T12:00:00Z", "/proj-a", "s1");
        text.push('\n');
        text.push_str(&skill_tool_use_line(
            "lint-code",
            "2026-08-02T12:00:00Z",
            "/proj-b",
            "s2",
        ));
        let uses = parse_claude_code_uses(&text);
        assert_eq!(uses.len(), 2);
    }
}
