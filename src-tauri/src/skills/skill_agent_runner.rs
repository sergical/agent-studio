// ============================================================================
// Skills Module - Local Harness Runner
// Spawns a harness CLI (Claude Code, Codex, OpenCode, or pi) as a child
// process for one skill, parses its streaming JSON-lines output into a
// shared `SkillAgentEventKind`, and emits one `SkillAgentEvent` per line on
// "skill-agent://event". Also creates/removes the scratch directories these
// runs execute in, so a run only ever sees the one skill under test.
// ============================================================================

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Notify;

/// Event name every `SkillAgentEvent` is emitted on.
pub const SKILL_AGENT_EVENT: &str = "skill-agent://event";

/// stderr kept per run, for the error message when a harness exits non-zero
/// without ever producing final text.
const STDERR_TAIL_LEN: usize = 4096;

/// Hard cap on a single stdout line. A runaway harness that never emits a
/// newline would otherwise grow this run's buffer without bound.
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

/// How long a cancelled run gets to exit after SIGTERM before it's SIGKILLed.
const CANCEL_GRACE: Duration = Duration::from_secs(2);

/// One of the four first-class agents a skill run can target.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessId {
    ClaudeCode,
    Codex,
    OpenCode,
    Pi,
}

impl HarnessId {
    /// The binary name resolved on `PATH` to start this harness.
    pub fn bin_name(&self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
        }
    }
}

/// Whether a run may write to its scratch directory or only read it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WriteAccess {
    ReadOnly,
    Workspace,
}

/// Whether the run loaded the skill under test, inferred per-harness from
/// its tool-call events - see `parse_line`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillLoaded {
    Yes,
    No,
    #[default]
    Unknown,
}

/// Request to start one headless harness run against one skill.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillAgentRunRequest {
    pub harness: HarnessId,
    pub prompt: String,
    pub cwd: String,
    pub skill_name: String,
    pub write_access: WriteAccess,
    /// Set to continue an earlier run's session/thread.
    pub session_id: Option<String>,
}

/// One line of a run's transcript, in emission order (`seq`). Deserialize is
/// needed alongside Serialize because `skill_run_history` round-trips events
/// through its `.events.jsonl` transcript files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillAgentEvent {
    pub run_id: String,
    pub seq: u64,
    pub at: DateTime<Utc>,
    pub kind: SkillAgentEventKind,
}

/// The discriminated union of everything a run can report.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillAgentEventKind {
    Started {
        command: String,
        session_id: Option<String>,
    },
    AssistantText {
        text: String,
        is_delta: bool,
    },
    ToolCall {
        name: String,
        summary: String,
        detail: Option<String>,
    },
    ToolResult {
        name: String,
        summary: String,
    },
    Finished {
        ok: bool,
        final_text: String,
        session_id: Option<String>,
        cost_usd: Option<f64>,
        duration_ms: u64,
        skill_loaded: SkillLoaded,
    },
    Error {
        message: String,
    },
}

/// Per-run state `parse_line` accumulates as it walks a harness's lines:
/// the session/thread id (for follow-ups), the latest assistant text seen,
/// whether the skill under test was loaded, and Codex's last completed
/// `agent_message` (its closest thing to a running "final text").
#[derive(Debug, Default)]
pub struct ParseState {
    pub session_id: Option<String>,
    pub last_text: String,
    pub skill_loaded: SkillLoaded,
    pub last_agent_message: Option<String>,
}

/// Build the `(program, args, env, cwd)` to spawn for `request`, per the
/// per-harness CLI shapes observed in the field - see the module doc in the
/// spec this was built from. Pure: takes the resolved binary as input rather
/// than resolving it itself, so it's unit-testable without a real harness.
pub fn build_command(
    request: &SkillAgentRunRequest,
    binary: &Path,
) -> (String, Vec<String>, Vec<(String, String)>, PathBuf) {
    let program = binary.to_string_lossy().to_string();
    let cwd = PathBuf::from(&request.cwd);
    let env = Vec::new();

    let args = match request.harness {
        HarnessId::ClaudeCode => {
            let mut args = vec![
                "-p".to_string(),
                request.prompt.clone(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--verbose".to_string(),
                "--permission-mode".to_string(),
            ];
            match request.write_access {
                WriteAccess::ReadOnly => {
                    args.push("default".to_string());
                    args.push("--allowedTools".to_string());
                    args.push(
                        "Read,Grep,Glob,Bash(ls *),Bash(cat *),Bash(git diff *),Bash(git status *)"
                            .to_string(),
                    );
                }
                WriteAccess::Workspace => args.push("auto".to_string()),
            }
            if let Some(session_id) = &request.session_id {
                args.push("--resume".to_string());
                args.push(session_id.clone());
            }
            args
        }
        HarnessId::Codex => {
            let mode = match request.write_access {
                WriteAccess::ReadOnly => "read-only",
                WriteAccess::Workspace => "workspace-write",
            };
            let mut args = vec!["exec".to_string()];
            if let Some(thread_id) = &request.session_id {
                args.push("resume".to_string());
                args.push(thread_id.clone());
                args.push("--json".to_string());
            } else {
                args.push("--json".to_string());
                args.push("--skip-git-repo-check".to_string());
                args.push("-s".to_string());
                args.push(mode.to_string());
                args.push("-C".to_string());
                args.push(request.cwd.clone());
            }
            args.push(request.prompt.clone());
            args
        }
        HarnessId::OpenCode => {
            let mut args = vec![
                "run".to_string(),
                "--format".to_string(),
                "json".to_string(),
            ];
            if request.write_access == WriteAccess::Workspace {
                args.push("--auto".to_string());
            }
            args.push("--dir".to_string());
            args.push(request.cwd.clone());
            if let Some(session_id) = &request.session_id {
                args.push("--session".to_string());
                args.push(session_id.clone());
            }
            args.push(request.prompt.clone());
            args
        }
        HarnessId::Pi => {
            let mut args = vec!["-p".to_string(), "--mode".to_string(), "json".to_string()];
            if request.write_access == WriteAccess::ReadOnly {
                args.push("--tools".to_string());
                args.push("read,grep,find,ls".to_string());
            }
            if let Some(session_id) = &request.session_id {
                args.push("--session".to_string());
                args.push(session_id.clone());
            }
            args.push(request.prompt.clone());
            args
        }
    };

    (program, args, env, cwd)
}

/// Parse one line of a harness's stdout into zero or more transcript events,
/// updating `state` along the way. Unrecognized or malformed lines parse to
/// no events, matching the "ignore what we don't understand" stance the
/// observed formats require (hook/rate-limit lines, reasoning steps, etc).
pub fn parse_line(
    harness: HarnessId,
    line: &str,
    skill_name: &str,
    state: &mut ParseState,
) -> Vec<SkillAgentEventKind> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return Vec::new();
    };

    match harness {
        HarnessId::ClaudeCode => parse_claude_line(&value, skill_name, state),
        HarnessId::Codex => parse_codex_line(&value, skill_name, state),
        HarnessId::OpenCode => parse_open_code_line(&value, state),
        HarnessId::Pi => parse_pi_line(&value, skill_name, state),
    }
}

