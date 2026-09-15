use crate::TelemetryEnvironment;
use crate::{
    FlushOutcome, SentryTransport, TelemetryIdentity, TransportSetupError, TransportStats,
};
use sentry_core::{Client, ClientOptions, Hub};
use sentry_types::Dsn;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub enum SessionSetupError {
    InvalidSamplingRate,
    Transport(TransportSetupError),
}

pub struct SentrySession {
    client: Arc<Client>,
    transport: Arc<SentryTransport>,
}
impl SentrySession {
    pub fn http(
        dsn: &Dsn,
        identity: TelemetryIdentity,
        rate: f32,
    ) -> Result<Self, SessionSetupError> {
        if !rate.is_finite() || !(0.0..=1.0).contains(&rate) {
            return Err(SessionSetupError::InvalidSamplingRate);
        }
        let transport =
            Arc::new(SentryTransport::http(dsn, identity).map_err(SessionSetupError::Transport)?);
        let client = Arc::new(Client::from(
            ClientOptions::new()
                .dsn(&dsn.to_string())
                .default_integrations(false)
                .traces_sample_rate(rate)
                .transport(transport.clone()),
        ));
        Ok(Self { client, transport })
    }
    pub fn bind_main(&self) {
        Hub::main().bind_client(Some(self.client.clone()));
    }
    pub fn stats(&self) -> TransportStats {
        self.transport.stats()
    }
    pub fn close(self, budget: Duration) -> FlushOutcome {
        let deadline = Instant::now() + budget.min(Duration::from_secs(2));
        if Hub::main()
            .client()
            .as_ref()
            .is_some_and(|client| Arc::ptr_eq(client, &self.client))
        {
            Hub::main().bind_client(None);
        }
        let (done, finished) = mpsc::sync_channel(1);
        if std::thread::Builder::new()
            .name("skill-telemetry-close".into())
            .spawn(move || {
                let _ = done.try_send(
                    self.client
                        .close(Some(deadline.saturating_duration_since(Instant::now()))),
                );
            })
            .is_err()
        {
            return FlushOutcome::Failed;
        }
        match finished.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(true) => FlushOutcome::Drained,
            Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => FlushOutcome::Failed,
            Err(mpsc::RecvTimeoutError::Timeout) => FlushOutcome::TimedOut,
        }
    }
}

#[derive(Default)]
pub struct SessionDefaults<'a> {
    pub dsn: Option<&'a str>,
    pub environment: Option<&'a str>,
    pub traces_sample_rate: Option<&'a str>,
    pub build_revision: Option<&'a str>,
}

struct SessionConfiguration {
    dsn: Dsn,
    environment: TelemetryEnvironment,
    rate: f32,
}

fn resolve_configuration(
    defaults: SessionDefaults<'_>,
    mut read: impl FnMut(&str) -> Option<std::ffi::OsString>,
) -> Result<Option<SessionConfiguration>, &'static str> {
    let mut setting = |name, fallback: Option<&str>| {
        read(name).or_else(|| fallback.map(std::ffi::OsString::from))
    };
    let Some(dsn) = setting("SKILL_STUDIO_SENTRY_DSN", defaults.dsn) else {
        return Ok(None);
    };
    let dsn = dsn.to_str().ok_or("invalid-configuration")?.trim();
    if dsn.is_empty() {
        return Ok(None);
    }
    let dsn: Dsn = dsn.parse().map_err(|_| "invalid-configuration")?;
    let environment_value = setting("SKILL_STUDIO_SENTRY_ENVIRONMENT", defaults.environment);
    let environment = match environment_value.as_ref().map(|value| value.to_str()) {
        Some(Some("development")) | None => TelemetryEnvironment::Development,
        Some(Some("test")) => TelemetryEnvironment::Test,
        Some(Some("staging")) => TelemetryEnvironment::Staging,
        Some(Some("production")) => TelemetryEnvironment::Production,
        _ => return Err("invalid-configuration"),
    };
    let rate = match setting(
        "SKILL_STUDIO_SENTRY_TRACES_SAMPLE_RATE",
        defaults.traces_sample_rate,
    ) {
        Some(value) => value
            .to_str()
            .ok_or("invalid-configuration")?
            .parse::<f32>()
            .map_err(|_| "invalid-configuration")?,
        None => 0.1,
    };
    if !rate.is_finite() || !(0.0..=1.0).contains(&rate) {
        return Err("invalid-configuration");
    }
    Ok(Some(SessionConfiguration {
        dsn,
        environment,
        rate,
    }))
}

