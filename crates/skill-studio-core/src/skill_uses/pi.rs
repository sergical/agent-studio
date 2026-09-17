//! pi transcript parsing: `~/.pi/agent/sessions/**/*.jsonl`. Like Codex, a pi
//! line doesn't repeat its session id or project path - those are stated
//! once, in the file's first `session` header line - so parsing threads a
//! [`TranscriptContext`] through the whole file (see `docs/agent-skill-
//! conventions.md` "Skill uses" for the record shapes). pi has no skill
//! tool: every use is either the user's typed `<skill>` block or a file read.

use chrono::{DateTime, Utc};

use crate::identity::AgentId;
use crate::skill_uses::{
    push_shell_reads, push_use, skill_name_from_read_path, SkillInvocation, SkillTrigger,
    TranscriptContext,
};

/// Fast-path substrings a line must contain before it's worth a full JSON
/// parse.
const SESSION_MARKER: &str = "\"type\":\"session\"";
const SKILL_BLOCK_MARKER: &str = "<skill name=";
const SKILL_MD_MARKER: &str = "SKILL.md";

/// The name in a user `<skill name="X" location="P">...` block, or `None`
/// when `text` doesn't open with that exact shape (a literal `/skill:X`
/// command, which pi did not load, must not match).
fn parse_user_skill_block(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("<skill name=\"")?;
    let end = rest.find('"')?;
    let name = &rest[..end];
    if name.is_empty() || name.contains('\n') {
        return None;
    }
    rest[end + 1..].starts_with(" location=\"").then_some(name)
}

/// The text items in a user message's `content`: each item's `text` when
/// `content` is an array of `{"type":"text","text":..}` items, or `content`
/// itself, treated as one text item, when it's a plain string.
fn content_texts(content: &serde_json::Value) -> Vec<&str> {
    if let Some(text) = content.as_str() {
        return vec![text];
    }
    let Some(items) = content.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|item| item.get("type").and_then(|v| v.as_str()) == Some("text"))
        .filter_map(|item| item.get("text").and_then(|v| v.as_str()))
        .collect()
}

/// A tool call's `arguments`, parsed: the value as-is, or (when it's a JSON
/// string) that string parsed as JSON.
fn tool_call_arguments(value: &serde_json::Value) -> Option<serde_json::Value> {
    match value.as_str() {
        Some(raw) => serde_json::from_str(raw).ok(),
        None => Some(value.clone()),
    }
}

