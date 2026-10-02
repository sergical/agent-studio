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

use std::path::Path;

use skill_studio_core::dto::{ParkOutcome, ParkRequest, UnparkOutcome, UnparkRequest};
use skill_studio_core::identity::{CorrelationId, DeploymentId};
use skill_studio_core::ops::{self, Operation, ResultEnvelope};
use skill_studio_core::ports::OpContext;

use tauri::Manager;

use super::skill_dto::{BulkTargetResult, LifecycleTarget};
use super::skill_refresh::{self, SkillRefreshState};

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

/// The skill folder names `ids` were derived for, each once, in first-seen
/// order. An id that names no skill is skipped.
pub(super) fn skill_names_for_deployments<'a>(
    ids: impl IntoIterator<Item = &'a DeploymentId>,
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in ids.into_iter().filter_map(DeploymentId::skill_name) {
        if !names.contains(&name.0) {
            names.push(name.0);
        }
    }
    names
}

/// Publishes the rows of the skills a lifecycle write just changed, so the
/// UI does not wait for the watcher's debounce. When the targeted rebuild
/// fails or no name can be derived, skills go dirty and the full rebuild
/// covers it.
pub(super) fn emit_snapshot_for_names(app: &tauri::AppHandle, command: &str, names: Vec<String>) {
    let refresh_state = app.state::<SkillRefreshState>();
    if names.is_empty() {
        refresh_state.mark_skills_dirty();
        return;
    }
    if let Err(error) =
        skill_refresh::reconcile_skill_names_and_emit(app, &refresh_state, names, &[])
    {
        eprintln!("[{command}] targeted snapshot reconciliation failed: {error}");
        refresh_state.mark_skills_dirty();
    }
}

/// Runs `ops::park` against a `Runtime` rooted at `home`/`data_root` given
/// directly, rather than the host's own home directory. `park_skill` below
/// (the real Tauri command) calls this with the host's paths; a test calls
/// it with a tempdir so it can compare the desktop's write against the
/// CLI's and MCP's for the same op, without a `tauri::AppHandle`.
pub fn park_with_runtime(
    home: &Path,
    data_root: &Path,
    deployment_id: DeploymentId,
) -> Result<ParkOutcome, String> {
    let rt = super::core_runtime::build_runtime_write_at(home, data_root)?;
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let result = ops::park(&rt, &ctx, &ParkRequest { deployment_id });
    let envelope = ResultEnvelope::from_result(Operation::Park, &rt.scope, &ctx, result);
    super::core_runtime::to_command_result(envelope)
}

