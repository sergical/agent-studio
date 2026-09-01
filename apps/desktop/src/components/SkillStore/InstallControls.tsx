// ============================================================================
// InstallControls - Agent selector, scope, project picker, and
// install/remove/update actions for a skill
// ============================================================================

import { useState } from "react";
import { Download, Trash2, RefreshCw, FolderPlus } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { Button } from "@skill-studio/ui";
import { AgentTargetSelector } from "./AgentTargetSelector";
import { installSkill, removeSkill, updateSkill } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import type { AgentId, InstallScope, SkillWithStatus } from "@skill-studio/lib";
import { COMMON_AGENTS } from "@skill-studio/lib";

const SCOPE_OPTION_CLASS =
  "flex-1 rounded-sm border border-border bg-bg-primary px-2.5 py-2.5 text-body font-medium text-text-secondary transition-colors hover:border-border-focus";
const SCOPE_OPTION_SELECTED_CLASS = "border-accent bg-accent-softer text-accent";
const ACTION_BUTTON_CLASS =
  "h-(--control-height) w-full justify-center gap-2 rounded-md px-3.5 text-body font-medium";

const AGENT_PREFS_STORAGE_KEY = "skill-store-agent-prefs.v1";
/** Pre-versioning key name, read once as a migration fallback so existing prefs aren't lost. */
const AGENT_PREFS_STORAGE_KEY_LEGACY = "skill-store-agent-prefs";

function loadAgentPrefs(): AgentId[] {
  const saved =
    localStorage.getItem(AGENT_PREFS_STORAGE_KEY) ??
    localStorage.getItem(AGENT_PREFS_STORAGE_KEY_LEGACY);
  if (saved) {
    try {
      return JSON.parse(saved);
    } catch {
      return COMMON_AGENTS;
    }
  }
  return COMMON_AGENTS;
}

interface InstallControlsProps {
  skill: SkillWithStatus;
  resolvedTopSource: string | null;
  onInstallStart: (skillName: string) => void;
  onInstallComplete: (result: { success: boolean; error?: string; skillName?: string }) => void;
  onRemoveComplete: () => void;
}

