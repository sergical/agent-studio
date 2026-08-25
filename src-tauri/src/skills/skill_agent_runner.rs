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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Event name every `SkillAgentEvent` is emitted on.
pub const SKILL_AGENT_EVENT: &str = "skill-agent://event";

/// stderr kept per run, for the error message when a harness exits non-zero
/// without ever producing final text.
const STDERR_TAIL_LEN: usize = 2000;

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

/// One line of a run's transcript, in emission order (`seq`).
#[derive(Debug, Clone, Serialize)]
pub struct SkillAgentEvent {
    pub run_id: String,
    pub seq: u64,
    pub at: DateTime<Utc>,
    pub kind: SkillAgentEventKind,
}

/// The discriminated union of everything a run can report.
#[derive(Debug, Clone, Serialize)]
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

/// A run in progress, and the harnesses whose absolute binary path has
/// already been resolved once (`$SHELL -lc 'command -v <bin>'` is not
/// cheap, and the app's own `PATH` doesn't see the user's shell config).
#[derive(Default)]
pub struct SkillAgentRunnerState {
    runs: Mutex<HashMap<String, tokio::task::AbortHandle>>,
    binaries: Mutex<HashMap<HarnessId, PathBuf>>,
}

fn resolve_binary(harness: HarnessId, state: &SkillAgentRunnerState) -> Result<PathBuf, String> {
    if let Some(cached) = state.binaries.lock().unwrap().get(&harness) {
        return Ok(cached.clone());
    }

    let bin = harness.bin_name();
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let output = std::process::Command::new(&shell)
        .arg("-lc")
        .arg(format!("command -v {bin}"))
        .output()
        .map_err(|_| format!("{bin} is not installed or not on PATH"))?;
    let found = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || found.is_empty() {
        return Err(format!("{bin} is not installed or not on PATH"));
    }

    let path = PathBuf::from(found);
    state.binaries.lock().unwrap().insert(harness, path.clone());
    Ok(path)
}

/// A run id unique within this process: a nanosecond timestamp plus a
/// monotonic counter, so two runs started in the same tick still differ.
fn new_run_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!(
        "run-{}-{n}",
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    )
}

fn emit_event(app: &AppHandle, run_id: &str, seq: &AtomicU64, kind: SkillAgentEventKind) {
    let event = SkillAgentEvent {
        run_id: run_id.to_string(),
        seq: seq.fetch_add(1, Ordering::SeqCst),
        at: Utc::now(),
        kind,
    };
    let _ = app.emit(SKILL_AGENT_EVENT, &event);
}

/// Start a headless harness run for one skill: resolves the binary, spawns
/// it, streams `Started`/`AssistantText`/`ToolCall`/`ToolResult`/`Error`
/// events as its stdout lines parse, and always emits exactly one
/// `Finished` when it exits (or is cancelled).
#[tauri::command]
pub async fn start_skill_agent_run(
    request: SkillAgentRunRequest,
    app: AppHandle,
    state: tauri::State<'_, SkillAgentRunnerState>,
) -> Result<String, String> {
    let binary = resolve_binary(request.harness, &state)?;
    let run_id = new_run_id();

    let task_app = app.clone();
    let task_run_id = run_id.clone();
    let join_handle = tokio::spawn(async move {
        run_skill_agent(task_app, task_run_id, request, binary).await;
    });
    state
        .runs
        .lock()
        .unwrap()
        .insert(run_id.clone(), join_handle.abort_handle());

    Ok(run_id)
}

