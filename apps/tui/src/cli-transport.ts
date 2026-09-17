// ============================================================================
// Skill Studio TUI - CLI transport
// The only module that spawns a process. Every screen and hook goes through
// `run()` or `watch()` here; no component calls `Bun.spawn` directly.
// ============================================================================

import { z } from "zod";

import type { Envelope, WatchLine } from "./cli-types.ts";

/** Subcommand names this module knows how to run. Matches `apps/cli/src/main.rs::Command`. */
export type CliOp = "scan" | "diagnose" | "capabilities";

/** Scope flags shared by every call, so a test can point the whole app at one fixture. */
export interface ScopeConfig {
  /** Path to the `skill-studio` binary. Defaults to `SKILL_STUDIO_BIN`, then `skill-studio` on `PATH`. */
  bin?: string;
  /** `--home <dir>`. */
  home?: string;
  /** `--fixture <dir>`. */
  fixture?: string;
  /** `--project <dir>`, repeatable. */
  projects?: string[];
}

/** A transport failure: a spawn error, unparsable output, or an error-status envelope.
 *
 * Never a bare string: every catch site gets a stable `code` plus the full
 * envelope when one was printed, so the UI can show `code`/`message` without
 * a stack trace. */
export class CliTransportError extends Error {
  readonly code: string;
  readonly envelope: Envelope<unknown> | null;

  constructor(code: string, message: string, envelope: Envelope<unknown> | null = null) {
    super(message);
    this.name = "CliTransportError";
    this.code = code;
    this.envelope = envelope;
  }
}

function binaryPath(config: ScopeConfig): string {
  return config.bin ?? process.env["SKILL_STUDIO_BIN"] ?? "skill-studio";
}

function scopeFlags(config: ScopeConfig): string[] {
  const flags: string[] = [];
  if (config.fixture !== undefined) flags.push("--fixture", config.fixture);
  if (config.home !== undefined) flags.push("--home", config.home);
  for (const project of config.projects ?? []) flags.push("--project", project);
  return flags;
}

/** The envelope's required top-level shape, parsed at this module's one I/O
 * boundary. Not a full DTO schema: `data`'s fields are trusted once this
 * passes, since the wire contract is fixed (`docs/spec-core-primitives.md`
 * 8.1) and a shape drift there is a contract break the CLI's own tests catch
 * first. */
const EnvelopeContractSchema = z.object({
  operation: z.string(),
  status: z.string(),
  errors: z.array(z.unknown()),
});

/** Runs one `skill-studio <op> --json` invocation and returns its envelope.
 *
 * Throws a `CliTransportError` when the process could not be run, its
 * output was not one JSON document, or the envelope's `status` is
 * `"error"`. Resolves normally for `"ok"` and `"partial"`, since both carry
 * usable `data` (a `diagnose` with warnings is `"ok"` with a non-zero exit;
 * a timed-out root is `"partial"` with the rest of the inventory). */
export async function run<T>(
  op: CliOp,
  args: string[],
  config: ScopeConfig = {},
): Promise<Envelope<T>> {
  const command = [binaryPath(config), op, ...scopeFlags(config), ...args, "--json"];
  let proc: Bun.ReadableSubprocess;
  try {
    proc = Bun.spawn(command, { stdout: "pipe", stderr: "pipe", env: process.env });
  } catch (cause) {
    throw new CliTransportError(
      "transport_error",
      `could not spawn ${command[0]}: ${cause instanceof Error ? cause.message : String(cause)}`,
    );
  }

  const [stdout, stderr] = await Promise.all([proc.stdout.text(), proc.stderr.text()]);
  const exitCode = await proc.exited;

  let parsed: unknown;
  try {
    parsed = JSON.parse(stdout.trim());
  } catch {
    throw new CliTransportError(
      "transport_error",
      `skill-studio ${op} did not print one JSON document (exit ${String(exitCode)}): ${
        stderr.trim() || stdout.trim() || "(empty output)"
      }`,
    );
  }
  if (!EnvelopeContractSchema.safeParse(parsed).success) {
    throw new CliTransportError(
      "transport_error",
      `skill-studio ${op} printed JSON that is not an envelope`,
    );
  }
  // SAFETY: `EnvelopeContractSchema` checked the envelope's required top-level
  // fields; the DTO fields inside `data` are trusted per the fixed wire
  // contract (docs/spec-core-primitives.md 8.1).
  const envelope = parsed as Envelope<T>;

  if (envelope.status === "error") {
    const first = envelope.errors[0];
    throw new CliTransportError(
      first?.code ?? "transport_error",
      first?.message ?? `skill-studio ${op} failed`,
      envelope,
    );
  }

  return envelope;
}

