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
import type {
  AgentId,
  Deployment,
  ForkRecord,
  InstalledSkill,
  InvocationPolicy,
  LifecycleTarget,
} from "@skill-studio/lib";
import {
  addSkill,
  forkSkill,
  openSkillPath,
  parkSkill,
  removeSkill,
  repairSkillLink,
  restoreMovedDeployment,
  setHarnessEnabled,
  setPluginEnabled,
  setSkillsInvocation,
  unparkSkill,
} from "../../lib/skill-api";
import {
  lifecycleTargetForDeployment,
  lifecycleTargetForPark,
  lifecycleTargetForSkill,
} from "../../lib/skill-lifecycle-target";
import { useAppStore } from "../../store/appStore";
import { canOfferHarnessSwitch } from "./skill-location-helpers";
import { claudeCodeSwitchToast } from "./skill-claude-switch-note";
import { hasUpstreamOwner } from "./skill-location-status";
import type { InvocationFile, LocationAction } from "./skill-location-status";

interface UseLocationActionsResult {
  /** Resolves `true` when the action succeeded or only opened a dialog, `false` after its error toast. */
  run: (action: LocationAction) => Promise<boolean>;
  isBusy: boolean;
  /** The kind of every action still running, for a control that shows its own pending state. */
  busyKinds: LocationAction["kind"][];
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
  /** Set while a "Split into harness folders…" action is pending confirmation. */
  splitRequest: SplitLocationRequest | null;
  closeSplitRequest: () => void;
}

type SplitLocationRequest = Omit<Extract<LocationAction, { kind: "split" }>, "kind">;

interface MaterializeLocationRequest {
  target: LifecycleTarget;
  harness: string;
  harnessLabel: string;
  root: string;
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

/**
 * Routes the explicit "Convert to per-skill links…" action to the conversion
 * dialog. The Enabled switch never does: every harness switch writes its own
 * setting and leaves a whole-folder link in place.
 */
export function materializeRequestForLocationAction(
  action: LocationAction,
): MaterializeLocationRequest | null {
  if (action.kind !== "convert-root") return null;
  return {
    target: action.target,
    harness: action.harness,
    harnessLabel: materializeHarnessLabel(action.harness),
    root: action.root,
  };
}

/** Every `LocationAction` this hook runs directly, without a confirm dialog first. */
export function useLocationActions(
  skill: InstalledSkill,
  onCompareCopies?: () => void,
): UseLocationActionsResult {
  const addToast = useAppStore((state) => state.addToast);
  const [busyKinds, setBusyKinds] = useState<LocationAction["kind"][]>([]);
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
  const [splitRequest, setSplitRequest] = useState<SplitLocationRequest | null>(null);

  /** Resolves `true` when `fn` succeeded, `false` after showing its error toast. Never rejects. */
  const runWithErrorToast = (
    title: string,
    fn: () => Promise<void>,
    onSuccess?: () => void,
  ): Promise<boolean> =>
    fn().then(
      () => {
        onSuccess?.();
        return true;
      },
      (err) => {
        addToast({
          type: "error",
          title,
          message: err instanceof Error ? err.message : "Unknown error",
        });
        return false;
      },
    );

  const dispatch = (action: LocationAction): Promise<boolean> => {
    switch (action.kind) {
      case "relink":
        return runWithErrorToast("Couldn't relink", () =>
          repairSkillLink(action.deployment.path, "relink"),
        );
      case "remove-link":
        return runWithErrorToast("Couldn't remove link", () =>
          repairSkillLink(action.deployment.path, "remove"),
        );
      case "edit-skill-md":
      case "open-editor":
        return runWithErrorToast("Couldn't open in your editor", () =>
          openSkillPath(action.path, "editor"),
        );
      case "reveal":
        return runWithErrorToast("Couldn't reveal in Finder", () =>
          openSkillPath(action.path, "reveal"),
        );
      case "compare":
        onCompareCopies?.();
        return Promise.resolve(true);
      case "convert-root":
        setMaterializeRequest(materializeRequestForLocationAction(action));
        return Promise.resolve(true);
      case "make-independent-copy":
        setIndependentCopyRequest({
          deployment: action.deployment,
          scopeLabel: action.scopeLabel,
        });
        return Promise.resolve(true);
      case "set-enabled": {
        const { deployment, enabled } = action;
        const readerAgent = agentIdFromDeploymentLabel(deployment.agent);
        // `park` is the off switch only for the Global Universal deployment - never this row's
        // (see `canOfferHarnessSwitch`). Both the rail and the Locations card disable the
        // control for any row that fails this check, so the rejection below is a
        // defense-in-depth backstop, not the normal path.
        return runWithErrorToast(
          enabled ? "Couldn't enable" : "Couldn't disable",
          () =>
            deployment.disabled_by === "studio-moved"
              ? restoreMovedDeployment({ deployment_id: deployment.id })
              : readerAgent && readerAgent !== "shared" && canOfferHarnessSwitch(deployment)
                ? setHarnessEnabled({ deployment_id: deployment.id }, readerAgent, enabled)
                : Promise.reject(new Error("This copy has no off switch")),
          () => {
            if (readerAgent === "claude-code") addToast(claudeCodeSwitchToast(skill, enabled));
          },
        );
      }
      case "set-reader-enabled":
        return runWithErrorToast(
          action.enabled ? "Couldn't enable" : "Couldn't disable",
          () => setHarnessEnabled(action.target, action.agent, action.enabled),
          () => {
            if (action.agent === "claude-code") {
              addToast(claudeCodeSwitchToast(skill, action.enabled));
            }
          },
        );
      case "set-plugin-enabled": {
        const { deployment, enabled } = action;
        return runWithErrorToast(
          enabled ? "Couldn't enable plugin" : "Couldn't disable plugin",
          () => setPluginEnabled(deployment.plugin!.id, deployment.agent, enabled),
        );
      }
      case "uninstall-plugin":
        setPluginUninstallRequest(action.deployment);
        return Promise.resolve(true);
      case "promote-global": {
        const { source, agents } = action;
        return runWithErrorToast("Couldn't promote to global", async () => {
          await addSkill({
            source: toWireParsedSkillSource({ kind: "local", localPath: source }),
            method: "copy",
            destination: "universal",
            agents,
            disabled_harnesses: [],
            link_mode: "link",
            scope: "global",
            project_path: null,
          });
          addToast({
            type: "success",
            title: `${skill.name} is global now`,
            message: "Copied to ~/.agents/skills. Every project reads it from there.",
          });
        });
      }
      case "park":
        return runWithErrorToast("Couldn't park skill", () =>
          parkSkill(lifecycleTargetForPark(skill)),
        );
      case "unpark":
        return runWithErrorToast("Couldn't unpark skill", () =>
          unparkSkill(lifecycleTargetForPark(skill)),
        );
      case "split":
        setSplitRequest({
          target: action.target,
          projectPath: action.projectPath,
          readers: action.readers,
        });
        return Promise.resolve(true);
      case "remove-scope":
        setRemoveRequest({ scopeLabel: action.scopeLabel, projectPath: action.projectPath });
        return Promise.resolve(true);
      case "remove-deployment":
        setRemoveRequest({
          scopeLabel: action.scopeLabel,
          projectPath: action.deployment.project_path ?? null,
          deployment: action.deployment,
        });
        return Promise.resolve(true);
      case "install-again":
        return runWithErrorToast("Couldn't reinstall", async () => {
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
            link_mode: "link",
            project_path: null,
          });
        });
      case "remove-lock-entry":
        return runWithErrorToast("Couldn't remove lock entry", async () => {
          await removeSkill(lifecycleTargetForSkill(skill, "global"));
        });
    }
  };

  const run = (action: LocationAction): Promise<boolean> => {
    setBusyKinds((kinds) => [...kinds, action.kind]);
    return dispatch(action).finally(() =>
      setBusyKinds((kinds) => {
        const index = kinds.indexOf(action.kind);
        return kinds.filter((_, i) => i !== index);
      }),
    );
  };

  return {
    run,
    isBusy: busyKinds.length > 0,
    busyKinds,
    materializeRequest,
    closeMaterializeRequest: () => setMaterializeRequest(null),
    independentCopyRequest,
    closeIndependentCopyRequest: () => setIndependentCopyRequest(null),
    removeRequest,
    closeRemoveRequest: () => setRemoveRequest(null),
    pluginUninstallRequest,
    closePluginUninstallRequest: () => setPluginUninstallRequest(null),
    splitRequest,
    closeSplitRequest: () => setSplitRequest(null),
  };
}

