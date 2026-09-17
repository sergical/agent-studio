// ============================================================================
// Skill Studio - skill-api
// Tauri IPC communication for skills.sh integration
// ============================================================================

import { invoke } from "@tauri-apps/api/core";
import type { InvokeArgs } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { recordIpcCall } from "./perf-marks";
import type {
  AddMethodDefaults,
  AddSkillOperationEvent,
  AddSkillRequest,
  AddSkillResult,
  AddSkillsRequest,
  AgentId,
  DiscoverySourceSetting,
  ImportResult,
  InstallResult,
  ForkRecord,
  FrontmatterRepairApplyMode,
  FrontmatterRepairPreview,
  GithubSkillListing,
  InstalledSkill,
  HarnessVisibilityTarget,
  LifecycleTarget,
  InvocationPolicy,
  PackImportPreflightResult,
  PackImportRequest,
  PaginatedSkillsResponse,
  ProjectFolder,
  PullResult,
  SkillDetails,
  SkillEvent,
  SkillSnapshot,
  TrackedProjects,
} from "@skill-studio/lib";

let ipcCallSeq = 0;

/** Every wrapper below routes through this instead of calling `invoke` directly, so every IPC
 * round trip gets one "ipc:<command>" `performance` measure - the overlay's source, and visible in
 * devtools' Performance panel too. The mark name carries a counter so concurrent calls to the same
 * command don't clobber each other's mark. */
function callCommand<T>(command: string, args?: InvokeArgs): Promise<T> {
  const startMark = `ipc:${command}:${ipcCallSeq++}`;
  performance.mark(startMark);
  const finish = (resolved: boolean) => {
    const measure = performance.measure(`ipc:${command}`, startMark);
    recordIpcCall(command, measure.duration, resolved);
    performance.clearMarks(startMark);
    performance.clearMeasures(`ipc:${command}`);
  };
  return invoke<T>(command, args).then(
    (result) => {
      finish(true);
      return result;
    },
    (cause: unknown) => {
      finish(false);
      throw cause;
    },
  );
}

/** Tauri rejects a failed command with the Rust `Result::Err` string directly, not an `Error` -
 * `err instanceof Error ? err.message : "Unknown error"` would discard it, so every catch block
 * that surfaces an invoke failure as a toast goes through this instead. */
export function invokeErrorMessage(cause: unknown): string {
  if (cause instanceof Error) return cause.message;
  if (cause == null) return "Unknown error";
  return `${cause}`;
}

export async function previewSkillFrontmatterRepair(
  target: LifecycleTarget,
): Promise<FrontmatterRepairPreview> {
  return callCommand("preview_skill_frontmatter_repair", { target });
}

export async function applySkillFrontmatterRepair(
  target: LifecycleTarget,
  preview: FrontmatterRepairPreview,
  mode: FrontmatterRepairApplyMode,
): Promise<void> {
  return callCommand("apply_skill_frontmatter_repair", {
    request: {
      target,
      proposal_id: preview.proposal_id,
      expected_content_fingerprint: preview.expected_content_fingerprint,
      mode,
    },
  });
}

// ============================================================================
// Search API
// ============================================================================

/**
 * Search for skills on skills.sh. The v1 search endpoint has no pagination -
 * it returns up to `limit` results in one shot.
 */
export async function searchSkills(
  query: string,
  limit?: number,
): Promise<PaginatedSkillsResponse> {
  return callCommand("search_skills", { query, limit });
}

/**
 * Get popular skills (sorted by install count), `page` 0-indexed.
 */
export async function getPopularSkills(
  page?: number,
  perPage?: number,
): Promise<PaginatedSkillsResponse> {
  return callCommand("get_popular_skills", { page, perPage });
}

/**
 * Get skill details, including the skill's SKILL.md/AGENTS.md body, from
 * skills.sh. `skillId` is the full `owner/repo/slug` id.
 */
export async function getSkillDetails(skillId: string): Promise<SkillDetails> {
  return callCommand("get_skill_details", { skillId });
}

// ============================================================================
// Installed Skills API
// ============================================================================