/** A line handler for `watch()`, called once per parsed `WatchLine`. */
export type WatchLineHandler = (line: WatchLine) => void;

/** A status handler for `watch()`, called on a restart after an unexpected exit. */
export type WatchStatusHandler = (status: {
  kind: "restarting";
  attempt: number;
  delayMs: number;
}) => void;

const RESTART_BASE_DELAY_MS = 250;
const RESTART_MAX_DELAY_MS = 5000;

/** Handle returned by `watch()`. */
export interface WatchHandle {
  /** Kills the watch child and stops any pending restart. */
  stop: () => void;
}

/** Runs `skill-studio watch --json --since <revision>` and streams parsed
 * lines to `onLine`. Restarts the child with exponential backoff if it
 * exits unexpectedly (any exit before `stop()` is called), reporting each
 * restart to `onStatus` so the UI can show it on the status line. */
export function watch(
  since: number | undefined,
  onLine: WatchLineHandler,
  onStatus: WatchStatusHandler,
  config: ScopeConfig = {},
): WatchHandle {
  let stopped = false;
  let restartAttempt = 0;
  let currentProc: Bun.ReadableSubprocess | null = null;
  let restartTimer: ReturnType<typeof setTimeout> | null = null;

  const args = since !== undefined ? ["--since", String(since)] : [];
  const command = [binaryPath(config), "watch", ...scopeFlags(config), ...args, "--json"];

  async function readLines(proc: Bun.ReadableSubprocess): Promise<void> {
    const reader = proc.stdout.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let newlineIndex = buffer.indexOf("\n");
        while (newlineIndex !== -1) {
          const raw = buffer.slice(0, newlineIndex).trim();
          buffer = buffer.slice(newlineIndex + 1);
          if (raw.length > 0) {
            try {
              // SAFETY: `WatchLine` is a fixed struct emitted only by
              // `skill-studio watch --json` (apps/cli/src/main.rs); a
              // line that fails to parse is dropped rather than crashing
              // the reader, since a partial write can straddle a flush.
              const line = JSON.parse(raw) as WatchLine;
              onLine(line);
            } catch {
              // Dropped: see the SAFETY comment above.
            }
          }
          newlineIndex = buffer.indexOf("\n");
        }
      }
    } finally {
      reader.releaseLock();
    }
  }

  function scheduleRestart(): void {
    if (stopped) return;
    restartAttempt += 1;
    const delayMs = Math.min(
      RESTART_BASE_DELAY_MS * 2 ** (restartAttempt - 1),
      RESTART_MAX_DELAY_MS,
    );
    onStatus({ kind: "restarting", attempt: restartAttempt, delayMs });
    restartTimer = setTimeout(() => {
      restartTimer = null;
      spawnAndRead();
    }, delayMs);
  }

  function spawnAndRead(): void {
    if (stopped) return;
    const proc = Bun.spawn(command, { stdout: "pipe", stderr: "pipe", env: process.env });
    currentProc = proc;
    readLines(proc)
      .catch(() => {
        // A read failure is treated the same as an unexpected exit below.
      })
      .finally(() => {
        currentProc = null;
        if (!stopped) scheduleRestart();
      });
  }

  spawnAndRead();

  return {
    stop() {
      stopped = true;
      if (restartTimer !== null) clearTimeout(restartTimer);
      if (currentProc !== null) currentProc.kill();
    },
  };
}

/** The transport surface a screen depends on. Component tests inject a stub
 * that implements this interface instead of mocking the module (the
 * `anti-slop/no-module-mocking` lint rule forbids `vi.mock`), so `App` takes
 * one of these as a prop rather than importing `run`/`watch` directly. */
export interface Transport {
  run: <T>(op: CliOp, args: string[], config?: ScopeConfig) => Promise<Envelope<T>>;
  watch: (
    since: number | undefined,
    onLine: WatchLineHandler,
    onStatus: WatchStatusHandler,
    config?: ScopeConfig,
  ) => WatchHandle;
}

/** The real transport, backed by a child `skill-studio` process. */
export const cliTransport: Transport = { run, watch };
