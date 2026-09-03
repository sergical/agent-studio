// ============================================================================
// useLocationActions - runs every `LocationAction` the Locations card's rows
// and menus can produce: toggling a switch, relinking or removing a broken
// link, reveal/open-editor, park/unpark, update, and the two dialogs
// (Convert-to-per-skill-links, Remove from scope) that need a confirm step
// first. One hook so `SkillLocationsCard`, `SkillLocationScope` and
// `SkillLocationRow` share one source of truth for what a click does.
// ============================================================================

import { useState } from "react";
import { agentIdFromDeploymentLabel, COMMON_AGENTS } from "@skill-studio/lib";
import type { InstalledSkill } from "@skill-studio/lib";
import {
  addSkill,
  installSkill,
  openSkillPath,
  parkSkill,
  removeSkill,
  repairSkillLink,
  setDeploymentEnabled,
  setHarnessEnabled,
  setSkillInvocation,
  unparkSkill,
  updateSkill,
} from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import { canToggleHarness } from "./skill-location-helpers";
import type { LocationAction } from "./skill-location-status";

interface UseLocationActionsResult {
  run: (action: LocationAction) => void;
  isBusy: boolean;
  /** Set while a "Convert to per-skill links…" action is pending confirmation. */
  materializeRequest: { harness: string; harnessLabel: string; root: string } | null;
  closeMaterializeRequest: () => void;
  /** Set while a "Remove from <Scope>…" action is pending confirmation. */
  removeRequest: { scopeLabel: string; projectPath: string | null } | null;
  closeRemoveRequest: () => void;
}

/** Every `LocationAction` this hook runs directly, without a confirm dialog first. */
export function useLocationActions(
  skill: InstalledSkill,
  onCompareCopies?: () => void,
): UseLocationActionsResult {
  const addToast = useAppStore((state) => state.addToast);
  const [isBusy, setIsBusy] = useState(false);
  const [materializeRequest, setMaterializeRequest] = useState<{
    harness: string;
    harnessLabel: string;
    root: string;
  } | null>(null);
  const [removeRequest, setRemoveRequest] = useState<{
    scopeLabel: string;
    projectPath: string | null;
  } | null>(null);

  const runWithErrorToast = (title: string, fn: () => Promise<void>) => {
    setIsBusy(true);
    fn()
      .catch((err) => {
        addToast({
          type: "error",
          title,
          message: err instanceof Error ? err.message : "Unknown error",
        });
      })
      .finally(() => setIsBusy(false));
  };

  const run = (action: LocationAction) => {
    switch (action.kind) {
      case "relink":
        runWithErrorToast("Couldn't relink", () =>
          repairSkillLink(action.deployment.path, "relink"),
        );
        return;
      case "remove-link":
        runWithErrorToast("Couldn't remove link", () =>
          repairSkillLink(action.deployment.path, "remove"),
        );
        return;
      case "edit-skill-md":
      case "open-editor":
        runWithErrorToast("Couldn't open in your editor", () =>
          openSkillPath(action.path, "editor"),
        );
        return;
      case "reveal":
        runWithErrorToast("Couldn't reveal in Finder", () => openSkillPath(action.path, "reveal"));
        return;
      case "compare":
        onCompareCopies?.();
        return;
      case "convert-root":
        setMaterializeRequest({
          harness: action.harness,
          harnessLabel: action.harness,
          root: action.root,
        });
        return;
      case "set-enabled": {
        const { deployment, enabled } = action;
        if (deployment.shared_via_whole_dir_link) {
          setMaterializeRequest({
            harness: agentIdFromDeploymentLabel(deployment.agent) ?? deployment.agent,
            harnessLabel: deployment.agent,
            root: deployment.path.slice(0, deployment.path.lastIndexOf("/")),
          });
          return;
        }
        runWithErrorToast(enabled ? "Couldn't enable" : "Couldn't disable", () =>
          deployment.disabled_by === "studio-moved" || !canToggleHarness(deployment)
            ? setDeploymentEnabled(skill.name, deployment.path, enabled)
            : setHarnessEnabled(
                skill.name,
                agentIdFromDeploymentLabel(deployment.agent) ?? "",
                enabled,
              ),
        );
        return;
      }
      case "set-reader-enabled":
        runWithErrorToast(action.enabled ? "Couldn't enable" : "Couldn't disable", () =>
          setHarnessEnabled(skill.name, action.agent, action.enabled),
        );
        return;
      case "promote-global": {
        const { source, agents } = action;
        runWithErrorToast("Couldn't promote to global", async () => {
          await addSkill({
            source: { kind: "local", localPath: source },
            method: "copy",
            agents,
            disabled_harnesses: [],
            scope: "global",
            trial: false,
          });
          addToast({
            type: "success",
            title: `${skill.name} is global now`,
            message: "Copied to ~/.agents/skills. Every project reads it from there.",
          });
        });
        return;
      }
      case "park":
        runWithErrorToast("Couldn't park skill", () => parkSkill(skill.name));
        return;
      case "unpark":
        runWithErrorToast("Couldn't unpark skill", () => unparkSkill(skill.name));
        return;
      case "remove-scope":
        setRemoveRequest({ scopeLabel: action.scopeLabel, projectPath: action.projectPath });
        return;
      case "update":
        runWithErrorToast("Update failed", async () => {
          await updateSkill(skill.name, true);
        });
        return;
      case "install-again":
        // A lock-only entry has no deployment to say where it used to live,
        // so "Install again" reinstalls to every first-class agent globally
        // - the same default `AddSkillSheet` offers a fresh install.
        runWithErrorToast("Couldn't reinstall", async () => {
          await installSkill({
            skill_source: skill.source,
            scope: "global",
            agents: COMMON_AGENTS,
            disabled_harnesses: [],
          });
        });
        return;
      case "remove-lock-entry":
        runWithErrorToast("Couldn't remove lock entry", async () => {
          await removeSkill(skill.name, null);
        });
        return;
    }
  };

  return {
    run,
    isBusy,
    materializeRequest,
    closeMaterializeRequest: () => setMaterializeRequest(null),
    removeRequest,
    closeRemoveRequest: () => setRemoveRequest(null),
  };
}

// `setSkillInvocation` is used by the Invocation footer's segmented control,
// re-exported here so `SkillLocationsCard` has one import site for every
// Locations-card write call.
export { setSkillInvocation };
