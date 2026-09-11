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

mod scope;

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ProgressNotificationParam, ServerCapabilities, ServerInfo};
use rmcp::service::RequestContext;
use rmcp::transport::stdio;
use rmcp::{tool, tool_handler, tool_router, RoleServer, ServerHandler, ServiceExt};
use skill_studio_core::dto::{
    CapabilitiesRequest, ListEventsRequest, RepairApplyRequest, RepairPreviewRequest,
    RestoreRequest, ScanRequest,
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
struct SkillStudioServer;

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
            ports.discovery = Some(Arc::new(
                skill_studio_host::TranscriptProjectDiscovery::new(),
            ));
        }
        ports.tools = Some(Arc::new(skill_studio_host::PathToolLookup::new()));
    } else if runtime_scope.kind == skill_studio_core::scope::ScopeKind::Fixture {
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
            let ctx = OpContext::uncancellable(correlation_id.clone());
            let result = call(&rt, &ctx);
            ResultEnvelope::from_result(operation, &rt.scope, correlation_id, result)
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
    }
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server = SkillStudioServer;
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
