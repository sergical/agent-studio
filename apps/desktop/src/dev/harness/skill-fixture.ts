// ============================================================================
// Skill Studio - skill-fixture
// The harness's in-memory skill estate: a realistic ~30-skill snapshot built
// with the same `deployment()`/`skill()` factories the marketing capture
// scenes used to define inline. Every Stack row state (universal/linked/own/
// broken, disabled, parked, drift, spec violations, plugin/manual source,
// updates, trials, invocations) appears on at least one skill here so an
// agent can screenshot every row treatment without a real disk scan.
// ============================================================================

import type { Deployment, InstalledSkill, SkillSnapshot } from "@skill-studio/lib";

export const HARNESS_HOME = "/Users/demo";
export const HARNESS_PROJECT = `${HARNESS_HOME}/src/agent-studio`;
const ACME_PROJECT = `${HARNESS_HOME}/src/acme-dashboard`;
const PERSONAL_PROJECT = `${HARNESS_HOME}/src/personal-site`;
const BACKEND_PROJECT = `${HARNESS_HOME}/src/internal-tools-backend`;
const MOBILE_PROJECT = `${HARNESS_HOME}/src/internal-tools-mobile`;

const CAPTURE_NOW = Date.now();
const SCANNED_AT = new Date(CAPTURE_NOW).toISOString();

/** Exported for `mock-tauri.ts`'s `add_skill`/`start_add_skill_operation` handlers, which build a
 * freshly "installed" skill from whatever request the Add Skill sheet actually sent. */
export function deployment(
  input: Partial<Deployment> & Pick<Deployment, "agent" | "scope" | "path">,
): Deployment {
  return {
    id: `harness:${input.path}`,
    destination: input.agent === "shared" || input.is_symlink ? "universal" : "per-harness",
    owner_kind: input.agent === "shared" ? "dotagents" : "manual",
    mutability: "mutable",
    backing: input.symlink_target
      ? { kind: "linked-to", deployment_id: `harness:${input.symlink_target}` }
      : input.agent === "shared"
        ? { kind: "canonical" }
        : { kind: "independent" },
    is_symlink: false,
    symlink_is_broken: false,
    shared_via_whole_dir_link: false,
    content_hash: "v1",
    disabled: false,
    disabled_by: null,
    codex_implicit_invocation: null,
    spec_violations: [],
    invocation: "both",
    ...input,
  };
}

export function skill(
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
    update_owners: [],
    update_commit: null,
    update_commit_at: null,
    source_kind: "dotagents",
    has_spec: true,
    description: "A generic skill used to round out the harness fixture's estate.",
    spec_violations: [],
    skill_md_tokens: 52,
    description_tokens: 18,
    folder_bytes: 18432,
    file_count: 4,
    content_hash: input.deployments[0]?.content_hash ?? "v1",
    content_hashes: [...new Set(input.deployments.map((item) => item.content_hash))],
    modified_at: "2026-09-03T18:10:00.000Z",
    frontmatter_fields: { name: input.name, description: "A generic skill" },
    folder_truncated: false,
    parked: false,
    parked_at: null,
    trial: null,
    trials: [],
    fork: null,
    skill_path: null,
    invocation: "both",
    ...input,
  };
}

function globalUniversal(name: string, hash: string): Deployment {
  return deployment({
    agent: "shared",
    scope: "global",
    path: `${HARNESS_HOME}/.agents/skills/${name}`,
    content_hash: hash,
  });
}

function globalLinked(name: string, agent: string, dir: string, hash: string): Deployment {
  const target = `${HARNESS_HOME}/.agents/skills/${name}`;
  return deployment({
    agent,
    scope: "global",
    path: `${HARNESS_HOME}/${dir}/skills/${name}`,
    is_symlink: true,
    symlink_target: target,
    resolved_path: target,
    content_hash: hash,
  });
}

function projectUniversal(name: string, project: string, hash: string): Deployment {
  return deployment({
    agent: "shared",
    scope: "project",
    project_path: project,
    path: `${project}/.agents/skills/${name}`,
    content_hash: hash,
  });
}

function projectLinked(
  name: string,
  agent: string,
  dir: string,
  project: string,
  hash: string,
): Deployment {
  const target = `${project}/.agents/skills/${name}`;
  return deployment({
    agent,
    scope: "project",
    project_path: project,
    path: `${project}/${dir}/skills/${name}`,
    is_symlink: true,
    symlink_target: target,
    resolved_path: target,
    content_hash: hash,
  });
}

function ownGlobal(
  name: string,
  agent: string,
  dir: string,
  hash: string,
  disabled = false,
): Deployment {
  return deployment({
    agent,
    scope: "global",
    path: `${HARNESS_HOME}/${dir}/skills/${name}`,
    content_hash: hash,
    disabled,
    disabled_by: disabled ? "studio-moved" : null,
  });
}

function brokenLink(
  name: string,
  agent: string,
  dir: string,
  project: string | null,
  hash: string,
): Deployment {
  const base = project
    ? `${project}/${dir}/skills/${name}`
    : `${HARNESS_HOME}/${dir}/skills/${name}`;
  return deployment({
    agent,
    scope: project ? "project" : "global",
    project_path: project ?? undefined,
    path: base,
    is_symlink: true,
    symlink_is_broken: true,
    resolved_path: null,
    content_hash: hash,
  });
}

// --- The four original capture-scene skills (names/content depended on by
// the marketing capture scenes) -----------------------------------------

const commitDeployments = [
  globalUniversal("commit", "commit-global-v1"),
  globalLinked("commit", "Claude Code", ".claude", "commit-global-v1"),
  projectUniversal("commit", HARNESS_PROJECT, "commit-project-v2"),
  projectLinked("commit", "Claude Code", ".claude", HARNESS_PROJECT, "commit-project-v2"),
  deployment({
    agent: "Codex",
    scope: "project",
    project_path: HARNESS_PROJECT,
    path: `${HARNESS_PROJECT}/.codex/skills/commit`,
    content_hash: "commit-codex-v3",
    codex_implicit_invocation: true,
  }),
];
const commitSkill = skill({
  name: "commit",
  description: "Create clear, conventional Git commits with an accurate message and focused scope.",
  deployments: commitDeployments,
});

