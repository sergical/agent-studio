// ============================================================================
// SkillBrowser - Grid/list of skills from search results
// ============================================================================

import { Download, Check, ArrowUp } from 'lucide-react';
import type { SkillWithStatus } from '../../lib/skillsTypes';

interface SkillBrowserProps {
  skills: SkillWithStatus[];
  selectedSkill: SkillWithStatus | null;
  onSelectSkill: (skill: SkillWithStatus) => void;
  isLoading: boolean;
  isLoadingMore: boolean;
  hasMore: boolean;
  onLoadMore: () => void;
  emptyMessage: string;
  hideInstalledIndicator?: boolean;
}

export function SkillBrowser({
  skills,
  selectedSkill,
  onSelectSkill,
  isLoading,
  isLoadingMore,
  hasMore,
  onLoadMore,
  emptyMessage,
  hideInstalledIndicator = false,
}: SkillBrowserProps) {
  if (isLoading && skills.length === 0) {
    return (
      <div className="skill-browser skill-browser-loading">
        <div className="skill-browser-spinner" />
        <p>Searching skills.sh...</p>
      </div>
    );
  }

  if (skills.length === 0) {
    return (
      <div className="skill-browser skill-browser-empty">
        <p>{emptyMessage}</p>
      </div>
    );
  }

  return (
    <div className="skill-browser">
      <div className="skill-browser-grid">
        {skills.map(skill => (
          <SkillCard
            key={skill.id}
            skill={skill}
            isSelected={selectedSkill?.id === skill.id}
            onClick={() => onSelectSkill(skill)}
            hideInstalledIndicator={hideInstalledIndicator}
          />
        ))}
      </div>
      {hasMore && (
        <div className="skill-browser-load-more">
          <button
            className="load-more-button"
            onClick={onLoadMore}
            disabled={isLoadingMore}
          >
            {isLoadingMore ? (
              <>
                <span className="load-more-spinner" />
                Loading...
              </>
            ) : (
              'Load More'
            )}
          </button>
        </div>
      )}
    </div>
  );
}

interface SkillCardProps {
  skill: SkillWithStatus;
  isSelected: boolean;
  onClick: () => void;
  hideInstalledIndicator?: boolean;
}

function SkillCard({ skill, isSelected, onClick, hideInstalledIndicator = false }: SkillCardProps) {
  const formatInstalls = (count: number): string => {
    if (count >= 1000) {
      return `${(count / 1000).toFixed(1)}k`;
    }
    return count.toString();
  };

  return (
    <button
      className={`skill-card ${isSelected ? 'selected' : ''} ${skill.is_installed && !hideInstalledIndicator ? 'installed' : ''}`}
      onClick={onClick}
    >
      <div className="skill-card-header">
        <h3 className="skill-card-name">{skill.name}</h3>
        <div className="skill-card-status">
          {skill.is_installed && !hideInstalledIndicator ? (
            skill.installed_info?.has_update ? (
              <span className="skill-status-update" title="Update available">
                <ArrowUp size={12} />
              </span>
            ) : (
              <span className="skill-status-installed" title="Installed">
                <Check size={12} />
              </span>
            )
          ) : null}
        </div>
      </div>

      {skill.top_source && (
        <div className="skill-card-source">{skill.top_source}</div>
      )}

      {skill.description && (
        <p className="skill-card-description">{skill.description}</p>
      )}

      <div className="skill-card-footer">
        <span className="skill-card-installs" title="Install count">
          <Download size={12} />
          {formatInstalls(skill.installs)}
        </span>
        {skill.tags && skill.tags.length > 0 && (
          <div className="skill-card-tags">
            {skill.tags.slice(0, 2).map(tag => (
              <span key={tag} className="skill-card-tag">{tag}</span>
            ))}
          </div>
        )}
      </div>
    </button>
  );
}
