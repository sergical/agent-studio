// ============================================================================
// SkillLocationsCard - "Where it lives": one row per deployment (harness,
// relation, path, an Enabled switch where the harness supports it, and a
// Reveal button), plus an Invocation footer row. Replaces the old
// SkillDetailDetails Locations/Invocation sections and the raw frontmatter
// key list.
// ============================================================================

import { useState } from "react";
import { AlertTriangle, FolderOpen } from "lucide-react";
import {
  agentIdFromDeploymentLabel,
  deploymentLinkKind,
  deploymentLinkTarget,
} from "../../lib/skill-coverage";
import {
  forkSkill,
  openSkillPath,
  setHarnessEnabled,
  setSkillInvocation,
} from "../../lib/skill-api";
import { FIRST_CLASS_AGENTS } from "../../lib/skill-health";
import { homeRelativePath } from "../../lib/skill-path-format";
import type { Deployment, InstalledSkill, InvocationPolicy } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";
import { TooltipControl } from "../ui/TooltipControl";
import { SwitchControl } from "../ui/SwitchControl";

interface SkillLocationsCardProps {
  skill: InstalledSkill;
  /** The SKILL.md path `setSkillInvocation` should rewrite - unset when the skill has no own deployment. */
  skillMdPath?: string;
  /** The deployment `skillMdPath` came from, for the Fork-and-save flow's `forkSkill(name, path)` call. */
  skillMdDeployment?: Deployment;
}

const INVOCATION_POLICY_OPTIONS: { value: InvocationPolicy; label: string }[] = [
  { value: "both", label: "Both" },
  { value: "user-only", label: "User only" },
  { value: "model-only", label: "Model only" },
];

/** "Both: you can call /skill-name and the model can pick it", etc. */
function invocationPolicyExplanation(policy: InvocationPolicy, skillName: string): string {
  switch (policy) {
    case "both":
      return `Both: you can call /${skillName} and the model can pick it`;
    case "user-only":
      return `User only: only /${skillName} starts it`;
    case "model-only":
      return `Model only: the model picks it; no /${skillName}`;
  }
}

/** Harnesses with a per-skill disable switch - see `skill_harness_disable.rs`. */
const HARNESSES_WITH_PER_SKILL_DISABLE = ["codex", "open-code", "claude-code"];

/** The relation text shown after a deployment's harness name - `null` for a broken link, which keeps its own warning line below. */
function relationLabel(deployment: Deployment): string | null {
  const target = deploymentLinkTarget(deployment);
  switch (deploymentLinkKind(deployment)) {
    case "shared-root":
      // The row's own name is already "Shared folder" - no need to repeat it here.
      return "source of truth";
    case "linked-to-shared":
      return deployment.is_symlink
        ? `symlink → ${target ? homeRelativePath(target) : "unknown target"}`
        : `linked folder → ${target ? homeRelativePath(target) : "unknown target"}`;
    case "own":
      return deployment.is_symlink
        ? `symlink → ${target ? homeRelativePath(target) : "unknown target"}`
        : "copy";
    case "broken":
      return `broken link → ${deployment.symlink_target ?? "unknown target"}`;
  }
}

/** "Claude Code" / "Codex" / ... for a deployment's harness label, "Shared folder" for the shared root. */
function harnessDisplayName(deployment: Deployment): string {
  return deployment.agent === "shared" ? "Shared folder" : deployment.agent;
}

/** The last path segment, for a project row's muted prefix chip. */
function basename(path: string): string {
  return path.split("/").filter(Boolean).pop() ?? path;
}

/**
 * Sorts deployments for display: global scope first, then project rows
 * grouped by project folder name (A→Z); within a group the shared root
 * first, then harnesses in `FIRST_CLASS_AGENTS` order.
 */
function sortedDeployments(deployments: Deployment[]): Deployment[] {
  function rank(d: Deployment): [number, string, number] {
    const projectGroup = d.scope === "project" && d.project_path ? basename(d.project_path) : "";
    // SAFETY: indexOf on a readonly string tuple needs its search value typed
    // as one of the tuple's members; -1 (not found) is a valid, handled
    // result for any other agent label, so a non-member string is harmless.
    const agentRank = d.agent === "shared" ? -1 : FIRST_CLASS_AGENTS.indexOf(d.agent as never);
    return [projectGroup ? 1 : 0, projectGroup, agentRank];
  }
  return [...deployments].sort((a, b) => {
    const [groupA, projectA, agentA] = rank(a);
    const [groupB, projectB, agentB] = rank(b);
    if (groupA !== groupB) return groupA - groupB;
    if (projectA !== projectB) return projectA.localeCompare(projectB);
    return agentA - agentB;
  });
}