fn parse_claude_line(
    value: &Value,
    skill_name: &str,
    state: &mut ParseState,
) -> Vec<SkillAgentEventKind> {
    let mut events = Vec::new();
    match value.get("type").and_then(Value::as_str).unwrap_or("") {
        "system" => {
            if let Some(session_id) = value.get("session_id").and_then(Value::as_str) {
                state.session_id = Some(session_id.to_string());
            }
        }
        "assistant" => {
            if let Some(blocks) = value.pointer("/message/content").and_then(Value::as_array) {
                for block in blocks {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(text) = block.get("text").and_then(Value::as_str) {
                                state.last_text = text.to_string();
                                events.push(SkillAgentEventKind::AssistantText {
                                    text: text.to_string(),
                                    is_delta: false,
                                });
                            }
                        }
                        Some("tool_use") => {
                            let name = block
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("tool")
                                .to_string();
                            let input = block.get("input").cloned().unwrap_or(Value::Null);
                            if name == "Skill" {
                                let loaded_this_skill =
                                    input.get("skill").and_then(Value::as_str) == Some(skill_name);
                                if loaded_this_skill {
                                    state.skill_loaded = SkillLoaded::Yes;
                                } else if state.skill_loaded == SkillLoaded::Unknown {
                                    state.skill_loaded = SkillLoaded::No;
                                }
                            }
                            let summary = input
                                .get("skill")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .unwrap_or_else(|| input.to_string());
                            events.push(SkillAgentEventKind::ToolCall {
                                name,
                                summary,
                                detail: Some(input.to_string()),
                            });
                        }
                        // "thinking" and anything else observed here are ignored.
                        _ => {}
                    }
                }
            }
        }
        "user" => {
            if let Some(blocks) = value.pointer("/message/content").and_then(Value::as_array) {
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                        let summary = block
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        events.push(SkillAgentEventKind::ToolResult {
                            name: "tool_result".to_string(),
                            summary,
                        });
                    }
                }
            }
        }
        "result" => {
            let is_error = value
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let final_text = value
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| state.last_text.clone());
            let session_id = value
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| state.session_id.clone());
            if let Some(session_id) = &session_id {
                state.session_id = Some(session_id.clone());
            }
            events.push(SkillAgentEventKind::Finished {
                ok: !is_error,
                final_text,
                session_id,
                cost_usd: value.get("total_cost_usd").and_then(Value::as_f64),
                duration_ms: value
                    .get("duration_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                skill_loaded: state.skill_loaded,
            });
        }
        // "system/hook_*", "rate_limit_event": explicitly ignored per the
        // observed shapes; anything else unrecognized is ignored too.
        _ => {}
    }
    events
}

fn parse_codex_line(
    value: &Value,
    skill_name: &str,
    state: &mut ParseState,
) -> Vec<SkillAgentEventKind> {
    let mut events = Vec::new();
    match value.get("type").and_then(Value::as_str).unwrap_or("") {
        "thread.started" => {
            if let Some(thread_id) = value.get("thread_id").and_then(Value::as_str) {
                state.session_id = Some(thread_id.to_string());
            }
        }
        "item.completed" => {
            if let Some(item) = value.get("item") {
                match item.get("type").and_then(Value::as_str).unwrap_or("") {
                    "agent_message" => {
                        if let Some(text) = item.get("text").and_then(Value::as_str) {
                            state.last_agent_message = Some(text.to_string());
                            events.push(SkillAgentEventKind::AssistantText {
                                text: text.to_string(),
                                is_delta: false,
                            });
                        }
                    }
                    "command_execution" => {
                        let command = item
                            .get("command")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        if command.contains(&format!("/{skill_name}/SKILL.md")) {
                            state.skill_loaded = SkillLoaded::Yes;
                        }
                        let detail = item
                            .get("aggregated_output")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        events.push(SkillAgentEventKind::ToolCall {
                            name: "command_execution".to_string(),
                            summary: command,
                            detail,
                        });
                    }
                    "file_change" => {
                        events.push(SkillAgentEventKind::ToolCall {
                            name: "file_change".to_string(),
                            summary: "file change".to_string(),
                            detail: Some(item.to_string()),
                        });
                    }
                    "error" => {
                        let message = item
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("Unknown error")
                            .to_string();
                        events.push(SkillAgentEventKind::Error { message });
                    }
                    // "reasoning" and anything else observed here are ignored.
                    _ => {}
                }
            }
        }
        // "turn.started"/"turn.completed"/"turn.failed"/"item.started": the
        // runner derives Finished from process exit plus accumulated state
        // instead, so these carry nothing new to surface here.
        _ => {}
    }
    events
}

fn parse_pi_line(
    value: &Value,
    skill_name: &str,
    state: &mut ParseState,
) -> Vec<SkillAgentEventKind> {
    let mut events = Vec::new();
    match value.get("type").and_then(Value::as_str).unwrap_or("") {
        "session" => {
            if let Some(id) = value.get("id").and_then(Value::as_str) {
                state.session_id = Some(id.to_string());
            }
        }
        "message_update" => {
            let is_text_delta = value
                .pointer("/assistantMessageEvent/type")
                .and_then(Value::as_str)
                == Some("text_delta");
            if is_text_delta {
                if let Some(delta) = value
                    .pointer("/assistantMessageEvent/delta")
                    .and_then(Value::as_str)
                {
                    state.last_text.push_str(delta);
                    events.push(SkillAgentEventKind::AssistantText {
                        text: delta.to_string(),
                        is_delta: true,
                    });
                }
            }
        }
        "tool_execution_start" => {
            let tool_name = value
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let args = value.get("args").cloned().unwrap_or(Value::Null);
            if tool_name == "read" {
                let path = args.get("path").and_then(Value::as_str).unwrap_or("");
                let needle = format!("/{skill_name}/SKILL.md");
                if path.ends_with(&needle) || path.contains(&needle) {
                    state.skill_loaded = SkillLoaded::Yes;
                }
            }
            events.push(SkillAgentEventKind::ToolCall {
                name: tool_name,
                summary: args.to_string(),
                detail: None,
            });
        }
        "tool_execution_end" => {
            let tool_name = value
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let result = value.get("result").cloned().unwrap_or(Value::Null);
            events.push(SkillAgentEventKind::ToolResult {
                name: tool_name,
                summary: result.to_string(),
            });
        }
        "turn_end" => {
            if let Some(blocks) = value.pointer("/message/content").and_then(Value::as_array) {
                let text: String = blocks
                    .iter()
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect();
                if !text.is_empty() {
                    // Not re-emitted as an event: the same text already streamed
                    // as `text_delta`s above, so this only updates `last_text`
                    // for `Finished`'s `final_text`.
                    state.last_text = text;
                }
            }
        }
        // "agent_end"/"agent_settled": the runner derives Finished from
        // process exit plus accumulated state instead.
        _ => {}
    }
    events
}

