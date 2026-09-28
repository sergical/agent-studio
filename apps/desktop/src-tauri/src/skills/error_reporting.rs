// ============================================================================
// Skills Module - error_reporting
// Telemetry (unit 6.4): a crash-report switch, off in the registry by
// default (`error_reporting_enabled` in `~/.agents/skill-studio.json`,
// alongside the other settings in `skill_fork_registry`) - the welcome
// screen offers it on and `save_harnesses_choice` writes the user's
// explicit choice; Settings' "Telemetry" card keeps it in sync afterward.
// The actual Sentry client, its panic hook, and its consent gate live in
// `skill_studio_host::telemetry`; this module only owns the two Tauri
// commands that read and flip the persisted switch, and the live
// `Consent` handle they flip alongside it.
// ============================================================================

use std::path::Path;
use std::sync::Mutex;

use skill_studio_host::telemetry::{Consent, TelemetryGuard};
use tauri::Manager;

/// The Tauri-managed telemetry state: the live consent flag every command
/// below flips, and the Sentry client guard `run()`'s exit handler takes
/// out of the `Mutex` to flush and close on `RunEvent::Exit`.
pub struct ReportingState {
    /// The gate `skill_studio_host::telemetry::ConsentTransport` checks
    /// before forwarding an envelope.
    pub consent: Consent,
    /// `None` when `telemetry::init` found no DSN (every build until
    /// `SKILL_STUDIO_SENTRY_DSN` is set) - there is nothing to flush.
    pub guard: Mutex<Option<TelemetryGuard>>,
}

/// The saved switch, straight off disk - like `get_discovery_sources`, this
/// doesn't change anything, so it reads the registry directly.
#[tauri::command]
pub async fn get_error_reporting_enabled(app: tauri::AppHandle) -> Result<bool, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(
        &timing_app,
        "get_error_reporting_enabled",
        move || {
            let home = dirs::home_dir().ok_or("Could not find home directory")?;
            Ok(super::skill_fork_registry::read_fork_registry(&home)?.error_reporting_enabled)
        },
    )
    .await
}

/// Saves the switch and flips the live `Consent` so it takes effect
/// without a restart. Returns the saved value.
#[tauri::command]
pub async fn set_error_reporting_enabled(
    enabled: bool,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(
        &timing_app,
        "set_error_reporting_enabled",
        move || {
            let home = dirs::home_dir().ok_or("Could not find home directory")?;
            let consent = app.state::<ReportingState>().consent.clone();
            set_error_reporting_enabled_at(&home, enabled, &consent)?;
            Ok(enabled)
        },
    )
    .await
}

/// The persist-and-flip body of `set_error_reporting_enabled`, kept apart so
/// a test can drive it with a plain `home` path and `Consent` - a
/// `tauri::AppHandle` can't be constructed outside a running app (see
/// `harness_first_run::save_harnesses_choice_at`'s own split for the same
/// reason). Saves the switch and flips the live `Consent` so it takes
/// effect without a restart.
pub(crate) fn set_error_reporting_enabled_at(
    home: &Path,
    enabled: bool,
    consent: &Consent,
) -> Result<(), String> {
    let mut registry = super::skill_fork_registry::read_fork_registry(home)?;
    registry.error_reporting_enabled = enabled;
    super::skill_fork_registry::write_fork_registry(home, &registry)?;
    consent.set(enabled);
    Ok(())
}

/// Forwards one `WebView` error (a React `componentDidCatch`, an uncaught
/// `window` error, or an unhandled promise rejection) to
/// `skill_studio_host::telemetry::report_frontend_error`, unless consent is
/// off - so `FRONTEND_ERROR_REPORT_COUNT`'s per-process cap isn't spent by
/// reports that `ConsentTransport` would have dropped anyway. `ConsentTransport`
/// stays the authoritative consent check: this is only a courtesy that skips
/// the count, not a second copy of the gate.
#[allow(clippy::needless_pass_by_value)] // Tauri's command extractor requires owned arguments.
#[tauri::command]
pub fn report_frontend_error(
    component: String,
    kind: String,
    state: tauri::State<'_, ReportingState>,
) {
    forward_frontend_error(&state.consent, &component, &kind);
}

/// The body of `report_frontend_error`, kept apart so a test can drive it
/// with a plain `Consent` - a `tauri::State` can't be constructed outside a
/// running app. Returns whether the report was forwarded, so a test can
/// assert on it without a fake host client.
pub(crate) fn forward_frontend_error(consent: &Consent, component: &str, kind: &str) -> bool {
    if !consent.enabled() {
        return false;
    }
    skill_studio_host::telemetry::report_frontend_error(component, kind);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `turning_the_settings_switch_off_stops_reports_before_restart`: the
    /// same persist-and-flip function called with `false` must leave both
    /// the registry and the live `Consent` off - the property that makes a
    /// telemetry opt-out take effect immediately rather than at next
    /// launch.
    #[test]
    fn turning_the_settings_switch_off_stops_reports_before_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join(".agents")).unwrap();
        let mut registry = super::super::skill_fork_registry::read_fork_registry(&home).unwrap();
        registry.error_reporting_enabled = true;
        super::super::skill_fork_registry::write_fork_registry(&home, &registry).unwrap();
        let consent = Consent::new(true);

        set_error_reporting_enabled_at(&home, false, &consent).unwrap();

        assert!(
            !consent.enabled(),
            "turning the switch off must flip the live Consent before restart"
        );
        let after = super::super::skill_fork_registry::read_fork_registry(&home).unwrap();
        assert!(
            !after.error_reporting_enabled,
            "turning the switch off must persist false to the registry"
        );
    }

    /// guards: `forward_frontend_error` counting a report against
    /// `FRONTEND_ERROR_REPORT_COUNT` while consent is off - `ConsentTransport`
    /// stays the authoritative gate, but this command shouldn't call the host
    /// at all when it already knows the envelope would be dropped. No Sentry
    /// client is bound in tests, so the host call itself is a no-op either
    /// way; this only asserts the return value that signals whether it ran.
    #[test]
    fn a_webview_error_is_not_forwarded_while_the_switch_is_off() {
        let off = Consent::new(false);
        assert!(
            !forward_frontend_error(&off, "SkillList", "TypeError"),
            "a report must not be forwarded while consent is off"
        );

        let on = Consent::new(true);
        assert!(
            forward_frontend_error(&on, "SkillList", "TypeError"),
            "a report must be forwarded once consent is on"
        );
    }
}
