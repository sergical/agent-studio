// ============================================================================
// SkillListEmptyState - SkillListTable's empty state: nothing installed at
// all, or nothing matches the caller's current filters.
// ============================================================================

import { Button } from "@skill-studio/ui";

interface SkillListEmptyStateProps {
  /** False when the caller's underlying list (before any filter) is empty. */
  hasAnySkills: boolean;
  onClearFilters?: () => void;
  onAddSkill?: () => void;
}

export function SkillListEmptyState({
  hasAnySkills,
  onClearFilters,
  onAddSkill,
}: SkillListEmptyStateProps) {
  return (
    <div className="flex flex-col items-start gap-2 text-pretty text-small text-text-tertiary">
      {hasAnySkills ? (
        <>
          <p className="m-0">No skills match</p>
          {onClearFilters && (
            <Button
              variant="secondary"
              className="rounded-sm border border-border text-text-primary"
              onClick={onClearFilters}
            >
              Clear filters
            </Button>
          )}
        </>
      ) : (
        <>
          <p className="m-0">You haven't added a skill yet</p>
          {onAddSkill && (
            <Button
              variant="secondary"
              className="rounded-sm border border-border text-text-primary"
              onClick={onAddSkill}
            >
              Add skill
            </Button>
          )}
        </>
      )}
    </div>
  );
}