const reviewSkill = skill({
  name: "code-review",
  description: "Review a change for correctness, regressions, security risks, and missing tests.",
  deployments: [
    globalUniversal("code-review", "review-v1"),
    globalLinked("code-review", "Claude Code", ".claude", "review-v1"),
  ],
});

const browserSkill = skill({
  name: "agent-browser",
  source: "vercel-labs/agent-browser",
  source_url: "https://github.com/vercel-labs/agent-browser",
  description: "Automate browser workflows and verify rendered product behavior.",
  deployments: [
    globalUniversal("agent-browser", "browser-v1"),
    globalLinked("agent-browser", "Claude Code", ".claude", "browser-v1"),
  ],
});

const repairSkill = skill({
  name: "release-notes",
  description: "Prepare accurate release notes from the changes in a repository.",
  deployments: [
    globalUniversal("release-notes", "release-v1"),
    brokenLink("release-notes", "Claude Code", ".claude", HARNESS_PROJECT, ""),
  ],
});

// --- ~26 more skills, covering every remaining Stack row state ----------

const globalOnlySkill = skill({
  name: "pdf-processing",
  source: "anthropics/skills",
  source_url: "https://github.com/anthropics/skills",
  description: "Extract, split, and merge PDF documents from the command line.",
  deployments: [globalUniversal("pdf-processing", "pdf-v1")],
  skill_md_tokens: 180,
  description_tokens: 12,
  folder_bytes: 14_000,
  file_count: 3,
  installed_at: "2026-01-09T10:00:00.000Z",
  updated_at: "2026-01-22T15:30:00.000Z",
});

const projectOnlySkill = skill({
  name: "sdk-migration",
  source: "vercel/ai",
  source_url: "https://github.com/vercel/ai",
  description: "Migrate an app across major versions of the Vercel AI SDK.",
  deployments: [
    projectUniversal("sdk-migration", ACME_PROJECT, "sdk-v1"),
    projectLinked("sdk-migration", "Claude Code", ".claude", ACME_PROJECT, "sdk-v1"),
  ],
  skill_md_tokens: 320,
  description_tokens: 11,
  folder_bytes: 26_000,
  file_count: 5,
  installed_at: "2026-01-14T11:20:00.000Z",
  updated_at: "2026-02-03T09:15:00.000Z",
});

const onePlusProjectSkill = skill({
  name: "sentry-triage",
  source: "getsentry/skills",
  description: "Triage a Sentry issue from its stack trace to a root cause.",
  deployments: [
    globalUniversal("sentry-triage", "triage-v1"),
    projectUniversal("sentry-triage", HARNESS_PROJECT, "triage-v1"),
  ],
  skill_md_tokens: 95,
  description_tokens: 12,
  folder_bytes: 7_000,
  file_count: 2,
  installed_at: "2026-02-05T08:45:00.000Z",
  updated_at: "2026-02-18T13:00:00.000Z",
});

const twoPlusProjectSkill = skill({
  name: "prompt-caching",
  source: "openai/codex",
  source_url: "https://github.com/openai/codex",
  description: "Structure prompts to maximize provider-side cache hits.",
  deployments: [
    globalUniversal("prompt-caching", "cache-v1"),
    projectUniversal("prompt-caching", HARNESS_PROJECT, "cache-v1"),
    projectUniversal("prompt-caching", ACME_PROJECT, "cache-v1"),
  ],
  skill_md_tokens: 1_450,
  description_tokens: 9,
  folder_bytes: 92_000,
  file_count: 9,
  installed_at: "2026-02-11T16:10:00.000Z",
  updated_at: "2026-03-01T10:40:00.000Z",
});

const fourPlusProjectSkill = skill({
  name: "filesystem-audit",
  source: "modelcontextprotocol/servers",
  source_url: "https://github.com/modelcontextprotocol/servers",
  description: "Audit filesystem MCP server permissions against least privilege.",
  deployments: [
    globalUniversal("filesystem-audit", "audit-v1"),
    projectUniversal("filesystem-audit", HARNESS_PROJECT, "audit-v1"),
    projectUniversal("filesystem-audit", ACME_PROJECT, "audit-v1"),
    projectUniversal("filesystem-audit", PERSONAL_PROJECT, "audit-v1"),
    projectUniversal("filesystem-audit", BACKEND_PROJECT, "audit-v1"),
  ],
  skill_md_tokens: 210,
  description_tokens: 10,
  folder_bytes: 16_000,
  file_count: 3,
  installed_at: "2026-03-04T09:30:00.000Z",
  updated_at: "2026-03-20T14:05:00.000Z",
});

const oneHarnessSkill = skill({
  name: "changelog-draft",
  source: "vercel/turborepo",
  source_url: "https://github.com/vercel/turborepo",
  description: "Draft a changelog entry from a pull request's diff.",
  deployments: [
    globalUniversal("changelog-draft", "changelog-v1"),
    globalLinked("changelog-draft", "Claude Code", ".claude", "changelog-v1"),
  ],
  skill_md_tokens: 60,
  description_tokens: 10,
  folder_bytes: 4_200,
  file_count: 1,
  installed_at: "2026-03-09T12:00:00.000Z",
  updated_at: "2026-03-27T17:50:00.000Z",
});

const threeHarnessSkill = skill({
  name: "test-flake-hunt",
  source: "vitest-dev/vitest",
  source_url: "https://github.com/vitest-dev/vitest",
  description: "Reproduce and isolate a flaky test from its CI history.",
  deployments: [
    globalUniversal("test-flake-hunt", "flake-v1"),
    globalLinked("test-flake-hunt", "Claude Code", ".claude", "flake-v1"),
    ownGlobal("test-flake-hunt", "Codex", ".codex", "flake-v1"),
    ownGlobal("test-flake-hunt", "OpenCode", ".opencode", "flake-v1"),
  ],
  skill_md_tokens: 680,
  description_tokens: 11,
  folder_bytes: 48_000,
  file_count: 6,
  installed_at: "2026-04-02T08:00:00.000Z",
  updated_at: "2026-04-16T11:25:00.000Z",
});

