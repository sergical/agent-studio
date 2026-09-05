// ============================================================================
// InstalledSkillHeader - Controls above a responsive identity and source
// ledger area, followed by any blocking violation for the rendered copy.
// ============================================================================

import type { RefObject } from "react";
import { ArrowLeft, AlertTriangle, MoreHorizontal, PanelRight } from "lucide-react";
import { Button } from "@skill-studio/ui";
import { isBlockingSpecViolation } from "@skill-studio/lib";
import { trialHoursLeft } from "@skill-studio/lib";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import { isFeatureEnabled } from "../../lib/feature-flags";
import type { ActiveView } from "../../store/appStore";
import { MenuControl, MenuItem, MenuSeparator } from "../ui/MenuControl";
import { TooltipControl } from "../ui/TooltipControl";
import { SKILL_ASSISTANT_DRAWER_ID } from "./SkillAssistantDrawer";
import { InstalledSkillSourceLedger } from "./InstalledSkillSourceLedger";
import { useSkillPageActions } from "./skill-page-actions";

interface InstalledSkillHeaderProps {
  skill: InstalledSkill;
  /** The deployment whose SKILL.md the page renders - the header's violation line follows it. */
  deployment?: Deployment;
  from: ActiveView;
  onBack: () => void;
  onRemoveComplete: () => void;
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

/**
 * The installed skill header keeps controls separate from identity and exact
 * lifecycle ownership facts. It shows no location or invocation details.
 */
export function InstalledSkillHeader({
  skill,
  deployment,
  from,
  onBack,
  onRemoveComplete,
  isAssistantOpen,
  onOpenAssistant,
  assistantTriggerRef,
}: InstalledSkillHeaderProps) {
  const actions = useSkillPageActions(skill, onRemoveComplete);
  const assistantEnabled = isFeatureEnabled("skill-assistant");
  const nonBlockingCount =
    skill.spec_violations.length - skill.spec_violations.filter(isBlockingSpecViolation).length;
  // The red violation line names only the deployment whose SKILL.md the page
  // actually renders - other deployments' violations show on their own
  // Locations rows instead (see `SkillLocationsCard`). The union in
  // `skill.spec_violations` (and its "N spec notes" chip above) is unchanged:
  // Home's spec-violation issue still relies on it covering every copy.
  const renderedDeployment = deployment ?? skill.deployments.find((d) => d.content_hash);
  const blockingViolations = (renderedDeployment?.spec_violations ?? []).filter(
    isBlockingSpecViolation,
  );

  return (
    <header className="flex flex-col gap-4">
      <div className="flex items-center justify-between gap-4">
        <button
          className="flex shrink-0 items-center gap-1.5 border-0 bg-transparent p-1 text-small text-text-tertiary transition-colors hover:text-text-primary"
          onClick={onBack}
          aria-label="Back"
        >
          <ArrowLeft size={16} />
          <span>{backLabel(from)}</span>
        </button>
        <div className="flex shrink-0 items-center gap-2">
          {actions.primaryAction && (
            <Button onClick={actions.primaryAction.run} disabled={actions.primaryAction.busy}>
              {actions.primaryAction.busy ? "Working…" : actions.primaryAction.label}
            </Button>
          )}
          {assistantEnabled && (
            <button
              ref={assistantTriggerRef}
              type="button"
              className="flex h-(--control-height) items-center gap-1.5 rounded-sm border border-border px-3 text-body text-text-secondary transition-colors hover:bg-bg-tertiary hover:text-text-primary aria-expanded:border-border-focus aria-expanded:text-accent"
              onClick={onOpenAssistant}
              aria-expanded={isAssistantOpen}
              aria-controls={isAssistantOpen ? SKILL_ASSISTANT_DRAWER_ID : undefined}
              title="Assistant"
            >
              <PanelRight size={16} />
              <span>Assistant</span>
            </button>
          )}
          <MenuControl
            triggerClassName="flex h-(--control-height) w-(--control-height) cursor-pointer items-center justify-center rounded-sm border border-border text-text-secondary transition-colors hover:bg-bg-tertiary hover:text-text-primary"
            triggerAriaLabel="More actions"
            trigger={<MoreHorizontal size={16} />}
            align="end"
          >
            <MenuItem closeOnClick onClick={actions.reveal} disabled={!actions.path}>
              Reveal in Finder
            </MenuItem>
            <MenuItem closeOnClick onClick={actions.openEditor} disabled={!actions.path}>
              Open in editor
            </MenuItem>
            <MenuItem closeOnClick onClick={actions.copyPath} disabled={!actions.path}>
              Copy path
            </MenuItem>
            <MenuSeparator />
            <MenuItem
              closeOnClick
              onClick={actions.parkAction.run}
              disabled={actions.parkAction.busy}
            >
              {actions.parkAction.label}
            </MenuItem>
            {actions.forkAction && (
              <MenuItem
                closeOnClick
                onClick={actions.forkAction.run}
                disabled={actions.forkAction.busy}
              >
                {actions.forkAction.label}
              </MenuItem>
            )}
            {actions.removeAction && (
              <>
                <MenuSeparator />
                <MenuItem
                  closeOnClick
                  variant="destructive"
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

      <div className="grid w-full grid-cols-1 gap-6 min-[900px]:grid-cols-[minmax(0,1.35fr)_minmax(260px,1fr)] min-[900px]:gap-8">
        <div className="flex min-w-0 flex-col gap-4">
          <div>
            <h1 className="text-title font-semibold text-text-primary">{skill.name}</h1>
            {skill.description && (
              <p className="mt-3 max-w-[65ch] text-pretty text-body leading-[1.5] text-text-secondary">
                {skill.description}
              </p>
            )}
          </div>

          <div className="flex flex-wrap items-center gap-1.5">
            {skill.parked && (
              <span className="inline-flex items-center gap-1 rounded-full bg-bg-tertiary px-2 py-0.5 text-caption text-warning">
                {parkedChipLabel(skill.parked_at)}
              </span>
            )}
            {(skill.trials ?? (skill.trial ? [skill.trial] : [])).map((trial) => {
              const keepAction = actions.keepTrial(trial);
              const scope = trial.scope === "global" ? "Global" : "Project";
              return (
                <span
                  key={trial.deployment_id || `${trial.scope}/${trial.project_path ?? ""}`}
                  className="inline-flex items-center gap-1 rounded-full bg-bg-tertiary px-2 py-0.5 text-caption text-accent"
                >
                  {scope}{" "}
                  {trial.status === "recovery-required"
                    ? "expiry needs review"
                    : trial.status === "expiring"
                      ? "expiry in progress"
                      : trialChipLabel(trial.expires_at)}
                  <button
                    type="button"
                    className="cursor-pointer rounded-sm border-0 bg-bg-primary px-1.5 py-px text-caption font-semibold text-inherit disabled:cursor-not-allowed disabled:opacity-50"
                    onClick={keepAction.run}
                    disabled={keepAction.busy}
                  >
                    Keep
                  </button>
                </span>
              );
            })}
            {skill.update_owner_ids.length > 0 && (
              <span className="inline-flex items-center gap-1 rounded-full bg-bg-tertiary px-2 py-0.5 text-caption text-accent">
                Update available
              </span>
            )}
            {nonBlockingCount > 0 && (
              <TooltipControl
                content={skill.spec_violations
                  .filter((violation) => !isBlockingSpecViolation(violation))
                  .join("; ")}
              >
                <span className="inline-flex items-center gap-1 rounded-full bg-bg-tertiary px-2 py-0.5 text-caption text-text-tertiary">
                  {nonBlockingCount} spec note{nonBlockingCount !== 1 ? "s" : ""}
                </span>
              </TooltipControl>
            )}
          </div>
        </div>

        <InstalledSkillSourceLedger skill={skill} />
      </div>

      {blockingViolations.length > 0 && (
        <div className="flex items-center gap-1.5 text-small text-error">
          <AlertTriangle size={13} />
          <span>{blockingViolations.join("; ")}</span>
        </div>
      )}
    </header>
  );
}
