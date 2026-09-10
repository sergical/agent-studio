// ============================================================================
// useLocationActions - runs every `LocationAction` the Locations card's rows
// and menus can produce: toggling a switch, relinking or removing a broken
// link, reveal/open-editor, park/unpark, update, and the two dialogs
// (Convert-to-per-skill-links, Remove from scope) that need a confirm step
// first. One hook so `SkillLocationsCard`, `SkillLocationScope` and
// `SkillLocationRow` share one source of truth for what a click does.
// ============================================================================

import { useState } from "react";
import {
  agentIdFromDeploymentLabel,
  parseSkillSource,
  toWireParsedSkillSource,
} from "@skill-studio/lib";
import type { AgentId, Deployment, InstalledSkill, LifecycleTarget } from "@skill-studio/lib";
import {
  addSkill,
  openSkillPath,
  parkSkill,
  removeSkill,
  repairSkillLink,
  setDeploymentEnabled,
  setHarnessEnabled,
  setPluginEnabled,
  setSkillInvocation,
  unparkSkill,
  updateSkill,
} from "../../lib/skill-api";
import {
  lifecycleTargetForPark,
  lifecycleTargetForSkill,
  updateSkillOwners,
} from "../../lib/skill-lifecycle-target";
import { useAppStore } from "../../store/appStore";
import { canToggleHarness } from "./skill-location-helpers";
import type { LocationAction } from "./skill-location-status";

interface UseLocationActionsResult {
  run: (action: LocationAction) => void;
  isBusy: boolean;
  /** Set while a "Convert to per-skill links…" action is pending confirmation. */
  materializeRequest: MaterializeLocationRequest | null;
  closeMaterializeRequest: () => void;
  independentCopyRequest: { deployment: Deployment; scopeLabel: string } | null;
  closeIndependentCopyRequest: () => void;
  /** Set while a "Remove from <Scope>…" action is pending confirmation. */
  removeRequest: {
    scopeLabel: string;
    projectPath: string | null;
    deployment?: Deployment;
  } | null;
  closeRemoveRequest: () => void;
  /** Set while an "Uninstall the <name> plugin…" action is pending confirmation. */
  pluginUninstallRequest: Deployment | null;
  closePluginUninstallRequest: () => void;
}

export interface MaterializeLocationRequest {
  target: LifecycleTarget;
  harness: string;
  harnessLabel: string;
  root: string;
  intent: "convert-only" | "convert-then-disable";
}

/** Display label for a harness whose whole skills root can be materialized. */
function materializeHarnessLabel(harness: AgentId): string {
  switch (harness) {
    case "claude-code":
      return "Claude Code";
    case "codex":
      return "Codex";
    case "open-code":
      return "OpenCode";
    case "pi":
      return "pi";
    case "cursor":
      return "Cursor";
    case "grok-build":
      return "Grok Build";
    default:
      return harness;
  }
}

/** Routes only explicit conversion and whole-root toggle-off actions to the conversion dialog. */
export function materializeRequestForLocationAction(
  action: LocationAction,
): MaterializeLocationRequest | null {
  if (action.kind === "convert-root") {
    return {
      target: action.target,
      harness: action.harness,
      harnessLabel: materializeHarnessLabel(action.harness),
      root: action.root,
      intent: "convert-only",
    };
  }
  if (
    action.kind !== "set-enabled" ||
    action.enabled ||
    !action.deployment.shared_via_whole_dir_link
  ) {
    return null;
  }
  const { deployment } = action;
  return {
    target: { deployment_id: deployment.id },
    harness: agentIdFromDeploymentLabel(deployment.agent) ?? deployment.agent,
    harnessLabel: deployment.agent,
    root: deployment.path.slice(0, deployment.path.lastIndexOf("/")),
    intent: "convert-then-disable",
  };
}

