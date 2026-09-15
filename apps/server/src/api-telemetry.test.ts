// ============================================================================
// Skill Studio - Sentry transport acceptance
// ============================================================================

import { afterEach, expect, it, vi } from "vitest";
import * as Sentry from "@sentry/hono/node";
import { initializeApiTelemetry } from "./api-telemetry";
import { z } from "zod";
type Envelope = Parameters<ReturnType<NonNullable<Sentry.NodeOptions["transport"]>>["send"]>[0];

afterEach(async () => {
  await Sentry.close(2000);
  Sentry.getCurrentScope().setClient(undefined);
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("stays uninitialized without a DSN and rejects invalid sampling", () => {
  expect(initializeApiTelemetry({})).toBeUndefined();
  expect(Sentry.getClient()).toBeUndefined();
  expect(() =>
    initializeApiTelemetry({
      SENTRY_DSN: "https://public@example.invalid/1",
      SENTRY_TRACES_SAMPLE_RATE: "private-invalid",
    }),
  ).toThrow("SENTRY_TRACES_SAMPLE_RATE must be between 0 and 1");
});

it("exports correlated sanitized requests, upstream errors, logs and metrics without network", async () => {
  const envelopes: Envelope[] = [];
  const client = initializeApiTelemetry(
    {
      SENTRY_DSN: "https://public@example.invalid/1",
      SENTRY_RELEASE: "api-fixture-1",
      SENTRY_ENVIRONMENT: "test",
      SENTRY_TRACES_SAMPLE_RATE: "1",
    },
    () => ({
      send: async (envelope) => {
        envelopes.push(envelope);
        return {};
      },
      flush: async () => true,
    }),
  );
  expect(client).toBeDefined();
  const { createNodeRequestHandler, createApp } = await import("./server");
  vi.spyOn(process.stdout, "write").mockReturnValue(true);
  vi.spyOn(process.stdout, "writableNeedDrain", "get").mockReturnValue(true);
  vi.stubGlobal(
    "fetch",
    vi.fn().mockRejectedValue(new Error("private-token /Users/private-person private-skill-body")),
  );
  const handler = createNodeRequestHandler("private-api-key");
  const target = "/api/v1/skills/private-owner/private-repo/private-skill?q=private-prompt";
  const response = await handler(
    new Request(`http://localhost${target}`, {
      headers: { authorization: "Bearer private-credential", baggage: "private-baggage" },
    }),
    { incoming: { url: target } },
  );
  expect(response.status).toBe(502);
  expect(await response.json()).toEqual({
    error: "private-token /Users/private-person private-skill-body",
  });
  await Sentry.flush(2000);
  const wire = JSON.stringify(envelopes);
  expect(wire).not.toContain("private-");
  expect(wire).not.toContain("/Users/");
  const items = envelopes.flatMap<Envelope[1][number]>((envelope) => envelope[1]);
  const types = items.map((item) => item[0].type);
  expect(types).toContain("event");
  expect(types).toContain("transaction");
  expect(types).toContain("log");
  expect(types).toContain("trace_metric");
  const traceContext = z.object({ trace_id: z.string(), span_id: z.string() });
  const eventSchema = z.object({ contexts: z.object({ trace: traceContext }) });
  const error = eventSchema.parse(items.find((item) => item[0].type === "event")?.[1]);
  const transaction = eventSchema
    .extend({ spans: z.array(z.object({ op: z.string(), span_id: z.string() })) })
    .parse(items.find((item) => item[0].type === "transaction")?.[1]);
  expect(error.contexts.trace.trace_id).toBe(transaction.contexts.trace.trace_id);
  expect(transaction.spans.find((span) => span.op === "http.client")?.span_id).toBe(
    error.contexts.trace.span_id,
  );
  const logSchema = z.object({ items: z.array(z.object({ trace_id: z.string() })) });
  for (const item of items.filter((item) => ["log", "trace_metric"].includes(item[0].type))) {
    for (const record of logSchema.parse(item[1]).items) {
      expect(record.trace_id).toBe(transaction.contexts.trace.trace_id);
    }
  }
  expect(wire).toContain("skills.upstream");
  expect(wire).toContain("api.request.count");
  expect(wire).toContain("api.request.duration");
  const metricSchema = z.object({
    items: z.array(
      z.object({ name: z.string(), type: z.string(), unit: z.string(), value: z.number() }),
    ),
  });
  const dropped = items
    .filter((item) => item[0].type === "trace_metric")
    .flatMap((item) => metricSchema.parse(item[1]).items)
    .filter((metric) => metric.name === "api.request.stdout_dropped");
  expect(dropped).toEqual([
    { name: "api.request.stdout_dropped", type: "counter", unit: "none", value: 1 },
  ]);
  expect(wire).toContain("/api/v1/skills/:owner/:repo/:slug");

  envelopes.length = 0;
  Sentry.captureEvent({
    message: "private-message",
    server_name: "private-host",
    user: { email: "private-email" },
    request: {
      url: "https://private-repo/?q=private-prompt",
      headers: { authorization: "private-token" },
    },
    extra: { content: "private-skill-body" },
    tags: { path: "/Users/private-person" },
    exception: {
      values: [
        {
          type: "private-error-type",
          value: "private-error",
          stacktrace: {
            frames: [
              {
                filename: new URL("./server.ts", import.meta.url).href,
                lineno: 1,
                colno: 1,
                context_line: "private-source",
                vars: { credential: "private-token" },
              },
              { filename: new URL("./server.js", import.meta.url).href, lineno: 2, colno: 3 },
              { filename: new URL("./instrument.js", import.meta.url).href, lineno: 4, colno: 5 },
              { filename: new URL("../private/server.js", import.meta.url).href },
              { filename: "file://private-host/invalid", abs_path: "/Users/private-person" },
            ],
          },
        },
      ],
    },
  });
  Sentry.logger.info("private-message", { content: "private-body" });
  Sentry.metrics.count("private-metric", 1, { attributes: { content: "private-body" } });
  await Sentry.flush(2000);
  expect(JSON.stringify(envelopes)).not.toContain("private-");
  expect(JSON.stringify(envelopes)).toContain("app:///src/server.ts");
  expect(JSON.stringify(envelopes)).toContain("app:///dist/server.js");
  expect(JSON.stringify(envelopes)).toContain("app:///dist/instrument.js");
  expect(JSON.stringify(envelopes)).not.toContain("private/server.js");
  expect(
    envelopes.flatMap<Envelope[1][number]>((envelope) => envelope[1]).map((item) => item[0].type),
  ).toEqual(["event"]);

  envelopes.length = 0;
  const releaseRequests: Array<() => void> = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(
      () =>
        new Promise<Response>((resolve) => {
          const status = releaseRequests.length === 0 ? 200 : 503;
          releaseRequests.push(() =>
            resolve(Response.json({ secret: "private-content" }, { status })),
          );
        }),
    ),
  );
  const pending = ["/api/v1/skills", "/api/v1/skills/search?q=private-search"].map((path) =>
    handler(new Request(`http://localhost${path}`), { incoming: { url: path } }),
  );
  await vi.waitFor(() => expect(releaseRequests).toHaveLength(2));
  for (const release of [...releaseRequests].reverse()) release();
  expect((await Promise.all(pending)).map((result) => result.status)).toEqual([200, 503]);
  await Sentry.flush(2000);
  const concurrentItems = envelopes.flatMap<Envelope[1][number]>((envelope) => envelope[1]);
  const transactions = concurrentItems
    .filter((item) => item[0].type === "transaction")
    .map((item) => eventSchema.extend({ transaction: z.string() }).parse(item[1]));
  expect(transactions).toHaveLength(2);
  expect(new Set(transactions.map((item) => item.contexts.trace.trace_id)).size).toBe(2);
  const correlatedRecord = z.object({
    items: z.array(
      z.object({
        trace_id: z.string(),
        attributes: z.object({ route: z.object({ value: z.string() }) }),
      }),
    ),
  });
  expect(
    concurrentItems
      .filter((item) => item[0].type === "log")
      .flatMap((item) => correlatedRecord.parse(item[1]).items),
  ).toHaveLength(2);
  expect(
    concurrentItems
      .filter((item) => item[0].type === "trace_metric")
      .flatMap((item) => correlatedRecord.parse(item[1]).items),
  ).toHaveLength(6);
  for (const item of concurrentItems.filter((item) =>
    ["log", "trace_metric"].includes(item[0].type),
  )) {
    for (const record of correlatedRecord.parse(item[1]).items) {
      expect(
        transactions.find((transaction) => transaction.contexts.trace.trace_id === record.trace_id)
          ?.transaction,
      ).toBe(`GET ${record.attributes.route.value}`);
    }
  }
  expect(JSON.stringify(envelopes)).not.toContain("private-");

  envelopes.length = 0;
  const stderr = vi.spyOn(console, "error").mockImplementation(() => {});
  const app = createApp("private-api-key");
  app.get("/failure", () => {
    throw new Error("private-unhandled-error");
  });
  const failure = await Sentry.withIsolationScope(() =>
    Sentry.startSpan({ name: "api.request", op: "http.server" }, () => app.request("/failure")),
  );
  expect(failure.status).toBe(500);
  expect(await failure.text()).toBe("Internal Server Error");
  expect(stderr).not.toHaveBeenCalled();
  await Sentry.flush(2000);
  expect(
    envelopes
      .flatMap<Envelope[1][number]>((envelope) => envelope[1])
      .filter((item) => item[0].type === "event"),
  ).toHaveLength(1);
  expect(JSON.stringify(envelopes)).not.toContain("private-");
});
