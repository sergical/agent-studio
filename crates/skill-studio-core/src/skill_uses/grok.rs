//! Grok Build transcript parsing: `~/.grok/sessions/<encoded cwd>/<session
//! id>/updates.jsonl`. A line is an ACP envelope, `{"timestamp":..,"method":
//! "session/update","params":{"update":{..}}}` (the legacy bare form drops
//! `method` and states `update` at the top level); `_x.ai/session/update` is
//! an xAI extension this reader skips. `update.sessionUpdate` tags the
//! record; keys inside `update` are camelCase. A forked session copies its
//! parent's lines with a fresh `timestamp`, so every copied line has
//! `timestamp <= forked_at`; `context.forked_at` (from the session's
//! `summary.json`, filled by the host) lets a resumed fork skip them (see
//! `docs/agent-skill-conventions.md` "Skill uses"). `context.counted_calls`
//! dedupes a tool call Grok writes on more than one line
//! (`tool_call`/`tool_call_update` share a `toolCallId`).

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::identity::AgentId;
use crate::skill_uses::{
    push_shell_reads, push_use, skill_name_from_read_path, SkillInvocation, SkillTrigger,
    TranscriptContext,
};

/// Fast-path substrings a line must contain before it's worth a full JSON
/// parse.
const USER_MESSAGE_MARKER: &str = "user_message_chunk";
const TOOL_CALL_MARKER: &str = "tool_call";

/// `update._meta["x.ai/tool"]`, when it's an object - Grok's canonical tool
/// classification, present for its own tools but absent for MCP/backend
/// tools.
fn tool_meta(update: &Value) -> Option<&Value> {
    update
        .get("_meta")
        .and_then(|m| m.get("x.ai/tool"))
        .filter(|v| v.is_object())
}

/// A skill name candidate from a typed `/name ...` token: the text after the
/// `/`, lowercased, or `None` when it's empty or holds another `/` (an
/// absolute path token, not a command).
fn user_skill_candidate(token: &str) -> Option<String> {
    let rest = token.strip_prefix('/')?;
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    Some(rest.to_lowercase())
}

