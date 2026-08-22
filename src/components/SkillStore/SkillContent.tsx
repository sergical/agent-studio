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
    <div className="skill-detail-content-section">
      <div className="skill-detail-content-header">
        <FileText size={14} />
        <span>SKILL.md</span>
      </div>
      <div className="skill-detail-content">
        {isLoadingContent ? (
          <div className="skill-detail-content-loading">
            <span className="spinner" />
            Loading content...
          </div>
        ) : skillContent ? (
          <pre className="skill-detail-content-text">{skillContent}</pre>
        ) : skill.description ? (
          <p className="skill-detail-content-fallback">{skill.description}</p>
        ) : (
          <p className="skill-detail-content-empty">No content available</p>
        )}
      </div>
    </div>
  );
}