const fiveHarnessSkill = skill({
  name: "schema-migration-check",
  source: "prisma/prisma",
  source_url: "https://github.com/prisma/prisma",
  description: "Validate a database schema migration is safe to run forward.",
  deployments: [
    globalUniversal("schema-migration-check", "schema-v1"),
    globalLinked("schema-migration-check", "Claude Code", ".claude", "schema-v1"),
    ownGlobal("schema-migration-check", "Codex", ".codex", "schema-v1"),
    ownGlobal("schema-migration-check", "OpenCode", ".opencode", "schema-v1"),
    ownGlobal("schema-migration-check", "pi", ".pi", "schema-v1"),
    ownGlobal("schema-migration-check", "Cursor", ".cursor", "schema-v1"),
  ],
  skill_md_tokens: 2_400,
  description_tokens: 13,
  folder_bytes: 165_000,
  file_count: 12,
  installed_at: "2026-04-07T13:40:00.000Z",
  updated_at: "2026-05-01T09:10:00.000Z",
});

const sevenHarnessSkill = skill({
  name: "dependency-upgrade-plan",
  source: "renovatebot/renovate",
  source_url: "https://github.com/renovatebot/renovate",
  description: "Plan a batched dependency upgrade and flag breaking changes.",
  deployments: [
    globalUniversal("dependency-upgrade-plan", "upgrade-v1"),
    globalLinked("dependency-upgrade-plan", "Claude Code", ".claude", "upgrade-v1"),
    ownGlobal("dependency-upgrade-plan", "Codex", ".codex", "upgrade-v1"),
    ownGlobal("dependency-upgrade-plan", "OpenCode", ".opencode", "upgrade-v1"),
    ownGlobal("dependency-upgrade-plan", "pi", ".pi", "upgrade-v1"),
    ownGlobal("dependency-upgrade-plan", "Cursor", ".cursor", "upgrade-v1"),
    ownGlobal("dependency-upgrade-plan", "Grok Build", ".grok", "upgrade-v1"),
  ],
  skill_md_tokens: 1_120,
  description_tokens: 10,
  folder_bytes: 78_000,
  file_count: 8,
  installed_at: "2026-05-05T15:20:00.000Z",
  updated_at: "2026-05-19T10:35:00.000Z",
});

/** The size/timestamp fields every filler-skill builder below fills in per skill. */
type SizingFields = Pick<
  InstalledSkill,
  | "skill_md_tokens"
  | "description_tokens"
  | "folder_bytes"
  | "file_count"
  | "installed_at"
  | "updated_at"
>;

function parkedSkill(
  name: string,
  source: string,
  description: string,
  sizing: SizingFields,
): InstalledSkill {
  return skill({
    name,
    source,
    description,
    parked: true,
    parked_at: "2026-08-25T09:00:00.000Z",
    deployments: [],
    content_hash: "",
    content_hashes: [],
    ...sizing,
  });
}

const parkedSkillA = parkedSkill(
  "log-triage",
  "honeycombio/honeycomb-cli",
  "Correlate application logs against a Honeycomb trace.",
  {
    skill_md_tokens: 140,
    description_tokens: 9,
    folder_bytes: 9_000,
    file_count: 2,
    installed_at: "2026-05-12T09:00:00.000Z",
    updated_at: "2026-05-28T14:30:00.000Z",
  },
);
const parkedSkillB = parkedSkill(
  "incident-timeline",
  "pagerduty/incident-workflows",
  "Reconstruct an incident timeline from paging and deploy events.",
  {
    skill_md_tokens: 310,
    description_tokens: 11,
    folder_bytes: 21_000,
    file_count: 4,
    installed_at: "2026-06-01T08:15:00.000Z",
    updated_at: "2026-06-15T11:00:00.000Z",
  },
);
const parkedSkillC = parkedSkill(
  "cost-report",
  "infracost/infracost",
  "Summarize a Terraform plan's infrastructure cost delta.",
  {
    skill_md_tokens: 75,
    description_tokens: 9,
    folder_bytes: 5_200,
    file_count: 1,
    installed_at: "2026-06-09T13:45:00.000Z",
    updated_at: "2026-06-24T16:20:00.000Z",
  },
);

function disabledOwnCopySkill(
  name: string,
  source: string,
  description: string,
  agent: string,
  dir: string,
  sizing: SizingFields,
): InstalledSkill {
  return skill({
    name,
    source,
    description,
    deployments: [globalUniversal(name, "v1"), ownGlobal(name, agent, dir, "v1", true)],
    ...sizing,
  });
}

const disabledOwnCopySkillA = disabledOwnCopySkill(
  "sql-explain",
  "planetscale/cli",
  "Read an EXPLAIN plan and suggest an index.",
  "Codex",
  ".codex",
  {
    skill_md_tokens: 190,
    description_tokens: 9,
    folder_bytes: 13_000,
    file_count: 3,
    installed_at: "2026-07-03T09:10:00.000Z",
    updated_at: "2026-07-17T15:40:00.000Z",
  },
);
const disabledOwnCopySkillB = disabledOwnCopySkill(
  "feature-flag-cleanup",
  "launchdarkly/ldcli",
  "Find and remove a fully-rolled-out feature flag.",
  "OpenCode",
  ".opencode",
  {
    skill_md_tokens: 260,
    description_tokens: 9,
    folder_bytes: 19_000,
    file_count: 3,
    installed_at: "2026-07-08T10:30:00.000Z",
    updated_at: "2026-07-22T09:55:00.000Z",
  },
);
const disabledOwnCopySkillC = disabledOwnCopySkill(
  "bundle-size-audit",
  "webpack/webpack",
  "Track down what grew a JS bundle between two builds.",
  "pi",
  ".pi",
  {
    skill_md_tokens: 830,
    description_tokens: 12,
    folder_bytes: 61_000,
    file_count: 6,
    installed_at: "2026-08-01T12:00:00.000Z",
    updated_at: "2026-08-14T17:25:00.000Z",
  },
);

function brokenLinkSkill(
  name: string,
  source: string,
  description: string,
  sizing: SizingFields,
): InstalledSkill {
  return skill({
    name,
    source,
    description,
    content_hash: "",
    content_hashes: [""],
    deployments: [
      globalUniversal(name, "v1"),
      brokenLink(name, "Claude Code", ".claude", null, ""),
    ],
    ...sizing,
  });
}

