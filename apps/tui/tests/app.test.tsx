import { testRender } from "@opentui/react/test-utils";
import { describe, expect, test } from "bun:test";

import { App } from "../src/App.tsx";
import {
  CliTransportError,
  type Transport,
  type WatchHandle,
  type WatchLineHandler,
  type WatchStatusHandler,
} from "../src/cli-transport.ts";
import type { Envelope, Inventory } from "../src/cli-types.ts";

function emptyInventory(): Inventory {
  return { skills: [], projects: [], completeness: "complete", observations: [], timings: [] };
}

function okEnvelope<T>(data: T): Envelope<T> {
  return {
    schema_version: 1,
    operation: "scan",
    scope: {
      id: "scope:v1/test",
      kind: "fixture",
      home: "/test",
      projects: [],
      history_root: "/test/.history",
    },
    status: "ok",
    data,
    errors: [],
    correlation_id: "01TESTCORRELATIONID",
    event_id: null,
  };
}

/** A stub transport: `scan` resolves once, `watch` immediately invokes `onLine`
 * with the given line (or never, when omitted), and never touches a process. */
function stubTransport(options: {
  watchLine?: { revision: number; inventory: Inventory };
  scanResult?: Promise<Envelope<Inventory>>;
}): Transport {
  const scanResult = options.scanResult ?? Promise.resolve(okEnvelope(emptyInventory()));
  return {
    run<T>() {
      // SAFETY: this stub only backs `App`'s `scan`/`diagnose` calls in
      // these tests, both of which request `Envelope<Inventory>`; the
      // real `Transport.run<T>` signature is generic over every op.
      return scanResult as Promise<Envelope<T>>;
    },
    watch: (
      _since: number | undefined,
      onLine: WatchLineHandler,
      _onStatus: WatchStatusHandler,
    ): WatchHandle => {
      if (options.watchLine !== undefined) onLine(options.watchLine);
      return { stop() {} };
    },
  };
}

describe("App", () => {
  test("a watch line updates the revision in the status line", async () => {
    const transport = stubTransport({ watchLine: { revision: 42, inventory: emptyInventory() } });
    const setup = await testRender(<App transport={transport} />, { width: 80, height: 24 });
    await setup.flush();
    const frame = await setup.waitForFrame((f) => f.includes("revision: 42"));
    expect(frame).toContain("revision: 42");
  });

  test("an error envelope renders the error state without a stack trace", async () => {
    const transport = stubTransport({
      scanResult: Promise.reject(
        new CliTransportError("invalid_scope", "fixture directory does not exist"),
      ),
    });
    const setup = await testRender(<App transport={transport} />, { width: 80, height: 24 });
    await setup.flush();
    const frame = await setup.waitForFrame((f) => f.includes("invalid_scope"));
    expect(frame).toContain("error (invalid_scope): fixture directory does not exist");
    expect(frame).not.toContain(" at ");
  });
});
