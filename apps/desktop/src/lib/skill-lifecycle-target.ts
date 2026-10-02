import {
  agentIdFromDeploymentLabel,
  basename,
  homeRelativePath,
  parentDirectory,
} from "@skill-studio/lib";
import type {
  Deployment,
  InstalledSkill,
  InstallScope,
  LifecycleTarget,
  ForkRecord,
  LocalEditsDto,
  PullResult,
  Toast,
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
  /** Copies in the same scope that this removal leaves alone. */
  staying: Deployment[];
}

type SkillRemovalAvailability =
  | { available: true; preview: SkillRemovalPreview }
  | { available: false; reason: string };

interface SkillOwnerUpdateFailure {
  ownerId: string;
  message: string;
}

interface SkillOwnerUpdateSummary {
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

/** Where a folder's bytes really are: its own path unless the scan resolved it elsewhere. */
function realPath(deployment: Deployment): string {
  return deployment.resolved_path ?? deployment.path;
}

/**
 * The links the backend deletes together with `folder` (`find_all_links` in
 * `skill-studio-core`): per-skill links whose target is that folder. A
 * whole-directory link is the folder itself, so it is not one of them.
 */
function linksRemovedWith(skill: Pick<SkillLifecycleView, "deployments">, folder: Deployment) {
  return skill.deployments.filter(
    (deployment) =>
      deployment.backing.kind === "linked-to" &&
      !deployment.shared_via_whole_dir_link &&
      deployment.symlink_target === realPath(folder),
  );
}

/**
 * Why Skill Studio will not remove anything in a scope whose deployments are
 * all read-only. The backend only removes copies it owns.
 */
function readOnlyScopeReason(skillName: string, inScope: Deployment[]): string {
  if (inScope.some((deployment) => deployment.owner_kind === "in-repo")) {
    return `${skillName} is part of a repository, so Skill Studio will not delete it. Delete it in the repository.`;
  }
  if (inScope.some((deployment) => deployment.plugin)) {
    return `${skillName} comes with a plugin. Uninstall the plugin from Locations.`;
  }
  if (inScope.some((deployment) => deployment.owner_kind === "manual")) {
    return `Skill Studio did not install ${skillName}, so it will not delete it. Use Reveal in Finder on its row and delete the folder yourself.`;
  }
  return `${skillName} is read-only here, so Skill Studio cannot remove it.`;
}

/** Resolve an aggregate skill only when one mutable owner matches the requested scope. */
export function lifecycleTargetForSkill(
  skill: SkillLifecycleView,
  scope: "global" | "project",
  projectPath?: string | null,
): LifecycleTarget {
  const inScope = deploymentsInScope(skill, scope, projectPath);
  const deployments = inScope.filter((deployment) => deployment.mutability === "mutable");
  if (deployments.length === 0 && inScope.length > 0) {
    throw new Error(readOnlyScopeReason(skill.name, inScope));
  }
  const ownerIds = [...new Set(deployments.flatMap((deployment) => deployment.owner_id ?? []))];
  if (ownerIds.length === 1) return { owner_id: ownerIds[0] };
  if (ownerIds.length > 1) {
    throw new Error(
      `${skill.name} is installed from more than one source here. Manage each copy in Locations.`,
    );
  }
  const canonicalDeployments = deployments.filter(
    (deployment) =>
      deployment.backing.kind === "canonical" && deployment.destination === "universal",
  );
  if (canonicalDeployments.length === 1) {
    return lifecycleTargetForDeployment(canonicalDeployments[0]);
  }
  if (skill.deployments.length === 0 && skill.source_kind === "skills-sh" && scope === "global") {
    return { owner_id: `owner:v1/global/${skill.name}` };
  }
  throw new Error(
    `${skill.name} has no single Universal folder here that Skill Studio can remove. Its copies are separate agent folders; use Reveal in Finder on each row and delete them yourself.`,
  );
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
      reason: error instanceof Error ? error.message : "Remove each separate copy from Locations.",
    };
  }
}

export interface SkillRemovalChoice {
  /** Stable per scope and project path, since two projects can share a folder name. */
  key: string;
  selection: SkillLifecycleScopeSelection;
  preview: SkillRemovalPreview;
  label: string;
  confirmTitle: string;
  confirmMessage: string;
}

function pathSegments(path: string): string[] {
  return path.split("/").filter(Boolean);
}

function projectName(projectPath: string): string {
  const segments = pathSegments(projectPath);
  return segments[segments.length - 1] ?? projectPath;
}