/** Every `LocationAction` this hook runs directly, without a confirm dialog first. */
export function useLocationActions(
  skill: InstalledSkill,
  onCompareCopies?: () => void,
): UseLocationActionsResult {
  const addToast = useAppStore((state) => state.addToast);
  const [isBusy, setIsBusy] = useState(false);
  const [materializeRequest, setMaterializeRequest] = useState<MaterializeLocationRequest | null>(
    null,
  );
  const [independentCopyRequest, setIndependentCopyRequest] = useState<{
    deployment: Deployment;
    scopeLabel: string;
  } | null>(null);
  const [removeRequest, setRemoveRequest] = useState<{
    scopeLabel: string;
    projectPath: string | null;
    deployment?: Deployment;
  } | null>(null);
  const [pluginUninstallRequest, setPluginUninstallRequest] = useState<Deployment | null>(null);

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
        setMaterializeRequest(materializeRequestForLocationAction(action));
        return;
      case "make-independent-copy":
        setIndependentCopyRequest({
          deployment: action.deployment,
          scopeLabel: action.scopeLabel,
        });
        return;
      case "set-enabled": {
        const { deployment, enabled } = action;
        const readerAgent = agentIdFromDeploymentLabel(deployment.agent);
        const conversion = materializeRequestForLocationAction(action);
        if (conversion) {
          setMaterializeRequest(conversion);
          return;
        }
        runWithErrorToast(enabled ? "Couldn't enable" : "Couldn't disable", () =>
          deployment.disabled_by === "studio-moved" || !canToggleHarness(deployment)
            ? setDeploymentEnabled({ deployment_id: deployment.id }, enabled)
            : readerAgent && readerAgent !== "shared"
              ? setHarnessEnabled({ deployment_id: deployment.id }, readerAgent, enabled)
              : Promise.reject(new Error(`${deployment.agent} is not a supported reader`)),
        );
        return;
      }
      case "set-reader-enabled":
        runWithErrorToast(action.enabled ? "Couldn't enable" : "Couldn't disable", () =>
          setHarnessEnabled(action.target, action.agent, action.enabled),
        );
        return;
      case "set-plugin-enabled": {
        const { deployment, enabled } = action;
        runWithErrorToast(enabled ? "Couldn't enable plugin" : "Couldn't disable plugin", () =>
          setPluginEnabled(deployment.plugin!.id, deployment.agent, enabled),
        );
        return;
      }
      case "uninstall-plugin":
        setPluginUninstallRequest(action.deployment);
        return;
      case "promote-global": {
        const { source, agents } = action;
        runWithErrorToast("Couldn't promote to global", async () => {
          await addSkill({
            source: toWireParsedSkillSource({ kind: "local", localPath: source }),
            method: "copy",
            destination: "universal",
            agents,
            disabled_harnesses: [],
            scope: "global",
            project_path: null,
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
        runWithErrorToast("Couldn't park skill", () => parkSkill(lifecycleTargetForPark(skill)));
        return;
      case "unpark":
        runWithErrorToast("Couldn't unpark skill", () =>
          unparkSkill(lifecycleTargetForPark(skill)),
        );
        return;
      case "remove-scope":
        setRemoveRequest({ scopeLabel: action.scopeLabel, projectPath: action.projectPath });
        return;
      case "remove-deployment":
        setRemoveRequest({
          scopeLabel: action.scopeLabel,
          projectPath: action.deployment.project_path ?? null,
          deployment: action.deployment,
        });
        return;
      case "update":
        runWithErrorToast("Update failed", async () => {
          const summary = await updateSkillOwners(skill, updateSkill);
          if (summary.failures.length > 0) {
            throw new Error(
              `Updated ${summary.succeeded} of ${summary.attempted} deployments. ${summary.failures.map((failure) => failure.message).join("; ")}`,
            );
          }
        });
        return;
      case "install-again":
        runWithErrorToast("Couldn't reinstall", async () => {
          const source = parseSkillSource(skill.source);
          if ("error" in source || source.kind !== "github" || !source.repo) {
            throw new Error(`Cannot reinstall ${skill.name}: no GitHub repository is recorded.`);
          }
          await addSkill({
            source: toWireParsedSkillSource({
              ...source,
              path: source.path ?? skill.name,
              skillName: skill.name,
            }),
            method: "skills-sh",
            scope: "global",
            destination: "universal",
            agents: [],
            disabled_harnesses: [],
            project_path: null,
            trial: false,
          });
        });
        return;
      case "remove-lock-entry":
        runWithErrorToast("Couldn't remove lock entry", async () => {
          await removeSkill(lifecycleTargetForSkill(skill, "global"));
        });
        return;
    }
  };

  return {
    run,
    isBusy,
    materializeRequest,
    closeMaterializeRequest: () => setMaterializeRequest(null),
    independentCopyRequest,
    closeIndependentCopyRequest: () => setIndependentCopyRequest(null),
    removeRequest,
    closeRemoveRequest: () => setRemoveRequest(null),
    pluginUninstallRequest,
    closePluginUninstallRequest: () => setPluginUninstallRequest(null),
  };
}

// `setSkillInvocation` is used by the Invocation footer's segmented control,
// re-exported here so `SkillLocationsCard` has one import site for every
// Locations-card write call.
export { setSkillInvocation };
