import { z } from "zod";
import { createRoot } from "react-dom/client";
import type { InvokeArgs } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type {
  AddSkillOperationEvent,
  Deployment,
  InstalledSkill,
  SkillEvent,
  SkillSnapshot,
} from "../../lib/src/index.ts";

const HOME = "/Users/demo";
const PROJECT = `${HOME}/src/agent-studio`;
const CAPTURE_NOW = Date.now();
const SCANNED_AT = new Date(CAPTURE_NOW).toISOString();

type Scene = "map" | "coverage" | "invoke" | "drift" | "install" | "activity" | "repair";

const SCENES: readonly Scene[] = [
  "map",
  "coverage",
  "invoke",
  "drift",
  "install",
  "activity",
  "repair",
];

interface CaptureControl {
  setScene: (scene: Scene) => void;
  snapshot: () => SkillSnapshot;
}

type ObjectInvokeArgs = Exclude<InvokeArgs, number[] | ArrayBuffer | Uint8Array>;

function isObjectInvokeArgs(payload: InvokeArgs | undefined): payload is ObjectInvokeArgs {
  return (
    payload !== undefined &&
    !Array.isArray(payload) &&
    !(payload instanceof ArrayBuffer) &&
    !(payload instanceof Uint8Array)
  );
}

declare global {
  interface Window {
    __captureReady?: boolean;
    __capture?: CaptureControl;
  }
}

function deployment(
  input: Partial<Deployment> & Pick<Deployment, "agent" | "scope" | "path">,
): Deployment {
  return {
    id: `capture:${input.path}`,
    destination: input.agent === "shared" || input.is_symlink ? "universal" : "per-harness",
    owner_kind: input.agent === "shared" ? "dotagents" : "manual",
    mutability: "mutable",
    backing: input.symlink_target
      ? { kind: "linked-to", deployment_id: `capture:${input.symlink_target}` }
      : input.agent === "shared"
        ? { kind: "canonical" }
        : { kind: "independent" },
    is_symlink: false,
    symlink_is_broken: false,
    content_hash: "commit-global-v1",
    disabled: false,
    spec_violations: [],
    invocation: "both",
    ...input,
  };
}

function skill(
  input: Partial<InstalledSkill> & Pick<InstalledSkill, "name" | "deployments">,
): InstalledSkill {
  return {
    source: "getsentry/skills",
    source_type: "github",
    source_url: "https://github.com/getsentry/skills",
    installed_at: "2026-08-20T14:00:00.000Z",
    updated_at: "2026-09-03T18:10:00.000Z",
    has_update: false,
    update_owner_ids: [],
    source_kind: "dotagents",
    has_spec: true,
    description:
      "Create clear, conventional Git commits with an accurate message and focused scope.",
    spec_violations: [],
    skill_md_tokens: 52,
    description_tokens: 18,
    folder_bytes: 18432,
    file_count: 4,
    content_hash: input.deployments[0]?.content_hash ?? "commit-global-v1",
    content_hashes: [...new Set(input.deployments.map((item) => item.content_hash))],
    modified_at: "2026-09-03T18:10:00.000Z",
    frontmatter_fields: { name: input.name, description: "Create clear commits" },
    folder_truncated: false,
    parked: false,
    invocation: "both",
    ...input,
  };
}

const commitDeployments = [
  deployment({ agent: "shared", scope: "global", path: `${HOME}/.agents/skills/commit` }),
  deployment({
    agent: "Claude Code",
    scope: "global",
    path: `${HOME}/.claude/skills/commit`,
    is_symlink: true,
    symlink_target: `${HOME}/.agents/skills/commit`,
    resolved_path: `${HOME}/.agents/skills/commit`,
  }),
  deployment({
    agent: "shared",
    scope: "project",
    project_path: PROJECT,
    path: `${PROJECT}/.agents/skills/commit`,
    content_hash: "commit-project-v2",
    invocation: "user-only",
  }),
  deployment({
    agent: "Claude Code",
    scope: "project",
    project_path: PROJECT,
    path: `${PROJECT}/.claude/skills/commit`,
    is_symlink: true,
    symlink_target: `${PROJECT}/.agents/skills/commit`,
    resolved_path: `${PROJECT}/.agents/skills/commit`,
    content_hash: "commit-project-v2",
    invocation: "user-only",
  }),
  deployment({
    agent: "Codex",
    scope: "project",
    project_path: PROJECT,
    path: `${PROJECT}/.codex/skills/commit`,
    content_hash: "commit-codex-v3",
    codex_implicit_invocation: true,
  }),
];