/** The shortest trailing part of each path that no other path in `paths` ends with. */
function distinctProjectNames(paths: string[]): Map<string, string> {
  const names = new Map<string, string>();
  for (const path of paths) {
    const segments = pathSegments(path);
    let name = path;
    for (let count = 1; count <= segments.length; count += 1) {
      const suffix = segments.slice(-count).join("/");
      const clash = paths.some(
        (other) => other !== path && pathSegments(other).slice(-count).join("/") === suffix,
      );
      if (!clash) {
        name = suffix;
        break;
      }
    }
    names.set(path, name);
  }
  return names;
}

/** Every scope the page header can remove the skill from, global first. */
export function skillRemovalChoices(skill: SkillLifecycleView): SkillRemovalChoice[] {
  const available = skillMutableLifecycleScopes(skill).flatMap((selection) => {
    const availability = skillRemovalAvailability(skill, selection);
    return availability.available ? [{ selection, preview: availability.preview }] : [];
  });
  const projectNames = distinctProjectNames(
    available.flatMap(({ selection }) => selection.projectPath ?? []),
  );
  return available.map(({ selection, preview }) => {
    const description = skillRemovalDescription(preview);
    if (selection.projectPath == null) {
      return {
        key: "global",
        selection,
        preview,
        label: available.length === 1 ? "Remove" : "Remove global install",
        confirmTitle: `Remove ${skill.name}?`,
        confirmMessage: description,
      };
    }
    const project = projectNames.get(selection.projectPath) ?? selection.projectPath;
    return {
      key: `project:${selection.projectPath}`,
      selection,
      preview,
      label: `Remove from ${project}`,
      confirmTitle: `Remove ${skill.name} from ${project}?`,
      confirmMessage: `Project: ${selection.projectPath}\n\n${description}`,
    };
  });
}

/** Why the page header offers no Remove, when the answer is somewhere else - `null` otherwise. */
export function skillRemovalBlockedReason(skill: SkillLifecycleView): string | null {
  if (skillRemovalChoices(skill).length > 0) return null;
  const inRepo = skill.deployments.find((deployment) => deployment.owner_kind === "in-repo");
  if (inRepo) {
    const repository = inRepo.project_path
      ? `the ${projectName(inRepo.project_path)} repository`
      : "a repository";
    return `Part of ${repository}; delete it there`;
  }
  if (skill.deployments.some((deployment) => deployment.plugin)) {
    return "Comes with a plugin; uninstall the plugin from Locations";
  }
  return null;
}

/** Whether removing `selection` leaves no deployment behind, so the page has nothing left to show. */
export function skillRemovalEmptiesSkill(
  skill: SkillLifecycleView,
  selection: SkillLifecycleScopeSelection,
): boolean {
  return skill.deployments.every(
    (deployment) =>
      deployment.scope === selection.scope &&
      (selection.scope === "global" || deployment.project_path === selection.projectPath),
  );
}

/**
 * Exact owner targets whose persisted update state reports a newer commit. An
 * owner whose deployments are all read-only is skipped: the backend refuses to
 * update it, so offering it only produces a failure.
 */
