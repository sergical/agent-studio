#![forbid(unsafe_code)]
// stdio is the MCP transport itself: responses go to stdout, diagnostics to
// stderr.
#![allow(clippy::print_stdout, clippy::print_stderr)]
// unwrap/expect are fine in test code; production code must use ?
// or an explicit error.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! `skill-studio-mcp`: the stateless local MCP server over the Skill Studio
//! core, stdio transport.
//!
//! One tool per [`Operation`]. Every call builds a fresh `RuntimeScope` and
//! `Ports`, re-reads disk, runs one core operation, and drops everything:
//! no sessions, no subscriptions (decision D13). The one thing kept between
//! calls is `skill_usage`'s in-memory use index, which only saves re-reading
//! unchanged session history; it never changes an answer, so restarting the
//! process between two calls must still give identical results. Input
//! schemas are the core's request DTOs' `schemars` output directly, not
//! redeclared types. A core
//! error is never a panic and never a bare string: it comes back as a tool
//! error whose payload is the same `ResultEnvelope` the CLI prints.
//!
//! `main.rs` is a thin binary entry point over this crate: everything a
//! test needs to call a tool handler directly (as opposed to spawning the
//! binary and talking stdio) lives here instead.

pub mod scope;

use std::sync::{Arc, Mutex, PoisonError};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Implementation, ProgressNotificationParam, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::transport::stdio;
use rmcp::{schemars, tool, tool_handler, tool_router, RoleServer, ServerHandler, ServiceExt};
use serde::Deserialize;
use skill_studio_core::dto::{
    CapabilitiesRequest, DiagnoseConflictRequest, DoctorRequest, FixSkillRequest, HarnessesRequest,
    InstallPreferencesRequest, InstallRequest, ListEventsRequest, ParkRequest, RemoveRequest,
    RepairApplyRequest, RepairPreviewRequest, RestoreRequest, ScanRequest, SweepQuarantineRequest,
    UnparkRequest, UpdateAllRequest, UpdateRequest,
};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::CorrelationId;
use skill_studio_core::ops::{self, Operation, Outcome, ResultEnvelope};
use skill_studio_core::ports::{OpContext, Runtime};
use skill_studio_core::CoreError;
use skill_studio_host::SkillUsage;

/// Tools left out of `tools/list` unless `SKILL_STUDIO_MCP_DEV_TOOLS=1`:
/// each tool's description costs the client context on every turn, and an
/// agent tidying skills never needs these.
const DEV_TOOLS: &[&str] = &[
    "capabilities",
    "harnesses",
    "doctor",
    "sweep_quarantine",
    "install_preferences",
];

fn dev_tools_enabled() -> bool {
    std::env::var("SKILL_STUDIO_MCP_DEV_TOOLS").is_ok_and(|value| value == "1")
}

/// The server. Holds the tool table and the use index behind
/// `skill_usage`, which every clone shares and which loads on its first
/// call. No `Runtime` is kept: each call builds its own.
#[derive(Clone)]
pub struct SkillStudioServer {
    tool_router: ToolRouter<Self>,
    usage: Arc<Mutex<Option<SkillUsage>>>,
}

impl Default for SkillStudioServer {
    fn default() -> Self {
        Self {
            tool_router: Self::published_tool_router(),
            usage: Arc::default(),
        }
    }
}

/// Input for `skill_usage`.
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct SkillUsageRequest {
    /// How many days back to count uses. Default 30.
    pub days: Option<u32>,
}

/// Runs the MCP server over stdin and stdout until the client disconnects,
/// with crash reporting set up as the MCP surface. Blocks the calling thread
/// on its own tokio runtime, so a synchronous caller (the `skill-studio mcp`
/// subcommand) can run it without one. Writes nothing to stdout itself:
/// stdout carries only JSON-RPC.
pub fn run_stdio() -> anyhow::Result<()> {
    // Consent lives on the real machine, same as the CLI's own startup
    // resolution; there is no scope flag here to point it anywhere else.
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
    let registry_telemetry_enabled = skill_studio_host::telemetry::consent_from_registry(&home);
    let consent =
        skill_studio_host::telemetry::Consent::new(skill_studio_host::telemetry::resolve_consent(
            std::env::var("SKILL_STUDIO_TELEMETRY").ok(),
            registry_telemetry_enabled,
        ));
    let _telemetry_guard = skill_studio_host::telemetry::init(
        skill_studio_host::telemetry::Surface::Mcp,
        env!("CARGO_PKG_VERSION"),
        consent,
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let service = SkillStudioServer::default().serve(stdio()).await?;
        service.waiting().await?;
        anyhow::Ok(())
    })
}

