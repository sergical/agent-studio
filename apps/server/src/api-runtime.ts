// ============================================================================
// Skill Studio - Node API lifetime
// ============================================================================

import { createServer } from "node:http";
import { getRequestListener } from "@hono/node-server";
import * as Sentry from "@sentry/hono/node";

type RequestHandler = Parameters<typeof getRequestListener>[0];

async function settleWithin(work: Promise<unknown>, milliseconds: number): Promise<boolean> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      work.then(() => true),
      new Promise<boolean>((resolve) => {
        timer = setTimeout(() => resolve(false), milliseconds);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

export function createApiRuntime(bind: (signal: AbortSignal) => RequestHandler) {
  const cancellation = new AbortController();
  const handler = bind(cancellation.signal);
  const pending = new Set<Promise<unknown>>();
  const server = createServer(
    getRequestListener((request, env) => {
      const response = Promise.resolve().then(() => handler(request, env));
      pending.add(response);
      void response.then(
        () => pending.delete(response),
        () => pending.delete(response),
      );
      return response;
    }),
  );
  let shutdown: Promise<boolean> | undefined;
  return {
    server,
    shutdown(): Promise<boolean> {
      if (shutdown) return shutdown;
      shutdown = (async () => {
        const closed = new Promise<void>((resolve) => server.close(() => resolve()));
        await settleWithin(closed, 2000);
        cancellation.abort();
        server.closeAllConnections();
        const settled = await settleWithin(Promise.allSettled(pending), 2000);
        const flushed = await Sentry.close(2000);
        return settled && (flushed || !Sentry.getClient());
      })();
      return shutdown;
    },
  };
}

export function handleTerminationSignals(runtime: ReturnType<typeof createApiRuntime>): void {
  let stopping = false;
  const terminate = () => {
    if (stopping) return;
    stopping = true;
    const deadline = setTimeout(() => process.exit(1), 6500);
    void runtime.shutdown().then(
      (complete) => {
        clearTimeout(deadline);
        process.exit(complete ? 0 : 1);
      },
      () => {
        clearTimeout(deadline);
        process.exit(1);
      },
    );
  };
  process.on("SIGINT", terminate);
  process.on("SIGTERM", terminate);
}
