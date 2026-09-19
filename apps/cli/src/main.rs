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
    CapabilitiesRequest, HarnessesRequest, InstallFile, InstallMethod, InstallPreferencesRequest,
    InstallRequest, Inventory, ListEventsRequest, ParkRequest, RepairApplyMode, RepairApplyRequest,
    RepairPreviewRequest, RestoreRequest, ScanRequest, UnparkRequest, UpdateRequest,
};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::health::{self, Outcome, TimingRow};
use skill_studio_core::identity::{
    AgentId, CorrelationId, DeploymentId, EventId, ProjectRef, RootScope, SkillName,
};
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

/// `--method` for `add`: the wire-level `InstallMethod`, spelled the way a
/// flag reads (`skills-sh`, not `skills_sh`).
#[derive(Clone, Copy, clap::ValueEnum)]
enum AddMethod {
    Copy,
    Dotagents,
    #[value(name = "skills-sh")]
    SkillsSh,
}

impl From<AddMethod> for InstallMethod {
    fn from(method: AddMethod) -> Self {
        match method {
            AddMethod::Copy => InstallMethod::Copy,
            AddMethod::Dotagents => InstallMethod::Dotagents,
            AddMethod::SkillsSh => InstallMethod::SkillsSh,
        }
    }
}

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
    /// Codex, `OpenCode`, pi).
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
    /// Run the doctor invariants for one skill and repair whatever it can;
    /// anything it cannot repair is named with its path.
    Fix {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Skill to fix.
        #[arg(long)]
        skill: String,
        #[arg(long)]
        json: bool,
    },
    /// Install one skill by `copy`, `dotagents`, or `skills-sh`.
    Add {
        #[command(flatten)]
        scope: ScopeArgs,
        /// For `copy`: the local skill folder to stage. For `dotagents`/
        /// `skills-sh`: the source argument passed to the CLI's own `add`.
        source: String,
        /// Which method writes the bytes.
        #[arg(long, value_enum, default_value_t = AddMethod::SkillsSh)]
        method: AddMethod,
        /// Harnesses to link the new skill into right after install
        /// (repeatable). Only Claude Code gets a per-skill link this build
        /// writes.
        #[arg(long = "harness")]
        harnesses: Vec<String>,
        /// Install under the scope home. Default when neither this nor
        /// `--project-path` is given.
        #[arg(long, conflicts_with = "project_path")]
        global: bool,
        /// Install under this project instead of the scope home. Named
        /// `--project-path`, not `--project`: `ScopeArgs` already flattens a
        /// repeatable `--project` for discovery scoping.
        #[arg(long)]
        project_path: Option<PathBuf>,
        /// Folder name the skill is installed under. For `copy`, defaults to
        /// `source`'s final path segment. For `dotagents`/`skills-sh`,
        /// required: it is the skill slug the repo publishes, which the CLI
        /// this build shells out to always writes under - a derived name
        /// can name the wrong folder for a multi-skill or differently named
        /// repo.
        #[arg(long)]
        name: Option<String>,
        /// Confirms the trust prompt for an untrusted dotagents source.
        #[arg(long)]
        trust: bool,
        #[arg(long)]
        json: bool,
    },
    /// Print the method and harnesses the next `add` pre-selects: the last
    /// install's saved preference, or the environment default when nothing
    /// has been saved for this scope yet.
    InstallPreferences {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Project whose preference to read; omit for the scope home's.
        /// Named `--project-path` for the same reason `add`'s flag is:
        /// `ScopeArgs` already flattens a repeatable `--project`.
        #[arg(long)]
        project_path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Find differing copies of a skill without merging them; writes
    /// nothing.
    Conflicts {
        #[command(flatten)]
        scope: ScopeArgs,
        #[arg(long)]
        json: bool,
    },
    /// Run every lifecycle invariant in `docs/action-map/lifecycle-states.md`
    /// over the whole scope; writes nothing. Exit code 0 with an empty
    /// violation list on a healthy home, 1 when any violation is found
    /// (the same "found something" code `scan`/`diagnose`/`fix` use).
    Doctor {
        #[command(flatten)]
        scope: ScopeArgs,
        #[arg(long)]
        json: bool,
    },
    /// Take a mutable deployment off disk. `Copy`/`Fork` land intact in
    /// quarantine; `Dotagents`/`SkillsSh` are removed by their own CLI.
    Remove {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Deployment to remove, as printed by `scan`.
        #[arg(long)]
        deployment_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Move a universal deployment's directory to the parked root and
    /// remove its Claude Code link, if any.
    Park {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Universal deployment to park, as printed by `scan`.
        #[arg(long)]
        deployment_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Move a parked deployment's directory back to the universal root and
    /// recreate its Claude Code link, if it had one.
    Unpark {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Parked deployment to restore, as printed by `scan`.
        #[arg(long)]
        deployment_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Report per-skill currency ("update available") for every install
    /// method that tracks one: skills.sh by tree SHA, dotagents by pinned
    /// commit, plugin by marketplace manifest version.
    Outdated {
        #[command(flatten)]
        scope: ScopeArgs,
        #[arg(long = "skill")]
        skills: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Prune the global quarantine cap without a `remove` call, via
    /// `ops::sweep_quarantine`. Global scope only - see that function's
    /// own doc.
    SweepQuarantine {
        #[command(flatten)]
        scope: ScopeArgs,
        #[arg(long)]
        json: bool,
    },
    /// Refresh one or more already-installed skills in place.
    Update {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Skill to refresh. Repeat the flag to refresh several in one
        /// batch; each one gets its own journal row via `ops::update_all`.
        #[arg(long = "skill", required = true)]
        skills: Vec<String>,
        /// Which method wrote the deployment being refreshed.
        #[arg(long, value_parser = ["copy", "dotagents", "skills-sh"])]
        method: String,
        /// Project the targeted deployment lives under; omit for the
        /// global (`.agents/skills`) deployment.
        #[arg(long)]
        project_path: Option<PathBuf>,
        /// `Copy` only: directory to read fresh files from, recursively.
        #[arg(long)]
        source_dir: Option<PathBuf>,
        /// `Dotagents`/`SkillsSh` only: the source argument the CLI's
        /// `add` command needs to re-fetch.
        #[arg(long)]
        source: Option<String>,
        /// `Dotagents` only: an already-resolved commit for a pinned
        /// (`declared_ref`) ledger entry.
        #[arg(long)]
        ref_pin: Option<String>,
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
        Command::Add {
            scope,
            source,
            method,
            harnesses,
            global,
            project_path,
            name,
            trust,
            json,
        } => {
            // `global` is `--project-path`'s own inverse for clap's help
            // text and `conflicts_with`; the request's scope is derived
            // from `project_path` alone (`None` means global), so this
            // flag carries no further meaning once parsing is done.
            let _ = global;
            run_add(
                &scope,
                AddArgs {
                    source,
                    method,
                    harnesses,
                    project: project_path,
                    name,
                    trust,
                },
                json,
                time,
            )
        }
        Command::Fix { scope, skill, json } => run_fix(&scope, &skill, json, time),
        Command::InstallPreferences {
            scope,
            project_path,
            json,
        } => run_install_preferences(&scope, project_path, json, time),
        Command::Conflicts { scope, json } => run_diagnose_conflict(&scope, json, time),
        Command::Doctor { scope, json } => run_doctor(&scope, json, time),
        Command::Remove {
            scope,
            deployment_id,
            json,
        } => run_remove(&scope, &deployment_id, json, time),
        Command::Park {
            scope,
            deployment_id,
            json,
        } => run_park(&scope, &deployment_id, json, time),
        Command::Unpark {
            scope,
            deployment_id,
            json,
        } => run_unpark(&scope, &deployment_id, json, time),
        Command::Outdated {
            scope,
            skills,
            json,
        } => run_outdated(&scope, skills, json, time),
        Command::SweepQuarantine { scope, json } => run_sweep_quarantine(&scope, json, time),
        Command::Update {
            scope,
            skills,
            method,
            project_path,
            source_dir,
            source,
            ref_pin,
            json,
        } => run_update(
            UpdateArgs {
                scope: &scope,
                skills,
                method,
                project_path,
                source_dir,
                source,
                ref_pin,
                json,
            },
            time,
        ),
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
    build_runtime_write_with_project::<T>(scope, operation, json, None)
}

/// Like [`build_runtime_write`], but folds `extra_project` into the scope
/// before it resolves. Only `add --project-path` needs this: it installs
/// into a project the scope was never otherwise told about (see
/// [`crate::scope::ScopeArgs::resolve_with_extra_project`]).
fn build_runtime_write_with_project<T: ops::Outcome + serde::Serialize>(
    scope: &ScopeArgs,
    operation: Operation,
    json: bool,
    extra_project: Option<&std::path::Path>,
) -> Result<Runtime, ExitCode> {
    let (runtime_scope, lease_root) = scope.resolve_with_extra_project(extra_project);
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

/// `add`'s own flags, bundled so `run_add` stays under clippy's
/// argument-count lint.
struct AddArgs {
    source: String,
    method: AddMethod,
    harnesses: Vec<String>,
    project: Option<PathBuf>,
    name: Option<String>,
    trust: bool,
}

/// Reads `dir` into the `InstallFile` list `InstallMethod::Copy` stages,
/// walking every subdirectory; each entry's `relative_path` is relative to
/// `dir` itself. `Dotagents`/`SkillsSh` never call this - their bytes come
/// from the CLI the core op shells out to.
fn read_skill_files(dir: &std::path::Path) -> std::io::Result<Vec<InstallFile>> {
    fn walk(
        root: &std::path::Path,
        dir: &std::path::Path,
        out: &mut Vec<InstallFile>,
    ) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            // `DirEntry::file_type` is `lstat`-based and reports a symlink as
            // neither a file nor a directory, so a symlinked file would
            // silently drop out of the copy. `std::fs::metadata` follows the
            // link and reports what it points at.
            let metadata = std::fs::metadata(&path)?;
            if metadata.is_dir() {
                walk(root, &path, out)?;
            } else if metadata.is_file() {
                let contents = std::fs::read(&path)?;
                let relative_path = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                out.push(InstallFile {
                    relative_path,
                    contents,
                });
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

/// Parses one `--harness` value, rejecting anything `catalog` does not
/// recognize. `AgentId::parse` alone only checks the kebab-case shape, so a
/// well-formed but unknown id (a typo, or a harness this build never
/// shipped) would otherwise reach `ops::install` and fail there with a less
/// specific error.
fn parse_known_harness(
    raw: &str,
    catalog: &HarnessCatalog,
) -> Result<AgentId, skill_studio_core::CoreError> {
    let id = AgentId::parse(raw)?;
    if catalog.get(&id).is_some() {
        Ok(id)
    } else {
        let accepted = catalog
            .facts
            .iter()
            .map(|f| f.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Err(skill_studio_core::CoreError::new(
            skill_studio_core::ErrorCode::InvalidRequest,
            format!("`{raw}` is not a known harness; accepted values: {accepted}"),
        ))
    }
}

/// Installs one skill via `ops::install`, by `Copy`, `Dotagents`, or
/// `SkillsSh`. `NeedsTrust` is printed/returned as a non-error outcome, not
/// an exit failure - the caller retries with `--trust` once it confirms.
fn run_add(scope: &ScopeArgs, args: AddArgs, json: bool, time: bool) -> ExitCode {
    let rt = match build_runtime_write_with_project::<skill_studio_core::dto::InstallOutcome>(
        scope,
        Operation::Install,
        json,
        args.project.as_deref(),
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let method: InstallMethod = args.method.into();
    // `dotagents`/`skills-sh` shell out to `npx skills add`, which always
    // installs under the repo's own skill slug; a derived `--name` (the
    // repo's last path segment) names the wrong folder for a multi-skill or
    // differently named repo, so the CLI requires the caller to say the
    // slug. `copy` has no such mismatch: its name is the folder it copies.
    if args.name.is_none() && !matches!(method, InstallMethod::Copy) {
        let err = skill_studio_core::CoreError::new(
            skill_studio_core::ErrorCode::InvalidRequest,
            "--name is required for --method dotagents or skills-sh: it is the skill slug the repo publishes",
        );
        let envelope = ResultEnvelope::<skill_studio_core::dto::InstallOutcome>::from_result(
            Operation::Install,
            &rt.scope,
            &ctx,
            Err(err),
        );
        return finish(&envelope, json, time, output::print_install_outcome_table);
    }
    let name = args.name.clone().unwrap_or_else(|| {
        std::path::Path::new(&args.source)
            .file_name()
            .map_or_else(|| args.source.clone(), |n| n.to_string_lossy().into_owned())
    });
    let harnesses = match args
        .harnesses
        .iter()
        .map(|h| parse_known_harness(h, &rt.ports.catalog))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(harnesses) => harnesses,
        Err(err) => {
            let envelope = ResultEnvelope::<skill_studio_core::dto::InstallOutcome>::from_result(
                Operation::Install,
                &rt.scope,
                &ctx,
                Err(err),
            );
            return finish(&envelope, json, time, output::print_install_outcome_table);
        }
    };
    let files = if matches!(method, InstallMethod::Copy) {
        match read_skill_files(std::path::Path::new(&args.source)) {
            Ok(files) => files,
            Err(e) => {
                let err = skill_studio_core::CoreError::io(&args.source, e);
                let envelope =
                    ResultEnvelope::<skill_studio_core::dto::InstallOutcome>::from_result(
                        Operation::Install,
                        &rt.scope,
                        &ctx,
                        Err(err),
                    );
                return finish(&envelope, json, time, output::print_install_outcome_table);
            }
        }
    } else {
        Vec::new()
    };
    let scope_target = match args.project {
        Some(project) => RootScope::Project(ProjectRef(project)),
        None => RootScope::Global,
    };
    let req = InstallRequest {
        skill: SkillName(name),
        method,
        scope: scope_target,
        harnesses,
        files,
        source: (!matches!(method, InstallMethod::Copy)).then_some(args.source),
        trust_identity: None,
        trust_confirmed: args.trust,
        save_as_preference: true,
    };
    let result = ops::install(&rt, &ctx, &req);
    let envelope = ResultEnvelope::from_result(Operation::Install, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_install_outcome_table)
}

fn run_fix(scope: &ScopeArgs, skill: &str, json: bool, time: bool) -> ExitCode {
    let rt = match build_runtime_write::<skill_studio_core::dto::FixSkillOutcome>(
        scope,
        Operation::FixSkill,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let req = skill_studio_core::dto::FixSkillRequest {
        skill: SkillName(skill.to_string()),
    };
    let result = ops::fix_skill(&rt, &ctx, &req);
    let envelope = ResultEnvelope::from_result(Operation::FixSkill, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_fix_outcome_table)
}

/// Reads one scope's saved install preference, via
/// `ops::install_preferences`. A read: it never writes the preference back,
/// which only a completed `add` does.
fn run_install_preferences(
    scope: &ScopeArgs,
    project_path: Option<PathBuf>,
    json: bool,
    time: bool,
) -> ExitCode {
    let rt = match build_runtime::<skill_studio_core::dto::InstallPreferences>(
        scope,
        Operation::InstallPreferences,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let req = InstallPreferencesRequest {
        scope: match project_path {
            Some(project) => RootScope::Project(ProjectRef(project)),
            None => RootScope::Global,
        },
    };
    let result = ops::install_preferences(&rt, &ctx, &req.scope);
    let envelope =
        ResultEnvelope::from_result(Operation::InstallPreferences, &rt.scope, &ctx, result);
    finish(
        &envelope,
        json,
        time,
        output::print_install_preferences_table,
    )
}

fn run_diagnose_conflict(scope: &ScopeArgs, json: bool, time: bool) -> ExitCode {
    let rt = match build_runtime::<skill_studio_core::dto::ConflictReport>(
        scope,
        Operation::DiagnoseConflict,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let result = ops::diagnose_conflict(
        &rt,
        &ctx,
        &skill_studio_core::dto::DiagnoseConflictRequest::default(),
    );
    let envelope =
        ResultEnvelope::from_result(Operation::DiagnoseConflict, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_conflict_report_table)
}

fn run_doctor(scope: &ScopeArgs, json: bool, time: bool) -> ExitCode {
    let rt =
        match build_runtime::<skill_studio_core::dto::DoctorReport>(scope, Operation::Doctor, json)
        {
            Ok(rt) => rt,
            Err(code) => return code,
        };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let result = ops::doctor(&rt, &ctx, &skill_studio_core::dto::DoctorRequest::default());
    let envelope = ResultEnvelope::from_result(Operation::Doctor, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_doctor_report_table)
}

/// Reads every regular file under `dir` (recursively) into an
/// [`InstallFile`] list with paths relative to `dir`, sorted by path so a
/// re-run stages the same bytes in the same order. Used only for `--method
/// copy`'s `--source-dir`; `dotagents`/`skills-sh` re-fetch through their
/// own CLI and never call this.
fn read_install_files(
    dir: &std::path::Path,
) -> Result<Vec<InstallFile>, skill_studio_core::CoreError> {
    fn walk(
        root: &std::path::Path,
        dir: &std::path::Path,
        out: &mut Vec<InstallFile>,
    ) -> Result<(), skill_studio_core::CoreError> {
        let entries =
            std::fs::read_dir(dir).map_err(|e| skill_studio_core::CoreError::io(dir, e))?;
        let mut names: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        names.sort();
        for path in names {
            let meta = std::fs::symlink_metadata(&path)
                .map_err(|e| skill_studio_core::CoreError::io(&path, e))?;
            if meta.is_dir() {
                walk(root, &path, out)?;
            } else if meta.is_file() {
                let contents =
                    std::fs::read(&path).map_err(|e| skill_studio_core::CoreError::io(&path, e))?;
                let relative_path = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                out.push(InstallFile {
                    relative_path,
                    contents,
                });
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

/// Parses `--method`. `clap`'s `value_parser` already restricts the raw
/// string to the three names below, so this never sees anything else.
fn parse_install_method(method: &str) -> InstallMethod {
    match method {
        "copy" => InstallMethod::Copy,
        "dotagents" => InstallMethod::Dotagents,
        _ => InstallMethod::SkillsSh,
    }
}

/// `Command::Update`'s own clap fields, carried as one value (U6): the
/// command has more of its own inputs than `clippy::too_many_arguments`
/// allows as separate parameters, and every one of them already comes from
/// a single clap variant, so a struct names the grouping the flags already
/// have instead of suppressing the lint.
struct UpdateArgs<'a> {
    scope: &'a ScopeArgs,
    skills: Vec<String>,
    method: String,
    project_path: Option<PathBuf>,
    source_dir: Option<PathBuf>,
    source: Option<String>,
    ref_pin: Option<String>,
    json: bool,
}

fn run_update(args: UpdateArgs<'_>, time: bool) -> ExitCode {
    let UpdateArgs {
        scope,
        skills,
        method,
        project_path,
        source_dir,
        source,
        ref_pin,
        json,
    } = args;
    let operation = if skills.len() > 1 {
        Operation::UpdateAll
    } else {
        Operation::Update
    };
    let rt = match build_runtime_write::<skill_studio_core::dto::UpdateAllOutcome>(
        scope, operation, json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let method = parse_install_method(&method);
    let files = match (method, source_dir.as_ref()) {
        (InstallMethod::Copy, Some(dir)) => match read_install_files(dir) {
            Ok(files) => files,
            Err(err) => {
                let envelope =
                    ResultEnvelope::<skill_studio_core::dto::UpdateAllOutcome>::from_result(
                        operation,
                        &rt.scope,
                        &ctx,
                        Err(err),
                    );
                return finish(
                    &envelope,
                    json,
                    time,
                    output::print_update_all_outcome_table,
                );
            }
        },
        (InstallMethod::Copy, None) => {
            let err = skill_studio_core::CoreError::new(
                skill_studio_core::ErrorCode::InvalidRequest,
                "update --method copy needs --source-dir",
            );
            let envelope = ResultEnvelope::<skill_studio_core::dto::UpdateAllOutcome>::from_result(
                operation,
                &rt.scope,
                &ctx,
                Err(err),
            );
            return finish(
                &envelope,
                json,
                time,
                output::print_update_all_outcome_table,
            );
        }
        _ => Vec::new(),
    };
    let scope_field = match project_path {
        Some(path) => RootScope::Project(ProjectRef(path.clone())),
        None => RootScope::Global,
    };
    let requests: Vec<UpdateRequest> = skills
        .into_iter()
        .map(|skill| UpdateRequest {
            skill: SkillName(skill),
            method,
            scope: scope_field.clone(),
            files: files.clone(),
            source: source.clone(),
            ref_pin: ref_pin.clone(),
        })
        .collect();
    if requests.len() == 1 {
        let result = ops::update(&rt, &ctx, &requests[0]);
        let envelope = ResultEnvelope::from_result(Operation::Update, &rt.scope, &ctx, result);
        return finish(&envelope, json, time, output::print_update_outcome_table);
    }
    let outcome = ops::update_all(&rt, &ctx, &requests, |_, _| {});
    let envelope = ResultEnvelope::from_result(Operation::UpdateAll, &rt.scope, &ctx, Ok(outcome));
    finish(
        &envelope,
        json,
        time,
        output::print_update_all_outcome_table,
    )
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

/// Takes a mutable deployment off disk, via `ops::remove`.
fn run_remove(scope: &ScopeArgs, deployment_id: &str, json: bool, time: bool) -> ExitCode {
    let rt = match build_runtime_write::<skill_studio_core::dto::RemoveOutcome>(
        scope,
        Operation::Remove,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let deployment_id = match DeploymentId::parse(deployment_id) {
        Ok(id) => id,
        Err(err) => {
            let envelope = ResultEnvelope::<skill_studio_core::dto::RemoveOutcome>::from_result(
                Operation::Remove,
                &rt.scope,
                &ctx,
                Err(err),
            );
            return finish(&envelope, json, time, output::print_remove_outcome_table);
        }
    };
    let req = skill_studio_core::dto::RemoveRequest { deployment_id };
    let result = ops::remove(&rt, &ctx, &req);
    let envelope = ResultEnvelope::from_result(Operation::Remove, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_remove_outcome_table)
}

/// Moves a universal deployment to the parked root, via `ops::park`.
fn run_park(scope: &ScopeArgs, deployment_id: &str, json: bool, time: bool) -> ExitCode {
    let rt = match build_runtime_write::<skill_studio_core::dto::ParkOutcome>(
        scope,
        Operation::Park,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let deployment_id = match DeploymentId::parse(deployment_id) {
        Ok(id) => id,
        Err(err) => {
            let envelope = ResultEnvelope::<skill_studio_core::dto::ParkOutcome>::from_result(
                Operation::Park,
                &rt.scope,
                &ctx,
                Err(err),
            );
            return finish(&envelope, json, time, output::print_park_outcome_table);
        }
    };
    let req = ParkRequest { deployment_id };
    let result = ops::park(&rt, &ctx, &req);
    let envelope = ResultEnvelope::from_result(Operation::Park, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_park_outcome_table)
}

/// Moves a parked deployment back to the universal root, via `ops::unpark`.
fn run_unpark(scope: &ScopeArgs, deployment_id: &str, json: bool, time: bool) -> ExitCode {
    let rt = match build_runtime_write::<skill_studio_core::dto::UnparkOutcome>(
        scope,
        Operation::Unpark,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let deployment_id = match DeploymentId::parse(deployment_id) {
        Ok(id) => id,
        Err(err) => {
            let envelope = ResultEnvelope::<skill_studio_core::dto::UnparkOutcome>::from_result(
                Operation::Unpark,
                &rt.scope,
                &ctx,
                Err(err),
            );
            return finish(&envelope, json, time, output::print_unpark_outcome_table);
        }
    };
    let req = UnparkRequest { deployment_id };
    let result = ops::unpark(&rt, &ctx, &req);
    let envelope = ResultEnvelope::from_result(Operation::Unpark, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_unpark_outcome_table)
}

/// A [`skill_studio_core::skill_update_check::SourceTreeLookup`],
/// [`skill_studio_core::skill_update_check::CommitLookup`], and
/// [`skill_studio_core::skill_update_check::PluginManifestLookup`] all in
/// one: when `gh` is not on `PATH`, every lookup a currency check makes
/// fails, which `ops::outdated` already turns into `Currency::Unknown` per
/// skill rather than a hard error - matching the desktop's own fallback for
/// an unresolved `gh` binary.
struct NoGhLookup;

impl skill_studio_core::skill_update_check::SourceTreeLookup for NoGhLookup {
    fn tree_shas_at_head(
        &self,
        _repo: &str,
    ) -> Result<std::collections::HashMap<String, String>, skill_studio_core::CoreError> {
        Err(skill_studio_core::CoreError::new(
            skill_studio_core::ErrorCode::Unsupported,
            "gh is not on PATH",
        ))
    }
}

impl skill_studio_core::skill_update_check::CommitLookup for NoGhLookup {
    fn latest_commit(
        &self,
        _repo: &str,
        _path: &str,
    ) -> Result<Option<String>, skill_studio_core::CoreError> {
        Err(skill_studio_core::CoreError::new(
            skill_studio_core::ErrorCode::Unsupported,
            "gh is not on PATH",
        ))
    }
}

impl skill_studio_core::skill_update_check::PluginManifestLookup for NoGhLookup {
    fn marketplace_version(
        &self,
        _marketplace: &str,
        _plugin: &str,
    ) -> Result<Option<String>, skill_studio_core::CoreError> {
        Ok(None)
    }
}

/// Reports per-skill currency, via `ops::outdated`. Resolves `gh` off
/// `rt.ports.tools` the same way other CLI surfaces resolve external
/// binaries; a machine with no `gh` still returns a result, with every
/// skills.sh/dotagents skill's currency `Unknown` rather than an error.
fn run_outdated(scope: &ScopeArgs, skills: Vec<String>, json: bool, time: bool) -> ExitCode {
    let rt = match build_runtime_write::<
        std::collections::BTreeMap<String, skill_studio_core::skill_update_check::Currency>,
    >(scope, Operation::Outdated, json)
    {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let req = ScanRequest {
        skills: skills.into_iter().map(SkillName).collect(),
        timings: false,
    };
    let gh_bin = rt.ports.tools.as_ref().and_then(|t| t.find_binary("gh"));
    let result = match gh_bin {
        Some(gh_bin) => ops::outdated(
            &rt,
            &ctx,
            &req,
            &skill_studio_host::GhSourceTreeLookup::new(gh_bin.clone()),
            &skill_studio_host::GhCommitLookup::new(gh_bin),
            &skill_studio_host::GhPluginManifestLookup,
        ),
        None => ops::outdated(&rt, &ctx, &req, &NoGhLookup, &NoGhLookup, &NoGhLookup),
    };
    let envelope = ResultEnvelope::from_result(Operation::Outdated, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_outdated_table)
}

/// Prunes the global quarantine cap, via `ops::sweep_quarantine`. Global
/// scope only, matching the desktop's own startup sweep
/// (`skill_refresh.rs::run_startup_quarantine_sweep`) - a project's
/// `.agents/skills` quarantine directory is swept the next time that
/// project's own `remove` runs.
fn run_sweep_quarantine(scope: &ScopeArgs, json: bool, time: bool) -> ExitCode {
    let rt = match build_runtime_write::<()>(scope, Operation::SweepQuarantine, json) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let result = ops::sweep_quarantine(&rt, &ctx, &RootScope::Global);
    let envelope = ResultEnvelope::from_result(Operation::SweepQuarantine, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_sweep_quarantine_table)
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
