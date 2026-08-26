// ============================================================================
// InstalledSkillHeader - Name, description, badges, and the icon-button row
// (reveal, open in editor, copy path, enable/disable)
// ============================================================================

import { useState } from "react";
import {
  AlertTriangle,
  Copy,
  FolderOpen,
  Link2,
  PackageOpen,
  TerminalSquare,
  Unlink,
} from "lucide-react";
import { deploymentLinkKind } from "../../lib/skill-coverage";
import { keepSkillTrial, openSkillPath, parkSkill, unparkSkill } from "../../lib/skill-api";
import { pluginLabelForSkill } from "../../lib/skill-plugin-partition";
import type { SkillRunSummary } from "../../lib/skill-run-history-types";
import { formatRelativeTime, shortSha } from "../../lib/skill-stats";
import { SOURCE_KIND_LABELS, trialHoursLeft } from "../../lib/skill-types";
import type { InstalledSkill } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";

/** "Parked · Aug 25, 2026" / "Parked" when the timestamp is missing or unparseable. */
function parkedChipLabel(parkedAt: string | undefined): string {
  if (!parkedAt) return "Parked";
  const date = new Date(parkedAt);
  if (Number.isNaN(date.getTime())) return "Parked";
  return `Parked · ${date.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" })}`;
}

/** "Trial · 17 h" / "Trial · <1 h" / "Trial · expired". */
function trialChipLabel(expiresAt: string): string {
  const hours = trialHoursLeft(expiresAt);
  if (hours < 0) return "Trial · expired";
  if (hours < 1) return "Trial · <1 h";
  return `Trial · ${hours} h`;
}

interface InstalledSkillHeaderProps {
  skill: InstalledSkill;
  /** The newest "Test" run recorded for this skill - `undefined` when it was never tested. */
  lastTest?: SkillRunSummary;
  /** Opens the "Runs" history list in the assistant panel. */
  onOpenHistory?: () => void;
}

/** "Last test: passed 2 h ago on Codex" / "failed …" / "never tested". */
function lastTestLabel(lastTest: SkillRunSummary | undefined): string {
  if (!lastTest) return "Last test: never tested";
  const outcome = lastTest.passed === undefined ? "ran" : lastTest.passed ? "passed" : "failed";
  return `Last test: ${outcome} ${formatRelativeTime(lastTest.at)} on ${lastTest.harness}`;
}

/** "skills.sh" / "plugin: openai-templates" / ".agents" / "manual". */
function sourceKindBadgeLabel(skill: InstalledSkill): string {
  if (skill.source_kind === "plugin") {
    const pluginName = pluginLabelForSkill(skill);
    return pluginName ? `plugin: ${pluginName}` : SOURCE_KIND_LABELS.plugin;
  }
  return SOURCE_KIND_LABELS[skill.source_kind];
}

/**
 * The link marker for one agent's chip: worst case across every deployment
 * under that agent label (broken beats linked-to-shared beats plain own; a
 * shared-root deployment already has its own "shared" chip, so it never
 * gets an extra marker here).
 */
function chipLinkMarker(
  skill: InstalledSkill,
  agent: string,
): "broken" | "linked-to-shared" | null {
  const kinds = skill.deployments.filter((d) => d.agent === agent).map(deploymentLinkKind);
  if (kinds.includes("broken")) return "broken";
  if (kinds.includes("linked-to-shared")) return "linked-to-shared";
  return null;
}

/**
 * Header: name, description, a badge row (source kind, agent chips, a
 * "Parked" chip, and a spec-violation warning when any), and an icon-button
 * row for the file actions every installed skill supports, including
 * park/unpark (disable globally - see `skill_park.rs`).
 */
