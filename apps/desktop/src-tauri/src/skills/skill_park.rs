// ============================================================================
// Skills Module - skill_park
// "Park" moves a Global Universal skill's shared folder
// (`~/.agents/skills/<name>`) to `~/.agents/skills-parked/<name>` and
// removes a per-skill Claude Code symlink pointing at it, if any; `unpark`
// reverses both. This is the desktop's first command wired onto
// `skill-studio-core`'s `ops` functions (see `core_runtime.rs`) rather than
// its own `std::fs` calls: `park_skill`/`unpark_skill` are thin adapters
// over `skill_studio_core::ops::park`/`ops::unpark`, the same functions the
// CLI's `park`/`unpark` subcommands and the MCP server's `park`/`unpark`
// tools call, so all three surfaces leave the same disk state.
//
// Remaining gap in this build: `ops::park`/`ops::unpark` never write the
// legacy fork registry's `parked` bucket (`skill_fork_registry::
// ParkedRecord`). `skill_refresh.rs`'s "parked" badge now follows the
// on-disk `scope == "parked"` deployment core scan reports, so a skill
// parked through this command shows the badge correctly; only its
// `parked_at` timestamp is missing (`None`), since without a registry
// record the timestamp would require opening the SQLite history store on
// every refresh cycle - out of scope for this unit's badge fix (see
// `skill_refresh.rs`'s `apply_skill_snapshot_overlays`).
// ============================================================================

use skill_studio_core::dto::{ParkOutcome, ParkRequest, UnparkOutcome, UnparkRequest};
use skill_studio_core::identity::{CorrelationId, DeploymentId};
use skill_studio_core::ops::{self, Operation, ResultEnvelope};
use skill_studio_core::ports::OpContext;

use super::skill_dto::LifecycleTarget;

fn deployment_id_from_target(
    target: &LifecycleTarget,
    action: &str,
) -> Result<DeploymentId, String> {
    let raw = target
        .deployment_id
        .as_deref()
        .ok_or_else(|| format!("{action} requires a deployment id"))?;
    DeploymentId::parse(raw).map_err(|e| e.message)
}

#[tauri::command]
pub async fn park_skill(
    target: LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<ParkOutcome, String> {
    crate::timing_log::time_command_blocking(&app, "park_skill", move || {
        let deployment_id = deployment_id_from_target(&target, "Park")?;
        let rt = super::core_runtime::build_runtime_write()?;
        let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
        let result = ops::park(&rt, &ctx, &ParkRequest { deployment_id });
        let envelope = ResultEnvelope::from_result(Operation::Park, &rt.scope, &ctx, result);
        super::core_runtime::to_command_result(envelope)
    })
    .await
}

#[tauri::command]
pub async fn unpark_skill(
    target: LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<UnparkOutcome, String> {
    crate::timing_log::time_command_blocking(&app, "unpark_skill", move || {
        let deployment_id = deployment_id_from_target(&target, "Unpark")?;
        let rt = super::core_runtime::build_runtime_write()?;
        let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
        let result = ops::unpark(&rt, &ctx, &UnparkRequest { deployment_id });
        let envelope = ResultEnvelope::from_result(Operation::Unpark, &rt.scope, &ctx, result);
        super::core_runtime::to_command_result(envelope)
    })
    .await
}
