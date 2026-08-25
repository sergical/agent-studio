// ============================================================================
// Agent Studio - skill-api
// Tauri IPC communication for skills.sh integration
// ============================================================================

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AgentTarget,
  InstallRequest,
  InstallResult,
  InstalledSkill,
  PaginatedSkillsResponse,
  SkillSearchResult,
  SkillSnapshot,
} from "./skill-types";

// ============================================================================
// Search API
// ============================================================================

/**
 * Search for skills on skills.sh
 */
export async function searchSkills(
  query: string,
  limit?: number,
  offset?: number,
): Promise<PaginatedSkillsResponse> {
  return invoke("search_skills", { query, limit, offset });
}

/**
 * Get popular skills (sorted by install count)
 */
export async function getPopularSkills(
  limit?: number,
  offset?: number,
): Promise<PaginatedSkillsResponse> {
  return invoke("get_popular_skills", { limit, offset });
}

/**
 * Get skill details from skills.sh
 */
export async function getSkillDetails(skillId: string): Promise<SkillSearchResult> {
  return invoke("get_skill_details", { skillId });
}

// ============================================================================
// Installed Skills API
// ============================================================================

/**
 * Get all installed skills, merged from a directory scan of the four
 * first-class agents (Claude Code, Codex, OpenCode, pi) and the lock file.
 * Pass known project directories so project-scoped skills are found too.
 */
export async function getInstalledSkills(projectPaths?: string[]): Promise<InstalledSkill[]> {
  return invoke("get_installed_skills", { projectPaths });
}

/**
 * Check if a skill is installed
 */
export async function isSkillInstalled(skillName: string): Promise<boolean> {
  return invoke("is_skill_installed", { skillName });
}

/**
 * List project directories discovered from Codex config and Claude Code
 * transcripts that have a first-class agent's skill directory.
 */
export async function listSkillProjects(): Promise<string[]> {
  return invoke("list_skill_projects");
}

/**
 * Register project paths the caller cares about (e.g. one the user just
 * opened) so future background rebuilds always include them. Returns
 * immediately; listen for `onSkillSnapshot` to see the result.
 */
export async function registerSkillProjects(paths: string[]): Promise<void> {
  return invoke("register_skill_projects", { paths });
}

/**
 * Un-register a project path (e.g. one the user closed) so future
 * background rebuilds stop including it. Returns immediately; listen for
 * `onSkillSnapshot` to see the result.
 */
export async function unregisterSkillProject(path: string): Promise<void> {
  return invoke("unregister_skill_project", { path });
}

// ============================================================================
// Agent Targets API
// ============================================================================

/**
 * Get all supported agent targets
 */
export async function getAgentTargets(): Promise<AgentTarget[]> {
  return invoke("get_agent_targets");
}

// ============================================================================
// Installation API
// ============================================================================

/**
 * Install a skill using npx skills CLI
 */
export async function installSkill(request: InstallRequest): Promise<InstallResult> {
  return invoke("install_skill", { request });
}

/**
 * Remove a skill using npx skills CLI
 */
export async function removeSkill(skillName: string, global: boolean): Promise<InstallResult> {
  return invoke("remove_skill", { skillName, global });
}

/**
 * Update a skill using npx skills CLI
 */
export async function updateSkill(skillName: string, global: boolean): Promise<InstallResult> {
  return invoke("update_skill", { skillName, global });
}

/**
 * Read up to 2 MiB of an installed skill's SKILL.md straight off disk.
 */
export async function readInstalledSkillMd(path: string): Promise<string> {
  return invoke("read_installed_skill_md", { path });
}

/**
 * Overwrite an installed skill's `SKILL.md` with `content`, for the detail
 * drawer's inline editor. Refused when the file belongs to a plugin-managed
 * deployment or falls outside the current snapshot.
 */
export async function writeInstalledSkillMd(path: string, content: string): Promise<void> {
  return invoke("write_installed_skill_md", { path, content });
}

/**
 * Reveal a skill's folder in Finder, or open it in the user's default editor.
 */
export async function openSkillPath(path: string, mode: "reveal" | "editor"): Promise<void> {
  return invoke("open_skill_path", { path, mode });
}

// ============================================================================
// Background Refresh API
// ============================================================================

/**
 * Instant read of the background refresh thread's latest snapshot, or
 * `undefined` before the first snapshot has landed.
 */
export async function getSkillSnapshot(): Promise<SkillSnapshot | undefined> {
  return invoke("get_skill_snapshot");
}

/**
 * Ask the background refresh thread to rebuild the snapshot. Returns
 * immediately; listen for `onSkillSnapshot` to see the result.
 */
export async function requestSkillRescan(): Promise<void> {
  return invoke("request_skill_rescan");
}

/**
 * Subscribe to `skills://snapshot`, emitted every time the background
 * refresh thread (re)builds the snapshot. Returns an unlisten function.
 */
export function onSkillSnapshot(cb: (snapshot: SkillSnapshot) => void): () => void {
  let unlisten: (() => void) | undefined;
  let cancelled = false;

  listen<SkillSnapshot>("skills://snapshot", (event) => {
    cb(event.payload);
  }).then((fn) => {
    if (cancelled) {
      fn();
    } else {
      unlisten = fn;
    }
  });

  return () => {
    cancelled = true;
    unlisten?.();
  };
}
