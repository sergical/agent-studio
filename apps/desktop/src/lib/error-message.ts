/** Preserves Error messages and Tauri string rejections for display. */
export function errorMessage(cause: unknown, fallback = "Unknown error"): string {
  // oxlint-disable-next-line anti-slop/no-runtime-typeof -- Tauri IPC rejects with an untyped string; this helper is its display boundary.
  if (typeof cause === "string") return cause;
  return cause instanceof Error ? cause.message : fallback;
}
