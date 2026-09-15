import { writeFileSync } from "node:fs";
import { initializeApiTelemetry } from "../src/api-telemetry";
import { createApiRuntime, handleTerminationSignals } from "../src/api-runtime";
import type * as Sentry from "@sentry/hono/node";

type Envelope = Parameters<ReturnType<NonNullable<Sentry.NodeOptions["transport"]>>["send"]>[0];
const [socketPath, evidencePath, mode] = process.argv.slice(2);
const envelopes: Envelope[] = [];
initializeApiTelemetry(
  mode === "disabled"
    ? {}
    : {
        SENTRY_DSN: "https://public@example.invalid/1",
        SENTRY_RELEASE: "shutdown-fixture",
        SENTRY_ENVIRONMENT: "test",
        SENTRY_TRACES_SAMPLE_RATE: "1",
      },
  () => ({
    send: async (envelope) => {
      envelopes.push(envelope);
      writeFileSync(evidencePath, JSON.stringify(envelopes));
      return {};
    },
    flush: () =>
      mode === "transport-stall" ? new Promise<boolean>(() => {}) : Promise.resolve(true),
  }),
);
const { createNodeRequestHandler } = await import("../src/server");
globalThis.fetch = async (_input, options) => {
  process.stdout.write("UPSTREAM_STARTED\n");
  if (mode === "ignore-abort") return new Promise<Response>(() => {});
  const signal = options?.signal;
  if (!signal) throw new Error("Fixture requires cancellation");
  if (mode === "body") {
    return new Response(
      new ReadableStream({
        start(controller) {
          signal.addEventListener(
            "abort",
            () => controller.error(new Error("private-body-abort")),
            { once: true },
          );
        },
      }),
    );
  }
  return new Promise<Response>((_resolve, reject) => {
    signal.addEventListener("abort", () => reject(new Error("private-header-abort")), {
      once: true,
    });
  });
};
const runtime = createApiRuntime((signal) => createNodeRequestHandler("fixture-key", signal));
handleTerminationSignals(runtime);
runtime.server.listen(socketPath, () => process.stdout.write("READY\n"));
