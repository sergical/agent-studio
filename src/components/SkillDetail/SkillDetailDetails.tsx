// ============================================================================
// SkillDetailDetails - Collapsed-by-default <details>: locations, invocation
// control frontmatter, and source/install metadata
// ============================================================================

import { useState } from "react";
import { AlertTriangle, FolderOpen, Link2 } from "lucide-react";
import {
  forkSkill,
  openSkillPath,
  setHarnessEnabled,
  setSkillInvocation,
} from "../../lib/skill-api";
import { formatRelativeTime } from "../../lib/skill-stats";
import { agentIdFromDeploymentLabel } from "../../lib/skill-coverage";
import type { Deployment, InstalledSkill, InvocationPolicy } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";

interface SkillDetailDetailsProps {
  skill: InstalledSkill;
  /** The SKILL.md path `setSkillInvocation` should rewrite - unset when the skill has no own deployment. */
  skillMdPath?: string;
  /** The deployment `skillMdPath` came from, for the Fork-and-save flow's `forkSkill(name, path)` call. */
  skillMdDeployment?: Deployment;
}

/** Frontmatter keys that gate model auto-invocation; see docs/agent-skill-conventions.md. */
const INVOCATION_CONTROL_KEYS = ["disable-model-invocation", "user-invocable", "allowed-tools"];

const INVOCATION_POLICY_OPTIONS: { value: InvocationPolicy; label: string }[] = [
  { value: "both", label: "Both" },
  { value: "user-only", label: "User only" },
  { value: "model-only", label: "Model only" },
];

/** Harnesses with a per-skill disable switch - see `skill_harness_disable.rs`. */
const HARNESSES_WITH_PER_SKILL_DISABLE = ["codex", "open-code", "claude-code"];

/**
 * A native `<details>`, collapsed by default: every deployment's location
 * (with a per-row Reveal button), the invocation-control frontmatter keys,
 * and where/when the skill was installed.
 */
export function SkillDetailDetails({
  skill,
  skillMdPath,
  skillMdDeployment,
}: SkillDetailDetailsProps) {
  const addToast = useAppStore((state) => state.addToast);
  const invocationEntries = INVOCATION_CONTROL_KEYS.filter(
    (key) => skill.frontmatter_fields[key] !== undefined,
  );
  const installed = formatRelativeTime(skill.installed_at);
  const updated = skill.updated_at ? formatRelativeTime(skill.updated_at) : "unknown";
  const [isSavingInvocation, setIsSavingInvocation] = useState(false);
  const [pendingHarness, setPendingHarness] = useState<string | null>(null);

  // A dotagents/skills.sh-managed skill would have its edits overwritten by
  // the next sync/update - forking first (same rule as SkillPage's SKILL.md
  // editor) makes the invocation-policy change stick.
  const needsForkToSave = skill.source_kind === "dotagents" || skill.source_kind === "skills-sh";

  const handleReveal = async (path: string) => {
    try {
      await openSkillPath(path, "reveal");
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't reveal in Finder",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

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

  const handleToggleHarness = async (deployment: Deployment) => {
    const agent = agentIdFromDeploymentLabel(deployment.agent);
    if (!agent || agent === "shared") return;
    setPendingHarness(deployment.agent);
    try {
      await setHarnessEnabled(skill.name, agent, deployment.disabled);
    } catch (err) {
      addToast({
        type: "error",
        title: deployment.disabled ? "Couldn't enable" : "Couldn't disable",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
      setPendingHarness(null);
    }
  };

  return (
    <details className="skill-detail-details">
      <summary>Details</summary>

      <div className="skill-detail-details-section">
        <h4>Locations</h4>
        {skill.deployments.length === 0 ? (
          <p className="skill-detail-content-empty">Known only from the lock file</p>
        ) : (
          <div className="skill-detail-deployment-list">
            {skill.deployments.map((d, i) => (
              <div key={`${d.agent}-${d.scope}-${i}`} className="skill-detail-deployment-row">
                <div className="skill-detail-deployment-row-main">
                  <span>{d.agent}</span>
                  <span className={`scope-badge ${d.scope}`}>{d.scope}</span>
                  {d.plugin && (
                    <span className="skill-detail-fact-label">
                      {d.plugin.name}
                      {d.plugin.version ? ` v${d.plugin.version}` : ""}
                    </span>
                  )}
                  {d.disabled && <span className="skill-detail-fact-warning">Disabled</span>}
                  {HARNESSES_WITH_PER_SKILL_DISABLE.includes(
                    agentIdFromDeploymentLabel(d.agent) ?? "",
                  ) && (
                    <button
                      className="skill-detail-deployment-row-reveal"
                      onClick={() => handleToggleHarness(d)}
                      disabled={pendingHarness === d.agent}
                      title={
                        d.disabled
                          ? `Enable for ${d.agent}`
                          : `Disable for ${d.agent}${d.disabled_by ? ` (${d.disabled_by})` : ""}`
                      }
                    >
                      {d.disabled ? "Enable" : "Disable"}
                    </button>
                  )}
                  <button
                    className="skill-detail-deployment-row-reveal"
                    onClick={() => handleReveal(d.path)}
                    title="Reveal in Finder"
                  >
                    <FolderOpen size={12} />
                  </button>
                </div>
                <div className="skill-detail-deployment-row-path">{d.path}</div>
                {d.is_symlink &&
                  (d.symlink_is_broken ? (
                    <div className="skill-detail-fact-warning">
                      <AlertTriangle size={11} />
                      broken link{d.symlink_error ? `: ${d.symlink_error}` : ""}
                    </div>
                  ) : (
                    <div className="skill-detail-deployment-row-symlink">
                      <Link2 size={11} />
                      <span>→ {d.symlink_target}</span>
                    </div>
                  ))}
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="skill-detail-details-section">
        <h4>Invocation</h4>
        {skillMdPath && (
          <div
            className="skill-detail-invocation-control"
            title="Both: model and user can invoke this skill. User only: the model won't auto-invoke it. Model only: not listed for the user to invoke directly."
          >
            {INVOCATION_POLICY_OPTIONS.map((option) => (
              <button
                key={option.value}
                className={`skill-detail-invocation-option ${skill.invocation === option.value ? "active" : ""}`}
                onClick={() => handleSetInvocation(option.value)}
                disabled={isSavingInvocation || skill.invocation === option.value}
              >
                {option.label}
              </button>
            ))}
          </div>
        )}
        {invocationEntries.length > 0 && (
          <div className="skill-detail-frontmatter">
            {invocationEntries.map((key) => (
              <div key={key} className="skill-detail-frontmatter-row">
                <span className="skill-detail-frontmatter-key">{key}</span>
                <span className="skill-detail-frontmatter-value">
                  {skill.frontmatter_fields[key]}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="skill-detail-details-section">
        <h4>Source</h4>
        <div className="skill-detail-facts-grid">
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Source</span>
            {skill.source_url ? (
              <a
                href={skill.source_url}
                target="_blank"
                rel="noreferrer"
                className="skill-detail-link"
              >
                {skill.source || skill.source_url}
              </a>
            ) : (
              <span>{skill.source || "manual/plugin"}</span>
            )}
          </div>
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Installed</span>
            <span>{installed}</span>
          </div>
          {updated !== "unknown" && (
            <div className="skill-detail-fact">
              <span className="skill-detail-fact-label">Updated</span>
              <span>{updated}</span>
            </div>
          )}
        </div>
      </div>
    </details>
  );
}