function DeploymentRow({ deployment, skill }: { deployment: Deployment; skill: InstalledSkill }) {
  const addToast = useAppStore((state) => state.addToast);
  const [isTogglingHarness, setIsTogglingHarness] = useState(false);
  const harnessId = harnessIdFromLabel(deployment.agent);
  const relation = relationLabel(deployment);
  const projectName =
    deployment.scope === "project" && deployment.project_path
      ? basename(deployment.project_path)
      : null;
  const supportsDisableSwitch = HARNESSES_WITH_PER_SKILL_DISABLE.includes(
    agentIdFromDeploymentLabel(deployment.agent) ?? "",
  );

  const handleReveal = () => {
    openSkillPath(deployment.path, "reveal").catch((err) => {
      addToast({
        type: "error",
        title: "Couldn't reveal in Finder",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    });
  };

  const handleToggleEnabled = async (nextEnabled: boolean) => {
    setIsTogglingHarness(true);
    try {
      await setHarnessEnabled(
        skill.name,
        agentIdFromDeploymentLabel(deployment.agent) ?? "",
        nextEnabled,
      );
    } catch (err) {
      addToast({
        type: "error",
        title: nextEnabled ? "Couldn't enable" : "Couldn't disable",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
      setIsTogglingHarness(false);
    }
  };

  return (
    <div className="skill-locations-row">
      <div className="skill-locations-row-main">
        {projectName && <span className="skill-locations-row-project">{projectName}</span>}
        {harnessId && <HarnessIcon harness={harnessId} size={16} />}
        <span className="skill-locations-row-name">{harnessDisplayName(deployment)}</span>
        {relation && (
          <span
            className={`skill-locations-row-relation ${deploymentLinkKind(deployment) === "broken" ? "broken" : ""}`}
          >
            {deploymentLinkKind(deployment) === "broken" && <AlertTriangle size={11} />}
            {relation}
          </span>
        )}
        {deployment.plugin && (
          <span className="skill-locations-row-relation">
            plugin · {deployment.plugin.name}
            {deployment.plugin.version ? ` v${deployment.plugin.version}` : ""}
          </span>
        )}
      </div>
      <TooltipControl content={deployment.path}>
        <span className="skill-locations-row-path">
          <span dir="ltr">{homeRelativePath(deployment.path)}</span>
        </span>
      </TooltipControl>
      <div className="skill-locations-row-controls">
        {supportsDisableSwitch && (
          <label className="switch-label">
            <SwitchControl
              checked={!deployment.disabled}
              onCheckedChange={handleToggleEnabled}
              disabled={isTogglingHarness}
              ariaLabel={`Enabled for ${deployment.agent}`}
            />
            Enabled
          </label>
        )}
        <TooltipControl content={`Reveal ${deployment.agent} copy in Finder`}>
          <button
            type="button"
            className="skill-locations-row-reveal"
            onClick={handleReveal}
            aria-label={`Reveal ${deployment.agent} copy in Finder`}
          >
            <FolderOpen size={14} />
          </button>
        </TooltipControl>
      </div>
    </div>
  );
}

/**
 * "Where it lives": one row per deployment plus an Invocation footer row
 * (segmented Both/User only/Model only control, a one-line explanation, and
 * an `allowed-tools` chip when the frontmatter sets one).
 */
export function SkillLocationsCard({
  skill,
  skillMdPath,
  skillMdDeployment,
}: SkillLocationsCardProps) {
  const addToast = useAppStore((state) => state.addToast);
  const [isSavingInvocation, setIsSavingInvocation] = useState(false);
  const allowedTools = skill.frontmatter_fields["allowed-tools"];

  // A dotagents/skills.sh-managed skill would have its edits overwritten by
  // the next sync/update - forking first (same rule as the SKILL.md editor)
  // makes the invocation-policy change stick.
  const needsForkToSave = skill.source_kind === "dotagents" || skill.source_kind === "skills-sh";

  const handleSetInvocation = async (policy: InvocationPolicy) => {
    if (!skillMdPath || isSavingInvocation) return;
    setIsSavingInvocation(true);
    try {
      if (needsForkToSave && skillMdDeployment) {
        await forkSkill(skill.name, skillMdDeployment.path);
      }
      await setSkillInvocation(skill.name, skillMdPath, policy);
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't change invocation policy",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
      setIsSavingInvocation(false);
    }
  };

  return (
    <div className="home-block skill-locations-card">
      <div className="home-block-header">Locations</div>

      {skill.deployments.length === 0 ? (
        <p className="skill-detail-content-empty">
          Known only from the lock file — no folder on disk.
        </p>
      ) : (
        <div className="skill-locations-list">
          {sortedDeployments(skill.deployments).map((deployment, i) => (
            <DeploymentRow
              key={`${deployment.agent}-${deployment.scope}-${i}`}
              deployment={deployment}
              skill={skill}
            />
          ))}
        </div>
      )}

      <div className="skill-locations-footer">
        <span className="skill-locations-footer-label">Invocation</span>
        {skillMdPath && (
          <div className="segmented" role="group" aria-label="Invocation">
            {INVOCATION_POLICY_OPTIONS.map((option) => (
              <button
                key={option.value}
                type="button"
                className="segmented-item"
                aria-pressed={skill.invocation === option.value}
                onClick={() => handleSetInvocation(option.value)}
                disabled={isSavingInvocation || skill.invocation === option.value}
              >
                {option.label}
              </button>
            ))}
          </div>
        )}
        <p className="skill-locations-footer-explanation">
          {invocationPolicyExplanation(skill.invocation, skill.name)}
        </p>
        {allowedTools && (
          <div className="skill-locations-allowed-tools">Allowed tools: {allowedTools}</div>
        )}
      </div>
    </div>
  );
}
