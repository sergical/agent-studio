// ============================================================================
// Skill Studio - Worker tests
// Covers `sentryOptionsFor`: tracing must stay off (no sample rate, no
// sampler, transactions dropped, no trace headers on outgoing requests) so a
// caller can never force a transaction that skips `beforeSend`'s scrub.
// ============================================================================

import { describe, expect, it } from "vitest";

import { sentryOptionsFor } from "./worker";
import { scrubSentryEvent } from "./sentry-event-scrub";

import type { Env } from "./worker";

describe("sentryOptionsFor", () => {
  it("wires beforeSend to the shared scrub function", () => {
    // SAFETY: only `SENTRY_DSN` matters to this factory; the other `Env` fields are unused here.
    const options = sentryOptionsFor({
      SENTRY_DSN: "https://examplePublicKey@o0.ingest.sentry.io/0",
    } as Env);

    expect(options.beforeSend).toBe(scrubSentryEvent);
  });

  it("drops every transaction, since a transaction never reaches beforeSend", () => {
    // SAFETY: see above.
    const options = sentryOptionsFor({
      SENTRY_DSN: "https://examplePublicKey@o0.ingest.sentry.io/0",
    } as Env);
    // SAFETY: `beforeSendTransaction` ignores both arguments and always returns null; their shape doesn't matter here.
    const result = options.beforeSendTransaction?.({} as never, {} as never);

    expect(result).toBeNull();
  });

  it("attaches no trace headers to outgoing requests, keeping sentry-trace off skills.sh calls", () => {
    // SAFETY: see above.
    const options = sentryOptionsFor({
      SENTRY_DSN: "https://examplePublicKey@o0.ingest.sentry.io/0",
    } as Env);

    expect(options.tracePropagationTargets).toEqual([]);
  });

  it("sets no sample rate or sampler, so tracing never turns on", () => {
    // SAFETY: see above.
    const options = sentryOptionsFor({
      SENTRY_DSN: "https://examplePublicKey@o0.ingest.sentry.io/0",
    } as Env);

    expect("tracesSampleRate" in options).toBe(false);
    expect("tracesSampler" in options).toBe(false);
  });
});
