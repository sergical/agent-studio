// ============================================================================
// InstallControls - Agent selector, scope, project picker, and
// install/remove/update actions for a skill
// ============================================================================

import { useCallback, useState } from "react";
import { Download, Trash2, RefreshCw, FolderPlus } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { AgentTargetSelector } from "./AgentTargetSelector";
import { installSkill, removeSkill, updateSkill } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import type { AgentId, InstallScope, SkillWithStatus } from "../../lib/skill-types";
import { COMMON_AGENTS } from "../../lib/skill-types";

const AGENT_PREFS_STORAGE_KEY = "skill-store-agent-prefs";

function loadAgentPrefs(): AgentId[] {
  const saved = localStorage.getItem(AGENT_PREFS_STORAGE_KEY);
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
  const [selectedAgents, setSelectedAgentsState] = useState<AgentId[]>(loadAgentPrefs);
  const [installScope, setInstallScope] = useState<InstallScope>("global");
  const [isInstalling, setIsInstalling] = useState(false);
  const [isRemoving, setIsRemoving] = useState(false);
  const [isUpdating, setIsUpdating] = useState(false);
  const [showRemoveConfirm, setShowRemoveConfirm] = useState(false);
  const [selectedProject, setSelectedProjectState] = useState<string | null>(null);

  // Project directories the user has pointed at for project-scoped installs
  const availableProjects = useAppStore((state) => state.userAddedProjects);
  const addProject = useAppStore((state) => state.addProject);

  // Persist agent preferences to localStorage whenever they change
  const setSelectedAgents = useCallback((agents: AgentId[]) => {
    setSelectedAgentsState(agents);
    localStorage.setItem(AGENT_PREFS_STORAGE_KEY, JSON.stringify(agents));
  }, []);

  // Auto-select first project when switching to project scope
  const handleSetInstallScope = useCallback(
    (scope: InstallScope) => {
      setInstallScope(scope);
      if (scope === "project" && availableProjects.length > 0 && !selectedProject) {
        setSelectedProjectState(availableProjects[0]);
      }
    },
    [availableProjects, selectedProject],
  );

  const handleInstall = useCallback(async () => {
    if (selectedAgents.length === 0) {
      return;
    }

    // For project scope, require a selected project
    if (installScope === "project" && !selectedProject) {
      return;
    }

    setIsInstalling(true);
    onInstallStart(skill.name);

    try {
      // Build skill source: repo/skill-name for multi-skill repos, or just the source
      const repoSource = skill.top_source || resolvedTopSource;
      // If we have a repo source and skill name differs from repo, include both
      const skillSource = repoSource ? `${repoSource}/${skill.name}` : skill.name;

      const result = await installSkill({
        skill_source: skillSource,
        scope: installScope,
        agents: selectedAgents,
        project_path: installScope === "project" ? (selectedProject ?? undefined) : undefined,
      });
      onInstallComplete({ success: result.success, error: result.error, skillName: skill.name });
    } catch (err) {
      onInstallComplete({
        success: false,
        error: err instanceof Error ? err.message : "Unknown error",
        skillName: skill.name,
      });
    } finally {
      setIsInstalling(false);
    }
  }, [
    skill,
    resolvedTopSource,
    selectedAgents,
    installScope,
    selectedProject,
    onInstallStart,
    onInstallComplete,
  ]);

  const handleRemove = useCallback(async () => {
    setIsRemoving(true);
    try {
      const result = await removeSkill(skill.name, installScope === "global");
      if (result.success) {
        onRemoveComplete();
      }
    } finally {
      setIsRemoving(false);
      setShowRemoveConfirm(false);
    }
  }, [skill.name, installScope, onRemoveComplete]);

  const handleUpdate = useCallback(async () => {
    setIsUpdating(true);
    try {
      const result = await updateSkill(skill.name, installScope === "global");
      if (result.success) {
        onInstallComplete({ success: true, skillName: skill.name });
      } else {
        onInstallComplete({ success: false, error: result.error, skillName: skill.name });
      }
    } catch (err) {
      onInstallComplete({
        success: false,
        error: err instanceof Error ? err.message : "Unknown error",
        skillName: skill.name,
      });
    } finally {
      setIsUpdating(false);
    }
  }, [skill.name, installScope, onInstallComplete]);

  const handleBrowseProject = useCallback(async () => {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Select Project Directory",
    });
    if (selected) {
      addProject(selected);
      setSelectedProjectState(selected);
    }
  }, [addProject]);

  if (!skill.is_installed) {
    return (
      <>
        <div className="skill-detail-section">
          <h4>Install scope</h4>
          <div className="skill-detail-scope-toggle">
            <button
              type="button"
              className={`scope-option ${installScope === "global" ? "selected" : ""}`}
              onClick={() => handleSetInstallScope("global")}
            >
              Global
            </button>
            <button
              type="button"
              className={`scope-option ${installScope === "project" ? "selected" : ""}`}
              onClick={() => handleSetInstallScope("project")}
            >
              Project
            </button>
          </div>

          {/* Project selector dropdown when project scope is selected */}
          {installScope === "project" && (
            <div className="skill-detail-project-select">
              <label>Project directory</label>
              <div className="skill-detail-project-select-row">
                {availableProjects.length > 0 && (
                  <select
                    value={selectedProject || ""}
                    onChange={(e) => setSelectedProjectState(e.target.value)}
                  >
                    {availableProjects.map((p) => (
                      <option key={p} value={p}>
                        {p.split("/").pop()} – {p}
                      </option>
                    ))}
                  </select>
                )}
                <button type="button" className="skill-action-button" onClick={handleBrowseProject}>
                  <FolderPlus size={14} />
                  {availableProjects.length === 0 ? "Choose Directory" : "Add"}
                </button>
              </div>
            </div>
          )}
        </div>

        <div className="skill-detail-section">
          <AgentTargetSelector
            selectedAgents={selectedAgents}
            onChange={setSelectedAgents}
            disabled={isInstalling}
          />
        </div>

        <div className="skill-detail-actions">
          <button
            className="skill-action-button primary"
            onClick={handleInstall}
            disabled={
              isInstalling ||
              selectedAgents.length === 0 ||
              (installScope === "project" && !selectedProject)
            }
          >
            {isInstalling ? (
              <>
                <span className="spinner" />
                Installing…
              </>
            ) : (
              <>
                <Download size={16} />
                Install Skill
              </>
            )}
          </button>
        </div>
      </>
    );
  }

  return (
    <div className="skill-detail-actions">
      {skill.installed_info?.has_update && (
        <button
          className="skill-action-button primary"
          onClick={handleUpdate}
          disabled={isUpdating}
        >
          {isUpdating ? (
            <>
              <span className="spinner" />
              Updating…
            </>
          ) : (
            <>
              <RefreshCw size={16} />
              Update Skill
            </>
          )}
        </button>
      )}
      {showRemoveConfirm ? (
        <div style={{ display: "flex", gap: "8px" }}>
          <button
            className="skill-action-button danger"
            onClick={handleRemove}
            disabled={isRemoving}
            style={{ flex: 1 }}
          >
            {isRemoving ? (
              <>
                <span className="spinner" />
                Removing…
              </>
            ) : (
              "Confirm Remove"
            )}
          </button>
          <button
            className="skill-action-button"
            onClick={() => setShowRemoveConfirm(false)}
            disabled={isRemoving}
            style={{
              flex: 1,
              background: "var(--color-bg-tertiary)",
              color: "var(--color-text-secondary)",
            }}
          >
            Cancel
          </button>
        </div>
      ) : (
        <button
          className="skill-action-button danger"
          onClick={() => setShowRemoveConfirm(true)}
          disabled={isRemoving}
        >
          <Trash2 size={16} />
          Remove Skill
        </button>
      )}
    </div>
  );
}
