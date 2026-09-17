import * as Sentry from "@sentry/react";
import type { Event, StackFrame } from "@sentry/react";

interface DesktopTelemetryEnvironment {
  VITE_DESKTOP_SENTRY_DSN?: string;
  VITE_DESKTOP_SENTRY_RELEASE?: string;
  VITE_DESKTOP_SENTRY_ENVIRONMENT?: string;
  VITE_DESKTOP_SENTRY_TRACES_SAMPLE_RATE?: string;
}

type SpanJSON = NonNullable<Event["spans"]>[number];
type Envelope = Parameters<ReturnType<NonNullable<Sentry.BrowserOptions["transport"]>>["send"]>[0];
type LogOrMetricItem = Extract<Envelope[1][number], [{ type: "log" | "trace_metric" }, unknown]>;

function isLogOrMetricItem(item: Envelope[1][number]): item is LogOrMetricItem {
  return item[0].type === "log" || item[0].type === "trace_metric";
}
export type InventoryRead = "inventory.read" | "snapshot.read" | "history.read";

function safeFrame(frame: StackFrame): StackFrame {
  let filename = "<external>";
  try {
    const path = new URL(frame.filename ?? "").pathname;
    if (/^\/assets\/[A-Za-z0-9_-]+-[A-Za-z0-9_-]+\.js$/.test(path)) {
      filename = `app://${path}`;
    }
  } catch {
    // Local development paths have no packaged asset identity.
  }
  return { filename, lineno: frame.lineno, colno: frame.colno, in_app: filename !== "<external>" };
}

function safeOperation(op: SpanJSON["op"]): string {
  return op === "ui.ipc" || op === "app.bootstrap" ? op : "ui.react";
}

function safeName(name: Event["transaction"]): string {
  return name === "inventory.read" || name === "snapshot.read" || name === "history.read"
    ? name
    : "desktop.ui";
}

function safeSpan(span: SpanJSON): SpanJSON {
  return {
    trace_id: span.trace_id,
    span_id: span.span_id,
    parent_span_id: span.parent_span_id,
    start_timestamp: span.start_timestamp,
    timestamp: span.timestamp,
    op: safeOperation(span.op),
    description: safeName(span.description),
    status: span.status === "ok" ? "ok" : "unknown_error",
    data: {},
  };
}

function safeEvent(event: Event, release: string | undefined, environment: string): Event {
  const trace = event.contexts?.trace;
  return {
    event_id: event.event_id,
    type: event.type,
    timestamp: event.timestamp,
    start_timestamp: event.start_timestamp,
    platform: "javascript",
    level: event.level,
    release,
    environment,
    transaction: safeName(event.transaction),
    tags: { surface: "desktop-ui" },
    contexts: trace
      ? {
          trace: {
            trace_id: trace.trace_id,
            span_id: trace.span_id,
            parent_span_id: trace.parent_span_id,
            op: safeOperation(trace.op),
            status: trace.status === "ok" ? "ok" : "unknown_error",
          },
        }
      : undefined,
    exception: event.exception
      ? {
          values: event.exception.values?.slice(0, 4).map((exception) => ({
            type: "Error",
            value: "Desktop application error",
            mechanism: { type: "generic", handled: exception.mechanism?.handled ?? true },
            stacktrace: { frames: exception.stacktrace?.frames?.slice(-64).map(safeFrame) },
          })),
        }
      : undefined,
    spans: event.spans?.slice(0, 128).map(safeSpan),
  };
}

