//! Pure parsing of OpenCode's own session history rows into
//! [`SkillInvocation`]s. No I/O: the host reads the rows (from either the
//! `session_message`/v2 or the `part`/v1 schema) and hands them here.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::identity::AgentId;
use crate::skill_uses::{skill_name_from_skill_md_path, SkillInvocation, SkillTrigger};

/// One `session_message` row (OpenCode v2), as read by the host.
pub struct OpenCodeMessageRow<'a> {
    /// The owning session, used for [`SkillInvocation::session`].
    pub session_id: &'a str,
    /// The row's `type` column: `"skill"` or `"assistant"` matter here.
    pub kind: &'a str,
    /// The row's `time_created` column, epoch milliseconds.
    pub time_created: i64,
    /// The row's `data` column, JSON text.
    pub data: &'a str,
    /// `session_v2.directory`, or `session.directory` when there's no v2 row.
    pub directory: Option<&'a str>,
}

/// One `part` row (OpenCode v1 schema), as read by the host.
pub struct OpenCodePartRow<'a> {
    /// The owning session, used for [`SkillInvocation::session`].
    pub session_id: &'a str,
    /// The row's `time_created` column, epoch milliseconds.
    pub time_created: i64,
    /// The row's `data` column, JSON text.
    pub data: &'a str,
    /// `session.directory`.
    pub directory: Option<&'a str>,
}

/// A non-empty string at `key`, or `None` for a missing key, a non-string
/// value, or an empty string.
fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|s| !s.is_empty())
}

/// `directory`, trimmed of the "no directory recorded" case (an empty
/// string is treated the same as absent).
fn project_path(directory: Option<&str>) -> Option<String> {
    directory.filter(|s| !s.is_empty()).map(|s| s.to_string())
}

/// `ms` as a UTC timestamp, or `None` when it's out of range for
/// [`DateTime::from_timestamp_millis`].
fn timestamp(ms: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(ms)
}

/// A tool item's `at`: `time.created` when it's an integer timestamp
/// `DateTime::from_timestamp_millis` accepts, else the row's own
/// `time_created`.
fn item_at(item: &Value, row_time_created: i64) -> Option<DateTime<Utc>> {
    match item
        .get("time")
        .and_then(|t| t.get("created"))
        .and_then(|v| v.as_i64())
    {
        Some(ms) => timestamp(ms),
        None => timestamp(row_time_created),
    }
}

/// Parses one `session_message` row into zero or more uses: a `type =
/// 'skill'` row gives one `User` use; a `type = 'assistant'` row gives one
/// use per `skill` or `read` tool item in `data.content[]`. Never panics:
/// invalid JSON, a missing or non-string skill name, or a rejected
/// timestamp drops that item rather than the whole row.
pub fn parse_opencode_message(row: &OpenCodeMessageRow) -> Vec<SkillInvocation> {
    let Ok(data) = serde_json::from_str::<Value>(row.data) else {
        return Vec::new();
    };
    let project_path = project_path(row.directory);

    match row.kind {
        "skill" => {
            let Some(skill) = string_field(&data, "skill").or_else(|| string_field(&data, "name"))
            else {
                return Vec::new();
            };
            let Some(at) = timestamp(row.time_created) else {
                return Vec::new();
            };
            vec![SkillInvocation {
                skill: skill.to_string(),
                harness: AgentId::OPEN_CODE.to_string(),
                trigger: SkillTrigger::User,
                at,
                project_path,
                session: Some(row.session_id.to_string()),
            }]
        }
        "assistant" => {
            let Some(content) = data.get("content").and_then(|c| c.as_array()) else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for item in content {
                if item.get("type").and_then(|v| v.as_str()) != Some("tool") {
                    continue;
                }
                let Some(tool_name) = item.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                if !matches!(tool_name, "skill" | "read") {
                    continue;
                }
                let state = item.get("state");
                if state.and_then(|s| s.get("status")).and_then(|v| v.as_str()) == Some("error") {
                    continue;
                }
                let Some(at) = item_at(item, row.time_created) else {
                    continue;
                };
                let input = state.and_then(|s| s.get("input"));

                let use_ = if tool_name == "skill" {
                    let Some(skill) = input
                        .and_then(|i| string_field(i, "id").or_else(|| string_field(i, "name")))
                    else {
                        continue;
                    };
                    SkillInvocation {
                        skill: skill.to_string(),
                        harness: AgentId::OPEN_CODE.to_string(),
                        trigger: SkillTrigger::Agent,
                        at,
                        project_path: project_path.clone(),
                        session: Some(row.session_id.to_string()),
                    }
                } else {
                    let path = input
                        .and_then(|i| string_field(i, "path"))
                        .or_else(|| input.and_then(|i| string_field(i, "filePath")));
                    let Some(skill) = path.and_then(skill_name_from_skill_md_path) else {
                        continue;
                    };
                    SkillInvocation {
                        skill: skill.to_string(),
                        harness: AgentId::OPEN_CODE.to_string(),
                        trigger: SkillTrigger::FileRead,
                        at,
                        project_path: project_path.clone(),
                        session: Some(row.session_id.to_string()),
                    }
                };
                out.push(use_);
            }
            out
        }
        _ => Vec::new(),
    }
}

