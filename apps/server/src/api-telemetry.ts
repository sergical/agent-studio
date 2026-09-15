// ============================================================================
// Skill Studio - API Sentry policy
// ============================================================================

import { fileURLToPath } from "node:url";
import { relative } from "node:path";
import { z } from "zod";
import * as Sentry from "@sentry/hono/node";
import { METHOD_LABEL, ROUTE_LABEL } from "./request-telemetry-labels";
import type { Event, StackFrame } from "@sentry/hono/node";

type SpanJSON = NonNullable<Event["spans"]>[number];
type Envelope = Parameters<ReturnType<NonNullable<Sentry.NodeOptions["transport"]>>["send"]>[0];
type LogOrMetricItem = Extract<Envelope[1][number], [{ type: "log" | "trace_metric" }, unknown]>;

function isLogOrMetricItem(item: Envelope[1][number]): item is LogOrMetricItem {
  return item[0].type === "log" || item[0].type === "trace_metric";
}

const SOURCE_ROOT = fileURLToPath(new URL("./", import.meta.url));

function safeFrame(frame: StackFrame): StackFrame {
  let source = frame.filename;
  if (source?.startsWith("file://")) {
    try {
      source = fileURLToPath(source);
    } catch {
      source = undefined;
    }
  }
  const path = source ? relative(SOURCE_ROOT, source) : "";
  const isSource = [
    "server.ts",
    "api-runtime.ts",
    "api-telemetry.ts",
    "instrument.ts",
    "request-telemetry.ts",
    "request-telemetry-labels.ts",
  ].includes(path);
  const isBundle = path === "server.js" || path === "instrument.js";
  return {
    filename: isSource ? `app:///src/${path}` : isBundle ? `app:///dist/${path}` : "<external>",
    lineno: frame.lineno,
    colno: frame.colno,
    in_app: isSource || isBundle,
  };
}

const HTTP_ATTRIBUTES = z.object({
  method: METHOD_LABEL,
  route: ROUTE_LABEL,
  status: z.number().int().min(100).max(599).optional().catch(undefined),
});

function safeAttributes(attributes: NonNullable<Sentry.Log["attributes"]> = {}) {
  return HTTP_ATTRIBUTES.parse({
    method: attributes.method ?? attributes["http.request.method"],
    route: attributes.route ?? attributes["http.route"],
    status: attributes.status ?? attributes["http.response.status_code"],
  });
}

function safeOperation(operation: SpanJSON["op"]): string {
  switch (operation) {
    case "http.server":
    case "http.client":
    case "middleware.hono":
    case "hono.request":
      return operation;
    default:
      return "function";
  }
}

function safeSpan(span: SpanJSON, requestLabels?: { method: string; route: string }): SpanJSON {
  const attributes = safeAttributes({ ...span.data, ...requestLabels });
  return {
    trace_id: span.trace_id,
    span_id: span.span_id,
    parent_span_id: span.parent_span_id,
    start_timestamp: span.start_timestamp,
    timestamp: span.timestamp,
    op: safeOperation(span.op),
    description:
      span.op === "http.client" ? "skills.upstream" : `${attributes.method} ${attributes.route}`,
    status: span.status,
    data: attributes,
  };
}

function safeEvent(event: Event): Event {
  const trace = event.contexts?.trace;
  const attributes = safeAttributes(event.contexts?.["api.request"] ?? trace?.data);
  return {
    event_id: event.event_id,
    type: event.type,
    timestamp: event.timestamp,
    start_timestamp: event.start_timestamp,
    platform: "node",
    level: event.level,
    release: event.release,
    environment: event.environment,
    transaction: `${attributes.method} ${attributes.route}`,
    contexts: trace
      ? {
          trace: {
            trace_id: trace.trace_id,
            span_id: trace.span_id,
            parent_span_id: trace.parent_span_id,
            op: safeOperation(trace.op),
            status: trace.status,
            data: attributes,
          },
        }
      : undefined,
    exception: event.exception
      ? {
          values: event.exception.values?.map((exception) => ({
            type: "Error",
            value: "API operation failed",
            mechanism: { type: "generic", handled: exception.mechanism?.handled ?? true },
            stacktrace: { frames: exception.stacktrace?.frames?.map(safeFrame) },
          })),
        }
      : undefined,
    spans: event.spans?.map((span) =>
      safeSpan(span, { method: attributes.method, route: attributes.route }),
    ),
  };
}

export function initializeApiTelemetry(
  env: NodeJS.ProcessEnv,
  transport?: Sentry.NodeOptions["transport"],
): ReturnType<typeof Sentry.init> {
  const dsn = env.SENTRY_DSN?.trim();
  if (!dsn) return undefined;
  const tracesSampleRate = Number(env.SENTRY_TRACES_SAMPLE_RATE?.trim() || "0.1");
  if (!Number.isFinite(tracesSampleRate) || tracesSampleRate < 0 || tracesSampleRate > 1) {
    throw new Error("SENTRY_TRACES_SAMPLE_RATE must be between 0 and 1");
  }
  return Sentry.init({
    dsn,
    release: env.SENTRY_RELEASE,
    environment: env.SENTRY_ENVIRONMENT ?? "development",
    tracesSampleRate,
    defaultIntegrations: false,
    integrations: [
      {
        name: "ApiTelemetryPrivacy",
        setup(client) {
          client.on("beforeEnvelope", (envelope) => {
            // Scope attributes are merged after the SDK log and metric filters.
            for (const [, payload] of envelope[1].filter(isLogOrMetricItem)) {
              for (const item of payload.items) {
                const labels = HTTP_ATTRIBUTES.parse({
                  method: item.attributes?.method?.value,
                  route: item.attributes?.route?.value,
                  status: item.attributes?.status?.value,
                });
                item.attributes = {
                  method: { type: "string", value: labels.method },
                  route: { type: "string", value: labels.route },
                };
                if (labels.status !== undefined) {
                  item.attributes.status = { type: "integer", value: labels.status };
                }
              }
            }
          });
        },
      },
      Sentry.onUncaughtExceptionIntegration(),
      Sentry.onUnhandledRejectionIntegration(),
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
    beforeSendSpan: (span) => safeSpan(span),
    beforeSendLog: (log) =>
      log.message === "api.request.completed"
        ? {
            level: "info",
            message: "api.request.completed",
            attributes: safeAttributes(log.attributes),
          }
        : null,
    beforeSendMetric: (metric) => {
      if (
        metric.name !== "api.request.count" &&
        metric.name !== "api.request.duration" &&
        metric.name !== "api.request.stdout_dropped"
      )
        return null;
      if (!Number.isFinite(metric.value) || metric.value < 0) return null;
      return {
        name: metric.name,
        type: metric.name === "api.request.duration" ? "distribution" : "counter",
        value: metric.value,
        unit: metric.name === "api.request.duration" ? "millisecond" : "none",
        attributes: safeAttributes(metric.attributes),
      };
    },
  });
}
