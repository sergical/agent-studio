// ============================================================================
// SkillInvocationFooter - the Locations card's Invocation section, the skill
// page's only invocation control. One file: one segmented Both/User only/Model
// only control. Two or more files: an "All locations" control first, which
// sets every editable file at once, then one control per file, so a file can
// still differ from the rest.
// ============================================================================

import { useRef, useState } from "react";
import { Loader2 } from "lucide-react";
import { ToggleGroup, ToggleGroupItem } from "@skill-studio/ui";
import { SETTLE_MS, clearIfCurrent, useOptimisticAction } from "../../hooks/useOptimisticAction";
import type { OptimisticAction } from "../../hooks/useOptimisticAction";
import { singleSelectToggleValue } from "../../lib/single-select-toggle-group";
import { HarnessIcon } from "../ui/HarnessIcon";
import { StatusIcon } from "../ui/StatusIcon";
import { TooltipControl } from "../ui/TooltipControl";
import { setInvocationForFiles } from "./skill-location-actions";
import {
  INVOCATION_POLICY_OPTIONS,
  invocationFooterNote,
  toTooltipLines,
} from "./skill-location-status";
import type { InvocationFile } from "./skill-location-status";
import type { InstalledSkill, InvocationPolicy } from "@skill-studio/lib";

interface InvocationToggleProps {
  label: string;
  /** Null when the files disagree: no item is pressed. */
  value: InvocationPolicy | null;
  /** Locks every item except the pressed one. */
  locked: boolean;
  lockedReason?: string;
  /** Gets the control's optimistic state so the save can show the new value at once. */
  onSelect: (
    policy: InvocationPolicy,
    optimistic: OptimisticAction<InvocationPolicy | null>,
  ) => void;
}

