// ============================================================================
// Skill Studio - API telemetry privacy fixtures
// ============================================================================

import { Writable } from "node:stream";
import { afterEach, expect, it, vi } from "vitest";
import { logRequestCompleted } from "./request-telemetry";
import { createNodeRequestHandler } from "./server";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it.each([
  ["/api/v1/skills?q=private-prompt", "/api/v1/skills", 200],
  ["/api/v1/skills/search?q=private-prompt", "/api/v1/skills/search", 200],
  [
    "/api/v1/skills/private-owner/private-repo/private-skill",
    "/api/v1/skills/:owner/:repo/:slug",
    200,
  ],
  ["/private-repository", "unmatched", 404],
  ["/api/v1/skills/private-owner/%2E%2E/private-skill", "unmatched", 400],
])("logs one bounded route record for %s", async (target, route, status) => {
  const output = vi.spyOn(process.stdout, "write").mockReturnValue(true);
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue(Response.json({ body: "private-skill-content" })),
  );
  const handler = createNodeRequestHandler("private-api-key");
  const response = await handler(
    new Request(`http://localhost${target}`, {
      headers: { Authorization: "Bearer private-credential" },
    }),
    { incoming: { url: target } },
  );

  expect(response.status).toBe(status);
  expect(output).toHaveBeenCalledTimes(1);
  const line = String(output.mock.calls[0][0]);
  expect(line).not.toContain("private-");
  const event = JSON.parse(line);
  expect(event).toEqual({
    schema_version: 1,
    event: "api.request.completed",
    method: "GET",
    route,
    status,
    duration_ms: expect.any(Number),
  });
  expect(event.duration_ms).toBeGreaterThanOrEqual(0);
});

it("does not retain an arbitrary route or method in telemetry", () => {
  const output = vi.spyOn(process.stdout, "write").mockReturnValue(true);
  logRequestCompleted({
    method: "private-method",
    route: "/Users/private-name",
    status: 400,
    durationMs: 1.23456,
  });
  expect(JSON.parse(String(output.mock.calls[0][0]))).toEqual({
    schema_version: 1,
    event: "api.request.completed",
    method: "OTHER",
    route: "unmatched",
    status: 400,
    duration_ms: 1.235,
  });
});

it("bounds queued stdout records during backpressure and resumes after drain", async () => {
  const writes: string[] = [];
  const callbacks: Array<() => void> = [];
  const sink = new Writable({
    highWaterMark: 64,
    write(chunk, _encoding, callback) {
      writes.push(chunk.toString());
      callbacks.push(callback);
    },
  });
  vi.spyOn(process.stdout, "write").mockImplementation((chunk) => sink.write(chunk));
  vi.spyOn(process.stdout, "writableNeedDrain", "get").mockImplementation(
    () => sink.writableNeedDrain,
  );
  const request = { method: "GET", route: "/api/v1/skills", status: 200, durationMs: 1 };
  try {
    logRequestCompleted(request);
    const queued = sink.writableLength;
    expect(queued).toBeGreaterThan(64);
    expect(queued).toBeLessThan(1024);
    for (let index = 0; index < 20_000; index++) logRequestCompleted(request);
    expect(sink.writableLength).toBe(queued);
    expect(writes).toHaveLength(1);
    expect(callbacks).toHaveLength(1);
    const drained = new Promise<void>((resolve) => sink.once("drain", resolve));
    callbacks.shift()!();
    await drained;
    expect(sink.writableLength).toBe(0);
    logRequestCompleted(request);
    expect(writes).toHaveLength(2);
    expect(JSON.parse(writes[1]).event).toBe("api.request.completed");
    callbacks.shift()!();
  } finally {
    sink.destroy();
  }
});

it("does not enqueue stdout records on a destroyed stream", () => {
  const output = vi.spyOn(process.stdout, "write").mockReturnValue(true);
  vi.spyOn(process.stdout, "destroyed", "get").mockReturnValue(true);
  logRequestCompleted({ method: "GET", route: "/api/v1/skills", status: 200, durationMs: 1 });
  expect(output).not.toHaveBeenCalled();
});
