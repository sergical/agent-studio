// ============================================================================
// PerfOverlay - Dev-only HUD over the last 20 IPC round trips, each with its
// commit-to-paint time. Rendered from main.tsx only when
// `import.meta.env.DEV` and the URL has `?perf=1` - see perf-marks.ts, which
// skill-api.ts's callCommand wrapper feeds.
// ============================================================================

import { useEffect, useState } from "react";
import { Button } from "@skill-studio/ui";
import { subscribe } from "../../lib/perf-marks";
import type { PerfEntry } from "../../lib/perf-marks";

/** How many of the most recent calls the overlay shows, newest first. */
const VISIBLE_COUNT = 20;
/** A row over this many milliseconds - IPC or paint - is flagged, roughly one 60Hz frame. */
const SLOW_MS = 16;

function formatMs(value: number | null): string {
  return value === null ? "…" : `${value.toFixed(1)}ms`;
}

export function PerfOverlay() {
  const [entries, setEntries] = useState<PerfEntry[]>([]);
  const [closed, setClosed] = useState(false);

  useEffect(() => subscribe(setEntries), []);

  if (closed) return null;

  const visible = entries.slice(-VISIBLE_COUNT).reverse();

  return (
    <div
      className="fixed right-3 bottom-3 z-50 w-80 select-none rounded-lg border shadow-lg"
      style={{
        borderColor: "var(--color-border)",
        backgroundColor: "var(--color-bg-elevated)",
        color: "var(--color-text-secondary)",
        fontFamily: "var(--font-mono)",
        fontSize: 11,
      }}
    >
      <div
        className="flex items-center justify-between gap-2 border-b px-2 py-1"
        style={{ borderColor: "var(--color-border-subtle)" }}
      >
        <span style={{ color: "var(--color-text-primary)" }}>perf</span>
        <Button variant="ghost" size="xs" onClick={() => setClosed(true)}>
          Close
        </Button>
      </div>
      <ul className="max-h-64 overflow-y-auto">
        {visible.length === 0 ? (
          <li className="px-2 py-1.5" style={{ color: "var(--color-text-tertiary)" }}>
            No IPC calls yet
          </li>
        ) : (
          visible.map((entry) => {
            const slow = entry.ipcMs > SLOW_MS || (entry.paintMs ?? 0) > SLOW_MS;
            return (
              <li
                key={entry.id}
                className="flex items-center justify-between gap-2 px-2 py-1"
                style={{ color: slow ? "var(--color-warning)" : undefined }}
              >
                <span className="truncate">{entry.command}</span>
                <span className="shrink-0 tabular-nums">
                  ipc {formatMs(entry.ipcMs)} · paint {formatMs(entry.paintMs)}
                </span>
              </li>
            );
          })
        )}
      </ul>
    </div>
  );
}
