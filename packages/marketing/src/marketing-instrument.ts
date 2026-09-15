export const marketingTelemetry = import.meta.env.VITE_SENTRY_DSN?.trim()
  ? await import("./marketing-telemetry").then((telemetry) => {
      telemetry.initializeMarketingTelemetry(import.meta.env);
      return telemetry;
    })
  : undefined;