export function InstalledSkillHeader({
  skill,
  lastTest,
  onOpenHistory,
}: InstalledSkillHeaderProps) {
  const [copied, setCopied] = useState(false);
  const [isKeeping, setIsKeeping] = useState(false);
  const [isParking, setIsParking] = useState(false);
  const addToast = useAppStore((state) => state.addToast);
  const path = skill.deployments[0]?.path ?? skill.skill_path;
  const agents = [...new Set(skill.deployments.map((d) => d.agent))];

  const handleTogglePark = async () => {
    setIsParking(true);
    try {
      if (skill.parked) {
        await unparkSkill(skill.name);
        addToast({ type: "success", title: `Unparked ${skill.name}` });
      } else {
        await parkSkill(skill.name);
        addToast({ type: "success", title: `Parked ${skill.name}` });
      }
    } catch (err) {
      addToast({
        type: "error",
        title: skill.parked ? "Couldn't unpark skill" : "Couldn't park skill",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
      setIsParking(false);
    }
  };

  const handleKeepTrial = async () => {
    setIsKeeping(true);
    try {
      await keepSkillTrial(skill.name, skill.trial?.scope ?? "global", skill.trial?.project_path);
      addToast({ type: "success", title: `Kept ${skill.name}` });
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't keep skill",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
      setIsKeeping(false);
    }
  };

  const handleReveal = async () => {
    if (!path) return;
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

  const handleOpenEditor = async () => {
    if (!path) return;
    try {
      await openSkillPath(path, "editor");
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't open in editor",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

  const handleCopyPath = async () => {
    if (!path) return;
    await navigator.clipboard.writeText(path);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className="skill-detail-header">
      <div className="skill-detail-header-body">
        <div className="skill-detail-title">
          <h2>{skill.name}</h2>
        </div>

        {skill.description && <p className="skill-detail-description">{skill.description}</p>}

        <button
          type="button"
          className={`skill-detail-last-test ${lastTest?.passed === false ? "failed" : ""}`}
          onClick={onOpenHistory}
          disabled={!onOpenHistory}
        >
          {lastTestLabel(lastTest)}
        </button>

        {skill.source_kind === "fork" && skill.fork && (
          <p className="skill-detail-fork-line">
            Forked from {skill.fork.origin_source} · base {shortSha(skill.fork.base_commit)} ·{" "}
            {formatRelativeTime(skill.fork.forked_at)}
          </p>
        )}

        {skill.has_update && (
          <p className="skill-detail-update-line">
            {skill.source_kind === "fork" ? "Upstream moved" : "Update available"}
            {skill.update_commit && ` · ${shortSha(skill.update_commit)}`}
            {skill.update_commit_at && ` · ${formatRelativeTime(skill.update_commit_at)}`}
          </p>
        )}

        <div className="skill-detail-badge-row">
          <span className={`skill-detail-badge source-kind ${skill.source_kind}`}>
            {sourceKindBadgeLabel(skill)}
          </span>
          {agents.map((agent) => {
            const harnessId = harnessIdFromLabel(agent);
            const linkMarker = chipLinkMarker(skill, agent);
            return (
              <span key={agent} className="skill-detail-agent-chip">
                {harnessId && <HarnessIcon harness={harnessId} size={12} />}
                {agent}
                {linkMarker === "linked-to-shared" && (
                  <span
                    className="skill-detail-agent-chip-marker"
                    title="Symlink to the shared folder"
                  >
                    <Link2 size={10} />
                  </span>
                )}
                {linkMarker === "broken" && (
                  <span className="skill-detail-agent-chip-marker broken" title="Broken link">
                    <Unlink size={10} />
                  </span>
                )}
              </span>
            );
          })}
          {skill.spec_violations.length > 0 && (
            <span
              className="skill-detail-badge spec-violation"
              title={skill.spec_violations.join("; ")}
            >
              <AlertTriangle size={12} />
              {skill.spec_violations.length} spec issue
              {skill.spec_violations.length !== 1 ? "s" : ""}
            </span>
          )}
          {skill.parked && (
            <span className="skill-detail-badge parked">{parkedChipLabel(skill.parked_at)}</span>
          )}
          {skill.trial && (
            <span className="skill-detail-badge trial">
              {trialChipLabel(skill.trial.expires_at)}
              <button
                type="button"
                className="skill-detail-trial-keep"
                onClick={handleKeepTrial}
                disabled={isKeeping}
              >
                Keep
              </button>
            </span>
          )}
        </div>

        <div className="skill-detail-icon-row">
          <button className="skill-detail-icon-button" onClick={handleReveal} disabled={!path}>
            <FolderOpen size={14} />
            Reveal in Finder
          </button>
          <button className="skill-detail-icon-button" onClick={handleOpenEditor} disabled={!path}>
            <TerminalSquare size={14} />
            Open in editor
          </button>
          <button className="skill-detail-icon-button" onClick={handleCopyPath} disabled={!path}>
            <Copy size={14} />
            {copied ? "Copied!" : "Copy path"}
          </button>
          <button
            className="skill-detail-icon-button"
            onClick={handleTogglePark}
            disabled={isParking}
          >
            <PackageOpen size={14} />
            {skill.parked ? "Unpark" : "Park"}
          </button>
        </div>
      </div>
    </div>
  );
}
