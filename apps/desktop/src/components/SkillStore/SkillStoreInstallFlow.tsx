// ============================================================================
// SkillStoreInstallFlow - scope, destination, and Universal visibility for a
// skills.sh installation
// ============================================================================

import { useEffect, useState } from "react";
import { Download, FolderPlus } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { Button } from "@skill-studio/ui";
import { ProjectDirectorySelect } from "./ProjectDirectorySelect";
import { ScopeToggleGroup } from "./ScopeToggleGroup";
import { SkillDestinationSelector } from "./SkillDestinationSelector";
import { UniversalVisibilitySelector } from "./UniversalVisibilitySelector";
import {
  universalDisabledHarnesses,
  universalInstallHarnesses,
} from "./universal-install-visibility";
import { addSkill, getAddMethodDefaults } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import type { SkillInstallCompletion } from "./InstallControls";
import type { AgentId, InstallScope, SkillDestination, SkillWithStatus } from "@skill-studio/lib";

const ACTION_BUTTON_CLASS =
  "h-(--control-height) w-full justify-center gap-2 rounded-md px-3.5 text-body font-medium";
const PER_HARNESS_DISABLED_REASON =
  "skills.sh installs to Universal. Use Add by source with Copy for Per harness.";
const ignoreHarnessChange = () => {};

interface SkillStoreInstallFlowProps {
  skill: SkillWithStatus;
  resolvedTopSource: string | null;
  onInstallStart: (skillName: string) => void;
  onInstallComplete: (result: SkillInstallCompletion) => void;
}

/** Configures and starts a not-installed skills.sh skill deployment. */
export function SkillStoreInstallFlow({
  skill,
  resolvedTopSource,
  onInstallStart,
  onInstallComplete,
}: SkillStoreInstallFlowProps) {
  const [readers, setReaders] = useState<AgentId[]>([]);
  const [enabledReaders, setEnabledReaders] = useState<AgentId[]>([]);
  const [claudeReadsUniversal, setClaudeReadsUniversal] = useState(true);
  const [claudeLink, setClaudeLink] = useState(true);
  const [destination, setDestination] = useState<SkillDestination>("universal");
  const [installScope, setInstallScope] = useState<InstallScope>("global");
  const [selectedProject, setSelectedProject] = useState<string | null>(null);
  const [isInstalling, setIsInstalling] = useState(false);
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
        setClaudeReadsUniversal(defaults.claude_reads_shared_folder);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  const handleReaderEnabledChange = (agent: AgentId, enabled: boolean) => {
    setEnabledReaders((current) => {
      if (!enabled) return current.filter((id) => id !== agent);
      const enabledReaderSet = new Set(current);
      return readers.filter((id) => id === agent || enabledReaderSet.has(id));
    });
  };

  const handleInstallScopeChange = (scope: InstallScope) => {
    setInstallScope(scope);
    if (scope === "project" && availableProjects.length > 0 && !selectedProject) {
      setSelectedProject(availableProjects[0]);
    }
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

  const handleInstall = () => {
    if (installScope === "project" && !selectedProject) return;

    setIsInstalling(true);
    onInstallStart(skill.name);
    const repoSource = skill.top_source || resolvedTopSource;
    if (!repoSource) {
      onInstallComplete({
        success: false,
        error: `No GitHub repository is available for ${skill.name}`,
        skillName: skill.name,
      });
      setIsInstalling(false);
      return;
    }

    addSkill({
      source: {
        kind: "github",
        repo: repoSource,
        path: skill.name,
        skillName: skill.name,
      },
      method: "skills-sh",
      scope: installScope,
      destination,
      agents: universalInstallHarnesses(enabledReaders, claudeLink),
      disabled_harnesses: universalDisabledHarnesses(
        readers,
        enabledReaders,
        claudeReadsUniversal,
        claudeLink,
      ),
      project_path: installScope === "project" ? (selectedProject ?? undefined) : undefined,
      trial: false,
    })
      .then((result) => {
        onInstallComplete({ success: true, skillName: result.name, warning: result.warning });
      })
      .catch((error) => {
        onInstallComplete({
          success: false,
          error: error instanceof Error ? error.message : "Unknown error",
          skillName: skill.name,
        });
      })
      .finally(() => {
        setIsInstalling(false);
      });
  };

  return (
    <>
      <div className="p-5">
        <h4 className="m-0 mb-3 text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
          Scope
        </h4>
        <ScopeToggleGroup scope={installScope} onScopeChange={handleInstallScopeChange} />
        <div className="mt-3">
          <SkillDestinationSelector
            destination={destination}
            harnesses={[]}
            scope={installScope}
            onDestinationChange={setDestination}
            onHarnessChange={ignoreHarnessChange}
            disabled={isInstalling}
            perHarnessDisabledReason={PER_HARNESS_DISABLED_REASON}
          />
        </div>

        {installScope === "project" && (
          <div className="mt-3">
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
        <UniversalVisibilitySelector
          readers={readers}
          enabledReaders={enabledReaders}
          onReaderEnabledChange={handleReaderEnabledChange}
          claudeReadsShared={claudeReadsUniversal}
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
