//! Codex transcript parsing: `~/.codex/sessions/**/*.jsonl` and
//! `~/.codex/archived_sessions/**/*.jsonl`. Unlike Claude Code, a Codex line
//! doesn't repeat its session id or project path - those are stated once, in
//! the file's first `session_meta` line - so parsing threads a
//! [`TranscriptContext`] through the whole file (see `docs/agent-skill-
//! conventions.md` "Skill uses" for the record shapes).

use chrono::{DateTime, Utc};

use crate::identity::AgentId;
use crate::skill_uses::{
    push_shell_reads, push_use, skill_name_from_skill_md_path, SkillInvocation, SkillTrigger,
    TranscriptContext,
};

/// Fast-path substrings a line must contain before it's worth a full JSON
/// parse.
const SESSION_META_MARKER: &str = "\"session_meta\"";
const SKILL_BLOCK_MARKER: &str = "<skill>";
const SKILL_MD_MARKER: &str = "SKILL.md";
const SKILLS_NAMESPACE_MARKER: &str = "\"namespace\":\"skills\"";

/// The name in a user `<skill>\n<name>X</name>\n<path>P</path>...` block, or
/// `None` when `text` doesn't open with that exact shape (many other lines
/// mention `<skill>` mid-text and must not match).
fn parse_skill_block(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("<skill>\n<name>")?;
    let end = rest.find("</name>")?;
    let name = &rest[..end];
    if name.is_empty() || name.contains('\n') {
        return None;
    }
    let after = &rest[end + "</name>".len()..];
    after.starts_with("\n<path>").then_some(name)
}

/// The command a `function_call` `exec_command`/`shell` call runs, from its
/// already-parsed `arguments`: `cmd`/`command`, either a plain string, or (for
/// the 3+ item `["bash", "-c"|"-lc", "<command>", ...]` shape) the third
/// item; any other array is joined with spaces.
fn command_from_function_call_args(args: &serde_json::Value) -> Option<String> {
    let value = args.get("cmd").or_else(|| args.get("command"))?;
    if let Some(command) = value.as_str() {
        return Some(command.to_string());
    }
    let items = value.as_array()?;
    let strings: Vec<&str> = items.iter().filter_map(|v| v.as_str()).collect();
    if strings.len() != items.len() {
        return None;
    }
    if strings.len() >= 3 {
        let shell = strings[0].rsplit('/').next().unwrap_or(strings[0]);
        if matches!(shell, "bash" | "sh" | "zsh") && matches!(strings[1], "-c" | "-lc") {
            return Some(strings[2].to_string());
        }
    }
    Some(strings.join(" "))
}

