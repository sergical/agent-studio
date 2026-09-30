// ============================================================================
// Skill Studio - Update-all progress event
// Mirrors `UpdateAllProgress` in apps/desktop/src-tauri/src/skills/commands.rs,
// emitted on `skills://update-all-progress`.
// ============================================================================

/** One finished target (updated or failed) of an "Update all" batch. */
export interface UpdateAllProgress {
  /** Targets finished so far, including refused ones. */
  done: number;
  /** Targets in the batch. */
  total: number;
  skill_name: string;
}