const brokenLinkSkillA = brokenLinkSkill(
  "api-contract-diff",
  "stoplightio/spectral",
  "Diff two OpenAPI contracts for a breaking change.",
  {
    skill_md_tokens: 420,
    description_tokens: 9,
    folder_bytes: 30_000,
    file_count: 4,
    installed_at: "2026-08-05T09:20:00.000Z",
    updated_at: "2026-08-19T14:50:00.000Z",
  },
);
const brokenLinkSkillB = brokenLinkSkill(
  "load-test-plan",
  "grafana/k6",
  "Draft a k6 load test plan from expected traffic.",
  {
    skill_md_tokens: 150,
    description_tokens: 10,
    folder_bytes: 10_500,
    file_count: 2,
    installed_at: "2026-08-11T11:05:00.000Z",
    updated_at: "2026-08-26T16:10:00.000Z",
  },
);

function updateAvailableSkill(
  name: string,
  source: string,
  description: string,
  sizing: SizingFields,
): InstalledSkill {
  const deploymentList = [
    globalUniversal(name, "v1"),
    globalLinked(name, "Claude Code", ".claude", "v1"),
  ];
  return skill({
    name,
    source,
    description,
    deployments: deploymentList,
    has_update: true,
    update_owner_ids: [`owner:v1/dotagents/${name}`],
    update_owners: [
      {
        owner_id: `owner:v1/dotagents/${name}`,
        latest_commit: "abc1234",
        latest_commit_at: "2026-09-10T00:00:00.000Z",
      },
    ],
    update_commit: "abc1234",
    update_commit_at: "2026-09-10T00:00:00.000Z",
    ...sizing,
  });
}

const updateAvailableSkillA = updateAvailableSkill(
  "docker-multistage",
  "docker/buildx",
  "Convert a Dockerfile to a leaner multi-stage build.",
  {
    skill_md_tokens: 560,
    description_tokens: 9,
    folder_bytes: 39_000,
    file_count: 5,
    installed_at: "2026-09-02T08:40:00.000Z",
    updated_at: "2026-09-10T00:00:00.000Z",
  },
);
const updateAvailableSkillB = updateAvailableSkill(
  "release-checklist",
  "changesets/changesets",
  "Run a repository's release checklist before publishing.",
  {
    skill_md_tokens: 240,
    description_tokens: 8,
    folder_bytes: 17_000,
    file_count: 3,
    installed_at: "2026-09-06T10:15:00.000Z",
    updated_at: "2026-09-10T00:00:00.000Z",
  },
);

const trialSkill = skill({
  name: "frontend-design",
  source: "anthropics/skills",
  source_url: "https://github.com/anthropics/skills",
  description: "Build distinctive, production-grade frontend interfaces with strong visual craft.",
  deployments: [
    projectUniversal("frontend-design", HARNESS_PROJECT, "frontend-v1"),
    projectLinked("frontend-design", "Claude Code", ".claude", HARNESS_PROJECT, "frontend-v1"),
  ],
  skill_md_tokens: 1_780,
  description_tokens: 12,
  folder_bytes: 130_000,
  file_count: 10,
  installed_at: "2026-09-25T08:00:00.000Z",
  updated_at: "2026-10-09T12:30:00.000Z",
  trial: {
    deployment_id: `harness:${HARNESS_PROJECT}/.agents/skills/frontend-design`,
    expires_at: new Date(CAPTURE_NOW + 20 * 60 * 60_000).toISOString(),
    method: "dotagents",
    scope: "project",
    project_path: HARNESS_PROJECT,
    status: "active",
  },
  trials: [
    {
      deployment_id: `harness:${HARNESS_PROJECT}/.agents/skills/frontend-design`,
      expires_at: new Date(CAPTURE_NOW + 20 * 60 * 60_000).toISOString(),
      method: "dotagents",
      scope: "project",
      project_path: HARNESS_PROJECT,
      status: "active",
    },
  ],
});

function specViolationSkill(
  name: string,
  source: string,
  description: string,
  violation: string,
  sizing: SizingFields,
): InstalledSkill {
  const deploymentList = [globalUniversal(name, "v1")];
  deploymentList[0] = { ...deploymentList[0], spec_violations: [violation] };
  return skill({
    name,
    source,
    description,
    deployments: deploymentList,
    has_spec: false,
    spec_violations: [violation],
    ...sizing,
  });
}

const specViolationSkillA = specViolationSkill(
  "commit-message-lint",
  "conventional-changelog/commitlint",
  "Lint a commit message against a conventional format.",
  "missing required frontmatter field: description",
  {
    skill_md_tokens: 110,
    description_tokens: 9,
    folder_bytes: 7_800,
    file_count: 2,
    installed_at: "2026-10-13T09:00:00.000Z",
    updated_at: "2026-10-27T12:20:00.000Z",
  },
);
const specViolationSkillB = specViolationSkill(
  "readme-audit",
  "othneildrew/best-readme-template",
  "Check a README against a project's documentation checklist.",
  "missing required frontmatter field: name",
  {
    skill_md_tokens: 175,
    description_tokens: 9,
    folder_bytes: 12_000,
    file_count: 2,
    installed_at: "2026-10-18T10:45:00.000Z",
    updated_at: "2026-11-01T15:10:00.000Z",
  },
);

function pluginSkill(
  name: string,
  description: string,
  marketplace: string,
  sizing: SizingFields,
): InstalledSkill {
  return skill({
    name,
    source: "manual",
    source_type: "plugin",
    source_kind: "plugin",
    description,
    deployments: [
      deployment({
        agent: "Claude Code",
        scope: "global",
        path: `${HARNESS_HOME}/.claude/plugins/cache/${marketplace}/${name}/skills/${name}`,
        content_hash: "v1",
        owner_kind: "plugin",
        mutability: "read-only",
        plugin: {
          harness: "Claude Code",
          id: `${name}@${marketplace}`,
          marketplace,
          name,
          version: "1.2.0",
        },
      }),
    ],
    ...sizing,
  });
}

