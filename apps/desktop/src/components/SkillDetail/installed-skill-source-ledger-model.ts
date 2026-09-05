// ============================================================================
// Skill Studio - Installed skill source ledger model
// Derives source and lifecycle facts without exposing deployment locations.
// ============================================================================

import { formatBytes, formatTokens, pluginLabelForSkill } from "@skill-studio/lib";
import type { Deployment, InstalledSkill, LifecycleOwnerKind } from "@skill-studio/lib";

/** Text rendered by the installed skill source ledger. */
export interface InstalledSkillSourceLedgerModel {
  source: string;
  lifecycleOwner: string;
  lifecycleManagement: "Managed" | "Read-only" | "Mixed" | "Unknown";
  installed: string;
  lastModified?: string;
  updateState: string;
  size?: string;
  tokens?: string;
}

const LIFECYCLE_OWNER_LABELS = {
  "skills-sh": "skills.sh",
  dotagents: "dotagents",
  copy: "Skill Studio Copy",
  fork: "Fork",
  plugin: "Plugin",
  "in-repo": "In repository",
  manual: "Manual",
  "wildcard-dotagents": "Ambiguous",
  ambiguous: "Ambiguous",
} satisfies Record<LifecycleOwnerKind, string>;

function displayLedgerDate(value: string | undefined): string | undefined {
  if (!value) return undefined;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return undefined;
  return date.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

function sourceLedgerLabel(skill: InstalledSkill): string {
  if (skill.source_kind === "plugin") {
    const pluginName = pluginLabelForSkill(skill);
    return pluginName ? `Plugin · ${pluginName}` : "Agent plugin";
  }
  if (skill.source_kind === "in-repo") return "Local repository";
  if (skill.source_kind === "fork") return skill.fork?.origin_source ?? "Local fork";
  if (skill.source && !["local", "manual", "plugin"].includes(skill.source)) return skill.source;
  if (skill.source_url) return skill.source_url;
  if (skill.source_kind === "dotagents") return "dotagents";
  if (skill.source_kind === "skills-sh") return "skills.sh";
  if (skill.deployments.some((deployment) => deployment.owner_kind === "copy")) {
    return "Local copy";
  }
  return "Local skill";
}

function effectiveOwnerDeployment(
  deployment: Deployment,
  deploymentsById: ReadonlyMap<string, Deployment>,
): Deployment {
  if (deployment.backing.kind !== "linked-to") return deployment;
  return deploymentsById.get(deployment.backing.deployment_id) ?? deployment;
}

function effectiveDeployments(deployments: Deployment[]): Deployment[] {
  const deploymentsById = new Map(deployments.map((deployment) => [deployment.id, deployment]));
  return deployments.map((deployment) => effectiveOwnerDeployment(deployment, deploymentsById));
}

function lifecycleOwnerLabel(deployments: Deployment[]): string {
  const owners = new Map<string, Deployment>();

  for (const owner of effectiveDeployments(deployments)) {
    const identity = owner.owner_id
      ? `owner:${owner.owner_id}`
      : owner.owner_kind === "copy" && owner.mutability === "mutable"
        ? `deployment:${owner.id}`
        : `kind:${owner.owner_kind}`;
    if (!owners.has(identity)) owners.set(identity, owner);
  }

  if (owners.size === 0) return "Ambiguous";
  if (owners.size === 1) {
    const owner = owners.values().next().value;
    return owner ? LIFECYCLE_OWNER_LABELS[owner.owner_kind] : "Ambiguous";
  }
  if ([...owners.values()].some((owner) => owner.mutability === "mutable")) {
    return "Mixed ownership";
  }
  return "Ambiguous";
}

function lifecycleManagementLabel(
  deployments: Deployment[],
): InstalledSkillSourceLedgerModel["lifecycleManagement"] {
  if (deployments.length === 0) return "Unknown";
  const resolvedDeployments = effectiveDeployments(deployments);
  const hasMutable = resolvedDeployments.some((deployment) => deployment.mutability === "mutable");
  const hasReadOnly = resolvedDeployments.some(
    (deployment) => deployment.mutability === "read-only",
  );
  if (hasMutable && hasReadOnly) return "Mixed";
  if (hasMutable) return "Managed";
  return "Read-only";
}

function updateStateLabel(skill: InstalledSkill): string {
  const ownerCount = new Set(
    (skill.update_owners?.map((owner) => owner.owner_id) ?? skill.update_owner_ids).filter(Boolean),
  ).size;
  if (ownerCount === 0) return "Up to date";
  if (ownerCount === 1) return "1 update available";
  return `${ownerCount} owner updates available`;
}

/** Builds source ledger text from exact deployment ownership, with linked readers deduplicated. */
export function buildInstalledSkillSourceLedgerModel(
  skill: InstalledSkill,
): InstalledSkillSourceLedgerModel {
  const contentHashes = new Set(skill.content_hashes.filter(Boolean));
  const hasOneReadableContent = contentHashes.size === 1;

  return {
    source: sourceLedgerLabel(skill),
    lifecycleOwner: lifecycleOwnerLabel(skill.deployments),
    lifecycleManagement: lifecycleManagementLabel(skill.deployments),
    installed: displayLedgerDate(skill.installed_at) ?? "Unknown",
    lastModified: displayLedgerDate(skill.modified_at),
    updateState: updateStateLabel(skill),
    size: hasOneReadableContent ? formatBytes(skill.folder_bytes) : undefined,
    tokens: hasOneReadableContent ? `${formatTokens(skill.skill_md_tokens)} tokens` : undefined,
  };
}