/// Parses one pi session transcript's text (newline-delimited JSON) into
/// skill uses: a `User` use per user `<skill name="X" location="P">` block,
/// and a `FileRead` use per `read` or `bash` assistant tool call that reads a
/// skill's `SKILL.md`. `context` carries the session id and project path
/// across calls (see the module doc); a `session` header only sets it the
/// first time it's seen. Never panics: a malformed line, or one missing a
/// `timestamp`, is skipped rather than failing the whole file.
pub fn parse_pi_uses(text: &str, context: &mut TranscriptContext) -> Vec<SkillInvocation> {
    let mut out = Vec::new();

    for line in text.lines() {
        if !line.contains(SESSION_MARKER)
            && !line.contains(SKILL_BLOCK_MARKER)
            && !line.contains(SKILL_MD_MARKER)
        {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let record_type = record.get("type").and_then(|v| v.as_str());

        if record_type == Some("session") {
            if context.session.is_none() {
                if let Some(id) = record.get("id").and_then(|v| v.as_str()) {
                    if !id.is_empty() {
                        context.session = Some(id.to_string());
                    }
                }
                if let Some(cwd) = record.get("cwd").and_then(|v| v.as_str()) {
                    if !cwd.is_empty() {
                        context.project_path = Some(cwd.to_string());
                    }
                }
            }
            continue;
        }
        if record_type != Some("message") {
            continue;
        }

        let Some(timestamp) = record.get("timestamp").and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(at) = DateTime::parse_from_rfc3339(timestamp) else {
            continue;
        };
        let at = at.with_timezone(&Utc);

        let Some(message) = record.get("message") else {
            continue;
        };
        let role = message.get("role").and_then(|v| v.as_str());
        let Some(content) = message.get("content") else {
            continue;
        };

        match role {
            Some("user") => {
                for item in content_texts(content) {
                    if let Some(name) = parse_user_skill_block(item) {
                        push_use(&mut out, context, AgentId::PI, name, SkillTrigger::User, at);
                    }
                }
            }
            Some("assistant") => {
                let Some(items) = content.as_array() else {
                    continue;
                };
                for item in items {
                    if item.get("type").and_then(|v| v.as_str()) != Some("toolCall") {
                        continue;
                    }
                    let name = item.get("name").and_then(|v| v.as_str());
                    let Some(arguments) = item.get("arguments") else {
                        continue;
                    };
                    let Some(arguments) = tool_call_arguments(arguments) else {
                        continue;
                    };
                    match name {
                        Some("read") => {
                            let Some(path) = arguments.get("path").and_then(|v| v.as_str()) else {
                                continue;
                            };
                            if let Some(skill) =
                                skill_name_from_read_path(path, context.project_path.as_deref())
                            {
                                push_use(
                                    &mut out,
                                    context,
                                    AgentId::PI,
                                    &skill,
                                    SkillTrigger::FileRead,
                                    at,
                                );
                            }
                        }
                        Some("bash") => {
                            let Some(command) = arguments.get("command").and_then(|v| v.as_str())
                            else {
                                continue;
                            };
                            push_shell_reads(&mut out, context, AgentId::PI, [command], at);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_line(id: &str, cwd: &str) -> String {
        format!(
            r#"{{"type":"session","id":"{id}","cwd":"{cwd}","timestamp":"2026-09-16T11:00:00Z","version":"1.0.0"}}"#
        )
    }

    fn user_array_line(timestamp: &str, name: &str, location: &str) -> String {
        format!(
            r#"{{"type":"message","id":"m1","parentId":null,"timestamp":"{timestamp}","message":{{"role":"user","content":[{{"type":"text","text":"<skill name=\"{name}\" location=\"{location}\">"}}]}}}}"#
        )
    }

    fn user_string_line(timestamp: &str, name: &str, location: &str) -> String {
        format!(
            r#"{{"type":"message","id":"m1","parentId":null,"timestamp":"{timestamp}","message":{{"role":"user","content":"<skill name=\"{name}\" location=\"{location}\">"}}}}"#
        )
    }

    fn read_tool_call_line(timestamp: &str, path: &str) -> String {
        format!(
            r#"{{"type":"message","id":"m2","parentId":"m1","timestamp":"{timestamp}","message":{{"role":"assistant","content":[{{"type":"toolCall","name":"read","arguments":{{"path":"{path}"}}}}]}}}}"#
        )
    }

    fn bash_tool_call_line(timestamp: &str, command: &str) -> String {
        format!(
            r#"{{"type":"message","id":"m2","parentId":"m1","timestamp":"{timestamp}","message":{{"role":"assistant","content":[{{"type":"toolCall","name":"bash","arguments":{{"command":"{command}"}}}}]}}}}"#
        )
    }

    #[test]
    fn header_sets_context_and_a_second_one_does_not_change_it() {
        let text = format!(
            "{}\n{}\n",
            header_line("s1", "/proj-a"),
            header_line("s2", "/proj-b")
        );
        let mut context = TranscriptContext::default();
        assert!(parse_pi_uses(&text, &mut context).is_empty());
        assert_eq!(context.session.as_deref(), Some("s1"));
        assert_eq!(context.project_path.as_deref(), Some("/proj-a"));
    }

    #[test]
    fn user_skill_block_array_content_gives_a_user_use() {
        let text = format!(
            "{}\n{}\n",
            header_line("s1", "/proj-a"),
            user_array_line(
                "2026-09-16T12:00:00Z",
                "foo",
                "/x/.pi/agent/skills/foo/SKILL.md"
            ),
        );
        let mut context = TranscriptContext::default();
        let uses = parse_pi_uses(&text, &mut context);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::User);
        assert_eq!(uses[0].harness, crate::identity::AgentId::PI);
        assert_eq!(uses[0].session.as_deref(), Some("s1"));
        assert_eq!(uses[0].project_path.as_deref(), Some("/proj-a"));
    }

    #[test]
    fn user_skill_block_string_content_gives_a_user_use() {
        let text = user_string_line(
            "2026-09-16T12:00:00Z",
            "foo",
            "/x/.pi/agent/skills/foo/SKILL.md",
        );
        let mut context = TranscriptContext::default();
        let uses = parse_pi_uses(&text, &mut context);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::User);
    }

    #[test]
    fn literal_typed_skill_command_gives_nothing() {
        let text = r#"{"type":"message","id":"m1","parentId":null,"timestamp":"2026-09-16T12:00:00Z","message":{"role":"user","content":[{"type":"text","text":"/skill:foo"}]}}"#;
        let mut context = TranscriptContext::default();
        assert!(parse_pi_uses(text, &mut context).is_empty());
    }

    #[test]
    fn assistant_text_starting_with_the_skill_block_gives_nothing() {
        let text = r#"{"type":"message","id":"m1","parentId":null,"timestamp":"2026-09-16T12:00:00Z","message":{"role":"assistant","content":[{"type":"text","text":"<skill name=\"foo\" location=\"/x/skills/foo/SKILL.md\">"}]}}"#;
        let mut context = TranscriptContext::default();
        assert!(parse_pi_uses(text, &mut context).is_empty());
    }

    #[test]
    fn read_absolute_gives_a_file_read() {
        let text = format!(
            "{}\n{}\n",
            header_line("s1", "/proj"),
            read_tool_call_line("2026-09-16T12:00:00Z", "/x/skills/foo/SKILL.md"),
        );
        let mut context = TranscriptContext::default();
        let uses = parse_pi_uses(&text, &mut context);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
    }

    #[test]
    fn read_relative_joined_with_cwd_gives_a_file_read() {
        let text = format!(
            "{}\n{}\n",
            header_line("s1", "/proj"),
            read_tool_call_line("2026-09-16T12:00:00Z", "skills/foo/SKILL.md"),
        );
        let mut context = TranscriptContext::default();
        let uses = parse_pi_uses(&text, &mut context);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
    }

    #[test]
    fn read_outside_a_root_gives_nothing() {
        let text = format!(
            "{}\n{}\n",
            header_line("s1", "/proj"),
            read_tool_call_line("2026-09-16T12:00:00Z", "notes/SKILL.md"),
        );
        let mut context = TranscriptContext::default();
        assert!(parse_pi_uses(&text, &mut context).is_empty());
    }

    #[test]
    fn bash_head_of_a_skill_md_gives_a_file_read() {
        let text = bash_tool_call_line("2026-09-16T12:00:00Z", "head -n 5 /x/skills/foo/SKILL.md");
        let mut context = TranscriptContext::default();
        let uses = parse_pi_uses(&text, &mut context);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
    }

    #[test]
    fn bash_ls_of_a_skill_md_gives_nothing() {
        let text = bash_tool_call_line("2026-09-16T12:00:00Z", "ls /x/skills/foo/SKILL.md");
        let mut context = TranscriptContext::default();
        assert!(parse_pi_uses(&text, &mut context).is_empty());
    }

    #[test]
    fn arguments_as_a_json_string_gives_a_file_read() {
        let text = r#"{"type":"message","id":"m2","parentId":"m1","timestamp":"2026-09-16T12:00:00Z","message":{"role":"assistant","content":[{"type":"toolCall","name":"read","arguments":"{\"path\": \"/x/skills/foo/SKILL.md\"}"}]}}"#;
        let mut context = TranscriptContext::default();
        let uses = parse_pi_uses(text, &mut context);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
    }

    #[test]
    fn no_timestamp_gives_nothing() {
        let text = r#"{"type":"message","id":"m1","parentId":null,"message":{"role":"user","content":[{"type":"text","text":"<skill name=\"foo\" location=\"/x/skills/foo/SKILL.md\">"}]}}"#;
        let mut context = TranscriptContext::default();
        assert!(parse_pi_uses(text, &mut context).is_empty());
    }

    #[test]
    fn bad_json_line_is_skipped() {
        let mut context = TranscriptContext::default();
        assert!(parse_pi_uses("{\"type\":\"session\" not json", &mut context).is_empty());
    }

    #[test]
    fn context_is_kept_between_two_calls() {
        let mut context = TranscriptContext::default();
        let first = header_line("s1", "/proj-a");
        assert!(parse_pi_uses(&first, &mut context).is_empty());

        let second = user_array_line(
            "2026-09-16T12:00:00Z",
            "foo",
            "/x/.pi/agent/skills/foo/SKILL.md",
        );
        let uses = parse_pi_uses(&second, &mut context);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].session.as_deref(), Some("s1"));
        assert_eq!(uses[0].project_path.as_deref(), Some("/proj-a"));
    }
}
