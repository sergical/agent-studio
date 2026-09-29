// ============================================================================
// Skill Studio - scanned-deployment
// Builds `Deployment` rows in the shapes the scanner produces, for tests and
// the dev harness. `candidateIdentity` mirrors `id_for_candidate` in
// src-tauri/src/skills/skill_deployment.rs: the id, destination, and backing
// all follow from where the entry sits and how it reaches the Universal
// folder. `scannerMismatches` names every field that disagrees with it.
// ============================================================================

import type { Deployment } from "@skill-studio/lib";

/** What `skill_deployment.rs`'s `harness_slot` returns for each root label. */
const SLOT_FOR_ROOT_LABEL = new Map([
  ["Claude Code", "claude-code"],
  ["Codex", "codex"],
  ["OpenCode", "opencode"],
  ["pi", "pi"],
  ["Cursor", "cursor"],
  ["Grok Build", "grok-build"],
  ["shared", "universal"],
  ["universal", "universal"],
  ["parked", "universal"],
]);

/** A harness root label, as the scanner writes it into `Deployment.agent`. */
type HarnessLabel = "Claude Code" | "Codex" | "OpenCode" | "pi" | "Cursor" | "Grok Build";

/** The identity inputs of one scanned entry - `DeploymentCandidate` in Rust. */
interface DeploymentCandidate {
  name: string;
  agent: string;
  scope: string;
  path: string;
  project_path?: string | null;
  is_symlink: boolean;
  symlink_target?: string | null;
  resolved_path?: string | null;
  shared_via_whole_dir_link: boolean;
}

type CandidateIdentity = Pick<Deployment, "id" | "destination" | "backing">;

