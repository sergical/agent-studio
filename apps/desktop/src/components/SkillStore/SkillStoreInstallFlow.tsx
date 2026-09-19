// ============================================================================
// SkillStoreInstallFlow - scope, destination, and Universal visibility for a
// skills.sh installation
// ============================================================================

import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ProjectDirectoryField } from "./ProjectDirectoryField";
import { ScopeToggleGroup } from "./ScopeToggleGroup";
import { SkillDestinationSelector } from "./SkillDestinationSelector";
import { StoreInstallFooter } from "./StoreInstallFooter";
import { UniversalVisibilitySelector } from "./UniversalVisibilitySelector";
import {
  universalDisabledHarnesses,
  universalInstallHarnesses,
} from "./universal-install-visibility";
import { parentProgressForPhase, startStoreInstall } from "./store-install-flow";
import { useStoreTrustStep } from "./use-store-trust-step";
import {
  cancelAddSkillOperation,
  confirmAddSkillTrust,
  getAddMethodDefaults,
  getAddSkillOperation,
  invokeErrorMessage,
  onAddSkillOperation,
  registerSkillProjects,
  startAddSkillOperation,
} from "../../lib/skill-api";
import {
  applyAddSkillOperationEvent,
  listenForAddSkillOperation,
} from "../../hooks/useAddSkillOperation";
import { useAppStore } from "../../store/appStore";
import type { SkillInstallCompletion } from "./InstallControls";
import {
  addSkillFinishAction,
  shouldConsumeAddSkillOperation,
  toWireParsedSkillSource,
} from "@skill-studio/lib";
import type {
  AddSkillOperationEvent,
  AgentId,
  InstallScope,
  SkillDestination,
  SkillWithStatus,
} from "@skill-studio/lib";

const PER_HARNESS_DISABLED_REASON =
  "skills.sh installs to Universal. Use Add by source with Copy for Per harness.";
const ignoreHarnessChange = () => {};

interface SkillStoreInstallFlowProps {
  skill: SkillWithStatus;
  resolvedTopSource: string | null;
  onInstallStart: (skillName: string) => void;
  onInstallPaused: () => void;
  onInstallComplete: (result: SkillInstallCompletion) => void;
}

