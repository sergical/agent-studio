// ============================================================================
// InstalledSkillHeader - Row 1: back button, name, one primary action, and an
// overflow menu holding every other action. Row 2: description. Row 3: the
// chip row (provenance, parked, trial, update available, spec notes). Row 4:
// a quiet tabular metadata line.
// ============================================================================

import type { RefObject } from "react";
import { ArrowLeft, AlertTriangle, MoreHorizontal, PanelRight } from "lucide-react";
import { Button } from "@skill-studio/ui";
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

/** Reproduces the shared `.menu-control-item` look inline - see SkillListFilterBar's copy of the same class. */
const MENU_ITEM_CLASS =
  "flex h-(--control-height) cursor-pointer items-center gap-2 rounded-sm px-2.5 text-body text-text-secondary transition-colors data-highlighted:bg-bg-hover data-highlighted:text-text-primary data-disabled:cursor-not-allowed data-disabled:text-text-quaternary";
const MENU_ITEM_DANGER_CLASS =
  "flex h-(--control-height) cursor-pointer items-center gap-2 rounded-sm px-2.5 text-body text-error transition-colors data-highlighted:bg-error-soft data-highlighted:text-error";
const MENU_SEPARATOR_CLASS = "mx-0.5 my-1 h-px border-none bg-border-subtle";

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
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-4">
        <button
          className="flex shrink-0 items-center gap-1.5 border-0 bg-transparent p-1 text-small text-text-tertiary transition-colors hover:text-text-primary"
          onClick={onBack}
          aria-label="Back"
        >
          <ArrowLeft size={16} />
          <span>{backLabel(from)}</span>
        </button>
        <h1 className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-title font-semibold text-text-primary">
          {skill.name}
        </h1>
        <div className="flex shrink-0 items-center gap-2">
          {actions.primaryAction && (
            <Button onClick={actions.primaryAction.run} disabled={actions.primaryAction.busy}>
              {actions.primaryAction.busy ? "Working…" : actions.primaryAction.label}
            </Button>
          )}
          <button
            ref={assistantTriggerRef}
            type="button"
            className="flex h-(--control-height) items-center gap-1.5 rounded-sm border border-border px-3 text-body text-text-secondary transition-colors hover:bg-bg-tertiary hover:text-text-primary aria-expanded:border-border-focus aria-expanded:text-accent"
            onClick={onOpenAssistant}
            aria-expanded={isAssistantOpen}
            aria-controls={SKILL_ASSISTANT_DRAWER_ID}
            title="Assistant"
          >
            <PanelRight size={16} />
            <span>Assistant</span>
          </button>
          <MenuControl
            triggerClassName="flex h-(--control-height) w-(--control-height) cursor-pointer items-center justify-center rounded-sm border border-border text-text-secondary transition-colors hover:bg-bg-tertiary hover:text-text-primary"
            triggerAriaLabel="More actions"
            trigger={<MoreHorizontal size={16} />}
            align="end"
          >
            <MenuItem
              closeOnClick
              className={MENU_ITEM_CLASS}
              onClick={actions.reveal}
              disabled={!actions.path}
            >
              Reveal in Finder
            </MenuItem>
            <MenuItem
              closeOnClick
              className={MENU_ITEM_CLASS}
              onClick={actions.openEditor}
              disabled={!actions.path}
            >
              Open in editor
            </MenuItem>
            <MenuItem
              closeOnClick
              className={MENU_ITEM_CLASS}
              onClick={actions.copyPath}
              disabled={!actions.path}
            >
              Copy path
            </MenuItem>
            <MenuSeparator className={MENU_SEPARATOR_CLASS} />
            <MenuItem
              closeOnClick
              className={MENU_ITEM_CLASS}
              onClick={actions.parkAction.run}
              disabled={actions.parkAction.busy}
            >
              {actions.parkAction.label}
            </MenuItem>
            {actions.forkAction && (
              <MenuItem
                closeOnClick
                className={MENU_ITEM_CLASS}
                onClick={actions.forkAction.run}
                disabled={actions.forkAction.busy}
              >
                {actions.forkAction.label}
              </MenuItem>
            )}
            {actions.removeAction && (
              <>
                <MenuSeparator className={MENU_SEPARATOR_CLASS} />
                <MenuItem
                  closeOnClick
                  className={MENU_ITEM_DANGER_CLASS}
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

      {skill.description && (
        <p className="text-pretty text-body leading-[1.5] text-text-secondary">
          {skill.description}
        </p>
      )}

      <div className="flex flex-wrap items-center gap-1.5">
        <span className="inline-flex items-center gap-1 rounded-full bg-bg-tertiary px-2 py-0.5 text-caption text-text-tertiary">
          {provenanceChipLabel(skill)}
        </span>
        {skill.parked && (
          <span className="inline-flex items-center gap-1 rounded-full bg-bg-tertiary px-2 py-0.5 text-caption text-warning">
            {parkedChipLabel(skill.parked_at)}
          </span>
        )}
        {skill.trial && (
          <span className="inline-flex items-center gap-1 rounded-full bg-bg-tertiary px-2 py-0.5 text-caption text-accent">
            {trialChipLabel(skill.trial.expires_at)}
            <button
              type="button"
              className="cursor-pointer rounded-sm border-0 bg-bg-primary px-1.5 py-px text-caption font-semibold text-inherit disabled:cursor-not-allowed disabled:opacity-50"
              onClick={actions.keepTrial.run}
              disabled={actions.keepTrial.busy}
            >
              Keep
            </button>
          </span>
        )}
        {skill.has_update && (
          <span className="inline-flex items-center gap-1 rounded-full bg-bg-tertiary px-2 py-0.5 text-caption text-accent">
            Update available
          </span>
        )}
        {nonBlockingCount > 0 && (
          <span
            className="inline-flex items-center gap-1 rounded-full bg-bg-tertiary px-2 py-0.5 text-caption text-text-tertiary"
            title={skill.spec_violations.filter((v) => !isBlockingSpecViolation(v)).join("; ")}
          >
            {nonBlockingCount} spec note{nonBlockingCount !== 1 ? "s" : ""}
          </span>
        )}
      </div>

      {blockingViolations.length > 0 && (
        <div className="flex items-center gap-1.5 text-small text-error">
          <AlertTriangle size={13} />
          <span>{blockingViolations.join("; ")}</span>
        </div>
      )}

      <div className="text-small text-text-tertiary">
        {metaSegments.join(" · ")}
        {lastTest && (
          <>
            {metaSegments.length > 0 && " · "}
            <button
              type="button"
              className="cursor-pointer border-0 bg-transparent p-0 font-inherit text-small text-text-tertiary hover:text-text-secondary hover:underline"
              onClick={onOpenHistory}
            >
              {lastTestLabel(lastTest)}
            </button>
          </>
        )}
      </div>
    </div>
  );
}
