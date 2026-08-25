// ============================================================================
// SkillRunDiff - Read-only whole-tree diff for a Worktree/InPlace "Test" run,
// with the Apply/Discard (Worktree) or Keep/Revert (InPlace) footer
// ============================================================================

import { PatchDiff } from "@pierre/diffs/react";
import type { SkillRunTargetKind } from "../../lib/skill-run-target-types";

interface SkillRunDiffProps {
  projectLabel: string;
  targetKind: Extract<SkillRunTargetKind, "worktree" | "in_place">;
  diff: string;
  isBusy: boolean;
  onPrimary: () => void;
  onSecondary: () => void;
}

/** App CSS (src/App.css) is dark by default and light only under `:root[data-theme="light"]`. */
function currentTheme(): "github-light" | "github-dark" {
  return document.documentElement.dataset.theme === "light" ? "github-light" : "github-dark";
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
  const theme = currentTheme();
  const primaryLabel = targetKind === "worktree" ? "Apply to project" : "Keep";
  const secondaryLabel = targetKind === "worktree" ? "Discard" : "Revert";

  return (
    <div className="skill-run-diff">
      <div className="skill-proposed-header">
        <span className="skill-proposed-label">Changes in {projectLabel}</span>
      </div>
      {diff.trim().length === 0 ? (
        <p className="skill-assistant-panel-note">No changes.</p>
      ) : (
        <div className="skill-proposed-hunk-body">
          <PatchDiff patch={diff} options={{ theme }} />
        </div>
      )}
      <div className="skill-proposed-footer">
        <button
          type="button"
          className="skill-action-button primary"
          onClick={onPrimary}
          disabled={isBusy}
        >
          {primaryLabel}
        </button>
        <button
          type="button"
          className="skill-action-button"
          onClick={onSecondary}
          disabled={isBusy}
        >
          {secondaryLabel}
        </button>
      </div>
    </div>
  );
}