const pluginSkillA = pluginSkill(
  "git-worktree-helper",
  "Manage parallel git worktrees for concurrent branches.",
  "anthropic-plugins",
  {
    skill_md_tokens: 300,
    description_tokens: 8,
    folder_bytes: 21_000,
    file_count: 4,
    installed_at: "2026-11-04T08:30:00.000Z",
    updated_at: "2026-11-18T13:15:00.000Z",
  },
);
const pluginSkillB = pluginSkill(
  "changelog-formatter",
  "Format a changelog entry to Keep a Changelog style.",
  "community-plugins",
  {
    skill_md_tokens: 130,
    description_tokens: 10,
    folder_bytes: 9_200,
    file_count: 2,
    installed_at: "2026-11-09T11:50:00.000Z",
    updated_at: "2026-11-23T16:40:00.000Z",
  },
);

const manualSkill = skill({
  name: "internal-runbook",
  source: "manual",
  source_kind: "manual",
  description: "Follow this team's internal on-call runbook.",
  deployments: [ownGlobal("internal-runbook", "Claude Code", ".claude", "v1")],
  skill_md_tokens: 890,
  description_tokens: 8,
  folder_bytes: 64_000,
  file_count: 7,
  installed_at: "2026-01-06T09:15:00.000Z",
  updated_at: "2026-01-20T14:45:00.000Z",
});

const driftSkill = skill({
  name: "api-client-codegen",
  source: "openapi-generator/openapi-generator",
  source_url: "https://github.com/openapi-generator/openapi-generator",
  description: "Regenerate a typed API client from an OpenAPI schema.",
  deployments: [
    globalUniversal("api-client-codegen", "codegen-global-v1"),
    projectUniversal("api-client-codegen", HARNESS_PROJECT, "codegen-project-v2"),
  ],
  skill_md_tokens: 1_240,
  description_tokens: 10,
  folder_bytes: 88_000,
  file_count: 8,
  installed_at: "2026-02-10T10:00:00.000Z",
  updated_at: "2026-02-24T15:35:00.000Z",
});

const noDescriptionSkill = skill({
  name: "worktree-cleanup",
  source: "manual",
  description: null,
  deployments: [globalUniversal("worktree-cleanup", "v1")],
  skill_md_tokens: 95,
  description_tokens: 0,
  folder_bytes: 6_800,
  file_count: 2,
  installed_at: "2026-03-13T08:20:00.000Z",
  updated_at: "2026-03-27T12:00:00.000Z",
});

const longNameSkill = skill({
  name: "cross-repository-dependency-graph-builder",
  source: "dependabot/dependabot-core",
  source_url: "https://github.com/dependabot/dependabot-core",
  description: "Build a dependency graph spanning every repository in an org.",
  deployments: [globalUniversal("cross-repository-dependency-graph-builder", "v1")],
  skill_md_tokens: 520,
  description_tokens: 12,
  folder_bytes: 36_000,
  file_count: 5,
  installed_at: "2026-04-06T09:40:00.000Z",
  updated_at: "2026-04-20T14:10:00.000Z",
});

const longDescriptionSkill = skill({
  name: "incident-postmortem",
  source: "google/sre-workbook",
  source_url: "https://github.com/google/sre-workbook",
  description:
    "Write a blameless incident postmortem that reconstructs the full timeline from detection through resolution, names the contributing factors without assigning fault to any one engineer, lists concrete follow-up action items with owners and due dates, and links every claim back to a dashboard, log line, or page so a reader six months from now can verify it without re-interviewing anyone.",
  deployments: [globalUniversal("incident-postmortem", "v1")],
  skill_md_tokens: 990,
  description_tokens: 60,
  folder_bytes: 70_000,
  file_count: 6,
  installed_at: "2026-05-11T11:15:00.000Z",
  updated_at: "2026-05-25T16:50:00.000Z",
});

// --- Simple filler skills with zero invocations, rounding the estate out
// to a realistic size -----------------------------------------------------

interface FillerSpec {
  name: string;
  source: string;
  description: string;
  skillMdTokens: number;
  descriptionTokens: number;
  folderBytes: number;
  fileCount: number;
  installedAt: string;
  updatedAt: string;
}

const fillerSpecs: FillerSpec[] = [
  {
    name: "eslint-config-audit",
    source: "eslint/eslint",
    description: "Audit an ESLint config against a team's style guide.",
    skillMdTokens: 165,
    descriptionTokens: 10,
    folderBytes: 11_500,
    fileCount: 2,
    installedAt: "2026-06-03T09:00:00.000Z",
    updatedAt: "2026-06-17T14:20:00.000Z",
  },
  {
    name: "storybook-story-gen",
    source: "storybookjs/storybook",
    description: "Generate a Storybook story from a component's props.",
    skillMdTokens: 210,
    descriptionTokens: 9,
    folderBytes: 15_000,
    fileCount: 3,
    installedAt: "2026-06-08T10:30:00.000Z",
    updatedAt: "2026-06-22T15:45:00.000Z",
  },
  {
    name: "graphql-schema-diff",
    source: "graphql/graphql-js",
    description: "Diff two GraphQL schemas for a breaking field change.",
    skillMdTokens: 350,
    descriptionTokens: 10,
    folderBytes: 24_000,
    fileCount: 4,
    installedAt: "2026-07-02T08:45:00.000Z",
    updatedAt: "2026-07-16T13:05:00.000Z",
  },
  {
    name: "terraform-plan-review",
    source: "hashicorp/terraform",
    description: "Review a Terraform plan for destructive changes.",
    skillMdTokens: 480,
    descriptionTokens: 8,
    folderBytes: 33_000,
    fileCount: 5,
    installedAt: "2026-07-09T11:20:00.000Z",
    updatedAt: "2026-07-23T16:35:00.000Z",
  },
  {
    name: "kubernetes-manifest-lint",
    source: "kubernetes/kubectl",
    description: "Lint a Kubernetes manifest against best practices.",
    skillMdTokens: 125,
    descriptionTokens: 8,
    folderBytes: 8_800,
    fileCount: 2,
    installedAt: "2026-08-04T09:50:00.000Z",
    updatedAt: "2026-08-18T14:15:00.000Z",
  },
  {
    name: "figma-token-sync",
    source: "figma/plugin-samples",
    description: "Sync design tokens from Figma into a codebase.",
    skillMdTokens: 615,
    descriptionTokens: 9,
    folderBytes: 43_000,
    fileCount: 5,
    installedAt: "2026-09-01T10:10:00.000Z",
    updatedAt: "2026-09-11T15:30:00.000Z",
  },
  {
    name: "cron-schedule-explain",
    source: "robfig/cron",
    description: "Explain a cron schedule in plain language.",
    skillMdTokens: 40,
    descriptionTokens: 8,
    folderBytes: 2_800,
    fileCount: 1,
    installedAt: "2026-12-02T08:30:00.000Z",
    updatedAt: "2026-12-16T13:50:00.000Z",
  },
];

