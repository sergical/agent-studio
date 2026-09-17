// ============================================================================
// Skill Studio - perf-marks
// In-memory log of IPC round trips and commit-to-paint timing, fed by
// skill-api.ts's callCommand wrapper. Dev-only consumer: components/dev/PerfOverlay.tsx.
// ============================================================================

/** One IPC call's timing. `paintMs` starts `null` and fills in after the next
 * animation frame following the IPC resolve/reject. */
export interface PerfEntry {
  id: number;
  command: string;
  ipcMs: number;
  paintMs: number | null;
}

const MAX_ENTRIES = 50;

let nextId = 0;
let entries: PerfEntry[] = [];
const listeners = new Set<(entries: PerfEntry[]) => void>();

function notify(): void {
  for (const listener of listeners) listener(entries);
}

export function subscribe(listener: (entries: PerfEntry[]) => void): () => void {
  listeners.add(listener);
  listener(entries);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * Records one IPC round trip and, for a resolved call, schedules the paint
 * measure: a mark right now, then one `requestAnimationFrame` later a
 * measure from that mark to the frame Chrome/WebKit next paints. A rejected
 * call keeps `paintMs` `null` - there's no successful render to time.
 */
export function recordIpcCall(command: string, ipcMs: number, resolved = true): void {
  const entry: PerfEntry = { id: nextId++, command, ipcMs, paintMs: null };
  entries = entries.length >= MAX_ENTRIES ? [...entries.slice(1), entry] : [...entries, entry];
  notify();
  if (!resolved) return;

  // The mark name carries the entry id so two calls of the same command that both resolve
  // before the next frame don't clear or measure each other's mark.
  const resolveMark = `paint:${command}:${entry.id}:resolve`;
  const measureName = `paint:${command}:${entry.id}`;
  performance.mark(resolveMark);
  requestAnimationFrame(() => {
    const measure = performance.measure(measureName, resolveMark);
    entry.paintMs = measure.duration;
    performance.clearMarks(resolveMark);
    performance.clearMeasures(measureName);
    notify();
  });
}
