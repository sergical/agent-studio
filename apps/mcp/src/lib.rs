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
//! `Ports`, re-reads disk, runs one core operation, and drops everything —
//! per MCP revision 2026-07-28, decision D13: no sessions, no
//! subscriptions, no cache between calls. Restarting the process between
//! two calls must give identical results. Input schemas are the core's
//! request DTOs' `schemars` output directly, not redeclared types. A core
//! error is never a panic and never a bare string: it comes back as a tool
//! error whose payload is the same `ResultEnvelope` the CLI prints.
//!
//! `main.rs` is a thin binary entry point over this crate: everything a
//! test needs to call a tool handler directly (as opposed to spawning the
//! binary and talking stdio) lives here instead.

pub mod scope;

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ProgressNotificationParam, ServerCapabilities, ServerInfo};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_handler, tool_router, RoleServer, ServerHandler};
use skill_studio_core::dto::{
    CapabilitiesRequest, DiagnoseConflictRequest, FixSkillRequest, HarnessesRequest,
    InstallPreferencesRequest, InstallRequest, ListEventsRequest, ParkRequest, RemoveRequest,
    RepairApplyRequest, RepairPreviewRequest, RestoreRequest, ScanRequest,
    SetHarnessEnabledRequest, SweepQuarantineRequest, UnparkRequest, UpdateAllRequest,
    UpdateRequest,
};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::CorrelationId;
use skill_studio_core::ops::{self, Operation, Outcome, ResultEnvelope};
use skill_studio_core::ports::{OpContext, Runtime};
use skill_studio_core::CoreError;

/// The server. Holds no fields: `#[tool_handler]`'s default router
/// expression (`Self::tool_router()`) builds the tool dispatch table fresh
/// per call, matching the "one `Runtime` per call, nothing cached between
/// calls" rule the rest of this file follows.
#[derive(Clone, Default)]
pub struct SkillStudioServer;

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
    let correlation_id = CorrelationId(ulid::Ulid::new().to_string());
    let progress_token = context.meta.get_progress_token();
    if let Some(token) = &progress_token {
        let _ = context
            .peer
            .notify_progress(ProgressNotificationParam::new(token.clone(), 0.0))
            .await;
    }

    let envelope = match build_runtime(with_history) {
        Ok(rt) => {
            let ctx = OpContext::uncancellable(correlation_id);
            let result = call(&rt, &ctx);
            ResultEnvelope::from_result(operation, &rt.scope, &ctx, result)
        }
        Err(err) => scope_error_envelope(operation, err, correlation_id),
    };

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
    fn latest_commit(&self, _repo: &str, _path: &str) -> Result<Option<String>, CoreError> {
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

/// The same call the `park` tool method below makes - `build_runtime(true)`
/// then `ops::park` - minus the `RequestContext`/progress-notification
/// wrapping, which needs a live transport peer a test has no reason to
/// stand up. A parity test that wants to prove the MCP server writes the
/// same disk state as the CLI and the desktop calls this directly instead
/// of spawning the binary over stdio.
pub fn park_direct(
    req: &skill_studio_core::dto::ParkRequest,
) -> Result<skill_studio_core::dto::ParkOutcome, CoreError> {
    let rt = build_runtime(true)?;
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    ops::park(&rt, &ctx, req)
}

#[tool_router]
impl SkillStudioServer {
    #[tool(description = "Inventory every installed skill.")]
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

    #[tool(description = "Inventory every installed skill and derive issues.")]
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

    #[tool(description = "Report harness capability facts.")]
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
        description = "Detect first-class harnesses installed on this machine: PATH, version, install method, configured, and used evidence."
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

    #[tool(description = "Preview a frontmatter repair for one deployment, without writing.")]
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

    #[tool(description = "Apply a previously-previewed frontmatter repair.")]
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

    #[tool(description = "List history rows, newest first.")]
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

    #[tool(description = "Revert one event.")]
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
        description = "Run the doctor invariants for one skill and repair whatever it can; anything it cannot repair is named with its path."
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

    #[tool(description = "Find differing copies of a skill without merging them; writes nothing.")]
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

    #[tool(description = "Refresh one already-installed skill in place.")]
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
        description = "Refresh a batch of already-installed skills in place, each its own journal entry."
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
        description = "Take a mutable deployment off disk. Copy/Fork land intact in quarantine; Dotagents/SkillsSh are removed by their own CLI."
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
        description = "Install one skill by copy, dotagents, or skills.sh. Returns NeedsTrust, not an error, when an untrusted dotagents source needs trust_confirmed on a retry."
    )]
    async fn add(
        &self,
        Parameters(req): Parameters<InstallRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::Install, true, &context, |rt, ctx| {
            if let Some(unknown) = req
                .harnesses
                .iter()
                .find(|h| rt.ports.catalog.get(h).is_none())
            {
                let accepted = rt
                    .ports
                    .catalog
                    .facts
                    .iter()
                    .map(|f| f.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(CoreError::new(
                    skill_studio_core::ErrorCode::InvalidRequest,
                    format!(
                        "`{unknown}` is not a known harness; accepted values: {accepted}",
                        unknown = unknown.as_str()
                    ),
                ));
            }
            ops::install(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Report the install method and harnesses the next add pre-selects: the last install's saved preference for that scope, or the environment default when nothing has been saved yet."
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

    #[tool(description = "Move a universal deployment to the parked root.")]
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

    #[tool(description = "Move a parked deployment back to the universal root.")]
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
        description = "Enable or disable a skill for one harness, by whatever mechanism that harness supports natively."
    )]
    async fn set_harness_enabled(
        &self,
        Parameters(req): Parameters<SetHarnessEnabledRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_op(Operation::SetHarnessEnabled, true, &context, |rt, ctx| {
            ops::set_harness_enabled(rt, ctx, &req)
        })
        .await
    }

    #[tool(
        description = "Report per-skill currency against each install method's source: skills.sh by lock hash, dotagents by pinned commit, plugin by cache version. Needs `gh` on PATH; without it every skill reports Unknown."
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
        description = "Prune the global quarantine cap without a remove call. Global scope only."
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

#[tool_handler]
impl ServerHandler for SkillStudioServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Stateless Skill Studio core, one tool per operation. Set \
             SKILL_STUDIO_FIXTURE, SKILL_STUDIO_HOME, or neither (real \
             machine) before starting; scope is fixed for the process \
             lifetime the same way the CLI's flags are fixed per call.",
        )
    }
}