/**
 * Get all installed skills, merged from a directory scan of the four
 * first-class agents (Claude Code, Codex, OpenCode, pi) and the lock file,
 * over the tracked project list already saved in
 * `~/.agents/skill-studio.json` - the same list `getTrackedProjects` reads.
 */
export async function getInstalledSkills(): Promise<InstalledSkill[]> {
  return callCommand("get_installed_skills");
}

/**
 * The saved project list from `~/.agents/skill-studio.json`'s `projects`
 * key - the same list the CLI and the MCP server scan against.
 */
export async function getTrackedProjects(): Promise<TrackedProjects> {
  return callCommand("get_tracked_projects");
}

/**
 * Add project paths the caller cares about (e.g. one the user just opened)
 * to the saved list, un-excluding any of them that were previously stopped.
 * Returns the updated list, already persisted; listen for `onSkillSnapshot`
 * to see the rebuilt skill scan that follows.
 */
export async function registerSkillProjects(paths: string[]): Promise<TrackedProjects> {
  return callCommand("register_skill_projects", { paths });
}

/**
 * Move a project path (e.g. one the user "Stop tracking"-ed) from added to
 * excluded in the saved list, so future scans skip it even if discovery
 * would otherwise find it again. Returns the updated list, already
 * persisted; listen for `onSkillSnapshot` to see the rebuilt skill scan
 * that follows.
 */
export async function unregisterSkillProject(path: string): Promise<TrackedProjects> {
  return callCommand("unregister_skill_project", { path });
}

/**
 * Remove a folder the user added by hand from the saved list, recording no
 * exclusion - unlike `unregisterSkillProject`, discovery can offer the
 * folder again later. Returns the updated list, already persisted; listen
 * for `onSkillSnapshot` to see the rebuilt skill scan that follows.
 */
export async function removeSkillProject(path: string): Promise<TrackedProjects> {
  return callCommand("remove_skill_project", { path });
}

/**
 * The saved per-harness discovery switches, in display order - see the
 * Settings "Project folders" card.
 */
export async function getDiscoverySources(): Promise<DiscoverySourceSetting[]> {
  return callCommand("get_discovery_sources");
}

/**
 * Switch one discovery harness's history search on or off. Returns the
 * updated switches, already persisted; listen for `onSkillSnapshot` to see
 * the rebuilt skill scan that follows.
 */
export async function setDiscoverySource(
  harness: string,
  enabled: boolean,
): Promise<DiscoverySourceSetting[]> {
  return callCommand("set_discovery_source", { harness, enabled });
}

/**
 * Every project folder discovery found or the user added by hand, labelled
 * by source, for the Settings "Project folders" card. A harness-history
 * scan runs on every call, so this is not cheap - callers should refetch on
 * a meaningful change, not on every render.
 */
export async function listProjectFolders(): Promise<ProjectFolder[]> {
  return callCommand("list_project_folders");
}

/**
 * One-shot migration of the desktop's old localStorage project lists into
 * the saved `~/.agents/skill-studio.json` list: `added` is registered,
 * `excluded` is un-registered. Callers should clear the localStorage
 * entries only after this resolves.
 */
export async function importTrackedProjects(
  added: string[],
  excluded: string[],
): Promise<TrackedProjects> {
  return callCommand("import_tracked_projects", { added, excluded });
}

/**
 * Remove a skill using npx skills CLI. `projectPath` is `null` for a global
 * removal, or the project directory to remove from - validated on the Rust
 * side against the current snapshot and used as the CLI's working directory.
 */
export async function removeSkill(target: LifecycleTarget): Promise<InstallResult> {
  return callCommand("remove_skill", { target });
}

/**
 * Update a skill through whichever CLI owns it (dotagents or skills.sh).
 * `result.tool`/`result.command` say what actually ran.
 */
export async function updateSkill(target: LifecycleTarget): Promise<InstallResult> {
  return callCommand("update_skill", { target });
}

/**
 * Read up to 2 MiB of an installed skill's SKILL.md straight off disk.
 */
