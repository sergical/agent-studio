// ============================================================================
// SkillRunDiff - Read-only whole-tree diff for a Worktree/InPlace "Test" run,
// with the Apply/Discard (Worktree) or Keep/Revert (InPlace) footer
// ============================================================================

import { PatchDiff } from "@pierre/diffs/react";
import { Button } from "@skill-studio/ui";
import type { SkillRunTargetKind } from "../../lib/skill-run-target-types";
import { diffTheme } from "../../lib/theme";
import { useAppStore } from "../../store/appStore";

interface SkillRunDiffProps {
  projectLabel: string;
  targetKind: Extract<SkillRunTargetKind, "worktree" | "in_place">;
  diff: string;
  isBusy: boolean;
  onPrimary: () => void;
  onSecondary: () => void;
}

/**
 * Renders `diff` (a full `git diff` unified patch, potentially many files) as
 * one read-only `PatchDiff`, plus a footer whose two buttons depend on the
 * target kind: Worktree gets "Apply to project" / "Discard", InPlace gets
 * "Keep" / "Revert".
 */
export function SkillRunDiff({
  projectLabel,
  targetKind,
  diff,
  isBusy,
  onPrimary,
  onSecondary,
}: SkillRunDiffProps) {
  const resolvedTheme = useAppStore((state) => state.resolvedTheme);
  const theme = diffTheme(resolvedTheme);
  const primaryLabel = targetKind === "worktree" ? "Apply to project" : "Keep";
  const secondaryLabel = targetKind === "worktree" ? "Discard" : "Revert";

  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex items-baseline justify-between">
        <span className="text-caption font-semibold tracking-[0.04em] text-text-tertiary uppercase">
          Changes in {projectLabel}
        </span>
      </div>
      {diff.trim().length === 0 ? (
        <p className="m-0 text-caption text-text-tertiary">No changes.</p>
      ) : (
        <div className="skill-proposed-hunk-body">
          <PatchDiff patch={diff} options={{ theme }} />
        </div>
      )}
      <div className="flex gap-2">
        <Button size="sm" onClick={onPrimary} disabled={isBusy}>
          {primaryLabel}
        </Button>
        <Button variant="outline" size="sm" onClick={onSecondary} disabled={isBusy}>
          {secondaryLabel}
        </Button>
      </div>
    </div>
  );
}
