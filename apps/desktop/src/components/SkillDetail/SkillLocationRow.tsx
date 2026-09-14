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

/** The label tooltip: just the row's path, or the path plus its symlink target for a link row. */
function labelTipFor(row: LocationRow) {
  if (row.kind === "link" && row.deployment?.symlink_target) {
    return [
      { text: homeRelativePath(row.path), mono: true as const },
      { text: `→ ${homeRelativePath(row.deployment.symlink_target)}`, mono: true as const },
    ];
  }
  return [{ text: homeRelativePath(row.path), mono: true as const }];
}

/**
 * The switch slot: a live switch where the row has one of its own, a
 * disabled-but-explained switch for an always-on reader or a
 * plugin-disabled-by-Claude row, or an empty placeholder to keep columns
 * aligned.
 */
function LocationRowSwitch({
  row,
  onAction,
}: {
  row: LocationRow;
  onAction: (action: LocationAction) => void;
}) {
  if (row.hasSwitch) {
    return (
      <SwitchControl
        checked={row.switchOn}
        onCheckedChange={(next) =>
          onAction(
            row.kind === "reader"
              ? {
                  kind: "set-reader-enabled",
                  target: row.lifecycleTarget,
                  agent: row.harness,
                  enabled: next,
                }
              : { kind: "set-enabled", deployment: row.deployment!, enabled: next },
          )
        }
        ariaLabel={`Enabled for ${row.harnessLabel}`}
      />
    );
  }

  const isAlwaysOnReader = row.kind === "reader";
  if (isAlwaysOnReader) {
    return (
      <TooltipControl
        content={
          row.switchOn
            ? `Always on because ${row.harnessLabel} has no per-skill switch.`
            : `Off because this skill is disabled in the Universal folder.`
        }
      >
        <span className="inline-flex">
          <SwitchControl
            checked={row.switchOn}
            disabled
            onCheckedChange={() => undefined}
            ariaLabel={
              row.switchOn
                ? `Always enabled for ${row.harnessLabel}`
                : `Disabled for ${row.harnessLabel} while this skill is off`
            }
          />
        </span>
      </TooltipControl>
    );
  }

  const pluginDisabledByClaudeLabel =
    row.kind === "plugin" && row.deployment?.disabled_by === "claude-plugin-disabled"
      ? `Off because the ${row.deployment.plugin?.name ?? "plugin"} plugin is disabled in Claude Code.`
      : null;
  if (pluginDisabledByClaudeLabel) {
    return (
      <TooltipControl content={pluginDisabledByClaudeLabel}>
        <span className="inline-flex">
          <SwitchControl
            checked={false}
            disabled
            onCheckedChange={() => undefined}
            ariaLabel={pluginDisabledByClaudeLabel}
          />
        </span>
      </TooltipControl>
    );
  }

  return <span className="w-6" aria-hidden="true" />;
}

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
  const labelTip = labelTipFor(row);

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
        <LocationRowSwitch row={row} onAction={onAction} />
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
