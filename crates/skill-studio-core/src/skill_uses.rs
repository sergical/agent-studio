//! Skill uses: records of a harness running a skill, read from that
//! harness's own session history, plus the per-skill counts the app shows.
//!
//! This module holds no filesystem access; a host adapter reads the raw
//! session history and hands this module the parsed records.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::discovery_sources::DiscoverySources;

mod opencode;
pub use opencode::{
    parse_opencode_message, parse_opencode_part, OpenCodeMessageRow, OpenCodePartRow,
};

/// How a skill use started.
#[derive(
    Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SkillTrigger {
    /// The user typed the skill's command.
    User,
    /// The model called a skill tool.
    Agent,
    /// The model read the skill's `SKILL.md` without a skill tool.
    FileRead,
}

/// One recorded skill use from a harness's own session history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SkillInvocation {
    /// The skill's name, as recorded by the harness (may carry a
    /// `prefix:base` plugin qualifier).
    pub skill: String,
    /// Which harness recorded this use - an [`AgentId`](crate::identity::AgentId)
    /// wire name, e.g. [`AgentId::CLAUDE_CODE`](crate::identity::AgentId::CLAUDE_CODE).
    pub harness: String,
    /// How the use started.
    pub trigger: SkillTrigger,
    /// When the use happened.
    pub at: DateTime<Utc>,
    /// The project directory the use happened in, if the harness recorded one.
    pub project_path: Option<String>,
    /// The harness's own session id, used to dedupe file reads. Not every
    /// harness records one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// Use counts by [`SkillTrigger`], over a rolling window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillTriggerCounts {
    /// Uses the user typed.
    pub user: u32,
    /// Uses the model called as a tool.
    pub agent: u32,
    /// Uses the model triggered by reading `SKILL.md` directly.
    pub file_read: u32,
}

/// Per-skill use summary sent to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillInvocationStats {
    /// The skill's name.
    pub skill: String,
    /// Total counted uses across every cached transcript.
    pub total: u32,
    /// Counted uses in the last 24 hours.
    pub last_24_hours: u32,
    /// Counted uses in the last 7 days.
    pub last_7_days: u32,
    /// Counted uses in the last 14 days.
    pub last_14_days: u32,
    /// Counted uses in the last 30 days.
    pub last_30_days: u32,
    /// The most recent use's timestamp, RFC 3339.
    pub last_used: Option<String>,
    /// Use counts by full project path, over the last 30 days only.
    pub by_project_30_days: BTreeMap<String, u32>,
    /// Per-day use counts, "YYYY-MM-DD" (UTC), over the last 365 days.
    pub by_day: BTreeMap<String, u32>,
    /// Use counts by harness id, over the last 30 days only.
    pub by_harness_30_days: BTreeMap<String, u32>,
    /// Use counts by trigger, over the last 30 days only.
    pub by_trigger_30_days: SkillTriggerCounts,
}

/// Per-day use counts for the heatmap (date "YYYY-MM-DD" -> count).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct InvocationHeatmap {
    /// Counted uses per day.
    pub days: BTreeMap<String, u32>,
}

/// Which recorded uses count toward [`skill_stats`] and [`skill_heatmap`].
pub struct SkillUseFilter<'a> {
    /// Installed skill names; a `User` or `FileRead` use whose skill isn't
    /// here (or isn't the `base` of a `prefix:base` name here) is dropped.
    pub known_skills: &'a BTreeSet<String>,
    /// Per-harness switches; a use whose harness is switched off is dropped.
    pub sources: &'a DiscoverySources,
}

/// True when `skill` is in `known_skills`, or is a `prefix:base` name whose
/// `base` (the part after the last `:`) is in `known_skills`.
fn is_known_skill(skill: &str, known_skills: &BTreeSet<String>) -> bool {
    if known_skills.contains(skill) {
        return true;
    }
    match skill.rsplit_once(':') {
        Some((_, base)) => known_skills.contains(base),
        None => false,
    }
}

