// ============================================================================
// SkillBrowser - Grid/list of skills from search results
// ============================================================================

import { Download, Check, ArrowUp, Link2, FileCheck2, AlertTriangle } from "lucide-react";
import { Button } from "@skill-studio/ui";
import type { SkillWithStatus } from "@skill-studio/lib";
import { SOURCE_KIND_LABELS } from "@skill-studio/lib";

/**
 * The Browse tab pages through skills.sh and shows an install indicator per
 * card; the Installed tab always shows everything it has (no pagination) and
 * hides that indicator, since every card there is already installed. One
 * discriminated prop keeps those two states from combining into invalid
 * on/off pairs (e.g. `isLoadingMore` true while `hideInstalledIndicator` is
 * also true) that no call site ever actually produces.
 */
export type SkillBrowserMode =
  | {
      kind: "browse";
      isLoading: boolean;
      isLoadingMore: boolean;
      hasMore: boolean;
      onLoadMore: () => void;
    }
  | { kind: "installed" };

interface SkillBrowserProps {
  skills: SkillWithStatus[];
  selectedSkill: SkillWithStatus | null;
  onSelectSkill: (skill: SkillWithStatus) => void;
  emptyMessage: string;
  mode: SkillBrowserMode;
}

export function SkillBrowser({
  skills,
  selectedSkill,
  onSelectSkill,
  emptyMessage,
  mode,
}: SkillBrowserProps) {
  const isLoading = mode.kind === "browse" && mode.isLoading;
  const hideInstalledIndicator = mode.kind === "installed";

  if (isLoading && skills.length === 0) {
    return (
      <div className="flex h-[200px] flex-col items-center justify-center text-pretty text-body text-text-tertiary">
        <span className="mb-3 size-6 animate-spin rounded-full border-2 border-border border-t-accent" />
        <p>Searching skills.sh…</p>
      </div>
    );
  }

  if (skills.length === 0) {
    return (
      <div className="flex h-[200px] flex-col items-center justify-center text-pretty text-body text-text-tertiary">
        <p>{emptyMessage}</p>
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-y-auto px-7 py-5">
      <div className="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-4">
        {skills.map((skill) => (
          <SkillCard
            key={skill.id}
            skill={skill}
            isSelected={selectedSkill?.id === skill.id}
            onClick={() => onSelectSkill(skill)}
            hideInstalledIndicator={hideInstalledIndicator}
          />
        ))}
      </div>
      {mode.kind === "browse" && mode.hasMore && (
        <div className="flex justify-center pt-6 pb-2">
          <Button
            variant="outline"
            className="h-auto rounded-md border-border bg-bg-secondary px-6 py-2.5 text-body font-medium text-text-secondary hover:border-border-focus hover:bg-bg-tertiary hover:text-text-primary"
            onClick={mode.onLoadMore}
            disabled={mode.isLoadingMore}
          >
            {mode.isLoadingMore ? (
              <>
                <span className="size-3.5 animate-spin rounded-full border-2 border-border border-t-accent" />
                Loading…
              </>
            ) : (
              "Load More"
            )}
          </Button>
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

  const isInstalledMarked = skill.is_installed && !hideInstalledIndicator;
  // Mirrors the old cascade: an installed border always wins over selected,
  // but a selected background always wins over the plain hover background.
  const borderClass = isInstalledMarked
    ? "border-success"
    : isSelected
      ? "border-accent"
      : "border-border hover:border-border-focus";
  const bgClass = isSelected ? "bg-accent-softer" : "bg-bg-secondary hover:bg-bg-tertiary";

  return (
    <button
      className={`flex flex-col gap-2 rounded-md border p-4 text-left transition-colors ${borderClass} ${bgClass}`}
      onClick={onClick}
    >
      <div className="flex items-center justify-between gap-2">
        <h3 className="m-0 truncate text-emphasis font-semibold text-text-primary">{skill.name}</h3>
        <div className="flex items-center gap-1">
          {isInstalledMarked ? (
            skill.installed_info?.has_update ? (
              <span
                className="flex size-[18px] items-center justify-center rounded-full bg-warning-soft text-warning"
                title="Update available"
              >
                <ArrowUp size={12} />
              </span>
            ) : (
              <span
                className="flex size-[18px] items-center justify-center rounded-full bg-success-soft text-success"
                title="Installed"
              >
                <Check size={12} />
              </span>
            )
          ) : null}
        </div>
      </div>

      {skill.top_source && (
        <div className="font-mono text-caption text-text-tertiary">{skill.top_source}</div>
      )}

      {hideInstalledIndicator && skill.installed_info && (
        <div className="flex flex-wrap items-center gap-1">
          <span
            className={`inline-flex w-fit items-center rounded-sm px-1.5 py-0.5 text-caption tracking-[0.04em] uppercase ${sourceKindClass(skill.installed_info.source_kind)}`}
          >
            {SOURCE_KIND_LABELS[skill.installed_info.source_kind]}
          </span>
          {skill.installed_info.has_spec && (
            <span
              className="inline-flex items-center gap-[3px] rounded-sm bg-success-soft px-1.5 py-0.5 text-caption text-success"
              title="Ships behavior specs/evals"
            >
              <FileCheck2 size={11} />
              spec
            </span>
          )}
          {skill.installed_info.spec_violations.length > 0 && (
            <span
              className="inline-flex items-center gap-[3px] rounded-sm bg-warning-soft px-1.5 py-0.5 text-caption text-warning"
              title={skill.installed_info.spec_violations.join("\n")}
            >
              <AlertTriangle size={11} />
              spec issues
            </span>
          )}
          {skill.installed_info.deployments.map((deployment) => (
            <span
              key={deployment.path}
              className="inline-flex items-center gap-[3px] rounded-sm bg-bg-tertiary px-1.5 py-0.5 text-caption text-text-tertiary"
            >
              {deployment.is_symlink && <Link2 size={10} />}
              {deployment.plugin
                ? `${deployment.agent} · via ${deployment.plugin.name}`
                : `${deployment.agent} · ${deployment.scope}`}
            </span>
          ))}
        </div>
      )}

      {(skill.description || skill.installed_info?.description) && (
        <p
          className="m-0 line-clamp-2 text-pretty text-small text-text-secondary"
          title={skill.description || skill.installed_info?.description}
        >
          {skill.description || skill.installed_info?.description}
        </p>
      )}

      <div className="mt-auto flex items-center justify-between gap-2 pt-2">
        <span
          className="flex items-center gap-1 text-caption tabular-nums text-text-tertiary"
          title="Install count"
        >
          <Download size={12} />
          {formatInstalls(skill.installs)}
        </span>
        {skill.tags && skill.tags.length > 0 && (
          <div className="flex gap-1">
            {skill.tags.slice(0, 2).map((tag) => (
              <span
                key={tag}
                className="rounded-xs bg-bg-tertiary px-1.5 py-0.5 text-caption text-text-tertiary"
              >
                {tag}
              </span>
            ))}
          </div>
        )}
      </div>
    </button>
  );
}

/** skills.sh: accent; dotagents: info; plugin: warning; in-repo/manual: tertiary; fork: success. */
function sourceKindClass(kind: string): string {
  switch (kind) {
    case "skills-sh":
      return "text-accent bg-accent-softer";
    case "dotagents":
      return "text-info bg-info-soft";
    case "plugin":
      return "text-warning bg-warning-soft";
    case "fork":
      return "text-success bg-success-soft";
    default:
      return "text-text-tertiary bg-bg-tertiary";
  }
}
