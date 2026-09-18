// ============================================================================
// Skill Studio - mock-tauri
// Installs a `mockIPC` handler that answers every `invoke` the desktop
// frontend makes from an in-memory `SkillSnapshot`, so the app runs in a
// plain browser tab with no Tauri runtime behind it. Generalized from the
// single-skill mock `packages/marketing/capture/main.tsx` used to hand-roll:
// every handler here acts on whichever skill/deployment the payload names.
// ============================================================================

import type { InvokeArgs } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { z } from "zod";
import { isProjectPattern } from "@skill-studio/lib";
import type {
  AddSkillOperationEvent,
  CommandHealth,
  Deployment,
  DiscoverySourceSetting,
  InstalledSkill,
  ProjectFolder,
  RemoveOutcome,
  SkillSnapshot,
  TrackedProjects,
} from "@skill-studio/lib";
import type { EditorChoices, EditorOption } from "../../lib/skill-api";
import {
  deployment,
  fallbackSkillContent,
  HARNESS_HOME,
  HARNESS_PROJECT,
  harnessSkillContent,
  skill,
} from "./skill-fixture";

/** What the harness exposes on `window.__harness` for an agent driving the app. */
export interface HarnessControl {
  snapshot(): SkillSnapshot;
  /** Bumps `revision` and emits `skills://snapshot`, exactly like a real background rebuild. */
  publish(next: SkillSnapshot): Promise<void>;
  /** Publishes `snapshot` with one skill replaced by `transform`'s result. */
  updateSkill(name: string, transform: (skill: InstalledSkill) => InstalledSkill): Promise<void>;
}

declare global {
  interface Window {
    __harness?: HarnessControl;
  }
}

/** The `AddSkillRequest`/`AddSkillsRequest` fields the harness's `add_skill` family of handlers
 * actually reads - narrower than the real DTOs so `z.parse` doesn't have to reproduce every
 * field the Add Skill sheet sends. */
const addRequestSchema = z
  .object({
    scope: z.enum(["global", "project"]),
    project_path: z.string().nullable().optional(),
    trial: z.boolean(),
    method: z.enum(["dotagents", "skills-sh", "copy"]),
    source: z
      .object({
        skillName: z.string().nullable(),
        repo: z.string().nullable(),
        url: z.string().nullable(),
      })
      .passthrough(),
  })
  .passthrough();
type AddRequestLike = z.infer<typeof addRequestSchema>;

type ObjectInvokeArgs = Exclude<InvokeArgs, number[] | ArrayBuffer | Uint8Array>;

function isObjectInvokeArgs(payload: InvokeArgs | undefined): payload is ObjectInvokeArgs {
  return (
    payload !== undefined &&
    !Array.isArray(payload) &&
    !(payload instanceof ArrayBuffer) &&
    !(payload instanceof Uint8Array)
  );
}

/** Seeded into `trackedProjects.added` below: a folder added by hand that no longer exists, so the
 * Settings "Project folders" card always has a "Folder not found" row to show. */
const ARCHIVED_CLIENT_PROJECT = `${HARNESS_HOME}/src/archived-client`;

/** The six discovery harnesses, in the display order `discovery_harnesses()` yields on the Rust
 * side - see `crates/skill-studio-host/src/discovery.rs`'s `HISTORY_SOURCES`. */
const DISCOVERY_HARNESSES = ["claude-code", "codex", "open-code", "pi", "cursor", "grok-build"];

/** Installs the mock Tauri IPC layer and returns the control the harness (or the marketing
 * capture page) drives it with. */
