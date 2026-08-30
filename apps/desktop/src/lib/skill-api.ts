// ============================================================================
// Skill Studio - skill-api
// Tauri IPC communication for skills.sh integration
// ============================================================================

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AddSkillRequest,
  AddSkillResult,
  AgentId,
  AgentTarget,
  ImportResult,
  InstallRequest,
  InstallResult,
  ForkRecord,
  InstalledSkill,
  InstallScope,
  InvocationPolicy,
  PackInfo,
  PackMember,
  PaginatedSkillsResponse,
  PullResult,
  SkillSearchResult,
  SkillSnapshot,
  UpdateCheckSummary,
  UpdatePackResult,
} from "@skill-studio/lib";

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
 * Update a skill through whichever CLI owns it (dotagents or skills.sh).
 * `result.tool`/`result.command` say what actually ran.
 */
export async function updateSkill(skillName: string, global: boolean): Promise<InstallResult> {
  return invoke("update_skill", { skillName, global });
}

/**
 * Run the background update check now, blocking until it finishes. Also
 * triggers a snapshot rebuild, so `onSkillSnapshot` fires with fresh
 * `has_update`/`update_check` data shortly after this resolves.
 */
export async function checkSkillUpdatesNow(): Promise<UpdateCheckSummary> {
  return invoke("check_skill_updates_now");
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
 * Like `writeInstalledSkillMd`, but refuses (compare-and-swap) when the
 * file's current content doesn't match `expectedContent` - the copy the
 * caller last loaded. Used by the Audit proposal's Apply action so a save
 * made elsewhere while the proposal was open can't be silently clobbered.
 */
export async function writeInstalledSkillMdIfUnchanged(
  path: string,
  expectedContent: string,
  content: string,
): Promise<void> {
  return invoke("write_installed_skill_md_if_unchanged", { path, expectedContent, content });
}

/**
 * Reveal a skill's folder in Finder, or open it in the user's default editor.
 */
export async function openSkillPath(path: string, mode: "reveal" | "editor"): Promise<void> {
  return invoke("open_skill_path", { path, mode });
}

// ============================================================================
// Fork / Pull upstream / Un-fork API
// ============================================================================

/**
 * Detach a dotagents- or skills.sh-managed skill from its ledger so local
 * edits survive `sync`/`update`. `path` must be the shared-folder deployment
 * (`~/.agents/skills/<name>`, or the Claude Code symlink to it) - the
 * backend refuses anything else. Refused for a manual/plugin skill or a
 * dotagents wildcard entry.
 */
export async function forkSkill(name: string, path: string): Promise<ForkRecord> {
  return invoke("fork_skill", { name, path });
}

/**
 * Three-way merge a forked skill's snapshot against its current on-disk
 * copy and a freshly fetched upstream copy, then advance the snapshot to
 * the new upstream commit.
 */
export async function pullForkUpstream(name: string): Promise<PullResult> {
  return invoke("pull_fork_upstream", { name });
}

/**
 * Discard a forked skill's local edits and reinstall it from its recorded
 * origin. Callers should confirm with the user first - this runs immediately.
 */
export async function unforkSkill(name: string): Promise<void> {
  return invoke("unfork_skill", { name });
}

// ============================================================================
// Share Packs API
// ============================================================================

/** List every pack recorded in `~/.agents/skill-studio.json`. */
export async function listSkillPacks(): Promise<PackInfo[]> {
  return invoke("list_skill_packs");
}

/**
 * Build `~/.agents/packs/<name>` from `members` and commit it with git.
 * Refused if a pack of that name (or its directory) already exists.
 */
export async function createSkillPack(name: string, members: PackMember[]): Promise<PackInfo> {
  return invoke("create_skill_pack", { name, members });
}

/** Rebuild a pack's tree from its recorded skill list, committing only if it changed. */
export async function updateSkillPack(name: string): Promise<UpdatePackResult> {
  return invoke("update_skill_pack", { name });
}

/**
 * Push a pack to GitHub. Creates the repo (`gh repo create ... --push`) the
 * first time; pushes to the recorded `repo` on every call after that.
 * Callers must confirm with the user first - this runs immediately.
 */
export async function publishSkillPack(name: string, visibility: string): Promise<PackInfo> {
  return invoke("publish_skill_pack", { name, visibility });
}

/** Delete a pack locally: its registry entry and its directory. Never touches GitHub. */
export async function deleteSkillPack(name: string): Promise<void> {
  return invoke("delete_skill_pack", { name });
}

/**
 * Import a pack from a GitHub repo: `dotagents add <source> --all` for its
 * bundled skills, plus a per-row `dotagents add` for any `[[skills]]` entry
 * in its `agents.toml` that points elsewhere.
 */
export async function importSkillPack(source: string, agents: AgentId[]): Promise<ImportResult> {
  return invoke("import_skill_pack", { source, agents });
}

// ============================================================================
// Add Skill / Trials API
// ============================================================================

/**
 * Submit the Add-skill sheet: installs `request.source` via `request.method`,
 * applying the Claude Code shared-folder symlink rule for `dotagents`/`copy`.
 */
export async function addSkill(request: AddSkillRequest): Promise<AddSkillResult> {
  return invoke("add_skill", { request });
}

/**
 * Drop `name`'s trial record so the expiry loop leaves it alone.
 */
export async function keepSkillTrial(
  name: string,
  scope: InstallScope,
  projectPath?: string,
): Promise<void> {
  return invoke("keep_skill_trial", { name, scope, projectPath });
}

/**
 * Copy a trashed skill (from a `skills://trial-expired` event's
 * `trash_path`) back into `~/.agents/skills/<name>` as an untracked skill.
 */
export async function restoreTrashedSkill(trashPath: string): Promise<void> {
  return invoke("restore_trashed_skill", { trashPath });
}

/**
 * Subscribe to `skills://trial-expired`, emitted once per skill the trial
 * expiry loop just moved to `~/.agents/skills-trash`. Returns an unlisten
 * function.
 */
export function onTrialExpired(
  cb: (payload: { name: string; trash_path: string }) => void,
): () => void {
  let unlisten: (() => void) | undefined;
  let cancelled = false;

  listen<{ name: string; trash_path: string }>("skills://trial-expired", (event) => {
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

// ============================================================================
// Park / Per-harness disable / Invocation policy API
// ============================================================================

/**
 * Park (disable globally): moves the shared-folder deployment to
 * `~/.agents/skills-parked/<name>`, removing a per-skill Claude Code symlink
 * first if one exists. Refused when `name` isn't deployed to the shared
 * folder, or already parked.
 */
export async function parkSkill(name: string): Promise<void> {
  return invoke("park_skill", { name });
}

/**
 * Reverse `parkSkill`: moves the parked copy back to
 * `~/.agents/skills/<name>` and restores the Claude Code symlink if one was
 * removed. When reinstalling created a new shared-folder copy while parked,
 * reconciles by discarding the parked copy (if byte-identical) or trashing it
 * to `~/.agents/skills-trash`.
 */
export async function unparkSkill(name: string): Promise<void> {
  return invoke("unpark_skill", { name });
}

/**
 * Enable or disable one harness's own view of `name`, via that harness's own
 * mechanism (Codex `config.toml`, OpenCode `opencode.json`, or - for Claude
 * Code - removing/restoring its per-skill symlink). Refused for harnesses
 * with no per-skill disable (pi, Cursor, Grok Build) and for Claude Code when
 * the skill is deployed via the whole-directory symlink.
 */
export async function setHarnessEnabled(
  name: string,
  agent: string,
  enabled: boolean,
): Promise<void> {
  return invoke("set_harness_enabled", { name, agent, enabled });
}

/**
 * Rewrite `disable-model-invocation`/`user-invocable` in `path`'s SKILL.md
 * frontmatter to match `policy`, byte-identical otherwise. Also
 * writes/patches `agents/openai.yaml`'s `policy.allow_implicit_invocation`
 * when the skill has a Codex deployment.
 */
export async function setSkillInvocation(
  name: string,
  path: string,
  policy: InvocationPolicy,
): Promise<void> {
  return invoke("set_skill_invocation", { name, path, policy });
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