#[tauri::command]
pub async fn park_skill(
    target: LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<ParkOutcome, String> {
    let state_app = app.clone();
    crate::timing_log::time_command_blocking(&app, "park_skill", move || {
        let deployment_id = deployment_id_from_target(&target, "Park")?;
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let names = skill_names_for_deployments([&deployment_id]);
        let outcome = park_with_runtime(&home, &super::core_runtime::data_root(), deployment_id)?;
        emit_snapshot_for_names(&state_app, "park_skill", names);
        Ok(outcome)
    })
    .await
}

#[tauri::command]
pub async fn unpark_skill(
    target: LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<UnparkOutcome, String> {
    let state_app = app.clone();
    crate::timing_log::time_command_blocking(&app, "unpark_skill", move || {
        let deployment_id = deployment_id_from_target(&target, "Unpark")?;
        let names = skill_names_for_deployments([&deployment_id]);
        let rt = super::core_runtime::build_runtime_write()?;
        let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
        let result = ops::unpark(&rt, &ctx, &UnparkRequest { deployment_id });
        let envelope = ResultEnvelope::from_result(Operation::Unpark, &rt.scope, &ctx, result);
        let outcome = super::core_runtime::to_command_result(envelope)?;
        emit_snapshot_for_names(&state_app, "unpark_skill", names);
        Ok(outcome)
    })
    .await
}

/// Parks every target with one runtime and reports each outcome on its own
/// target, so one refused folder never stops the rest. One targeted snapshot
/// refresh at the end covers the skills that moved; the watcher's full
/// rebuild then confirms them.
#[tauri::command]
pub async fn park_skills(
    targets: Vec<LifecycleTarget>,
    app: tauri::AppHandle,
) -> Result<Vec<BulkTargetResult>, String> {
    let state_app = app.clone();
    crate::timing_log::time_command_blocking(&app, "park_skills", move || {
        let results = run_batch("Park", &targets, |rt, ctx, deployment_id| {
            let result = ops::park(rt, ctx, &ParkRequest { deployment_id });
            let envelope = ResultEnvelope::from_result(Operation::Park, &rt.scope, ctx, result);
            super::core_runtime::to_command_result(envelope).map(|_| ())
        })?;
        emit_snapshot_for_names(&state_app, "park_skills", written_names(&targets, &results));
        Ok(results)
    })
    .await
}

#[tauri::command]
pub async fn unpark_skills(
    targets: Vec<LifecycleTarget>,
    app: tauri::AppHandle,
) -> Result<Vec<BulkTargetResult>, String> {
    let state_app = app.clone();
    crate::timing_log::time_command_blocking(&app, "unpark_skills", move || {
        let results = run_batch("Unpark", &targets, |rt, ctx, deployment_id| {
            let result = ops::unpark(rt, ctx, &UnparkRequest { deployment_id });
            let envelope = ResultEnvelope::from_result(Operation::Unpark, &rt.scope, ctx, result);
            super::core_runtime::to_command_result(envelope).map(|_| ())
        })?;
        emit_snapshot_for_names(
            &state_app,
            "unpark_skills",
            written_names(&targets, &results),
        );
        Ok(results)
    })
    .await
}

/// The skill names of the targets whose write succeeded. A target without a
/// parseable deployment id failed and is skipped.
fn written_names(targets: &[LifecycleTarget], results: &[BulkTargetResult]) -> Vec<String> {
    let ids: Vec<DeploymentId> = targets
        .iter()
        .zip(results)
        .filter(|(_, result)| result.error.is_none())
        .filter_map(|(target, _)| DeploymentId::parse(target.deployment_id.as_deref()?).ok())
        .collect();
    skill_names_for_deployments(&ids)
}

fn run_batch(
    action: &'static str,
    targets: &[LifecycleTarget],
    op: impl Fn(&skill_studio_core::ports::Runtime, &OpContext, DeploymentId) -> Result<(), String>,
) -> Result<Vec<BulkTargetResult>, String> {
    let start = std::time::Instant::now();
    let rt = super::core_runtime::build_runtime_write()?;
    let results = BulkTargetResult::collect(targets, |target| {
        let deployment_id = deployment_id_from_target(target, action)?;
        let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
        op(&rt, &ctx, deployment_id)
    });
    eprintln!(
        "skill refresh: batch {} {} targets in {} ms",
        action.to_lowercase(),
        targets.len(),
        start.elapsed().as_millis()
    );
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: &str) -> LifecycleTarget {
        LifecycleTarget {
            deployment_id: Some(id.to_string()),
            owner_id: None,
        }
    }

    const ALPHA: &str = "dep:v1/global/universal/universal/alpha/-/%2Fh%2F.agents%2Fskills%2Falpha";
    const BETA: &str = "dep:v1/global/universal/universal/beta/-/%2Fh%2F.agents%2Fskills%2Fbeta";

    /// Given two deployments of one skill and one of another, expect each
    /// skill name once, so the snapshot refresh rebuilds a row once.
    #[test]
    fn skill_names_for_deployments_lists_each_skill_once() {
        let alpha = DeploymentId::parse(ALPHA).unwrap();
        let alpha_link =
            DeploymentId::parse("dep:v1/global/claude-code/per-harness/alpha/-/%2Fh%2Flink")
                .unwrap();
        let beta = DeploymentId::parse(BETA).unwrap();

        let names = skill_names_for_deployments([&alpha, &beta, &alpha_link]);

        assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
    }

    /// Given a batch where the second target failed, expect only the first
    /// skill's name to be republished; a refused park changed nothing.
    #[test]
    fn written_names_skips_targets_whose_write_failed() {
        let targets = [target(ALPHA), target(BETA)];
        let results = [
            BulkTargetResult { error: None },
            BulkTargetResult {
                error: Some("refused".to_string()),
            },
        ];

        assert_eq!(written_names(&targets, &results), vec!["alpha".to_string()]);
    }
}
