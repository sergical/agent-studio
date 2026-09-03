// ============================================================================
// SkillLocationRow - one harness/reader row inside a scope's drawer: an
// identity icon carrying the row's one status dot, its name and path, a
// facts-only chip, a switch where the row has one of its own, and the ⋯
// menu. Status never lives in the name or the chip - see status-spec.md §1.
// ============================================================================

import { Link2, Puzzle } from "lucide-react";
import { HarnessIcon } from "../ui/HarnessIcon";
import { StatusIcon } from "../ui/StatusIcon";
import { SwitchControl } from "../ui/SwitchControl";
import { TooltipControl } from "../ui/TooltipControl";
import { homeRelativePath } from "@skill-studio/lib";
import { SkillLocationMenu } from "./SkillLocationMenu";
import { rowMenu, tipLines } from "./skill-location-status";
import type { LocationAction, LocationRow } from "./skill-location-status";

export function SkillLocationRow({
  row,
  scopeLabel,
  onAction,
}: {
  row: LocationRow;
  scopeLabel: string;
  onAction: (action: LocationAction) => void;
}) {
  const menu = rowMenu(row, scopeLabel);
  const tip = tipLines(row.conditions);
  const labelTip =
    row.kind === "link" && row.deployment?.symlink_target
      ? [
          { text: homeRelativePath(row.path), mono: true as const },
          { text: `→ ${homeRelativePath(row.deployment.symlink_target)}`, mono: true as const },
        ]
      : [{ text: homeRelativePath(row.path), mono: true as const }];

  return (
    <div className="grid h-9 grid-cols-[20px_minmax(0,1fr)_auto] items-center gap-3 rounded-sm px-2 hover:bg-bg-hover">
      <span aria-hidden="true" />
      <span className="grid min-w-0 grid-cols-[16px_12.5rem_minmax(0,1fr)] items-center gap-2">
        <StatusIcon
          icon={<HarnessIcon harness={row.harness} size={16} />}
          level={row.level ?? undefined}
          tip={tip}
        />
        <TooltipControl content={labelTip}>
          <span className="w-fit max-w-full truncate text-left text-body text-text-primary">
            {row.harnessLabel}
            {row.kind === "link" && (
              <Link2
                size={12}
                className="ml-1 inline-block align-[-1px] text-text-tertiary"
                aria-label="Symlink"
              />
            )}
            {row.kind === "plugin" && (
              <Puzzle
                size={12}
                className="ml-1 inline-block align-[-1px] text-text-tertiary"
                aria-label="Plugin"
              />
            )}
          </span>
        </TooltipControl>
        <span className="truncate text-caption text-text-tertiary">{row.caption}</span>
      </span>
      <span className="flex shrink-0 items-center gap-1">
        {row.chip && (
          <span className="mr-1 rounded-full bg-bg-tertiary px-1.5 py-0.5 text-caption text-text-tertiary">
            {row.chip}
          </span>
        )}
        {row.hasSwitch ? (
          <SwitchControl
            checked={row.switchOn}
            onCheckedChange={(next) =>
              onAction(
                row.kind === "reader"
                  ? { kind: "set-reader-enabled", agent: row.harness, enabled: next }
                  : { kind: "set-enabled", deployment: row.deployment!, enabled: next },
              )
            }
            ariaLabel={`Enabled for ${row.harnessLabel}`}
          />
        ) : (
          <span className="w-6" aria-hidden="true" />
        )}
        <SkillLocationMenu
          entries={menu.entries}
          danger={menu.danger}
          hint={menu.hint}
          onAction={onAction}
          ariaLabel={row.harnessLabel}
        />
      </span>
    </div>
  );
}