export function InstallControls({
  skill,
  resolvedTopSource,
  onInstallStart,
  onInstallComplete,
  onRemoveComplete,
}: InstallControlsProps) {
  const [selectedAgents, setSelectedAgents] = useState<AgentId[]>(loadAgentPrefs);
  const [installScope, setInstallScope] = useState<InstallScope>("global");
  const [isInstalling, setIsInstalling] = useState(false);
  const [isRemoving, setIsRemoving] = useState(false);
  const [isUpdating, setIsUpdating] = useState(false);
  const [showRemoveConfirm, setShowRemoveConfirm] = useState(false);
  const [selectedProject, setSelectedProject] = useState<string | null>(null);

  // Project directories the user has pointed at for project-scoped installs
  const availableProjects = useAppStore((state) => state.userAddedProjects);
  const addProject = useAppStore((state) => state.addProject);

  // Persist agent preferences to localStorage whenever they change
  const handleAgentsChange = (agents: AgentId[]) => {
    setSelectedAgents(agents);
    localStorage.setItem(AGENT_PREFS_STORAGE_KEY, JSON.stringify(agents));
  };

  // Auto-select first project when switching to project scope
  const handleSetInstallScope = (scope: InstallScope) => {
    setInstallScope(scope);
    if (scope === "project" && availableProjects.length > 0 && !selectedProject) {
      setSelectedProject(availableProjects[0]);
    }
  };

  const handleInstall = () => {
    if (selectedAgents.length === 0) {
      return;
    }

    // For project scope, require a selected project
    if (installScope === "project" && !selectedProject) {
      return;
    }

    setIsInstalling(true);
    onInstallStart(skill.name);

    // Build skill source: repo/skill-name for multi-skill repos, or just the source
    const repoSource = skill.top_source || resolvedTopSource;
    // If we have a repo source and skill name differs from repo, include both
    const skillSource = repoSource ? `${repoSource}/${skill.name}` : skill.name;

    installSkill({
      skill_source: skillSource,
      scope: installScope,
      agents: selectedAgents,
      project_path: installScope === "project" ? (selectedProject ?? undefined) : undefined,
    })
      .then((result) => {
        onInstallComplete({ success: result.success, error: result.error, skillName: skill.name });
      })
      .catch((err) => {
        onInstallComplete({
          success: false,
          error: err instanceof Error ? err.message : "Unknown error",
          skillName: skill.name,
        });
      })
      .finally(() => {
        setIsInstalling(false);
      });
  };

  const handleRemove = () => {
    setIsRemoving(true);
    // A `Promise.finally` call, not a `try/finally` statement: `removeSkill`
    // throwing (no catch here, same as before) still runs the cleanup
    // before the rejection propagates.
    return removeSkill(skill.name, installScope === "global")
      .then((result) => {
        if (result.success) {
          onRemoveComplete();
        }
      })
      .finally(() => {
        setIsRemoving(false);
        setShowRemoveConfirm(false);
      });
  };

  // A Promise chain, not a try/finally statement, so the compiler can still
  // optimize this component (it doesn't support `finally` clauses yet).
  const handleUpdate = () => {
    setIsUpdating(true);
    return updateSkill(skill.name, installScope === "global")
      .then((result) => {
        if (result.success) {
          onInstallComplete({ success: true, skillName: skill.name });
        } else {
          onInstallComplete({ success: false, error: result.error, skillName: skill.name });
        }
      })
      .catch((err) => {
        onInstallComplete({
          success: false,
          error: err instanceof Error ? err.message : "Unknown error",
          skillName: skill.name,
        });
      })
      .finally(() => {
        setIsUpdating(false);
      });
  };

  const handleBrowseProject = async () => {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Select Project Directory",
    });
    if (selected) {
      addProject(selected);
      setSelectedProject(selected);
    }
  };

  if (!skill.is_installed) {
    return (
      <>
        <div className="p-5">
          <h4 className="m-0 mb-3 text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
            Install scope
          </h4>
          <div className="flex gap-2">
            <button
              type="button"
              className={`${SCOPE_OPTION_CLASS} ${installScope === "global" ? SCOPE_OPTION_SELECTED_CLASS : ""}`}
              onClick={() => handleSetInstallScope("global")}
            >
              Global
            </button>
            <button
              type="button"
              className={`${SCOPE_OPTION_CLASS} ${installScope === "project" ? SCOPE_OPTION_SELECTED_CLASS : ""}`}
              onClick={() => handleSetInstallScope("project")}
            >
              Project
            </button>
          </div>

          {/* Project selector dropdown when project scope is selected */}
          {installScope === "project" && (
            <div className="mt-3">
              {/* A heading for the project picker below, not a form control's
                  label - a `<label>` here would have no associated control
                  when `availableProjects` is empty and the select doesn't render. */}
              <span className="mb-1.5 block text-caption font-medium tracking-[0.04em] text-text-tertiary uppercase">
                Project directory
              </span>
              <div className="flex gap-2">
                {availableProjects.length > 0 && (
                  <select
                    className="flex-1 rounded-sm border border-border bg-bg-primary px-2.5 py-2.5 text-body text-text-primary"
                    aria-label="Project directory"
                    value={selectedProject || ""}
                    onChange={(e) => setSelectedProject(e.target.value)}
                  >
                    {availableProjects.map((p) => (
                      <option key={p} value={p}>
                        {p.split("/").pop()} – {p}
                      </option>
                    ))}
                  </select>
                )}
                <Button
                  variant="outline"
                  className={ACTION_BUTTON_CLASS}
                  onClick={handleBrowseProject}
                >
                  <FolderPlus size={14} />
                  {availableProjects.length === 0 ? "Choose Directory" : "Add"}
                </Button>
              </div>
            </div>
          )}
        </div>

        <div className="p-5">
          <AgentTargetSelector
            selectedAgents={selectedAgents}
            onChange={handleAgentsChange}
            disabled={isInstalling}
          />
        </div>

        <div className="mt-auto flex flex-col gap-2 p-5">
          <Button
            className={`${ACTION_BUTTON_CLASS} bg-accent text-text-on-accent hover:bg-accent-hover`}
            onClick={handleInstall}
            disabled={
              isInstalling ||
              selectedAgents.length === 0 ||
              (installScope === "project" && !selectedProject)
            }
          >
            {isInstalling ? (
              <>
                <span className="size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent" />
                Installing…
              </>
            ) : (
              <>
                <Download size={16} />
                Install Skill
              </>
            )}
          </Button>
        </div>
      </>
    );
  }

  return (
    <div className="mt-auto flex flex-col gap-2 p-5">
      {skill.installed_info?.has_update && (
        <Button
          className={`${ACTION_BUTTON_CLASS} bg-accent text-text-on-accent hover:bg-accent-hover`}
          onClick={handleUpdate}
          disabled={isUpdating}
        >
          {isUpdating ? (
            <>
              <span className="size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent" />
              Updating…
            </>
          ) : (
            <>
              <RefreshCw size={16} />
              Update Skill
            </>
          )}
        </Button>
      )}
      {showRemoveConfirm ? (
        <div className="flex gap-2">
          <Button
            className={`${ACTION_BUTTON_CLASS} flex-1 bg-error-soft text-error hover:bg-error hover:text-white`}
            onClick={handleRemove}
            disabled={isRemoving}
          >
            {isRemoving ? (
              <>
                <span className="size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent" />
                Removing…
              </>
            ) : (
              "Confirm Remove"
            )}
          </Button>
          <Button
            variant="secondary"
            className={`${ACTION_BUTTON_CLASS} flex-1 bg-bg-tertiary text-text-secondary`}
            onClick={() => setShowRemoveConfirm(false)}
            disabled={isRemoving}
          >
            Cancel
          </Button>
        </div>
      ) : (
        <Button
          className={`${ACTION_BUTTON_CLASS} bg-error-soft text-error hover:bg-error hover:text-white`}
          onClick={() => setShowRemoveConfirm(true)}
          disabled={isRemoving}
        >
          <Trash2 size={16} />
          Remove Skill
        </Button>
      )}
    </div>
  );
}