const fillerSkills = fillerSpecs.map((spec) =>
  skill({
    name: spec.name,
    source: spec.source,
    source_url: `https://github.com/${spec.source}`,
    description: spec.description,
    deployments: [globalUniversal(spec.name, "v1")],
    skill_md_tokens: spec.skillMdTokens,
    description_tokens: spec.descriptionTokens,
    folder_bytes: spec.folderBytes,
    file_count: spec.fileCount,
    installed_at: spec.installedAt,
    updated_at: spec.updatedAt,
  }),
);

// --- Invocation history: a seeded year of uses across every harness, so the
// Activity page's heatmap, by-skill/by-project lists, and docked panel all
// have something to show without a real disk scan.

/** Every fixture skill that gets simulated uses, busiest first by real-world plausibility. */
const INVOCATION_SKILLS = [
  "commit",
  "code-review",
  "agent-browser",
  "release-notes",
  "pdf-processing",
  "sdk-migration",
  "sentry-triage",
  "prompt-caching",
  "filesystem-audit",
  "changelog-draft",
  "test-flake-hunt",
  "schema-migration-check",
  "dependency-upgrade-plan",
  "frontend-design",
  "internal-runbook",
  "api-client-codegen",
  "worktree-cleanup",
  "cross-repository-dependency-graph-builder",
  "incident-postmortem",
  "eslint-config-audit",
  "storybook-story-gen",
  "graphql-schema-diff",
  "terraform-plan-review",
  "kubernetes-manifest-lint",
  "figma-token-sync",
  "cron-schedule-explain",
];

const INVOCATION_PROJECTS = [
  HARNESS_PROJECT,
  ACME_PROJECT,
  PERSONAL_PROJECT,
  BACKEND_PROJECT,
  MOBILE_PROJECT,
];

type FixtureTrigger = "user" | "agent" | "file_read";
type FixtureHarness = "claude-code" | "codex" | "open-code" | "pi" | "cursor" | "grok-build";

const DAY_COUNT = 364;

/** The UTC "YYYY-MM-DD" key for the day `daysAgo` days before `CAPTURE_NOW`'s date, 0 = today. */
function fixtureDateKey(daysAgo: number): string {
  const date = new Date(CAPTURE_NOW);
  date.setUTCHours(0, 0, 0, 0);
  date.setUTCDate(date.getUTCDate() - daysAgo);
  return date.toISOString().slice(0, 10);
}

