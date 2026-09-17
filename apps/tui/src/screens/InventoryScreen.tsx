// ============================================================================
// Skill Studio TUI - Inventory screen
// Skills grouped by harness, showing scope, provenance, and state. Movement
// and `enter` are handled by the built-in `<select>`'s own key bindings
// (arrows, `j`/`k`, `return`); this screen only reacts to `onSelect`.
// ============================================================================

import type { SelectOption } from "@opentui/core";
import { useMemo } from "react";

import type { Inventory } from "../cli-types.ts";
import { buildSkillRows } from "../inventory-rows.ts";

export interface InventoryScreenProps {
  inventory: Inventory;
  onOpenSkill: (skillName: string) => void;
}

export function InventoryScreen({ inventory, onOpenSkill }: InventoryScreenProps) {
  const rows = useMemo(() => buildSkillRows(inventory), [inventory]);
  const options: SelectOption[] = rows.map((row) => ({
    name: `[${row.groupLabel}] ${row.skill.name}`,
    description: row.stateSummary,
    value: row.skill.name,
  }));

  if (options.length === 0) {
    return <text>No skills installed.</text>;
  }

  return (
    <select
      focused
      options={options}
      style={{ flexGrow: 1 }}
      onSelect={(_index, option) => {
        if (option === null) return;
        // SAFETY: `options` above always sets `value` to `row.skill.name`,
        // a string we constructed; `SelectOption.value` is typed `any` by
        // the library, not narrowed from external input.
        onOpenSkill(option.value as string);
      }}
    />
  );
}