const commitSkill = skill({ name: "commit", deployments: commitDeployments });
const reviewSkill = skill({
  name: "code-review",
  description: "Review a change for correctness, regressions, security risks, and missing tests.",
  deployments: [
    deployment({
      agent: "shared",
      scope: "global",
      path: `${HOME}/.agents/skills/code-review`,
      content_hash: "review-v1",
    }),
    deployment({
      agent: "Claude Code",
      scope: "global",
      path: `${HOME}/.claude/skills/code-review`,
      is_symlink: true,
      symlink_target: `${HOME}/.agents/skills/code-review`,
      resolved_path: `${HOME}/.agents/skills/code-review`,
      content_hash: "review-v1",
    }),
  ],
});
const browserSkill = skill({
  name: "agent-browser",
  source: "vercel-labs/agent-browser",
  description: "Automate browser workflows and verify rendered product behavior.",
  deployments: [
    deployment({
      agent: "shared",
      scope: "global",
      path: `${HOME}/.agents/skills/agent-browser`,
      content_hash: "browser-v1",
    }),
    deployment({
      agent: "Claude Code",
      scope: "global",
      path: `${HOME}/.claude/skills/agent-browser`,
      is_symlink: true,
      symlink_target: `${HOME}/.agents/skills/agent-browser`,
      resolved_path: `${HOME}/.agents/skills/agent-browser`,
      content_hash: "browser-v1",
    }),
  ],
});
const repairSkill = skill({
  name: "release-notes",
  description: "Prepare accurate release notes from the changes in a repository.",
  deployments: [
    deployment({
      agent: "shared",
      scope: "global",
      path: `${HOME}/.agents/skills/release-notes`,
      content_hash: "release-v1",
    }),
    deployment({
      agent: "Claude Code",
      scope: "project",
      project_path: PROJECT,
      path: `${PROJECT}/.claude/skills/release-notes`,
      is_symlink: true,
      symlink_target: `${PROJECT}/.agents/skills/release-notes`,
      symlink_is_broken: true,
      content_hash: "",
    }),
  ],
});

const skillContent = new Map<string, string>([
  [
    `${HOME}/.agents/skills/commit/SKILL.md`,
    `---\nname: commit\ndescription: Create clear commits\n---\n\n# Commit\n\n- Inspect the diff.\n- Stage one focused change.\n- Use a conventional commit message.\n`,
  ],
  [
    `${HOME}/.claude/skills/commit/SKILL.md`,
    `---\nname: commit\ndescription: Create clear commits\n---\n\n# Commit\n\n- Inspect the diff.\n- Stage one focused change.\n- Use a conventional commit message.\n`,
  ],
  [
    `${PROJECT}/.agents/skills/commit/SKILL.md`,
    `---\nname: commit\ndescription: Create commits for Skill Studio\ndisable-model-invocation: true\n---\n\n# Commit\n\n- Run the scoped checks.\n- Stage one focused change.\n- Include the issue number in the message.\n`,
  ],
  [
    `${PROJECT}/.claude/skills/commit/SKILL.md`,
    `---\nname: commit\ndescription: Create commits for Skill Studio\ndisable-model-invocation: true\n---\n\n# Commit\n\n- Run the scoped checks.\n- Stage one focused change.\n- Include the issue number in the message.\n`,
  ],
  [
    `${PROJECT}/.codex/skills/commit/SKILL.md`,
    `---\nname: commit\ndescription: Create commits for Skill Studio\n---\n\n# Commit\n\n- Run formatting and types.\n- Keep the commit focused.\n- Include the issue number in the message.\n`,
  ],
  [
    `${HOME}/.agents/skills/code-review/SKILL.md`,
    `---\nname: code-review\ndescription: Review repository changes\n---\n\n# Code review\n\nReport concrete findings with file and line evidence.\n`,
  ],
  [
    `${HOME}/.agents/skills/agent-browser/SKILL.md`,
    `---\nname: agent-browser\ndescription: Verify browser workflows\n---\n\n# Browser verification\n\nUse accessible controls and capture evidence.\n`,
  ],
  [
    `${HOME}/.agents/skills/release-notes/SKILL.md`,
    `---\nname: release-notes\ndescription: Prepare accurate release notes\n---\n\n# Release notes\n\nSummarize the shipped behavior and link each change to evidence.\n`,
  ],
]);