/// A `user_message_chunk`'s typed skill names: `content` must be
/// `{"type":"text","text":..}`, `text` must start with `/`, and a
/// `_meta.hostTurn: true` echo is skipped. Each name is pushed once per line.
fn push_user_uses(
    out: &mut Vec<SkillInvocation>,
    context: &TranscriptContext,
    update: &Value,
    at: DateTime<Utc>,
) {
    if update
        .get("_meta")
        .and_then(|m| m.get("hostTurn"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return;
    }
    let content = update.get("content");
    if content.and_then(|c| c.get("type")).and_then(Value::as_str) != Some("text") {
        return;
    }
    let Some(text) = content.and_then(|c| c.get("text")).and_then(Value::as_str) else {
        return;
    };
    if !text.starts_with('/') {
        return;
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for token in text.split_whitespace() {
        let Some(name) = user_skill_candidate(token) else {
            continue;
        };
        if seen.insert(name.clone()) {
            push_use(
                out,
                context,
                AgentId::GROK_BUILD,
                &name,
                SkillTrigger::User,
                at,
            );
        }
    }
}

/// The `Skill: X` name in `title`, trimmed, or `None` when `title` doesn't
/// start with that exact prefix.
fn skill_title_name(title: &str) -> Option<&str> {
    let name = title.strip_prefix("Skill: ")?.trim();
    (!name.is_empty()).then_some(name)
}

/// One `tool_call`/`tool_call_update`'s uses: a `Skill` tool call, a `read`
/// tool call on a skill's `SKILL.md`, or an `execute` (shell) call that
/// prints one.
fn push_tool_call_uses(
    out: &mut Vec<SkillInvocation>,
    context: &TranscriptContext,
    update: &Value,
    at: DateTime<Utc>,
) {
    let meta = tool_meta(update);
    let meta_kind = meta.and_then(|m| m.get("kind")).and_then(Value::as_str);
    let title = update.get("title").and_then(Value::as_str);
    let raw_input = update.get("rawInput");
    let raw_variant = raw_input
        .and_then(|r| r.get("variant"))
        .and_then(Value::as_str);

    let is_skill = meta_kind == Some("skill")
        || title.is_some_and(|t| t.starts_with("Skill: "))
        || raw_variant == Some("Skill");
    if is_skill {
        let name = title.and_then(skill_title_name).or_else(|| {
            raw_input
                .and_then(|r| r.get("skill"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        });
        if let Some(name) = name {
            push_use(
                out,
                context,
                AgentId::GROK_BUILD,
                name,
                SkillTrigger::Agent,
                at,
            );
        }
        return;
    }

    let is_read = meta_kind == Some("read")
        || (meta.is_none() && update.get("kind").and_then(Value::as_str) == Some("read"));
    if is_read {
        let path = meta
            .and_then(|m| m.get("input"))
            .and_then(|i| i.get("path"))
            .and_then(Value::as_str)
            .or_else(|| {
                update
                    .get("locations")
                    .and_then(|l| l.get(0))
                    .and_then(|loc| loc.get("path"))
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                raw_input
                    .filter(|_| raw_variant == Some("ReadFile"))
                    .and_then(|r| r.get("target_file"))
                    .and_then(Value::as_str)
            });
        if let Some(path) = path {
            if let Some(skill) = skill_name_from_read_path(path, context.project_path.as_deref()) {
                push_use(
                    out,
                    context,
                    AgentId::GROK_BUILD,
                    &skill,
                    SkillTrigger::FileRead,
                    at,
                );
            }
        }
        return;
    }

    if meta_kind == Some("execute") {
        if let Some(command) = meta
            .and_then(|m| m.get("input"))
            .and_then(|i| i.get("command"))
            .and_then(Value::as_str)
        {
            push_shell_reads(out, context, AgentId::GROK_BUILD, [command], at);
        }
    }
}

/// Parses one Grok Build session transcript's text (newline-delimited JSON,
/// `updates.jsonl`) into skill uses: a `User` use per typed `/name` token, an
/// `Agent` use per `Skill` tool call, and a `FileRead` use per `read` tool
/// call or shell command that reads a skill's `SKILL.md`. `context` carries
/// the session id, project path, fork cutoff, and already-counted tool call
/// ids across calls (see the module doc). `fallback_at` is used for a line
/// with no `timestamp`. Never panics: a malformed line, or one this reader
/// doesn't recognize, is skipped rather than failing the whole file.
pub fn parse_grok_uses(
    text: &str,
    context: &mut TranscriptContext,
    fallback_at: DateTime<Utc>,
) -> Vec<SkillInvocation> {
    let mut out = Vec::new();

    for line in text.lines() {
        if !line.contains(USER_MESSAGE_MARKER) && !line.contains(TOOL_CALL_MARKER) {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        let method = record.get("method").and_then(Value::as_str);
        let update = match method {
            Some("session/update") => record.get("params").and_then(|p| p.get("update")),
            Some(_) => continue, // an xAI extension method (e.g. `_x.ai/session/update`)
            None => record.get("update"),
        };
        let Some(update) = update else {
            continue;
        };

        let timestamp = record
            .get("timestamp")
            .and_then(Value::as_i64)
            .filter(|&t| t > 0);
        if let (Some(forked_at), Some(t)) = (context.forked_at, timestamp) {
            if t <= forked_at.timestamp() {
                continue;
            }
        }
        let at = timestamp
            .and_then(|t| DateTime::from_timestamp(t, 0))
            .unwrap_or(fallback_at);

        match update.get("sessionUpdate").and_then(Value::as_str) {
            Some("user_message_chunk") => push_user_uses(&mut out, context, update, at),
            Some("tool_call" | "tool_call_update") => {
                let Some(tool_call_id) = update.get("toolCallId").and_then(Value::as_str) else {
                    continue;
                };
                if context.counted_calls.contains(tool_call_id) {
                    continue;
                }
                let before = out.len();
                push_tool_call_uses(&mut out, context, update, at);
                if out.len() > before {
                    context.counted_calls.insert(tool_call_id.to_string());
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
    use serde_json::json;

    fn envelope(timestamp: Option<i64>, update: Value) -> String {
        let mut record = json!({"method": "session/update", "params": {"update": update}});
        if let Some(t) = timestamp {
            record["timestamp"] = json!(t);
        }
        record.to_string()
    }

    fn legacy(update: Value) -> String {
        json!({"update": update}).to_string()
    }

    #[test]
    fn tool_call_update_read_with_meta_gives_a_file_read() {
        let update = json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc1",
            "_meta": {"x.ai/tool": {
                "version": 1,
                "name": "read_file",
                "kind": "read",
                "namespace": "grok_build",
                "label": "Read",
                "read_only": true,
                "input": {"path": "/x/skills/foo/SKILL.md"}
            }}
        });
        let text = envelope(Some(1_757_000_000), update);
        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(&text, &mut context, Utc::now());
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
        assert_eq!(uses[0].harness, AgentId::GROK_BUILD);
        assert_eq!(
            uses[0].at,
            DateTime::from_timestamp(1_757_000_000, 0).unwrap()
        );
    }

    #[test]
    fn upstream_shape_without_meta_gives_a_file_read() {
        let update = json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tc1",
            "title": "Read `/x/skills/foo/SKILL.md`",
            "kind": "read",
            "locations": [{"path": "/x/skills/foo/SKILL.md"}]
        });
        let text = envelope(Some(1_757_000_000), update);
        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(&text, &mut context, Utc::now());
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
    }

    #[test]
    fn raw_input_read_file_variant_gives_a_file_read() {
        let update = json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tc1",
            "kind": "read",
            "rawInput": {"variant": "ReadFile", "target_file": "/x/skills/foo/SKILL.md"}
        });
        let text = envelope(Some(1_757_000_000), update);
        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(&text, &mut context, Utc::now());
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
    }

    #[test]
    fn a_tool_call_id_counts_once_within_and_across_parses() {
        let start = json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tc1",
            "title": "Skill: foo",
            "kind": "other"
        });
        let update = json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc1",
            "title": "Skill: foo",
            "kind": "other"
        });
        let text = format!(
            "{}\n{}\n",
            envelope(Some(1_757_000_000), start),
            envelope(Some(1_757_000_001), update.clone())
        );
        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(&text, &mut context, Utc::now());
        assert_eq!(uses.len(), 1);

        let second_pass = parse_grok_uses(
            &envelope(Some(1_757_000_002), update),
            &mut context,
            Utc::now(),
        );
        assert!(second_pass.is_empty());
    }

    #[test]
    fn skill_tool_call_gives_an_agent_use() {
        let title_only = json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tc1",
            "title": "Skill: foo",
            "kind": "other",
            "_meta": {"x.ai/tool": {"kind": "skill"}}
        });
        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(&envelope(Some(1), title_only), &mut context, Utc::now());
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::Agent);

        let title_no_meta = json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tc2",
            "title": "Skill: foo",
            "kind": "other"
        });
        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(&envelope(Some(1), title_no_meta), &mut context, Utc::now());
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].trigger, SkillTrigger::Agent);

        let raw_input_only = json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tc3",
            "kind": "other",
            "rawInput": {"variant": "Skill", "skill": "foo"}
        });
        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(&envelope(Some(1), raw_input_only), &mut context, Utc::now());
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::Agent);
    }

    #[test]
    fn execute_reads_a_skill_md_and_ls_gives_nothing() {
        let cat = json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tc1",
            "kind": "other",
            "_meta": {"x.ai/tool": {"kind": "execute", "input": {"command": "cat /x/skills/foo/SKILL.md"}}}
        });
        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(&envelope(Some(1), cat), &mut context, Utc::now());
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);

        let ls = json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tc2",
            "kind": "other",
            "_meta": {"x.ai/tool": {"kind": "execute", "input": {"command": "ls /x/skills/foo/SKILL.md"}}}
        });
        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(&envelope(Some(1), ls), &mut context, Utc::now());
        assert!(uses.is_empty());
    }

    #[test]
    fn typed_slash_tokens_are_parsed() {
        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(
            &envelope(
                Some(1),
                json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "/foo do it"}}),
            ),
            &mut context,
            Utc::now(),
        );
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::User);

        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(
            &envelope(
                Some(1),
                json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "/Foo"}}),
            ),
            &mut context,
            Utc::now(),
        );
        assert_eq!(uses[0].skill, "foo");

        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(
            &envelope(
                Some(1),
                json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "/model /bar"}}),
            ),
            &mut context,
            Utc::now(),
        );
        let names: Vec<&str> = uses.iter().map(|u| u.skill.as_str()).collect();
        assert_eq!(names, vec!["model", "bar"]);

        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(
            &envelope(
                Some(1),
                json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "/plug:foo"}}),
            ),
            &mut context,
            Utc::now(),
        );
        assert_eq!(uses[0].skill, "plug:foo");

        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(
            &envelope(
                Some(1),
                json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "/Users/a/b"}}),
            ),
            &mut context,
            Utc::now(),
        );
        assert!(uses.is_empty());

        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(
            &envelope(
                Some(1),
                json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "hello /foo"}}),
            ),
            &mut context,
            Utc::now(),
        );
        assert!(uses.is_empty());

        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(
            &envelope(
                Some(1),
                json!({
                    "sessionUpdate": "user_message_chunk",
                    "content": {"type": "text", "text": "/foo"},
                    "_meta": {"hostTurn": true}
                }),
            ),
            &mut context,
            Utc::now(),
        );
        assert!(uses.is_empty());

        let mut context = TranscriptContext::default();
        let uses = parse_grok_uses(
            &envelope(
                Some(1),
                json!({"sessionUpdate": "user_message_chunk", "content": {"type": "image", "data": "x"}}),
            ),
            &mut context,
            Utc::now(),
        );
        assert!(uses.is_empty());
    }

    #[test]
    fn a_legacy_line_uses_the_fallback_time() {
        let update = json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "/foo"}
        });
        let mut context = TranscriptContext::default();
        let fallback = Utc::now();
        let uses = parse_grok_uses(&legacy(update), &mut context, fallback);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].at, fallback);
    }

    #[test]
    fn an_xai_extension_method_is_skipped() {
        let mut record = envelope(
            Some(1),
            json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "/foo"}}),
        );
        record = record.replace("\"session/update\"", "\"_x.ai/session/update\"");
        let mut context = TranscriptContext::default();
        assert!(parse_grok_uses(&record, &mut context, Utc::now()).is_empty());
    }

    #[test]
    fn fork_copies_at_or_before_forked_at_are_skipped() {
        let forked_at = "2026-09-16T10:00:00.500Z".parse::<DateTime<Utc>>().unwrap();
        let mut context = TranscriptContext {
            forked_at: Some(forked_at),
            ..Default::default()
        };
        let update = json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "/foo"}
        });

        let at_the_second = parse_grok_uses(
            &envelope(Some(forked_at.timestamp()), update.clone()),
            &mut context,
            Utc::now(),
        );
        assert!(at_the_second.is_empty());

        let one_second_later = parse_grok_uses(
            &envelope(Some(forked_at.timestamp() + 1), update.clone()),
            &mut context,
            Utc::now(),
        );
        assert_eq!(one_second_later.len(), 1);

        let fallback = Utc::now();
        let legacy_line = parse_grok_uses(&legacy(update), &mut context, fallback);
        assert_eq!(legacy_line.len(), 1);
        assert_eq!(legacy_line[0].at, fallback);
    }

    #[test]
    fn malformed_or_incomplete_lines_are_skipped() {
        let mut context = TranscriptContext::default();
        assert!(parse_grok_uses("{\"tool_call\" not json", &mut context, Utc::now()).is_empty());

        let missing_tool_call_id =
            json!({"sessionUpdate": "tool_call", "title": "Skill: foo", "kind": "other"});
        let mut context = TranscriptContext::default();
        assert!(parse_grok_uses(
            &envelope(Some(1), missing_tool_call_id),
            &mut context,
            Utc::now()
        )
        .is_empty());

        let read_outside_a_root = json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tc1",
            "kind": "read",
            "locations": [{"path": "/x/notes/SKILL.md"}]
        });
        let mut context = TranscriptContext::default();
        assert!(parse_grok_uses(
            &envelope(Some(1), read_outside_a_root),
            &mut context,
            Utc::now()
        )
        .is_empty());
    }
}
