#![forbid(unsafe_code)]
// The CLI's job is printing the result envelope and human tables to stdout
// and errors to stderr; that is its whole output surface.
#![allow(clippy::print_stdout, clippy::print_stderr)]
// unwrap/expect are fine in test code; production code must use ?
// or an explicit error.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! `skill-studio`: the Skill Studio command line.
//!
//! A thin adapter over `skill-studio-core`: it builds a `RuntimeScope` and a
//! `Ports` from flags and the environment, calls one core operation, and
//! prints the `ResultEnvelope` (as JSON) or a short human table. It holds no
//! policy of its own; every rule (issue derivation, capability facts, exit
//! statuses) lives in the core.

mod output;
mod scope;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use skill_studio_core::dto::{
    CapabilitiesRequest, HarnessesRequest, Inventory, ListEventsRequest, RepairApplyMode,
    RepairApplyRequest, RepairPreviewRequest, RestoreRequest, ScanRequest,
};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::health::{self, Outcome, TimingRow};
use skill_studio_core::identity::{AgentId, CorrelationId, DeploymentId, EventId, SkillName};
use skill_studio_core::ops::{self, Operation, ResultEnvelope};
use skill_studio_core::ports::{OpContext, Runtime};
use skill_studio_core::snapshot::SnapshotCell;

use crate::scope::ScopeArgs;

/// The rollup window `run_health` folds `timing.jsonl` over, matching the
/// Settings "Command health" card and unit 6.5's ticket.
const HEALTH_WINDOW: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

/// The version clap prints for `--version`: the crate version and the build
/// commit, `SKILL_STUDIO_COMMIT` from `build.rs` ("dev" outside a release
/// build), on one line.
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("SKILL_STUDIO_COMMIT"),
    ")"
);

