import { afterEach, expect, it } from "vitest";
import * as Sentry from "@sentry/react";
import { z } from "zod";
import {
  desktopReactErrors,
  initializeDesktopTelemetry,
  traceInventoryRead,
} from "./desktop-telemetry";

type Envelope = Parameters<ReturnType<NonNullable<Sentry.BrowserOptions["transport"]>>["send"]>[0];
const config = {
  VITE_DESKTOP_SENTRY_DSN: "https://public@example.invalid/1",
  VITE_DESKTOP_SENTRY_ENVIRONMENT: "test",
  VITE_DESKTOP_SENTRY_RELEASE: "skill-studio@0.1.0",
  VITE_DESKTOP_SENTRY_TRACES_SAMPLE_RATE: "1",
};
function capture(release = config.VITE_DESKTOP_SENTRY_RELEASE) {
  const envelopes: Envelope[] = [];
  initializeDesktopTelemetry({ ...config, VITE_DESKTOP_SENTRY_RELEASE: release }, () => ({
    send: async (envelope) => {
      envelopes.push(envelope);
      return {};
    },
    flush: async () => true,
  }));
  return envelopes;
}
afterEach(async () => {
  await Sentry.close(2000);
  Sentry.getCurrentScope().setClient(undefined);
  Sentry.getCurrentScope().clear();
  Sentry.getIsolationScope().removeAttribute("private-isolation");
  Sentry.getIsolationScope().removeAttribute("operation");
});

it("keeps missing and invalid telemetry configuration nonfatal and disabled", () => {
  expect(initializeDesktopTelemetry({})).toBeUndefined();
  for (const invalid of [
    { VITE_DESKTOP_SENTRY_TRACES_SAMPLE_RATE: "NaN" },
    { VITE_DESKTOP_SENTRY_TRACES_SAMPLE_RATE: "2" },
    { VITE_DESKTOP_SENTRY_ENVIRONMENT: "private-project" },
    { VITE_DESKTOP_SENTRY_RELEASE: "private-branch" },
    { VITE_DESKTOP_SENTRY_RELEASE: "skill-studio@0.1.0+ABCDEF012345" },
    { VITE_DESKTOP_SENTRY_RELEASE: "skill-studio@0.1.0+" + "a".repeat(11) },
    { VITE_DESKTOP_SENTRY_RELEASE: "skill-studio@0.1.0+" + "a".repeat(65) },
    { VITE_DESKTOP_SENTRY_DSN: "http://public@remote.invalid/1" },
    { VITE_DESKTOP_SENTRY_DSN: "https://public:private-key@example.invalid/1" },
  ])
    expect(initializeDesktopTelemetry({ ...config, ...invalid })).toBeUndefined();
  expect(Sentry.getClient()).toBeUndefined();
});

it("preserves independent overlapping read results and failures with correlated safe signals", async () => {
  const envelopes = capture();
  const result = { path: "/Users/private-path", body: "private-skill-content" };
  const failure = new Error("private-read-error");
  let finish: (value: typeof result) => void = () => {
    throw new Error("Read did not start");
  };
  const first = traceInventoryRead(
    "inventory.read",
    () =>
      new Promise<typeof result>((resolve) => {
        finish = resolve;
      }),
  );
  const second = traceInventoryRead("snapshot.read", async () => {
    throw failure;
  });
  await expect(second).rejects.toBe(failure);
  finish(result);
  expect(await first).toBe(result);
  await Sentry.flush(2000);
  const items = envelopes.flatMap<Envelope[1][number]>((envelope) => envelope[1]);
  const transactions = items
    .filter(([header]) => header.type === "transaction")
    .map(([, value]) =>
      z
        .object({
          transaction: z.string(),
          contexts: z.object({ trace: z.object({ trace_id: z.string(), status: z.string() }) }),
        })
        .parse(value),
    );
  expect(transactions).toHaveLength(2);
  expect(new Set(transactions.map((tx) => tx.contexts.trace.trace_id)).size).toBe(2);
  expect(
    transactions.find((tx) => tx.transaction === "inventory.read")?.contexts.trace.status,
  ).toBe("ok");
  expect(transactions.find((tx) => tx.transaction === "snapshot.read")?.contexts.trace.status).toBe(
    "unknown_error",
  );
  for (const type of ["log", "trace_metric"]) {
    const signals = items
      .filter(([header]) => header.type === type)
      .flatMap(
        ([, value]) =>
          z.object({ items: z.array(z.object({ trace_id: z.string() })) }).parse(value).items,
      );
    const copies = type === "trace_metric" ? 2 : 1;
    expect(signals).toHaveLength(2 * copies);
    expect(signals.map((signal) => signal.trace_id).sort()).toEqual(
      transactions.flatMap((tx) => Array<string>(copies).fill(tx.contexts.trace.trace_id)).sort(),
    );
  }
  expect(items.some(([header]) => header.type === "event")).toBe(false);
  expect(JSON.stringify(envelopes)).not.toContain("private-");
});

