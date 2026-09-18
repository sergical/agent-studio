// ============================================================================
// Skills Module - skill_fix
// "Fix" runs the doctor invariants in `docs/action-map/lifecycle-states.md`
// for one skill by name and reports anything it repaired plus anything it
// couldn't; a conflict (two differing copies of the same skill) is reported,
// never merged. `fix_skill` is a thin adapter over
// `skill_studio_core::ops::fix_skill`, the same function the CLI's `fix`
// subcommand and the MCP server's `fix` tool call, so all three surfaces
// leave the same disk state.
// ============================================================================

use skill_studio_core::dto::{FixSkillOutcome, FixSkillRequest};
use skill_studio_core::identity::{CorrelationId, SkillName};
use skill_studio_core::ops::{self, Operation, ResultEnvelope};
use skill_studio_core::ports::OpContext;

#[tauri::command]
pub async fn fix_skill(skill: String, app: tauri::AppHandle) -> Result<FixSkillOutcome, String> {
    crate::timing_log::time_command_blocking(&app, "fix_skill", move || {
        let rt = super::core_runtime::build_runtime_write()?;
        let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
        let result = ops::fix_skill(
            &rt,
            &ctx,
            &FixSkillRequest {
                skill: SkillName(skill),
            },
        );
        let envelope = ResultEnvelope::from_result(Operation::FixSkill, &rt.scope, &ctx, result);
        super::core_runtime::to_command_result(envelope)
    })
    .await
}

/// Opens a conflict's two differing paths side by side in the user's chosen
/// editor. Writes nothing to either path itself; the caller already has both
/// paths from a `ConflictSummary` in a `FixSkillOutcome`.
#[tauri::command]
pub async fn open_conflict_paths(paths: Vec<String>, app: tauri::AppHandle) -> Result<(), String> {
    crate::timing_log::time_command_blocking(&app, "open_conflict_paths", move || {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let paths: Vec<std::path::PathBuf> =
            paths.into_iter().map(std::path::PathBuf::from).collect();
        super::skill_editor::open_paths_in_editor(&home, &paths)
    })
    .await
}
