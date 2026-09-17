// ============================================================================
// Skill Studio - API request telemetry
// ============================================================================

import * as Sentry from "@sentry/hono/node";
import { methodLabel, routeLabel } from "./request-telemetry-labels";

export function logRequestCompleted(request: {
  method: string;
  route: string;
  status: number;
  durationMs: number;
}): void {
  const attributes = {
    method: methodLabel(request.method),
    route: routeLabel(request.route),
    status: request.status,
  };
  const span = Sentry.getActiveSpan();
  if (span) {
    const root = Sentry.getRootSpan(span);
    root.setAttributes(attributes);
    if (request.status >= 500) root.setStatus({ code: 2 });
  }
  Sentry.logger.info("api.request.completed", attributes);
  Sentry.metrics.count("api.request.count", 1, { attributes });
  Sentry.metrics.distribution("api.request.duration", request.durationMs, {
    unit: "millisecond",
    attributes,
  });
  if (process.stdout.writableNeedDrain || process.stdout.destroyed) {
    Sentry.metrics.count("api.request.stdout_dropped", 1, { attributes });
    return;
  }
  process.stdout.write(
    `${JSON.stringify({
      schema_version: 1,
      event: "api.request.completed",
      ...attributes,
      duration_ms: Math.round(request.durationMs * 1000) / 1000,
    })}\n`,
  );
}
