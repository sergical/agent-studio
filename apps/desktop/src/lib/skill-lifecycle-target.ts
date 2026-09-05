import { agentIdFromDeploymentLabel, parentDirectory } from "@skill-studio/lib";
import type {
  Deployment,
  InstalledSkill,
  InstallScope,
  LifecycleTarget,
  TrialInfo,
} from "@skill-studio/lib";

type SkillLifecycleView = Pick<InstalledSkill, "name" | "deployments" | "source_kind">;

export interface SkillLifecycleScopeSelection {
  skillName: string;
  scope: InstallScope;
  projectPath: string | null;
}

export interface SkillRemovalPreview {
  target: LifecycleTarget;
  managedDeployments: Deployment[];
  linkedDeployments: Deployment[];
}

export type SkillRemovalAvailability =
  | { available: true; preview: SkillRemovalPreview }
  | { available: false; reason: string };

export interface SkillOwnerUpdateFailure {
  ownerId: string;
  message: string;
}

export interface SkillOwnerUpdateSummary {
  attempted: number;
  succeeded: number;
  failures: SkillOwnerUpdateFailure[];
}

export type SkillUpdateAvailability =
  | { available: true; target: LifecycleTarget }
  | { available: false; reason: string };

/** List only scopes that contain a mutable installed deployment. */
export function skillMutableLifecycleScopes(
  skill: SkillLifecycleView,
): SkillLifecycleScopeSelection[] {
  const selections: SkillLifecycleScopeSelection[] = [];
  const keys = new Set<string>();
  for (const deployment of skill.deployments) {
    if (deployment.mutability !== "mutable") continue;
    if (deployment.scope !== "global" && deployment.scope !== "project") continue;
    const projectPath = deployment.scope === "project" ? (deployment.project_path ?? null) : null;
    if (deployment.scope === "project" && !projectPath) continue;
    const key = `${deployment.scope}:${projectPath ?? ""}`;
    if (keys.has(key)) continue;
    keys.add(key);
    selections.push({ skillName: skill.name, scope: deployment.scope, projectPath });
  }
  return selections.sort((left, right) => {
    if (left.scope !== right.scope) return left.scope === "global" ? -1 : 1;
    return (left.projectPath ?? "").localeCompare(right.projectPath ?? "");
  });
}

/** Keep a valid scope selection for this skill, or select its first mutable scope. */
export function skillLifecycleScopeSelection(
  skill: SkillLifecycleView,
  current?: SkillLifecycleScopeSelection | null,
): SkillLifecycleScopeSelection | null {
  const selections = skillMutableLifecycleScopes(skill);
  if (current?.skillName === skill.name) {
    const match = selections.find(
      (selection) =>
        selection.scope === current.scope && selection.projectPath === current.projectPath,
    );
    if (match) return match;
  }
  return selections[0] ?? null;
}

/** Target the exact deployment selected in the UI. */
export function lifecycleTargetForDeployment(deployment: Deployment): LifecycleTarget {
  return { deployment_id: deployment.id };
}

/** Resolve the exact whole-directory-link deployment behind a linked-root repair row. */
export function lifecycleTargetForHarnessRoot(
  skill: SkillLifecycleView,
  harness: string,
  root: string,
): LifecycleTarget {
  const deployment = skill.deployments.find(
    (candidate) =>
      candidate.shared_via_whole_dir_link &&
      agentIdFromDeploymentLabel(candidate.agent) === harness &&
      parentDirectory(candidate.path) === root,
  );
  if (!deployment) {
    throw new Error(`${skill.name} has no ${harness} whole-directory link at ${root}`);
  }
  return lifecycleTargetForDeployment(deployment);
}

function deploymentsInScope(
  skill: Pick<SkillLifecycleView, "deployments">,
  scope: "global" | "project",
  projectPath?: string | null,
): Deployment[] {
  return skill.deployments.filter(
    (deployment) =>
      deployment.scope === scope && (scope === "global" || deployment.project_path === projectPath),
  );
}

