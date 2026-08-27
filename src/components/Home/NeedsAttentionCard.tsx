// ============================================================================
// NeedsAttentionCard - Home's "Needs attention" block: one row per issue
// (not grouped by kind), with a one-click fix where one exists.
// ============================================================================

import { CheckCircle2 } from "lucide-react";
import { useState } from "react";
import { unparkSkill } from "../../lib/skill-api";
import { HEALTH_ISSUE_SEVERITY } from "../../lib/skill-health";
import type { HealthIssue, HealthIssueKind } from "../../lib/skill-health";
import { useAppStore } from "../../store/appStore";

const MAX_ROWS = 8;

interface NeedsAttentionCardProps {
  issues: HealthIssue[];
  /** Opens the given skill's page. */
  onSelectSkill: (name: string) => void;
}

/** The row-level action label for one issue kind - see spec-ux-2.md section A.2 for which kinds get a one-click fix. */
function actionLabel(kind: HealthIssueKind): string {
  switch (kind) {
    case "parked-but-reinstalled":
      return "Unpark";
    case "duplicate":
      return "Compare";
    // No command removes a deployment/symlink yet, so a broken link just opens the skill.
    case "broken-symlink":
    case "spec-violation":
    case "lock-only":
      return "Open";
  }
}

/** One "Needs attention" row's action button: unparks in place, everything else opens the skill. */
function NeedsAttentionAction({
  issue,
  onSelectSkill,
}: {
  issue: HealthIssue;
  onSelectSkill: (name: string) => void;
}) {
  const [isUnparking, setIsUnparking] = useState(false);
  const addToast = useAppStore((state) => state.addToast);
  const openSkill = useAppStore((state) => state.openSkill);

  if (issue.kind === "duplicate") {
    return (
      <button
        className="home-needs-attention-action"
        onClick={() => openSkill(issue.skill.name, undefined, "compare")}
      >
        {actionLabel(issue.kind)}
      </button>
    );
  }

  if (issue.kind !== "parked-but-reinstalled") {
    return (
      <button
        className="home-needs-attention-action"
        onClick={() => onSelectSkill(issue.skill.name)}
      >
        {actionLabel(issue.kind)}
      </button>
    );
  }

  const handleUnpark = async () => {
    setIsUnparking(true);
    try {
      await unparkSkill(issue.skill.name);
      addToast({ type: "success", title: `Unparked ${issue.skill.name}` });
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't unpark skill",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
      setIsUnparking(false);
    }
  };

  return (
    <button className="home-needs-attention-action" onClick={handleUnpark} disabled={isUnparking}>
      {isUnparking ? "Unparking…" : "Unpark"}
    </button>
  );
}

/**
 * One row per issue (max 8): severity dot, skill name, detail, and a
 * one-click fix where one exists. "Show all N" links to the Skills view
 * filtered to every issue, not just the shown rows' kinds. Empty state
 * collapses to a single quiet line, no card chrome.
 */
export function NeedsAttentionCard({ issues, onSelectSkill }: NeedsAttentionCardProps) {
  const setSkillListFilter = useAppStore((state) => state.setSkillListFilter);
  const setActiveView = useAppStore((state) => state.setActiveView);

  if (issues.length === 0) {
    return (
      <p className="home-needs-attention-empty">
        <CheckCircle2 size={14} className="home-needs-attention-empty-icon" />
        Nothing needs attention
      </p>
    );
  }

  const shown = issues.slice(0, MAX_ROWS);

  const handleShowAll = () => {
    setSkillListFilter({ scope: "all", query: "", issue: "any" });
    setActiveView({ kind: "skills" });
  };

  return (
    <div className="home-needs-attention">
      {shown.map((issue, i) => (
        <div key={`${issue.kind}-${issue.skill.name}-${i}`} className="home-needs-attention-row">
          <span className={`home-needs-attention-dot ${HEALTH_ISSUE_SEVERITY[issue.kind]}`} />
          <button
            className="home-needs-attention-name"
            onClick={() => onSelectSkill(issue.skill.name)}
          >
            {issue.skill.name}
          </button>
          <span className="home-needs-attention-detail">{issue.detail}</span>
          <NeedsAttentionAction issue={issue} onSelectSkill={onSelectSkill} />
        </div>
      ))}
      {issues.length > MAX_ROWS && (
        <button className="home-needs-attention-more" onClick={handleShowAll}>
          Show all {issues.length}
        </button>
      )}
    </div>
  );
}