it("sanitizes React error hooks and retains only packaged frame locations", async () => {
  const envelopes = capture();
  Sentry.withScope((scope) => {
    scope.setUser({ email: "private-email" });
    scope.setExtra("skill", "private-content");
    scope.setTag("repo", "private-repo");
    desktopReactErrors.onCaughtError(new Error("private-error"), {
      componentStack: "/Users/private-component",
    });
  });
  Sentry.captureEvent({
    exception: {
      values: [
        {
          value: "private-message",
          stacktrace: {
            frames: [
              {
                filename: "tauri://localhost/assets/index-aBc123.js?private-query",
                lineno: 12,
                colno: 7,
                context_line: "private-code",
              },
              { filename: "file:///Users/private-source.ts", lineno: 99 },
            ],
          },
        },
      ],
    },
  });
  Sentry.logger.info("private-log");
  Sentry.metrics.count("private-metric");
  await Sentry.flush(2000);
  const items = envelopes.flatMap<Envelope[1][number]>((envelope) => envelope[1]);
  expect(items.filter(([header]) => header.type === "event")).toHaveLength(2);
  const wire = JSON.stringify(envelopes);
  expect(wire).toContain("app:///assets/index-aBc123.js");
  expect(wire).not.toContain("private-");
  expect(wire).not.toContain("/Users/");
});

it("passes only the current span trace header to the IPC callback", async () => {
  const envelopes = capture();
  let header: string | undefined;
  await traceInventoryRead("snapshot.read", async (telemetryTrace) => {
    header = telemetryTrace;
    return undefined;
  });
  await Sentry.flush(2000);
  const transaction = z
    .object({
      contexts: z.object({
        trace: z.object({
          trace_id: z.string(),
          span_id: z.string(),
        }),
      }),
    })
    .parse(
      envelopes
        .flatMap<Envelope[1][number]>((envelope) => envelope[1])
        .find(([item]) => item.type === "transaction")?.[1],
    );
  expect(header).toBe(
    `${transaction.contexts.trace.trace_id}-${transaction.contexts.trace.span_id}-1`,
  );
  expect(header).toMatch(/^[0-9a-f]{32}-[0-9a-f]{16}-[01]$/);
});

it("keeps history read names and private-free signals", async () => {
  const envelopes = capture();
  await expect(
    traceInventoryRead("history.read", async () => {
      throw new Error("PRIVATE_HISTORY_CONTENT");
    }),
  ).rejects.toThrow("PRIVATE_HISTORY_CONTENT");
  await Sentry.flush(2000);
  const encoded = JSON.stringify(envelopes);
  expect(encoded).toContain("history.read");
  expect(encoded).toContain("desktop.ipc.finished");
  expect(encoded).toContain("desktop.ipc.count");
  expect(encoded).not.toContain("PRIVATE_HISTORY_CONTENT");
});

it("exports the build-specific desktop release unchanged", async () => {
  const release = "skill-studio@0.1.0+012345abcdef";
  const envelopes = capture(release);
  Sentry.captureException(new Error("fixture error"));
  await Sentry.flush(2000);
  const releases: string[] = [];
  for (const [, items] of envelopes) {
    for (const [header, payload] of items) {
      if (header.type === "event") {
        releases.push(z.object({ release: z.string() }).parse(payload).release);
      }
    }
  }
  expect(releases).toEqual([release]);
});

