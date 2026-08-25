// ============================================================================
// SkillProposedEdits - Renders one Audit run's proposed SKILL.md rewrite as
// per-hunk unified diffs, each independently accept/reject-able, with an
// Apply footer that writes the accepted hunks back through to disk
// ============================================================================

import { useEffect, useState } from "react";
import { PatchDiff } from "@pierre/diffs/react";
import { writeInstalledSkillMd } from "../../lib/skill-api";
import type { SkillMdHunk } from "../../lib/skill-md-diff";
import { applyAcceptedHunks } from "../../lib/skill-md-diff";
import { useAppStore } from "../../store/appStore";

interface SkillProposedEditsProps {
  /** The file the audit ran against, before any of this proposal is applied. */
  fileAtAuditStart: string;
  /** The file's current content, read live from `SkillPage` - may have moved on since the audit started. */
  currentContent: string;
  skillMdPath: string;
  hunks: SkillMdHunk[];
  onHunksChange: (hunks: SkillMdHunk[]) => void;
  onApplied: (content: string) => void;
  onDiscard: () => void;
}

/** True when `document.documentElement` is explicitly light, or defers to the OS when it isn't set. */
function isLightTheme(): boolean {
  const attr = document.documentElement.getAttribute("data-theme");
  if (attr === "light") return true;
  if (attr === "dark") return false;
  return !window.matchMedia("(prefers-color-scheme: dark)").matches;
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
  hunks,
  onHunksChange,
  onApplied,
  onDiscard,
}: SkillProposedEditsProps) {
  const addToast = useAppStore((state) => state.addToast);
  const [isApplying, setIsApplying] = useState(false);
  const [theme, setTheme] = useState(() => (isLightTheme() ? "github-light" : "github-dark"));

  useEffect(() => {
    // No theme toggle sets `data-theme` in JS yet, so only the OS scheme
    // can change after mount; that's the only change worth watching for.
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const onMediaChange = () => setTheme(isLightTheme() ? "github-light" : "github-dark");
    media.addEventListener("change", onMediaChange);
    return () => media.removeEventListener("change", onMediaChange);
  }, []);

  const acceptedCount = hunks.filter((hunk) => hunk.accepted).length;
  const changedSinceAudit = currentContent !== fileAtAuditStart;

  const setAccepted = (index: number, accepted: boolean) => {
    onHunksChange(hunks.map((hunk) => (hunk.index === index ? { ...hunk, accepted } : hunk)));
  };

  const handleApply = async () => {
    if (acceptedCount === 0 || isApplying) return;
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
      await writeInstalledSkillMd(skillMdPath, patched);
      onApplied(patched);
      addToast({ type: "success", title: "SKILL.md updated" });
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't save SKILL.md",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
      setIsApplying(false);
    }
  };

  return (
    <div className="skill-proposed-edits">
      <div className="skill-proposed-header">
        <span className="skill-proposed-label">Proposed changes</span>
        <span className="skill-proposed-count">
          {hunks.length} hunks · {acceptedCount} accepted
        </span>
      </div>

      {hunks.map((hunk) => (
        <div
          key={hunk.index}
          className="skill-proposed-hunk"
          style={{ opacity: hunk.accepted ? 1 : 0.5 }}
        >
          <div className="skill-proposed-hunk-header">
            <span className="skill-proposed-hunk-header-text">{hunk.header}</span>
            <div className="skill-proposed-hunk-toggles">
              <button
                type="button"
                className="skill-proposed-hunk-toggle"
                aria-pressed={hunk.accepted}
                onClick={() => setAccepted(hunk.index, true)}
              >
                Accept
              </button>
              <button
                type="button"
                className="skill-proposed-hunk-toggle"
                aria-pressed={!hunk.accepted}
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
      ))}

      {changedSinceAudit && (
        <p className="skill-proposed-warning">SKILL.md changed since this audit ran.</p>
      )}

      <div className="skill-proposed-footer">
        <button
          type="button"
          className="skill-action-button primary"
          onClick={handleApply}
          disabled={acceptedCount === 0 || isApplying}
        >
          Apply {acceptedCount} changes
        </button>
        <button
          type="button"
          className="skill-action-button"
          onClick={onDiscard}
          disabled={isApplying}
        >
          Discard
        </button>
      </div>
    </div>
  );
}