/// Parses one Codex rollout's text (newline-delimited JSON): a `User` use
/// per user `<skill>` block, a `FileRead` use per shell command (`exec`
/// custom tool call, or an older `exec_command`/`shell` function call) that
/// reads a skill's `SKILL.md`, and an `Agent` use per `skills`/`read`
/// function call without a `resource`. `context` carries the session id and
/// project path across calls (see the module doc); a `session_meta` line
/// only sets it the first time it's seen (a forked file repeats it). Never
/// panics: a malformed line, or one missing a `timestamp`, is skipped rather
/// than failing the whole file.
pub fn parse_codex_uses(text: &str, context: &mut TranscriptContext) -> Vec<SkillInvocation> {
    let mut out = Vec::new();

    for line in text.lines() {
        if !line.contains(SESSION_META_MARKER)
            && !line.contains(SKILL_BLOCK_MARKER)
            && !line.contains(SKILL_MD_MARKER)
            && !line.contains(SKILLS_NAMESPACE_MARKER)
        {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let record_type = record.get("type").and_then(|v| v.as_str());

        if record_type == Some("session_meta") {
            if context.session.is_none() {
                if let Some(payload) = record.get("payload") {
                    if let Some(id) = payload.get("id").and_then(|v| v.as_str()) {
                        if !id.is_empty() {
                            context.session = Some(id.to_string());
                        }
                    }
                    if let Some(cwd) = payload.get("cwd").and_then(|v| v.as_str()) {
                        if !cwd.is_empty() {
                            context.project_path = Some(cwd.to_string());
                        }
                    }
                }
            }
            continue;
        }
        if record_type != Some("response_item") {
            continue;
        }

        let Some(timestamp) = record.get("timestamp").and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(at) = DateTime::parse_from_rfc3339(timestamp) else {
            continue;
        };
        let at = at.with_timezone(&Utc);

        let Some(payload) = record.get("payload") else {
            continue;
        };
        match payload.get("type").and_then(|v| v.as_str()) {
            Some("message") if payload.get("role").and_then(|v| v.as_str()) == Some("user") => {
                let Some(content) = payload.get("content").and_then(|c| c.as_array()) else {
                    continue;
                };
                for item in content {
                    if item.get("type").and_then(|v| v.as_str()) != Some("input_text") {
                        continue;
                    }
                    let Some(text) = item.get("text").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    if let Some(name) = parse_skill_block(text) {
                        push_use(
                            &mut out,
                            context,
                            AgentId::CODEX,
                            name,
                            SkillTrigger::User,
                            at,
                        );
                    }
                }
            }
            Some("custom_tool_call")
                if payload.get("name").and_then(|v| v.as_str()) == Some("exec") =>
            {
                let Some(input) = payload.get("input").and_then(|v| v.as_str()) else {
                    continue;
                };
                push_shell_reads(
                    &mut out,
                    context,
                    AgentId::CODEX,
                    exec_command_literals(input),
                    at,
                );
            }
            Some("function_call") => {
                let name = payload.get("name").and_then(|v| v.as_str());
                let namespace = payload.get("namespace").and_then(|v| v.as_str());
                let Some(args_str) = payload.get("arguments").and_then(|v| v.as_str()) else {
                    continue;
                };
                let Ok(args) = serde_json::from_str::<serde_json::Value>(args_str) else {
                    continue;
                };
                if namespace == Some("skills") && name == Some("read") {
                    let has_resource = args
                        .get("resource")
                        .is_some_and(|v| !v.is_null() && v.as_str() != Some(""));
                    if has_resource {
                        continue;
                    }
                    let Some(package) = args.get("package").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    if let Some(name) = codex_skill_name_from_package(package) {
                        push_use(
                            &mut out,
                            context,
                            AgentId::CODEX,
                            name,
                            SkillTrigger::Agent,
                            at,
                        );
                    }
                } else if matches!(name, Some("exec_command" | "shell")) {
                    if let Some(command) = command_from_function_call_args(&args) {
                        push_shell_reads(&mut out, context, AgentId::CODEX, [command], at);
                    }
                }
            }
            _ => {}
        }
    }

    out
}

/// The skill name in a Codex `skills.read` `package` value: the absolute
/// path to the skill's `SKILL.md` (host provider), or `skill://<root-id>/
/// <path of the skill inside that root>` (executor provider; a root alias
/// shortens the front of the value but keeps the tail). Either way the name
/// is the folder that holds `SKILL.md`, per the agentskills spec rule that
/// the folder name equals the skill name.
pub fn codex_skill_name_from_package(package: &str) -> Option<&str> {
    if let Some(name) = skill_name_from_skill_md_path(package) {
        return Some(name);
    }

    let path = if let Some(rest) = package.strip_prefix("skill://") {
        let (_, after_root) = rest.split_once('/')?;
        if after_root.is_empty() {
            return None;
        }
        after_root
    } else {
        package
    };

    let path = path.trim_end_matches('/');
    let path = match path.strip_suffix("/SKILL.md") {
        Some(stripped) => stripped,
        None if path == "SKILL.md" => return None,
        None => path,
    };

    let name = path.rsplit('/').next().unwrap_or(path);
    if name.is_empty() || name == "." || name == ".." {
        None
    } else {
        Some(name)
    }
}

/// The number of leading whitespace bytes (space/tab) in `s`.
fn leading_ws_len(s: &str) -> usize {
    s.find(|c: char| c != ' ' && c != '\t').unwrap_or(s.len())
}

/// Decodes `\n` -> newline, `\t` -> tab, `\X` -> `X`, for a literal whose
/// quote style (`'`/`` ` ``) `serde_json` can't decode.
fn simple_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(escaped) => out.push(escaped),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Every `cmd`/`"cmd"`/`'cmd'`/`` `cmd` `` -> literal value pulled out of a
/// Codex `exec` custom tool call's JS `input` string (see the module doc):
/// finds each `cmd` key not glued to a surrounding identifier, then an
/// optional `:` (with optional whitespace around it) and a `"`/`'`/`` ` ``
/// literal, read up to its matching unescaped closing quote. A backtick
/// literal that contains `${` (a template) is skipped, not decoded. An
/// unterminated literal ends the scan (nothing after it can be a complete
/// call).
fn exec_command_literals(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = input[i..].find("cmd") {
        let start = i + rel;
        let mut after = start + "cmd".len();

        let prev = input[..start].chars().next_back();
        let before_ok = prev.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        if !before_ok {
            i = start + 1;
            continue;
        }

        let quote_before = prev.filter(|c| matches!(c, '"' | '\'' | '`'));
        if let (Some(q), Some(next)) = (quote_before, input[after..].chars().next()) {
            if next == q {
                after += q.len_utf8();
            }
        }
        let after_ok = input[after..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        if !after_ok {
            i = start + 1;
            continue;
        }

        let mut j = after + leading_ws_len(&input[after..]);
        if !input[j..].starts_with(':') {
            i = start + 1;
            continue;
        }
        j += 1;
        j += leading_ws_len(&input[j..]);

        let Some(quote) = input[j..]
            .chars()
            .next()
            .filter(|c| matches!(c, '"' | '\'' | '`'))
        else {
            i = start + 1;
            continue;
        };
        let content_start = j + quote.len_utf8();

        let mut escaped = false;
        let mut end = None;
        for (offset, c) in input[content_start..].char_indices() {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == quote {
                end = Some(content_start + offset);
                break;
            }
        }
        let Some(end) = end else {
            // Unterminated: no complete call can follow.
            break;
        };

        let literal = &input[content_start..end];
        if quote == '`' && literal.contains("${") {
            i = end + 1;
            continue;
        }
        let decoded = if quote == '"' {
            serde_json::from_str::<String>(&format!("\"{literal}\""))
                .unwrap_or_else(|_| simple_decode(literal))
        } else {
            simple_decode(literal)
        };
        out.push(decoded);
        i = end + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_meta_line(id: &str, cwd: &str) -> String {
        format!(r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"{cwd}"}}}}"#)
    }

    fn skill_block_line(timestamp: &str, name: &str) -> String {
        format!(
            r#"{{"type":"response_item","timestamp":"{timestamp}","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"<skill>\n<name>{name}</name>\n<path>/u/.codex/skills/{name}/SKILL.md</path>\n"}}]}}}}"#
        )
    }

    fn exec_line(timestamp: &str, command: &str) -> String {
        format!(
            r#"{{"type":"response_item","timestamp":"{timestamp}","payload":{{"type":"custom_tool_call","name":"exec","input":"tools.exec_command({{\"cmd\": \"{command}\"}})"}}}}"#
        )
    }

    fn skills_read_line(timestamp: &str, package: &str, resource: Option<&str>) -> String {
        let args = match resource {
            Some(r) => format!(r#"{{\"package\": \"{package}\", \"resource\": \"{r}\"}}"#),
            None => format!(r#"{{\"package\": \"{package}\"}}"#),
        };
        format!(
            r#"{{"type":"response_item","timestamp":"{timestamp}","payload":{{"type":"function_call","namespace":"skills","name":"read","arguments":"{args}"}}}}"#
        )
    }

    #[test]
    fn codex_skill_name_from_package_rules() {
        assert_eq!(
            codex_skill_name_from_package("/u/.codex/skills/foo/SKILL.md"),
            Some("foo")
        );
        assert_eq!(
            codex_skill_name_from_package("/repo/.agents/skills/foo/SKILL.md"),
            Some("foo")
        );
        assert_eq!(
            codex_skill_name_from_package("/u/plugins/cache/p/foo/SKILL.md"),
            Some("foo")
        );
        assert_eq!(
            codex_skill_name_from_package("skill://root_1/foo"),
            Some("foo")
        );
        assert_eq!(
            codex_skill_name_from_package("skill://root_1/group/foo/"),
            Some("foo")
        );
        assert_eq!(
            codex_skill_name_from_package("skill://root_1/foo/SKILL.md"),
            Some("foo")
        );
        assert_eq!(codex_skill_name_from_package("skill://root_1"), None);
        assert_eq!(codex_skill_name_from_package("skill://root_1/"), None);
        assert_eq!(codex_skill_name_from_package("SKILL.md"), None);
        assert_eq!(codex_skill_name_from_package(""), None);
    }

    #[test]
    fn exec_command_literals_rules() {
        assert_eq!(
            exec_command_literals(r#""cmd": "cat /x""#),
            vec!["cat /x".to_string()]
        );
        assert_eq!(
            exec_command_literals(r#"cmd: "cat /x""#),
            vec!["cat /x".to_string()]
        );
        assert_eq!(
            exec_command_literals("cmd: 'cat /x'"),
            vec!["cat /x".to_string()]
        );
        assert_eq!(
            exec_command_literals("cmd: `cat /x`"),
            vec!["cat /x".to_string()]
        );
        assert_eq!(
            exec_command_literals(r"cmd: 'ls /x\nsed -n 1p /x'"),
            vec!["ls /x\nsed -n 1p /x".to_string()]
        );
        assert!(exec_command_literals("cmd: `cat ${path}`").is_empty());
        assert!(exec_command_literals(r#"cmdline: "cat /x""#).is_empty());
        assert_eq!(
            exec_command_literals(r#"cmd: "cat \"/x/skills/foo/SKILL.md\"""#),
            vec![r#"cat "/x/skills/foo/SKILL.md""#.to_string()]
        );
        assert_eq!(
            exec_command_literals(
                r#"tools.exec_command({"cmd": "cat /a"}); tools.exec_command({"cmd": "cat /b"})"#
            ),
            vec!["cat /a".to_string(), "cat /b".to_string()]
        );
    }

    #[test]
    fn session_meta_sets_context_and_a_second_one_does_not_change_it() {
        let text = format!(
            "{}\n{}\n",
            session_meta_line("s1", "/proj-a"),
            session_meta_line("s2", "/proj-b"),
        );
        let mut context = TranscriptContext::default();
        assert!(parse_codex_uses(&text, &mut context).is_empty());
        assert_eq!(context.session.as_deref(), Some("s1"));
        assert_eq!(context.project_path.as_deref(), Some("/proj-a"));
    }

    #[test]
    fn user_skill_block_gives_a_user_use_with_context_and_line_timestamp() {
        let text = format!(
            "{}\n{}\n",
            session_meta_line("s1", "/proj-a"),
            skill_block_line("2026-09-16T12:00:00Z", "foo"),
        );
        let mut context = TranscriptContext::default();
        let uses = parse_codex_uses(&text, &mut context);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::User);
        assert_eq!(uses[0].session.as_deref(), Some("s1"));
        assert_eq!(uses[0].project_path.as_deref(), Some("/proj-a"));
        assert_eq!(uses[0].at.to_rfc3339(), "2026-09-16T12:00:00+00:00");
    }

    #[test]
    fn skill_block_mid_text_gives_nothing() {
        let text = r#"{"type":"response_item","timestamp":"2026-09-16T12:00:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"just chatting about <skill>\n<name>foo</name>\n<path>/u/.codex/skills/foo/SKILL.md</path> today"}]}}"#;
        let mut context = TranscriptContext::default();
        assert!(parse_codex_uses(text, &mut context).is_empty());
    }

    #[test]
    fn event_msg_with_a_skill_block_gives_nothing() {
        let text = r#"{"type":"event_msg","timestamp":"2026-09-16T12:00:00Z","payload":{"type":"item_completed","text":"<skill>\n<name>foo</name>\n<path>/u/.codex/skills/foo/SKILL.md</path>"}}"#;
        let mut context = TranscriptContext::default();
        assert!(parse_codex_uses(text, &mut context).is_empty());
    }

    #[test]
    fn exec_sed_n_read_gives_a_file_read() {
        let text = exec_line(
            "2026-09-16T12:00:00Z",
            "sed -n '1,50p' /u/.codex/skills/foo/SKILL.md",
        );
        let mut context = TranscriptContext::default();
        let uses = parse_codex_uses(&text, &mut context);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
    }

    #[test]
    fn exec_multi_line_command_gives_a_file_read() {
        let text = exec_line(
            "2026-09-16T12:00:00Z",
            r"ls /x\\nsed -n 1p /u/.codex/skills/foo/SKILL.md",
        );
        let mut context = TranscriptContext::default();
        let uses = parse_codex_uses(&text, &mut context);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
    }

    #[test]
    fn exec_wc_gives_nothing() {
        let text = exec_line(
            "2026-09-16T12:00:00Z",
            "wc -l /u/.codex/skills/foo/SKILL.md",
        );
        let mut context = TranscriptContext::default();
        assert!(parse_codex_uses(&text, &mut context).is_empty());
    }

    #[test]
    fn skills_read_without_resource_gives_an_agent_use() {
        let text = skills_read_line(
            "2026-09-16T12:00:00Z",
            "/u/.codex/skills/foo/SKILL.md",
            None,
        );
        let mut context = TranscriptContext::default();
        let uses = parse_codex_uses(&text, &mut context);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::Agent);
    }

    #[test]
    fn skills_read_with_resource_gives_nothing() {
        let text = skills_read_line(
            "2026-09-16T12:00:00Z",
            "/u/.codex/skills/foo/SKILL.md",
            Some("reference.md"),
        );
        let mut context = TranscriptContext::default();
        assert!(parse_codex_uses(&text, &mut context).is_empty());
    }

    #[test]
    fn skills_list_gives_nothing() {
        let text = r#"{"type":"response_item","timestamp":"2026-09-16T12:00:00Z","payload":{"type":"function_call","namespace":"skills","name":"list","arguments":"{}"}}"#;
        let mut context = TranscriptContext::default();
        assert!(parse_codex_uses(text, &mut context).is_empty());
    }

    #[test]
    fn older_exec_command_function_call_gives_a_file_read() {
        let text = r#"{"type":"response_item","timestamp":"2026-09-16T12:00:00Z","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\": \"cat /u/.codex/skills/foo/SKILL.md\"}"}}"#;
        let mut context = TranscriptContext::default();
        let uses = parse_codex_uses(text, &mut context);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
    }

    #[test]
    fn older_shell_function_call_with_bash_lc_array_gives_a_file_read() {
        let text = r#"{"type":"response_item","timestamp":"2026-09-16T12:00:00Z","payload":{"type":"function_call","name":"shell","arguments":"{\"command\": [\"bash\", \"-lc\", \"cat /u/.codex/skills/foo/SKILL.md\"]}"}}"#;
        let mut context = TranscriptContext::default();
        let uses = parse_codex_uses(text, &mut context);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].skill, "foo");
        assert_eq!(uses[0].trigger, SkillTrigger::FileRead);
    }

    #[test]
    fn bad_json_line_is_skipped() {
        let mut context = TranscriptContext::default();
        assert!(parse_codex_uses("{\"type\":\"session_meta\" not json", &mut context).is_empty());
    }

    #[test]
    fn line_without_timestamp_is_skipped() {
        let text = r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<skill>\n<name>foo</name>\n<path>/u/.codex/skills/foo/SKILL.md</path>"}]}}"#;
        let mut context = TranscriptContext::default();
        assert!(parse_codex_uses(text, &mut context).is_empty());
    }

    #[test]
    fn context_is_kept_between_two_calls() {
        let mut context = TranscriptContext::default();
        let first = session_meta_line("s1", "/proj-a");
        assert!(parse_codex_uses(&first, &mut context).is_empty());

        let second = skill_block_line("2026-09-16T12:00:00Z", "foo");
        let uses = parse_codex_uses(&second, &mut context);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].session.as_deref(), Some("s1"));
        assert_eq!(uses[0].project_path.as_deref(), Some("/proj-a"));
    }
}
