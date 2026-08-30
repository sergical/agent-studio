// ============================================================================
// SkillMarkdownCard - the "SKILL.md" card: header (Edit, or Cancel/Save while
// editing), and the rendered markdown / raw-text editor / loading skeleton /
// error state below it.
// ============================================================================

import ReactMarkdown from "react-markdown";
import { pluginLabelForSkill } from "../../lib/skill-plugin-partition";
import type { Deployment, InstalledSkill } from "../../lib/skill-types";
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
function MarkdownSkeleton() {
  return (
    <div className="skill-markdown-skeleton" aria-hidden="true">
      {Array.from({ length: 6 }, (_, i) => (
        <div key={i} className="skill-markdown-skeleton-line" />
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
    <div className="home-block skill-markdown-card">
      <div className="home-block-header">
        <span>SKILL.md</span>
        {isEditing ? (
          <span className="skill-markdown-card-edit-status">
            {isEditorDirty && <span className="skill-markdown-editor-dirty">Unsaved changes</span>}
          </span>
        ) : (
          canEdit && (
            <button type="button" className="skill-action-button" onClick={onStartEdit}>
              Edit
            </button>
          )
        )}
      </div>

      {deploymentUnresolved ? (
        <div className="skill-detail-content-fallback">
          <p>The copy you opened is no longer installed.</p>
          {ownDeploymentOptions.length > 0 && (
            <div className="skill-detail-actions-row">
              {ownDeploymentOptions.map((d) => (
                <button
                  key={d.path}
                  className="skill-action-button"
                  onClick={() => onSelectDeployment(d.path)}
                >
                  {d.agent} · {d.scope}
                </button>
              ))}
            </div>
          )}
        </div>
      ) : isPluginManaged ? (
        <p className="skill-detail-content-fallback">
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
        <div className="skill-detail-content-fallback skill-markdown-card-error">
          <span>{loadError}</span>
          <button type="button" className="skill-action-button" onClick={onRetry}>
            Retry
          </button>
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
        <p className="skill-detail-content-empty">No content available</p>
      )}
    </div>
  );
}
