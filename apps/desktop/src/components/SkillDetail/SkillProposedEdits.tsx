// ============================================================================
// SkillProposedEdits - Renders one Audit run's proposed SKILL.md rewrite as
// per-hunk unified diffs, each independently accept/reject-able, with an
// Apply footer that writes the accepted hunks back through to disk
// ============================================================================

import { useState } from "react";
import { PatchDiff } from "@pierre/diffs/react";
import { Button } from "@skill-studio/ui";
import { writeInstalledSkillMdIfUnchanged } from "../../lib/skill-api";
import type { SkillMdHunk } from "@skill-studio/lib";
import { applyAcceptedHunks } from "@skill-studio/lib";
import { diffTheme } from "../../lib/theme";
import { useAppStore } from "../../store/appStore";

interface SkillProposedEditsProps {
  /** The file the audit ran against, before any of this proposal is applied. */
  fileAtAuditStart: string;
  /** The file's current content, read live from `SkillPage` - may have moved on since the audit started. */
  currentContent: string;
  skillMdPath: string;
  /** The deployment path this proposal was made for - Apply is disabled when it no longer matches `skillMdPath`. */
  proposalSkillMdPath: string;
  hunks: SkillMdHunk[];
  onHunksChange: (hunks: SkillMdHunk[]) => void;
  onApplied: (content: string) => void;
  onDiscard: () => void;
  /** Called when Apply is refused because the file drifted on disk, so the caller re-reads it. */
  onDiskChanged: () => void;
}

/**
 * One card per hunk (header + Accept/Reject + `PatchDiff` body), an Apply/
 * Discard footer, and a warning line when the file on disk has drifted from
 * the copy the audit reviewed.
 */
export function SkillProposedEdits({
  fileAtAuditStart,
  currentContent,
  skillMdPath,
  proposalSkillMdPath,
  hunks,
  onHunksChange,
  onApplied,
  onDiscard,
  onDiskChanged,
}: SkillProposedEditsProps) {
  const addToast = useAppStore((state) => state.addToast);
  const resolvedTheme = useAppStore((state) => state.resolvedTheme);
  const [isApplying, setIsApplying] = useState(false);
  const theme = diffTheme(resolvedTheme);

  const acceptedCount = hunks.filter((hunk) => hunk.accepted).length;
  const changedSinceAudit = currentContent !== fileAtAuditStart;
  const isStaleDeployment = proposalSkillMdPath !== skillMdPath;

  const setAccepted = (index: number, accepted: boolean) => {
    onHunksChange(hunks.map((hunk) => (hunk.index === index ? { ...hunk, accepted } : hunk)));
  };

  const handleApply = async () => {
    if (acceptedCount === 0 || isApplying || isStaleDeployment) return;
    const patched = applyAcceptedHunks(currentContent, hunks);
    if (patched === null) {
      addToast({
        type: "error",
        title: "Couldn't apply the changes",
        message: "The file changed since the audit ran.",
      });
      return;
    }
    setIsApplying(true);
    try {
      await writeInstalledSkillMdIfUnchanged(skillMdPath, currentContent, patched);
      onApplied(patched);
      addToast({ type: "success", title: "SKILL.md updated" });
    } catch (err) {
      const message = err instanceof Error ? err.message : "Unknown error";
      addToast({ type: "error", title: "Couldn't save SKILL.md", message });
      onDiskChanged();
    } finally {
      setIsApplying(false);
    }
  };

  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex items-baseline justify-between">
        <span className="text-caption font-semibold tracking-[0.04em] text-text-tertiary uppercase">
          Proposed changes
        </span>
        <span className="text-caption tabular-nums text-text-tertiary">
          {hunks.length} hunks · {acceptedCount} accepted
        </span>
      </div>

      {hunks.map((hunk, index) => {
        const headerId = `skill-hunk-${index}`;
        return (
          <div
            key={hunk.index}
            className="flex flex-col overflow-hidden rounded-sm border border-border-subtle transition-opacity"
            style={{ opacity: hunk.accepted ? 1 : 0.5 }}
          >
            <div
              className="flex h-7 items-center justify-between gap-2 border-b border-border-subtle bg-bg-tertiary px-2"
              id={headerId}
            >
              <span className="overflow-hidden text-ellipsis whitespace-nowrap font-mono text-caption text-text-secondary">
                {hunk.header}
              </span>
              <div className="flex shrink-0 gap-1" role="group" aria-labelledby={headerId}>
                <button
                  type="button"
                  className="h-6 cursor-pointer rounded-sm border border-border bg-transparent px-2 text-caption text-text-secondary transition-colors aria-pressed:bg-accent-soft aria-pressed:text-text-primary"
                  aria-pressed={hunk.accepted}
                  aria-label={`Accept change ${index + 1} of ${hunks.length}`}
                  onClick={() => setAccepted(hunk.index, true)}
                >
                  Accept
                </button>
                <button
                  type="button"
                  className="h-6 cursor-pointer rounded-sm border border-border bg-transparent px-2 text-caption text-text-secondary transition-colors aria-pressed:bg-accent-soft aria-pressed:text-text-primary"
                  aria-pressed={!hunk.accepted}
                  aria-label={`Reject change ${index + 1} of ${hunks.length}`}
                  onClick={() => setAccepted(hunk.index, false)}
                >
                  Reject
                </button>
              </div>
            </div>
            <div className="skill-proposed-hunk-body">
              <PatchDiff
                patch={hunk.patchText}
                options={{ theme, disableLineNumbers: true, disableFileHeader: true }}
              />
            </div>
          </div>
        );
      })}

      {changedSinceAudit && (
        <p className="m-0 text-caption text-warning">SKILL.md changed since this audit ran.</p>
      )}

      {isStaleDeployment && (
        <p className="m-0 text-caption text-warning">
          This proposal was made for a different copy of the skill.
        </p>
      )}

      <div className="flex gap-2">
        <Button
          size="sm"
          onClick={handleApply}
          disabled={acceptedCount === 0 || isApplying || isStaleDeployment}
        >
          Apply {acceptedCount} changes
        </Button>
        <Button variant="outline" size="sm" onClick={onDiscard} disabled={isApplying}>
          Discard
        </Button>
      </div>
    </div>
  );
}
