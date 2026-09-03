// ============================================================================
// Skill Studio - skill-api
// Tauri IPC communication for skills.sh integration
// ============================================================================

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AddMethodDefaults,
  AddSkillOutcome,
  AddSkillRequest,
  AddSkillResult,
  AddSkillsRequest,
  AgentId,
  AgentTarget,
  ImportResult,
  InstallRequest,
  InstallResult,
  ForkRecord,
  GithubSkillListing,
  InstalledSkill,
  InstallScope,
  InvocationPolicy,
  PackInfo,
  PackMember,
  PaginatedSkillsResponse,
  PullResult,
  SkillDetails,
  SkillEvent,
  SkillsShAccessInfo,
  SkillSnapshot,
  UpdatePackResult,
} from "@skill-studio/lib";

// ============================================================================
// Search API
// ============================================================================

/**
 * Whether discovery goes straight to skills.sh with a developer-override key
 * (`mode: "direct"`) or through the local Skill Studio server (`mode:
 * "server"`, with its URL). Never returns the key itself.
 */
export async function getSkillsShAccess(): Promise<SkillsShAccessInfo> {
  return invoke("get_skills_sh_access");
}

/**
 * Save `key` as the skills.sh API key. Trimmed and rejected server-side if
 * empty.
 */
export async function setSkillsShApiKey(key: string): Promise<void> {
  return invoke("set_skills_sh_api_key", { key });
}

/**
 * Search for skills on skills.sh. The v1 search endpoint has no pagination -
 * it returns up to `limit` results in one shot.
 */
export async function searchSkills(
  query: string,
  limit?: number,
): Promise<PaginatedSkillsResponse> {
  return invoke("search_skills", { query, limit });
}

/**
 * Get popular skills (sorted by install count), `page` 0-indexed.
 */
export async function getPopularSkills(
  page?: number,
  perPage?: number,
): Promise<PaginatedSkillsResponse> {
  return invoke("get_popular_skills", { page, perPage });
}

/**
 * Get skill details, including the skill's SKILL.md/AGENTS.md body, from
 * skills.sh. `skillId` is the full `owner/repo/slug` id.
 */
export async function getSkillDetails(skillId: string): Promise<SkillDetails> {
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
 * Remove a skill using npx skills CLI. `projectPath` is `null` for a global
 * removal, or the project directory to remove from - validated on the Rust
 * side against the current snapshot and used as the CLI's working directory.
 */
export async function removeSkill(
  skillName: string,
  projectPath: string | null,
): Promise<InstallResult> {
  return invoke("remove_skill", { skillName, projectPath });
}

/**
 * Update a skill through whichever CLI owns it (dotagents or skills.sh).
 * `result.tool`/`result.command` say what actually ran.
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

/** One editor offered by the Settings picker - see the Rust `skill_editor`. */
export interface EditorOption {
  /** The macOS application name `open -a` takes, without `.app`. */
  app_name: string;
  label: string;
}

/** The known code editors actually installed on this machine. */
export async function listInstalledEditors(): Promise<EditorOption[]> {
  return invoke("list_installed_editors");
}

/** The app "Open in editor" uses, or `null` for the system default. */
export async function getPreferredEditor(): Promise<string | null> {
  return invoke("get_preferred_editor");
}

/** `null` restores the system default. An editor that is not installed is refused. */
export async function setPreferredEditor(appName: string | null): Promise<void> {
  return invoke("set_preferred_editor", { appName });
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
 * Installs every skill in `request.skills` from one source. The promise
 * rejects only when nothing could be attempted; a single skill's failure
 * comes back as that entry's `error`.
 */
export async function addSkills(request: AddSkillsRequest): Promise<AddSkillOutcome[]> {
  return invoke("add_skills", { request });
}

/**
 * Which skill folders a GitHub repo (or `path` within it) contains, so the
 * Add-skill sheet can install one skill or offer a picker. Results are
 * cached per repo and ref in the backend; `refresh` bypasses that cache.
 */
export async function listGithubSkills(
  repo: string,
  path?: string,
  gitRef?: string,
  refresh?: boolean,
): Promise<GithubSkillListing> {
  return invoke("list_github_skills", { repo, path, gitRef, refresh });
}

/**
 * Whether dotagents can run, whether skills.sh has been used before, and
 * which first-class agents are installed - fetched once when the Add Skill
 * sheet opens to pick its Method and Harnesses defaults.
 */
export async function getAddMethodDefaults(): Promise<AddMethodDefaults> {
  return invoke("get_add_method_defaults");
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
 * Enable or disable one deployment that has no native per-harness switch, by
 * renaming its directory into (or out of) a sibling `.skill-studio-disabled/`
 * holding directory in the same skills root - the universal fallback for
 * plain directory copies and project-scope symlinks. Refused for shared-root
 * and plugin-provided deployments.
 */
export async function setDeploymentEnabled(
  name: string,
  path: string,
  enabled: boolean,
): Promise<void> {
  return invoke("set_deployment_enabled", { name, path, enabled });
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
// Event Store API
// ============================================================================

/**
 * Lists events newest-first, for the Activity view's History section.
 * Defaults to the last 200 events across every skill.
 */
export async function listSkillEvents(limit?: number, skill?: string): Promise<SkillEvent[]> {
  return invoke("list_skill_events", { limit, skill });
}

/**
 * Undoes one event. Refused with a drift-guard message naming the drifted
 * path unless `force` is set, in which case the current (drifted) content is
 * itself backed up and restorable before the inverse is applied.
 */
export async function restoreSkillEvent(eventId: string, force: boolean): Promise<void> {
  return invoke("restore_skill_event", { eventId, force });
}

/**
 * The Locations card's entry point for disabling/enabling one skill under
 * one harness that reads from the shared root. Refuses when `harness`'s root
 * is still a whole-dir link to the shared folder - call
 * `materializeHarnessRoot` first (the Convert dialog).
 */
export async function setSharedHarnessSkillEnabled(
  rootPath: string,
  skill: string,
  harness: string,
  enabled: boolean,
): Promise<void> {
  return invoke("set_shared_harness_skill_enabled", { rootPath, skill, harness, enabled });
}

/**
 * Converts a harness's whole-dir link to the shared skills root into a real
 * directory of per-skill links, as an explicit, named action - the
 * Locations card's Convert dialog and Home's linked-root repair card. Recorded
 * in Activity and can be undone from there.
 */
export async function materializeHarnessRoot(harness: string, root: string): Promise<void> {
  return invoke("materialize_harness_root", { harness, root });
}

/**
 * SkillPage's "Repair this location" entry point for a broken deployment
 * symlink: `"remove"` deletes the dangling link, `"relink"` repoints it at
 * `target` (a healthy deployment path of the same skill). Both are validated
 * against the current snapshot on the Rust side.
 */
export async function repairSkillLink(
  path: string,
  action: "remove" | "relink",
  target?: string,
): Promise<void> {
  return invoke("repair_skill_link", { path, action, target });
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