/// The subset of `uses` that count under `filter`, applying the enabled-harness,
/// known-skill, and file-read-dedupe rules shared by [`skill_stats`] and
/// [`skill_heatmap`].
fn counted_uses<'a>(
    uses: impl IntoIterator<Item = &'a SkillInvocation>,
    filter: &SkillUseFilter,
) -> Vec<&'a SkillInvocation> {
    let candidates: Vec<&SkillInvocation> = uses
        .into_iter()
        .filter(|use_| filter.sources.is_enabled(&use_.harness))
        .filter(|use_| {
            use_.trigger == SkillTrigger::Agent || is_known_skill(&use_.skill, filter.known_skills)
        })
        .collect();

    let (file_reads, mut kept): (Vec<&SkillInvocation>, Vec<&SkillInvocation>) = candidates
        .into_iter()
        .partition(|use_| use_.trigger == SkillTrigger::FileRead);

    // File-read dedupe: keyed by (harness, session, skill). Drop a
    // `FileRead` when a counted `User` or `Agent` use shares its key.
    let non_file_read_keys: BTreeSet<(&str, &str, &str)> = kept
        .iter()
        .filter_map(|use_| {
            use_.session
                .as_deref()
                .map(|session| (use_.harness.as_str(), session, use_.skill.as_str()))
        })
        .collect();

    // Among the remaining file reads, keep only the earliest per key. A
    // `FileRead` with `session: None` is never deduped against anything.
    let mut earliest_file_read: BTreeMap<(&str, &str, &str), &SkillInvocation> = BTreeMap::new();
    for use_ in &file_reads {
        let Some(session) = use_.session.as_deref() else {
            kept.push(use_);
            continue;
        };
        let key = (use_.harness.as_str(), session, use_.skill.as_str());
        if non_file_read_keys.contains(&key) {
            continue;
        }
        earliest_file_read
            .entry(key)
            .and_modify(|earliest| {
                if use_.at < earliest.at {
                    *earliest = use_;
                }
            })
            .or_insert(use_);
    }
    kept.extend(earliest_file_read.into_values());

    kept
}

/// Per-skill use totals across `uses`, with the rolling windows
/// (24h/7d/14d/30d, `by_project_30_days`, `by_day`, `by_harness_30_days`,
/// `by_trigger_30_days`) computed relative to `now` rather than the wall
/// clock. Output is sorted by skill name.
pub fn skill_stats<'a>(
    uses: impl IntoIterator<Item = &'a SkillInvocation>,
    filter: &SkillUseFilter,
    now: DateTime<Utc>,
) -> Vec<SkillInvocationStats> {
    struct Acc {
        total: u32,
        last_24_hours: u32,
        last_7_days: u32,
        last_14_days: u32,
        last_30_days: u32,
        last_used: Option<DateTime<Utc>>,
        by_project_30_days: BTreeMap<String, u32>,
        by_day: BTreeMap<String, u32>,
        by_harness_30_days: BTreeMap<String, u32>,
        by_trigger_30_days: SkillTriggerCounts,
    }

    let cutoff_24h = now - chrono::Duration::hours(24);
    let cutoff_7 = now - chrono::Duration::days(7);
    let cutoff_14 = now - chrono::Duration::days(14);
    let cutoff_30 = now - chrono::Duration::days(30);
    let cutoff_365 = now - chrono::Duration::days(365);
    let mut by_skill: BTreeMap<String, Acc> = BTreeMap::new();

    for use_ in counted_uses(uses, filter) {
        let acc = by_skill.entry(use_.skill.clone()).or_insert(Acc {
            total: 0,
            last_24_hours: 0,
            last_7_days: 0,
            last_14_days: 0,
            last_30_days: 0,
            last_used: None,
            by_project_30_days: BTreeMap::new(),
            by_day: BTreeMap::new(),
            by_harness_30_days: BTreeMap::new(),
            by_trigger_30_days: SkillTriggerCounts::default(),
        });
        acc.total += 1;
        if use_.at >= cutoff_24h {
            acc.last_24_hours += 1;
        }
        if use_.at >= cutoff_7 {
            acc.last_7_days += 1;
        }
        if use_.at >= cutoff_14 {
            acc.last_14_days += 1;
        }
        if acc.last_used.is_none_or(|last| use_.at > last) {
            acc.last_used = Some(use_.at);
        }
        if use_.at >= cutoff_30 {
            acc.last_30_days += 1;
            if let Some(project) = &use_.project_path {
                *acc.by_project_30_days.entry(project.clone()).or_insert(0) += 1;
            }
            *acc.by_harness_30_days
                .entry(use_.harness.clone())
                .or_insert(0) += 1;
            match use_.trigger {
                SkillTrigger::User => acc.by_trigger_30_days.user += 1,
                SkillTrigger::Agent => acc.by_trigger_30_days.agent += 1,
                SkillTrigger::FileRead => acc.by_trigger_30_days.file_read += 1,
            }
        }
        if use_.at >= cutoff_365 {
            let day = use_.at.format("%Y-%m-%d").to_string();
            *acc.by_day.entry(day).or_insert(0) += 1;
        }
    }

    by_skill
        .into_iter()
        .map(|(skill, acc)| SkillInvocationStats {
            skill,
            total: acc.total,
            last_24_hours: acc.last_24_hours,
            last_7_days: acc.last_7_days,
            last_14_days: acc.last_14_days,
            last_30_days: acc.last_30_days,
            last_used: acc.last_used.map(|at| at.to_rfc3339()),
            by_project_30_days: acc.by_project_30_days,
            by_day: acc.by_day,
            by_harness_30_days: acc.by_harness_30_days,
            by_trigger_30_days: acc.by_trigger_30_days,
        })
        .collect()
}