function distributeDays(total: number, count: number, startIndex: number): number[] {
  const weights = Array.from({ length: count }, (_, index) => {
    const day = startIndex + index;
    const weekday = new Date(CAPTURE_NOW - day * 86_400_000).getUTCDay();
    return weekday === 0 ? 0 : weekday === 6 ? 1 : 2 + ((day * 17) % 5);
  });
  const weightTotal = weights.reduce((sum, weight) => sum + weight, 0);
  let cumulativeWeight = 0;
  return weights.map((weight) => {
    const previous = Math.floor((cumulativeWeight * total) / weightTotal);
    cumulativeWeight += weight;
    return Math.floor((cumulativeWeight * total) / weightTotal) - previous;
  });
}

function invocationDays(
  windowTotals: [number, number, number, number],
  earlier: number,
): Record<string, number> {
  const [today, sevenDays, fourteenDays, thirtyDays] = windowTotals;
  const counts = [
    today,
    ...distributeDays(sevenDays - today, 6, 1),
    ...distributeDays(fourteenDays - sevenDays, 7, 7),
    ...distributeDays(thirtyDays - fourteenDays, 16, 14),
    ...distributeDays(earlier, 335, 30),
  ];
  return Object.fromEntries(
    counts.map((count, index) => {
      const date = new Date(CAPTURE_NOW);
      date.setUTCHours(0, 0, 0, 0);
      date.setUTCDate(date.getUTCDate() - index);
      return [date.toISOString().slice(0, 10), count];
    }),
  );
}

const commitDays = invocationDays([8, 42, 79, 128], 1056);
const reviewDays = invocationDays([4, 27, 51, 76], 645);
const browserDays = invocationDays([2, 15, 29, 44], 423);
const heatmapDays = Object.fromEntries(
  Object.keys(commitDays).map((date) => [
    date,
    commitDays[date] + reviewDays[date] + browserDays[date],
  ]),
);

let currentSnapshot: SkillSnapshot = {
  revision: 1,
  skills: [commitSkill, reviewSkill, browserSkill, repairSkill],
  projects: [PROJECT, `${HOME}/src/acme-dashboard`],
  invocations: [
    {
      skill: "commit",
      total: Object.values(commitDays).reduce((sum, count) => sum + count, 0),
      last_24_hours: 8,
      last_7_days: 42,
      last_14_days: 79,
      last_30_days: 128,
      last_used: new Date(CAPTURE_NOW - 38 * 60_000).toISOString(),
      by_project_30_days: { [PROJECT]: 81, [`${HOME}/src/acme-dashboard`]: 47 },
      by_day: commitDays,
    },
    {
      skill: "code-review",
      total: Object.values(reviewDays).reduce((sum, count) => sum + count, 0),
      last_24_hours: 4,
      last_7_days: 27,
      last_14_days: 51,
      last_30_days: 76,
      last_used: new Date(CAPTURE_NOW - 2.3 * 60 * 60_000).toISOString(),
      by_project_30_days: { [PROJECT]: 63, [`${HOME}/src/acme-dashboard`]: 13 },
      by_day: reviewDays,
    },
    {
      skill: "agent-browser",
      total: Object.values(browserDays).reduce((sum, count) => sum + count, 0),
      last_24_hours: 2,
      last_7_days: 15,
      last_14_days: 29,
      last_30_days: 44,
      last_used: new Date(CAPTURE_NOW - 16.8 * 60 * 60_000).toISOString(),
      by_project_30_days: { [PROJECT]: 44 },
      by_day: browserDays,
    },
  ],
  heatmap: { days: heatmapDays },
  scanned_at: SCANNED_AT,
  last_test_by_skill: {},
  update_check: { checked_at: null, gh_status: "ok", message: null, updates_available: 0 },
  opencode_config_kind: "json",
};