pub fn session_from_environment(
    surface: crate::TelemetrySurface,
    version: (u32, u32, u32),
) -> Result<Option<SentrySession>, &'static str> {
    session_from_environment_with_defaults(surface, version, SessionDefaults::default())
}

pub fn session_from_environment_with_defaults(
    surface: crate::TelemetrySurface,
    version: (u32, u32, u32),
    defaults: SessionDefaults<'_>,
) -> Result<Option<SentrySession>, &'static str> {
    let build_revision = defaults.build_revision;
    let Some(config) = resolve_configuration(defaults, |name| std::env::var_os(name))? else {
        return Ok(None);
    };
    let identity = TelemetryIdentity::new(surface, config.environment, version);
    let identity = match build_revision {
        Some(revision) => identity.with_build_revision(revision)?,
        None => identity,
    };
    SentrySession::http(&config.dsn, identity, config.rate)
        .map(Some)
        .map_err(|_| "initialization-failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{ExportFailure, TelemetrySurface};
    use sentry_core::{Scope, Transport};
    use std::sync::Mutex;

    static MAIN_HUB: Mutex<()> = Mutex::new(());
    struct RestoreMain(Option<Arc<Client>>);
    impl Drop for RestoreMain {
        fn drop(&mut self) {
            Hub::main().bind_client(self.0.take());
        }
    }

    fn session_with_sink(
        sink: impl FnMut(&[u8]) -> Result<(), ExportFailure> + Send + 'static,
    ) -> SentrySession {
        let identity = TelemetryIdentity::new(
            TelemetrySurface::Desktop,
            TelemetryEnvironment::Test,
            (0, 1, 0),
        );
        let transport = Arc::new(SentryTransport::with_sink(identity, sink).unwrap());
        let client = Arc::new(Client::from(
            ClientOptions::new()
                .dsn("https://public@example.invalid/1")
                .default_integrations(false)
                .transport(transport.clone()),
        ));
        SentrySession { client, transport }
    }

    fn capture_error(session: &SentrySession) {
        let hub = Arc::new(Hub::new(
            Some(session.client.clone()),
            Arc::new(Scope::default()),
        ));
        Hub::run(hub, || {
            sentry_core::capture_message("fixture error", sentry_core::Level::Error);
        });
    }

    #[test]
    fn session_close_maps_delivery_results_and_unbinds_its_main_client() {
        let _serial = MAIN_HUB.lock().unwrap();
        let _restore = RestoreMain(Hub::main().client());
        for fail in [false, true] {
            let session =
                session_with_sink(move |_| if fail { Err(ExportFailure) } else { Ok(()) });
            session.bind_main();
            capture_error(&session);
            let transport = session.transport.clone();
            assert_eq!(
                session.close(Duration::from_secs(1)),
                if fail {
                    FlushOutcome::Failed
                } else {
                    FlushOutcome::Drained
                }
            );
            assert!(Hub::main().client().is_none());
            assert_eq!(transport.stats().export.accepted, 1);
            assert_eq!(transport.stats().export.failed, u64::from(fail));
            assert_eq!(transport.stats().export.delivered, u64::from(!fail));
        }
    }

    #[test]
    fn session_close_bounds_wait_and_allows_later_delivery() {
        let _serial = MAIN_HUB.lock().unwrap();
        let _restore = RestoreMain(Hub::main().client());
        for budget in [Duration::ZERO, Duration::from_millis(20)] {
            let (started, waiting) = mpsc::sync_channel(1);
            let (release, blocked) = mpsc::sync_channel(1);
            let session = session_with_sink(move |_| {
                started.send(()).map_err(|_| ExportFailure)?;
                blocked.recv().map_err(|_| ExportFailure)
            });
            session.bind_main();
            capture_error(&session);
            waiting.recv_timeout(Duration::from_secs(1)).unwrap();
            let transport = session.transport.clone();
            let start = Instant::now();
            let outcome = session.close(budget);
            // SDK deadline failure can arrive before the wrapper's receive timeout.
            assert!(matches!(
                outcome,
                FlushOutcome::TimedOut | FlushOutcome::Failed
            ));
            assert!(start.elapsed() < Duration::from_secs(1));
            assert!(Hub::main().client().is_none());
            release.send(()).unwrap();
            assert!(transport.shutdown(Duration::from_secs(1)));
            assert_eq!(transport.stats().export.delivered, 1);
        }
    }

    #[test]
    fn session_close_preserves_a_replacement_main_client() {
        let _serial = MAIN_HUB.lock().unwrap();
        let _restore = RestoreMain(Hub::main().client());
        let first = session_with_sink(|_| Ok(()));
        first.bind_main();
        let replacement = session_with_sink(|_| Ok(()));
        replacement.bind_main();
        assert_eq!(first.close(Duration::from_secs(1)), FlushOutcome::Drained);
        assert!(Arc::ptr_eq(
            &Hub::main().client().unwrap(),
            &replacement.client
        ));
        assert_eq!(
            replacement.close(Duration::from_secs(1)),
            FlushOutcome::Drained
        );
        assert!(Hub::main().client().is_none());
    }

    fn packaged_defaults() -> SessionDefaults<'static> {
        SessionDefaults {
            dsn: Some("https://packaged@example.invalid/1"),
            environment: Some("production"),
            traces_sample_rate: Some("0.2"),
            build_revision: None,
        }
    }

    #[test]
    fn packaged_configuration_does_not_require_a_shell_environment() {
        let config = resolve_configuration(packaged_defaults(), |_| None)
            .unwrap()
            .unwrap();
        assert_eq!(
            config.dsn.to_string(),
            "https://packaged:@example.invalid/1"
        );
        assert!(matches!(
            config.environment,
            TelemetryEnvironment::Production
        ));
        assert_eq!(config.rate, 0.2);
        assert!(resolve_configuration(SessionDefaults::default(), |_| None)
            .unwrap()
            .is_none());
    }

    #[test]
    fn runtime_values_override_packaged_defaults_and_empty_dsn_disables() {
        let config = resolve_configuration(packaged_defaults(), |name| {
            Some(
                match name {
                    "SKILL_STUDIO_SENTRY_DSN" => "https://runtime@example.invalid/2",
                    "SKILL_STUDIO_SENTRY_ENVIRONMENT" => "staging",
                    "SKILL_STUDIO_SENTRY_TRACES_SAMPLE_RATE" => "0",
                    _ => panic!("unexpected setting"),
                }
                .into(),
            )
        })
        .unwrap()
        .unwrap();
        assert_eq!(config.dsn.to_string(), "https://runtime:@example.invalid/2");
        assert!(matches!(config.environment, TelemetryEnvironment::Staging));
        assert_eq!(config.rate, 0.0);
        assert!(resolve_configuration(packaged_defaults(), |name| {
            assert_eq!(name, "SKILL_STUDIO_SENTRY_DSN");
            Some("  ".into())
        })
        .unwrap()
        .is_none());
    }

    #[test]
    fn invalid_runtime_configuration_never_falls_back_to_packaged_values() {
        for (key, value) in [
            ("SKILL_STUDIO_SENTRY_DSN", "not a dsn"),
            ("SKILL_STUDIO_SENTRY_ENVIRONMENT", "private-environment"),
            ("SKILL_STUDIO_SENTRY_TRACES_SAMPLE_RATE", "NaN"),
            ("SKILL_STUDIO_SENTRY_TRACES_SAMPLE_RATE", "1.1"),
            ("SKILL_STUDIO_SENTRY_TRACES_SAMPLE_RATE", "-1"),
        ] {
            assert!(matches!(
                resolve_configuration(packaged_defaults(), |name| {
                    (name == key).then(|| value.into())
                }),
                Err("invalid-configuration")
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_runtime_values_are_rejected() {
        use std::os::unix::ffi::OsStringExt;
        for key in [
            "SKILL_STUDIO_SENTRY_DSN",
            "SKILL_STUDIO_SENTRY_ENVIRONMENT",
            "SKILL_STUDIO_SENTRY_TRACES_SAMPLE_RATE",
        ] {
            assert!(matches!(
                resolve_configuration(packaged_defaults(), |name| {
                    (name == key).then(|| std::ffi::OsString::from_vec(vec![0xff]))
                }),
                Err("invalid-configuration")
            ));
        }
    }
}