/**
 * Sets every file in `files` to `policy`, forking first when needed - the same rule the SKILL.md
 * editor uses: only the global Universal folder can need a fork before editing (`fileEditability`
 * keeps managed Project folders and copies out of this branch). The forks run one at a time, then
 * one backend call writes every file, so the skill list refreshes once and nothing is still
 * writing when this returns. Throws with how many files changed and the first failure. Used by `SkillInvocationFooter`.
 */
export async function setInvocationForFiles(
  skill: InstalledSkill,
  files: InvocationFile[],
  policy: InvocationPolicy,
): Promise<void> {
  for (const file of files) {
    // react-doctor-disable-next-line react-doctor/async-await-in-loop -- each fork takes an exclusive lease, so the forks must not overlap
    await forkBeforeInvocationEdit(file);
  }
  const results = await setSkillsInvocation(
    files.map((file) => ({ name: skill.name, path: `${file.path}/SKILL.md` })),
    policy,
  );
  // A missing result counts as a failure, the same as the list's bulk Invocation action.
  const failures = files.flatMap((file, index) => {
    const error = results[index] ? results[index].error : "The batch returned no result.";
    return error === null ? [] : [{ path: file.path, error }];
  });
  if (failures.length === 0) return;
  const [first] = failures;
  if (files.length === 1) throw new Error(first.error);
  const changed = files.length - failures.length;
  throw new Error(`Changed ${changed} of ${files.length} files. ${first.path}: ${first.error}`);
}

/** Forks a shared folder an update would write over, so the edit stays. Ambiguous and manual folders have no upstream, so they are edited in place. */
export async function forkBeforeInvocationEdit(
  file: InvocationFile,
  fork: (target: LifecycleTarget) => Promise<ForkRecord | void> = forkSkill,
): Promise<void> {
  if (file.kind === "shared" && hasUpstreamOwner(file.deployment)) {
    await fork(lifecycleTargetForDeployment(file.deployment));
  }
}