/// Builds the `Runtime` for a read-only tool: no history store (a plain
/// `NoHistoryOpener`), discovery enabled outside fixture mode. Matches
/// `apps/cli/src/main.rs::build_runtime`.
fn build_runtime(with_history: bool) -> Result<Runtime, CoreError> {
    let (runtime_scope, lease_root) = scope::resolve();
    let catalog = Arc::new(HarnessCatalog::builtin());
    let mut ports = if with_history {
        let db_path = runtime_scope.history_root.join("events.sqlite3");
        skill_studio_host::default_ports_with_history(lease_root, catalog, db_path)
    } else {
        skill_studio_host::default_ports_with_discovery(lease_root, catalog)
    };
    ports.telemetry = skill_studio_host::telemetry::port(
        skill_studio_host::telemetry::Surface::Mcp,
        env!("CARGO_PKG_VERSION"),
    );
    if with_history {
        if runtime_scope.kind == skill_studio_core::scope::ScopeKind::Fixture {
            ports.discovery = None;
        } else {
            ports.discovery = Some(Arc::new(skill_studio_host::HostProjectDiscovery::new()));
        }
        ports.tools = Some(Arc::new(skill_studio_host::PathToolLookup::new()));
    }
    ports.spawner = Some(Arc::new(skill_studio_host::RealProcessSpawner::new()));
    if !with_history && runtime_scope.kind == skill_studio_core::scope::ScopeKind::Fixture {
        // Fixture scopes name their own projects explicitly; discovery would
        // otherwise walk the real machine's transcripts for a fake home.
        ports.discovery = None;
    }
    Runtime::new(&runtime_scope, ports)
}

/// The transport-free half of [`run_op`]: builds the `Runtime`, runs the
/// operation, and returns the envelope, with no `RequestContext` and no
/// progress notifications. Every tool goes through here, so a test that
/// calls it runs exactly the code a tool call runs, minus the live
/// transport peer it has no reason to stand up - which is how
/// `apps/desktop/src-tauri/tests/park_parity.rs` drives the MCP surface.
pub fn run_op_envelope<T: Outcome + serde::Serialize>(
    operation: Operation,
    with_history: bool,
    call: impl FnOnce(&Runtime, &OpContext) -> Result<T, CoreError>,
) -> ResultEnvelope<T> {
    let correlation_id = CorrelationId(ulid::Ulid::new().to_string());
    match build_runtime(with_history) {
        Ok(rt) => {
            let ctx = OpContext::uncancellable(correlation_id);
            let result = call(&rt, &ctx);
            ResultEnvelope::from_result(operation, &rt.scope, &ctx, result)
        }
        Err(err) => scope_error_envelope(operation, err, correlation_id),
    }
}

/// Runs one core operation end to end for one tool call: builds a fresh
/// `Runtime` from the environment, reports progress around the call when
/// the caller supplied a progress token, and wraps the result in the same
/// `ResultEnvelope` JSON the CLI prints. Never panics and never returns a
/// bare string on error; the envelope, `Ok` or `Error`, is always the tool
/// payload, with `is_error` set to match.
async fn run_op<T: Outcome + serde::Serialize>(
    operation: Operation,
    with_history: bool,
    context: &RequestContext<RoleServer>,
    call: impl FnOnce(&Runtime, &OpContext) -> Result<T, CoreError>,
) -> CallToolResult {
    let progress_token = context.meta.get_progress_token();
    if let Some(token) = &progress_token {
        let _ = context
            .peer
            .notify_progress(ProgressNotificationParam::new(token.clone(), 0.0))
            .await;
    }

    let envelope = run_op_envelope(operation, with_history, call);

    if let Some(token) = &progress_token {
        let _ = context
            .peer
            .notify_progress(ProgressNotificationParam::new(token.clone(), 1.0).with_total(1.0))
            .await;
    }

    let value = serde_json::to_value(&envelope).unwrap_or(serde_json::Value::Null);
    if envelope.status == skill_studio_core::OpStatus::Error {
        CallToolResult::structured_error(value)
    } else {
        CallToolResult::structured(value)
    }
}

