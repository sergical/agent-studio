// ============================================================================
// SkillContent - Fetches and renders the SKILL.md viewer for a skill
// ============================================================================

import { useEffect, useState } from "react";
import { FileText } from "lucide-react";
import { fetchSkillMdContent } from "../../lib/github-skill-source";
import type { SkillWithStatus } from "../../lib/skill-types";

interface SkillContentProps {
  skill: SkillWithStatus;
  resolvedTopSource: string | null;
}

export function SkillContent({ skill, resolvedTopSource }: SkillContentProps) {
  const [skillContent, setSkillContent] = useState<string | null>(null);
  const [isLoadingContent, setIsLoadingContent] = useState(false);

  // Fetch SKILL.md content - handles both well-known (direct URL) and GitHub sources
  useEffect(() => {
    let cancelled = false;

    setIsLoadingContent(true);
    fetchSkillMdContent({
      name: skill.name,
      topSource: resolvedTopSource,
      installedInfo: skill.installed_info,
    })
      .then((content) => {
        if (!cancelled) {
          setSkillContent(content);
        }
      })
      .finally(() => {
        if (!cancelled) {
          setIsLoadingContent(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [resolvedTopSource, skill.name, skill.installed_info]);

  return (
    <div className="mx-5 mb-4 overflow-hidden rounded-md border border-border">
      <div className="flex items-center gap-1.5 border-b border-border bg-bg-tertiary px-3 py-2 text-caption font-medium tracking-[0.04em] text-text-secondary uppercase">
        <FileText size={14} />
        <span>SKILL.md</span>
      </div>
      <div className="max-h-[400px] overflow-y-auto bg-bg-secondary">
        {isLoadingContent ? (
          <div className="flex items-center justify-center gap-2 p-6 text-small text-text-tertiary">
            <span className="size-3.5 animate-spin rounded-full border-2 border-border border-t-accent" />
            Loading content…
          </div>
        ) : skillContent ? (
          <pre className="m-0 p-3 font-mono text-small leading-[1.5] break-words whitespace-pre-wrap text-text-secondary">
            {skillContent}
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