/// OpenCode's format errors out before running on this machine, so this
/// parses best-effort: any `text` string nested under a `part`/`content`
/// object is assistant text, and any object naming a `tool`/`toolName` is a
/// tool call. Skill-loaded detection isn't possible from this alone, so it
/// always stays `Unknown`.
fn parse_open_code_line(value: &Value, state: &mut ParseState) -> Vec<SkillAgentEventKind> {
    let mut events = Vec::new();
    if let Some(text) = find_open_code_text(value, false) {
        state.last_text = text.clone();
        events.push(SkillAgentEventKind::AssistantText {
            text,
            is_delta: false,
        });
    }
    if let Some((name, summary)) = find_open_code_tool(value) {
        events.push(SkillAgentEventKind::ToolCall {
            name,
            summary,
            detail: None,
        });
    }
    events
}

fn find_open_code_text(value: &Value, inside_part_or_content: bool) -> Option<String> {
    match value {
        Value::Object(map) => {
            if inside_part_or_content {
                if let Some(text) = map.get("text").and_then(Value::as_str) {
                    return Some(text.to_string());
                }
            }
            map.iter().find_map(|(key, v)| {
                let nested = inside_part_or_content || key == "part" || key == "content";
                find_open_code_text(v, nested)
            })
        }
        Value::Array(items) => items
            .iter()
            .find_map(|v| find_open_code_text(v, inside_part_or_content)),
        _ => None,
    }
}

fn find_open_code_tool(value: &Value) -> Option<(String, String)> {
    match value {
        Value::Object(map) => {
            let name = map
                .get("tool")
                .and_then(Value::as_str)
                .or_else(|| map.get("toolName").and_then(Value::as_str));
            if let Some(name) = name {
                return Some((name.to_string(), value.to_string()));
            }
            map.values().find_map(find_open_code_tool)
        }
        Value::Array(items) => items.iter().find_map(find_open_code_tool),
        _ => None,
    }
}

/// A run's cancellation state, shared between `cancel_skill_agent_run` and
/// the task running it. `cancelled` makes a cancel request idempotent;
/// `cancel` is what wakes the run task's `tokio::select!` out of reading
/// stdout / waiting on the child.
#[derive(Default)]
struct RunHandle {
    cancel: Notify,
    cancelled: AtomicBool,
}

/// Where a completed run's transcript events go. A trait object rather than
/// a bare `AppHandle` so the run task is testable without a real Tauri app.
type EventSink = Box<dyn Fn(SkillAgentEvent) + Send + Sync>;

/// A run in progress, and the harnesses whose absolute binary path has
/// already been resolved once (`$SHELL -lc 'command -v <bin>'` is not
/// cheap, and the app's own `PATH` doesn't see the user's shell config).
/// `runs` is an `Arc` so the spawned run task can hold its own clone and
/// deregister itself when it finishes, without borrowing `State`.
#[derive(Default)]
pub struct SkillAgentRunnerState {
    runs: Arc<Mutex<HashMap<String, Arc<RunHandle>>>>,
    binaries: Mutex<HashMap<HarnessId, PathBuf>>,
}

/// A run id must be safe to use as a HashMap key and to log: short, and
/// drawn from a small alphabet.
fn validate_run_id(run_id: &str) -> Result<(), String> {
    if run_id.is_empty() || run_id.len() > 64 {
        return Err("Run id must be 1-64 characters".to_string());
    }
    if !run_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("Run id must contain only letters, digits, and '-'".to_string());
    }
    Ok(())
}

/// `stdout`'s last non-empty line, required to be an absolute, executable,
/// regular file. Pure and separated from `resolve_binary` so the many shapes
/// a login shell can print before/around the real path (startup banners,
/// aliases, shell functions) are unit-testable without a real shell.
fn pick_executable_line(stdout: &str, is_executable: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let last = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())?;
    if !last.starts_with('/') {
        return None;
    }
    let path = PathBuf::from(last);
    if is_executable(&path) {
        Some(path)
    } else {
        None
    }
}

fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match fs::metadata(path) {
            Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
    }
}

