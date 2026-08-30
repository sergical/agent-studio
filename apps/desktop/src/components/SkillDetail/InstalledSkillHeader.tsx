// ============================================================================
// InstalledSkillHeader - Row 1: back button, name, one primary action, and an
// overflow menu holding every other action. Row 2: description. Row 3: the
// chip row (provenance, parked, trial, update available, spec notes). Row 4:
// a quiet tabular metadata line.
// ============================================================================

import type { RefObject } from "react";
import { ArrowLeft, AlertTriangle, MoreHorizontal, PanelRight } from "lucide-react";
import { isBlockingSpecViolation } from "../../lib/skill-health";
import { pluginLabelForSkill } from "../../lib/skill-plugin-partition";
import type { SkillRunSummary } from "../../lib/skill-run-history-types";
import { formatBytes, formatRelativeTime, formatTokens } from "../../lib/skill-stats";
import { SOURCE_KIND_LABELS, trialHoursLeft } from "../../lib/skill-types";
import type { InstalledSkill, SkillInvocationStats } from "../../lib/skill-types";
import type { ActiveView } from "../../store/appStore";
import { MenuControl, MenuItem, MenuSeparator } from "../ui/MenuControl";
import { SKILL_ASSISTANT_DRAWER_ID } from "./SkillAssistantDrawer";
import { useSkillPageActions } from "./skill-page-actions";

interface InstalledSkillHeaderProps {
  skill: InstalledSkill;
  from: ActiveView;
  onBack: () => void;
  onRemoveComplete: () => void;
  invocationStats: SkillInvocationStats | undefined;
  /** The newest "Test" run recorded for this skill - `undefined` when it was never tested. */
  lastTest?: SkillRunSummary;
  /** Opens the "Runs" history list in the assistant panel. */
  onOpenHistory: () => void;
  /** Whether the assistant drawer is open, for the trigger's `aria-expanded`. */
  isAssistantOpen: boolean;
  onOpenAssistant: () => void;
  /** So the drawer can return focus here when it closes. */
  assistantTriggerRef: RefObject<HTMLButtonElement | null>;
}

/** The back button's label: the name of the view the page was opened from. */
function backLabel(from: ActiveView): string {
  switch (from.kind) {
    case "home":
      return "Home";
    case "skills":
      return "Skills";
    case "activity":
      return "Activity";
    case "packs":
      return "Packs";
    default:
      // `ActiveView`'s "skill" kind never nests as its own `from` (see `openSkill`).
      return "Back";
  }
}

/** "dotagents" / "skills.sh" / "plugin · openai-templates" / "manual" / "fork". */
function provenanceChipLabel(skill: InstalledSkill): string {
  if (skill.source_kind === "plugin") {
    const pluginName = pluginLabelForSkill(skill);
    return pluginName ? `plugin · ${pluginName}` : SOURCE_KIND_LABELS.plugin;
  }
  return SOURCE_KIND_LABELS[skill.source_kind];
}

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

