// ============================================================================
// SkillCompareDialog - Side-by-side diff between two of a skill's own copies,
// opened from the "Compare" action on a duplicate-content issue or the
// Locations card's "Compare copies" button.
// ============================================================================

import { useEffect, useState } from "react";
import { PatchDiff } from "@pierre/diffs/react";
import { Dialog, DialogContent, DialogTitle } from "@skill-studio/ui";
import { readInstalledSkillMd } from "../../lib/skill-api";
import { deploymentLabel } from "@skill-studio/lib";
import { unifiedSkillMdDiff } from "@skill-studio/lib";
import { ownDeployments, skillMdPathForDeployment } from "@skill-studio/lib";
import { pickCompareDefaults, resolveCompareSelection } from "@skill-studio/lib";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import { diffTheme } from "../../lib/theme";
import { useAppStore } from "../../store/appStore";
import { SelectControl } from "../ui/SelectControl";

interface SkillCompareDialogProps {
  skill: InstalledSkill;
  onClose: () => void;
}

/**
 * One side's loaded state: not yet read, read ok, failed (keeping the path
 * that failed, not just the message), or `missing` - the deployment the side
 * had selected is no longer in the candidate list, e.g. a background rescan
 * dropped or renamed it. `missing` renders an explicit message rather than
 * leaving the previous side's content on screen.
 */
type SideState =
  | { status: "loading" }
  | { status: "loaded"; content: string }
  | { status: "error"; message: string; path: string }
  | { status: "missing" };

function useSideContent(deployment: Deployment | undefined): SideState {
  // Keyed on the path string, not `deployment` itself: deployments are
  // rebuilt on every snapshot, so keying on the object would re-read the file
  // on every background rescan even when the selected copy hasn't changed.
  const path = deployment ? skillMdPathForDeployment(deployment) : undefined;
  const [state, setState] = useState<SideState>(
    path ? { status: "loading" } : { status: "missing" },
  );

  useEffect(() => {
    if (!path) {
      setState({ status: "missing" });
      return;
    }
    let ignore = false;
    setState({ status: "loading" });
    readInstalledSkillMd(path)
      .then((content) => {
        if (!ignore) setState({ status: "loaded", content });
      })
      .catch((err) => {
        if (!ignore) {
          setState({
            status: "error",
            message: err instanceof Error ? err.message : "Unknown error",
            path,
          });
        }
      });
    return () => {
      ignore = true;
    };
  }, [path]);

  return state;
}

/**
 * Two `SelectControl`s pick the deployments to compare; the body renders
 * their SKILL.md as a read-only diff (or "These copies are the same." when
 * the content is identical), or the read error and path when either side
 * fails to load.
 */
export function SkillCompareDialog({ skill, onClose }: SkillCompareDialogProps) {
  const candidates = ownDeployments(skill).filter((d) => d.content_hash);
  const defaults = pickCompareDefaults(candidates);
  const [leftPath, setLeftPath] = useState(defaults.left?.path);
  const [rightPath, setRightPath] = useState(defaults.right?.path ?? defaults.left?.path);

  // Re-picked every render, not in an effect: a selection the user made and
  // that's still a candidate must never be overridden, but a background
  // rescan can drop the selected deployment out from under `leftPath`/
  // `rightPath`, in which case falling back to the current default is
  // better than pointing at a copy that no longer exists.
  const effectiveLeftPath = resolveCompareSelection(leftPath, candidates, defaults.left?.path);
  const effectiveRightPath = resolveCompareSelection(
    rightPath,
    candidates,
    defaults.right?.path ?? defaults.left?.path,
  );

  const left = candidates.find((d) => d.path === effectiveLeftPath);
  const right = candidates.find((d) => d.path === effectiveRightPath);
  const leftState = useSideContent(left);
  const rightState = useSideContent(right);

  const items = candidates.map((d) => ({ value: d.path, label: deploymentLabel(d) }));

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        className="flex max-h-[85vh] w-[min(1200px,92vw)] max-w-none flex-col gap-0 rounded-lg bg-bg-elevated p-0 text-body text-text-primary shadow-lg sm:max-w-none"
        aria-label={`Compare copies of ${skill.name}`}
      >
        <div className="flex items-center justify-between border-b border-border px-5 py-4">
          <DialogTitle className="m-0 text-emphasis font-semibold text-text-primary">
            Compare copies of {skill.name}
          </DialogTitle>
        </div>

        <div className="flex gap-3 border-b border-border-subtle px-5 py-3">
          <div className="flex-1">
            <SelectControl
              ariaLabel="Left copy"
              value={effectiveLeftPath ?? ""}
              onValueChange={setLeftPath}
              items={items}
            />
          </div>
          <div className="flex-1">
            <SelectControl
              ariaLabel="Right copy"
              value={effectiveRightPath ?? ""}
              onValueChange={setRightPath}
              items={items}
            />
          </div>
        </div>

        <div className="skill-compare-dialog-body flex-1 overflow-auto px-5 py-4">
          <SkillCompareBody leftState={leftState} rightState={rightState} />
        </div>
      </DialogContent>
    </Dialog>
  );
}

function SkillCompareBody({
  leftState,
  rightState,
}: {
  leftState: SideState;
  rightState: SideState;
}) {
  const resolvedTheme = useAppStore((state) => state.resolvedTheme);
  if (leftState.status === "error") {
    return (
      <p className="m-0 text-body text-error">
        Couldn't read {leftState.path}: {leftState.message}
      </p>
    );
  }
  if (rightState.status === "error") {
    return (
      <p className="m-0 text-body text-error">
        Couldn't read {rightState.path}: {rightState.message}
      </p>
    );
  }
  if (leftState.status === "loading" || rightState.status === "loading") {
    return <p className="m-0 text-caption text-text-tertiary">Loading…</p>;
  }
  if (leftState.status === "missing" || rightState.status === "missing") {
    return <p className="m-0 text-body text-error">This copy is no longer on disk.</p>;
  }
  if (leftState.content === rightState.content) {
    return <p className="m-0 text-body text-text-tertiary">These copies are the same.</p>;
  }
  const diff = unifiedSkillMdDiff(leftState.content, rightState.content);
  return (
    <PatchDiff
      patch={diff}
      options={{ theme: diffTheme(resolvedTheme), disableFileHeader: true, overflow: "wrap" }}
    />
  );
}
