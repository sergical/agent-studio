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
    CapabilitiesRequest, HarnessesRequest, InstallFile, InstallLinkMode, InstallMethod,
    InstallPreferencesRequest, InstallRequest, Inventory, ListEventsRequest, ParkRequest,
    RepairApplyMode, RepairApplyRequest, RepairPreviewRequest, RestoreRequest, ScanRequest,
    UnparkRequest, UpdateRequest,
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
#[command(
    name = "skill-studio",
    about = "Tidy up your agent skills: find broken, duplicate and unused skills, and clear them out safely, with undo.",
    version = VERSION
)]
struct Cli {
    /// Print how long each step took, to stderr.
    #[arg(long, global = true)]
    time: bool,
    #[command(subcommand)]
    command: Command,
}

// clap lists subcommands in declaration order: the user commands come first,
// in the order a person tidies skills, and the `hide = true` dev commands
// follow.
#[derive(Subcommand)]
enum Command {
    /// List every skill installed for your agents.
    Scan {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Only show this skill. Repeat for more than one.
        #[arg(long = "skill")]
        skills: Vec<String>,
        /// Include per-phase timings.
        #[arg(long, hide = true)]
        timings: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Find broken skills and say what is wrong with each.
    Diagnose {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Only check this skill. Repeat for more than one.
        #[arg(long = "skill")]
        skills: Vec<String>,
        /// Include per-phase timings.
        #[arg(long, hide = true)]
        timings: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Find skills that have different copies in different places.
    Conflicts {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show which skills your agents used, and which they never used.
    Usage {
        #[command(flatten)]
        scope: ScopeArgs,
        /// How many days back to count uses.
        #[arg(long, default_value_t = skill_studio_host::DEFAULT_USAGE_DAYS)]
        days: u32,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Turn a skill off for all agents. You can turn it on again later.
    Park {
        #[command(flatten)]
        scope: ScopeArgs,
        #[command(flatten)]
        target: TargetArgs,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Turn a parked skill on again.
    Unpark {
        #[command(flatten)]
        scope: ScopeArgs,
        #[command(flatten)]
        target: TargetArgs,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Same as `unpark`: turn a parked skill on again.
    Enable {
        #[command(flatten)]
        scope: ScopeArgs,
        #[command(flatten)]
        target: TargetArgs,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Same as `park`: turn a skill off for all agents.
    Disable {
        #[command(flatten)]
        scope: ScopeArgs,
        #[command(flatten)]
        target: TargetArgs,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove a skill. Copies you made go to a recovery folder.
    Remove {
        #[command(flatten)]
        scope: ScopeArgs,
        #[command(flatten)]
        target: TargetArgs,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Repair a skill's problems and list the ones it cannot repair.
    Fix {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Skill to fix.
        #[arg(long)]
        skill: String,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Reverse your last change.
    Undo {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Reverse it even if the files changed after it.
        #[arg(long)]
        force: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List your past changes, newest first.
    #[command(visible_alias = "history")]
    Events {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Only show changes to this skill.
        #[arg(long)]
        skill: Option<String>,
        /// Most changes to show. 0 shows the default number.
        #[arg(long, default_value_t = 0)]
        limit: u32,
        /// Only show changes older than this one. Use an id from this list.
        #[arg(long)]
        after: Option<String>,
        /// Also check which files changed after each change.
        #[arg(long)]
        check_drift: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Reverse one change from your history.
    Restore {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Change to reverse, as printed by `events`.
        #[arg(long)]
        event_id: String,
        /// Reverse it even if the files changed after it.
        #[arg(long)]
        force: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Install a skill for one or more agents.
    Add {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Where the skill comes from: a repo such as owner/repo for
        /// skills-sh or dotagents, or a local folder for copy.
        source: String,
        /// How to install the skill.
        #[arg(long, value_enum, default_value_t = AddMethod::SkillsSh)]
        method: AddMethod,
        /// Agent to install for. Repeat for more than one: universal (the
        /// shared folder), claude-code, codex, opencode, cursor, pi or
        /// grok-build. Default: universal.
        #[arg(long = "agent", alias = "harness", value_name = "AGENT")]
        harnesses: Vec<String>,
        /// Put a real copy in each agent's folder, not a link to the shared
        /// copy.
        #[arg(long)]
        copy: bool,
        /// Install for your user account. This is the default.
        #[arg(long, conflicts_with = "project_path")]
        global: bool,
        /// Install into this project instead of for your user account.
        // Not `--project`: `ScopeArgs` already flattens a repeatable
        // `--project` for discovery.
        #[arg(long)]
        project_path: Option<PathBuf>,
        /// Folder name for the skill. Needed for skills-sh and dotagents: it
        /// is the skill's name in the repo. For copy, the default is the
        /// source folder's name.
        #[arg(long)]
        name: Option<String>,
        /// Trust a dotagents source you have not used before.
        #[arg(long)]
        trust: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List skills that have an update.
    Outdated {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Only check this skill. Repeat for more than one.
        #[arg(long = "skill")]
        skills: Vec<String>,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Update installed skills.
    Update {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Skill to update. Repeat for more than one.
        #[arg(long = "skill", required = true)]
        skills: Vec<String>,
        /// How the skill was installed.
        #[arg(long, value_parser = ["copy", "dotagents", "skills-sh"])]
        method: String,
        /// Project the skill is in. Leave it out for your global skills.
        #[arg(long)]
        project_path: Option<PathBuf>,
        /// For copy: the folder to read the new files from.
        #[arg(long)]
        source_dir: Option<PathBuf>,
        /// For skills-sh or dotagents: where to get the skill again, such as
        /// owner/repo.
        #[arg(long)]
        source: Option<String>,
        /// For dotagents: the commit to update to.
        #[arg(long)]
        ref_pin: Option<String>,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Start the MCP server so an agent can tidy your skills.
    Mcp,
    /// Run every lifecycle invariant in `docs/action-map/lifecycle-states.md`
    /// over the whole scope; writes nothing. Exit code 0 with an empty
    /// violation list on a healthy home, 1 when any violation is found
    /// (the same "found something" code `scan`/`diagnose`/`fix` use).
    #[command(hide = true)]
    Doctor {
        #[command(flatten)]
        scope: ScopeArgs,
        #[arg(long)]
        json: bool,
    },
    /// Replace a Universal skill folder with one real copy per chosen
    /// harness. Harnesses not named lose the skill.
    #[command(hide = true)]
    Split {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Universal deployment to split, as printed by `scan`.
        #[arg(long)]
        deployment_id: String,
        /// A harness that keeps the skill. Repeat for more than one.
        #[arg(long = "harness", required = true)]
        harnesses: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Report harness capability facts.
    #[command(hide = true)]
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
    #[command(hide = true)]
    Harnesses {
        #[command(flatten)]
        scope: ScopeArgs,
        #[arg(long)]
        json: bool,
    },
    /// Preview a frontmatter repair for one deployment, without writing.
    #[command(hide = true)]
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
    #[command(hide = true)]
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
    /// Print the method and harnesses the next `add` pre-selects: the last
    /// install's saved preference, or the environment default when nothing
    /// has been saved for this scope yet.
    #[command(hide = true)]
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
    /// Prune the global quarantine cap without a `remove` call, via
    /// `ops::sweep_quarantine`. Global scope only - see that function's
    /// own doc.
    #[command(hide = true)]
    SweepQuarantine {
        #[command(flatten)]
        scope: ScopeArgs,
        #[arg(long)]
        json: bool,
    },
    /// Write one JSON Schema file per request/result DTO.
    #[command(hide = true)]
    Schema {
        /// Directory to write schema files into.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Print the command health rollup: count, failures, p50/p95 duration,
    /// and last error, per command, over the last 7 days of `timing.jsonl`.
    #[command(hide = true)]
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
    #[command(hide = true)]
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

/// The copy `park`, `unpark` and `remove` act on: a skill name, or the id
/// `scan` prints when the name matches more than one copy.
#[derive(clap::Args)]
struct TargetArgs {
    /// Name of the skill.
    #[arg(required_unless_present = "id", conflicts_with = "id")]
    skill: Option<String>,
    /// Id of one copy, as printed by `scan`. Use it when the name matches
    /// more than one copy.
    // `--deployment-id` is the flag's name before names were accepted;
    // scripts and the test suite still pass it.
    #[arg(long, alias = "deployment-id")]
    id: Option<String>,
}

fn main() -> ExitCode {
    // Parse before telemetry starts: `mcp` reports as its own surface, and
    // `run_stdio` sets that up itself.
    let cli = Cli::parse();
    if matches!(cli.command, Command::Mcp) {
        return match skill_studio_mcp::run_stdio() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("The MCP server stopped: {err}");
                ExitCode::FAILURE
            }
        };
    }

    // Consent lives in the same home `ScopeArgs::resolve`'s unflagged branch
    // reads, resolved once here rather than per-command: `--fixture`/`--home`
    // point a single invocation's scope elsewhere, but the switch itself
    // always lives on the real machine.
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let registry_telemetry_enabled = skill_studio_host::telemetry::consent_from_registry(&home);
    let consent =
        skill_studio_host::telemetry::Consent::new(skill_studio_host::telemetry::resolve_consent(
            std::env::var("SKILL_STUDIO_TELEMETRY").ok(),
            registry_telemetry_enabled,
        ));
    let _telemetry_guard = skill_studio_host::telemetry::init(
        skill_studio_host::telemetry::Surface::Cli,
        env!("CARGO_PKG_VERSION"),
        consent,
    );

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
        Command::Usage { scope, days, json } => run_usage(&scope, days, json, time),
        Command::Mcp => unreachable!("handled before telemetry starts"),
        Command::Add {
            scope,
            source,
            method,
            harnesses,
            copy,
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
                    copy,
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
            target,
            json,
        } => run_remove(&scope, &target, json, time),
        Command::Park {
            scope,
            target,
            json,
        }
        | Command::Disable {
            scope,
            target,
            json,
        } => run_park(&scope, &target, json, time),
        Command::Split {
            scope,
            deployment_id,
            harnesses,
            json,
        } => run_split(&scope, &deployment_id, &harnesses, json, time),
        Command::Unpark {
            scope,
            target,
            json,
        }
        | Command::Enable {
            scope,
            target,
            json,
        } => run_unpark(&scope, &target, json, time),
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
    ports.telemetry = skill_studio_host::telemetry::port(
        skill_studio_host::telemetry::Surface::Cli,
        env!("CARGO_PKG_VERSION"),
    );
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
    ports.telemetry = skill_studio_host::telemetry::port(
        skill_studio_host::telemetry::Surface::Cli,
        env!("CARGO_PKG_VERSION"),
    );
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
    copy: bool,
    project: Option<PathBuf>,
    name: Option<String>,
    trust: bool,
}

/// The permission bits of a file read from disk, for `InstallFile::mode`;
/// `None` off Unix.
#[allow(clippy::unnecessary_wraps)] // `None` off Unix
fn unix_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
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
                    mode: unix_mode(&metadata),
                });
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
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
        return early_error(
            Operation::Install,
            &rt.scope,
            &ctx,
            err,
            json,
            time,
            output::print_install_outcome_table,
        );
    }
    let name = args.name.clone().unwrap_or_else(|| {
        std::path::Path::new(&args.source)
            .file_name()
            .map_or_else(|| args.source.clone(), |n| n.to_string_lossy().into_owned())
    });
    let harnesses = match args
        .harnesses
        .iter()
        .map(|h| AgentId::parse_harness(h))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(harnesses) => harnesses,
        Err(err) => {
            return early_error(
                Operation::Install,
                &rt.scope,
                &ctx,
                err,
                json,
                time,
                output::print_install_outcome_table,
            )
        }
    };
    let files = if matches!(method, InstallMethod::Copy) {
        match read_skill_files(std::path::Path::new(&args.source)) {
            Ok(files) => files,
            Err(e) => {
                let err = skill_studio_core::CoreError::io(&args.source, e);
                return early_error(
                    Operation::Install,
                    &rt.scope,
                    &ctx,
                    err,
                    json,
                    time,
                    output::print_install_outcome_table,
                );
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
        link_mode: if args.copy {
            InstallLinkMode::Copy
        } else {
            InstallLinkMode::Link
        },
        destination: skill_studio_core::identity::SkillDestination::Universal,
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
                    mode: unix_mode(&meta),
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
                return early_error(
                    operation,
                    &rt.scope,
                    &ctx,
                    err,
                    json,
                    time,
                    output::print_update_all_outcome_table,
                )
            }
        },
        (InstallMethod::Copy, None) => {
            let err = skill_studio_core::CoreError::new(
                skill_studio_core::ErrorCode::InvalidRequest,
                "update --method copy needs --source-dir",
            );
            return early_error(
                operation,
                &rt.scope,
                &ctx,
                err,
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

/// Which command a skill name is being matched for: each one accepts a
/// different kind of copy, so the name matches only copies the command can
/// act on.
#[derive(Clone, Copy)]
enum TargetKind {
    Park,
    Unpark,
    Remove,
}

impl TargetKind {
    /// Mirrors the first checks `ops::park`, `ops::unpark` and `ops::remove`
    /// make, so a name never picks a copy the op then refuses.
    fn accepts(self, deployment: &skill_studio_core::dto::DeploymentDto) -> bool {
        use skill_studio_core::identity::{BackingRelationship, RootKind};
        match self {
            TargetKind::Park => {
                deployment.root.kind == RootKind::Universal
                    && deployment.backing == BackingRelationship::Canonical
            }
            TargetKind::Remove => {
                deployment.root.kind == RootKind::Universal
                    && deployment.backing == BackingRelationship::Canonical
                    && deployment.owner_kind.is_mutable()
            }
            TargetKind::Unpark => deployment.root.kind == RootKind::Parked,
        }
    }

    fn past_tense(self) -> &'static str {
        match self {
            TargetKind::Park => "parked",
            TargetKind::Unpark => "unparked",
            TargetKind::Remove => "removed",
        }
    }
}

/// Turns `--id`, or a skill name, into the id of the one copy to act on.
/// A name that matches more than one copy is an `ambiguous_target` error
/// whose message lists each match's path and id.
fn resolve_target(
    rt: &Runtime,
    target: &TargetArgs,
    kind: TargetKind,
) -> Result<DeploymentId, skill_studio_core::CoreError> {
    if let Some(id) = &target.id {
        return DeploymentId::parse(id);
    }
    let name = target.skill.as_deref().unwrap_or_default();
    // Its own context: the op that follows records its timing on the
    // caller's context, and a scan there would be reported as part of it.
    let scan_ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let inventory = ops::scan(
        rt,
        &scan_ctx,
        &ScanRequest {
            skills: vec![SkillName(name.to_string())],
            timings: false,
        },
    )?;
    let Some(skill) = inventory.skills.iter().find(|skill| skill.name.0 == name) else {
        return Err(skill_studio_core::CoreError::new(
            skill_studio_core::ErrorCode::InvalidRequest,
            format!("No skill named {name} was found."),
        ));
    };
    let matches: Vec<_> = skill
        .deployments
        .iter()
        .filter(|deployment| kind.accepts(deployment))
        .collect();
    match matches.as_slice() {
        [] => Err(skill_studio_core::CoreError::new(
            skill_studio_core::ErrorCode::InvalidRequest,
            format!("{name} has no copy that can be {}.", kind.past_tense()),
        )),
        [only] => Ok(only.id.clone()),
        several => {
            let mut message =
                format!("{name} has more than one copy. Pass --id with one of these ids:");
            for deployment in several {
                message.push_str("\n  ");
                message.push_str(&deployment.path.display().to_string());
                message.push_str("  id ");
                message.push_str(deployment.id.as_str());
            }
            Err(skill_studio_core::CoreError::new(
                skill_studio_core::ErrorCode::AmbiguousTarget,
                message,
            ))
        }
    }
}

/// Takes a mutable deployment off disk, via `ops::remove`.
fn run_remove(scope: &ScopeArgs, target: &TargetArgs, json: bool, time: bool) -> ExitCode {
    let rt = match build_runtime_write::<skill_studio_core::dto::RemoveOutcome>(
        scope,
        Operation::Remove,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let result = resolve_target(&rt, target, TargetKind::Remove).and_then(|deployment_id| {
        ops::remove(
            &rt,
            &ctx,
            &skill_studio_core::dto::RemoveRequest { deployment_id },
        )
    });
    let envelope = ResultEnvelope::from_result(Operation::Remove, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_remove_outcome_table)
}

/// Moves a universal deployment to the parked root, via `ops::park`.
fn run_park(scope: &ScopeArgs, target: &TargetArgs, json: bool, time: bool) -> ExitCode {
    let rt = match build_runtime_write::<skill_studio_core::dto::ParkOutcome>(
        scope,
        Operation::Park,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let result = resolve_target(&rt, target, TargetKind::Park)
        .and_then(|deployment_id| ops::park(&rt, &ctx, &ParkRequest { deployment_id }));
    let envelope = ResultEnvelope::from_result(Operation::Park, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_park_outcome_table)
}

/// Splits a Universal deployment into per-harness copies, via `ops::split`.
fn run_split(
    scope: &ScopeArgs,
    deployment_id: &str,
    harnesses: &[String],
    json: bool,
    time: bool,
) -> ExitCode {
    let rt = match build_runtime_write::<skill_studio_core::dto::SplitOutcome>(
        scope,
        Operation::Split,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let parsed = DeploymentId::parse(deployment_id).and_then(|deployment_id| {
        let harnesses = harnesses
            .iter()
            .map(|raw| AgentId::parse_harness(raw))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(skill_studio_core::dto::SplitRequest {
            deployment_id,
            harnesses,
        })
    });
    let result = parsed.and_then(|req| ops::split(&rt, &ctx, &req));
    let envelope = ResultEnvelope::from_result(Operation::Split, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_split_outcome_table)
}

/// Moves a parked deployment back to the universal root, via `ops::unpark`.
fn run_unpark(scope: &ScopeArgs, target: &TargetArgs, json: bool, time: bool) -> ExitCode {
    let rt = match build_runtime_write::<skill_studio_core::dto::UnparkOutcome>(
        scope,
        Operation::Unpark,
        json,
    ) {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let result = resolve_target(&rt, target, TargetKind::Unpark)
        .and_then(|deployment_id| ops::unpark(&rt, &ctx, &UnparkRequest { deployment_id }));
    let envelope = ResultEnvelope::from_result(Operation::Unpark, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_unpark_outcome_table)
}

/// Counts each skill's uses over the last `days` days, via
/// [`skill_studio_host::usage_report`]. Reads the desktop app's use cache
/// only for the real home, and never writes it.
fn run_usage(scope: &ScopeArgs, days: u32, json: bool, time: bool) -> ExitCode {
    let rt =
        match build_runtime::<skill_studio_host::UsageReport>(scope, Operation::SkillUsage, json) {
            Ok(rt) => rt,
            Err(code) => return code,
        };
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let cache = scope.desktop_usage_cache();
    let result = ops::scan(&rt, &ctx, &ScanRequest::default()).map(|inventory| {
        skill_studio_host::usage_report(
            &rt.scope.home.canonical,
            &inventory,
            days,
            cache.as_deref(),
        )
    });
    let envelope = ResultEnvelope::from_result(Operation::SkillUsage, &rt.scope, &ctx, result);
    finish(&envelope, json, time, output::print_usage_table)
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
    ) -> Result<
        Option<skill_studio_core::skill_update_check::CommitInfo>,
        skill_studio_core::CoreError,
    > {
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
        std::collections::BTreeMap<String, skill_studio_core::skill_update_check::OutdatedRecord>,
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

/// A `run_*` command's early exit for a client-side validation failure
/// (a bad flag combination, an unreadable `--source-dir`) caught before the
/// op itself ever runs: wraps `err` in the same `ResultEnvelope` shape a
/// failed op would produce, so a caller sees one consistent error report
/// either way.
fn early_error<T: serde::Serialize + ops::Outcome>(
    operation: Operation,
    scope: &skill_studio_core::NormalizedScope,
    ctx: &OpContext,
    err: skill_studio_core::CoreError,
    json: bool,
    time: bool,
    print_table: impl FnOnce(&ResultEnvelope<T>),
) -> ExitCode {
    let envelope = ResultEnvelope::<T>::from_result(operation, scope, ctx, Err(err));
    finish(&envelope, json, time, print_table)
}