/** "12 Mar 2026" - the metadata line's absolute install date, `null` for an unparseable/missing date. */
function absoluteDate(iso: string): string | null {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return null;
  return date.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

/** "passed 2 h ago on Claude Code" / "failed 2 h ago on Codex" / "ran 2 h ago on pi". */
function lastTestLabel(lastTest: SkillRunSummary): string {
  const outcome = lastTest.passed === undefined ? "ran" : lastTest.passed ? "passed" : "failed";
  return `last test ${outcome} ${formatRelativeTime(lastTest.at)} on ${lastTest.harness}`;
}

/**
 * The page header: back button and name with the one primary action and an
 * overflow menu on the right (row 1); description (row 2); provenance/parked/
 * trial/update/spec-notes chips, with blocking violations rendered as a
 * warning line instead of a chip (row 3); a quiet tabular metadata line
 * (row 4).
 */
export function InstalledSkillHeader({
  skill,
  from,
  onBack,
  onRemoveComplete,
  invocationStats,
  lastTest,
  onOpenHistory,
  isAssistantOpen,
  onOpenAssistant,
  assistantTriggerRef,
}: InstalledSkillHeaderProps) {
  const actions = useSkillPageActions(skill, onRemoveComplete);
  const blockingViolations = skill.spec_violations.filter(isBlockingSpecViolation);
  const nonBlockingCount = skill.spec_violations.length - blockingViolations.length;

  const modifiedRelative = skill.modified_at ? formatRelativeTime(skill.modified_at) : undefined;
  const installedDate = absoluteDate(skill.installed_at);
  const metaSegments = [
    formatBytes(skill.folder_bytes),
    `${formatTokens(skill.skill_md_tokens)} tokens`,
    `${invocationStats?.last_30_days ?? 0} uses in 30 days`,
    modifiedRelative && `edited ${modifiedRelative}`,
    skill.source && skill.source !== "local" && `source ${skill.source}`,
    installedDate && `installed ${installedDate}`,
  ].filter((segment): segment is string => Boolean(segment));

  return (
    <div className="skill-page-header">
      <div className="skill-page-header-row-1">
        <button className="skill-page-back" onClick={onBack} aria-label="Back">
          <ArrowLeft size={16} />
          <span>{backLabel(from)}</span>
        </button>
        <h1 className="skill-page-name">{skill.name}</h1>
        <div className="skill-page-header-actions">
          {actions.primaryAction && (
            <button
              type="button"
              className="skill-action-button primary"
              onClick={actions.primaryAction.run}
              disabled={actions.primaryAction.busy}
            >
              {actions.primaryAction.busy ? "Working…" : actions.primaryAction.label}
            </button>
          )}
          <button
            ref={assistantTriggerRef}
            type="button"
            className="skill-page-assistant-trigger"
            onClick={onOpenAssistant}
            aria-expanded={isAssistantOpen}
            aria-controls={SKILL_ASSISTANT_DRAWER_ID}
            title="Assistant"
          >
            <PanelRight size={16} />
            <span>Assistant</span>
          </button>
          <MenuControl
            triggerClassName="skill-page-overflow-trigger"
            triggerAriaLabel="More actions"
            trigger={<MoreHorizontal size={16} />}
            align="end"
          >
            <MenuItem
              closeOnClick
              className="menu-control-item"
              onClick={actions.reveal}
              disabled={!actions.path}
            >
              Reveal in Finder
            </MenuItem>
            <MenuItem
              closeOnClick
              className="menu-control-item"
              onClick={actions.openEditor}
              disabled={!actions.path}
            >
              Open in editor
            </MenuItem>
            <MenuItem
              closeOnClick
              className="menu-control-item"
              onClick={actions.copyPath}
              disabled={!actions.path}
            >
              Copy path
            </MenuItem>
            <MenuSeparator className="menu-control-separator" />
            <MenuItem
              closeOnClick
              className="menu-control-item"
              onClick={actions.parkAction.run}
              disabled={actions.parkAction.busy}
            >
              {actions.parkAction.label}
            </MenuItem>
            {actions.forkAction && (
              <MenuItem
                closeOnClick
                className="menu-control-item"
                onClick={actions.forkAction.run}
                disabled={actions.forkAction.busy}
              >
                {actions.forkAction.label}
              </MenuItem>
            )}
            {actions.removeAction && (
              <>
                <MenuSeparator className="menu-control-separator" />
                <MenuItem
                  closeOnClick
                  className="menu-control-item"
                  data-danger=""
                  onClick={actions.removeAction.run}
                  disabled={actions.removeAction.busy}
                >
                  Remove
                </MenuItem>
              </>
            )}
          </MenuControl>
        </div>
      </div>

      {skill.description && <p className="skill-page-description">{skill.description}</p>}

      <div className="skill-page-chip-row">
        <span className={`skill-page-chip provenance ${skill.source_kind}`}>
          {provenanceChipLabel(skill)}
        </span>
        {skill.parked && (
          <span className="skill-page-chip parked">{parkedChipLabel(skill.parked_at)}</span>
        )}
        {skill.trial && (
          <span className="skill-page-chip trial">
            {trialChipLabel(skill.trial.expires_at)}
            <button
              type="button"
              className="skill-detail-trial-keep"
              onClick={actions.keepTrial.run}
              disabled={actions.keepTrial.busy}
            >
              Keep
            </button>
          </span>
        )}
        {skill.has_update && (
          <span className="skill-page-chip update-available">Update available</span>
        )}
        {nonBlockingCount > 0 && (
          <span
            className="skill-page-chip spec-notes"
            title={skill.spec_violations.filter((v) => !isBlockingSpecViolation(v)).join("; ")}
          >
            {nonBlockingCount} spec note{nonBlockingCount !== 1 ? "s" : ""}
          </span>
        )}
      </div>

      {blockingViolations.length > 0 && (
        <div className="skill-page-blocking-warning">
          <AlertTriangle size={13} />
          <span>{blockingViolations.join("; ")}</span>
        </div>
      )}

      <div className="skill-page-meta-line">
        {metaSegments.join(" · ")}
        {lastTest && (
          <>
            {metaSegments.length > 0 && " · "}
            <button type="button" className="skill-page-meta-link" onClick={onOpenHistory}>
              {lastTestLabel(lastTest)}
            </button>
          </>
        )}
      </div>
    </div>
  );
}
