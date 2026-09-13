// ============================================================================
// SkillRowMenu - the row's one menu: fixes first when the row has a decision
// to make, then the open/park verbs every row offers. `SkillRowMenuScope`
// shares one menu root across a whole table: every `SkillRowMenu` inside it
// (a row has two - the state glyph and the trailing Ellipsis) renders only a
// detached trigger keyed to a handle, so the table mounts one menu instance
// instead of two per row.
// ============================================================================

import { createContext, useContext, useMemo, useRef } from "react";
import type { ReactNode } from "react";
import {
  cn,
  createDropdownMenuHandle,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
  Kbd,
} from "@skill-studio/ui";
import type { DropdownMenuHandle } from "@skill-studio/ui";
import type { InstalledSkill } from "@skill-studio/lib";
import { DEFAULT_POPUP_CLASS, MenuControl, MenuItem, MenuSeparator } from "../ui/MenuControl";
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
  onOpenChange?: (open: boolean) => void;
  /** Only the trailing ⋯ menu (the row's own menu) offers this - toggles the row's checkbox. */
  onToggleSelect?: () => void;
}

/** The active trigger's data, carried through the handle so the scope's one popup can build the
 * right items without knowing about any particular row. */
interface SkillRowMenuPayload {
  skill: InstalledSkill;
  state: RowState | null;
  onOpen: () => void;
  onAct: (label: string) => void;
  onOpenChange?: (open: boolean) => void;
  onToggleSelect?: () => void;
}

/** The handle a `SkillRowMenuScope` shares with every `SkillRowMenu` inside it. `null` outside a
 * scope, so `SkillRowMenu` falls back to mounting its own root. */
const SkillRowMenuHandleContext = createContext<DropdownMenuHandle<SkillRowMenuPayload> | null>(
  null,
);

/** The item list shared by both the scoped and unscoped menu: title, detail, fixes, then the
 * open/select/park verbs every row offers. */
function SkillRowMenuItems({
  skill,
  state,
  onOpen,
  onAct,
  onToggleSelect,
}: Pick<SkillRowMenuPayload, "skill" | "state" | "onOpen" | "onAct" | "onToggleSelect">) {
  const fixes = state ? fixesFor(state) : [];
  return (
    <>
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
      <MenuItem onClick={onOpen} className="justify-between">
        Open skill
        <Kbd>↵</Kbd>
      </MenuItem>
      {onToggleSelect && (
        <MenuItem onClick={onToggleSelect} className="justify-between">
          Select
          <Kbd>X</Kbd>
        </MenuItem>
      )}
      <MenuItem onClick={() => onAct(skill.parked ? "Unpark" : "Park")}>
        {skill.parked ? "Unpark" : "Park"}
      </MenuItem>
    </>
  );
}

/** Wraps a table of rows in one shared menu root, keyed by a handle. Every `SkillRowMenu` inside
 * renders only a detached trigger; this owns the one popup, built from whichever trigger's
 * payload is active. The active payload is remembered (not cleared to `null`) once the popup
 * starts closing, so the close notification below still reaches the row that opened it - that's
 * how the row's `.`/Shift+F10 shortcut refocuses the right row once its menu closes. */
export function SkillRowMenuScope({ children }: { children: ReactNode }) {
  // Created once, not stored in state: the handle never changes, so there's no setter to pair it
  // with.
  const handle = useMemo(() => createDropdownMenuHandle<SkillRowMenuPayload>(), []);
  const activePayloadRef = useRef<SkillRowMenuPayload | null>(null);
  return (
    <SkillRowMenuHandleContext.Provider value={handle}>
      {children}
      <DropdownMenu
        handle={handle}
        onOpenChange={(open) => activePayloadRef.current?.onOpenChange?.(open)}
      >
        {({ payload }) => {
          if (!payload) return null;
          activePayloadRef.current = payload;
          return (
            <DropdownMenuContent
              align="start"
              sideOffset={4}
              className={cn(DEFAULT_POPUP_CLASS, "min-w-[220px]")}
            >
              <SkillRowMenuItems
                skill={payload.skill}
                state={payload.state}
                onOpen={payload.onOpen}
                onAct={payload.onAct}
                onToggleSelect={payload.onToggleSelect}
              />
            </DropdownMenuContent>
          );
        }}
      </DropdownMenu>
    </SkillRowMenuHandleContext.Provider>
  );
}

export function SkillRowMenu({
  skill,
  state,
  trigger,
  triggerClassName,
  triggerAriaLabel,
  onOpen,
  onAct,
  onOpenChange,
  onToggleSelect,
}: SkillRowMenuProps) {
  const handle = useContext(SkillRowMenuHandleContext);
  if (handle) {
    const payload: SkillRowMenuPayload = {
      skill,
      state,
      onOpen,
      onAct,
      onOpenChange,
      onToggleSelect,
    };
    return (
      <span onClick={(e) => e.stopPropagation()}>
        <DropdownMenuTrigger
          handle={handle}
          payload={payload}
          className={triggerClassName}
          aria-label={triggerAriaLabel}
        >
          {trigger}
        </DropdownMenuTrigger>
      </span>
    );
  }
  return (
    <span onClick={(e) => e.stopPropagation()}>
      <MenuControl
        trigger={trigger}
        triggerClassName={triggerClassName}
        triggerAriaLabel={triggerAriaLabel}
        popupClassName="min-w-[220px]"
        onOpenChange={onOpenChange}
      >
        <SkillRowMenuItems
          skill={skill}
          state={state}
          onOpen={onOpen}
          onAct={onAct}
          onToggleSelect={onToggleSelect}
        />
      </MenuControl>
    </span>
  );
}
