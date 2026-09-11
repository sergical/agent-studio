import { describe, expect, test } from "bun:test";

import { CliTransportError, run, watch } from "../src/cli-transport.ts";
import type { Inventory } from "../src/cli-types.ts";

const FAKE_CLI = new URL("./fixtures/fake-cli.ts", import.meta.url).pathname;

interface FakeCliConfig {
  bin: string;
  home: string;
}

function config(mode: string): FakeCliConfig {
  // `home` is unused by the fake CLI but keeps the call shape realistic.
  process.env["FAKE_CLI_MODE"] = mode;
  return { bin: FAKE_CLI, home: "/fake-home" };
}

describe("run", () => {
  test("resolves with the parsed envelope on an ok status", async () => {
    const envelope = await run<Inventory>("scan", [], config("ok"));
    expect(envelope.status).toBe("ok");
    expect(envelope.data?.skills).toHaveLength(1);
    expect(envelope.data?.skills[0]?.name).toBe("write-tests");
  });

  test("throws a CliTransportError with the envelope's error code on an error status", async () => {
    await expect(run("scan", [], config("error"))).rejects.toMatchObject({
      name: "CliTransportError",
      code: "invalid_scope",
    });
  });

  test("throws a transport_error on non-JSON output", async () => {
    await expect(run("scan", [], config("malformed"))).rejects.toMatchObject({
      name: "CliTransportError",
      code: "transport_error",
    });
  });

  test("throws a transport_error when the binary cannot be spawned", async () => {
    await expect(
      run("scan", [], { bin: "/nonexistent/skill-studio-binary" }),
    ).rejects.toBeInstanceOf(CliTransportError);
  });
});

describe("watch", () => {
  test("calls onLine with each parsed WatchLine", async () => {
    process.env["FAKE_CLI_MODE"] = "two-lines";
    const lines: number[] = [];
    const handle = watch(
      undefined,
      (line) => {
        lines.push(line.revision);
      },
      () => {},
      { bin: FAKE_CLI },
    );
    await new Promise((resolve) => setTimeout(resolve, 200));
    handle.stop();
    expect(lines).toEqual([1, 2]);
  });

  test("restarts with backoff and reports status after an unexpected exit", async () => {
    process.env["FAKE_CLI_MODE"] = "ok-watch";
    const statuses: number[] = [];
    const handle = watch(
      undefined,
      () => {},
      (status) => {
        statuses.push(status.attempt);
      },
      { bin: FAKE_CLI },
    );
    await new Promise((resolve) => setTimeout(resolve, 400));
    handle.stop();
    expect(statuses.length).toBeGreaterThan(0);
    expect(statuses[0]).toBe(1);
  });
});
