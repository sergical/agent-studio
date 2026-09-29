// ============================================================================
// SkillPageHeaderActions - The header bar's action cluster: the one primary
// action, Park and Fork, the assistant toggle, and the ⋯ overflow menu. Rendered as
// `PageShell`'s `actions` - pulled out of `InstalledSkillHeader`, which now
// shows identity only.
// ============================================================================

import type { ReactNode, RefObject } from "react";
import { CirclePause, CirclePlay, GitFork, MoreHorizontal, PanelRight } from "lucide-react";
import { Button } from "@skill-studio/ui";
import { MenuControl, MenuItem, MenuSeparator } from "../ui/MenuControl";
import { SKILL_ASSISTANT_DRAWER_ID } from "./SkillAssistantDrawer";
import type { SkillPageAction, SkillPageActions } from "./skill-page-actions";

// The header bar is 40px tall with gap-2 between controls: the hit area
// grows 6px up and down and 4px sideways, so neighbouring targets never overlap.
const HEADER_CONTROL_CLASS =
  "relative before:absolute before:-inset-x-1 before:-inset-y-1.5 transition-[color,background-color,border-color,box-shadow,transform] active:scale-[0.96]";

const SECONDARY_BUTTON_CLASS = `${HEADER_CONTROL_CLASS} h-(--control-height) gap-1.5 rounded-sm pr-3 pl-2.5 text-body`;

function SecondaryActionButton({ action, icon }: { action: SkillPageAction; icon: ReactNode }) {
  return (
    <Button
      variant="outline"
      className={SECONDARY_BUTTON_CLASS}
      onClick={action.run}
      disabled={action.busy}
    >
      {icon}
      <span>{action.label}</span>
    </Button>
  );
}

interface SkillPageHeaderActionsProps {
  actions: SkillPageActions;
  assistantEnabled: boolean;
  isAssistantOpen: boolean;
  onOpenAssistant: () => void;
  /** So the drawer can return focus here when it closes. */
  assistantTriggerRef: RefObject<HTMLButtonElement | null>;
}

export function SkillPageHeaderActions({
  actions,
  assistantEnabled,
  isAssistantOpen,
  onOpenAssistant,
  assistantTriggerRef,
}: SkillPageHeaderActionsProps) {
  return (
    <>
      {actions.primaryAction && (
        <Button
          className={HEADER_CONTROL_CLASS}
          onClick={actions.primaryAction.run}
          disabled={actions.primaryAction.busy}
        >
          {actions.primaryAction.busy ? "Working…" : actions.primaryAction.label}
        </Button>
      )}
      {actions.parkAction && (
        <SecondaryActionButton
          action={actions.parkAction}
          icon={
            actions.parkAction.label === "Unpark" ? (
              <CirclePlay size={16} aria-hidden />
            ) : (
              <CirclePause size={16} aria-hidden />
            )
          }
        />
      )}
      {actions.forkAction && (
        <SecondaryActionButton
          action={actions.forkAction}
          icon={<GitFork size={16} aria-hidden />}
        />
      )}
      {assistantEnabled && (
        <Button
          ref={assistantTriggerRef}
          variant="outline"
          className={`${SECONDARY_BUTTON_CLASS} aria-expanded:border-border-focus aria-expanded:text-accent`}
          onClick={onOpenAssistant}
          aria-expanded={isAssistantOpen}
          aria-controls={isAssistantOpen ? SKILL_ASSISTANT_DRAWER_ID : undefined}
          aria-label="Assistant"
        >
          <PanelRight size={16} />
          <span>Assistant</span>
        </Button>
      )}
      <MenuControl
        triggerClassName={`${HEADER_CONTROL_CLASS} flex h-(--control-height) w-(--control-height) cursor-pointer items-center justify-center rounded-sm border border-border text-text-secondary hover:bg-bg-tertiary hover:text-text-primary`}
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
        {actions.removeActions.length > 0 && <MenuSeparator />}
        {actions.removeActions.map((removeAction) => (
          <MenuItem
            key={removeAction.key}
            closeOnClick
            variant="destructive"
            onClick={removeAction.run}
            disabled={removeAction.busy}
          >
            {removeAction.label}
          </MenuItem>
        ))}
        {actions.removeBlockedReason && (
          <>
            <MenuSeparator />
            <MenuItem disabled className="h-auto flex-col items-start gap-0.5 py-1.5">
              <span>Remove</span>
              <span className="max-w-56 text-caption text-text-tertiary">
                {actions.removeBlockedReason}
              </span>
            </MenuItem>
          </>
        )}
      </MenuControl>
    </>
  );
}
