import * as Sentry from "@sentry/react";
import type { Event, StackFrame } from "@sentry/react";

type MarketingEnvironment = Pick<ImportMetaEnv, "MODE"> & {
  VITE_SENTRY_DSN?: string;
  VITE_SENTRY_RELEASE?: string;
  VITE_SENTRY_ENVIRONMENT?: string;
  VITE_SENTRY_TRACES_SAMPLE_RATE?: string;
};
type SpanJSON = NonNullable<Event["spans"]>[number];

function safeFrame(frame: StackFrame): StackFrame {
  let filename = "<external>";
  try {
    const path = new URL(frame.filename ?? "").pathname;
    if (/^\/assets\/(?:index|react|marketing-telemetry)-[A-Za-z0-9_-]+\.js$/.test(path)) {
      filename = `app://${path}`;
    }
  } catch {
    // Development and external frames do not have a deployed asset identity.
  }
  return { filename, lineno: frame.lineno, colno: frame.colno, in_app: filename !== "<external>" };
}

function safeOperation(operation: SpanJSON["op"]): string {
  switch (operation) {
    case "pageload":
    case "navigation":
    case "resource.script":
    case "resource.css":
    case "resource.img":
    case "http.client":
    case "ui.interaction.click":
    case "ui.interaction.press":
    case "ui.interaction.drag":
    case "app.bootstrap":
      return operation;
    default:
      return "function";
  }
}

function safeMeasurements(measurements: Event["measurements"]): Event["measurements"] {
  const result: NonNullable<Event["measurements"]> = {};
  for (const name of ["cls", "lcp", "fcp", "fp", "ttfb", "inp"]) {
    const measurement = measurements?.[name];
    if (measurement && Number.isFinite(measurement.value) && measurement.value >= 0) {
      result[name] = { value: measurement.value, unit: name === "cls" ? "none" : "millisecond" };
    }
  }
  return result;
}

function safeSpan(span: SpanJSON): SpanJSON {
  return {
    trace_id: span.trace_id,
    span_id: span.span_id,
    parent_span_id: span.parent_span_id,
    start_timestamp: span.start_timestamp,
    timestamp: span.timestamp,
    op: safeOperation(span.op),
    description: "marketing.page",
    status: span.status,
    data: {},
    measurements: safeMeasurements(span.measurements),
  };
}

function safeEvent(event: Event): Event {
  const trace = event.contexts?.trace;
  return {
    event_id: event.event_id,
    type: event.type,
    timestamp: event.timestamp,
    start_timestamp: event.start_timestamp,
    platform: "javascript",
    level: event.level,
    release: event.release,
    environment: event.environment,
    transaction: "marketing.page",
    contexts: trace
      ? {
          trace: {
            trace_id: trace.trace_id,
            span_id: trace.span_id,
            parent_span_id: trace.parent_span_id,
            op: safeOperation(trace.op),
            status: trace.status,
          },
        }
      : undefined,
    exception: event.exception
      ? {
          values: event.exception.values?.map((exception) => ({
            type: "Error",
            value: "Marketing application error",
            mechanism: { type: "generic", handled: exception.mechanism?.handled ?? true },
            stacktrace: { frames: exception.stacktrace?.frames?.map(safeFrame) },
          })),
        }
      : undefined,
    spans: event.spans?.map(safeSpan),
    measurements: safeMeasurements(event.measurements),
  };
}

export function initializeMarketingTelemetry(
  env: MarketingEnvironment,
  transport?: Sentry.BrowserOptions["transport"],
): ReturnType<typeof Sentry.init> {
  const dsn = env.VITE_SENTRY_DSN?.trim();
  if (!dsn) return undefined;
  const sampleRate = env.VITE_SENTRY_TRACES_SAMPLE_RATE?.trim();
  const tracesSampleRate = Number(sampleRate || "0.1");
  if (!Number.isFinite(tracesSampleRate) || tracesSampleRate < 0 || tracesSampleRate > 1) {
    throw new Error("VITE_SENTRY_TRACES_SAMPLE_RATE must be between 0 and 1");
  }
  return Sentry.init({
    dsn,
    release: env.VITE_SENTRY_RELEASE,
    environment: env.VITE_SENTRY_ENVIRONMENT ?? env.MODE,
    tracesSampleRate,
    defaultIntegrations: false,
    integrations: [
      Sentry.globalHandlersIntegration(),
      Sentry.browserTracingIntegration({
        traceFetch: false,
        traceXHR: false,
        beforeStartSpan: (options) => ({ ...options, name: "marketing.page", attributes: {} }),
      }),
    ],
    sendDefaultPii: false,
    sendClientReports: false,
    maxBreadcrumbs: 0,
    tracePropagationTargets: [],
    enableLogs: true,
    enableMetrics: true,
    transport,
    beforeSend: (event) => ({ ...safeEvent(event), type: undefined }),
    beforeSendTransaction: (event) => ({ ...safeEvent(event), type: "transaction" }),
    beforeSendSpan: safeSpan,
    beforeSendLog: (log) =>
      log.message === "marketing.bootstrap"
        ? { level: "info", message: "marketing.bootstrap", attributes: {} }
        : null,
    beforeSendMetric: (metric) =>
      metric.name === "marketing.bootstrap.count"
        ? {
            name: "marketing.bootstrap.count",
            type: "counter",
            value: 1,
            unit: "none",
            attributes: {},
          }
        : null,
  });
}

export function recordMarketingBootstrap(): void {
  Sentry.startSpan({ name: "marketing.page", op: "app.bootstrap" }, () => {
    Sentry.logger.info("marketing.bootstrap");
    Sentry.metrics.count("marketing.bootstrap.count", 1);
  });
}

export const marketingReactErrors = {
  onUncaughtError: Sentry.reactErrorHandler(),
  onCaughtError: Sentry.reactErrorHandler(),
  onRecoverableError: Sentry.reactErrorHandler(),
};
