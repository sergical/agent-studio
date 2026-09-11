// ============================================================================
// Skill Studio TUI - Status line
// Persistent footer: current revision, last refresh time, and any watch
// reconnect, per the TUI contract's screen D.
// ============================================================================

export type WatchStatus =
  | { kind: "connecting" }
  | { kind: "connected" }
  | { kind: "restarting"; attempt: number; delayMs: number };

export interface StatusLineProps {
  revision: number | null;
  lastRefreshedAt: Date | null;
  watchStatus: WatchStatus;
}

function watchStatusText(status: WatchStatus): string {
  switch (status.kind) {
    case "connecting":
      return "watch: connecting";
    case "connected":
      return "watch: connected";
    case "restarting":
      return `watch: reconnecting (attempt ${String(status.attempt)}, ${String(status.delayMs)}ms)`;
  }
}

export function StatusLine({ revision, lastRefreshedAt, watchStatus }: StatusLineProps) {
  const revisionText = revision === null ? "revision: -" : `revision: ${String(revision)}`;
  const refreshedText =
    lastRefreshedAt === null
      ? "refreshed: -"
      : `refreshed: ${lastRefreshedAt.toLocaleTimeString()}`;
  return (
    <box style={{ flexDirection: "row", justifyContent: "space-between" }}>
      <text>{`${revisionText}  ${refreshedText}`}</text>
      <text>{watchStatusText(watchStatus)}</text>
    </box>
  );
}