function InvocationToggle({ label, value, locked, lockedReason, onSelect }: InvocationToggleProps) {
  const optimistic = useOptimisticAction(value);
  const shown = optimistic.value;
  const group = (
    <ToggleGroup
      variant="segmented"
      aria-label={label}
      value={shown ? [shown] : []}
      onValueChange={(next) =>
        singleSelectToggleValue<InvocationPolicy>(next, (policy) => onSelect(policy, optimistic))
      }
    >
      {INVOCATION_POLICY_OPTIONS.map((option) => (
        <ToggleGroupItem
          key={option.value}
          value={option.value}
          className="h-[26px] px-3 text-small"
          disabled={locked && shown !== option.value}
        >
          {option.label}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
  return (
    <span className="flex items-center gap-1.5">
      {/* A fixed slot, so the control does not shift when the spinner appears. */}
      <span className="flex size-3 items-center justify-center" aria-live="polite">
        {optimistic.pending && (
          <Loader2
            size={12}
            className="animate-spin text-text-tertiary motion-reduce:animate-none"
            aria-label="Saving"
          />
        )}
      </span>
      {lockedReason ? <TooltipControl content={lockedReason}>{group}</TooltipControl> : group}
    </span>
  );
}

interface SkillInvocationFooterProps {
  skill: InstalledSkill;
  files: InvocationFile[];
}

export function SkillInvocationFooter({ skill, files }: SkillInvocationFooterProps) {
  // A save in flight drops further clicks rather than disabling the items: a disabled item loses
  // keyboard focus. The pressed value moves at once (`useOptimisticAction`) and settles when the
  // snapshot refresh arrives.
  const isSaving = useRef(false);

  const editableFiles = files.filter((file) => file.editable);
  const lockedFiles = files.filter((file) => !file.editable);
  // "All locations" can only change the editable files, so its pressed value follows those; a
  // locked file that differs gets its own caption instead of keeping the control unpressed.
  const policyFiles = editableFiles.length > 0 ? editableFiles : files;
  const policies = new Set(policyFiles.map((file) => file.invocation));
  const sharedPolicy = policies.size === 1 ? policyFiles[0].invocation : null;
  const allCaption = [
    sharedPolicy === null ? "Files differ" : null,
    editableFiles.length > 0 && lockedFiles.length > 0
      ? `${lockedFiles.length} locked file${lockedFiles.length === 1 ? "" : "s"} not changed`
      : null,
  ]
    .filter(Boolean)
    .join(" · ");

  // An "All locations" save also moves every editable file's control at once, until the snapshot
  // brings the new shared value.
  const [allOverride, setAllOverride] = useState<{
    base: InvocationPolicy | null;
    value: InvocationPolicy;
  } | null>(null);
  if (allOverride !== null && !Object.is(sharedPolicy, allOverride.base)) setAllOverride(null);
  const allPolicy = allOverride?.value ?? null;

  const save = async (
    optimistic: OptimisticAction<InvocationPolicy | null>,
    targets: InvocationFile[],
    policy: InvocationPolicy,
  ) => {
    if (isSaving.current || targets.length === 0) return false;
    isSaving.current = true;
    const saved = await optimistic.run(
      policy,
      () => setInvocationForFiles(skill, targets, policy),
      "Couldn't change invocation policy",
    );
    isSaving.current = false;
    return saved;
  };

  return (
    <div className="mt-3 flex flex-col gap-1.5 border-t border-border-subtle pt-3">
      <span className="text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
        Invocation
      </span>
      {files.length > 1 && (
        <div className="grid min-h-8 grid-cols-[16px_minmax(0,1fr)_auto] items-center gap-2.5 border-b border-border-subtle pb-2">
          <span aria-hidden="true" />
          <span className="flex min-w-0 flex-col">
            <span className="truncate text-body font-medium text-text-primary">All locations</span>
            {allCaption && (
              <span className="truncate text-caption text-text-tertiary">{allCaption}</span>
            )}
          </span>
          <InvocationToggle
            label="Invocation for all locations"
            value={sharedPolicy}
            locked={editableFiles.length === 0}
            lockedReason={editableFiles.length === 0 ? files[0].disabledReason : undefined}
            onSelect={(policy, optimistic) => {
              // A click during a running save is dropped by `save`; it must not touch the first click's override.
              if (isSaving.current) return;
              const mine = { base: sharedPolicy, value: policy };
              setAllOverride(mine);
              void save(optimistic, editableFiles, policy).then((saved) => {
                // A save that leaves the shared value unchanged never ends the override on its own.
                if (saved) setTimeout(() => setAllOverride(clearIfCurrent(mine)), SETTLE_MS);
                else setAllOverride(clearIfCurrent(mine));
              });
            }}
          />
        </div>
      )}
      {files.map((file) => (
        <div
          key={file.path}
          className="grid min-h-8 grid-cols-[16px_minmax(0,1fr)_auto_auto] items-center gap-2.5"
        >
          <StatusIcon
            icon={<HarnessIcon harness={file.harness} size={16} />}
            level={file.level ?? undefined}
            tip={toTooltipLines(file.tip)}
          />
          <span className="flex min-w-0 flex-col">
            <TooltipControl content={[{ text: `${file.path}/SKILL.md`, mono: true }]}>
              <span className="w-fit max-w-full truncate text-body text-text-primary">
                {file.name}
              </span>
            </TooltipControl>
            {file.caption && (
              <span className="truncate text-caption text-text-tertiary">{file.caption}</span>
            )}
          </span>
          {file.chip ? (
            <span className="rounded-full bg-bg-tertiary px-1.5 py-0.5 text-caption text-text-tertiary">
              {file.chip}
            </span>
          ) : (
            <span />
          )}
          <InvocationToggle
            label={`Invocation for ${file.name}`}
            value={file.editable ? (allPolicy ?? file.invocation) : file.invocation}
            locked={!file.editable}
            lockedReason={file.editable ? undefined : file.disabledReason}
            onSelect={(policy, optimistic) => void save(optimistic, [file], policy)}
          />
        </div>
      ))}
      <p className="text-small text-text-tertiary">{invocationFooterNote(files, skill.name)}</p>
    </div>
  );
}