export function installMockTauri(initial: SkillSnapshot): HarnessControl {
  let currentSnapshot = initial;
  const addOperations = new Map<string, AddSkillOperationEvent>();
  // Mirrors the backend's `~/.agents/skill-studio.json` `projects` key - the register/unregister/
  // import handlers below mutate it with the same track/untrack rules as `TrackedProjects` in
  // `crates/skill-studio-core`, and `get_tracked_projects` reads it back.
  let trackedProjects: TrackedProjects = { added: [ARCHIVED_CLIENT_PROJECT], excluded: [] };
  const discoverySources = new Map<string, boolean>(
    DISCOVERY_HARNESSES.map((harness) => [harness, true]),
  );
  // Mirrors `preferred_editor` in `~/.agents/skill-studio.json` - an app name, a `.app` path,
  // or "$EDITOR", or `null` for the system default. See `skill_editor::EditorChoices`.
  let editorPreference: string | null = null;
  const KNOWN_EDITOR_APPS: EditorOption[] = [
    { app_name: "Cursor", label: "Cursor" },
    { app_name: "Visual Studio Code", label: "Visual Studio Code" },
    { app_name: "IntelliJ IDEA Ultimate Edition", label: "IntelliJ IDEA Ultimate Edition" },
  ];

  // Settings' "Command health" card - see `crates/skill-studio-core/src/health.rs`.
  const COMMAND_HEALTH_ROWS: CommandHealth[] = [
    { command: "scan", count: 42, failures: 0, p50_ms: 18, p95_ms: 64, last_error: null },
    {
      command: "add_skill",
      count: 9,
      failures: 1,
      p50_ms: 210,
      p95_ms: 890,
      last_error: "disk full",
    },
    { command: "list_events", count: 130, failures: 0, p50_ms: 4, p95_ms: 11, last_error: null },
  ];

  function editorChoices(): EditorChoices {
    const apps = [...KNOWN_EDITOR_APPS];
    if (
      editorPreference?.endsWith(".app") &&
      !apps.some((app) => app.app_name === editorPreference)
    ) {
      const fileName = editorPreference.split("/").pop() ?? editorPreference;
      apps.push({ app_name: editorPreference, label: fileName.replace(/\.app$/, "") });
    }
    return {
      automatic_label: "Cursor (first found)",
      apps,
      terminal: { app_name: "$EDITOR", label: "nvim" },
      selected: editorPreference,
    };
  }

  function snapshotTrackedProjects(): TrackedProjects {
    return { added: [...trackedProjects.added], excluded: [...trackedProjects.excluded] };
  }

  /** Appends each path to `added` if not already there; removes it from `excluded`. */
  function trackProjects(paths: string[]): void {
    const excludedSet = new Set(trackedProjects.excluded);
    paths.forEach((path) => excludedSet.delete(path));
    trackedProjects = {
      added: [...new Set([...trackedProjects.added, ...paths])],
      excluded: [...excludedSet],
    };
  }

  /** Removes the path from `added`; appends it to `excluded` if not already there. */
  function untrackProject(path: string): void {
    trackedProjects = {
      added: trackedProjects.added.filter((p) => p !== path),
      excluded: [...new Set([...trackedProjects.excluded, path])],
    };
  }

  /** Removes the path from `added` only, recording no exclusion - mirrors `TrackedProjects::forget`. */
  function forgetProject(path: string): void {
    trackedProjects = {
      ...trackedProjects,
      added: trackedProjects.added.filter((p) => p !== path),
    };
  }

  function discoverySourceSettings(): DiscoverySourceSetting[] {
    return DISCOVERY_HARNESSES.map((harness) => ({
      harness,
      enabled: discoverySources.get(harness) ?? false,
    }));
  }

  /** Expands a leading `~` against the harness's home, mirroring `tracked_projects`'s own
   * expansion, so a pattern's parent can be compared against snapshot project paths. */
  function expandMockHome(path: string): string {
    if (path === "~") return HARNESS_HOME;
    if (path.startsWith("~/")) return `${HARNESS_HOME}${path.slice(1)}`;
    return path;
  }

  /** The folder a `*` pattern's children live in. */
  function patternParent(pattern: string): string {
    const withoutStar = pattern.slice(0, -1).replace(/\/$/, "") || "/";
    return expandMockHome(withoutStar);
  }

  /** Mirrors `skill_project_folders::project_folders`: the fixture snapshot's projects, labelled
   * `discovered` unless the user added them by hand, one row per `*` pattern in `added` instead of
   * one row per folder it matches (a matched child is hidden unless it's also a plain added
   * entry), followed by any plain added folder missing from the snapshot's project list (not
   * found on disk). */
  function projectFolders(): ProjectFolder[] {
    const addedSet = new Set(trackedProjects.added);
    const patterns = trackedProjects.added.filter(isProjectPattern);
    const plainAddedSet = new Set(trackedProjects.added.filter((path) => !isProjectPattern(path)));
    const patternEntries = patterns.map((pattern) => ({ pattern, parent: patternParent(pattern) }));
    const patternParents = new Set(patternEntries.map((entry) => entry.parent));

    const rows: ProjectFolder[] = [];
    for (const path of currentSnapshot.projects) {
      const parent = path.slice(0, path.lastIndexOf("/"));
      if (patternParents.has(parent) && !plainAddedSet.has(path)) continue;
      rows.push({
        path,
        source: addedSet.has(path) ? "added" : "discovered",
        missing: false,
        matches: null,
      });
    }
    for (const { pattern, parent } of patternEntries) {
      const matches = currentSnapshot.projects.filter(
        (path) => path.slice(0, path.lastIndexOf("/")) === parent,
      ).length;
      rows.push({ path: pattern, source: "added", missing: false, matches });
    }

    const listedPaths = new Set(rows.map((row) => row.path));
    for (const path of plainAddedSet) {
      if (!listedPaths.has(path))
        rows.push({ path, source: "added", missing: true, matches: null });
    }
    return rows;
  }

  async function publish(next: SkillSnapshot): Promise<void> {
    currentSnapshot = {
      ...next,
      revision: currentSnapshot.revision + 1,
      scanned_at: new Date().toISOString(),
    };
    await emit("skills://snapshot", currentSnapshot);
  }

  function updateSkill(
    name: string,
    transform: (item: InstalledSkill) => InstalledSkill,
  ): Promise<void> {
    return publish({
      ...currentSnapshot,
      skills: currentSnapshot.skills.map((item) => (item.name === name ? transform(item) : item)),
    });
  }

  /** Resolves a `LifecycleTarget`-shaped payload (`deployment_id` or `owner_id`) to the skill it
   * names - every deployment/owner id this fixture mints ends in `/<name>`. */
  function skillNameForTarget(target: {
    deployment_id?: string | null;
    owner_id?: string | null;
  }): string {
    if (target.deployment_id) {
      const found = currentSnapshot.skills.find((item) =>
        item.deployments.some((d) => d.id === target.deployment_id),
      );
      if (found) return found.name;
    }
    const key = target.deployment_id ?? target.owner_id ?? "";
    const name = key.split("/").pop();
    if (!name) throw new Error(`harness: target names no skill: ${JSON.stringify(target)}`);
    return name;
  }

  function buildAddedSkill(
    request: AddRequestLike,
    name: string,
    description: string,
  ): InstalledSkill {
    const isProject = request.scope === "project";
    const project = isProject ? (request.project_path ?? HARNESS_PROJECT) : null;
    const base = project
      ? `${project}/.agents/skills/${name}`
      : `${HARNESS_HOME}/.agents/skills/${name}`;
    const deployments: Deployment[] = [
      deployment({
        agent: "shared",
        scope: isProject ? "project" : "global",
        project_path: project ?? undefined,
        path: base,
        content_hash: `${name}-v1`,
      }),
      deployment({
        agent: "Claude Code",
        scope: isProject ? "project" : "global",
        project_path: project ?? undefined,
        path: project
          ? `${project}/.claude/skills/${name}`
          : `${HARNESS_HOME}/.claude/skills/${name}`,
        is_symlink: true,
        symlink_target: base,
        resolved_path: base,
        content_hash: `${name}-v1`,
      }),
    ];
    harnessSkillContent.set(
      `${base}/SKILL.md`,
      `---\nname: ${name}\ndescription: ${description}\n---\n\n# ${name}\n\n${description}\n`,
    );
    return skill({
      name,
      source: request.source.repo ?? request.source.url ?? "manual",
      description,
      installed_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
      modified_at: new Date().toISOString(),
      deployments,
      trial: request.trial
        ? {
            deployment_id: deployments[0].id,
            expires_at: new Date(Date.now() + 24 * 60 * 60_000).toISOString(),
            method: request.method,
            scope: isProject ? "project" : "global",
            project_path: project,
            status: "active",
          }
        : null,
      trials: [],
    });
  }

  mockWindows("main");
  mockIPC(
    async (command, rawPayload) => {
      const payload = isObjectInvokeArgs(rawPayload) ? rawPayload : {};
      switch (command) {
        case "get_skill_snapshot":
          return currentSnapshot;
        case "get_installed_skills":
          return currentSnapshot.skills;
        case "request_skill_rescan":
          await publish(currentSnapshot);
          return undefined;
        case "get_tracked_projects":
          return snapshotTrackedProjects();
        case "register_skill_projects": {
          const typed = z.array(z.string()).parse(payload.paths);
          for (const path of typed) {
            const segments = path.split("/");
            const strayStar = segments.some(
              (segment, i) =>
                segment.includes("*") && !(i === segments.length - 1 && segment === "*"),
            );
            if (strayStar) {
              throw new Error("Only the last part of a path can be *, as in ~/src/*.");
            }
          }
          // A pattern is saved exactly as typed; a plain path is expanded, matching the backend's
          // `entry_to_save`.
          const paths = typed.map((path) => (isProjectPattern(path) ? path : expandMockHome(path)));
          trackProjects(paths);
          return snapshotTrackedProjects();
        }
        case "unregister_skill_project": {
          const path = z.string().parse(payload.path);
          untrackProject(path);
          return snapshotTrackedProjects();
        }
        case "import_tracked_projects": {
          const added = z.array(z.string()).parse(payload.added);
          const excluded = z.array(z.string()).parse(payload.excluded);
          trackProjects(added);
          excluded.forEach((path) => untrackProject(path));
          return snapshotTrackedProjects();
        }
        case "remove_skill_project": {
          const path = z.string().parse(payload.path);
          forgetProject(path);
          return snapshotTrackedProjects();
        }
        case "get_discovery_sources":
          return discoverySourceSettings();
        case "set_discovery_source": {
          const harness = z.string().parse(payload.harness);
          const enabled = z.boolean().parse(payload.enabled);
          if (!discoverySources.has(harness)) {
            throw new Error(`Unknown discovery source: ${harness}`);
          }
          discoverySources.set(harness, enabled);
          return discoverySourceSettings();
        }
        case "list_project_folders":
          return projectFolders();
        case "detect_harnesses":
          return {
            harnesses: DISCOVERY_HARNESSES.map((id) => ({
              id,
              display_name: id,
              state: discoverySources.get(id) ? "configured" : "not_found",
              executable: null,
              version: { value: null, evidence: { source: "mock", confidence: "unknown" } },
              install_method: { value: null, evidence: { source: "mock", confidence: "unknown" } },
              configured: discoverySources.get(id) ?? false,
              used: false,
            })),
          };
        case "get_harnesses_choice":
          return null;
        case "save_harnesses_choice":
          return undefined;
        case "open_skill_path":
        case "restore_trashed_skill":
        case "unfork_skill":
          return undefined;
        case "app_version":
          return {
            version: "0.1.0",
            commit: "dev",
            notes: "### Added\n\n- The dev harness mock for `app_version`.",
          };
        case "get_editor_choices":
          return editorChoices();
        case "command_health":
          return COMMAND_HEALTH_ROWS;
        case "set_preferred_editor":
          editorPreference = payload.appName == null ? null : String(payload.appName);
          return undefined;
        case "get_error_reporting_enabled":
          return false;
        case "set_error_reporting_enabled":
          return Boolean(payload.enabled);

        case "read_installed_skill_md": {
          const path = String(payload.path);
          const name = path.split("/").filter(Boolean).pop() ?? "skill";
          return harnessSkillContent.get(path) ?? fallbackSkillContent(name);
        }
        case "write_installed_skill_md_if_unchanged": {
          harnessSkillContent.set(String(payload.path), String(payload.content));
          return undefined;
        }

        case "preview_skill_frontmatter_repair":
          return {
            deployment_id: "mock-deployment",
            path: "/mock/SKILL.md",
            scope: "global",
            reason: "Frontmatter is missing the required `name` field.",
            expected_content_fingerprint: "mock-fingerprint",
            proposal_id: "mock-proposal",
            original_content: "---\ndescription: mock\n---\n",
            proposed_content: "---\nname: mock-skill\ndescription: mock\n---\n",
            allowed_apply_modes: ["apply-fix"],
          };
        case "apply_skill_frontmatter_repair":
          return undefined;
        case "fix_skill":
          return { skill: String(payload.skill ?? ""), applied: [], unrepaired: [], conflicts: [] };
        case "open_conflict_paths":
          return undefined;

        case "list_skill_events":
          return [];
        case "get_add_method_defaults":
          return {
            dotagents_installed: true,
            has_skill_lock: true,
            installed_harnesses: ["claude-code", "codex", "open-code", "pi", "cursor"],
            claude_reads_shared_folder: false,
          };
        case "list_github_skills":
          return {
            repo: "anthropics/skills",
            git_ref: "main",
            commit: "fixture",
            skills: [{ name: "frontend-design", path: "frontend-design" }],
            truncated: false,
          };
        case "install_preferences":
          return { method: "dotagents", harnesses: ["claude-code", "codex"], saved: true };
        case "import_skill_pack":
          return { status: "imported", result: { bundled: [], referenced: [], errors: [] } };
        case "confirm_skill_pack_trust":
          return { bundled: [], referenced: [], errors: [] };
        case "abandon_pack_import_trust":
          return true;
        case "set_plugin_enabled":
        case "uninstall_plugin":
          return undefined;
        case "search_skills":
        case "get_popular_skills":
          return { has_more: false, skills: [] };
        case "get_skill_details":
          return {
            id: String(payload.skillId ?? ""),
            slug: "",
            source: "",
            hash: "",
            installs: 0,
            skill_md: null,
          };
        case "list_skill_packs":
          return [];
        case "list_skill_runs":
          return [];
        case "read_skill_run_events":
          return [];

        case "park_skill": {
          const { deployment_id, owner_id } = z
            .object({ deployment_id: z.string().nullish(), owner_id: z.string().nullish() })
            .parse(payload.target);
          const name = skillNameForTarget({ deployment_id, owner_id });
          await updateSkill(name, (item) => ({
            ...item,
            parked: true,
            parked_at: new Date().toISOString(),
            deployments: item.deployments.map((d) => ({ ...d, disabled: true })),
          }));
          return undefined;
        }
        case "unpark_skill": {
          const { deployment_id, owner_id } = z
            .object({ deployment_id: z.string().nullish(), owner_id: z.string().nullish() })
            .parse(payload.target);
          const name = skillNameForTarget({ deployment_id, owner_id });
          await updateSkill(name, (item) => ({
            ...item,
            parked: false,
            parked_at: null,
            deployments: item.deployments.map((d) => ({ ...d, disabled: false })),
          }));
          return undefined;
        }

        case "set_harness_enabled": {
          const { deployment_id, reader_agent: agent } = z
            .object({ deployment_id: z.string(), reader_agent: z.string() })
            .parse(payload.target);
          const enabled = payload.enabled === true;
          const name = skillNameForTarget({ deployment_id });
          await updateSkill(name, (item) => ({
            ...item,
            deployments: item.deployments.map((entry) => {
              if (entry.id !== deployment_id) return entry;
              if (entry.agent === "shared") {
                const disabledReaders = new Set(entry.disabled_readers ?? []);
                if (enabled) disabledReaders.delete(agent);
                else disabledReaders.add(agent);
                return { ...entry, disabled_readers: [...disabledReaders] };
              }
              return entry.agent.toLowerCase().replace(/ /g, "-") === agent
                ? { ...entry, disabled: !enabled, disabled_by: enabled ? null : "codex-config" }
                : entry;
            }),
          }));
          return undefined;
        }
        case "set_skill_invocation": {
          const name = z.string().parse(payload.name);
          const path = z.string().parse(payload.path);
          const policy =
            payload.policy === "user-only" || payload.policy === "model-only"
              ? payload.policy
              : "both";
          await updateSkill(name, (item) => ({
            ...item,
            invocation: policy,
            deployments: item.deployments.map((entry) =>
              entry.path === path ? { ...entry, invocation: policy } : entry,
            ),
          }));
          return undefined;
        }
        case "make_skill_independent_copy":
        case "materialize_harness_root":
        case "materialize_harness_root_then_disable": {
          return undefined;
        }

        case "remove_skill": {
          const target = z
            .object({ deployment_id: z.string().nullish(), owner_id: z.string().nullish() })
            .parse(payload.target);
          const name = skillNameForTarget(target);
          const deploymentId =
            target.deployment_id ??
            currentSnapshot.skills.find((s) => s.name === name)?.deployments[0]?.id ??
            `dep:v1/mock/${name}`;
          await publish({
            ...currentSnapshot,
            skills: currentSnapshot.skills.filter((s) => s.name !== name),
          });
          return {
            event_id: `evt:v1/mock/${name}`,
            deployment_id: deploymentId,
            skill: name,
            tree_hash_before: "mock-tree-hash",
            quarantine_path: null,
          } satisfies RemoveOutcome;
        }
        case "update_skill": {
          const target = z
            .object({ deployment_id: z.string().nullish(), owner_id: z.string().nullish() })
            .parse(payload.target);
          const name = skillNameForTarget(target);
          const deployment = currentSnapshot.skills.find((s) => s.name === name)?.deployments[0];
          await updateSkill(name, (item) => ({
            ...item,
            has_update: false,
            update_owner_ids: [],
            update_owners: [],
            update_commit: null,
            update_commit_at: null,
          }));
          return {
            event_id: `fixture-update-${name}`,
            skill: name,
            deployment_path: deployment?.path ?? `${HARNESS_HOME}/.agents/skills/${name}`,
            tree_hash_before: "fixture-before",
            tree_hash_after: "fixture-after",
          };
        }
        case "update_all_skills": {
          const targets = z
            .array(
              z.object({ deployment_id: z.string().nullish(), owner_id: z.string().nullish() }),
            )
            .parse(payload.targets);
          const skillsByName = new Map(currentSnapshot.skills.map((s) => [s.name, s]));
          const items = [];
          for (const target of targets) {
            const name = skillNameForTarget(target);
            const deployment = skillsByName.get(name)?.deployments[0];
            // react-doctor-disable-next-line react-doctor/async-await-in-loop -- fixture applies each update sequentially, mirroring the real op's per-request loop
            await updateSkill(name, (item) => ({
              ...item,
              has_update: false,
              update_owner_ids: [],
              update_owners: [],
              update_commit: null,
              update_commit_at: null,
            }));
            items.push({
              skill: name,
              outcome: {
                event_id: `fixture-update-${name}`,
                skill: name,
                deployment_path: deployment?.path ?? `${HARNESS_HOME}/.agents/skills/${name}`,
                tree_hash_before: "fixture-before",
                tree_hash_after: "fixture-after",
              },
            });
          }
          return { items, errors: {} };
        }

        case "fork_skill": {
          const target = z
            .object({ deployment_id: z.string().nullish(), owner_id: z.string().nullish() })
            .parse(payload.target);
          const name = skillNameForTarget(target);
          const skillItem = currentSnapshot.skills.find((s) => s.name === name);
          return {
            forked_at: new Date().toISOString(),
            origin_tool: "dotagents",
            origin_source: skillItem?.source ?? "manual",
            repo: skillItem?.source ?? "manual",
            path: skillItem?.deployments[0]?.path ?? `${HARNESS_HOME}/.agents/skills/${name}`,
            base_commit: "fixture",
          };
        }
        case "pull_fork_upstream":
          return {
            added: [],
            removed: [],
            merged: [],
            conflicts: [],
            unchanged: 0,
            from_commit: "fixture",
            to_commit: "fixture",
            message: "Already up to date",
          };
        case "keep_skill_trial": {
          const { deployment_id } = z.object({ deployment_id: z.string() }).parse(payload.target);
          await publish({
            ...currentSnapshot,
            skills: currentSnapshot.skills.map((item) =>
              item.trial?.deployment_id === deployment_id
                ? {
                    ...item,
                    trial: null,
                    trials: item.trials.filter((t) => t.deployment_id !== deployment_id),
                  }
                : item,
            ),
          });
          return undefined;
        }

        case "add_skill":
        case "start_add_skill_operation":
        case "start_add_skills_operation": {
          const request = addRequestSchema.parse(payload.request);
          const name =
            request.source.skillName ?? request.source.repo?.split("/").pop() ?? "added-skill";
          const added = buildAddedSkill(
            request,
            name,
            "A skill added through the harness fixture.",
          );
          await publish({ ...currentSnapshot, skills: [...currentSnapshot.skills, added] });
          const result = {
            name,
            tool: "dotagents",
            command: `dotagents add ${request.source.repo ?? name}`,
            deployments_created: added.deployments.map((entry) => entry.path),
            warning: null,
          };
          if (command === "add_skill") return result;
          const operationId = z.string().parse(payload.operationId);
          const completed: AddSkillOperationEvent = {
            operation_id: operationId,
            sequence: 1,
            phase: "completed",
            message: `Added ${name}`,
            ...(command === "start_add_skills_operation"
              ? { outcomes: [{ name, result, error: null }] }
              : { result }),
          };
          addOperations.set(operationId, completed);
          await emit("skills://add-skill-operation", completed);
          return completed;
        }
        case "get_add_skill_operation": {
          const operationId = z.string().parse(payload.operationId);
          const operation = addOperations.get(operationId);
          if (!operation) throw new Error(`harness: unknown add-skill operation ${operationId}`);
          return operation;
        }
        case "cancel_add_skill_operation":
        case "confirm_add_skill_trust": {
          const operationId = z.string().parse(payload.operationId ?? payload.retryOperationId);
          const operation = addOperations.get(operationId);
          if (!operation) throw new Error(`harness: unknown add-skill operation ${operationId}`);
          return operation;
        }

        case "repair_skill_link": {
          const { path, target } = z
            .object({
              path: z.string(),
              action: z.enum(["remove", "relink"]),
              target: z.string().nullish(),
            })
            .parse(payload);
          const name = path.split("/skills/").pop() ?? "";
          await updateSkill(name, (item) => {
            const content = target ? harnessSkillContent.get(`${target}/SKILL.md`) : undefined;
            if (target && content) harnessSkillContent.set(`${path}/SKILL.md`, content);
            return {
              ...item,
              deployments: item.deployments.map((entry) =>
                entry.path === path
                  ? {
                      ...entry,
                      symlink_is_broken: false,
                      symlink_target: target ?? entry.symlink_target,
                      resolved_path: target ?? entry.resolved_path,
                    }
                  : entry,
              ),
            };
          });
          return undefined;
        }
        case "restore_skill_event":
          await publish(currentSnapshot);
          return undefined;

        case "plugin:path|home_dir":
        case "plugin:path|resolve_directory":
          return HARNESS_HOME;
        case "plugin:dialog|ask":
          return true;
        case "plugin:dialog|open": {
          const parsedFilters = z
            .object({ filters: z.array(z.object({ extensions: z.array(z.string()) })).optional() })
            .safeParse(payload);
          const wantsApp =
            parsedFilters.success &&
            (parsedFilters.data.filters ?? []).some((filter) => filter.extensions.includes("app"));
          return wantsApp ? "/Applications/Zed Preview.app" : null;
        }
        case "plugin:opener|open_url":
        case "plugin:opener|open_path":
        case "plugin:shell|open":
          return undefined;

        default:
          // eslint-disable-next-line no-console
          console.warn("[harness] unhandled invoke", command, payload);
          throw new Error(`harness: unhandled command ${command}`);
      }
    },
    { shouldMockEvents: true },
  );

  return {
    snapshot: () => currentSnapshot,
    publish,
    updateSkill,
  };
}