/** Resolve an aggregate skill only when one mutable owner matches the requested scope. */
export function lifecycleTargetForSkill(
  skill: SkillLifecycleView,
  scope: "global" | "project",
  projectPath?: string | null,
): LifecycleTarget {
  const deployments = deploymentsInScope(skill, scope, projectPath).filter(
    (deployment) => deployment.mutability === "mutable",
  );
  const ownerIds = [...new Set(deployments.flatMap((deployment) => deployment.owner_id ?? []))];
  if (ownerIds.length === 1) return { owner_id: ownerIds[0] };
  if (ownerIds.length > 1) {
    throw new Error(`${skill.name} has multiple lifecycle owners in this scope`);
  }
  const canonicalDeployments = deployments.filter(
    (deployment) => deployment.backing.kind === "canonical",
  );
  if (canonicalDeployments.length === 1) {
    return lifecycleTargetForDeployment(canonicalDeployments[0]);
  }
  if (deployments.length === 1) return lifecycleTargetForDeployment(deployments[0]);
  if (skill.deployments.length === 0 && skill.source_kind === "skills-sh" && scope === "global") {
    return { owner_id: `owner:v1/global/${skill.name}` };
  }
  throw new Error(`${skill.name} has no unambiguous mutable deployment in this scope`);
}

/** Resolve aggregate removal without throwing during render. */
export function skillRemovalAvailability(
  skill: SkillLifecycleView,
  selection: SkillLifecycleScopeSelection,
): SkillRemovalAvailability {
  try {
    return { available: true, preview: skillRemovalPreview(skill, selection) };
  } catch (error) {
    return {
      available: false,
      reason:
        error instanceof Error
          ? `${error.message}. Manage each independent Copy deployment in Locations.`
          : "Manage each independent Copy deployment in Locations.",
    };
  }
}

/** Exact owner targets whose persisted update state reports a newer commit. */
export function skillUpdateOwnerTargets(
  skill: Pick<InstalledSkill, "update_owner_ids" | "update_owners">,
): LifecycleTarget[] {
  const ownerIds = skill.update_owners?.map((update) => update.owner_id) ?? skill.update_owner_ids;
  return [...new Set(ownerIds)].map((owner_id) => ({ owner_id }));
}

/** Resolve an update only when the selected scope has one owner and that owner has an update. */
export function skillUpdateAvailability(
  skill: Pick<InstalledSkill, "name" | "deployments" | "update_owner_ids" | "update_owners">,
  selection: SkillLifecycleScopeSelection,
): SkillUpdateAvailability {
  const ownerIds = new Set<string>();
  for (const deployment of deploymentsInScope(skill, selection.scope, selection.projectPath)) {
    if (deployment.mutability === "mutable" && deployment.owner_id) {
      ownerIds.add(deployment.owner_id);
    }
  }
  if (ownerIds.size > 1) {
    return {
      available: false,
      reason: `${skill.name} has multiple lifecycle owners in this scope. Update a specific deployment in Locations.`,
    };
  }
  const ownerId = ownerIds.values().next().value;
  if (!ownerId) {
    return { available: false, reason: "The selected scope has no managed update owner." };
  }
  const updateOwnerIds = new Set(
    skillUpdateOwnerTargets(skill).flatMap(({ owner_id }) => owner_id ?? []),
  );
  if (!updateOwnerIds.has(ownerId)) {
    return { available: false, reason: "The selected deployment is up to date." };
  }
  return { available: true, target: { owner_id: ownerId } };
}

/** Run each owner update and return every failure for the UI. */
export async function updateSkillOwners(
  skill: Pick<InstalledSkill, "update_owner_ids" | "update_owners">,
  updateOwner: (target: LifecycleTarget) => Promise<{ success: boolean; error?: string }>,
): Promise<SkillOwnerUpdateSummary> {
  const targets = skillUpdateOwnerTargets(skill);
  const failures: SkillOwnerUpdateFailure[] = [];
  let succeeded = 0;
  for (const target of targets) {
    const ownerId = target.owner_id;
    if (!ownerId) continue;
    try {
      // Owner updates are sequential because the CLIs share lock files.
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- concurrent owner CLIs can race on the same ledger and are rejected by the backend mutation lock
      const result = await updateOwner(target);
      if (result.success) succeeded += 1;
      else {
        failures.push({
          ownerId,
          message: result.error ?? "Update command failed without an error message.",
        });
      }
    } catch (error) {
      failures.push({
        ownerId,
        message: error instanceof Error ? error.message : "Update failed without an error message.",
      });
    }
  }
  return { attempted: targets.length, succeeded, failures };
}

