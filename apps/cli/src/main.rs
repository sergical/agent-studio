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
    CapabilitiesRequest, Inventory, ListEventsRequest, RepairApplyMode, RepairApplyRequest,
    RepairPreviewRequest, RestoreRequest, ScanRequest,
};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{AgentId, CorrelationId, DeploymentId, EventId, SkillName};
use skill_studio_core::ops::{self, Operation, ResultEnvelope};
use skill_studio_core::ports::{OpContext, Runtime};
use skill_studio_core::snapshot::SnapshotCell;

use crate::scope::ScopeArgs;

#[derive(Parser)]
#[command(name = "skill-studio", about = "Manage agent skills across harnesses")]
struct Cli {
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
    /// Write one JSON Schema file per request/result DTO.
    Schema {
        /// Directory to write schema files into.
        #[arg(long)]
        out: Option<PathBuf>,
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
    match cli.command {
        Command::Scan {
            scope,
            skills,
            timings,
            json,
        } => run_scan(scope, skills, timings, json),
        Command::Diagnose {
            scope,
            skills,
            timings,
            json,
        } => run_diagnose(scope, skills, timings, json),
        Command::Capabilities {
            scope,
            harnesses,
            observe,
            tools,
            json,
        } => run_capabilities(scope, harnesses, observe, tools, json),
        Command::PreviewRepair {
            scope,
            deployment_id,
            json,
        } => run_preview_repair(scope, deployment_id, json),
        Command::ApplyRepair {
            scope,
            preview_json,
            json,
        } => run_apply_repair(scope, preview_json, json),
        Command::Events {
            scope,
            skill,
            limit,
            after,
            check_drift,
            json,
        } => run_events(scope, skill, limit, after, check_drift, json),
        Command::Restore {
            scope,
            event_id,
            force,
            json,
        } => run_restore(scope, event_id, force, json),
        Command::Schema { out } => output::write_schemas(out),
        Command::Watch { scope, since, json } => run_watch(scope, since, json),
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
        ports.discovery = Some(Arc::new(
            skill_studio_host::TranscriptProjectDiscovery::new(),
        ));
    }
    ports.tools = Some(Arc::new(skill_studio_host::PathToolLookup::new()));
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
    }
}

fn exit_code(status: i32) -> ExitCode {
    ExitCode::from(status.clamp(0, 255) as u8)
}

fn run_scan(scope: ScopeArgs, skills: Vec<String>, timings: bool, json: bool) -> ExitCode {
    let rt = match build_runtime::<skill_studio_core::dto::Inventory>(&scope, Operation::Scan, json)
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
    let envelope =
        ResultEnvelope::from_result(Operation::Scan, &rt.scope, ctx.correlation_id, result);
    let code = exit_code(envelope.exit_status());
    if json {
        output::print_json(&envelope);
    } else {
        output::print_scan_table(&envelope);
    }
    code
}

fn run_diagnose(scope: ScopeArgs, skills: Vec<String>, timings: bool, json: bool) -> ExitCode {
    let rt =
        match build_runtime::<skill_studio_core::dto::Diagnosis>(&scope, Operation::Diagnose, json)
        {
            Ok(rt) => rt,
            Err(code) => return code,
        };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let req = ScanRequest {
        skills: skills.into_iter().map(SkillName).collect(),
        timings,
    };
    let result = ops::diagnose(&rt, &ctx, &req);
    let envelope =
        ResultEnvelope::from_result(Operation::Diagnose, &rt.scope, ctx.correlation_id, result);
    let code = exit_code(envelope.exit_status());
    if json {
        output::print_json(&envelope);
    } else {
        output::print_diagnose_table(&envelope);
    }
    code
}