export function initializeDesktopTelemetry(
  env: DesktopTelemetryEnvironment,
  transport?: Sentry.BrowserOptions["transport"],
): ReturnType<typeof Sentry.init> {
  const dsn = env.VITE_DESKTOP_SENTRY_DSN?.trim();
  if (!dsn) return undefined;
  const rate = Number(env.VITE_DESKTOP_SENTRY_TRACES_SAMPLE_RATE ?? "0.1");
  const environment = env.VITE_DESKTOP_SENTRY_ENVIRONMENT ?? "development";
  const release = env.VITE_DESKTOP_SENTRY_RELEASE;
  if (
    !Number.isFinite(rate) ||
    rate < 0 ||
    rate > 1 ||
    !["development", "test", "staging", "production"].includes(environment) ||
    (release !== undefined && !/^skill-studio@\d+\.\d+\.\d+(?:\+[a-f0-9]{12,64})?$/.test(release))
  )
    return undefined;
  try {
    const url = new URL(dsn);
    const loopback = ["127.0.0.1", "[::1]"].includes(url.hostname);
    if (url.password || (url.protocol !== "https:" && !(url.protocol === "http:" && loopback))) {
      return undefined;
    }
  } catch {
    return undefined;
  }
  return Sentry.init({
    dsn,
    release,
    environment,
    tracesSampleRate: rate,
    defaultIntegrations: false,
    integrations: [
      {
        name: "DesktopTelemetryPrivacy",
        setup(client) {
          client.on("beforeEnvelope", (envelope) => {
            // Scope attributes are merged after the SDK log and metric filters.
            for (const [, payload] of envelope[1].filter(isLogOrMetricItem)) {
              for (const item of payload.items) {
                const operation = item.attributes?.operation?.value;
                item.attributes = {};
                if (
                  "name" in item &&
                  item.name === "desktop.ipc.duration" &&
                  (operation === "inventory.read" ||
                    operation === "snapshot.read" ||
                    operation === "history.read")
                ) {
                  item.attributes.operation = { type: "string", value: operation };
                }
              }
            }
          });
        },
      },
      Sentry.globalHandlersIntegration(),
    ],
    sendDefaultPii: false,
    sendClientReports: false,
    maxBreadcrumbs: 0,
    tracePropagationTargets: [],
    enableLogs: true,
    enableMetrics: true,
    transport,
    beforeSend: (event) => ({ ...safeEvent(event, release, environment), type: undefined }),
    beforeSendTransaction: (event) => ({
      ...safeEvent(event, release, environment),
      type: "transaction",
    }),
    beforeSendSpan: safeSpan,
    beforeSendLog: (log) =>
      log.message === "desktop.ipc.finished"
        ? { level: "info", message: "desktop.ipc.finished", attributes: {} }
        : null,
    beforeSendMetric: (metric) => {
      if (metric.name === "desktop.ipc.count") {
        return {
          name: "desktop.ipc.count",
          type: "counter",
          value: 1,
          unit: "none",
          attributes: {},
        };
      }
      const operation = metric.attributes?.operation;
      if (
        metric.name !== "desktop.ipc.duration" ||
        metric.type !== "distribution" ||
        metric.unit !== "millisecond" ||
        !Number.isFinite(metric.value) ||
        metric.value < 0 ||
        (operation !== "inventory.read" &&
          operation !== "snapshot.read" &&
          operation !== "history.read")
      )
        return null;
      return {
        name: "desktop.ipc.duration",
        type: "distribution",
        value: metric.value,
        unit: "millisecond",
        attributes: { operation },
      };
    },
  });
}

export async function traceInventoryRead<T>(
  operation: InventoryRead,
  read: (telemetryTrace?: string) => Promise<T>,
): Promise<T> {
  const started = performance.now();
  const span = Sentry.startNewTrace(() =>
    Sentry.startInactiveSpan({ name: operation, op: "ui.ipc", forceTransaction: true }),
  );
  try {
    const result = await read(span ? Sentry.spanToTraceHeader(span) : undefined);
    span?.setStatus({ code: 1 });
    return result;
  } catch (error) {
    span?.setStatus({ code: 2 });
    throw error;
  } finally {
    const durationMs = performance.now() - started;
    Sentry.withActiveSpan(span ?? null, () => {
      Sentry.logger.info("desktop.ipc.finished");
      Sentry.metrics.count("desktop.ipc.count", 1);
      Sentry.metrics.distribution("desktop.ipc.duration", durationMs, {
        unit: "millisecond",
        attributes: { operation },
      });
    });
    span?.end();
  }
}

const captureHandledReactError: ReturnType<typeof Sentry.reactErrorHandler> = (
  error,
  errorInfo,
) => {
  Sentry.captureReactException(error, errorInfo, {
    mechanism: { handled: true, type: "auto.function.react.error_handler" },
  });
};

export const desktopReactErrors = {
  onUncaughtError: Sentry.reactErrorHandler(),
  onCaughtError: captureHandledReactError,
  onRecoverableError: captureHandledReactError,
};
