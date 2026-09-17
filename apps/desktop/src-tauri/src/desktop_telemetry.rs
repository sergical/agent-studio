use skill_studio_telemetry::{
    desktop_error_sentry_layer, read_sentry_layer, session_from_environment_with_defaults,
    ScanMetricsLayer, SentrySession, SessionDefaults, TelemetrySurface,
};
use std::time::Duration;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Clone, Copy)]
pub(crate) struct TelemetryConfiguration(&'static str);

pub(crate) struct DesktopTelemetry {
    session: Option<SentrySession>,
    configuration: TelemetryConfiguration,
}
impl DesktopTelemetry {
    pub(crate) fn initialize() -> Self {
        let session = session_from_environment_with_defaults(
            TelemetrySurface::Desktop,
            (
                env!("CARGO_PKG_VERSION_MAJOR")
                    .parse()
                    .expect("numeric version"),
                env!("CARGO_PKG_VERSION_MINOR")
                    .parse()
                    .expect("numeric version"),
                env!("CARGO_PKG_VERSION_PATCH")
                    .parse()
                    .expect("numeric version"),
            ),
            SessionDefaults {
                dsn: option_env!("SKILL_STUDIO_DESKTOP_SENTRY_DSN"),
                environment: option_env!("SKILL_STUDIO_DESKTOP_SENTRY_ENVIRONMENT"),
                traces_sample_rate: option_env!("SKILL_STUDIO_DESKTOP_SENTRY_TRACES_SAMPLE_RATE"),
                build_revision: option_env!("SKILL_STUDIO_DESKTOP_BUILD_REVISION"),
            },
        );
        let (session, configuration) = match session {
            Ok(None) => (None, "disabled"),
            Err(status) => (None, status),
            Ok(Some(session)) => {
                if tracing_subscriber::registry()
                    .with(read_sentry_layer())
                    .with(desktop_error_sentry_layer())
                    .with(ScanMetricsLayer)
                    .try_init()
                    .is_ok()
                {
                    session.bind_main();
                    (Some(session), "enabled")
                } else {
                    session.close(Duration::ZERO);
                    (None, "initialization-failed")
                }
            }
        };
        Self {
            session,
            configuration: TelemetryConfiguration(configuration),
        }
    }
    pub(crate) fn configuration(&self) -> TelemetryConfiguration {
        self.configuration
    }
    pub(crate) fn shutdown(self) {
        if let Some(session) = self.session {
            session.close(Duration::from_secs(2));
        }
    }
}

#[tauri::command]
pub(crate) fn get_telemetry_configuration(
    configuration: tauri::State<'_, TelemetryConfiguration>,
) -> &'static str {
    configuration.0
}
