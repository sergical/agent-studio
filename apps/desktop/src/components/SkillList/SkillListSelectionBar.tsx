// ============================================================================
// SkillListSelectionBar - The docked bar that appears once any row is
// checked: the selected count, the bulk actions (Park/Unpark, Invocation,
// Update, Remove), and Cancel. Each action runs the single-skill call on every
// selected skill it applies to (see skill-bulk-actions.ts); a button that
// applies to none is disabled and its tooltip says why. Pack creation stays
// deferred (unit 4.3).
// ============================================================================

import { useState } from "react";
import type { ReactNode } from "react";
import { ChevronDown } from "lucide-react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  Button,
} from "@skill-studio/ui";
import type { InstalledSkill } from "@skill-studio/lib";
import { INVOCATION_POLICY_OPTIONS } from "../SkillDetail/skill-location-status";
import { MenuControl, MenuItem } from "../ui/MenuControl";
import { TooltipControl } from "../ui/TooltipControl";
import { bulkDisabledReason, describeSkipped, planBulkAction } from "./skill-bulk-actions";
import type { BulkAction } from "./skill-bulk-actions";
import { UpdateOverwritesEditsDialog } from "../SkillDetail/UpdateOverwritesEditsDialog";
import { useSkillBulkActions } from "./useSkillBulkActions";

interface SkillListSelectionBarProps {
  /** The skills whose rows are checked, in list order. */
  selectedSkills: InstalledSkill[];
  onCancel: () => void;
  /** Called after a bulk action; the caller clears the selection unless `hadFailures`. */
  onActionFinished: (hadFailures: boolean) => void;
}

const BAR_BUTTON_CLASS = "rounded-sm px-2 text-text-secondary";

/** A bar button; when `disabledReason` is set it is disabled and a tooltip gives the reason. */
function BarButton({
  disabledReason,
  busy,
  onClick,
  children,
  className,
}: {
  disabledReason: string | null;
  busy: boolean;
  onClick: () => void;
  children: ReactNode;
  className?: string;
}) {
  const button = (
    <Button
      variant="outline"
      size="sm"
      className={className ?? BAR_BUTTON_CLASS}
      onClick={onClick}
      disabled={busy || disabledReason !== null}
    >
      {children}
    </Button>
  );
  if (disabledReason === null) return button;
  return (
    <TooltipControl content={disabledReason}>
      <span className="inline-flex">{button}</span>
    </TooltipControl>
  );
}