/** Deterministic PRNG (mulberry32) so the fixture's numbers never change between reloads. */
function mulberry32(seed: number): () => number {
  let a = seed;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function cumulate(weights: number[]): number[] {
  let sum = 0;
  return weights.map((w) => (sum += w));
}

function pickWeighted(rand: () => number, cumulative: number[]): number {
  const target = rand() * cumulative[cumulative.length - 1];
  let lo = 0;
  let hi = cumulative.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (cumulative[mid] < target) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

interface HarnessPlan {
  id: FixtureHarness;
  perYear: number;
  mix: [FixtureTrigger, number][];
  /** How many of the ranked skills this harness ever uses. */
  skillCount: number;
  /** 0..1 relative activity for `daysAgo` (0 = today, DAY_COUNT - 1 = oldest). */
  weight: (daysAgo: number) => number;
}

const HARNESS_PLANS: HarnessPlan[] = [
  {
    id: "claude-code",
    perYear: 5100,
    mix: [
      ["agent", 0.68],
      ["user", 0.32],
    ],
    skillCount: INVOCATION_SKILLS.length,
    // Ramps up toward the present, like a project that has picked up steam.
    weight: (daysAgo) => 0.55 + (0.45 * (DAY_COUNT - 1 - daysAgo)) / DAY_COUNT,
  },
  {
    id: "codex",
    perYear: 1450,
    mix: [
      ["file_read", 0.94],
      ["user", 0.06],
    ],
    skillCount: 18,
    weight: (daysAgo) => (daysAgo > 240 ? 0 : daysAgo < 60 ? 1 : 0.4),
  },
  {
    id: "open-code",
    perYear: 780,
    mix: [
      ["agent", 0.85],
      ["file_read", 0.14],
      ["user", 0.01],
    ],
    skillCount: 12,
    weight: (daysAgo) => (daysAgo > 300 ? 0 : daysAgo > 120 ? 1 : 0.2),
  },
  {
    id: "pi",
    perYear: 240,
    mix: [["file_read", 1]],
    skillCount: 7,
    weight: (daysAgo) => (daysAgo > 110 ? 0 : 1),
  },
  {
    id: "cursor",
    perYear: 110,
    mix: [["file_read", 1]],
    skillCount: 5,
    weight: (daysAgo) => (daysAgo % 29 < 6 ? 1 : 0.1),
  },
  {
    id: "grok-build",
    perYear: 95,
    mix: [["file_read", 1]],
    skillCount: 4,
    weight: (daysAgo) => (daysAgo % 23 < 5 ? 1 : 0.1),
  },
];

interface FixtureUse {
  daysAgo: number;
  /** 0-23, local to the fixture's own clock (UTC). */
  hour: number;
  skill: string;
  harness: FixtureHarness;
  trigger: FixtureTrigger;
  project: string;
}

/** One seeded year of uses across every harness: daytime/evening-weighted hours, a busier
 * recent quarter, and each harness favoring its own slice of the skill list. */
function generateInvocations(): FixtureUse[] {
  const rand = mulberry32(20260916);
  const dayWeight = Array.from({ length: DAY_COUNT }, (_, daysAgo) => {
    const weekday = new Date(CAPTURE_NOW - daysAgo * 86_400_000).getUTCDay();
    const base = weekday === 0 ? 0.25 : weekday === 6 ? 0.4 : 1;
    return base * (0.7 + rand() * 0.6);
  });
  const projectCumulative = cumulate([10, 6, 3, 2, 1]);
  // Daytime and evening clustering: 8am-11pm, weighted toward the working day with an evening tail.
  const hourWeights = Array.from({ length: 16 }, (_, i) => {
    const hour = 8 + i;
    return hour <= 18 ? 3 : hour <= 21 ? 2 : 1;
  });
  const hourCumulative = cumulate(hourWeights);

  const uses: FixtureUse[] = [];
  for (const plan of HARNESS_PLANS) {
    const days = cumulate(dayWeight.map((w, daysAgo) => w * plan.weight(daysAgo)));
    // Each harness favors a different slice of the skill list - shuffle before slicing.
    const shuffled = [...INVOCATION_SKILLS];
    for (let i = 0; i < shuffled.length; i++) {
      if (rand() < 0.35) {
        const j = Math.floor(rand() * shuffled.length);
        [shuffled[i], shuffled[j]] = [shuffled[j], shuffled[i]];
      }
    }
    const skills = shuffled.slice(0, plan.skillCount);
    const skillCumulative = cumulate(skills.map((_, rank) => 1 / Math.pow(rank + 1, 1.05)));
    const triggerCumulative = cumulate(plan.mix.map(([, share]) => share));
    for (let n = 0; n < plan.perYear; n++) {
      const daysAgo = DAY_COUNT - 1 - pickWeighted(rand, days);
      uses.push({
        daysAgo,
        hour: 8 + pickWeighted(rand, hourCumulative),
        skill: skills[pickWeighted(rand, skillCumulative)],
        harness: plan.id,
        trigger: plan.mix[pickWeighted(rand, triggerCumulative)][0],
        project: INVOCATION_PROJECTS[pickWeighted(rand, projectCumulative)],
      });
    }
  }
  return uses;
}

const FIXTURE_USES = generateInvocations();

/** Epoch-hour timestamp for one generated use, matching `SkillUseHour.hour`'s meaning. */
function epochHourOf(use: FixtureUse): number {
  const date = new Date(CAPTURE_NOW);
  date.setUTCHours(0, 0, 0, 0);
  date.setUTCDate(date.getUTCDate() - use.daysAgo);
  return Math.floor(date.getTime() / 3_600_000) + use.hour;
}

interface FixtureSkillStats {
  total: number;
  last_used: string | null;
  last_24_hours: number;
  last_7_days: number;
  last_14_days: number;
  last_30_days: number;
  by_day: Record<string, number>;
  by_harness_30_days: Record<string, number>;
  by_project_30_days: Record<string, number>;
  by_trigger_30_days: Record<FixtureTrigger, number>;
  by_hour: {
    hour: number;
    harness: string;
    trigger: FixtureTrigger;
    project_path: string | null;
    count: number;
  }[];
}

const CAPTURE_HOUR = Math.floor(CAPTURE_NOW / 3_600_000);
const WINDOW_HOURS = {
  "24h": 24,
  "7d": 7 * 24,
  "14d": 14 * 24,
  "30d": 30 * 24,
} satisfies Record<string, number>;

/** Every generated use, aggregated into the exact shape `skill_stats()` produces on the Rust
 * side, so the fixture's totals, windows, and hourly buckets always agree with each other. */
function buildInvocationStats(): Map<string, FixtureSkillStats> {
  const bySkill = new Map<string, FixtureSkillStats>();
  const hourBuckets = new Map<string, Map<string, FixtureSkillStats["by_hour"][number]>>();

  for (const use of FIXTURE_USES) {
    let stats = bySkill.get(use.skill);
    if (!stats) {
      stats = {
        total: 0,
        last_used: null,
        last_24_hours: 0,
        last_7_days: 0,
        last_14_days: 0,
        last_30_days: 0,
        by_day: {},
        by_harness_30_days: {},
        by_project_30_days: {},
        by_trigger_30_days: { user: 0, agent: 0, file_read: 0 },
        by_hour: [],
      };
      bySkill.set(use.skill, stats);
      hourBuckets.set(use.skill, new Map());
    }

    const epochHour = epochHourOf(use);
    const iso = new Date(epochHour * 3_600_000).toISOString();
    stats.total++;
    if (!stats.last_used || iso > stats.last_used) stats.last_used = iso;
    const dayKey = fixtureDateKey(use.daysAgo);
    stats.by_day[dayKey] = (stats.by_day[dayKey] ?? 0) + 1;

    const hoursAgo = CAPTURE_HOUR - epochHour;
    if (hoursAgo < WINDOW_HOURS["24h"]) stats.last_24_hours++;
    if (hoursAgo < WINDOW_HOURS["7d"]) stats.last_7_days++;
    if (hoursAgo < WINDOW_HOURS["14d"]) stats.last_14_days++;
    if (hoursAgo < WINDOW_HOURS["30d"]) {
      stats.last_30_days++;
      stats.by_harness_30_days[use.harness] = (stats.by_harness_30_days[use.harness] ?? 0) + 1;
      stats.by_project_30_days[use.project] = (stats.by_project_30_days[use.project] ?? 0) + 1;
      stats.by_trigger_30_days[use.trigger]++;
    }

    const buckets = hourBuckets.get(use.skill)!;
    const bucketKey = `${epochHour}|${use.harness}|${use.trigger}|${use.project}`;
    const bucket = buckets.get(bucketKey);
    if (bucket) {
      bucket.count++;
    } else {
      buckets.set(bucketKey, {
        hour: epochHour,
        harness: use.harness,
        trigger: use.trigger,
        project_path: use.project,
        count: 1,
      });
    }
  }

  for (const [skillName, buckets] of hourBuckets) {
    const stats = bySkill.get(skillName);
    if (stats) stats.by_hour = [...buckets.values()];
  }
  return bySkill;
}

const invocationStatsBySkill = buildInvocationStats();

const heatmapDays: Record<string, number> = {};
for (const stats of invocationStatsBySkill.values()) {
  for (const [date, count] of Object.entries(stats.by_day)) {
    heatmapDays[date] = (heatmapDays[date] ?? 0) + count;
  }
}

const allSkills: InstalledSkill[] = [
  commitSkill,
  reviewSkill,
  browserSkill,
  repairSkill,
  globalOnlySkill,
  projectOnlySkill,
  onePlusProjectSkill,
  twoPlusProjectSkill,
  fourPlusProjectSkill,
  oneHarnessSkill,
  threeHarnessSkill,
  fiveHarnessSkill,
  sevenHarnessSkill,
  parkedSkillA,
  parkedSkillB,
  parkedSkillC,
  disabledOwnCopySkillA,
  disabledOwnCopySkillB,
  disabledOwnCopySkillC,
  brokenLinkSkillA,
  brokenLinkSkillB,
  updateAvailableSkillA,
  updateAvailableSkillB,
  trialSkill,
  specViolationSkillA,
  specViolationSkillB,
  pluginSkillA,
  pluginSkillB,
  manualSkill,
  driftSkill,
  noDescriptionSkill,
  longNameSkill,
  longDescriptionSkill,
  ...fillerSkills,
];

/** Repeats the filler skills under numbered names until the estate holds `count` skills, so
 * performance runs can use a realistic size (the real estate measured 378). */
function padSkills(count: number): InstalledSkill[] {
  const padded = [...allSkills];
  for (let index = 0; padded.length < count; index++) {
    const base = fillerSkills[index % fillerSkills.length];
    const name = `${base.name}-${index + 2}`;
    padded.push({ ...base, name, deployments: [globalUniversal(name, "v1")] });
  }
  return padded;
}

/** Builds a fresh copy of the harness's skill snapshot - a new object each
 * call, so mutating it in `mock-tauri.ts` never leaks back into this fixture.
 * `skillCount` pads the estate with copies of the filler skills. */
export function buildHarnessSnapshot(skillCount = 0): SkillSnapshot {
  return {
    revision: 1,
    skills: padSkills(skillCount),
    projects: [HARNESS_PROJECT, ACME_PROJECT, PERSONAL_PROJECT, BACKEND_PROJECT, MOBILE_PROJECT],
    invocations: [...invocationStatsBySkill.entries()].map(([skill, stats]) => ({
      skill,
      total: stats.total,
      last_24_hours: stats.last_24_hours,
      last_7_days: stats.last_7_days,
      last_14_days: stats.last_14_days,
      last_30_days: stats.last_30_days,
      last_used: stats.last_used,
      by_project_30_days: stats.by_project_30_days,
      by_day: stats.by_day,
      by_harness_30_days: stats.by_harness_30_days,
      by_trigger_30_days: stats.by_trigger_30_days,
      by_hour: stats.by_hour,
    })),
    heatmap: { days: heatmapDays },
    scanned_at: SCANNED_AT,
    scan_observations: [],
    scan_partial: false,
    unread_roots: [],
    last_test_by_skill: {},
    update_check: { checked_at: SCANNED_AT, gh_status: "ok", message: null, updates_available: 2 },
    opencode_config_kind: "json",
  };
}

// --- SKILL.md bodies -------------------------------------------------------

export const harnessSkillContent = new Map<string, string>([
  [
    `${HARNESS_HOME}/.agents/skills/commit/SKILL.md`,
    `---\nname: commit\ndescription: Create clear commits\n---\n\n# Commit\n\n- Inspect the diff.\n- Stage one focused change.\n- Use a conventional commit message.\n`,
  ],
  [
    `${HARNESS_HOME}/.claude/skills/commit/SKILL.md`,
    `---\nname: commit\ndescription: Create clear commits\n---\n\n# Commit\n\n- Inspect the diff.\n- Stage one focused change.\n- Use a conventional commit message.\n`,
  ],
  [
    `${HARNESS_PROJECT}/.agents/skills/commit/SKILL.md`,
    `---\nname: commit\ndescription: Create commits for Skill Studio\ndisable-model-invocation: true\n---\n\n# Commit\n\n- Run the scoped checks.\n- Stage one focused change.\n- Include the issue number in the message.\n`,
  ],
  [
    `${HARNESS_PROJECT}/.claude/skills/commit/SKILL.md`,
    `---\nname: commit\ndescription: Create commits for Skill Studio\ndisable-model-invocation: true\n---\n\n# Commit\n\n- Run the scoped checks.\n- Stage one focused change.\n- Include the issue number in the message.\n`,
  ],
  [
    `${HARNESS_PROJECT}/.codex/skills/commit/SKILL.md`,
    `---\nname: commit\ndescription: Create commits for Skill Studio\n---\n\n# Commit\n\n- Run formatting and types.\n- Keep the commit focused.\n- Include the issue number in the message.\n`,
  ],
  [
    `${HARNESS_HOME}/.agents/skills/code-review/SKILL.md`,
    `---\nname: code-review\ndescription: Review repository changes\n---\n\n# Code review\n\nReport concrete findings with file and line evidence.\n`,
  ],
  [
    `${HARNESS_HOME}/.agents/skills/agent-browser/SKILL.md`,
    `---\nname: agent-browser\ndescription: Verify browser workflows\n---\n\n# Browser verification\n\nUse accessible controls and capture evidence.\n`,
  ],
  [
    `${HARNESS_HOME}/.agents/skills/release-notes/SKILL.md`,
    `---\nname: release-notes\ndescription: Prepare accurate release notes\n---\n\n# Release notes\n\nSummarize the shipped behavior and link each change to evidence.\n`,
  ],
  [
    `${HARNESS_PROJECT}/.agents/skills/frontend-design/SKILL.md`,
    `---\nname: frontend-design\ndescription: Build polished interfaces\n---\n\n# Frontend design\n\nCreate a clear hierarchy and verify the rendered result.\n`,
  ],
]);

/** A short generic body for every fixture skill without a hand-written one above. */
export function fallbackSkillContent(name: string): string {
  return `---\nname: ${name}\ndescription: Fixture skill\n---\n\n# ${name}\n\nThis is placeholder content for the harness fixture.\n`;
}
