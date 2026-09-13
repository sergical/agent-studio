// ============================================================================
// SkillPageHeaderActions - The header bar's action cluster: the one primary
// action, the assistant toggle, and the ⋯ overflow menu. Rendered as
// `PageShell`'s `actions` - pulled out of `InstalledSkillHeader`, which now
// shows identity only.
// ============================================================================

import type { RefObject } from "react";
import { MoreHorizontal, PanelRight } from "lucide-react";
import { Button } from "@skill-studio/ui";
import { MenuControl, MenuItem, MenuSeparator } from "../ui/MenuControl";
import { SKILL_ASSISTANT_DRAWER_ID } from "./SkillAssistantDrawer";
import type { SkillPageActions } from "./skill-page-actions";

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
        <Button onClick={actions.primaryAction.run} disabled={actions.primaryAction.busy}>
          {actions.primaryAction.busy ? "Working…" : actions.primaryAction.label}
        </Button>
      )}
      {assistantEnabled && (
        <Button
          ref={assistantTriggerRef}
          variant="outline"
          className="h-(--control-height) gap-1.5 rounded-sm px-3 text-body aria-expanded:border-border-focus aria-expanded:text-accent"
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
        <MenuItem closeOnClick onClick={actions.parkAction.run} disabled={actions.parkAction.busy}>
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
    </>
  );
}