const events: SkillEvent[] = [
  {
    id: "evt-1",
    ts: new Date(CAPTURE_NOW - 70 * 60_000).toISOString(),
    kind: "harness_enable",
    skill: "commit",
    harness: "codex",
    scope: "global",
    status: "done",
    restorable: true,
    force_restorable: true,
  },
  {
    id: "evt-2",
    ts: new Date(CAPTURE_NOW - 19 * 60 * 60_000).toISOString(),
    kind: "explode_shared_dir",
    skill: "code-review",
    harness: "claude-code",
    scope: "global",
    status: "done",
    restorable: true,
    force_restorable: true,
    backup_path: `${HOME}/.agents/backups/evt-2`,
  },
];

async function publishSnapshot(next: SkillSnapshot): Promise<void> {
  currentSnapshot = { ...next, revision: currentSnapshot.revision + 1 };
  await emit("skills://snapshot", currentSnapshot);
}

function updateCommit(transform: (item: InstalledSkill) => InstalledSkill): Promise<void> {
  return publishSnapshot({
    ...currentSnapshot,
    scanned_at: SCANNED_AT,
    skills: currentSnapshot.skills.map((item) => (item.name === "commit" ? transform(item) : item)),
  });
}

const addOperations = new Map<string, AddSkillOperationEvent>();