/** Configures and starts a skills.sh install for a skill that is not installed. */
export function SkillStoreInstallFlow({
  skill,
  resolvedTopSource,
  onInstallStart,
  onInstallPaused,
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
  const [operation, setOperation] = useState<AddSkillOperationEvent | undefined>(undefined);
  const [trustBusy, setTrustBusy] = useState(false);
  const operationIdRef = useRef<string | undefined>(undefined);
  const consumedIdRef = useRef<string | undefined>(undefined);
  const unlistenRef = useRef<(() => void) | undefined>(undefined);
  const availableProjects = useAppStore((state) => state.userAddedProjects);
  const setTrackedProjects = useAppStore((state) => state.setTrackedProjects);
  const addToast = useAppStore((state) => state.addToast);

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

  useEffect(() => {
    return () => {
      unlistenRef.current?.();
    };
  }, []);

  // Review round 2 (B1): a skills.sh repo the user hasn't trusted through
  // the Add Skill sheet used to fail this install outright
  // (`add_skill`'s `Err` on `NeedsTrust`, `skill_install.rs`). Routing
  // through the background operation instead of the plain `addSkill` call
  // surfaces the same `needs-trust` phase the sheet already shows a prompt
  // for, so the Store install can pause and retry rather than dead-end.
  //
  // Called directly from whichever event handler produced the terminal
  // event (the operation listener, or an awaited start/confirm call) -
  // not a `useEffect` keyed to `operation` state, so the parent callback
  // fires once, from the handler that owns the result, not as a reaction
  // to a render.
  const finishOperation = (finished: AddSkillOperationEvent) => {
    unlistenRef.current?.();
    unlistenRef.current = undefined;
    operationIdRef.current = undefined;
    setIsInstalling(false);
    const action = addSkillFinishAction(finished);
    if (action.kind === "error") {
      onInstallComplete({ success: false, error: action.error, skillName: skill.name });
      return;
    }
    onInstallComplete({
      success: true,
      skillName: action.openName ?? skill.name,
      warning: action.message,
    });
  };

  /** Applies an operation event to the displayed `operation` state, and finishes the
   * install the moment that event is the terminal one for the operation this component
   * is tracking - regardless of whether it arrived from the event stream or from an
   * awaited command response. */
  const applyOperationEvent = (incoming: AddSkillOperationEvent) => {
    const trackedId = operationIdRef.current;
    setOperation((current) => applyAddSkillOperationEvent(current, incoming, trackedId));
    if (incoming.operation_id !== trackedId) return;
    // Review round 3 (B1): `needs-trust` hands control to `TrustConfirmFooter` in
    // this drawer, so the parent's `InstallProgressModal` must stop showing - else
    // the trust prompt sits hidden under an endless "Installing…" spinner.
    if (parentProgressForPhase(incoming.phase) === "clear") onInstallPaused();
    if (!shouldConsumeAddSkillOperation(incoming, consumedIdRef.current)) return;
    consumedIdRef.current = incoming.operation_id;
    finishOperation(incoming);
  };

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
    if (!selected) return;
    // Installing into the folder does not depend on tracking it, so the pick
    // stands even when the saved list cannot be written.
    setSelectedProject(selected);
    try {
      setTrackedProjects(await registerSkillProjects([selected]));
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't save project folder",
        message: invokeErrorMessage(err),
      });
    }
  };

  const handleInstall = async () => {
    if (installScope === "project" && !selectedProject) return;

    const repoSource = skill.top_source || resolvedTopSource;
    if (!repoSource) {
      onInstallComplete({
        success: false,
        error: `Cannot install ${skill.name}: no GitHub repository is recorded.`,
        skillName: skill.name,
      });
      return;
    }

    setIsInstalling(true);
    onInstallStart(skill.name);
    const operationId = crypto.randomUUID();
    operationIdRef.current = operationId;
    consumedIdRef.current = undefined;
    try {
      if (unlistenRef.current) unlistenRef.current();
      const unlisten = await listenForAddSkillOperation({
        isCancelled: () => false,
        listen: onAddSkillOperation,
        onEvent: applyOperationEvent,
      });
      unlistenRef.current = unlisten;
      const settled = await startStoreInstall(
        operationId,
        {
          source: toWireParsedSkillSource({
            kind: "github",
            repo: repoSource,
            path: skill.name,
            skillName: skill.name,
          }),
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
          project_path: installScope === "project" ? (selectedProject ?? null) : null,
        },
        { start: startAddSkillOperation, getOperation: getAddSkillOperation },
      );
      applyOperationEvent(settled);
    } catch (error) {
      unlistenRef.current?.();
      unlistenRef.current = undefined;
      operationIdRef.current = undefined;
      setIsInstalling(false);
      onInstallComplete({
        success: false,
        error: error instanceof Error ? error.message : "Install failed without an error message.",
        skillName: skill.name,
      });
    }
  };

  const { handleDeclineTrust, handleTrustAndRetry } = useStoreTrustStep({
    skillName: skill.name,
    operation,
    trustBusy,
    operationIdRef,
    consumedIdRef,
    unlistenRef,
    setOperation,
    setIsInstalling,
    setTrustBusy,
    onInstallStart,
    onInstallPaused,
    onInstallComplete,
    addToast,
    applyOperationEvent,
    cancelAddSkillOperation,
    confirmAddSkillTrust,
    getAddSkillOperation,
  });

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
          <ProjectDirectoryField
            availableProjects={availableProjects}
            selectedProject={selectedProject}
            onSelectProject={setSelectedProject}
            onBrowse={() => void handleBrowseProject()}
          />
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

      <StoreInstallFooter
        operation={operation}
        trustBusy={trustBusy}
        isInstalling={isInstalling}
        installDisabled={isInstalling || (installScope === "project" && !selectedProject)}
        onDeclineTrust={() => void handleDeclineTrust()}
        onTrustAndRetry={() => void handleTrustAndRetry()}
        onInstall={() => void handleInstall()}
      />
    </>
  );
}