/// Parses one `part` row (OpenCode v1) into zero or one use: a `read` tool
/// on a known `SKILL.md` path gives a `FileRead`, a `skill` tool gives an
/// `Agent` use. Never panics.
pub fn parse_opencode_part(row: &OpenCodePartRow) -> Vec<SkillInvocation> {
    let Ok(data) = serde_json::from_str::<Value>(row.data) else {
        return Vec::new();
    };
    if data.get("type").and_then(|v| v.as_str()) != Some("tool") {
        return Vec::new();
    }
    let Some(tool) = data.get("tool").and_then(|v| v.as_str()) else {
        return Vec::new();
    };
    if !matches!(tool, "skill" | "read") {
        return Vec::new();
    }
    let state = data.get("state");
    if state.and_then(|s| s.get("status")).and_then(|v| v.as_str()) == Some("error") {
        return Vec::new();
    }
    let Some(at) = timestamp(row.time_created) else {
        return Vec::new();
    };
    let input = state.and_then(|s| s.get("input"));

    let (trigger, skill) = if tool == "read" {
        let path = input
            .and_then(|i| string_field(i, "filePath"))
            .or_else(|| input.and_then(|i| string_field(i, "path")));
        let Some(skill) = path.and_then(skill_name_from_skill_md_path) else {
            return Vec::new();
        };
        (SkillTrigger::FileRead, skill.to_string())
    } else {
        let Some(skill) =
            input.and_then(|i| string_field(i, "name").or_else(|| string_field(i, "id")))
        else {
            return Vec::new();
        };
        (SkillTrigger::Agent, skill.to_string())
    };

    vec![SkillInvocation {
        skill,
        harness: AgentId::OPEN_CODE.to_string(),
        trigger,
        at,
        project_path: project_path(row.directory),
        session: Some(row.session_id.to_string()),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message_row<'a>(
        kind: &'a str,
        time_created: i64,
        data: &'a str,
        directory: Option<&'a str>,
    ) -> OpenCodeMessageRow<'a> {
        OpenCodeMessageRow {
            session_id: "sess-1",
            kind,
            time_created,
            data,
            directory,
        }
    }

    fn part_row<'a>(
        time_created: i64,
        data: &'a str,
        directory: Option<&'a str>,
    ) -> OpenCodePartRow<'a> {
        OpenCodePartRow {
            session_id: "sess-1",
            time_created,
            data,
            directory,
        }
    }

    #[test]
    fn skill_row_gives_one_user_use() {
        let row = message_row(
            "skill",
            1_700_000_000_000,
            r#"{"skill":"deploy","name":"deploy","time":{"created":1}}"#,
            Some("/proj"),
        );
        let uses = parse_opencode_message(&row);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "deploy");
        assert_eq!(uses[0].trigger, SkillTrigger::User);
        assert_eq!(uses[0].harness, AgentId::OPEN_CODE);
        assert_eq!(uses[0].session.as_deref(), Some("sess-1"));
        assert_eq!(uses[0].project_path.as_deref(), Some("/proj"));
        assert_eq!(uses[0].at.timestamp_millis(), 1_700_000_000_000);
    }

    #[test]
    fn skill_row_falls_back_to_name_field() {
        let row = message_row(
            "skill",
            1_700_000_000_000,
            r#"{"name":"deploy","time":{"created":1}}"#,
            None,
        );
        let uses = parse_opencode_message(&row);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "deploy");
    }

    #[test]
    fn skill_tool_item_completed_gives_agent_use() {
        let data = r#"{"content":[{"type":"tool","id":"call1","name":"skill","time":{"created":1700000000500},"state":{"status":"completed","input":{"id":"deploy"}}}]}"#;
        let row = message_row("assistant", 1_700_000_000_000, data, Some("/proj"));
        let uses = parse_opencode_message(&row);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "deploy");
        assert_eq!(uses[0].trigger, SkillTrigger::Agent);
        assert_eq!(uses[0].at.timestamp_millis(), 1_700_000_000_500);
    }

    #[test]
    fn skill_tool_item_errored_gives_nothing() {
        let data = r#"{"content":[{"type":"tool","id":"call1","name":"skill","time":{"created":1},"state":{"status":"error","input":{"id":"deploy"}}}]}"#;
        let row = message_row("assistant", 1_700_000_000_000, data, None);
        assert!(parse_opencode_message(&row).is_empty());
    }

    #[test]
    fn read_tool_on_known_skill_md_path_gives_file_read() {
        let data = r#"{"content":[{"type":"tool","id":"call1","name":"read","state":{"status":"completed","input":{"path":"/x/.claude/skills/foo/SKILL.md"}}}]}"#;
        let row = message_row("assistant", 1_700_000_000_000, data, Some("/proj"));
        let uses = parse_opencode_message(&row);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
        assert_eq!(uses[0].at.timestamp_millis(), 1_700_000_000_000);
    }

    #[test]
    fn read_tool_outside_a_skills_root_gives_nothing() {
        let data = r#"{"content":[{"type":"tool","id":"call1","name":"read","state":{"status":"completed","input":{"path":"/x/notes.md"}}}]}"#;
        let row = message_row("assistant", 1_700_000_000_000, data, None);
        assert!(parse_opencode_message(&row).is_empty());
    }

    #[test]
    fn shell_tool_with_a_skill_md_looking_command_gives_nothing() {
        let data = r#"{"content":[{"type":"tool","id":"call1","name":"shell","state":{"status":"completed","input":{"command":"cat /x/.claude/skills/foo/SKILL.md"}}}]}"#;
        let row = message_row("assistant", 1_700_000_000_000, data, None);
        assert!(parse_opencode_message(&row).is_empty());
    }

    #[test]
    fn two_tool_items_give_two_uses() {
        let data = r#"{"content":[
            {"type":"tool","id":"call1","name":"skill","state":{"status":"completed","input":{"id":"deploy"}}},
            {"type":"tool","id":"call2","name":"read","state":{"status":"completed","input":{"path":"/x/.claude/skills/foo/SKILL.md"}}}
        ]}"#;
        let row = message_row("assistant", 1_700_000_000_000, data, None);
        assert_eq!(parse_opencode_message(&row).len(), 2);
    }

    #[test]
    fn bad_json_gives_nothing() {
        let row = message_row("assistant", 1_700_000_000_000, "not json", None);
        assert!(parse_opencode_message(&row).is_empty());
    }

    #[test]
    fn part_read_row_with_file_path_gives_file_read() {
        let data = r#"{"type":"tool","tool":"read","callID":"c1","state":{"status":"completed","input":{"filePath":"/x/.config/opencode/skill/foo/SKILL.md"}}}"#;
        let row = part_row(1_700_000_000_000, data, Some("/proj"));
        let uses = parse_opencode_part(&row);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
        assert_eq!(uses[0].project_path.as_deref(), Some("/proj"));
    }

    #[test]
    fn part_skill_row_gives_agent_use() {
        let data = r#"{"type":"tool","tool":"skill","callID":"c1","state":{"status":"completed","input":{"name":"deploy"}}}"#;
        let row = part_row(1_700_000_000_000, data, None);
        let uses = parse_opencode_part(&row);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "deploy");
        assert_eq!(uses[0].trigger, SkillTrigger::Agent);
    }

    #[test]
    fn part_row_with_error_status_gives_nothing() {
        let data = r#"{"type":"tool","tool":"read","callID":"c1","state":{"status":"error","input":{"filePath":"/x/.claude/skills/foo/SKILL.md"}}}"#;
        let row = part_row(1_700_000_000_000, data, None);
        assert!(parse_opencode_part(&row).is_empty());
    }
}