fn resolve_binary(harness: HarnessId, state: &SkillAgentRunnerState) -> Result<PathBuf, String> {
    if let Some(cached) = state
        .binaries
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&harness)
    {
        return Ok(cached.clone());
    }

    let bin = harness.bin_name();
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let output = std::process::Command::new(&shell)
        .arg("-lc")
        .arg(format!("command -v {bin}"))
        .output()
        .map_err(|_| format!("{bin} is not installed or not on PATH"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let path = pick_executable_line(&stdout, is_executable_file)
        .ok_or_else(|| format!("{bin} is not installed or not on PATH"))?;

    state
        .binaries
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(harness, path.clone());
    Ok(path)
}

fn emit_event(sink: &EventSink, run_id: &str, seq: &AtomicU64, kind: SkillAgentEventKind) {
    let event = SkillAgentEvent {
        run_id: run_id.to_string(),
        seq: seq.fetch_add(1, Ordering::SeqCst),
        at: Utc::now(),
        kind,
    };
    sink(event);
}

/// Reads newline-delimited stdout via `read_until`, capping any single line
/// at `MAX_LINE_BYTES` so a runaway harness can't grow this run's memory
/// without bound. A line over the cap is reported once via `on_skipped` and
/// dropped rather than parsed; a final line with no trailing newline (EOF
/// mid-record) still reaches `on_line`.
async fn read_capped_lines<R, FLine, FSkip>(
    mut reader: R,
    mut on_line: FLine,
    mut on_skipped: FSkip,
) where
    R: AsyncBufRead + Unpin,
    FLine: FnMut(String),
    FSkip: FnMut(),
{
    loop {
        let mut buf: Vec<u8> = Vec::new();
        let read = reader.read_until(b'\n', &mut buf).await;
        let Ok(n) = read else { break };
        if n == 0 {
            break; // EOF, nothing left to read
        }

        if buf.len() > MAX_LINE_BYTES {
            on_skipped();
            // `read_until` already consumed through the newline unless the
            // process closed stdout mid-line; in that case there's nothing
            // left to drain, so only keep going if we didn't end on '\n'.
            if buf.last() != Some(&b'\n') {
                loop {
                    let mut drain: Vec<u8> = Vec::new();
                    match reader.read_until(b'\n', &mut drain).await {
                        Ok(0) => break,
                        Ok(_) => {
                            if drain.last() == Some(&b'\n') {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            continue;
        }

        let had_newline = buf.last() == Some(&b'\n');
        if had_newline {
            buf.pop();
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
        }
        on_line(String::from_utf8_lossy(&buf).to_string());
        if !had_newline {
            break; // EOF without a trailing newline: nothing more to read
        }
    }
}

/// Append `chunk` to a bounded stderr tail, keeping only the last
/// `STDERR_TAIL_LEN` bytes. Byte-based (not `String`) so a cut can land
/// mid-codepoint without panicking; converted lossily once at the end.
fn push_stderr_tail(tail: &mut Vec<u8>, chunk: &[u8]) {
    tail.extend_from_slice(chunk);
    if tail.len() > STDERR_TAIL_LEN {
        let excess = tail.len() - STDERR_TAIL_LEN;
        tail.drain(0..excess);
    }
}

/// Drains `reader` to EOF, returning only the last `STDERR_TAIL_LEN` bytes
/// seen - enough for an error message without holding a whole noisy stderr.
async fn drain_stderr_tail<R>(mut reader: R) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut tail: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => push_stderr_tail(&mut tail, &chunk[..n]),
            Err(_) => break,
        }
    }
    tail
}

/// A run id unique within this process: a nanosecond timestamp plus a
/// monotonic counter, so two runs started in the same tick still differ.
/// Kept for callers that don't supply their own id (none currently do - the
/// frontend generates run ids with `crypto.randomUUID()` - but tests still
/// find this useful for scratch ids).
#[cfg(test)]
fn new_run_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!(
        "run-{}-{n}",
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    )
}

/// Start a headless harness run for one skill: resolves the binary, spawns
/// it, streams `Started`/`AssistantText`/`ToolCall`/`ToolResult`/`Error`
/// events as its stdout lines parse, and always emits exactly one
/// `Finished` when it exits (or is cancelled). `run_id` is supplied by the
/// caller (the frontend generates it before this call resolves) so the
/// frontend can start listening for it before the run exists.
#[tauri::command]
pub async fn start_skill_agent_run(
    request: SkillAgentRunRequest,
    run_id: String,
    app: AppHandle,
    state: tauri::State<'_, SkillAgentRunnerState>,
) -> Result<String, String> {
    validate_run_id(&run_id)?;

    let handle = Arc::new(RunHandle::default());
    {
        let mut runs = state.runs.lock().unwrap_or_else(|e| e.into_inner());
        if runs.contains_key(&run_id) {
            return Err(format!("A run with id {run_id} is already running"));
        }
        runs.insert(run_id.clone(), handle.clone());
    }

    let binary = match resolve_binary(request.harness, &state) {
        Ok(binary) => binary,
        Err(err) => {
            state
                .runs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&run_id);
            return Err(err);
        }
    };

    let runs = state.runs.clone();
    let task_app = app.clone();
    let sink: EventSink = Box::new(move |event| {
        let _ = task_app.emit(SKILL_AGENT_EVENT, &event);
    });

    let spawned_run_id = run_id.clone();
    tokio::spawn(run_and_deregister(
        runs,
        sink,
        spawned_run_id,
        request,
        binary,
        handle,
    ));

    Ok(run_id)
}

/// Runs one harness invocation to completion and then removes its own entry
/// from `runs`. This is the only place a run's registry entry is removed, so
/// a run that panics before this point would leak its entry - `run_skill_agent`
/// is written to avoid that (no mutex `.unwrap()`, no slicing).
async fn run_and_deregister(
    runs: Arc<Mutex<HashMap<String, Arc<RunHandle>>>>,
    sink: EventSink,
    run_id: String,
    request: SkillAgentRunRequest,
    binary: PathBuf,
    handle: Arc<RunHandle>,
) {
    run_skill_agent(&sink, &run_id, request, binary, &handle).await;
    runs.lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&run_id);
}

async fn run_skill_agent(
    sink: &EventSink,
    run_id: &str,
    request: SkillAgentRunRequest,
    binary: PathBuf,
    handle: &RunHandle,
) {
    let (program, args, env, cwd) = build_command(&request, &binary);
    run_process(
        sink,
        run_id,
        request.harness,
        &request.skill_name,
        &program,
        &args,
        &env,
        &cwd,
        request.session_id.as_deref(),
        handle,
    )
    .await;
}

/// The task that is the sole emitter of a run's events: spawns `program`,
/// races reading its stdout to completion (and waiting on it) against a
/// cancel request, and always ends with exactly one `Finished` event.
#[allow(clippy::too_many_arguments)]
async fn run_process(
    sink: &EventSink,
    run_id: &str,
    harness: HarnessId,
    skill_name: &str,
    program: &str,
    args: &[String],
    env: &[(String, String)],
    cwd: &Path,
    session_id: Option<&str>,
    handle: &RunHandle,
) {
    let command_display = format!("{program} {}", args.join(" "));
    let seq = AtomicU64::new(0);

    let mut command = Command::new(program);
    command
        .args(args)
        .envs(env.iter().cloned())
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        // Its own process group, so a cancel can signal the whole tree (a
        // harness's own child processes), not just the immediate PID.
        command.process_group(0);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            emit_event(
                sink,
                run_id,
                &seq,
                SkillAgentEventKind::Error {
                    message: format!("{} is not installed or not on PATH", harness.bin_name()),
                },
            );
            emit_event(
                sink,
                run_id,
                &seq,
                SkillAgentEventKind::Finished {
                    ok: false,
                    final_text: String::new(),
                    session_id: None,
                    cost_usd: None,
                    duration_ms: 0,
                    skill_loaded: SkillLoaded::Unknown,
                },
            );
            return;
        }
    };
    let pid = child.id();

    emit_event(
        sink,
        run_id,
        &seq,
        SkillAgentEventKind::Started {
            command: command_display,
            session_id: session_id.map(str::to_string),
        },
    );

    let started_at = Instant::now();
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(drain_stderr_tail(stderr)));

    let mut parse_state = ParseState::default();
    let mut finished_emitted = false;
    let mut had_error = false;

    let read_and_wait = async {
        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            read_capped_lines(
                reader,
                |line| {
                    for kind in parse_line(harness, &line, skill_name, &mut parse_state) {
                        match &kind {
                            SkillAgentEventKind::Finished { .. } => finished_emitted = true,
                            SkillAgentEventKind::Error { .. } => had_error = true,
                            _ => {}
                        }
                        emit_event(sink, run_id, &seq, kind);
                    }
                },
                || {
                    emit_event(
                        sink,
                        run_id,
                        &seq,
                        SkillAgentEventKind::Error {
                            message: "Skipped one output line over 4 MiB".to_string(),
                        },
                    );
                },
            )
            .await;
        }
        child.wait().await
    };

    let cancelled;
    let status = tokio::select! {
        status = read_and_wait => {
            cancelled = false;
            status
        }
        _ = handle.cancel.notified() => {
            cancelled = true;
            terminate_process_group(pid, &mut child).await
        }
    };

    let stderr_tail = match stderr_task {
        Some(task) => task.await.unwrap_or_default(),
        None => Vec::new(),
    };

    if cancelled {
        emit_event(
            sink,
            run_id,
            &seq,
            SkillAgentEventKind::Finished {
                ok: false,
                final_text: "Cancelled".to_string(),
                session_id: parse_state.session_id.clone(),
                cost_usd: None,
                duration_ms: started_at.elapsed().as_millis() as u64,
                skill_loaded: parse_state.skill_loaded,
            },
        );
        return;
    }

    if finished_emitted {
        return;
    }

    let duration_ms = started_at.elapsed().as_millis() as u64;
    let final_text = parse_state
        .last_agent_message
        .clone()
        .unwrap_or_else(|| parse_state.last_text.clone());
    let exit_ok = matches!(&status, Ok(s) if s.success());

    if !exit_ok && final_text.is_empty() {
        let tail = String::from_utf8_lossy(&stderr_tail).trim().to_string();
        let message = if !tail.is_empty() {
            tail
        } else {
            match &status {
                Ok(s) => format!("exited with code {}", s.code().unwrap_or(-1)),
                Err(e) => format!("failed to wait on process: {e}"),
            }
        };
        emit_event(sink, run_id, &seq, SkillAgentEventKind::Error { message });
        had_error = true;
    }

    emit_event(
        sink,
        run_id,
        &seq,
        SkillAgentEventKind::Finished {
            ok: exit_ok && !had_error,
            final_text,
            session_id: parse_state.session_id.clone(),
            cost_usd: None,
            duration_ms,
            skill_loaded: parse_state.skill_loaded,
        },
    );
}