async fn run_skill_agent(
    app: AppHandle,
    run_id: String,
    request: SkillAgentRunRequest,
    binary: PathBuf,
) {
    let (program, args, env, cwd) = build_command(&request, &binary);
    let command_display = format!("{program} {}", args.join(" "));
    let seq = AtomicU64::new(0);

    let mut command = Command::new(&program);
    command
        .args(&args)
        .envs(env)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            emit_event(
                &app,
                &run_id,
                &seq,
                SkillAgentEventKind::Error {
                    message: format!(
                        "{} is not installed or not on PATH",
                        request.harness.bin_name()
                    ),
                },
            );
            emit_event(
                &app,
                &run_id,
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

    emit_event(
        &app,
        &run_id,
        &seq,
        SkillAgentEventKind::Started {
            command: command_display,
            session_id: request.session_id.clone(),
        },
    );

    let started_at = Instant::now();
    let stderr_tail = std::sync::Arc::new(Mutex::new(String::new()));
    let stderr_task = child.stderr.take().map(|stderr| {
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut buf = tail.lock().unwrap();
                buf.push_str(&line);
                buf.push('\n');
                if buf.len() > STDERR_TAIL_LEN {
                    let cut = buf.len() - STDERR_TAIL_LEN;
                    *buf = buf.split_off(cut);
                }
            }
        })
    });

    let mut state = ParseState::default();
    let mut finished_emitted = false;
    let mut had_error = false;

    if let Some(stdout) = child.stdout.take() {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            for kind in parse_line(request.harness, &line, &request.skill_name, &mut state) {
                match &kind {
                    SkillAgentEventKind::Finished { .. } => finished_emitted = true,
                    SkillAgentEventKind::Error { .. } => had_error = true,
                    _ => {}
                }
                emit_event(&app, &run_id, &seq, kind);
            }
        }
    }

    if let Some(task) = stderr_task {
        let _ = task.await;
    }

    let status = child.wait().await;
    if finished_emitted {
        return;
    }

    let duration_ms = started_at.elapsed().as_millis() as u64;
    let final_text = state
        .last_agent_message
        .clone()
        .unwrap_or_else(|| state.last_text.clone());
    let exit_ok = matches!(&status, Ok(s) if s.success());

    if !exit_ok && final_text.is_empty() {
        let tail = stderr_tail.lock().unwrap().trim().to_string();
        let message = if !tail.is_empty() {
            tail
        } else {
            match &status {
                Ok(s) => format!("exited with code {}", s.code().unwrap_or(-1)),
                Err(e) => format!("failed to wait on process: {e}"),
            }
        };
        emit_event(&app, &run_id, &seq, SkillAgentEventKind::Error { message });
        had_error = true;
    }

    emit_event(
        &app,
        &run_id,
        &seq,
        SkillAgentEventKind::Finished {
            ok: exit_ok && !had_error,
            final_text,
            session_id: state.session_id.clone(),
            cost_usd: None,
            duration_ms,
            skill_loaded: state.skill_loaded,
        },
    );
}

/// Kill a run's child process (via `AbortHandle::abort`, which drops the
/// task's owned `Child` and, with `kill_on_drop(true)`, kills the OS
/// process) and emit the run's terminating `Finished` event.
#[tauri::command]
pub fn cancel_skill_agent_run(
    run_id: String,
    app: AppHandle,
    state: tauri::State<SkillAgentRunnerState>,
) -> Result<(), String> {
    let handle = state.runs.lock().unwrap().remove(&run_id);
    let Some(handle) = handle else {
        return Err(format!("No running skill agent run with id {run_id}"));
    };
    handle.abort();

    let event = SkillAgentEvent {
        run_id,
        seq: u64::MAX,
        at: Utc::now(),
        kind: SkillAgentEventKind::Finished {
            ok: false,
            final_text: "Cancelled".to_string(),
            session_id: None,
            cost_usd: None,
            duration_ms: 0,
            skill_loaded: SkillLoaded::Unknown,
        },
    };
    let _ = app.emit(SKILL_AGENT_EVENT, &event);
    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dest.join(entry.file_name());
        // `metadata` (not `symlink_metadata`) follows symlinks, per the spec:
        // a skill folder containing a symlinked file must copy its target.
        if fs::metadata(&path)?.is_dir() {
            copy_dir_recursive(&path, &dest_path)?;
        } else {
            fs::copy(&path, &dest_path)?;
        }
    }
    Ok(())
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
        copy_dir_recursive(Path::new(folder_path), &shared_skills.join(name))
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

/// Remove a scratch directory created by `create_skill_scratch_dir`. Refuses
/// any path outside the scratch root, so a bad `path` can't delete unrelated
/// files.
#[tauri::command]
pub fn remove_skill_scratch_dir(app: AppHandle, path: String) -> Result<(), String> {
    let root = scratch_root(&app)?;
    fs::create_dir_all(&root).map_err(|e| format!("Could not resolve scratch root: {e}"))?;
    let canonical_root =
        fs::canonicalize(&root).map_err(|e| format!("Could not resolve scratch root: {e}"))?;
    let canonical_target =
        fs::canonicalize(&path).map_err(|e| format!("Could not resolve {path}: {e}"))?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err("Refusing to remove a path outside the scratch root".to_string());
    }
    fs::remove_dir_all(&canonical_target).map_err(|e| format!("Could not remove {path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