export function SkillListSelectionBar({
  selectedSkills,
  onCancel,
  onActionFinished,
}: SkillListSelectionBarProps) {
  const [confirmingRemove, setConfirmingRemove] = useState(false);
  const { progress, run, updatePrompt } = useSkillBulkActions(onActionFinished);
  const busy = progress !== null;

  const parkPlan = planBulkAction(selectedSkills, { kind: "park" });
  const unparkPlan = planBulkAction(selectedSkills, { kind: "unpark" });
  const invocationAction: BulkAction = { kind: "invocation", policy: "both" };
  const invocationPlan = planBulkAction(selectedSkills, invocationAction);
  const updatePlan = planBulkAction(selectedSkills, { kind: "update" });
  const removePlan = planBulkAction(selectedSkills, { kind: "remove" });

  const invocationDisabled = bulkDisabledReason(invocationAction, invocationPlan);
  const showPark = parkPlan.applicable.length > 0 || unparkPlan.applicable.length === 0;
  const showUnpark = unparkPlan.applicable.length > 0;

  const handleConfirmRemove = () => {
    setConfirmingRemove(false);
    void run({ kind: "remove" }, selectedSkills);
  };

  return (
    // A zero-height wrapper so the sticky bar never reserves flow space of its own - checking a
    // row must not push any other row down. `sticky bottom-4` then docks the bar to the bottom of
    // the scroll area without an enter transition.
    <div className="pointer-events-none sticky inset-x-0 bottom-4 z-10 flex h-0 items-end justify-center">
      <div className="pointer-events-auto flex h-9 max-w-[calc(100%-2rem)] items-center gap-1.5 rounded-md border border-border bg-bg-secondary px-2 shadow">
        <span
          className="min-w-0 truncate px-1 text-small whitespace-nowrap text-text-secondary tabular-nums"
          aria-live="polite"
        >
          {progress ?? `${selectedSkills.length} selected`}
        </span>
        {showPark && (
          <BarButton
            busy={busy}
            disabledReason={bulkDisabledReason({ kind: "park" }, parkPlan)}
            onClick={() => void run({ kind: "park" }, selectedSkills)}
          >
            Park {parkPlan.applicable.length}
          </BarButton>
        )}
        {showUnpark && (
          <BarButton
            busy={busy}
            disabledReason={bulkDisabledReason({ kind: "unpark" }, unparkPlan)}
            onClick={() => void run({ kind: "unpark" }, selectedSkills)}
          >
            Unpark {unparkPlan.applicable.length}
          </BarButton>
        )}
        {invocationDisabled !== null || busy ? (
          <BarButton busy={busy} disabledReason={invocationDisabled} onClick={() => undefined}>
            Invocation
            <ChevronDown size={12} />
          </BarButton>
        ) : (
          <MenuControl
            align="end"
            triggerAriaLabel={`Set invocation on ${invocationPlan.applicable.length} skills`}
            triggerClassName="inline-flex h-7 cursor-pointer items-center gap-1 rounded-sm border border-border bg-transparent px-2 text-small text-text-secondary hover:bg-bg-hover"
            trigger={
              <>
                Invocation
                <ChevronDown size={12} />
              </>
            }
          >
            {INVOCATION_POLICY_OPTIONS.map((option) => (
              <MenuItem
                key={option.value}
                onClick={() =>
                  void run({ kind: "invocation", policy: option.value }, selectedSkills)
                }
              >
                {option.label}
              </MenuItem>
            ))}
          </MenuControl>
        )}
        <BarButton
          busy={busy}
          disabledReason={bulkDisabledReason({ kind: "update" }, updatePlan)}
          onClick={() => void run({ kind: "update" }, selectedSkills)}
        >
          Update {updatePlan.applicable.length}
        </BarButton>
        <BarButton
          busy={busy}
          disabledReason={bulkDisabledReason({ kind: "remove" }, removePlan)}
          onClick={() => setConfirmingRemove(true)}
          className="rounded-sm px-2 text-error"
        >
          Remove
        </BarButton>
        <Button
          variant="outline"
          size="sm"
          className="rounded-sm text-text-tertiary"
          onClick={onCancel}
          disabled={busy}
        >
          Cancel
        </Button>
      </div>

      <UpdateOverwritesEditsDialog
        skillNames={updatePrompt?.skillNames ?? []}
        isBulk
        canFork={updatePrompt?.canFork ?? true}
        onFork={() => updatePrompt?.fork()}
        onOverwrite={() => updatePrompt?.overwrite()}
        onCancel={() => updatePrompt?.cancel()}
      />

      <AlertDialog open={confirmingRemove} onOpenChange={setConfirmingRemove}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              Remove {removePlan.applicable.length} skill
              {removePlan.applicable.length === 1 ? "" : "s"}?
            </AlertDialogTitle>
            <AlertDialogDescription>
              This removes each skill's managed deployments and their verified dependent links.
              Independent copies outside those groups remain. This cannot be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <ul className="m-0 max-h-48 list-none overflow-y-auto p-0 text-small text-text-primary">
            {removePlan.applicable.map((skill) => (
              <li key={skill.name}>{skill.name}</li>
            ))}
          </ul>
          {removePlan.skipped.length > 0 && (
            <div className="text-small text-text-tertiary">
              <p className="m-0">Skipped ({describeSkipped(removePlan.skipped)}):</p>
              <ul className="m-0 max-h-32 list-none overflow-y-auto p-0">
                {removePlan.skipped.map(({ skill, reason }) => (
                  <li key={skill.name}>
                    {skill.name} · {reason}
                  </li>
                ))}
              </ul>
            </div>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={handleConfirmRemove}
              disabled={removePlan.applicable.length === 0}
            >
              Remove {removePlan.applicable.length}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