export function skillUpdateOwnerTargets(
  skill: Pick<InstalledSkill, "update_owner_ids" | "update_owners"> &
    Partial<Pick<InstalledSkill, "deployments">>,
): LifecycleTarget[] {
  const ownerIds = skill.update_owners?.map((update) => update.owner_id) ?? skill.update_owner_ids;
  const deployments = skill.deployments ?? [];
  return [...new Set(ownerIds)].flatMap((owner_id) => {
    const owned = deployments.filter((deployment) => deployment.owner_id === owner_id);
    const runnable =
      owned.length === 0 || owned.some((deployment) => deployment.mutability === "mutable");
    return runnable ? [{ owner_id }] : [];
  });
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
      reason: `${skill.name} is installed from more than one source here. Update each copy in Locations.`,
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

/**
 * The skills among `skills` whose update would replace local edits: a skills.sh
 * skill that is not a fork and whose installed folder no longer matches what
 * the install recorded. A check that cannot run (`checked: false`) or throws
 * counts as not edited, so Update keeps working exactly as before. `targetsOf`
 * narrows the check to the owners an update will actually replace.
 */
export async function skillsWithLocalEdits<
  T extends Pick<InstalledSkill, "source_kind" | "update_owner_ids" | "update_owners"> &
    Partial<Pick<InstalledSkill, "deployments">>,
>(
  skills: T[],
  checkEdits: (targets: LifecycleTarget[]) => Promise<LocalEditsDto[]>,
  targetsOf: (skill: T) => LifecycleTarget[] = skillUpdateOwnerTargets,
): Promise<T[]> {
  const entries = skills.flatMap((skill) =>
    skill.source_kind === "skills-sh" ? targetsOf(skill).map((target) => ({ skill, target })) : [],
  );
  if (entries.length === 0) return [];
  let verdicts: LocalEditsDto[];
  try {
    verdicts = await checkEdits(entries.map((entry) => entry.target));
  } catch {
    return [];
  }
  const edited = new Set<T>();
  for (const [index, entry] of entries.entries()) {
    const verdict = verdicts[index];
    if (verdict?.checked && verdict.edited) edited.add(entry.skill);
  }
  return skills.filter((skill) => edited.has(skill));
}

/** The one deployment a skills.sh skill can be forked from: its Global Universal folder. */
export function forkableDeployment(
  skill: Pick<InstalledSkill, "deployments">,
): Deployment | undefined {
  return skill.deployments.find(
    (deployment) =>
      deployment.scope === "global" &&
      deployment.destination === "universal" &&
      deployment.backing.kind === "canonical" &&
      deployment.owner_kind === "skills-sh" &&
      !deployment.plugin,
  );
}

/** The lifecycle target a fork of `skill` starts from; throws when the skill has no forkable folder. */
export function forkTargetForSkill(
  skill: Pick<InstalledSkill, "name" | "deployments">,
): LifecycleTarget {
  const deployment = forkableDeployment(skill);
  if (!deployment) throw new Error(`${skill.name} has no Global Universal folder to fork.`);
  return lifecycleTargetForDeployment(deployment);
}

/**
 * `targets` minus the owner a fork replaces. Forking an edited skill turns
 * its Global Universal owner into the fork; every other owner (a project
 * copy, a per-harness copy) still needs its normal update.
 */
export function excludeForkedOwner(
  skill: Pick<InstalledSkill, "deployments">,
  targets: LifecycleTarget[],
): LifecycleTarget[] {
  const forkedOwnerId = forkableDeployment(skill)?.owner_id;
  return targets.filter((target) => !target.owner_id || target.owner_id !== forkedOwnerId);
}

/** Thrown when the fork was made but pulling upstream onto it failed: the edits are safe, only the update is missing. */
export class ForkPullError extends Error {
  constructor(cause: unknown) {
    const reason = cause instanceof Error ? cause.message : String(cause);
    super(
      `The fork was made and your edits are kept, but pulling upstream failed: ${reason}. Use Pull latest to retry.`,
    );
    this.name = "ForkPullError";
  }
}

/**
 * Forks the skill at `target`, then pulls upstream on the new fork so the
 * user's edits and the update are merged. The pull goes to the fork's own
 * deployment id: the owner id the skill had before the fork no longer exists.
 * A failed fork changes nothing and rethrows; a failed pull after a good fork
 * throws `ForkPullError`.
 */
export async function forkThenPull(
  target: LifecycleTarget,
  fork: (target: LifecycleTarget) => Promise<ForkRecord>,
  pullFork: (target: LifecycleTarget) => Promise<PullResult>,
): Promise<PullResult> {
  const record = await fork(target);
  try {
    return await pullFork({ deployment_id: record.deployment_id ?? target.deployment_id });
  } catch (error) {
    throw new ForkPullError(error);
  }
}

/**
 * "Fork and update" for one edited skill: fork the Global Universal copy and
 * pull upstream onto it, then run the normal update for every other owner
 * (project or per-harness copies), which the fork does not touch.
 */
export async function forkEditedAndUpdate(
  skill: Pick<InstalledSkill, "name" | "deployments" | "update_owner_ids" | "update_owners">,
  deps: {
    fork: (target: LifecycleTarget) => Promise<ForkRecord>;
    pullFork: (target: LifecycleTarget) => Promise<PullResult>;
    updateOwner: (target: LifecycleTarget) => Promise<{ success: boolean; error?: string | null }>;
  },
  options: { updateOthers: boolean } = { updateOthers: true },
): Promise<{ pull: PullResult; others: SkillOwnerUpdateSummary }> {
  const pull = await forkThenPull(forkTargetForSkill(skill), deps.fork, deps.pullFork);
  // react-doctor-disable-next-line react-doctor/server-sequential-independent-await -- a failed fork must leave the other copies untouched, and both steps write ~/.agents/.skill-lock.json
  const others = await updateOwnerTargets(
    options.updateOthers ? excludeForkedOwner(skill, skillUpdateOwnerTargets(skill)) : [],
    deps.updateOwner,
  );
  return { pull, others };
}

/**
 * Builds the toast for a finished `pull_fork_upstream` call. Conflicts win
 * over `message` when both are set - the only case that happens in
 * practice is a failed editor open after a conflicted pull, where
 * `message` names the file and the open error (see `skill_fork.rs`'s
 * `pull_fork_upstream`) and would otherwise silently replace the conflict
 * count and title. `message` alone (the "Already up to date" case) still
 * gets its own info toast.
 */
export function pullUpstreamToast(result: PullResult): Omit<Toast, "id"> {
  if (result.conflicts.length > 0) {
    const conflictText = result.conflicts.join(", ");
    return {
      type: "warning",
      title: `${result.conflicts.length} conflicts — open the editor to resolve`,
      message: result.message ? `${conflictText} ${result.message}` : conflictText,
    };
  }
  if (result.message) {
    return { type: "info", title: result.message };
  }
  // No conflicts and no message: every file here was a clean pull from
  // upstream (nothing merged - a file both sides changed would have
  // landed in `result.conflicts` instead, with markers).
  const updatedCount = result.merged.length + result.added.length + result.removed.length;
  return { type: "success", title: `Updated ${updatedCount} files` };
}

/** Short toast line naming the skills whose pull left conflict markers, or `undefined` when none did. */
export function conflictedSkillsNote(skillNames: string[]): string | undefined {
  if (skillNames.length === 0) return undefined;
  return `Conflicts to resolve in the editor: ${skillNames.join(", ")}`;
}

/** Run each owner update and return every failure for the UI. */
export async function updateSkillOwners(
  skill: Pick<InstalledSkill, "update_owner_ids" | "update_owners"> &
    Partial<Pick<InstalledSkill, "deployments">>,
  updateOwner: (target: LifecycleTarget) => Promise<{ success: boolean; error?: string | null }>,
): Promise<SkillOwnerUpdateSummary> {
  return updateOwnerTargets(skillUpdateOwnerTargets(skill), updateOwner);
}

/** Run an update for each owner target in turn and return every failure. */
export async function updateOwnerTargets(
  targets: LifecycleTarget[],
  updateOwner: (target: LifecycleTarget) => Promise<{ success: boolean; error?: string | null }>,
): Promise<SkillOwnerUpdateSummary> {
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

/**
 * The toast for a finished skill update, the same words wherever an update
 * result shows: all copies updated, some of them, or none.
 */
export function skillUpdateToast(
  skillName: string,
  summary: SkillOwnerUpdateSummary,
): Omit<Toast, "id"> {
  const failureMessage = summary.failures.map((failure) => failure.message).join("; ");
  if (summary.failures.length === 0) {
    return { type: "success", title: `Updated ${skillName}` };
  }
  if (summary.succeeded === 0) {
    return { type: "error", title: `Could not update ${skillName}`, message: failureMessage };
  }
  return {
    type: "warning",
    title: `Updated ${summary.succeeded} of ${summary.attempted} copies of ${skillName}`,
    message: failureMessage,
  };
}

/**
 * Whether `npx skills remove` deletes whatever sits at this folder's path. Skills CLI 1.7.0
 * (`removeCommand`) runs `rm -rf <skills dir>/<name>` for every agent's skills directory:
 * all six first-class agents' plural `skills` folders in the global scope, and in a project
 * only Claude Code, Grok Build and pi, because Codex, Cursor and OpenCode read the project's
 * `.agents/skills` there. It never touches OpenCode's singular `skill` folder. The folder is
 * judged by agent, scope and its parent folder's name, so a moved config home still matches.
 */
function skillsCliRemovesFolderAt(deployment: Deployment): boolean {
  if (deployment.plugin || basename(parentDirectory(deployment.path)) !== "skills") return false;
  const agent = agentIdFromDeploymentLabel(deployment.agent);
  if (deployment.scope === "global") {
    return agent !== null && agent !== "shared";
  }
  return agent === "claude-code" || agent === "grok-build" || agent === "pi";
}

/** Describe the managed deployment group and linked locations removed by one exact target. */
export function skillRemovalPreview(
  skill: SkillLifecycleView,
  selection: SkillLifecycleScopeSelection,
): SkillRemovalPreview {
  const target = lifecycleTargetForSkill(skill, selection.scope, selection.projectPath);
  const managedDeployments = skill.deployments.filter(
    (deployment) =>
      (target.owner_id
        ? deployment.owner_id === target.owner_id
        : deployment.id === target.deployment_id) && deployment.backing.kind !== "linked-to",
  );
  const linkedDeployments = managedDeployments.flatMap((folder) => linksRemovedWith(skill, folder));
  const removed = new Set([...managedDeployments, ...linkedDeployments]);
  let staying = deploymentsInScope(skill, selection.scope, selection.projectPath).filter(
    (deployment) => !removed.has(deployment),
  );
  if (managedDeployments.some((deployment) => deployment.owner_kind === "skills-sh")) {
    const cleaned = staying.filter(skillsCliRemovesFolderAt);
    const managedRealPaths = new Set(managedDeployments.map(realPath));
    const lost = cleaned.filter((deployment) =>
      deployment.shared_via_whole_dir_link
        ? !managedRealPaths.has(realPath(deployment))
        : !deployment.is_symlink,
    );
    if (lost.length > 0) {
      const paths = lost.map((copy) => homeRelativePath(copy.path)).join(", ");
      throw new Error(
        `Removing would also delete the separate ${lost.length === 1 ? "copy" : "copies"} at ${paths}, and Undo could not bring ${lost.length === 1 ? "it" : "them"} back. Delete or move ${lost.length === 1 ? "that copy" : "those copies"} first.`,
      );
    }
    // A link to a folder elsewhere is deleted too; only the link goes, so nothing is lost.
    const strayLinks = cleaned.filter(
      (deployment) => deployment.is_symlink && !deployment.shared_via_whole_dir_link,
    );
    linkedDeployments.push(...strayLinks);
    const strayIds = new Set(strayLinks);
    staying = staying.filter((deployment) => !strayIds.has(deployment));
  }
  return {
    target,
    managedDeployments,
    linkedDeployments,
    staying: staying.filter((deployment) => !deployment.shared_via_whole_dir_link),
  };
}

/**
 * Preview one selected copy. The backend removes only a Universal folder that
 * holds its own bytes, so any other copy has no Remove.
 */
export function skillDeploymentRemovalAvailability(
  skill: SkillLifecycleView,
  deployment: Deployment,
): SkillRemovalAvailability {
  if (deployment.destination !== "universal" || deployment.backing.kind !== "canonical") {
    return {
      available: false,
      reason: `Skill Studio can only remove a Universal folder. Use Reveal in Finder and delete ${deployment.path} yourself.`,
    };
  }
  return {
    available: true,
    preview: {
      target: lifecycleTargetForDeployment(deployment),
      managedDeployments: [deployment],
      linkedDeployments: linksRemovedWith(skill, deployment),
      staying: [],
    },
  };
}

/** Describe the owner group and verified links that the target removes. */
export function skillRemovalDescription(preview: SkillRemovalPreview): string {
  const folderCount = preview.managedDeployments.length;
  const linkCount = preview.linkedDeployments.length;
  const removes = `This removes ${folderCount} folder${folderCount === 1 ? "" : "s"} and ${linkCount} link${linkCount === 1 ? "" : "s"} to ${folderCount === 1 ? "it" : "them"}.`;
  const stays =
    preview.staying.length > 0
      ? `The separate ${preview.staying.length === 1 ? "copy" : "copies"} at ${preview.staying.map((copy) => homeRelativePath(copy.path)).join(", ")} stay${preview.staying.length === 1 ? "s" : ""}.`
      : "Separate copies elsewhere stay.";
  return `${removes} ${stays} This cannot be undone.`;
}

function parkableDeployment(skill: SkillLifecycleView): Deployment | undefined {
  return (
    skill.deployments.find((deployment) => deployment.scope === "parked") ??
    skill.deployments.find(
      (deployment) =>
        deployment.scope === "global" &&
        deployment.destination === "universal" &&
        deployment.backing.kind === "canonical" &&
        !deployment.plugin,
    )
  );
}

/** Whether park/unpark has a folder to move - `ops::park` in the core refuses every other skill. */
export function skillCanPark(skill: SkillLifecycleView): boolean {
  return parkableDeployment(skill) !== undefined;
}

/** The park verb a skill offers, or `null` when it has no folder `ops::park` can move. */
export function skillParkVerb(
  skill: SkillLifecycleView & Pick<InstalledSkill, "parked">,
): "Park" | "Unpark" | null {
  if (!skillCanPark(skill)) return null;
  return skill.parked ? "Unpark" : "Park";
}

/** The Global Universal folder park/unpark may move. Project and Per harness stay independent. */
export function lifecycleTargetForPark(skill: SkillLifecycleView): LifecycleTarget {
  const canonical = parkableDeployment(skill);
  if (!canonical) {
    throw new Error(
      `${skill.name} has no Global Universal folder to park. Project and per-agent copies stay separate.`,
    );
  }
  return { deployment_id: canonical.id };
}
