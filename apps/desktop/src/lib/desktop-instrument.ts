export const desktopTelemetry = import.meta.env.VITE_DESKTOP_SENTRY_DSN?.trim()
  ? await import("./desktop-telemetry")
      .then((telemetry) => {
        return telemetry.initializeDesktopTelemetry({
          VITE_DESKTOP_SENTRY_DSN: import.meta.env.VITE_DESKTOP_SENTRY_DSN,
          VITE_DESKTOP_SENTRY_RELEASE: import.meta.env.VITE_DESKTOP_SENTRY_RELEASE,
          VITE_DESKTOP_SENTRY_ENVIRONMENT: import.meta.env.VITE_DESKTOP_SENTRY_ENVIRONMENT,
          VITE_DESKTOP_SENTRY_TRACES_SAMPLE_RATE: import.meta.env
            .VITE_DESKTOP_SENTRY_TRACES_SAMPLE_RATE,
        })
          ? telemetry
          : undefined;
      })
      .catch(() => undefined)
  : undefined;
