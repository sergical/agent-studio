// ============================================================================
// Skill Studio - legacy-project-paths
// One-shot migration for a machine whose added/excluded project folders still
// sit in localStorage (`project-paths`/`excluded-project-paths`). App.tsx's
// startup effect imports them into ~/.agents/skill-studio.json and then clears
// both keys; the saved file is the only source afterwards.
// ============================================================================

const PROJECT_PATHS_STORAGE_KEY = "project-paths";
const EXCLUDED_PROJECT_PATHS_STORAGE_KEY = "excluded-project-paths";

function loadRawPathList(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function splitPathList(raw: string | null): string[] {
  return (raw ?? "").split("\n").filter(Boolean);
}

/** The localStorage project lists, or `null` if this machine never had either key. */
export function readLegacyProjectPaths(): { added: string[]; excluded: string[] } | null {
  const added = loadRawPathList(PROJECT_PATHS_STORAGE_KEY);
  const excluded = loadRawPathList(EXCLUDED_PROJECT_PATHS_STORAGE_KEY);
  if (added === null && excluded === null) return null;
  return { added: splitPathList(added), excluded: splitPathList(excluded) };
}

/** Drops the legacy keys once their contents are safely migrated into the backend. */
export function clearLegacyProjectPaths(): void {
  try {
    localStorage.removeItem(PROJECT_PATHS_STORAGE_KEY);
    localStorage.removeItem(EXCLUDED_PROJECT_PATHS_STORAGE_KEY);
  } catch {
    // Storage can be unavailable (quota, private mode); nothing to clean up then.
  }
}
