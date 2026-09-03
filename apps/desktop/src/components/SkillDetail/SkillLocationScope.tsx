// ============================================================================
// SkillLocationScope - one scope block on the Locations card: an accordion
// for the shared `.agents/skills` folder plus the readers that have no
// filesystem entry of their own (Codex, OpenCode, pi, Cursor, Grok Build),
// followed by every harness with its own entry - a link, a copy, or a
// plugin - as a flat sibling row at the accordion's own indent. A scope with
// no shared folder has no accordion at all, only flat rows.
// ============================================================================

import { useState } from "react";
import { ChevronRight, Folder } from "lucide-react";
import { Collapsible, CollapsiblePanel, CollapsibleTrigger } from "@skill-studio/ui";
import { HarnessIcon } from "../ui/HarnessIcon";
import { StatusIcon } from "../ui/StatusIcon";
import { SwitchControl } from "../ui/SwitchControl";
import { TooltipControl } from "../ui/TooltipControl";
import { homeRelativePath } from "@skill-studio/lib";
import { SkillLocationMenu } from "./SkillLocationMenu";
import { SkillLocationRow } from "./SkillLocationRow";
import {
  folderReaders,
  rowMenu,
  siblingRows,
  tipLines,
  toTooltipLines,
} from "./skill-location-status";
import type { LocationAction, LocationRow, ScopeGroup } from "./skill-location-status";

export function SkillLocationScope({
  group,
  showEyebrow,
  onAction,
}: {
  group: ScopeGroup;
  showEyebrow: boolean;
  onAction: (action: LocationAction) => void;
}) {
  const [isOpen, setIsOpen] = useState(false);
  const { shared } = group;
  const readers = folderReaders(group);
  const siblings = siblingRows(group);
  const renderRow = (row: LocationRow) => (
    <SkillLocationRow
      key={`${row.kind}-${row.harness}-${row.path}`}
      row={row}
      scopeLabel={group.label}
      onAction={onAction}
    />
  );

  if (!shared) {
    return <div className="flex flex-col">{siblings.map(renderRow)}</div>;
  }

  const menu = rowMenu(shared, group.label, group.projectPath ?? null);
  const label = showEyebrow ? (group.isGlobal ? "Global folder" : "Project folder") : group.label;

  const labelPath = homeRelativePath(shared.path);
  const rowDelay = (i: number) => (isOpen ? { animationDelay: `${120 + i * 30}ms` } : undefined);
  const rowClass = isOpen
    ? "animate-[locationRowIn_160ms_ease-out_both] motion-reduce:animate-none"
    : "";
  const renderReaderRow = (row: LocationRow, i: number) => (
    <div key={`${row.kind}-${row.harness}-${row.path}`} className={rowClass} style={rowDelay(i)}>
      <SkillLocationRow row={row} scopeLabel={group.label} onAction={onAction} />
    </div>
  );

  return (
    <Collapsible open={isOpen} onOpenChange={setIsOpen} className="flex flex-col">
      <div className="grid h-9 grid-cols-[minmax(0,1fr)_auto] items-center gap-3 rounded-sm px-2 hover:bg-bg-hover">
        <CollapsibleTrigger className="grid h-full min-w-0 cursor-pointer grid-cols-[20px_minmax(0,1fr)_auto] items-center gap-3 border-0 bg-transparent p-0 text-left">
          <span className="flex size-5 items-center justify-center text-text-tertiary">
            <ChevronRight
              size={14}
              className={`transition-transform duration-150 ${isOpen ? "rotate-90" : ""}`}
            />
          </span>
          <span className="grid min-w-0 grid-cols-[16px_12.5rem_minmax(0,1fr)] items-center gap-2">
            <StatusIcon
              icon={<Folder size={16} />}
              level={group.folderLevel ?? undefined}
              tip={group.folderTip ? toTooltipLines(group.folderTip) : undefined}
            />
            <TooltipControl content={[{ text: labelPath, mono: true }]}>
              <span className="w-fit max-w-full truncate text-left text-body text-text-primary">
                {label}
              </span>
            </TooltipControl>
            <span className="truncate text-caption text-text-tertiary" />
          </span>
          <div className="group/stack relative shrink-0" style={{ width: readers.length * 20 }}>
            <div
              className={`relative h-[18px] transition-[opacity,transform] duration-150 ease-in motion-reduce:transition-none ${
                isOpen
                  ? "pointer-events-none -translate-y-1 opacity-0"
                  : "translate-y-0 opacity-100"
              }`}
            >
              {readers.map((row, i) => (
                <div
                  key={`${row.kind}-${row.harness}-${row.path}`}
                  // SAFETY: `--stack-i` is a CSS custom property, not one of `CSSProperties`'
                  // known keys - React passes it straight through to the inline style attribute.
                  style={{ "--stack-i": i } as React.CSSProperties}
                  className="absolute inset-y-0 left-0 inline-flex size-[18px] translate-x-[calc(var(--stack-i)*14px)] items-center justify-center rounded-full bg-bg-tertiary ring-2 ring-bg-primary transition-transform duration-150 ease-out group-hover/stack:translate-x-[calc(var(--stack-i)*20px)] motion-reduce:transition-none"
                >
                  <StatusIcon
                    icon={<HarnessIcon harness={row.harness} size={12} />}
                    level={row.level ?? undefined}
                    tip={[
                      `${row.harnessLabel} · ${row.conditions[0]?.status ?? "reads the folder"}`,
                      ...tipLines(row.conditions),
                    ]}
                    size={12}
                  />
                </div>
              ))}
            </div>
          </div>
        </CollapsibleTrigger>
        <span className="flex shrink-0 items-center gap-1">
          <SwitchControl
            checked={shared.switchOn}
            onCheckedChange={(next) =>
              onAction({ kind: "set-enabled", deployment: shared.deployment!, enabled: next })
            }
            ariaLabel="Enabled everywhere"
          />
          <SkillLocationMenu
            entries={menu.entries}
            danger={menu.danger}
            hint={menu.hint}
            onAction={onAction}
            ariaLabel={group.label}
          />
        </span>
      </div>
      <CollapsiblePanel>
        {readers.length > 0 && (
          <div className="ml-[17px] border-l border-border-subtle pl-3">
            {readers.map(renderReaderRow)}
          </div>
        )}
      </CollapsiblePanel>
      {siblings.map(renderRow)}
    </Collapsible>
  );
}
