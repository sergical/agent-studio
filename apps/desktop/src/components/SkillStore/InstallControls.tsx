// ============================================================================
// InstallControls - Agent selector, scope, project picker, and
// install/remove/update actions for a skill
// ============================================================================

import { useEffect, useState } from "react";
import { Download, Trash2, RefreshCw, FolderPlus } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  Button,
} from "@skill-studio/ui";
import {
  AgentTargetSelector,
  installDisabledHarnesses,
  installTargetAgents,
} from "./AgentTargetSelector";
import { ProjectDirectorySelect } from "./ProjectDirectorySelect";
import { ScopeToggleGroup } from "./ScopeToggleGroup";
import { getAddMethodDefaults, installSkill, removeSkill, updateSkill } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import type { AgentId, InstallScope, SkillWithStatus } from "@skill-studio/lib";

const ACTION_BUTTON_CLASS =
  "h-(--control-height) w-full justify-center gap-2 rounded-md px-3.5 text-body font-medium";

interface InstallControlsProps {
  skill: SkillWithStatus;
  resolvedTopSource: string | null;
  onInstallStart: (skillName: string) => void;
  onInstallComplete: (result: { success: boolean; error?: string; skillName?: string }) => void;
  onRemoveComplete: (result: { success: boolean; error?: string }) => void;
}

export function InstallControls({
  skill,
  resolvedTopSource,
  onInstallStart,
  onInstallComplete,
  onRemoveComplete,
}: InstallControlsProps) {
  const [readers, setReaders] = useState<AgentId[]>([]);
  const [enabledReaders, setEnabledReaders] = useState<AgentId[]>([]);
  const [claudeReadsShared, setClaudeReadsShared] = useState(true);
  const [claudeLink, setClaudeLink] = useState(true);
  const [installScope, setInstallScope] = useState<InstallScope>("global");
  const [isInstalling, setIsInstalling] = useState(false);
  const [isRemoving, setIsRemoving] = useState(false);
  const [isUpdating, setIsUpdating] = useState(false);
  const [showRemoveConfirm, setShowRemoveConfirm] = useState(false);
  const [selectedProject, setSelectedProject] = useState<string | null>(null);

  // Project directories the user has pointed at for project-scoped installs
  const availableProjects = useAppStore((state) => state.userAddedProjects);
  const addProject = useAppStore((state) => state.addProject);

  useEffect(() => {
    let cancelled = false;
    getAddMethodDefaults()
      .then((defaults) => {
        if (cancelled) return;
        const installedReaders = defaults.installed_harnesses.filter((id) => id !== "claude-code");
        setReaders(installedReaders);
        setEnabledReaders(installedReaders);
        setClaudeReadsShared(defaults.claude_reads_shared_folder);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  const handleReaderEnabledChange = (agent: AgentId, enabled: boolean) => {
    setEnabledReaders((current) =>
      enabled
        ? readers.filter((id) => id === agent || current.includes(id))
        : current.filter((id) => id !== agent),
    );
  };

  // Auto-select first project when switching to project scope
  const handleSetInstallScope = (scope: InstallScope) => {
    setInstallScope(scope);
    if (scope === "project" && availableProjects.length > 0 && !selectedProject) {
      setSelectedProject(availableProjects[0]);
    }
  };

  const handleInstall = () => {
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
      agents: installTargetAgents(enabledReaders, claudeLink),
      disabled_harnesses: installDisabledHarnesses(
        readers,
        enabledReaders,
        claudeReadsShared,
        claudeLink,
      ),
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
    // Surface both the resolved `{ success, error }` result and a thrown
    // rejection to `onRemoveComplete`, mirroring `handleInstall`: an in-repo
    // skill resolves `success: false` with an explanatory `error` (it isn't
    // tracked by skills.sh), which must reach the user as a toast rather
    // than being silently swallowed.
    return removeSkill(skill.name, installScope === "global" ? null : selectedProject)
      .then((result) => {
        onRemoveComplete({ success: result.success, error: result.error });
      })
      .catch((err) => {
        onRemoveComplete({
          success: false,
          error: err instanceof Error ? err.message : "Unknown error",
        });
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
          <ScopeToggleGroup scope={installScope} onScopeChange={handleSetInstallScope} />

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
                  <div className="flex-1">
                    <ProjectDirectorySelect
                      projects={availableProjects}
                      value={selectedProject ?? undefined}
                      onChange={setSelectedProject}
                    />
                  </div>
                )}
                <Button
                  variant="outline"
                  className={ACTION_BUTTON_CLASS}
                  onClick={handleBrowseProject}
                >
                  <FolderPlus size={14} />
                  {availableProjects.length === 0 ? "Choose directory" : "Add"}
                </Button>
              </div>
            </div>
          )}
        </div>

        <div className="p-5">
          <AgentTargetSelector
            readers={readers}
            enabledReaders={enabledReaders}
            onReaderEnabledChange={handleReaderEnabledChange}
            claudeReadsShared={claudeReadsShared}
            claudeLink={claudeLink}
            onClaudeLinkChange={setClaudeLink}
            scope={installScope}
            disabled={isInstalling}
          />
        </div>

        <div className="mt-auto flex flex-col gap-2 p-5">
          <Button
            className={`${ACTION_BUTTON_CLASS} bg-accent text-text-on-accent hover:bg-accent-hover`}
            onClick={handleInstall}
            disabled={isInstalling || (installScope === "project" && !selectedProject)}
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
      <Button
        className={`${ACTION_BUTTON_CLASS} bg-error-soft text-error hover:bg-error hover:text-white`}
        onClick={() => setShowRemoveConfirm(true)}
        disabled={isRemoving}
      >
        <Trash2 size={16} />
        Remove Skill
      </Button>

      <AlertDialog open={showRemoveConfirm} onOpenChange={setShowRemoveConfirm}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove {skill.name}?</AlertDialogTitle>
            <AlertDialogDescription>This cannot be undone.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={isRemoving}>Cancel</AlertDialogCancel>
            <AlertDialogAction variant="destructive" onClick={handleRemove} disabled={isRemoving}>
              {isRemoving ? "Removing…" : "Confirm Remove"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