/// Per-day use counts over the last `days` days, relative to `now`.
pub fn skill_heatmap<'a>(
    uses: impl IntoIterator<Item = &'a SkillInvocation>,
    filter: &SkillUseFilter,
    days: u32,
    now: DateTime<Utc>,
) -> InvocationHeatmap {
    let cutoff = now - chrono::Duration::days(days as i64);
    let mut result = BTreeMap::new();
    for use_ in counted_uses(uses, filter) {
        if use_.at < cutoff {
            continue;
        }
        let day = use_.at.format("%Y-%m-%d").to_string();
        *result.entry(day).or_insert(0) += 1;
    }
    InvocationHeatmap { days: result }
}

/// The skill name in a path that ends `/skills/<name>/SKILL.md` or
/// `/skill/<name>/SKILL.md`. Used to recognize a plain file read of a
/// skill's own doc as a use of that skill.
pub fn skill_name_from_skill_md_path(path: &str) -> Option<&str> {
    let segments: Vec<&str> = path.split('/').collect();
    let len = segments.len();
    if len < 3 || segments[len - 1] != "SKILL.md" {
        return None;
    }
    let name = segments[len - 2];
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    match segments[len - 3] {
        "skills" | "skill" => Some(name),
        _ => None,
    }
}

/// Fast-path substrings a line must contain before it's worth a full JSON
/// parse: a `Skill` tool_use, or a typed command block.
const SKILL_TOOL_MARKER: &str = "\"name\":\"Skill\"";
const COMMAND_MARKER: &str = "<command-name>";

