#!/usr/bin/env bun
// ============================================================================
// Fake `skill-studio` binary for cli-transport tests.
// Prints the same envelope/watch-line shapes the real CLI prints, chosen by
// `FAKE_CLI_MODE`, so the transport module's parsing and error mapping are
// exercised without spawning the real Rust binary (that's what the
// `tests/integration` suite does instead).
// ============================================================================

interface Envelope {
  schema_version: number;
  operation: string;
  scope: { id: string; kind: string; home: string; projects: string[]; history_root: string };
  status: "ok" | "partial" | "error";
  data: unknown;
  errors: { code: string; message: string; path: string | null }[];
  correlation_id: string;
  event_id: string | null;
}

const EMPTY_INVENTORY = {
  skills: [{ name: "write-tests", description: "Writes tests.", deployments: [] }],
  projects: [],
  completeness: "complete",
  observations: [],
  timings: [],
};

function envelope<T>(
  operation: string,
  status: Envelope["status"],
  data: T,
  errors: Envelope["errors"] = [],
): Envelope {
  return {
    schema_version: 1,
    operation,
    scope: {
      id: "scope:v1/fake",
      kind: "fixture",
      home: "/fake-home",
      projects: [],
      history_root: "/fake-home/.history",
    },
    status,
    data,
    errors,
    correlation_id: "01FAKECORRELATIONID",
    event_id: null,
  };
}

const [, , op] = process.argv;
const mode = process.env["FAKE_CLI_MODE"] ?? "ok";

if (op === "scan" || op === "diagnose") {
  if (mode === "error") {
    process.stdout.write(
      `${JSON.stringify(envelope(op, "error", null, [{ code: "invalid_scope", message: "fixture directory does not exist", path: "~/fixture" }]))}\n`,
    );
    process.exit(2);
  }
  if (mode === "malformed") {
    process.stdout.write("this is not json\n");
    process.exit(0);
  }
  const data = op === "diagnose" ? { inventory: EMPTY_INVENTORY, issues: [] } : EMPTY_INVENTORY;
  process.stdout.write(`${JSON.stringify(envelope(op, "ok", data))}\n`);
  process.exit(0);
}

if (op === "watch") {
  if (mode === "two-lines") {
    process.stdout.write(`${JSON.stringify({ revision: 1, inventory: EMPTY_INVENTORY })}\n`);
    process.stdout.write(`${JSON.stringify({ revision: 2, inventory: EMPTY_INVENTORY })}\n`);
  } else {
    process.stdout.write(`${JSON.stringify({ revision: 1, inventory: EMPTY_INVENTORY })}\n`);
  }
  // Exits immediately after its line(s), like an unexpectedly-killed watcher,
  // so tests can observe `watch()`'s restart behavior deterministically.
  process.exit(0);
}

process.stderr.write(`fake-cli: unknown op ${op ?? "(none)"}\n`);
process.exit(2);
