// ============================================================================
// Skill Studio - API process termination fixtures
// ============================================================================

import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, existsSync, mkdirSync } from "node:fs";
import { request } from "node:http";
import { join } from "node:path";
import { once } from "node:events";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";
import { z } from "zod";

const envelopeSchema = z.array(
  z.tuple([z.object({}), z.array(z.tuple([z.object({ type: z.string() }), z.unknown()]))]),
);

it.each([
  ["disabled", "SIGTERM", 0],
  ["idle", "SIGINT", 0],
  ["idle", "SIGTERM", 0],
  ["headers", "SIGINT", 0],
  ["headers", "SIGTERM", 0],
  ["body", "SIGTERM", 0],
  ["ignore-abort", "SIGTERM", 1],
  ["transport-stall", "SIGTERM", 1],
] satisfies Array<[string, NodeJS.Signals, number]>)(
  "terminates %s with %s",
  async (mode, signal, expectedCode) => {
    mkdirSync("/tmp/skill-studio-delivery", { recursive: true });
    const root = mkdtempSync("/tmp/skill-studio-delivery/api-");
    const socketPath = join(root, "api.sock");
    const evidencePath = join(root, "envelopes.json");
    const child = spawn(
      process.execPath,
      ["--import", "tsx", "tests/runtime-fixture.ts", socketPath, evidencePath, mode],
      {
        cwd: fileURLToPath(new URL("../", import.meta.url)),
        env: { PATH: process.env.PATH, TMPDIR: root, HOME: root },
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    const exited = once(child, "exit");
    let output = "";
    let errors = "";
    child.stdout.on("data", (data: Buffer) => {
      output += data.toString();
    });
    child.stderr.on("data", (data: Buffer) => {
      errors += data.toString();
    });
    const waitForOutput = async (marker: string) => {
      const deadline = Date.now() + 3000;
      while (!output.includes(marker)) {
        if (child.exitCode !== null || Date.now() >= deadline)
          throw new Error(`Missing ${marker}: ${errors}`);
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
    };
    let connection: ReturnType<typeof request> | undefined;
    try {
      await waitForOutput("READY");
      connection = request({
        socketPath,
        path: ["idle", "disabled", "transport-stall"].includes(mode) ? "/health" : "/api/v1/skills",
      });
      connection.on("error", () => {});
      connection.on("response", (response) => response.resume());
      connection.end();
      await waitForOutput(
        ["idle", "disabled", "transport-stall"].includes(mode)
          ? "api.request.completed"
          : "UPSTREAM_STARTED",
      );
      const started = performance.now();
      expect(child.kill(signal)).toBe(true);
      if (mode === "headers") {
        await new Promise((resolve) => setTimeout(resolve, 40));
        child.kill(signal === "SIGINT" ? "SIGTERM" : "SIGINT");
      }
      const [code, exitSignal] = await exited;
      const elapsed = performance.now() - started;
      expect(code, errors).toBe(expectedCode);
      expect(exitSignal).toBeNull();
      expect(elapsed).toBeLessThan(7500);
      if (["headers", "body"].includes(mode)) expect(elapsed).toBeGreaterThanOrEqual(1900);
      if (mode === "ignore-abort") expect(elapsed).toBeGreaterThanOrEqual(3900);
      if (mode === "transport-stall") expect(elapsed).toBeGreaterThanOrEqual(6400);
      expect(errors).not.toContain("private-");
      if (mode === "disabled") expect(existsSync(evidencePath)).toBe(false);
      if (expectedCode === 0 && mode !== "disabled") {
        expect(existsSync(evidencePath)).toBe(true);
        const wire = readFileSync(evidencePath, "utf8");
        expect(wire).not.toContain("private-");
        const items = envelopeSchema.parse(JSON.parse(wire)).flatMap((envelope) => envelope[1]);
        const types = items.map((item) => item[0].type);
        expect(types).not.toContain("event");
        expect(types).toContain("transaction");
        expect(types).toContain("log");
        expect(types).toContain("trace_metric");
        const logContainer = z.object({
          items: z.array(
            z.object({ attributes: z.object({ status: z.object({ value: z.number() }) }) }),
          ),
        });
        const logs = items
          .filter((item) => item[0].type === "log")
          .flatMap((item) => logContainer.parse(item[1]).items);
        expect(logs).toHaveLength(1);
        expect(logs[0].attributes.status.value).toBe(mode === "idle" ? 200 : 503);
      }
    } finally {
      connection?.destroy();
      if (child.exitCode === null && child.signalCode === null) {
        child.kill("SIGKILL");
        await exited;
      }
      rmSync(root, { recursive: true, force: true });
    }
  },
  11000,
);