export async function readInstalledSkillMd(path: string): Promise<string> {
  return callCommand("read_installed_skill_md", { path });
}

/**
 * Overwrites an installed skill's `SKILL.md` only when its current content
 * matches `expectedContent`. Audit proposal Apply and the inline editor use
 * this compare-and-swap so a save made elsewhere can't be silently clobbered.
 */
export async function writeInstalledSkillMdIfUnchanged(
  path: string,
  expectedContent: string,
  content: string,
): Promise<void> {
  return callCommand("write_installed_skill_md_if_unchanged", { path, expectedContent, content });
}

/**
 * Reveal a skill's folder in Finder, or open it in the user's default editor.
 */
export async function openSkillPath(path: string, mode: "reveal" | "editor"): Promise<void> {
  return callCommand("open_skill_path", { path, mode });
}

/** One editor offered by the Settings card - see the Rust `skill_editor`. */
export interface EditorOption {
  /** The value to save: a macOS application name, an absolute `.app` path, or `"$EDITOR"`. */
  app_name: string;
  label: string;
}

/** Everything the Settings "Open in editor" card shows - see the Rust `skill_editor::EditorChoices`. */
export interface EditorChoices {
  automatic_label: string;
  apps: EditorOption[];
  terminal: EditorOption | null;
  selected: string | null;
}

/** The editor card's state: installed/saved apps, the `$EDITOR` row, and the current choice. */
export async function getEditorChoices(): Promise<EditorChoices> {
  return callCommand("get_editor_choices");
}

/** `null` restores the system default. A value that isn't usable is refused. */
export async function setPreferredEditor(value: string | null): Promise<void> {
  return callCommand("set_preferred_editor", { appName: value });
}

// ============================================================================
// Fork / Pull upstream / Un-fork API
// ============================================================================

/**
 * Detach a dotagents- or skills.sh-managed skill from its ledger so local
 * edits survive `sync`/`update`. The target must resolve to the Universal
 * deployment or its Claude Code link. Refused for a manual/plugin skill or a
 * dotagents wildcard entry.
 */
export async function forkSkill(target: LifecycleTarget): Promise<ForkRecord> {
  return callCommand("fork_skill", { target });
}

/**
 * Three-way merge a forked skill's snapshot against its current on-disk
 * copy and a freshly fetched upstream copy, then advance the snapshot to
 * the new upstream commit.
 */
export async function pullForkUpstream(target: LifecycleTarget): Promise<PullResult> {
  return callCommand("pull_fork_upstream", { target });
}

/**
 * Discard a forked skill's local edits and reinstall it from its recorded
 * origin. Callers should confirm with the user first - this runs immediately.
 */
export async function unforkSkill(target: LifecycleTarget): Promise<void> {
  return callCommand("unfork_skill", { target });
}

// ============================================================================
// Share Packs API
// ============================================================================

/**
 * Preflight and import a pack from a GitHub repo or local folder. Remote
 * repository identities pause for explicit trust before any install.
 */
export async function importSkillPack(
  request: PackImportRequest,
): Promise<PackImportPreflightResult> {
  return callCommand("import_skill_pack", { request });
}

/** Confirm the exact repository list returned by pack import preflight. */
export async function confirmSkillPackTrust(
  confirmationToken: string,
  request: PackImportRequest,
): Promise<ImportResult> {
  return callCommand("confirm_skill_pack_trust", { confirmationToken, request });
}

/** Consume a pending pack trust prompt and remove its unchanged local snapshot. */
export async function abandonPackImportTrust(confirmationToken: string): Promise<boolean> {
  return callCommand("abandon_pack_import_trust", { confirmationToken });
}

// ============================================================================
// Add Skill / Trials API
// ============================================================================

/**
 * Submit the Add-skill sheet: installs `request.source` via `request.method`,
 * applying the Claude Code shared-folder symlink rule for `dotagents`/`copy`.
 * Kept for Skill Store / repair callers; the Add-skill sheet uses operations.
 */
export async function addSkill(request: AddSkillRequest): Promise<AddSkillResult> {
  return callCommand("add_skill", { request });
}

