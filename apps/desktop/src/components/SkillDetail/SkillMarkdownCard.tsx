// ============================================================================
// SkillMarkdownCard - the "SKILL.md" card: header (Edit, or Cancel/Save while
// editing), and the rendered markdown / raw-text editor / loading skeleton /
// error state below it.
// ============================================================================

import ReactMarkdown from "react-markdown";
import { Button } from "@skill-studio/ui";
import { pluginLabelForSkill } from "@skill-studio/lib";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import { SkillMarkdownEditor } from "./SkillMarkdownEditor";

interface SkillMarkdownCardProps {
  skill: InstalledSkill;
  isPluginManaged: boolean;
  /** True when the caller opened a specific deployment that's no longer installed. */
  deploymentUnresolved: boolean;
  /** Own deployments to offer as a fallback when `deploymentUnresolved` - each opens that copy instead. */
  ownDeploymentOptions: Deployment[];
  onSelectDeployment: (path: string) => void;
  rawContent: string | null;
  isLoadingContent: boolean;
  loadError: string | null;
  onRetry: () => void;
  isEditing: boolean;
  isEditorDirty: boolean;
  onStartEdit: () => void;
  isSaving: boolean;
  saveLabel: string;
  onSave: (content: string) => void;
  onCancelEdit: () => void;
  onDirtyChange: (isDirty: boolean) => void;
}

/** Strips a leading `---\n...\n---\n` YAML frontmatter block, if present. */
function stripFrontmatter(content: string): string {
  return content.replace(/^---\n[\s\S]*?\n---\n/, "");
}

/** A six-line skeleton shown while SKILL.md loads, instead of a spinner. */
/** Widths mirror the original skeleton's per-line variation, so a placeholder line doesn't read as a full sentence. */
const SKELETON_LINE_WIDTHS = ["100%", "92%", "96%", "60%", "88%", "40%"];

function MarkdownSkeleton() {
  return (
    <div className="flex flex-col gap-2.5 px-5 py-4" aria-hidden="true">
      {SKELETON_LINE_WIDTHS.map((width, i) => (
        <div key={i} className="h-3 animate-pulse rounded-xs bg-bg-tertiary" style={{ width }} />
      ))}
    </div>
  );
}

export function SkillMarkdownCard({
  skill,
  isPluginManaged,
  deploymentUnresolved,
  ownDeploymentOptions,
  onSelectDeployment,
  rawContent,
  isLoadingContent,
  loadError,
  onRetry,
  isEditing,
  isEditorDirty,
  onStartEdit,
  isSaving,
  saveLabel,
  onSave,
  onCancelEdit,
  onDirtyChange,
}: SkillMarkdownCardProps) {
  const canEdit = !isEditing && !isPluginManaged && !deploymentUnresolved && rawContent !== null;

  return (
    <div className="flex flex-col gap-1 rounded-lg border border-border-subtle p-4">
      <div className="flex items-baseline justify-between gap-3 text-body font-semibold text-text-primary">
        <span>SKILL.md</span>
        {isEditing ? (
          <span className="flex items-center">
            {isEditorDirty && (
              <span className="mr-auto text-caption text-warning">Unsaved changes</span>
            )}
          </span>
        ) : (
          canEdit && (
            <Button variant="outline" size="sm" onClick={onStartEdit}>
              Edit
            </Button>
          )
        )}
      </div>

      {deploymentUnresolved ? (
        <div className="m-0 p-3 text-body leading-[1.5] text-text-secondary">
          <p>The copy you opened is no longer installed.</p>
          {ownDeploymentOptions.length > 0 && (
            <div className="flex flex-wrap gap-2">
              {ownDeploymentOptions.map((d) => (
                <Button
                  key={d.path}
                  variant="outline"
                  size="sm"
                  onClick={() => onSelectDeployment(d.path)}
                >
                  {d.agent} · {d.scope}
                </Button>
              ))}
            </div>
          )}
        </div>
      ) : isPluginManaged ? (
        <p className="m-0 p-3 text-body leading-[1.5] text-text-secondary">
          Managed by the {pluginLabelForSkill(skill) ?? "harness"} plugin.
        </p>
      ) : isEditing && rawContent !== null ? (
        <SkillMarkdownEditor
          initialContent={rawContent}
          isSaving={isSaving}
          saveLabel={saveLabel}
          onSave={onSave}
          onCancel={onCancelEdit}
          onDirtyChange={onDirtyChange}
        />
      ) : isLoadingContent ? (
        <MarkdownSkeleton />
      ) : loadError ? (
        <div className="m-0 flex items-center justify-between gap-3 p-3 text-body leading-[1.5] text-error">
          <span>{loadError}</span>
          <Button variant="outline" size="sm" onClick={onRetry}>
            Retry
          </Button>
        </div>
      ) : rawContent !== null ? (
        <div className="skill-markdown">
          <ReactMarkdown
            components={{
              table: ({ children }) => (
                <div className="skill-markdown-table-wrap">
                  <table>{children}</table>
                </div>
              ),
            }}
          >
            {stripFrontmatter(rawContent)}
          </ReactMarkdown>
        </div>
      ) : (
        <p className="m-0 px-3 py-6 text-small text-text-tertiary">No content available</p>
      )}
    </div>
  );
}