fn run_capabilities(
    scope: ScopeArgs,
    harnesses: Vec<String>,
    observe: bool,
    tools: Vec<String>,
    json: bool,
) -> ExitCode {
    let rt = match build_runtime::<skill_studio_core::harness::Capabilities>(
        &scope,
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
                ctx.correlation_id,
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
    let envelope = ResultEnvelope::from_result(
        Operation::Capabilities,
        &rt.scope,
        ctx.correlation_id,
        result,
    );
    let code = exit_code(envelope.exit_status());
    if json {
        output::print_json(&envelope);
    } else {
        output::print_capabilities_table(&envelope);
    }
    code
}

fn run_preview_repair(scope: ScopeArgs, deployment_id: String, json: bool) -> ExitCode {
    let rt = match build_runtime::<skill_studio_core::dto::FrontmatterRepairPreview>(
        &scope,
        Operation::PreviewFrontmatterRepair,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let deployment_id = match DeploymentId::parse(&deployment_id) {
        Ok(id) => id,
        Err(err) => {
            let envelope =
                ResultEnvelope::<skill_studio_core::dto::FrontmatterRepairPreview>::from_result(
                    Operation::PreviewFrontmatterRepair,
                    &rt.scope,
                    ctx.correlation_id,
                    Err(err),
                );
            return finish(envelope, json, output::print_repair_preview_table);
        }
    };
    let req = RepairPreviewRequest { deployment_id };
    let result = ops::preview_frontmatter_repair(&rt, &ctx, &req);
    let envelope = ResultEnvelope::from_result(
        Operation::PreviewFrontmatterRepair,
        &rt.scope,
        ctx.correlation_id,
        result,
    );
    finish(envelope, json, output::print_repair_preview_table)
}

fn run_apply_repair(scope: ScopeArgs, preview_json: PathBuf, json: bool) -> ExitCode {
    let rt = match build_runtime_write::<skill_studio_core::dto::RepairOutcome>(
        &scope,
        Operation::ApplyFrontmatterRepair,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let preview = match std::fs::read_to_string(&preview_json)
        .map_err(|e| skill_studio_core::CoreError::io(&preview_json, e))
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
                ctx.correlation_id,
                Err(err),
            );
            return finish(envelope, json, output::print_repair_outcome_table);
        }
    };
    let req = RepairApplyRequest {
        preview,
        mode: RepairApplyMode::ApplyFix,
    };
    let result = ops::apply_frontmatter_repair(&rt, &ctx, &req);
    let envelope = ResultEnvelope::from_result(
        Operation::ApplyFrontmatterRepair,
        &rt.scope,
        ctx.correlation_id,
        result,
    );
    finish(envelope, json, output::print_repair_outcome_table)
}

fn run_events(
    scope: ScopeArgs,
    skill: Option<String>,
    limit: u32,
    after: Option<String>,
    check_drift: bool,
    json: bool,
) -> ExitCode {
    // `list_events` only ever opens `HistoryAccess::ReadIfExists`, but it
    // still needs the real `SqliteHistoryOpener` (not `build_runtime`'s
    // no-op history) to see rows a prior `apply-repair`/`restore` wrote.
    let rt = match build_runtime_write::<Vec<skill_studio_core::dto::EventDto>>(
        &scope,
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
    let envelope =
        ResultEnvelope::from_result(Operation::ListEvents, &rt.scope, ctx.correlation_id, result);
    finish(envelope, json, output::print_events_table)
}

fn run_restore(scope: ScopeArgs, event_id: String, force: bool, json: bool) -> ExitCode {
    let rt = match build_runtime_write::<skill_studio_core::dto::RestoreOutcome>(
        &scope,
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
    let envelope = ResultEnvelope::from_result(
        Operation::RestoreEvent,
        &rt.scope,
        ctx.correlation_id,
        result,
    );
    finish(envelope, json, output::print_restore_outcome_table)
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

fn run_watch(scope: ScopeArgs, since: Option<u64>, json: bool) -> ExitCode {
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

        let rt = match build_runtime::<Inventory>(&scope, Operation::Scan, json) {
            Ok(rt) => rt,
            Err(code) => return code,
        };
        let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
        let req = ScanRequest::default();
        match ops::scan(&rt, &ctx, &req) {
            Ok(inventory) => {
                let previous = snapshots.current();
                let changed = previous.as_ref().is_none_or(|prev| prev.value != inventory);
                if changed {
                    let is_initial = previous.is_none();
                    let revision = snapshots.publish(inventory);
                    let suppress_initial = is_initial && since.is_some_and(|s| s == revision.0);
                    if !suppress_initial {
                        let published = snapshots.current().expect("just published");
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
        let _ = writeln!(stdout, "{}", serde_json::to_string(line).unwrap());
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
    envelope: ResultEnvelope<T>,
    json: bool,
    print_table: impl FnOnce(&ResultEnvelope<T>),
) -> ExitCode {
    let code = exit_code(envelope.exit_status());
    if json {
        output::print_json(&envelope);
    } else {
        print_table(&envelope);
    }
    code
}
