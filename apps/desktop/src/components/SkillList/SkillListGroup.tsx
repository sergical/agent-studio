// ============================================================================
// SkillListGroup - One state group in SkillListTable: a sticky header plus
// the virtualized rows that belong to it, sized and positioned in the
// virtualizer's flat coordinate space.
// ============================================================================

import type { ReactNode } from "react";
import type { VirtualItem } from "@tanstack/react-virtual";
import { Collapsible } from "@skill-studio/ui";
import { GroupHead } from "./GroupHead";
import type { RowGroup } from "./skill-row-state";
import type { ListItem } from "./SkillListTable";

interface SkillListGroupProps {
  group: RowGroup;
  label: string;
  count: number;
  open: boolean;
  /** This group's offset and total size in the flat `ListItem[]` coordinate space. */
  start: number;
  height: number;
  items: ListItem[];
  virtualItems: VirtualItem[];
  scrollMargin: number;
  onToggle: () => void;
  renderRow: (item: Extract<ListItem, { kind: "row" }>, top: number) => ReactNode;
}

/** A group's header plus its rows: push-style sticky headers stay scoped to this group, since
 * only the virtual row items that belong to it render here, absolutely positioned within it, so
 * the next group's header pushes this one up exactly as it did unvirtualized. */
export function SkillListGroup({
  group,
  label,
  count,
  open,
  start,
  height,
  items,
  virtualItems,
  scrollMargin,
  onToggle,
  renderRow,
}: SkillListGroupProps) {
  const groupRowItems = open
    ? virtualItems.filter((virtualItem) => {
        const item = items[virtualItem.index];
        return item.kind === "row" && item.group === group;
      })
    : [];
  return (
    <Collapsible
      role="rowgroup"
      data-group={group}
      open={open}
      onOpenChange={onToggle}
      style={{ position: "relative", height }}
    >
      {/* Sticky here, not only inside `GroupHead`: a sticky element sticks within its
          parent, and this row is the header's parent. The group container is the
          next header's parent, so that header pushes this one up. */}
      <div role="row" className="sticky top-0 z-2">
        <div role="gridcell">
          <GroupHead label={label} count={count} groupId={group} />
        </div>
      </div>
      {groupRowItems.map((virtualItem) => {
        const item = items[virtualItem.index];
        // SAFETY: filtered to `kind === "row"` above.
        if (item.kind !== "row") return null;
        return renderRow(item, virtualItem.start - scrollMargin - start);
      })}
    </Collapsible>
  );
}