/** Event name every background Add Skill status is emitted on. */
export const ADD_SKILL_OPERATION_EVENT = "skills://add-skill-operation";

/**
 * Schedule a single-skill add. Returns the queued event before `npx` or
 * network work. Generate `operationId` and subscribe before calling.
 */
export async function startAddSkillOperation(
  operationId: string,
  request: AddSkillRequest,
): Promise<AddSkillOperationEvent> {
  return callCommand("start_add_skill_operation", { operationId, request });
}

/**
 * Schedule a batch add. Returns the queued event immediately.
 */
export async function startAddSkillsOperation(
  operationId: string,
  request: AddSkillsRequest,
): Promise<AddSkillOperationEvent> {
  return callCommand("start_add_skills_operation", { operationId, request });
}

/** Catch-up read after subscribe or remount. */
export async function getAddSkillOperation(operationId: string): Promise<AddSkillOperationEvent> {
  return callCommand("get_add_skill_operation", { operationId });
}

/** Request cancel. The worker still reports completed if mutation finished. */
export async function cancelAddSkillOperation(
  operationId: string,
): Promise<AddSkillOperationEvent> {
  return callCommand("cancel_add_skill_operation", { operationId });
}

/**
 * Trust this operation's repository identity and retry the same request.
 * Rejects a mismatched or replayed confirmation.
 */
export async function confirmAddSkillTrust(
  operationId: string,
  retryOperationId: string,
  identity: string,
): Promise<AddSkillOperationEvent> {
  return callCommand("confirm_add_skill_trust", { operationId, retryOperationId, identity });
}

/** Subscribe to background Add Skill status events. */
export function onAddSkillOperation(
  cb: (event: AddSkillOperationEvent) => void,
): Promise<() => void> {
  return listen<AddSkillOperationEvent>(ADD_SKILL_OPERATION_EVENT, (event) => {
    cb(event.payload);
  });
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
  return callCommand("list_github_skills", { repo, path, gitRef, refresh });
}

/**
 * Whether dotagents can run, whether skills.sh has been used before, and
 * which first-class agents are installed - fetched once when the Add Skill
 * sheet opens to pick its Method and Harnesses defaults.
 */
export async function getAddMethodDefaults(): Promise<AddMethodDefaults> {
  return callCommand("get_add_method_defaults");
}

/**
 * Drop the selected deployment's trial record so the expiry loop leaves it alone.
 */
export async function keepSkillTrial(target: LifecycleTarget): Promise<void> {
  return callCommand("keep_skill_trial", { target });
}

/**
 * Copy a trashed skill (from a `skills://trial-expired` event's
 * `trash_path`) back into `~/.agents/skills/<name>` as an untracked skill.
 */
