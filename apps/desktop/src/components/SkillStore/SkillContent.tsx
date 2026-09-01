// ============================================================================
// SkillContent - Renders the SKILL.md viewer for a selected skill
// ============================================================================

import { FileText } from "lucide-react";
import type { SkillWithStatus } from "@skill-studio/lib";

interface SkillContentProps {
  skill: SkillWithStatus;
  /** The skill's SKILL.md/AGENTS.md body, from `getSkillDetails` - `null`
   * while the parent's details fetch is still loading, or once it resolves
   * with no markdown file at all. */
  skillMd: string | null;
  /** True while the parent's details fetch is in flight. */
  isLoading: boolean;
}

export function SkillContent({ skill, skillMd, isLoading }: SkillContentProps) {
  return (
    <div className="mx-5 mb-4 overflow-hidden rounded-md border border-border">
      <div className="flex items-center gap-1.5 border-b border-border bg-bg-tertiary px-3 py-2 text-caption font-medium tracking-[0.04em] text-text-secondary uppercase">
        <FileText size={14} />
        <span>SKILL.md</span>
      </div>
      <div className="max-h-[400px] overflow-y-auto bg-bg-secondary">
        {isLoading ? (
          <div className="flex items-center justify-center gap-2 p-6 text-small text-text-tertiary">
            <span className="size-3.5 animate-spin rounded-full border-2 border-border border-t-accent" />
            Loading content…
          </div>
        ) : skillMd ? (
          <pre className="m-0 p-3 font-mono text-small leading-[1.5] break-words whitespace-pre-wrap text-text-secondary">
            {skillMd}
          </pre>
        ) : skill.description ? (
          <p className="m-0 p-3 text-body leading-[1.5] text-text-secondary">{skill.description}</p>
        ) : (
          <p className="text-pretty m-0 p-3 py-6 text-center text-small italic text-text-tertiary">
            No content available
          </p>
        )}
      </div>
    </div>
  );
}