/// Parses one Claude Code transcript's text (newline-delimited JSON) into
/// skill uses: an `Agent` use per `Skill` tool_use block, and a `User` use
/// per typed slash command line. Never panics: a malformed line, a missing
/// timestamp, or an unrecognized shape is skipped rather than failing the
/// whole file.
pub fn parse_claude_code_uses(text: &str) -> Vec<SkillInvocation> {
    let mut out = Vec::new();

    for line in text.lines() {
        if !line.contains(SKILL_TOOL_MARKER) && !line.contains(COMMAND_MARKER) {
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
                if block.get("name").and_then(|v| v.as_str()) != Some("Skill") {
                    continue;
                }
                let Some(skill) = block
                    .get("input")
                    .and_then(|i| i.get("skill"))
                    .and_then(|v| v.as_str())
                else {
                    continue;
                };
                out.push(SkillInvocation {
                    skill: skill.to_string(),
                    harness: crate::identity::AgentId::CLAUDE_CODE.to_string(),
                    trigger: SkillTrigger::Agent,
                    at,
                    project_path: project_path.clone(),
                    session: session.clone(),
                });
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
            out.push(SkillInvocation {
                skill: name.to_string(),
                harness: crate::identity::AgentId::CLAUDE_CODE.to_string(),
                trigger: SkillTrigger::User,
                at,
                project_path: project_path.clone(),
                session: session.clone(),
            });
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

    fn known(skills: &[&str]) -> BTreeSet<String> {
        skills.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn skill_name_from_skill_md_path_rules() {
        assert_eq!(
            skill_name_from_skill_md_path("/Users/me/.claude/skills/foo/SKILL.md"),
            Some("foo")
        );
        assert_eq!(
            skill_name_from_skill_md_path(".config/opencode/skill/foo/SKILL.md"),
            Some("foo")
        );
        assert_eq!(
            skill_name_from_skill_md_path(
                "/home/me/.codex/plugins/cache/m/p/1.0/skills/foo/SKILL.md"
            ),
            Some("foo")
        );
        assert_eq!(skill_name_from_skill_md_path("/x/foo/SKILL.md"), None);
        assert_eq!(skill_name_from_skill_md_path("/x/skills/SKILL.md"), None);
        assert_eq!(
            skill_name_from_skill_md_path("/x/skills/foo/bar/SKILL.md"),
            None
        );
        assert_eq!(
            skill_name_from_skill_md_path("/x/skills/foo/skill.md"),
            None
        );
        assert_eq!(
            skill_name_from_skill_md_path("/x/skills/foo/SKILL.md.bak"),
            None
        );
    }

    fn filter<'a>(
        known_skills: &'a BTreeSet<String>,
        sources: &'a DiscoverySources,
    ) -> SkillUseFilter<'a> {
        SkillUseFilter {
            known_skills,
            sources,
        }
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

    #[test]
    fn typed_unknown_command_is_dropped_but_unknown_agent_use_is_kept() {
        let known_skills = known(&["deploy"]);
        let sources = DiscoverySources::default();
        let clear = SkillInvocation {
            skill: "clear".to_string(),
            harness: crate::identity::AgentId::CLAUDE_CODE.to_string(),
            trigger: SkillTrigger::User,
            at: Utc::now(),
            project_path: None,
            session: None,
        };
        let unknown_agent_use = SkillInvocation {
            skill: "mystery".to_string(),
            harness: crate::identity::AgentId::CLAUDE_CODE.to_string(),
            trigger: SkillTrigger::Agent,
            at: Utc::now(),
            project_path: None,
            session: None,
        };
        let known_plugin_use = SkillInvocation {
            skill: "plugin:deploy".to_string(),
            harness: crate::identity::AgentId::CLAUDE_CODE.to_string(),
            trigger: SkillTrigger::User,
            at: Utc::now(),
            project_path: None,
            session: None,
        };
        let uses = [clear, unknown_agent_use.clone(), known_plugin_use.clone()];
        let stats = skill_stats(&uses, &filter(&known_skills, &sources), Utc::now());
        let names: BTreeSet<&str> = stats.iter().map(|s| s.skill.as_str()).collect();
        assert_eq!(names, BTreeSet::from(["mystery", "plugin:deploy"]));
    }

    #[test]
    fn a_switched_off_harness_is_dropped() {
        let known_skills = known(&["deploy"]);
        let mut sources = DiscoverySources::default();
        sources.set("claude-code", false);
        let use_ = SkillInvocation {
            skill: "deploy".to_string(),
            harness: crate::identity::AgentId::CLAUDE_CODE.to_string(),
            trigger: SkillTrigger::User,
            at: Utc::now(),
            project_path: None,
            session: None,
        };
        let stats = skill_stats(&[use_], &filter(&known_skills, &sources), Utc::now());
        assert!(stats.is_empty());
    }

    fn agent_use(skill: &str, session: Option<&str>, at: DateTime<Utc>) -> SkillInvocation {
        SkillInvocation {
            skill: skill.to_string(),
            harness: crate::identity::AgentId::CLAUDE_CODE.to_string(),
            trigger: SkillTrigger::Agent,
            at,
            project_path: None,
            session: session.map(|s| s.to_string()),
        }
    }

    fn file_read_use(skill: &str, session: Option<&str>, at: DateTime<Utc>) -> SkillInvocation {
        SkillInvocation {
            skill: skill.to_string(),
            harness: crate::identity::AgentId::CLAUDE_CODE.to_string(),
            trigger: SkillTrigger::FileRead,
            at,
            project_path: None,
            session: session.map(|s| s.to_string()),
        }
    }

    #[test]
    fn file_read_dedupe_rules() {
        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        let now = Utc::now();

        // agent use + file read in the same session -> only the agent use.
        let uses = [
            agent_use("write-tests", Some("s1"), now),
            file_read_use("write-tests", Some("s1"), now),
        ];
        let stats = skill_stats(&uses, &filter(&known_skills, &sources), now);
        assert_eq!(stats[0].total, 1);

        // two file reads in one session -> one.
        let uses = [
            file_read_use(
                "write-tests",
                Some("s1"),
                now - chrono::Duration::minutes(5),
            ),
            file_read_use("write-tests", Some("s1"), now),
        ];
        let stats = skill_stats(&uses, &filter(&known_skills, &sources), now);
        assert_eq!(stats[0].total, 1);

        // the same skill in two sessions -> two.
        let uses = [
            file_read_use("write-tests", Some("s1"), now),
            file_read_use("write-tests", Some("s2"), now),
        ];
        let stats = skill_stats(&uses, &filter(&known_skills, &sources), now);
        assert_eq!(stats[0].total, 2);

        // session: None reads are each kept.
        let uses = [
            file_read_use("write-tests", None, now),
            file_read_use("write-tests", None, now),
        ];
        let stats = skill_stats(&uses, &filter(&known_skills, &sources), now);
        assert_eq!(stats[0].total, 2);
    }

    #[test]
    fn by_harness_and_by_trigger_only_count_the_last_30_days() {
        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        let now = Utc::now();
        let recent = agent_use("write-tests", None, now);
        let mut old = agent_use("write-tests", None, now - chrono::Duration::days(31));
        old.session = None;
        let uses = [recent, old];
        let stats = skill_stats(&uses, &filter(&known_skills, &sources), now);
        assert_eq!(stats[0].total, 2);
        assert_eq!(
            stats[0]
                .by_harness_30_days
                .get(crate::identity::AgentId::CLAUDE_CODE),
            Some(&1)
        );
        assert_eq!(stats[0].by_trigger_30_days.agent, 1);
    }

    #[test]
    fn stats_at_windows_are_relative_to_the_given_now() {
        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        let now = Utc::now();
        let twenty_five_hours_ago =
            agent_use("write-tests", None, now - chrono::Duration::hours(25));
        let stats = skill_stats(
            &[twenty_five_hours_ago],
            &filter(&known_skills, &sources),
            now,
        );
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].last_24_hours, 0, "25h-old use counted in 24h");
        assert_eq!(stats[0].last_7_days, 1, "25h-old use missing from 7d");
    }

    #[test]
    fn stats_totals_last_30_days_and_by_project_30_days() {
        let known_skills = known(&["write-tests"]);
        let sources = DiscoverySources::default();
        let now = Utc::now();
        let old = now - chrono::Duration::days(60);
        let uses = [
            SkillInvocation {
                skill: "write-tests".to_string(),
                harness: crate::identity::AgentId::CLAUDE_CODE.to_string(),
                trigger: SkillTrigger::Agent,
                at: now,
                project_path: Some("/proj-a".to_string()),
                session: None,
            },
            SkillInvocation {
                skill: "write-tests".to_string(),
                harness: crate::identity::AgentId::CLAUDE_CODE.to_string(),
                trigger: SkillTrigger::Agent,
                at: now,
                project_path: Some("/proj-b".to_string()),
                session: None,
            },
            SkillInvocation {
                skill: "write-tests".to_string(),
                harness: crate::identity::AgentId::CLAUDE_CODE.to_string(),
                trigger: SkillTrigger::Agent,
                at: old,
                project_path: Some("/proj-a".to_string()),
                session: None,
            },
        ];
        let stats = skill_stats(&uses, &filter(&known_skills, &sources), now);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].total, 3);
        assert_eq!(stats[0].last_30_days, 2);
        assert_eq!(stats[0].by_project_30_days.get("/proj-a"), Some(&1));
        assert_eq!(stats[0].by_project_30_days.get("/proj-b"), Some(&1));
        let today = now.format("%Y-%m-%d").to_string();
        assert_eq!(stats[0].by_day.get(&today), Some(&2));
    }

    #[test]
    fn heatmap_buckets_by_day_and_applies_the_filter() {
        let known_skills = known(&["write-tests", "lint-code"]);
        let mut sources = DiscoverySources::default();
        let now = Utc::now();
        let uses = [
            agent_use("write-tests", None, now),
            agent_use("lint-code", None, now),
        ];
        let heatmap = skill_heatmap(&uses, &filter(&known_skills, &sources), 30, now);
        assert_eq!(heatmap.days.len(), 1);
        assert_eq!(*heatmap.days.values().next().unwrap(), 2);

        sources.set("claude-code", false);
        let heatmap = skill_heatmap(&uses, &filter(&known_skills, &sources), 30, now);
        assert!(heatmap.days.is_empty());
    }
}