/// Send SIGTERM to `pid`'s process group, give it `CANCEL_GRACE` to exit,
/// then SIGKILL the group and wait for real. On non-unix this just waits.
async fn terminate_process_group(
    pid: Option<u32>,
    child: &mut tokio::process::Child,
) -> std::io::Result<std::process::ExitStatus> {
    #[cfg(unix)]
    if let Some(pid) = pid {
        // SAFETY: `kill` with a negative pid signals the process group; no
        // pointers are involved, and a signal to an already-exited group is
        // a harmless no-op (ESRCH).
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;

    match tokio::time::timeout(CANCEL_GRACE, child.wait()).await {
        Ok(status) => status,
        Err(_) => {
            #[cfg(unix)]
            if let Some(pid) = pid {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            child.wait().await
        }
    }
}

/// Signal a run's task to stop and let it produce its own terminating
/// `Finished` event. Idempotent: a second cancel of the same run, or a
/// cancel after the run already finished, is a no-op.
#[tauri::command]
pub fn cancel_skill_agent_run(
    run_id: String,
    state: tauri::State<SkillAgentRunnerState>,
) -> Result<(), String> {
    let handle = state
        .runs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&run_id)
        .cloned();
    let Some(handle) = handle else {
        return Ok(());
    };
    if handle.cancelled.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    handle.cancel.notify_one();
    Ok(())
}

/// A skill (or scratch-dir agent folder) name, validated before it's used to
/// build any path: it must stay a single, ordinary path segment so it can
/// never escape the scratch directory it's joined onto.
fn validate_skill_dir_name(name: &str) -> Result<&str, String> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(format!("Invalid skill name: {name:?}"));
    }
    if name.len() > 128 {
        return Err(format!("Skill name too long: {name:?}"));
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err(format!("Invalid skill name: {name:?}"));
    }
    Ok(name)
}

/// How deep `copy_skill_dir` will recurse into directory symlinks before
/// giving up - bounds a symlink cycle to a finite amount of work.
const MAX_COPY_DEPTH: u32 = 32;

/// Copies `src` into `dest`, containing the walk to `src`'s own canonical
/// subtree: a directory symlink is only followed if its target resolves
/// inside that subtree, any entry that canonicalizes outside it is skipped,
/// and `.git` is never copied. Regular files and file symlinks are copied by
/// content (`fs::copy` follows symlinks).
fn copy_skill_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    let canonical_root = fs::canonicalize(src)?;
    fs::create_dir_all(dest)?;
    copy_dir_contained(&canonical_root, &canonical_root, dest, 0)
}

fn copy_dir_contained(root: &Path, src: &Path, dest: &Path, depth: u32) -> std::io::Result<()> {
    if depth > MAX_COPY_DEPTH {
        return Ok(());
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        let dest_path = dest.join(entry.file_name());

        // Anything that doesn't canonicalize inside `root` - a symlink
        // escaping it, or an entry removed mid-walk - is skipped.
        let canonical_path = match fs::canonicalize(&path) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !canonical_path.starts_with(root) {
            continue;
        }

        let is_dir = match fs::metadata(&path) {
            Ok(m) => m.is_dir(),
            Err(_) => continue,
        };
        if is_dir {
            fs::create_dir_all(&dest_path)?;
            copy_dir_contained(root, &canonical_path, &dest_path, depth + 1)?;
        } else {
            fs::copy(&path, &dest_path)?;
        }
    }
    Ok(())
}

/// Exposes `copy_skill_dir` to `skill_run_target`, which installs a skill
/// into a worktree the same way a scratch dir installs it into
/// `.agents/skills/<name>`.
pub(crate) fn copy_skill_dir_for_run_target(src: &Path, dest: &Path) -> std::io::Result<()> {
    copy_skill_dir(src, dest)
}

fn scratch_root(app: &AppHandle) -> Result<PathBuf, String> {
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("Could not resolve app cache dir: {e}"))?;
    Ok(cache_dir.join("skill-studio").join("scratch"))
}

/// Create a scratch directory containing only `skills` (name, source folder
/// path pairs): each copied to `.agents/skills/<name>`, with
/// `.claude/skills/<name>` and `.pi/skills/<name>` symlinked to it, so a run
/// only ever sees the skill(s) under test. Returns the scratch dir's path.
#[tauri::command]
pub fn create_skill_scratch_dir(
    app: AppHandle,
    skills: Vec<(String, String)>,
) -> Result<String, String> {
    let stamp = format!(
        "{}-{}",
        Utc::now().format("%Y%m%dT%H%M%S%.f"),
        std::process::id()
    );
    let root = scratch_root(&app)?.join(stamp);
    let shared_skills = root.join(".agents").join("skills");
    fs::create_dir_all(&shared_skills).map_err(|e| format!("Could not create scratch dir: {e}"))?;

    for (name, folder_path) in &skills {
        let name = validate_skill_dir_name(name)?;
        copy_skill_dir(Path::new(folder_path), &shared_skills.join(name))
            .map_err(|e| format!("Could not copy skill '{name}': {e}"))?;

        for agent_dir in [".claude/skills", ".pi/skills"] {
            let link_dir = root.join(agent_dir);
            fs::create_dir_all(&link_dir)
                .map_err(|e| format!("Could not create {agent_dir}: {e}"))?;
            let target = Path::new("../../.agents/skills").join(name);
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, link_dir.join(name))
                .map_err(|e| format!("Could not symlink {agent_dir}/{name}: {e}"))?;
        }
    }

    fs::write(root.join(".gitkeep"), b"").map_err(|e| format!("Could not write .gitkeep: {e}"))?;
    // Best-effort: several harnesses behave better inside a git repo, but a
    // missing `git` binary shouldn't fail scratch dir creation.
    let _ = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(&root)
        .output();

    Ok(root.to_string_lossy().to_string())
}

