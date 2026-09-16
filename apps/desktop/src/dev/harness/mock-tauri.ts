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
import type {
  AddSkillOperationEvent,
  Deployment,
  InstalledSkill,
  SkillSnapshot,
  TrackedProjects,
} from "@skill-studio/lib";
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

/** Installs the mock Tauri IPC layer and returns the control the harness (or the marketing
 * capture page) drives it with. */
export function installMockTauri(initial: SkillSnapshot): HarnessControl {
  let currentSnapshot = initial;
  const addOperations = new Map<string, AddSkillOperationEvent>();
  // Mirrors the backend's `~/.agents/skill-studio.json` `projects` key - the register/unregister/
  // import handlers below mutate it with the same track/untrack rules as `TrackedProjects` in
  // `crates/skill-studio-core`, and `get_tracked_projects` reads it back.
  let trackedProjects: TrackedProjects = { added: [], excluded: [] };

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

  function updateDeployment(
    skillItem: InstalledSkill,
    deploymentId: string,
    transform: (d: Deployment) => Deployment,
  ): InstalledSkill {
    return {
      ...skillItem,
      deployments: skillItem.deployments.map((d) => (d.id === deploymentId ? transform(d) : d)),
    };
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
        case "request_skill_rescan":
          await publish(currentSnapshot);
          return undefined;
        case "get_tracked_projects":
          return snapshotTrackedProjects();
        case "register_skill_projects": {
          const paths = z.array(z.string()).parse(payload.paths);
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
        case "open_skill_path":
        case "set_preferred_editor":
        case "restore_trashed_skill":
        case "unfork_skill":
          return undefined;
        case "list_installed_editors":
          return [];
        case "get_preferred_editor":
          return null;

        case "read_installed_skill_md": {
          const path = String(payload.path);
          const name = path.split("/").filter(Boolean).pop() ?? "skill";
          return harnessSkillContent.get(path) ?? fallbackSkillContent(name);
        }
        case "write_installed_skill_md_if_unchanged": {
          harnessSkillContent.set(String(payload.path), String(payload.content));
          return undefined;
        }

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

        case "set_shared_harness_skill_enabled": {
          const { target, harness, enabled } = z
            .object({
              target: z.object({ deployment_id: z.string() }),
              harness: z.string(),
              enabled: z.boolean(),
            })
            .parse(payload);
          const name = skillNameForTarget(target);
          await updateSkill(name, (item) =>
            updateDeployment(item, target.deployment_id, (d) => ({
              ...d,
              disabled_readers: enabled
                ? (d.disabled_readers ?? []).filter((id) => id !== harness)
                : [...new Set([...(d.disabled_readers ?? []), harness])],
            })),
          );
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
        case "set_deployment_enabled": {
          const { deployment_id } = z.object({ deployment_id: z.string() }).parse(payload.target);
          const enabled = payload.enabled === true;
          const name = skillNameForTarget({ deployment_id });
          await updateSkill(name, (item) =>
            updateDeployment(item, deployment_id, (d) => ({
              ...d,
              disabled: !enabled,
              disabled_by: enabled ? null : "studio-moved",
            })),
          );
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
          await publish({
            ...currentSnapshot,
            skills: currentSnapshot.skills.filter((s) => s.name !== name),
          });
          return {
            skill_name: name,
            success: true,
            error: null,
            installed_path: null,
            command: "npx skills remove",
            tool: "dotagents",
          };
        }
        case "update_skill": {
          const target = z
            .object({ deployment_id: z.string().nullish(), owner_id: z.string().nullish() })
            .parse(payload.target);
          const name = skillNameForTarget(target);
          await updateSkill(name, (item) => ({
            ...item,
            has_update: false,
            update_owner_ids: [],
            update_owners: [],
            update_commit: null,
            update_commit_at: null,
          }));
          return {
            skill_name: name,
            success: true,
            error: null,
            installed_path: null,
            command: "npx skills update",
            tool: "dotagents",
          };
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
        case "plugin:dialog|open":
          return null;
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