#[derive(Parser)]
#[command(name = "skill-studio", about = "Manage agent skills across harnesses", version = VERSION)]
struct Cli {
    /// Print each op's and each of its steps' elapsed time to stderr.
    #[arg(long, global = true)]
    time: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inventory every installed skill.
    Scan {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Restrict to these skill names.
        #[arg(long = "skill")]
        skills: Vec<String>,
        /// Include per-phase timings.
        #[arg(long)]
        timings: bool,
        /// Print the result envelope as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Inventory every installed skill and derive issues.
    Diagnose {
        #[command(flatten)]
        scope: ScopeArgs,
        #[arg(long = "skill")]
        skills: Vec<String>,
        #[arg(long)]
        timings: bool,
        #[arg(long)]
        json: bool,
    },
    /// Report harness capability facts.
    Capabilities {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Restrict to these harnesses.
        #[arg(long = "harness")]
        harnesses: Vec<String>,
        /// Also probe the machine (config presence, runner binary).
        #[arg(long)]
        observe: bool,
        /// Executables to look up on `PATH`.
        #[arg(long = "tool")]
        tools: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Detect first-class harnesses installed on this machine: `PATH`,
    /// version, install method, configured, and used evidence.
    Harnesses {
        #[command(flatten)]
        scope: ScopeArgs,
        #[arg(long)]
        json: bool,
    },
    /// Preview a frontmatter repair for one deployment, without writing.
    PreviewRepair {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Deployment to repair.
        #[arg(long)]
        deployment_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Apply a previously-previewed frontmatter repair.
    ApplyRepair {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Preview envelope, as printed by `preview-repair --json` (its
        /// `.data` field).
        #[arg(long)]
        preview_json: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// List history rows, newest first.
    Events {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Restrict to one skill.
        #[arg(long)]
        skill: Option<String>,
        /// Maximum rows; 0 means the core default.
        #[arg(long, default_value_t = 0)]
        limit: u32,
        /// Pagination cursor: only rows older than this event id.
        #[arg(long)]
        after: Option<String>,
        /// Compare live fingerprints with the recorded ones.
        #[arg(long)]
        check_drift: bool,
        #[arg(long)]
        json: bool,
    },
    /// Revert one event.
    Restore {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Event to revert.
        #[arg(long)]
        event_id: String,
        /// Proceed even if the touched paths have drifted.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
    /// Revert the most recent unreverted event.
    Undo {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Proceed even if the touched paths have drifted.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
    /// Turn a skill's native per-harness switch on or off (Claude Code,
    /// Codex, `OpenCode`; pi has no native switch).
    SetHarnessEnabled {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Skill to toggle.
        #[arg(long)]
        skill: String,
        /// Harness whose switch to flip.
        #[arg(long)]
        harness: String,
        /// Sets the switch to enabled; pass `--enabled=false` to disable.
        #[arg(long, default_value_t = true)]
        enabled: bool,
        /// Project the targeted row is scoped to; omit for the global row.
        /// Only Claude Code's switch (a per-scope symlink slot) reads this.
        #[arg(long)]
        project_path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Write one JSON Schema file per request/result DTO.
    Schema {
        /// Directory to write schema files into.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Print the command health rollup: count, failures, p50/p95 duration,
    /// and last error, per command, over the last 7 days of `timing.jsonl`.
    Health {
        /// Path to the timing log to read. Defaults to the desktop app's
        /// own `timing.jsonl` (its app data dir, resolved through `dirs` -
        /// the same file the desktop's Settings "Command health" card
        /// reads via its own Tauri command).
        #[arg(long)]
        timing_log: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Stream one JSON line per revision change: an initial line with the
    /// current revision and inventory, then one line per change, until
    /// interrupted.
    Watch {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Suppress the initial full line when the current revision already
        /// equals this one.
        #[arg(long)]
        since: Option<u64>,
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let time = cli.time;
    match cli.command {
        Command::Scan {
            scope,
            skills,
            timings,
            json,
        } => run_scan(&scope, skills, timings, json, time),
        Command::Diagnose {
            scope,
            skills,
            timings,
            json,
        } => run_diagnose(&scope, skills, timings, json, time),
        Command::Capabilities {
            scope,
            harnesses,
            observe,
            tools,
            json,
        } => run_capabilities(&scope, harnesses, observe, tools, json, time),
        Command::Harnesses { scope, json } => run_harnesses(&scope, json, time),
        Command::PreviewRepair {
            scope,
            deployment_id,
            json,
        } => run_preview_repair(&scope, &deployment_id, json, time),
        Command::ApplyRepair {
            scope,
            preview_json,
            json,
        } => run_apply_repair(&scope, &preview_json, json, time),
        Command::Events {
            scope,
            skill,
            limit,
            after,
            check_drift,
            json,
        } => run_events(&scope, skill, limit, after, check_drift, json, time),
        Command::Restore {
            scope,
            event_id,
            force,
            json,
        } => run_restore(&scope, event_id, force, json, time),
        Command::Undo { scope, force, json } => run_undo(&scope, force, json, time),
        Command::SetHarnessEnabled {
            scope,
            skill,
            harness,
            enabled,
            project_path,
            json,
        } => run_set_harness_enabled(&scope, skill, &harness, enabled, project_path, json, time),
        Command::Schema { out } => output::write_schemas(out),
        Command::Health { timing_log, json } => run_health(timing_log, json),
        Command::Watch { scope, since, json } => run_watch(&scope, since, json, time),
    }
}

/// Prints an op's timing to stderr when `--time` was passed: the op line,
/// then one indented line per step, in the order they were recorded.
/// Silent when `--time` was not passed or the call recorded no timing (a
/// scope-construction failure, which never reaches an op function).
fn print_timing(time: bool, timing: Option<&skill_studio_core::timing::OpTiming>) {
    if !time {
        return;
    }
    let Some(timing) = timing else {
        return;
    };
    eprintln!("{} .... {} ms", timing.op, timing.elapsed_ms);
    for step in &timing.steps {
        eprintln!("  {} .... {} ms", step.name, step.elapsed_ms);
    }
}

/// Like [`build_runtime`], but wires a real [`skill_studio_host::SqliteHistoryOpener`]
/// bound to `<history_root>/events.sqlite3` in place of the no-op history
/// opener, so [`skill_studio_core::ports::MutationSession::begin`]
/// (`apply-repair`, `restore`) has a store to write. A write command that
/// cannot take the exclusive lease surfaces as the ordinary `scope_busy`
/// error envelope from the failed operation call, not a hang: the lease
/// acquire in the core has its own bounded wait and returns
/// [`skill_studio_core::ErrorCode::ScopeBusy`] rather than blocking forever.
fn build_runtime_write<T: ops::Outcome + serde::Serialize>(
    scope: &ScopeArgs,
    operation: Operation,
    json: bool,
) -> Result<Runtime, ExitCode> {
    let (runtime_scope, lease_root) = scope.resolve();
    let catalog = Arc::new(HarnessCatalog::builtin());
    let db_path = runtime_scope.history_root.join("events.sqlite3");
    let mut ports = skill_studio_host::default_ports_with_history(lease_root, catalog, db_path);
    if runtime_scope.kind == skill_studio_core::scope::ScopeKind::Fixture {
        ports.discovery = None;
    } else {
        ports.discovery = Some(Arc::new(skill_studio_host::HostProjectDiscovery::new()));
    }
    ports.tools = Some(Arc::new(skill_studio_host::PathToolLookup::new()));
    ports.spawner = Some(Arc::new(skill_studio_host::RealProcessSpawner::new()));
    Runtime::new(&runtime_scope, ports).map_err(|err| {
        let envelope: ResultEnvelope<T> = error_envelope(operation, err, &runtime_scope);
        let code = exit_code(envelope.exit_status());
        if json {
            output::print_json(&envelope);
        } else {
            for error in &envelope.errors {
                eprintln!("{}: {}", error.code.as_str(), error.message);
            }
        }
        code
    })
}

fn build_runtime<T: ops::Outcome + serde::Serialize>(
    scope: &ScopeArgs,
    operation: Operation,
    json: bool,
) -> Result<Runtime, ExitCode> {
    let (runtime_scope, lease_root) = scope.resolve();
    let catalog = Arc::new(HarnessCatalog::builtin());
    let mut ports = skill_studio_host::default_ports_with_discovery(lease_root, catalog);
    ports.spawner = Some(Arc::new(skill_studio_host::RealProcessSpawner::new()));
    if runtime_scope.kind == skill_studio_core::scope::ScopeKind::Fixture {
        // Fixture scopes name their own projects explicitly; discovery would
        // otherwise walk the real machine's transcripts for a fake home.
        ports.discovery = None;
    }
    Runtime::new(&runtime_scope, ports).map_err(|err| {
        let envelope: ResultEnvelope<T> = error_envelope(operation, err, &runtime_scope);
        let code = exit_code(envelope.exit_status());
        if json {
            output::print_json(&envelope);
        } else {
            for error in &envelope.errors {
                eprintln!("{}: {}", error.code.as_str(), error.message);
            }
        }
        code
    })
}

/// Builds an envelope for a scope-construction failure, before a
/// `NormalizedScope` exists to hand `ResultEnvelope::from_result`.
fn error_envelope<T: ops::Outcome>(
    operation: Operation,
    err: skill_studio_core::CoreError,
    scope: &skill_studio_core::RuntimeScope,
) -> ResultEnvelope<T> {
    // A scope that failed to normalize still has an id and lexical paths a
    // person can read; approximate `EffectiveScope` from the raw scope so
    // the envelope is never empty just because normalization failed first.
    use skill_studio_core::scope::EffectiveScope;
    ResultEnvelope {
        schema_version: skill_studio_core::SCHEMA_VERSION,
        operation,
        scope: EffectiveScope {
            id: skill_studio_core::ScopeId::for_canonical_home(&scope.home_root),
            kind: scope.kind,
            home: scope.home_root.clone(),
            projects: Vec::new(),
            history_root: scope.history_root.clone(),
        },
        status: skill_studio_core::OpStatus::Error,
        data: None,
        errors: vec![skill_studio_core::ErrorEntry {
            code: err.code,
            message: err.message,
            path: err.path.map(|p| p.display().to_string()),
        }],
        correlation_id: CorrelationId(ulid::Ulid::new().to_string()),
        event_id: None,
        // No `OpContext` exists yet at this point: the scope failed to
        // normalize before any op function ran, so there's nothing to
        // build `ResultEnvelope::from_result` from.
        timings: None,
    }
}

fn exit_code(status: i32) -> ExitCode {
    ExitCode::from(status.clamp(0, 255) as u8)
}

fn run_scan(
    scope: &ScopeArgs,
    skills: Vec<String>,
    timings: bool,
    json: bool,
    time: bool,
) -> ExitCode {
    let rt = match build_runtime::<skill_studio_core::dto::Inventory>(scope, Operation::Scan, json)
    {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let req = ScanRequest {
        skills: skills.into_iter().map(SkillName).collect(),
        timings,
    };
    let result = ops::scan(&rt, &ctx, &req);
    let envelope = ResultEnvelope::from_result(Operation::Scan, &rt.scope, &ctx, result);
    let code = exit_code(envelope.exit_status());
    if json {
        output::print_json(&envelope);
    } else {
        output::print_scan_table(&envelope);
    }
    print_timing(time, envelope.timings.as_ref());
    code
}

fn run_diagnose(
    scope: &ScopeArgs,
    skills: Vec<String>,
    timings: bool,
    json: bool,
    time: bool,
) -> ExitCode {
    let rt = match build_runtime::<skill_studio_core::dto::Diagnosis>(
        scope,
        Operation::Diagnose,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let req = ScanRequest {
        skills: skills.into_iter().map(SkillName).collect(),
        timings,
    };
    let result = ops::diagnose(&rt, &ctx, &req);
    let envelope = ResultEnvelope::from_result(Operation::Diagnose, &rt.scope, &ctx, result);
    let code = exit_code(envelope.exit_status());
    if json {
        output::print_json(&envelope);
    } else {
        output::print_diagnose_table(&envelope);
    }
    print_timing(time, envelope.timings.as_ref());
    code
}

fn run_capabilities(
    scope: &ScopeArgs,
    harnesses: Vec<String>,
    observe: bool,
    tools: Vec<String>,
    json: bool,
    time: bool,
) -> ExitCode {
    let rt = match build_runtime::<skill_studio_core::harness::Capabilities>(
        scope,
        Operation::Capabilities,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let harnesses = match harnesses
        .into_iter()
        .map(|h| AgentId::parse(&h))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(harnesses) => harnesses,
        Err(err) => {
            let envelope = ResultEnvelope::<skill_studio_core::harness::Capabilities>::from_result(
                Operation::Capabilities,
                &rt.scope,
                &ctx,
                Err(err),
            );
            let code = exit_code(envelope.exit_status());
            if json {
                output::print_json(&envelope);
            } else {
                for error in &envelope.errors {
                    eprintln!("{}: {}", error.code.as_str(), error.message);
                }
            }
            return code;
        }
    };
    let req = CapabilitiesRequest {
        harnesses,
        observe,
        tools,
    };
    let result = ops::capabilities(&rt, &ctx, &req);
    let envelope = ResultEnvelope::from_result(Operation::Capabilities, &rt.scope, &ctx, result);
    let code = exit_code(envelope.exit_status());
    if json {
        output::print_json(&envelope);
    } else {
        output::print_capabilities_table(&envelope);
    }
    print_timing(time, envelope.timings.as_ref());
    code
}

fn run_harnesses(scope: &ScopeArgs, json: bool, time: bool) -> ExitCode {
    let rt = match build_runtime::<skill_studio_core::harness::HarnessReport>(
        scope,
        Operation::Harnesses,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let result = ops::harnesses(&rt, &ctx, &HarnessesRequest {});
    let envelope = ResultEnvelope::from_result(Operation::Harnesses, &rt.scope, &ctx, result);
    let code = exit_code(envelope.exit_status());
    if json {
        output::print_json(&envelope);
    } else {
        output::print_harnesses_table(&envelope);
    }
    print_timing(time, envelope.timings.as_ref());
    code
}

fn run_preview_repair(scope: &ScopeArgs, deployment_id: &str, json: bool, time: bool) -> ExitCode {
    let rt = match build_runtime::<skill_studio_core::dto::FrontmatterRepairPreview>(
        scope,
        Operation::PreviewFrontmatterRepair,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let deployment_id = match DeploymentId::parse(deployment_id) {
        Ok(id) => id,
        Err(err) => {
            let envelope =
                ResultEnvelope::<skill_studio_core::dto::FrontmatterRepairPreview>::from_result(
                    Operation::PreviewFrontmatterRepair,
                    &rt.scope,
                    &ctx,
                    Err(err),
                );
            return finish(&envelope, json, time, output::print_repair_preview_table);
        }
    };
    let req = RepairPreviewRequest { deployment_id };
    let result = ops::preview_frontmatter_repair(&rt, &ctx, &req);
    let envelope =
        ResultEnvelope::from_result(Operation::PreviewFrontmatterRepair, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_repair_preview_table)
}

fn run_apply_repair(scope: &ScopeArgs, preview_json: &PathBuf, json: bool, time: bool) -> ExitCode {
    let rt = match build_runtime_write::<skill_studio_core::dto::RepairOutcome>(
        scope,
        Operation::ApplyFrontmatterRepair,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let preview = match std::fs::read_to_string(preview_json)
        .map_err(|e| skill_studio_core::CoreError::io(preview_json, e))
        .and_then(|text| {
            serde_json::from_str::<skill_studio_core::dto::FrontmatterRepairPreview>(&text).map_err(
                |e| {
                    skill_studio_core::CoreError::new(
                        skill_studio_core::ErrorCode::InvalidRequest,
                        format!("could not parse {}: {e}", preview_json.display()),
                    )
                },
            )
        }) {
        Ok(preview) => preview,
        Err(err) => {
            let envelope = ResultEnvelope::<skill_studio_core::dto::RepairOutcome>::from_result(
                Operation::ApplyFrontmatterRepair,
                &rt.scope,
                &ctx,
                Err(err),
            );
            return finish(&envelope, json, time, output::print_repair_outcome_table);
        }
    };
    let req = RepairApplyRequest {
        preview,
        mode: RepairApplyMode::ApplyFix,
    };
    let result = ops::apply_frontmatter_repair(&rt, &ctx, &req);
    let envelope =
        ResultEnvelope::from_result(Operation::ApplyFrontmatterRepair, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_repair_outcome_table)
}

fn run_events(
    scope: &ScopeArgs,
    skill: Option<String>,
    limit: u32,
    after: Option<String>,
    check_drift: bool,
    json: bool,
    time: bool,
) -> ExitCode {
    // `list_events` only ever opens `HistoryAccess::ReadIfExists`, but it
    // still needs the real `SqliteHistoryOpener` (not `build_runtime`'s
    // no-op history) to see rows a prior `apply-repair`/`restore` wrote.
    let rt = match build_runtime_write::<Vec<skill_studio_core::dto::EventDto>>(
        scope,
        Operation::ListEvents,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let req = ListEventsRequest {
        skill: skill.map(SkillName),
        limit,
        after: after.map(EventId),
        check_drift,
    };
    let result = ops::list_events(&rt, &ctx, &req);
    let envelope = ResultEnvelope::from_result(Operation::ListEvents, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_events_table)
}

fn run_restore(
    scope: &ScopeArgs,
    event_id: String,
    force: bool,
    json: bool,
    time: bool,
) -> ExitCode {
    let rt = match build_runtime_write::<skill_studio_core::dto::RestoreOutcome>(
        scope,
        Operation::RestoreEvent,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let req = RestoreRequest {
        event_id: EventId(event_id),
        force,
    };
    let result = ops::restore_event(&rt, &ctx, &req);
    let envelope = ResultEnvelope::from_result(Operation::RestoreEvent, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_restore_outcome_table)
}

/// Reverts the newest event this scope's history still has an inverse for
/// (`skill_studio_core::dto::RestoreCapability::Yes`), across every skill and
/// write kind - `list_events` is already newest-first, so the first
/// restorable row is the last journal entry standing.
fn run_undo(scope: &ScopeArgs, force: bool, json: bool, time: bool) -> ExitCode {
    let rt = match build_runtime_write::<skill_studio_core::dto::RestoreOutcome>(
        scope,
        Operation::RestoreEvent,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    // A page of all non-restorable rows must not read as "nothing to undo":
    // page through `list_events` with `after` until a restorable row turns
    // up or a page comes back short of the limit (the end of the history).
    let mut after = None;
    let event_id = loop {
        let list_req = ListEventsRequest {
            skill: None,
            limit: ops::DEFAULT_EVENT_LIMIT,
            after,
            check_drift: false,
        };
        let events = match ops::list_events(&rt, &ctx, &list_req) {
            Ok(events) => events,
            Err(err) => {
                let envelope =
                    ResultEnvelope::<skill_studio_core::dto::RestoreOutcome>::from_result(
                        Operation::RestoreEvent,
                        &rt.scope,
                        &ctx,
                        Err(err),
                    );
                return finish(&envelope, json, time, output::print_restore_outcome_table);
            }
        };
        let page_len = events.len();
        let last_id = events.last().map(|event| event.id.clone());
        if let Some(found) = events.into_iter().find(|event| {
            matches!(
                event.restore,
                skill_studio_core::dto::RestoreCapability::Yes
            )
        }) {
            break Some(found.id);
        }
        if (page_len as u32) < ops::DEFAULT_EVENT_LIMIT {
            break None;
        }
        after = last_id;
    };
    let Some(event_id) = event_id else {
        let err = skill_studio_core::CoreError::new(
            skill_studio_core::ErrorCode::InvalidRequest,
            "nothing to undo: no restorable event in this scope's history",
        );
        let envelope = ResultEnvelope::<skill_studio_core::dto::RestoreOutcome>::from_result(
            Operation::RestoreEvent,
            &rt.scope,
            &ctx,
            Err(err),
        );
        return finish(&envelope, json, time, output::print_restore_outcome_table);
    };
    let req = RestoreRequest { event_id, force };
    let result = ops::restore_event(&rt, &ctx, &req);
    let envelope = ResultEnvelope::from_result(Operation::RestoreEvent, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_restore_outcome_table)
}

/// Turns a skill's native per-harness switch on or off, via
/// `ops::set_harness_enabled` (Claude Code link, Codex `config.toml` rows,
/// `OpenCode` `permission.skill`).
fn run_set_harness_enabled(
    scope: &ScopeArgs,
    skill: String,
    harness: &str,
    enabled: bool,
    project_path: Option<PathBuf>,
    json: bool,
    time: bool,
) -> ExitCode {
    let rt = match build_runtime_write::<skill_studio_core::dto::SetHarnessEnabledOutcome>(
        scope,
        Operation::SetHarnessEnabled,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let harness = match AgentId::parse_harness(harness) {
        Ok(harness) => harness,
        Err(err) => {
            let envelope =
                ResultEnvelope::<skill_studio_core::dto::SetHarnessEnabledOutcome>::from_result(
                    Operation::SetHarnessEnabled,
                    &rt.scope,
                    &ctx,
                    Err(err),
                );
            return finish(
                &envelope,
                json,
                time,
                output::print_set_harness_enabled_outcome_table,
            );
        }
    };
    let req = skill_studio_core::dto::SetHarnessEnabledRequest {
        skill: SkillName(skill),
        harness,
        enabled,
        project_path,
    };
    let result = ops::set_harness_enabled(&rt, &ctx, &req);
    let envelope =
        ResultEnvelope::from_result(Operation::SetHarnessEnabled, &rt.scope, &ctx, result);
    finish(
        &envelope,
        json,
        time,
        output::print_set_harness_enabled_outcome_table,
    )
}

/// One `timing.jsonl` line, as written by the desktop's `timing_log`
/// (`apps/desktop/src-tauri/src/timing_log.rs`). Deserialized field-by-field
/// rather than sharing that struct: the desktop crate isn't a CLI
/// dependency, and the CLI only ever needs these five fields.
#[derive(serde::Deserialize)]
struct TimingLine {
    ts: String,
    command: String,
    elapsed_ms: u64,
    #[serde(default)]
    outcome: String,
    #[serde(default)]
    error: Option<String>,
}

/// Parses `path` into [`TimingRow`]s for `health::health_rollup`. A missing
/// file (no command has run yet) or an unparsable line is treated the same
/// way the desktop's own `timing_log::read_rows` treats it: skipped, not a
/// hard error - `skill-studio health` on a fresh install just prints an
/// empty rollup.
fn read_timing_rows(path: &std::path::Path) -> Vec<TimingRow> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<TimingLine>(line).ok())
        .filter_map(|line| {
            let ts = chrono::DateTime::parse_from_rfc3339(&line.ts)
                .ok()?
                .with_timezone(&chrono::Utc);
            let outcome = if line.outcome == "error" {
                Outcome::Error
            } else {
                Outcome::Ok
            };
            Some(TimingRow {
                ts,
                command: line.command,
                elapsed_ms: line.elapsed_ms,
                outcome,
                error: line.error,
            })
        })
        .collect()
}

fn run_health(timing_log: Option<PathBuf>, json: bool) -> ExitCode {
    let path = timing_log.unwrap_or_else(scope::default_timing_log_path);
    let rows = read_timing_rows(&path);
    let now = chrono::Utc::now();
    let report = health::health_rollup(&rows, now, HEALTH_WINDOW);
    if json {
        // `HealthReport` borrows only our own DTOs; nothing in it can produce
        // a non-string map key or a non-finite float, the only ways this errs.
        let text = serde_json::to_string(&report)
            .unwrap_or_else(|e| format!(r#"{{"error":"failed to serialize the report: {e}"}}"#));
        println!("{text}");
    } else {
        output::print_health_table(&report);
    }
    ExitCode::SUCCESS
}

/// Polling interval for `watch`: a fixed-interval re-scan of the scope
/// roots, comparing the resulting `Inventory` (which carries the core's own
/// tag-and-length-framed content fingerprints, never a raw byte hash of our
/// own) against the last published snapshot. No `notify`/`fsevents`
/// dependency; this interval is short enough for the acceptance tests and
/// long enough not to hammer the disk.
const WATCH_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

/// One line of `watch --json` output: a revision and the inventory it names.
///
/// The watcher already holds the inventory it compared against, so it sends
/// it rather than making the reader run its own `scan`. A revision-only line
/// would force that second scan to race the watcher: the reader would read a
/// newer state from disk and label it with the older revision it was handed.
#[derive(serde::Serialize)]
struct WatchLine<'a> {
    revision: u64,
    inventory: &'a Inventory,
}

fn run_watch(scope: &ScopeArgs, since: Option<u64>, json: bool, time: bool) -> ExitCode {
    let interrupted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let interrupted = interrupted.clone();
        // Best-effort: if the handler cannot be installed, the process still
        // exits (non-130) on the next unhandled SIGINT rather than hanging.
        let _ = ctrlc::set_handler(move || {
            interrupted.store(true, std::sync::atomic::Ordering::SeqCst);
        });
    }

    let snapshots: SnapshotCell<Inventory> = SnapshotCell::new();
    let mut stdout = std::io::stdout();

    loop {
        if interrupted.load(std::sync::atomic::Ordering::SeqCst) {
            return ExitCode::from(130);
        }

        let rt = match build_runtime::<Inventory>(scope, Operation::Scan, json) {
            Ok(rt) => rt,
            Err(code) => return code,
        };
        let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
        let req = ScanRequest::default();
        let scan_result = ops::scan(&rt, &ctx, &req);
        print_timing(time, ctx.take_timing().as_ref());
        match scan_result {
            Ok(inventory) => {
                let previous = snapshots.current();
                let changed = previous.as_ref().is_none_or(|prev| prev.value != inventory);
                if changed {
                    let is_initial = previous.is_none();
                    let revision = snapshots.publish(inventory);
                    let suppress_initial = is_initial && since.is_some_and(|s| s == revision.0);
                    if !suppress_initial {
                        // `publish` just set this snapshot; `None` here
                        // would mean another thread cleared it between the
                        // two calls, which never happens in this
                        // single-threaded loop - skip the line rather than
                        // panic if that invariant is ever wrong.
                        let Some(published) = snapshots.current() else {
                            continue;
                        };
                        let line = WatchLine {
                            revision: revision.0,
                            inventory: &published.value,
                        };
                        print_watch_line(&mut stdout, &line, json);
                    }
                }
            }
            Err(err) => {
                eprintln!("{}: {}", err.code.as_str(), err.message);
            }
        }

        if interrupted.load(std::sync::atomic::Ordering::SeqCst) {
            return ExitCode::from(130);
        }
        std::thread::sleep(WATCH_POLL_INTERVAL);
    }
}

/// Prints one `watch` line and flushes immediately, so a piped reader sees
/// it without waiting for a full buffer.
fn print_watch_line(stdout: &mut std::io::Stdout, line: &WatchLine, json: bool) {
    use std::io::Write;
    if json {
        // `WatchLine` borrows only our own DTOs; nothing in it can produce a
        // non-string map key or a non-finite float, the only ways this errs.
        let text = serde_json::to_string(line).unwrap_or_else(|e| {
            format!(r#"{{"error":"failed to serialize the watch line: {e}"}}"#)
        });
        let _ = writeln!(stdout, "{text}");
    } else {
        let _ = writeln!(
            stdout,
            "revision {}: {} skill(s)",
            line.revision,
            line.inventory.skills.len()
        );
    }
    let _ = stdout.flush();
}

/// Shared tail for every `run_*`: prints JSON or the human table, then
/// returns the envelope's exit code.
fn finish<T: serde::Serialize + ops::Outcome>(
    envelope: &ResultEnvelope<T>,
    json: bool,
    time: bool,
    print_table: impl FnOnce(&ResultEnvelope<T>),
) -> ExitCode {
    let code = exit_code(envelope.exit_status());
    if json {
        output::print_json(envelope);
    } else {
        print_table(envelope);
    }
    print_timing(time, envelope.timings.as_ref());
    code
}
