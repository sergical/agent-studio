import { afterEach, expect, it } from "vitest";
import * as Sentry from "@sentry/react";
import { z } from "zod";
import {
  initializeMarketingTelemetry,
  marketingReactErrors,
  recordMarketingBootstrap,
} from "./marketing-telemetry";

type Envelope = Parameters<ReturnType<NonNullable<Sentry.BrowserOptions["transport"]>>["send"]>[0];

afterEach(async () => {
  await Sentry.close(2000);
  Sentry.getCurrentScope().setClient(undefined);
});

it("leaves telemetry disabled without a DSN", () => {
  expect(initializeMarketingTelemetry({ MODE: "test" })).toBeUndefined();
  expect(Sentry.getClient()).toBeUndefined();
});

it.each([
  [undefined, 0.1],
  ["", 0.1],
  ["   ", 0.1],
  ["0", 0],
])("uses a safe trace sample rate for %j", (VITE_SENTRY_TRACES_SAMPLE_RATE, tracesSampleRate) => {
  initializeMarketingTelemetry({
    MODE: "test",
    VITE_SENTRY_DSN: "https://public@example.invalid/1",
    VITE_SENTRY_TRACES_SAMPLE_RATE,
  });

  expect(Sentry.getClient()?.getOptions().tracesSampleRate).toBe(tracesSampleRate);
});

it("exports sanitized React errors, spans, logs and metrics to a fixture transport", async () => {
  const envelopes: Envelope[] = [];
  initializeMarketingTelemetry(
    {
      MODE: "test",
      VITE_SENTRY_DSN: "https://public@example.invalid/1",
      VITE_SENTRY_TRACES_SAMPLE_RATE: "1",
      VITE_SENTRY_RELEASE: "marketing-fixture",
    },
    () => ({
      send: async (envelope) => {
        envelopes.push(envelope);
        return {};
      },
      flush: async () => true,
    }),
  );
  Sentry.withScope((scope) => {
    scope.setUser({ email: "private-email" });
    scope.setExtra("body", "private-body");
    Sentry.startSpan(
      {
        name: "private-page?private-query",
        op: "pageload",
        attributes: { url: "https://private-host/?private-query" },
      },
      () => {
        marketingReactErrors.onUncaughtError(new Error("private-error"), {
          componentStack: "private-component /Users/private-person",
        });
        recordMarketingBootstrap();
        Sentry.startSpan({ name: "private-click-target", op: "ui.interaction.click" }, () => {});
      },
    );
  });
  Sentry.captureEvent({
    exception: {
      values: [
        {
          value: "private-message",
          stacktrace: {
            frames: [
              {
                filename: "https://private-host/assets/index-abc123.js?private-query#private-hash",
                context_line: "private-source",
                abs_path: "/Users/private-path",
                lineno: 42,
                colno: 3,
              },
              {
                filename:
                  "https://private-host/assets/marketing-telemetry-dGZ_Q8cX.js?private-query",
                lineno: 12,
                colno: 8,
              },
              {
                filename: "https://private-host/assets/react-OrosJ8bI.js#private-hash",
                lineno: 7,
                colno: 2,
              },
              { filename: "file:///Users/private-person/marketing-telemetry-dGZ_Q8cX.js" },
              { filename: "https://private-host/assets/private-content-abc123.js" },
              { filename: "https://private-host/assets/marketing-telemetry.js" },
            ],
          },
        },
      ],
    },
  });
  Sentry.logger.info("private-message");
  Sentry.metrics.count("private-metric");
  await Sentry.flush(2000);
  const wire = JSON.stringify(envelopes);
  expect(wire).not.toContain("private-");
  expect(wire).not.toContain("/Users/");
  expect(wire).toContain("app:///assets/index-abc123.js");
  expect(wire).toContain("app:///assets/marketing-telemetry-dGZ_Q8cX.js");
  expect(wire).toContain("app:///assets/react-OrosJ8bI.js");
  expect(wire).not.toContain("app:///assets/marketing-telemetry.js");
  expect(wire).toContain("ui.interaction.click");
  const items = envelopes.flatMap<Envelope[1][number]>((envelope) => envelope[1]);
  const types = items.map((item) => item[0].type);
  expect(types.filter((type) => type === "event")).toHaveLength(2);
  expect(types).toContain("transaction");
  expect(types).toContain("log");
  expect(types).toContain("trace_metric");
  const eventSchema = z.object({
    contexts: z.object({ trace: z.object({ trace_id: z.string() }) }),
  });
  const error = eventSchema.parse(items.find((item) => item[0].type === "event")?.[1]);
  const transaction = eventSchema.parse(items.find((item) => item[0].type === "transaction")?.[1]);
  expect(error.contexts.trace.trace_id).toBe(transaction.contexts.trace.trace_id);
  for (const item of items.filter((item) => ["log", "trace_metric"].includes(item[0].type))) {
    const container = z
      .object({ items: z.array(z.object({ trace_id: z.string() })) })
      .parse(item[1]);
    expect(container.items).toHaveLength(1);
    expect(container.items[0].trace_id).toBe(transaction.contexts.trace.trace_id);
  }
});
