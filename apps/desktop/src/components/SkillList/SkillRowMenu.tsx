// ============================================================================
// SkillRowMenu - the row's one menu: fixes first when the row has a decision
// to make, then the open/park verbs every row offers.
// ============================================================================

import type { ReactNode } from "react";
import type { InstalledSkill } from "@skill-studio/lib";
import { MenuControl, MenuItem, MenuSeparator } from "../ui/MenuControl";
import { fixesFor } from "./skill-row-state";
import type { RowState } from "./skill-row-state";

interface SkillRowMenuProps {
  skill: InstalledSkill;
  state: RowState | null;
  trigger: ReactNode;
  triggerClassName?: string;
  triggerAriaLabel?: string;
  onOpen: () => void;
  onAct: (label: string) => void;
}

export function SkillRowMenu({
  skill,
  state,
  trigger,
  triggerClassName,
  triggerAriaLabel,
  onOpen,
  onAct,
}: SkillRowMenuProps) {
  const fixes = state ? fixesFor(state) : [];
  return (
    <span onClick={(e) => e.stopPropagation()}>
      <MenuControl
        trigger={trigger}
        triggerClassName={triggerClassName}
        triggerAriaLabel={triggerAriaLabel}
        popupClassName="min-w-[220px]"
      >
        <div className="px-2 py-1.5 text-small text-text-primary">
          {state ? state.label : skill.name}
        </div>
        {state?.detail && (
          <div className="px-2 pb-1.5 text-caption text-text-tertiary">{state.detail}</div>
        )}
        <MenuSeparator />
        {fixes.length > 0 && (
          <>
            {fixes.map((fix) => (
              <MenuItem key={fix} onClick={() => onAct(fix)}>
                {fix}
              </MenuItem>
            ))}
            <MenuSeparator />
          </>
        )}
        <MenuItem onClick={onOpen}>Open skill</MenuItem>
        <MenuItem onClick={() => onAct(skill.parked ? "Unpark" : "Park")}>
          {skill.parked ? "Unpark" : "Park"}
        </MenuItem>
      </MenuControl>
    </span>
  );
}