export async function restoreTrashedSkill(trashPath: string): Promise<void> {
  return callCommand("restore_trashed_skill", { trashPath });
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
 * Park one Global Universal deployment: moves that folder to
 * `~/.agents/skills-parked/<name>`. Project and Per harness copies stay
 * independent. Refused when the target is not a Global Universal folder.
 */
export async function parkSkill(target: LifecycleTarget): Promise<void> {
  return callCommand("park_skill", { target });
}

/**
 * Reverse `parkSkill` for the selected parked or Global Universal target.
 * Project copies are not unparked as a side effect.
 */
export async function unparkSkill(target: LifecycleTarget): Promise<void> {
  return callCommand("unpark_skill", { target });
}

/**
 * Enable or disable one harness's own view of the selected deployment, via that harness's own
 * mechanism (Codex `config.toml`, OpenCode `opencode.json`, or - for Claude
 * Code - removing/restoring its per-skill symlink). Refused for harnesses
 * with no per-skill disable (pi, Cursor, Grok Build) and for Claude Code when
 * the skill is deployed via the whole-directory symlink.
 */
export async function setHarnessEnabled(
  target: LifecycleTarget,
  agent: AgentId,
  enabled: boolean,
): Promise<void> {
  if (!target.deployment_id) {
    throw new Error("Harness visibility needs one exact deployment");
  }
  const visibilityTarget: HarnessVisibilityTarget = {
    deployment_id: target.deployment_id,
    reader_agent: agent,
  };
  return callCommand("set_harness_enabled", { target: visibilityTarget, enabled });
}

/**
 * Enable or disable one deployment that has no native per-harness switch, by
 * renaming its directory into (or out of) a sibling `.skill-studio-disabled/`
 * holding directory in the same skills root - the universal fallback for
 * plain directory copies and project-scope symlinks. Refused for shared-root
 * and plugin-provided deployments.
 */
export async function setDeploymentEnabled(
  target: LifecycleTarget,
  enabled: boolean,
): Promise<void> {
  return callCommand("set_deployment_enabled", { target, enabled });
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
  return callCommand("set_skill_invocation", { name, path, policy });
}

/**
 * Enable or disable a Claude Code plugin (`claude plugin enable|disable
 * <id> -s user`), which moves every skill the plugin ships together.
 * Refused for any other harness.
 */
export async function setPluginEnabled(
  pluginId: string,
  harness: string,
  enabled: boolean,
): Promise<void> {
  return callCommand("set_plugin_enabled", { pluginId, harness, enabled });
}

/**
 * Uninstall a Claude Code plugin (`claude plugin uninstall <id> -s user -y`),
 * removing every skill it ships. Refused for any other harness.
 */
export async function uninstallPlugin(pluginId: string, harness: string): Promise<void> {
  return callCommand("uninstall_plugin", { pluginId, harness });
}

// ============================================================================
// Event Store API
// ============================================================================

/**
 * Lists events newest-first, for the Activity view's History section.
 * Defaults to the last 200 events across every skill.
 */
export async function listSkillEvents(limit?: number, skill?: string): Promise<SkillEvent[]> {
  return callCommand("list_skill_events", { limit, skill });
}

/**
 * Undoes one event. Refused with a drift-guard message naming the drifted
 * path unless `force` is set, in which case the current (drifted) content is
 * itself backed up and restorable before the inverse is applied.
 */
export async function restoreSkillEvent(eventId: string, force: boolean): Promise<void> {
  return callCommand("restore_skill_event", { eventId, force });
}

/**
 * Converts a harness's whole-dir link to the shared skills root into a real
 * directory of per-skill links, as an explicit, named action - the
 * Locations card's Convert dialog and Home's linked-root repair card. Recorded
 * in Activity and can be undone from there.
 */
export async function materializeHarnessRoot(
  target: LifecycleTarget,
  harness: string,
  root: string,
): Promise<void> {
  return callCommand("materialize_harness_root", { target, harness, root });
}

/** Converts a whole harness root and disables the selected deployment under one durable intent. */
export async function materializeHarnessRootThenDisable(
  target: LifecycleTarget,
  harness: string,
  root: string,
): Promise<void> {
  return callCommand("materialize_harness_root_then_disable", { target, harness, root });
}

/** Replaces one healthy Universal-backed deployment link with a local Copy directory. */
export async function makeSkillIndependentCopy(target: LifecycleTarget): Promise<void> {
  return callCommand("make_skill_independent_copy", { target });
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
  return callCommand("repair_skill_link", { path, action, target });
}

// ============================================================================
// Background Refresh API
// ============================================================================

/**
 * Instant read of the background refresh thread's latest snapshot, or
 * `undefined` before the first snapshot has landed.
 */
export async function getSkillSnapshot(): Promise<SkillSnapshot | undefined> {
  return callCommand("get_skill_snapshot");
}

/**
 * Ask the background refresh thread to rebuild the snapshot. Returns
 * immediately; listen for `onSkillSnapshot` to see the result.
 */
export async function requestSkillRescan(): Promise<void> {
  return callCommand("request_skill_rescan");
}

/**
 * Subscribe to `skills://snapshot`, emitted every time the background
 * refresh thread (re)builds the snapshot. Returns an unlisten function.
 */
export function onSkillSnapshot(cb: (snapshot: SkillSnapshot) => void): Promise<() => void> {
  return listen<SkillSnapshot>("skills://snapshot", (event) => {
    cb(event.payload);
  });
}
