// ============================================================================
// Skills Module - skill_doctor
// Unit 5.3: a thin adapter over `skill_studio_core::ops::doctor`, the op that
// runs all six lifecycle invariants (`docs/action-map/lifecycle-states.md`)
// over the whole scope rather than one skill at a time the way
// `ops::fix_skill`'s per-skill checks do. Two callers share `run_doctor`: the
// on-demand `doctor` command a Settings card invokes (returns its
// `DoctorReport` directly, no event), and `skill_refresh::init`'s startup
// sweep, which runs once after the first scan on the refresh loop's own
// background thread (`std::thread::spawn` in `skill_refresh::init`, not the
// UI thread) and has no caller to return to, so it emits [`DOCTOR_EVENT`]
// instead.
// ============================================================================

use skill_studio_core::dto::{DoctorReport, DoctorRequest};
use skill_studio_core::identity::CorrelationId;
use skill_studio_core::ops::{self, Operation, ResultEnvelope};
use skill_studio_core::ports::{OpContext, Runtime};

/// Event emitted on the main window once the startup doctor pass finishes
/// (see `skill_refresh::run_startup_doctor_pass`), carrying the same
/// [`DoctorReport`] the on-demand `doctor` command returns, so a Settings
/// card open from before that pass finished still sees its result without
/// asking the backend to rerun it.
pub const DOCTOR_EVENT: &str = "skills://doctor";

/// Runs `ops::doctor` off the UI thread, for the Settings card's "Run
/// doctor" button.
#[tauri::command]
pub async fn doctor(app: tauri::AppHandle) -> Result<DoctorReport, String> {
    crate::timing_log::time_command_blocking(&app, "doctor", move || {
        let rt = super::core_runtime::build_runtime_write()?;
        run_doctor(&rt)
    })
    .await
}

/// The shared body: build a correlation id, run `ops::doctor`, and unwrap
/// its envelope the way every other desktop command does. Kept apart from
/// the `#[tauri::command]` wrapper so `skill_refresh::init`'s startup sweep
/// (already running on its own background thread, not Tauri's async
/// runtime) can call it directly without a second `spawn_blocking`.
pub(crate) fn run_doctor(rt: &Runtime) -> Result<DoctorReport, String> {
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let result = ops::doctor(rt, &ctx, &DoctorRequest {});
    let envelope = ResultEnvelope::from_result(Operation::Doctor, &rt.scope, &ctx, result);
    super::core_runtime::to_command_result(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::core_runtime::{
        build_runtime_write_at_with_search_dirs, process_path_search_dirs,
    };

    /// Proves `run_doctor` reaches `ops::doctor` through the real desktop
    /// adapter stack (`build_runtime_write_at`'s host ports, not the core
    /// crate's own fake `Ports`), the same way `skill_fix.rs`'s
    /// `fix_skill_on_invalid_yaml_...` test proves `fix_skill`'s wiring
    /// rather than re-testing `ops::fix_skill` itself. A registry `copies`
    /// entry with no folder on disk (invariant 2) is the cheapest fixture
    /// that needs nothing from the filesystem watcher or a harness install.
    #[test]
    fn run_doctor_finds_a_registry_entry_with_no_folder_through_the_desktop_adapter_or_names_the_missed_invariant(
    ) {
        let home = tempfile::tempdir().expect("temp home");
        let universal_root = home.path().join(".agents/skills");
        std::fs::create_dir_all(&universal_root).expect("universal root");
        let registry_path = home.path().join(".agents/skill-studio.json");
        let stale_path = universal_root.join("stale-copy");
        std::fs::write(
            &registry_path,
            format!(
                r#"{{"copies":{{"stale-copy":{{"name":"stale-copy","path":{:?},"scope":"global","destination":"universal"}}}}}}"#,
                stale_path.display()
            ),
        )
        .expect("write registry");

        // The process's own PATH, not a real login-shell probe: `run_doctor`
        // here never spawns `npx`, so it doesn't need to pay for (or risk
        // hanging on) a real `$SHELL -lic` spawn.
        let rt = build_runtime_write_at_with_search_dirs(
            home.path(),
            &home.path().join(".skill-studio"),
            process_path_search_dirs(),
        )
        .expect("desktop runtime");
        let report = run_doctor(&rt).expect("run_doctor");

        assert!(
            report.violations.iter().any(|v| v.invariant
                == skill_studio_core::doctor::DoctorInvariant::RegistryEntryHasFolder),
            "run_doctor missed the stale registry entry through the desktop adapter: {:?}",
            report.violations
        );
    }
}