it("exports bounded IPC duration distributions and rejects unsafe metric data", async () => {
  const envelopes = capture();
  await traceInventoryRead("inventory.read", async () => "private-result");
  await expect(
    traceInventoryRead("snapshot.read", async () => {
      throw new Error("private-failure");
    }),
  ).rejects.toThrow("private-failure");
  Sentry.metrics.distribution("desktop.ipc.duration", 12.5, {
    unit: "millisecond",
    attributes: { operation: "history.read", path: "/Users/private-path" },
  });
  Sentry.metrics.distribution("desktop.ipc.duration", 42, {
    unit: "second",
    attributes: { operation: "history.read" },
  });
  for (const value of [-1, Number.NaN, Number.POSITIVE_INFINITY]) {
    Sentry.metrics.distribution("desktop.ipc.duration", value, {
      unit: "millisecond",
      attributes: { operation: "history.read" },
    });
  }
  Sentry.metrics.distribution("desktop.ipc.duration", 42, {
    unit: "millisecond",
    attributes: { operation: "private-operation" },
  });
  Sentry.metrics.count("desktop.ipc.duration", 42, {
    attributes: { operation: "history.read" },
  });
  await Sentry.flush(2000);
  const metrics = envelopes
    .flatMap<Envelope[1][number]>((envelope) => envelope[1])
    .filter(([header]) => header.type === "trace_metric")
    .flatMap(
      ([, payload]) =>
        z
          .object({
            items: z.array(
              z.object({
                name: z.string(),
                type: z.string(),
                value: z.number(),
                unit: z.string(),
              }),
            ),
          })
          .parse(payload).items,
    )
    .filter((metric) => metric.name === "desktop.ipc.duration");
  expect(metrics).toHaveLength(3);
  expect(
    metrics.every(
      (metric) =>
        metric.type === "distribution" &&
        metric.unit === "millisecond" &&
        Number.isFinite(metric.value) &&
        metric.value >= 0,
    ),
  ).toBe(true);
  expect(metrics.some((metric) => metric.value === 12.5)).toBe(true);
  expect(JSON.stringify(envelopes)).not.toContain("private-");
});

it("removes serialized scope attributes while preserving duration operation labels", async () => {
  const envelopes = capture();
  Sentry.getIsolationScope().setAttribute("private-isolation", "private-content");
  Sentry.getIsolationScope().setAttribute("operation", "private-operation");
  await Sentry.withScope(async (scope) => {
    scope.setAttribute("private-current", "private-content");
    await traceInventoryRead("history.read", async () => "private-result");
  });
  await Sentry.flush(2000);
  const items = envelopes.flatMap<Envelope[1][number]>((envelope) => envelope[1]);
  const schema = z.object({
    items: z.array(
      z.object({
        name: z.string().optional(),
        attributes: z.record(z.string(), z.unknown()),
      }),
    ),
  });
  const records = items
    .filter((item) => ["log", "trace_metric"].includes(item[0].type))
    .flatMap((item) => schema.parse(item[1]).items);
  expect(records).toHaveLength(3);
  for (const record of records) {
    expect(record.attributes).toEqual(
      record.name === "desktop.ipc.duration"
        ? { operation: { type: "string", value: "history.read" } }
        : {},
    );
  }
  expect(JSON.stringify(envelopes)).not.toContain("private-");
});

it.each([
  ["onUncaughtError", false],
  ["onCaughtError", true],
  ["onRecoverableError", true],
] as const)("classifies %s with handled=%s", async (hook, handled) => {
  const envelopes = capture();
  desktopReactErrors[hook](new Error("private-render-error"), {
    componentStack: "private-component",
  });
  await Sentry.flush(2000);
  const events = envelopes
    .flatMap<Envelope[1][number]>((envelope) => envelope[1])
    .filter((item) => item[0].type === "event");
  expect(events).toHaveLength(1);
  expect(events[0][1]).toMatchObject({ exception: { values: [{ mechanism: { handled } }] } });
  expect(JSON.stringify(envelopes)).not.toContain("private-");
});