/// Builds an envelope for a scope-construction failure (a fixture that does
/// not exist, a lease held elsewhere), before a `NormalizedScope` exists to
/// hand `ResultEnvelope::from_result`. Matches
/// `apps/cli/src/main.rs::error_envelope`.
fn scope_error_envelope<T: Outcome>(
    operation: Operation,
    err: CoreError,
    correlation_id: CorrelationId,
) -> ResultEnvelope<T> {
    use skill_studio_core::scope::EffectiveScope;
    let (runtime_scope, _) = scope::resolve();
    ResultEnvelope {
        schema_version: skill_studio_core::SCHEMA_VERSION,
        operation,
        scope: EffectiveScope {
            id: skill_studio_core::ScopeId::for_canonical_home(&runtime_scope.home_root),
            kind: runtime_scope.kind,
            home: runtime_scope.home_root.clone(),
            projects: Vec::new(),
            history_root: runtime_scope.history_root.clone(),
        },
        status: skill_studio_core::OpStatus::Error,
        data: None,
        errors: vec![skill_studio_core::ErrorEntry {
            code: err.code,
            message: err.message,
            path: err.path.map(|p| p.display().to_string()),
        }],
        correlation_id,
        event_id: None,
        // No `OpContext` exists yet at this point: the scope failed to
        // normalize before any op function ran.
        timings: None,
    }
}

/// A [`skill_studio_core::skill_update_check::SourceTreeLookup`],
/// [`skill_studio_core::skill_update_check::CommitLookup`], and
/// [`skill_studio_core::skill_update_check::PluginManifestLookup`] all in
/// one: when `gh` is not on `PATH`, every lookup a currency check makes
/// fails, which `ops::outdated` already turns into `Currency::Unknown` per
/// skill rather than a hard error. Matches `apps/cli/src/main.rs`'s
/// `NoGhLookup`.
struct NoGhLookup;