mockWindows("main");
mockIPC(
  async (command, rawPayload) => {
    const payload = isObjectInvokeArgs(rawPayload) ? rawPayload : {};
    switch (command) {
      case "get_skill_snapshot":
        return currentSnapshot;
      case "plugin:path|home_dir":
        return HOME;
      case "plugin:path|resolve_directory":
        return HOME;
      case "register_skill_projects":
      case "unregister_skill_project":
        return undefined;
      case "request_skill_rescan":
        await publishSnapshot(currentSnapshot);
        return undefined;
      case "read_installed_skill_md": {
        const content = skillContent.get(String(payload.path));
        if (content === undefined)
          throw new Error(`Fixture has no SKILL.md for ${String(payload.path)}`);
        return content;
      }
      case "list_skill_events":
        return events;
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
      case "set_shared_harness_skill_enabled": {
        const harness = String(payload.harness);
        const enabled = Boolean(payload.enabled);
        await updateCommit((item) => ({
          ...item,
          deployments: item.deployments.map((entry) =>
            entry.agent === "shared" && entry.scope === "global"
              ? {
                  ...entry,
                  disabled_readers: enabled
                    ? (entry.disabled_readers ?? []).filter((id) => id !== harness)
                    : [...new Set([...(entry.disabled_readers ?? []), harness])],
                }
              : entry,
          ),
        }));
        return undefined;
      }
      case "set_harness_enabled": {
        const { deployment_id, reader_agent: agent } = z
          .object({ deployment_id: z.string(), reader_agent: z.string() })
          .parse(payload.target);
        const enabled = payload.enabled === true;
        await updateCommit((item) => ({
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
              ? { ...entry, disabled: !enabled, disabled_by: enabled ? undefined : "codex-config" }
              : entry;
          }),
        }));
        return undefined;
      }
      case "set_deployment_enabled": {
        const { deployment_id } = z.object({ deployment_id: z.string() }).parse(payload.target);
        await updateCommit((item) => ({
          ...item,
          deployments: item.deployments.map((entry) =>
            entry.id === deployment_id
              ? {
                  ...entry,
                  disabled: payload.enabled !== true,
                  disabled_by: payload.enabled === true ? undefined : "studio-moved",
                }
              : entry,
          ),
        }));
        return undefined;
      }
      case "set_skill_invocation": {
        const path = String(payload.path).replace(/\/SKILL\.md$/, "");
        const policy =
          payload.policy === "user-only" || payload.policy === "model-only"
            ? payload.policy
            : "both";
        await updateCommit((item) => ({
          ...item,
          invocation: policy,
          deployments: item.deployments.map((entry) =>
            entry.path === path ? { ...entry, invocation: policy } : entry,
          ),
        }));
        return undefined;
      }
      case "fork_skill":
        return {
          forked_at: SCANNED_AT,
          origin_tool: "dotagents",
          origin_source: "getsentry/skills",
          repo: "getsentry/skills",
          path: `${HOME}/.agents/skills/commit`,
          base_commit: "fixture",
        };
      case "add_skill":
      case "start_add_skill_operation":
      case "start_add_skills_operation": {
        z.object({
          trial: z.literal(true),
          scope: z.literal("project"),
          project_path: z.literal(PROJECT),
          method: z.literal("dotagents"),
          destination: z.literal("universal"),
        }).parse(payload.request);
        const added = skill({
          name: "frontend-design",
          source: "anthropics/skills",
          description:
            "Build distinctive, production-grade frontend interfaces with strong visual craft.",
          source_kind: "dotagents",
          installed_at: new Date(CAPTURE_NOW).toISOString(),
          updated_at: new Date(CAPTURE_NOW).toISOString(),
          modified_at: new Date(CAPTURE_NOW).toISOString(),
          skill_md_tokens: 47,
          trial: {
            deployment_id: `capture:${PROJECT}/.agents/skills/frontend-design`,
            expires_at: new Date(CAPTURE_NOW + 24 * 60 * 60_000).toISOString(),
            method: "dotagents",
            scope: "project",
            project_path: PROJECT,
          },
          deployments: [
            deployment({
              agent: "shared",
              scope: "project",
              project_path: PROJECT,
              path: `${PROJECT}/.agents/skills/frontend-design`,
              content_hash: "frontend-v1",
            }),
            deployment({
              agent: "Claude Code",
              scope: "project",
              project_path: PROJECT,
              path: `${PROJECT}/.claude/skills/frontend-design`,
              is_symlink: true,
              symlink_target: `${PROJECT}/.agents/skills/frontend-design`,
              resolved_path: `${PROJECT}/.agents/skills/frontend-design`,
              content_hash: "frontend-v1",
            }),
          ],
        });
        skillContent.set(
          `${PROJECT}/.agents/skills/frontend-design/SKILL.md`,
          `---\nname: frontend-design\ndescription: Build polished interfaces\n---\n\n# Frontend design\n\nCreate a clear hierarchy and verify the rendered result.\n`,
        );
        await publishSnapshot({ ...currentSnapshot, skills: [...currentSnapshot.skills, added] });
        const result = {
          name: "frontend-design",
          tool: "dotagents",
          command: "dotagents add anthropics/skills",
          deployments_created: added.deployments.map((entry) => entry.path),
        };
        if (command === "add_skill") return result;
        const operationId = z.string().parse(payload.operationId);
        const completed: AddSkillOperationEvent = {
          operation_id: operationId,
          sequence: 1,
          phase: "completed",
          message: "Added frontend-design",
          ...(command === "start_add_skills_operation"
            ? { outcomes: [{ name: "frontend-design", result }] }
            : { result }),
        };
        addOperations.set(operationId, completed);
        await emit("skills://add-skill-operation", completed);
        return completed;
      }
      case "get_add_skill_operation": {
        const operationId = z.string().parse(payload.operationId);
        const operation = addOperations.get(operationId);
        if (!operation) throw new Error(`Unknown capture operation: ${operationId}`);
        return operation;
      }
      case "keep_skill_trial": {
        const { deployment_id } = z.object({ deployment_id: z.string() }).parse(payload.target);
        await publishSnapshot({
          ...currentSnapshot,
          skills: currentSnapshot.skills.map((item) =>
            item.trial?.deployment_id === deployment_id ? { ...item, trial: undefined } : item,
          ),
        });
        return undefined;
      }
      case "repair_skill_link": {
        const { path, target } = z
          .object({
            path: z.literal(`${PROJECT}/.claude/skills/release-notes`),
            action: z.literal("relink"),
            target: z.literal(`${HOME}/.agents/skills/release-notes`),
          })
          .parse(payload);
        const content = skillContent.get(`${target}/SKILL.md`);
        if (!content) throw new Error("Repair target has no fixture content");
        skillContent.set(`${path}/SKILL.md`, content);
        await publishSnapshot({
          ...currentSnapshot,
          skills: currentSnapshot.skills.map((item) =>
            item.name === "release-notes"
              ? {
                  ...item,
                  content_hashes: ["release-v1"],
                  deployments: item.deployments.map((entry) =>
                    entry.path === path
                      ? {
                          ...entry,
                          symlink_is_broken: false,
                          symlink_target: target,
                          resolved_path: target,
                          content_hash: "release-v1",
                        }
                      : entry,
                  ),
                }
              : item,
          ),
        });
        return undefined;
      }
      default:
        throw new Error(`Unhandled capture IPC command: ${command}`);
    }
  },
  { shouldMockEvents: true },
);