function encodeIdPath(path: string): string {
  return path.replace(/%/g, "%25").replace(/\//g, "%2F");
}

function isUnderUniversalSkills(path: string): boolean {
  const parts = path.split("/").filter(Boolean);
  return parts.some((part, i) => part === ".agents" && parts[i + 1] === "skills");
}

function deploymentId(
  name: string,
  scope: string,
  destination: Deployment["destination"],
  slot: string,
  projectPath: string | null | undefined,
  lexicalEntry: string,
): string {
  const project = projectPath ? encodeIdPath(projectPath) : "-";
  return `dep:v1/${scope}/${slot}/${destination}/${name}/${project}/${encodeIdPath(lexicalEntry)}`;
}

/** `id_for_candidate`, line for line. */
function candidateIdentity(candidate: DeploymentCandidate): CandidateIdentity {
  const { name, agent, scope, path, project_path } = candidate;
  if (agent === "shared" || agent === "universal" || scope === "parked") {
    return {
      id: deploymentId(name, scope, "universal", "universal", project_path, path),
      destination: "universal",
      backing: { kind: "canonical" },
    };
  }
  const slot = SLOT_FOR_ROOT_LABEL.get(agent) ?? "other";
  const linked =
    candidate.shared_via_whole_dir_link ||
    (candidate.is_symlink &&
      candidate.symlink_target != null &&
      isUnderUniversalSkills(candidate.symlink_target));
  if (linked) {
    const canonicalPath = candidate.resolved_path ?? candidate.symlink_target ?? path;
    return {
      id: deploymentId(name, scope, "universal", slot, project_path, path),
      destination: "universal",
      backing: {
        kind: "linked-to",
        deployment_id: deploymentId(
          name,
          scope,
          "universal",
          "universal",
          project_path,
          canonicalPath,
        ),
      },
    };
  }
  return {
    id: deploymentId(name, scope, "per-harness", slot, project_path, path),
    destination: "per-harness",
    backing: { kind: "independent" },
  };
}

/** The skill name the scanner uses for an entry: its folder name. */
function entryName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/**
 * Every way `deployment` differs from a row the scanner could produce: the
 * identity fields must match `candidateIdentity` of its own inputs, and the
 * link fields must fit together (a whole-folder entry is a real folder, so it
 * has no `symlink_target`; a real folder has no link fields at all).
 */
export function scannerMismatches(deployment: Deployment): string[] {
  const problems: string[] = [];
  const expected = candidateIdentity({ ...deployment, name: entryName(deployment.path) });
  if (deployment.id !== expected.id) {
    problems.push(`id is ${deployment.id}, the scanner derives ${expected.id}`);
  }
  if (deployment.destination !== expected.destination) {
    problems.push(`destination is ${deployment.destination}, expected ${expected.destination}`);
  }
  if (JSON.stringify(deployment.backing) !== JSON.stringify(expected.backing)) {
    problems.push(
      `backing is ${JSON.stringify(deployment.backing)}, expected ${JSON.stringify(expected.backing)}`,
    );
  }
  if (deployment.shared_via_whole_dir_link && deployment.is_symlink) {
    problems.push("a whole-folder entry is read through its parent link, so is_symlink is false");
  }
  if (deployment.shared_via_whole_dir_link && deployment.symlink_target) {
    problems.push("a whole-folder entry has no symlink_target of its own");
  }
  if (!deployment.is_symlink && deployment.symlink_target) {
    problems.push("symlink_target is set on an entry that is not a link");
  }
  if (deployment.symlink_is_broken && !deployment.is_symlink) {
    problems.push("symlink_is_broken is set on an entry that is not a link");
  }
  if ((deployment.scope === "project") !== Boolean(deployment.project_path)) {
    problems.push("project_path must be set for a project row and only for one");
  }
  return problems;
}

/** Fields that do not take part in identity, with the scanner's defaults. */
const ROW_DEFAULTS = {
  owner_kind: "manual",
  mutability: "mutable",
  symlink_is_broken: false,
  content_hash: "v1",
  disabled: false,
  disabled_by: null,
  codex_implicit_invocation: null,
  invocation: "both",
  spec_violations: [],
} satisfies Partial<Deployment>;

/** Recomputes `id`, `destination`, and `backing` from the row's own fields. */
export function withScannerIdentity(deployment: Deployment): Deployment {
  return {
    ...deployment,
    ...candidateIdentity({ ...deployment, name: entryName(deployment.path) }),
  };
}

type RowFields = Partial<Omit<Deployment, "id" | "destination" | "backing">>;

function row(candidate: Omit<DeploymentCandidate, "name">, fields: RowFields): Deployment {
  return withScannerIdentity({
    id: "",
    destination: "universal",
    backing: { kind: "canonical" },
    ...ROW_DEFAULTS,
    ...candidate,
    ...fields,
  });
}

interface LayoutInput {
  /** The Universal folder entry, e.g. `/home/.agents/skills/find-bugs`. */
  universalPath: string;
  scope?: "global" | "project";
  project_path?: string | null;
}

/** The canonical folder in `.agents/skills`. */
export function universalDeployment(input: LayoutInput, fields: RowFields = {}): Deployment {
  return row(
    {
      agent: "shared",
      scope: input.scope ?? "global",
      path: input.universalPath,
      project_path: input.project_path ?? null,
      is_symlink: false,
      shared_via_whole_dir_link: false,
    },
    { owner_kind: "dotagents", ...fields },
  );
}

/** A per-skill link in a real harness folder: `<root>/<name> -> .agents/skills/<name>`. */
export function perSkillLinkDeployment(
  input: LayoutInput & { agent: HarnessLabel; path: string },
  fields: RowFields = {},
): Deployment {
  return row(
    {
      agent: input.agent,
      scope: input.scope ?? "global",
      path: input.path,
      project_path: input.project_path ?? null,
      is_symlink: true,
      symlink_target: input.universalPath,
      resolved_path: input.universalPath,
      shared_via_whole_dir_link: false,
    },
    fields,
  );
}

/** An entry read through a whole-folder link: `<root> -> .agents/skills`. */
export function wholeFolderDeployment(
  input: LayoutInput & { agent: HarnessLabel; path: string },
  fields: RowFields = {},
): Deployment {
  return row(
    {
      agent: input.agent,
      scope: input.scope ?? "global",
      path: input.path,
      project_path: input.project_path ?? null,
      is_symlink: false,
      resolved_path: input.universalPath,
      shared_via_whole_dir_link: true,
    },
    fields,
  );
}

/** A real folder of the harness's own, independent of the Universal folder. */
export function realCopyDeployment(
  input: {
    agent: HarnessLabel;
    path: string;
    scope?: "global" | "project";
    project_path?: string | null;
  },
  fields: RowFields = {},
): Deployment {
  return row(
    {
      agent: input.agent,
      scope: input.scope ?? "global",
      path: input.path,
      project_path: input.project_path ?? null,
      is_symlink: false,
      shared_via_whole_dir_link: false,
    },
    fields,
  );
}