/** Describe the managed deployment group and linked locations removed by one exact target. */
export function skillRemovalPreview(
  skill: SkillLifecycleView,
  selection: SkillLifecycleScopeSelection,
): SkillRemovalPreview {
  const target = lifecycleTargetForSkill(skill, selection.scope, selection.projectPath);
  const targeted = skill.deployments.filter((deployment) =>
    target.owner_id
      ? deployment.owner_id === target.owner_id
      : deployment.id === target.deployment_id,
  );
  const managedDeployments = targeted.filter(
    (deployment) => deployment.backing.kind !== "linked-to",
  );
  const targetedIds = new Set(targeted.map((deployment) => deployment.id));
  const backingIds = new Set(managedDeployments.map((deployment) => deployment.id));
  const removesDotagentsOwner = managedDeployments.some(
    (deployment) => deployment.owner_kind === "dotagents",
  );
  const linkedDeployments = skill.deployments.filter(
    (deployment) =>
      deployment.backing.kind === "linked-to" &&
      (targetedIds.has(deployment.id) || backingIds.has(deployment.backing.deployment_id)) &&
      (!removesDotagentsOwner ||
        (agentIdFromDeploymentLabel(deployment.agent) === "claude-code" &&
          !deployment.shared_via_whole_dir_link)),
  );
  return { target, managedDeployments, linkedDeployments };
}

/** Preview one selected deployment and links verified as backed by it. */
export function skillDeploymentRemovalPreview(
  skill: SkillLifecycleView,
  deployment: Deployment,
): SkillRemovalPreview {
  const linkedDeployments = skill.deployments.filter(
    (candidate) =>
      candidate.backing.kind === "linked-to" && candidate.backing.deployment_id === deployment.id,
  );
  return {
    target: lifecycleTargetForDeployment(deployment),
    managedDeployments: [deployment],
    linkedDeployments,
  };
}

/** Describe the owner group and verified links that the target removes. */
export function skillRemovalDescription(preview: SkillRemovalPreview): string {
  const deploymentCount = preview.managedDeployments.length;
  const linkCount = preview.linkedDeployments.length;
  return `This removes ${deploymentCount} managed deployment${deploymentCount === 1 ? "" : "s"} and ${linkCount} verified dependent link${linkCount === 1 ? "" : "s"}. Independent copies outside this group remain. This cannot be undone.`;
}

/** The Global Universal folder park/unpark may move. Project and Per harness stay independent. */
export function lifecycleTargetForPark(skill: SkillLifecycleView): LifecycleTarget {
  const parked = skill.deployments.find((deployment) => deployment.scope === "parked");
  if (parked) return { deployment_id: parked.id };
  const canonical = skill.deployments.find(
    (deployment) =>
      deployment.scope === "global" &&
      deployment.destination === "universal" &&
      deployment.backing.kind === "canonical" &&
      !deployment.plugin,
  );
  if (!canonical) {
    throw new Error(
      `${skill.name} has no Global Universal folder to park. Project and Per harness copies stay independent.`,
    );
  }
  return { deployment_id: canonical.id };
}

/** Resolve the exact deployment owned by one trial, with legacy scope fallback. */
export function lifecycleTargetForTrial(
  skill: SkillLifecycleView,
  trial: TrialInfo,
): LifecycleTarget {
  if (trial.deployment_id) return { deployment_id: trial.deployment_id };
  const { scope, project_path: projectPath } = trial;
  const candidates = skill.deployments.filter(
    (deployment) =>
      deployment.mutability === "mutable" &&
      ((scope === "global" && deployment.scope === "parked") ||
        (deployment.scope === scope &&
          (scope === "global" || deployment.project_path === projectPath))),
  );
  const canonical = candidates.find((deployment) => deployment.backing.kind === "canonical");
  if (canonical) return lifecycleTargetForDeployment(canonical);
  if (candidates.length === 1) return lifecycleTargetForDeployment(candidates[0]);
  throw new Error(`${skill.name} has no unambiguous trial deployment`);
}