/// Removes `path`, requiring it to be an immediate child of `root` (not
/// `root` itself, and not a deeper descendant) once both are canonicalized.
fn remove_scratch_child(root: &Path, path: &Path) -> Result<(), String> {
    let canonical_root =
        fs::canonicalize(root).map_err(|e| format!("Could not resolve scratch root: {e}"))?;
    let canonical_target =
        fs::canonicalize(path).map_err(|e| format!("Could not resolve {}: {e}", path.display()))?;
    if canonical_target == canonical_root
        || canonical_target.parent() != Some(canonical_root.as_path())
    {
        return Err("Refusing to remove a path outside the scratch folder".to_string());
    }
    fs::remove_dir_all(&canonical_target)
        .map_err(|e| format!("Could not remove {}: {e}", path.display()))
}

/// Remove a scratch directory created by `create_skill_scratch_dir`. Refuses
/// any path that isn't an immediate child of the scratch root, so a bad
/// `path` can't delete unrelated files.
#[tauri::command]
pub fn remove_skill_scratch_dir(app: AppHandle, path: String) -> Result<(), String> {
    let root = scratch_root(&app)?;
    fs::create_dir_all(&root).map_err(|e| format!("Could not resolve scratch root: {e}"))?;
    remove_scratch_child(&root, Path::new(&path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn request(
        harness: HarnessId,
        write_access: WriteAccess,
        session_id: Option<&str>,
    ) -> SkillAgentRunRequest {
        SkillAgentRunRequest {
            harness,
            prompt: "What does this skill do?".to_string(),
            cwd: "/tmp/scratch".to_string(),
            skill_name: "say-banana".to_string(),
            write_access,
            session_id: session_id.map(str::to_string),
        }
    }

    fn build(
        harness: HarnessId,
        write_access: WriteAccess,
        session_id: Option<&str>,
    ) -> Vec<String> {
        let (_program, args, _env, _cwd) = build_command(
            &request(harness, write_access, session_id),
            Path::new("/usr/local/bin/claude"),
        );
        args
    }

    #[test]
    fn build_command_claude_read_only() {
        let args = build(HarnessId::ClaudeCode, WriteAccess::ReadOnly, None);
        assert!(args.contains(&"--permission-mode".to_string()));
        assert!(args.contains(&"default".to_string()));
        assert!(args.contains(&"--allowedTools".to_string()));
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[test]
    fn build_command_claude_workspace_resumes_session() {
        let args = build(
            HarnessId::ClaudeCode,
            WriteAccess::Workspace,
            Some("sess-1"),
        );
        assert!(args.contains(&"auto".to_string()));
        assert!(!args.contains(&"--allowedTools".to_string()));
        let resume_at = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(args[resume_at + 1], "sess-1");
    }

    #[test]
    fn build_command_codex_read_only() {
        let args = build(HarnessId::Codex, WriteAccess::ReadOnly, None);
        assert_eq!(args[0], "exec");
        assert!(args.contains(&"--json".to_string()));
        assert!(args.contains(&"--skip-git-repo-check".to_string()));
        let mode_at = args.iter().position(|a| a == "-s").unwrap();
        assert_eq!(args[mode_at + 1], "read-only");
        assert!(!args.contains(&"-a".to_string()));
    }

    #[test]
    fn build_command_codex_workspace_resumes_without_cwd_flag() {
        let args = build(HarnessId::Codex, WriteAccess::Workspace, Some("thread-1"));
        assert_eq!(args[0], "exec");
        assert_eq!(args[1], "resume");
        assert_eq!(args[2], "thread-1");
        assert!(!args.contains(&"-C".to_string()));
        assert!(!args.contains(&"-s".to_string()));
    }

    #[test]
    fn build_command_open_code_read_only() {
        let args = build(HarnessId::OpenCode, WriteAccess::ReadOnly, None);
        assert_eq!(args[0], "run");
        assert!(!args.contains(&"--auto".to_string()));
        assert!(args.contains(&"--dir".to_string()));
    }

    #[test]
    fn build_command_open_code_workspace() {
        let args = build(HarnessId::OpenCode, WriteAccess::Workspace, Some("sess-2"));
        assert!(args.contains(&"--auto".to_string()));
        let session_at = args.iter().position(|a| a == "--session").unwrap();
        assert_eq!(args[session_at + 1], "sess-2");
    }

    #[test]
    fn build_command_pi_read_only() {
        let args = build(HarnessId::Pi, WriteAccess::ReadOnly, None);
        assert_eq!(args[0], "-p");
        let tools_at = args.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(args[tools_at + 1], "read,grep,find,ls");
    }

    #[test]
    fn build_command_pi_workspace() {
        let args = build(HarnessId::Pi, WriteAccess::Workspace, None);
        assert!(!args.contains(&"--tools".to_string()));
    }

    #[test]
    fn parse_line_claude_reports_skill_loaded_and_result() {
        let mut state = ParseState::default();
        let init = r#"{"type":"system","subtype":"init","session_id":"sess-abc"}"#;
        assert!(parse_line(HarnessId::ClaudeCode, init, "say-banana", &mut state).is_empty());
        assert_eq!(state.session_id.as_deref(), Some("sess-abc"));

        let tool_use = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":"say-banana"}}]}}"#;
        let events = parse_line(HarnessId::ClaudeCode, tool_use, "say-banana", &mut state);
        assert!(matches!(events[0], SkillAgentEventKind::ToolCall { .. }));
        assert_eq!(state.skill_loaded, SkillLoaded::Yes);

        let text =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"BANANA"}]}}"#;
        let events = parse_line(HarnessId::ClaudeCode, text, "say-banana", &mut state);
        assert!(
            matches!(&events[0], SkillAgentEventKind::AssistantText { text, is_delta: false } if text == "BANANA")
        );

        let result = r#"{"type":"result","subtype":"success","is_error":false,"result":"BANANA","session_id":"sess-abc","total_cost_usd":0.061,"duration_ms":1200}"#;
        let events = parse_line(HarnessId::ClaudeCode, result, "say-banana", &mut state);
        match &events[0] {
            SkillAgentEventKind::Finished {
                ok,
                final_text,
                cost_usd,
                duration_ms,
                skill_loaded,
                ..
            } => {
                assert!(ok);
                assert_eq!(final_text, "BANANA");
                assert_eq!(*cost_usd, Some(0.061));
                assert_eq!(*duration_ms, 1200);
                assert_eq!(*skill_loaded, SkillLoaded::Yes);
            }
            other => panic!("expected Finished, got {other:?}"),
        }
    }

    #[test]
    fn parse_line_codex_reports_command_execution_and_agent_message() {
        let mut state = ParseState::default();
        let started = r#"{"type":"thread.started","thread_id":"thread-1"}"#;
        assert!(parse_line(HarnessId::Codex, started, "say-banana", &mut state).is_empty());
        assert_eq!(state.session_id.as_deref(), Some("thread-1"));

        let command = r#"{"type":"item.completed","item":{"id":"1","type":"command_execution","command":"cat .agents/skills/say-banana/SKILL.md","aggregated_output":"...","exit_code":0}}"#;
        let events = parse_line(HarnessId::Codex, command, "say-banana", &mut state);
        assert!(matches!(events[0], SkillAgentEventKind::ToolCall { .. }));
        assert_eq!(state.skill_loaded, SkillLoaded::Yes);

        let message =
            r#"{"type":"item.completed","item":{"id":"2","type":"agent_message","text":"BANANA"}}"#;
        let events = parse_line(HarnessId::Codex, message, "say-banana", &mut state);
        assert!(
            matches!(&events[0], SkillAgentEventKind::AssistantText { text, .. } if text == "BANANA")
        );
        assert_eq!(state.last_agent_message.as_deref(), Some("BANANA"));
    }

    #[test]
    fn parse_line_pi_reports_text_deltas_and_read_tool() {
        let mut state = ParseState::default();
        let session = r#"{"type":"session","id":"pi-sess-1","cwd":"/tmp"}"#;
        assert!(parse_line(HarnessId::Pi, session, "say-banana", &mut state).is_empty());
        assert_eq!(state.session_id.as_deref(), Some("pi-sess-1"));

        let read = r#"{"type":"tool_execution_start","toolName":"read","args":{"path":"/tmp/scratch/.agents/skills/say-banana/SKILL.md"}}"#;
        let events = parse_line(HarnessId::Pi, read, "say-banana", &mut state);
        assert!(matches!(events[0], SkillAgentEventKind::ToolCall { .. }));
        assert_eq!(state.skill_loaded, SkillLoaded::Yes);

        let delta = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"BAN"}}"#;
        let events = parse_line(HarnessId::Pi, delta, "say-banana", &mut state);
        assert!(
            matches!(&events[0], SkillAgentEventKind::AssistantText { text, is_delta: true } if text == "BAN")
        );
    }

    #[test]
    fn parse_line_open_code_finds_nested_text_and_tool() {
        let mut state = ParseState::default();
        let line = r#"{"part":{"text":"BANANA"}}"#;
        let events = parse_line(HarnessId::OpenCode, line, "say-banana", &mut state);
        assert!(
            matches!(&events[0], SkillAgentEventKind::AssistantText { text, .. } if text == "BANANA")
        );
        assert_eq!(state.skill_loaded, SkillLoaded::Unknown);

        let mut state = ParseState::default();
        let line = r#"{"toolName":"read","path":"SKILL.md"}"#;
        let events = parse_line(HarnessId::OpenCode, line, "say-banana", &mut state);
        assert!(matches!(&events[0], SkillAgentEventKind::ToolCall { name, .. } if name == "read"));
    }

    #[test]
    fn skill_loaded_yes_when_tool_use_matches_skill_name() {
        let mut state = ParseState::default();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":"say-banana"}}]}}"#;
        parse_line(HarnessId::ClaudeCode, line, "say-banana", &mut state);
        assert_eq!(state.skill_loaded, SkillLoaded::Yes);
    }

    #[test]
    fn skill_loaded_no_when_tool_use_names_a_different_skill() {
        let mut state = ParseState::default();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":"other-skill"}}]}}"#;
        parse_line(HarnessId::ClaudeCode, line, "say-banana", &mut state);
        assert_eq!(state.skill_loaded, SkillLoaded::No);
    }

    #[test]
    fn skill_loaded_unknown_with_no_skill_tool_use() {
        let mut state = ParseState::default();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;
        parse_line(HarnessId::ClaudeCode, line, "say-banana", &mut state);
        assert_eq!(state.skill_loaded, SkillLoaded::Unknown);
    }

    // -- F1: scratch dir containment --------------------------------------

    #[test]
    fn validate_skill_dir_name_rejects_traversal_and_bad_shapes() {
        assert!(validate_skill_dir_name("../x").is_err());
        assert!(validate_skill_dir_name("/abs").is_err());
        assert!(validate_skill_dir_name("a/b").is_err());
        assert!(validate_skill_dir_name("").is_err());
        assert!(validate_skill_dir_name(".").is_err());
        assert!(validate_skill_dir_name("..").is_err());
        assert!(validate_skill_dir_name(&"a".repeat(129)).is_err());
        assert!(validate_skill_dir_name("say-banana").is_ok());
    }

    #[test]
    fn copy_skill_dir_skips_dir_symlink_pointing_outside_source() {
        let outside = tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), b"secret").unwrap();

        let src = tempdir().unwrap();
        fs::write(src.path().join("SKILL.md"), b"hello").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), src.path().join("escape")).unwrap();

        let dest = tempdir().unwrap();
        let dest_path = dest.path().join("copied");
        copy_skill_dir(src.path(), &dest_path).unwrap();

        assert!(dest_path.join("SKILL.md").exists());
        assert!(!dest_path.join("escape").exists());
    }

    #[test]
    fn copy_skill_dir_terminates_on_a_symlink_cycle() {
        let src = tempdir().unwrap();
        fs::write(src.path().join("SKILL.md"), b"hello").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(src.path(), src.path().join("loop")).unwrap();

        let dest = tempdir().unwrap();
        let dest_path = dest.path().join("copied");
        // Must return (not hang) even though `loop` points back at `src`.
        copy_skill_dir(src.path(), &dest_path).unwrap();
        assert!(dest_path.join("SKILL.md").exists());
    }

    #[test]
    fn remove_scratch_child_refuses_the_root_itself() {
        let root = tempdir().unwrap();
        let err = remove_scratch_child(root.path(), root.path()).unwrap_err();
        assert!(err.contains("outside the scratch folder"));
    }

    #[test]
    fn remove_scratch_child_refuses_a_grandchild() {
        let root = tempdir().unwrap();
        let child = root.path().join("run-1");
        let grandchild = child.join("nested");
        fs::create_dir_all(&grandchild).unwrap();
        let err = remove_scratch_child(root.path(), &grandchild).unwrap_err();
        assert!(err.contains("outside the scratch folder"));
        assert!(grandchild.exists());
    }

    #[test]
    fn remove_scratch_child_allows_an_immediate_child() {
        let root = tempdir().unwrap();
        let child = root.path().join("run-1");
        fs::create_dir_all(&child).unwrap();
        remove_scratch_child(root.path(), &child).unwrap();
        assert!(!child.exists());
    }

    // -- F3/F4: exactly one Finished, cleanup on exit ----------------------

    #[test]
    fn cancel_is_idempotent_when_run_is_gone() {
        let state = SkillAgentRunnerState::default();
        // No entry for "missing-run" at all.
        assert!(cancel_skill_agent_run_for_test(&state, "missing-run").is_ok());
    }

    #[test]
    fn cancel_is_idempotent_when_already_cancelled() {
        let state = SkillAgentRunnerState::default();
        let handle = Arc::new(RunHandle::default());
        state
            .runs
            .lock()
            .unwrap()
            .insert("run-1".to_string(), handle.clone());
        assert!(cancel_skill_agent_run_for_test(&state, "run-1").is_ok());
        assert!(handle.cancelled.load(Ordering::SeqCst));
        // Second cancel of the same, still-registered run is a no-op.
        assert!(cancel_skill_agent_run_for_test(&state, "run-1").is_ok());
    }

    /// `cancel_skill_agent_run` takes a `tauri::State`, which needs a running
    /// app to construct; this exercises the same logic directly against a
    /// bare `SkillAgentRunnerState`.
    fn cancel_skill_agent_run_for_test(
        state: &SkillAgentRunnerState,
        run_id: &str,
    ) -> Result<(), String> {
        let handle = state
            .runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(run_id)
            .cloned();
        let Some(handle) = handle else {
            return Ok(());
        };
        if handle.cancelled.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        handle.cancel.notify_one();
        Ok(())
    }

    #[tokio::test]
    async fn run_task_removes_its_entry_and_emits_exactly_one_finished() {
        let runs: Arc<Mutex<HashMap<String, Arc<RunHandle>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let run_id = new_run_id();
        let handle = Arc::new(RunHandle::default());
        runs.lock().unwrap().insert(run_id.clone(), handle.clone());

        let events: Arc<Mutex<Vec<SkillAgentEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        let sink: EventSink = Box::new(move |event| {
            events_clone
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(event);
        });

        run_and_deregister(
            runs.clone(),
            sink,
            run_id.clone(),
            request(HarnessId::ClaudeCode, WriteAccess::ReadOnly, None),
            PathBuf::from("/bin/sh"),
            handle,
        )
        .await;

        assert!(runs.lock().unwrap().is_empty());
        let finished_count = events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e.kind, SkillAgentEventKind::Finished { .. }))
            .count();
        assert_eq!(finished_count, 1);
    }

    // -- F5: bounded output framing -----------------------------------------

    #[test]
    fn parse_line_malformed_and_partial_records_are_ignored() {
        for harness in [
            HarnessId::ClaudeCode,
            HarnessId::Codex,
            HarnessId::OpenCode,
            HarnessId::Pi,
        ] {
            let mut state = ParseState::default();
            assert!(parse_line(harness, "not json at all", "skill", &mut state).is_empty());
            let mut state = ParseState::default();
            assert!(parse_line(
                harness,
                r#"{"type":"assistant","message":"#,
                "skill",
                &mut state
            )
            .is_empty());
        }
    }

    #[test]
    fn stderr_tail_survives_a_cut_through_a_multibyte_character() {
        let mut tail: Vec<u8> = Vec::new();
        // "é" is 2 bytes; pad so the cap lands inside its second byte.
        let filler = vec![b'a'; STDERR_TAIL_LEN - 1];
        push_stderr_tail(&mut tail, &filler);
        push_stderr_tail(&mut tail, "é more text".as_bytes());
        assert_eq!(tail.len(), STDERR_TAIL_LEN);
        // Must not panic, and must still decode to *something*.
        let decoded = String::from_utf8_lossy(&tail);
        assert!(decoded.ends_with("more text"));
    }

    #[tokio::test]
    async fn read_capped_lines_skips_an_oversized_line_and_keeps_going() {
        let mut big = vec![b'x'; MAX_LINE_BYTES + 10];
        big.push(b'\n');
        big.extend_from_slice(b"{\"ok\":true}\n");
        let reader = tokio::io::BufReader::new(&big[..]);

        let mut lines = Vec::new();
        let mut skipped = 0;
        read_capped_lines(reader, |line| lines.push(line), || skipped += 1).await;

        assert_eq!(skipped, 1);
        assert_eq!(lines, vec![r#"{"ok":true}"#.to_string()]);
    }

    #[tokio::test]
    async fn read_capped_lines_parses_a_final_line_without_trailing_newline() {
        let bytes = b"{\"a\":1}\n{\"b\":2}".to_vec();
        let reader = tokio::io::BufReader::new(&bytes[..]);
        let mut lines = Vec::new();
        read_capped_lines(reader, |line| lines.push(line), || {}).await;
        assert_eq!(
            lines,
            vec![r#"{"a":1}"#.to_string(), r#"{"b":2}"#.to_string()]
        );
    }

    // -- F6: binary resolution ------------------------------------------------

    fn always_executable(_: &Path) -> bool {
        true
    }
    fn never_executable(_: &Path) -> bool {
        false
    }

    #[test]
    fn pick_executable_line_skips_startup_noise() {
        let stdout = "Welcome to zsh\nLoading dotfiles...\n/usr/local/bin/claude\n";
        assert_eq!(
            pick_executable_line(stdout, always_executable),
            Some(PathBuf::from("/usr/local/bin/claude"))
        );
    }

    #[test]
    fn pick_executable_line_rejects_an_alias_line() {
        let stdout = "alias claude=/usr/local/bin/claude-old\n";
        assert_eq!(pick_executable_line(stdout, always_executable), None);
    }

    #[test]
    fn pick_executable_line_rejects_a_shell_function() {
        let stdout = "claude () {\n  echo hi\n}\n";
        assert_eq!(pick_executable_line(stdout, always_executable), None);
    }

    #[test]
    fn pick_executable_line_rejects_a_relative_name() {
        let stdout = "claude\n";
        assert_eq!(pick_executable_line(stdout, always_executable), None);
    }

    #[test]
    fn pick_executable_line_rejects_a_non_executable_path() {
        let stdout = "/usr/local/bin/claude\n";
        assert_eq!(pick_executable_line(stdout, never_executable), None);
    }

    #[test]
    fn pick_executable_line_accepts_a_path_with_spaces() {
        let stdout = "/opt/my app/bin/claude\n";
        assert_eq!(
            pick_executable_line(stdout, always_executable),
            Some(PathBuf::from("/opt/my app/bin/claude"))
        );
    }
}