const params = new URLSearchParams(location.search);
const sceneParam = params.get("scene");
const initialScene = SCENES.find((scene) => scene === sceneParam) ?? "map";
const theme = params.get("theme") === "light" ? "light" : "dark";
localStorage.setItem("theme", theme);
localStorage.setItem("project-paths", PROJECT);
document.documentElement.setAttribute("data-theme", theme);

const [{ default: App }, { useAppStore }] = await Promise.all([
  import("../../../apps/desktop/src/App.tsx"),
  import("../../../apps/desktop/src/store/appStore.ts"),
]);

function setScene(scene: Scene): void {
  const common = {
    userAddedProjects: [PROJECT],
    excludedProjects: [],
    addSkillSheet: { open: false },
  };
  if (scene === "coverage") {
    useAppStore.setState({
      ...common,
      activeView: { kind: "skills" },
      showCoverage: true,
      skillListFilter: { ...useAppStore.getState().skillListFilter, query: "", scope: "all" },
    });
  } else if (scene === "activity") {
    useAppStore.setState({ ...common, activeView: { kind: "activity" }, usageWindow: "30d" });
  } else if (scene === "install") {
    useAppStore.setState({
      ...common,
      activeView: { kind: "home" },
      addSkillSheet: { open: true, prefill: "anthropics/skills/tree/main/frontend-design" },
    });
  } else if (scene === "repair") {
    useAppStore.setState({
      ...common,
      activeView: {
        kind: "skill",
        name: "release-notes",
        deploymentPath: `${PROJECT}/.claude/skills/release-notes`,
        from: { kind: "home" },
      },
    });
  } else {
    useAppStore.setState({
      ...common,
      activeView: {
        kind: "skill",
        name: "commit",
        from: { kind: "skills" },
        intent: scene === "drift" ? "compare" : undefined,
      },
    });
  }
}

setScene(initialScene);
window.__capture = { setScene, snapshot: () => currentSnapshot };
createRoot(document.getElementById("root")!).render(<App />);
requestAnimationFrame(() =>
  requestAnimationFrame(() => {
    window.__captureReady = true;
  }),
);