impl skill_studio_core::skill_update_check::SourceTreeLookup for NoGhLookup {
    fn tree_shas_at_head(
        &self,
        _repo: &str,
    ) -> Result<std::collections::HashMap<String, String>, CoreError> {
        Err(CoreError::new(
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
    ) -> Result<Option<skill_studio_core::skill_update_check::CommitInfo>, CoreError> {
        Err(CoreError::new(
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
    ) -> Result<Option<String>, CoreError> {
        Ok(None)
    }
}

#[tool_router]
impl SkillStudioServer {
    #[tool(
        description = "List every installed skill with its id, location and the agents that see it. Call this first. Other tools take its ids.",
        annotations(read_only_hint = true)
    )]
    async fn scan(
        &self,
        Parameters(req): Parameters<ScanRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::Scan, false, &context, |rt, ctx| {
            ops::scan(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Find broken, duplicate, invalid, parked and turned-off skills. Use it to choose what to fix or clear out.",
        annotations(read_only_hint = true)
    )]
    async fn diagnose(
        &self,
        Parameters(req): Parameters<ScanRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::Diagnose, false, &context, |rt, ctx| {
            ops::diagnose(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Show how often each skill was used in the last N days, and which skills were not used. Use it to find skills to park.",
        annotations(read_only_hint = true)
    )]
    async fn skill_usage(
        &self,
        Parameters(req): Parameters<SkillUsageRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let days = req.days.unwrap_or(skill_studio_host::DEFAULT_USAGE_DAYS);
        run_op(Operation::SkillUsage, false, &context, |rt, ctx| {
            let inventory = ops::scan(rt, ctx, &ScanRequest::default())?;
            let mut usage = self.usage.lock().unwrap_or_else(PoisonError::into_inner);
            let usage = usage.get_or_insert_with(|| {
                SkillUsage::load_read_only(scope::desktop_usage_cache().as_deref())
            });
            Ok(usage.report(
                &rt.scope.home.canonical,
                &inventory,
                days,
                chrono::Utc::now(),
            ))
        })
        .await
    }

    #[tool(
        description = "Show which operations each agent supports.",
        annotations(read_only_hint = true)
    )]
    async fn capabilities(
        &self,
        Parameters(req): Parameters<CapabilitiesRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::Capabilities, false, &context, |rt, ctx| {
            ops::capabilities(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Find the agents installed on this computer, with their version and install method.",
        annotations(read_only_hint = true)
    )]
    async fn harnesses(
        &self,
        Parameters(req): Parameters<HarnessesRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::Harnesses, false, &context, |rt, ctx| {
            ops::harnesses(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Show a fix for the header of one copy's SKILL.md. This writes nothing. Give the result to apply_frontmatter_repair.",
        annotations(read_only_hint = true)
    )]
    async fn preview_frontmatter_repair(
        &self,
        Parameters(req): Parameters<RepairPreviewRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(
            Operation::PreviewFrontmatterRepair,
            false,
            &context,
            |rt, ctx| ops::preview_frontmatter_repair(rt, ctx, &req),
        )
        .await
    }

    #[tool(
        description = "Write a fix that preview_frontmatter_repair showed. Undo with restore_event and the event_id from the result.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn apply_frontmatter_repair(
        &self,
        Parameters(req): Parameters<RepairApplyRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(
            Operation::ApplyFrontmatterRepair,
            true,
            &context,
            |rt, ctx| ops::apply_frontmatter_repair(rt, ctx, &req),
        )
        .await
    }

    #[tool(
        description = "List past changes, newest first, each with its event_id. Use it to find a change to undo.",
        annotations(read_only_hint = true)
    )]
    async fn list_events(
        &self,
        Parameters(req): Parameters<ListEventsRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::ListEvents, true, &context, |rt, ctx| {
            ops::list_events(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Undo one change. Give the event_id from the change's result or from list_events.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn restore_event(
        &self,
        Parameters(req): Parameters<RestoreRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::RestoreEvent, true, &context, |rt, ctx| {
            ops::restore_event(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Repair one skill. Fixes what it can and names each problem it cannot fix, with its path. Undo a repair with restore_event and the event_id in the result.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn fix(
        &self,
        Parameters(req): Parameters<FixSkillRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::FixSkill, true, &context, |rt, ctx| {
            ops::fix_skill(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Find skills that have copies with different content. This writes nothing.",
        annotations(read_only_hint = true)
    )]
    async fn diagnose_conflict(
        &self,
        Parameters(req): Parameters<DiagnoseConflictRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::DiagnoseConflict, false, &context, |rt, ctx| {
            ops::diagnose_conflict(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Check all skills for broken state. This writes nothing. An empty list means all is well.",
        annotations(read_only_hint = true)
    )]
    async fn doctor(
        &self,
        Parameters(req): Parameters<DoctorRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::Doctor, false, &context, |rt, ctx| {
            ops::doctor(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Update one installed skill from its source. Undo with restore_event and the event_id from the result.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn update(
        &self,
        Parameters(req): Parameters<UpdateRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::Update, true, &context, |rt, ctx| {
            ops::update(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Update several installed skills from their sources. Each update has its own event_id. Undo one with restore_event.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn update_all(
        &self,
        Parameters(req): Parameters<UpdateAllRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::UpdateAll, true, &context, |rt, ctx| {
            Ok(ops::update_all(rt, ctx, &req.requests, |_, _| {}))
        })
        .await
    }

    #[tool(
        description = "Delete one copy of a skill, such as a duplicate or a broken copy. Undo with restore_event and the event_id from the result.",
        annotations(read_only_hint = false, destructive_hint = true)
    )]
    async fn remove(
        &self,
        Parameters(req): Parameters<RemoveRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::Remove, true, &context, |rt, ctx| {
            ops::remove(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Install one skill by copy, dotagents or skills.sh, for a set of agents: universal, claude-code, codex, open-code, cursor, pi, grok-build. An empty set means universal. link_mode copy writes a real folder for each agent instead of links. An untrusted dotagents source returns NeedsTrust: call again with trust_confirmed. Undo with restore_event and the event_id from the result.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn add(
        &self,
        Parameters(req): Parameters<InstallRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::Install, true, &context, |rt, ctx| {
            ops::install(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Show the install method and agents that add uses when the caller does not name them.",
        annotations(read_only_hint = true)
    )]
    async fn install_preferences(
        &self,
        Parameters(req): Parameters<InstallPreferencesRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::InstallPreferences, false, &context, |rt, ctx| {
            ops::install_preferences(rt, ctx, &req.scope)
        })
        .await
    }

    #[tool(
        description = "Turn a skill off for every agent by moving it aside. Use it for unused skills. Undo with unpark.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn park(
        &self,
        Parameters(req): Parameters<ParkRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::Park, true, &context, |rt, ctx| {
            ops::park(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Replace one shared skill folder with one real copy for each agent you name. Agents you do not name lose the skill. After this, npx skills update does not update these copies. Undo with restore_event and the event_id from the result.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn split(
        &self,
        Parameters(req): Parameters<skill_studio_core::dto::SplitRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::Split, true, &context, |rt, ctx| {
            ops::split(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Turn a parked skill back on for every agent. This undoes park.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn unpark(
        &self,
        Parameters(req): Parameters<UnparkRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::Unpark, true, &context, |rt, ctx| {
            ops::unpark(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Check which installed skills have a newer version at their source. Needs gh on PATH. Without it, each skill reports unknown. This writes nothing.",
        annotations(read_only_hint = true)
    )]
    async fn outdated(
        &self,
        Parameters(req): Parameters<ScanRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::Outdated, true, &context, |rt, ctx| {
            match rt.ports.tools.as_ref().and_then(|t| t.find_binary("gh")) {
                Some(gh_bin) => ops::outdated(
                    rt,
                    ctx,
                    &req,
                    &skill_studio_host::GhSourceTreeLookup::new(gh_bin.clone()),
                    &skill_studio_host::GhCommitLookup::new(gh_bin),
                    &skill_studio_host::GhPluginManifestLookup,
                ),
                None => ops::outdated(rt, ctx, &req, &NoGhLookup, &NoGhLookup, &NoGhLookup),
            }
        })
        .await
    }

    #[tool(
        description = "Delete the oldest removed copies kept for undo, past the limit. This cannot be undone.",
        annotations(read_only_hint = false, destructive_hint = true)
    )]
    async fn sweep_quarantine(
        &self,
        Parameters(SweepQuarantineRequest {}): Parameters<SweepQuarantineRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::SweepQuarantine, true, &context, |rt, ctx| {
            ops::sweep_quarantine(rt, ctx, &skill_studio_core::identity::RootScope::Global)
        })
        .await
    }
}

impl SkillStudioServer {
    /// Every `#[tool]` above, minus [`DEV_TOOLS`] unless they are turned on,
    /// with each input schema made self-contained by [`inline_schema_refs`].
    fn published_tool_router() -> ToolRouter<Self> {
        let show_dev_tools = dev_tools_enabled();
        Self::tool_router()
            .into_iter()
            .filter(|route| show_dev_tools || !DEV_TOOLS.contains(&route.name()))
            .fold(ToolRouter::new(), |router, mut route| {
                route.attr.input_schema = Arc::new(inline_schema_refs(&route.attr.input_schema));
                router.with_route(route)
            })
    }
}

type JsonObject = serde_json::Map<String, serde_json::Value>;

/// Replaces every `$ref` into `$defs` with the definition itself, drops
/// `$defs`, and gives the root a `properties` object when it has none.
/// Some clients (Codex, `OpenAI` function calling) reject a tool whose input
/// schema has `$ref` or no `properties`, and schemars emits both for nested
/// and empty request types.
fn inline_schema_refs(schema: &JsonObject) -> JsonObject {
    let mut root = schema.clone();
    let defs = match root.remove("$defs") {
        Some(serde_json::Value::Object(defs)) => defs,
        _ => JsonObject::new(),
    };
    let mut inlined = inline_object(root, &defs, &mut Vec::new());
    inlined
        .entry("properties")
        .or_insert_with(|| serde_json::Value::Object(JsonObject::new()));
    inlined
}

/// `expanding` holds the definitions being inlined on the current path, so
/// a recursive type becomes an unconstrained `{}` instead of looping.
fn inline_object(
    mut object: JsonObject,
    defs: &JsonObject,
    expanding: &mut Vec<String>,
) -> JsonObject {
    let def_name = object.remove("$ref").and_then(|reference| {
        reference
            .as_str()?
            .strip_prefix("#/$defs/")
            .map(str::to_string)
    });
    if let Some(name) = def_name {
        if let (false, Some(serde_json::Value::Object(def))) =
            (expanding.contains(&name), defs.get(&name))
        {
            expanding.push(name);
            let def = inline_object(def.clone(), defs, expanding);
            expanding.pop();
            // Keys beside the `$ref` (a field's own description) win over
            // the definition's.
            for (key, def_value) in def {
                object.entry(key).or_insert(def_value);
            }
        }
    }
    object
        .into_iter()
        .map(|(key, child)| (key, inline_value(child, defs, expanding)))
        .collect()
}

fn inline_value(
    value: serde_json::Value,
    defs: &JsonObject,
    expanding: &mut Vec<String>,
) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => {
            serde_json::Value::Object(inline_object(object, defs, expanding))
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .into_iter()
                .map(|item| inline_value(item, defs, expanding))
                .collect(),
        ),
        other => other,
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SkillStudioServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "skill-studio",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Skill Studio manages the agent skills on this computer. To tidy up: \
                 call diagnose to find broken and duplicate skills, and skill_usage to \
                 find unused ones. Then park the skills you do not need, or remove a \
                 duplicate copy. Each change returns an event_id. restore_event undoes \
                 a change, and unpark undoes park.",
            )
    }
}
