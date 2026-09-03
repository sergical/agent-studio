// ============================================================================
// SkillAssistantPanel - Right-column card on the skill page: a harness
// picker, an "Ask about this skill" box that runs the local harness in a
// scratch folder containing only this skill, an "Audit" action that reviews
// SKILL.md and proposes a per-hunk rewrite via `SkillProposedEdits`, and a
// "Test" action that runs the skill against a scratch/worktree/in-place
// target, judges the result, and (for worktree/in-place) surfaces the diff
// ============================================================================

import { Suspense, lazy, useEffect, useReducer, useRef, useState } from "react";
import type { RefObject } from "react";
import { Button, Textarea } from "@skill-studio/ui";
import type { SkillAgentRunState } from "../../hooks/useSkillAgentRun";
import { useSkillAgentRun } from "../../hooks/useSkillAgentRun";
import { useSkillSnapshot } from "../../hooks/useSkillSnapshot";
import { createSkillScratchDir, removeSkillScratchDir } from "../../lib/skill-agent-api";
import {
  buildSkillAuditPrompt,
  buildSkillJudgePrompt,
  extractProposedSkillMd,
  normalizeProposalToOriginal,
  parseJudgeVerdict,
} from "@skill-studio/lib";
import type { HarnessId } from "@skill-studio/lib";
import { skillVisibleToAgent } from "@skill-studio/lib";
import type { SkillMdHunk } from "@skill-studio/lib";
import { diffSkillMd } from "@skill-studio/lib";
import { ownDeployments, ownSkillsView } from "@skill-studio/lib";
import { recordSkillRun } from "../../lib/skill-run-history-api";
import type { SkillRunAction, SkillRunJudge, SkillRunRecord } from "@skill-studio/lib";
import {
  applySkillRunTargetDiff,
  discardSkillRunTarget,
  prepareSkillRunTarget,
  revealSkillRunTarget,
  skillRunTargetDiff,
} from "../../lib/skill-run-target-api";
import type { SkillRunTargetInfo } from "@skill-studio/lib";
import { COMMON_AGENTS } from "@skill-studio/lib";
import type { AgentId, InstalledSkill, Toast } from "@skill-studio/lib";
import { useAppStore } from "../../store/appStore";
import { HarnessIcon } from "../ui/HarnessIcon";
import { HARNESS_LABELS } from "../../lib/harness-labels";
import type { SelectControlItem } from "../ui/SelectControl";
import { SelectControl } from "../ui/SelectControl";
import { SkillAgentTranscript } from "./SkillAgentTranscript";
import { SkillRunHistory } from "./SkillRunHistory";
import type { SkillTestRunParams } from "./SkillTestForm";
import { SkillTestForm } from "./SkillTestForm";

/**
 * `HARNESS_LABELS` as `SelectControl` items, for the assistant panel's
 * harness picker. A run doesn't use the installed copy - it copies the skill
 * into a fresh scratch folder (see `sourceFolderPath`) and runs there - so a
 * harness with no deployment for `skill` still runs normally. The label
 * still says so, since `visibleAgentsFor` is useful context, but the item
 * stays selectable: disabling it would leave a dead end when no harness sees
 * the skill (`defaultHarness` falls back to Claude Code either way).
 */
function harnessSelectItems(visibleAgents: readonly AgentId[]): SelectControlItem[] {
  return HARNESS_LABELS.map(([value, label]) => ({
    value,
    label: visibleAgents.includes(value) ? label : `${label} (doesn't see this skill)`,
  }));
}

interface SkillAssistantPanelProps {
  skill: InstalledSkill;
  /** The skill's SKILL.md, as loaded by `SkillPage` - null while loading or unavailable. */
  rawContent: string | null;
  skillMdPath: string | undefined;
  /** Whether the deployment `SkillPage` is showing is managed by a harness plugin - Audit needs a file it can write back to. */
  isPluginManaged: boolean;
  /** Called with the new file content once a proposal is applied, so `SkillPage`'s own copy stays in sync. */
  onApplied: (content: string) => void;
  /** Called when Apply is refused because the file drifted on disk, so `SkillPage` re-reads it. */
  onDiskChanged: () => void;
  /** Shows the "Runs" history list in place of the panel's normal content. */
  showHistory: boolean;
  onCloseHistory: () => void;
}

const MAX_TEXTAREA_ROWS = 8;

/** One audit run's outcome: the file it reviewed, the hunks proposed against it, and the deployment it was made for. */
interface Proposal {
  fileAtAuditStart: string;
  hunks: SkillMdHunk[];
  skillMdPath: string;
}

type TestPhase = "idle" | "preparing" | "running" | "judging" | "done";

/**
 * Everything one Ask/Audit/Test run produces, minus `harness` and `prompt`
 * (plain `useState`, since neither transitions alongside this cluster) -
 * `proposal`, `runTarget`, `runDiff`, `judgeVerdict`, and `testPhase` are all
 * cleared together on every transition (skill change, harness change, New
 * session), so a `useReducer` replaces what used to be five separate
 * `useState` calls all reset by the same call sites.
 */
interface RunSessionState {
  runKind: "ask" | "audit" | "test";
  proposal: Proposal | null;
  runTarget: SkillRunTargetInfo | null;
  runDiff: string | null;
  isDiffBusy: boolean;
  testPhase: TestPhase;
  judgeVerdict: ReturnType<typeof parseJudgeVerdict>;
}

function initialRunSessionState(): RunSessionState {
  return {
    runKind: "ask",
    proposal: null,
    runTarget: null,
    runDiff: null,
    isDiffBusy: false,
    testPhase: "idle",
    judgeVerdict: null,
  };
}

type RunSessionAction =
  | { type: "reset" }
  | { type: "start_audit" }
  | { type: "set_proposal"; proposal: Proposal | null }
  | { type: "start_test" }
  | { type: "set_target"; target: SkillRunTargetInfo | null }
  | { type: "set_phase"; phase: TestPhase }
  | { type: "set_diff"; diff: string | null }
  | { type: "set_diff_busy"; busy: boolean }
  | { type: "set_verdict"; verdict: ReturnType<typeof parseJudgeVerdict> }
  | { type: "set_run_kind"; kind: RunSessionState["runKind"] }
  | { type: "clear_diff" };

function runSessionReducer(state: RunSessionState, action: RunSessionAction): RunSessionState {
  switch (action.type) {
    case "reset":
      return {
        ...state,
        proposal: null,
        runTarget: null,
        runDiff: null,
        judgeVerdict: null,
        testPhase: "idle",
      };
    case "start_audit":
      return { ...state, runKind: "audit", proposal: null };
    case "set_proposal":
      return { ...state, proposal: action.proposal };
    case "start_test":
      return {
        ...state,
        runKind: "test",
        proposal: null,
        runDiff: null,
        runTarget: null,
        judgeVerdict: null,
        testPhase: "preparing",
      };
    case "set_target":
      return { ...state, runTarget: action.target };
    case "set_phase":
      return { ...state, testPhase: action.phase };
    case "set_diff":
      return { ...state, runDiff: action.diff };
    case "set_diff_busy":
      return { ...state, isDiffBusy: action.busy };
    case "set_verdict":
      return { ...state, judgeVerdict: action.verdict };
    case "set_run_kind":
      return { ...state, runKind: action.kind };
    case "clear_diff":
      return { ...state, runDiff: null, runTarget: null };
  }
}

/** Every first-class agent that can actually see `skill`, in `COMMON_AGENTS` order. */
function visibleAgentsFor(skill: InstalledSkill): AgentId[] {
  return COMMON_AGENTS.filter((agent) => skillVisibleToAgent(skill, agent) !== "none");
}

/** `skill`'s own skill folder, for the scratch dir - a plugin-only skill has no folder to copy. */
function sourceFolderPath(skill: InstalledSkill): string | undefined {
  return ownDeployments(skill)[0]?.path ?? skill.deployments[0]?.path;
}

/** The basename of a project path, for the "Changes in {name}" diff heading. */
function projectBasename(path: string): string {
  return path.split("/").filter(Boolean).pop() ?? path;
}

/** Builds the shared fields of a finished/errored run's history record - only
 * `action`, `targetKind`, and `judge` differ between Ask, Audit, and Test. */
function buildRunRecord(
  state: SkillAgentRunState,
  harness: HarnessId,
  skillName: string,
  action: SkillRunAction,
  targetKind: SkillRunTargetInfo["kind"] | undefined,
  judge: SkillRunJudge | undefined,
): SkillRunRecord {
  return {
    id: state.runId!,
    skill_name: skillName,
    harness,
    action,
    target_kind: targetKind,
    started_at: new Date().toISOString(),
    duration_ms: state.durationMs ?? 0,
    ok: state.status === "finished",
    skill_loaded: state.skillLoaded ?? "unknown",
    judge,
    cost_usd: state.costUsd,
    final_text: state.finalText ?? "",
    transcript_path: `${state.runId}.events.jsonl`,
  };
}

interface UseAssistantRunSessionParams {
  skill: InstalledSkill;
  skillMdPath: string | undefined;
  defaultHarness: AgentId;
  cancel: () => Promise<void>;
  judgeCancel: () => Promise<void>;
  reset: () => Promise<void>;
  judgeReset: () => Promise<void>;
  addToast: (toast: Omit<Toast, "id">) => string;
}

/**
 * Owns everything that gets torn down and rebuilt together whenever the
 * panel changes what it's pointed at: the harness picker, the scratch dir
 * (and its "preparing" flag), the `runSession` reducer, and the run target
 * that a "Test" run leaves active. A skill/deployment change, a harness
 * switch, and "New session" all reset the same cluster, so they share one
 * hook rather than three call sites hand-rolling the same reset list.
 */
function useAssistantRunSession({
  skill,
  skillMdPath,
  defaultHarness,
  cancel,
  judgeCancel,
  reset,
  judgeReset,
  addToast,
}: UseAssistantRunSessionParams) {
  const [harness, setHarness] = useState<AgentId>(defaultHarness);
  const [scratchDir, setScratchDir] = useState<string | undefined>(undefined);
  const [isPreparing, setIsPreparing] = useState(false);
  const [showTestForm, setShowTestForm] = useState(false);

  const [runSession, dispatchRunSession] = useReducer(
    runSessionReducer,
    undefined,
    initialRunSessionState,
  );

  // Bumped on every transition (skill/deployment change, harness change, New
  // session, unmount) so an in-flight `runTest`/`runAsk`/`runAudit` can tell
  // it's become stale and stop touching state after its next `await`.
  const opTokenRef = useRef(0);
  // Mirrors `runSession.runTarget` for cleanup code that runs outside React's
  // render cycle (unmount, and the async transition handlers below) and
  // can't rely on a state value captured by a stale closure.
  const activeTargetRef = useRef<SkillRunTargetInfo | null>(null);

  const setActiveTarget = (target: SkillRunTargetInfo | null) => {
    activeTargetRef.current = target;
    dispatchRunSession({ type: "set_target", target });
  };

  /** Ends whatever run target is still active when leaving a "Test" run
   * unfinished: a Scratch/Worktree target is discarded (best effort, toast on
   * failure); an InPlace target's changes are left on disk as-is, since
   * reverting them without the user asking to would be surprising. */
  const releaseActiveTarget = async () => {
    const target = activeTargetRef.current;
    activeTargetRef.current = null;
    if (!target) return;
    if (target.kind === "in_place") {
      addToast({
        type: "info",
        title: "Test changes were kept",
        message: `Test changes were kept in ${projectBasename(target.cwd)}`,
      });
      return;
    }
    try {
      await discardSkillRunTarget(target.id);
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't discard the test run",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

  useEffect(() => {
    // A new skill, or a different deployment's copy of the same skill, can't
    // reuse a previous one's scratch dir or transcript. Cancel any run
    // against the old skill/path before the scratch dir cleanup effect below
    // (keyed on `scratchDir`) removes the folder it runs in.
    let ignore = false;
    const token = ++opTokenRef.current;
    (async () => {
      await cancel();
      await judgeCancel();
      await releaseActiveTarget();
      if (ignore || opTokenRef.current !== token) return;
      setScratchDir(undefined);
      setHarness(defaultHarness);
      activeTargetRef.current = null;
      dispatchRunSession({ type: "reset" });
      setShowTestForm(false);
      reset();
      judgeReset();
    })();
    return () => {
      ignore = true;
    };
    // `defaultHarness`, `cancel`, and `reset` are recomputed every render;
    // only a skill or deployment change should re-run this.
  }, [skill.name, skillMdPath]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    return () => {
      if (scratchDir) removeSkillScratchDir(scratchDir).catch(() => {});
    };
  }, [scratchDir]);

  // Unmount (including Escape/back navigation away from the skill page,
  // which unmounts this panel) - fire-and-forget, nothing left to await into.
  useEffect(() => {
    return () => {
      // eslint-disable-next-line react-hooks/exhaustive-deps -- staleness counter, not a DOM ref; reading it at unmount time is exactly the point.
      opTokenRef.current++;
      cancel().catch(() => {});
      judgeCancel().catch(() => {});
      releaseActiveTarget().catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleSelectHarness = async (agent: AgentId) => {
    if (agent === harness) return;
    const token = ++opTokenRef.current;
    // Stop the previous harness's run before switching out from under it.
    await cancel();
    await judgeCancel();
    await releaseActiveTarget();
    if (opTokenRef.current !== token) return;
    setHarness(agent);
    activeTargetRef.current = null;
    dispatchRunSession({ type: "reset" });
    reset();
  };

  const handleNewSession = async () => {
    const token = ++opTokenRef.current;
    await cancel();
    await judgeCancel();
    await releaseActiveTarget();
    if (opTokenRef.current !== token) return;
    activeTargetRef.current = null;
    dispatchRunSession({ type: "reset" });
    reset();
  };

  /** Creates the scratch dir on first use, reusing it for every later run. */
  const ensureScratchDir = async (): Promise<string | undefined> => {
    if (scratchDir) return scratchDir;
    const sourcePath = sourceFolderPath(skill);
    if (!sourcePath) {
      addToast({
        type: "error",
        title: "Can't run the assistant",
        message: "This skill has no folder on disk to run against.",
      });
      return undefined;
    }
    setIsPreparing(true);
    try {
      const dir = await createSkillScratchDir([[skill.name, sourcePath]]);
      setScratchDir(dir);
      setIsPreparing(false);
      return dir;
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't prepare a scratch folder",
        message: err instanceof Error ? err.message : "Unknown error",
      });
      setIsPreparing(false);
      return undefined;
    }
  };

  return {
    harness,
    scratchDir,
    isPreparing,
    showTestForm,
    setShowTestForm,
    runSession,
    dispatchRunSession,
    activeTargetRef,
    setActiveTarget,
    opTokenRef,
    ensureScratchDir,
    handleSelectHarness,
    handleNewSession,
  };
}

interface RunSkillTestFlowParams {
  skill: InstalledSkill;
  params: SkillTestRunParams;
  sourcePath: string;
  extraSkills: [string, string][];
  harness: HarnessId;
  token: number;
  opTokenRef: RefObject<number>;
  setActiveTarget: (target: SkillRunTargetInfo | null) => void;
  dispatchRunSession: React.Dispatch<RunSessionAction>;
  run: (request: Parameters<ReturnType<typeof useSkillAgentRun>["run"]>[0]) => Promise<void>;
  waitForFinish: () => Promise<SkillAgentRunState>;
  judge: ReturnType<typeof useSkillAgentRun>;
  addToast: (toast: Omit<Toast, "id">) => string;
}

/**
 * Drives a whole "Test" run start-to-finish, once its target's inputs have
 * been validated and `dispatchRunSession({ type: "start_test" })` has fired:
 * prepare the target, run the skill in it, await its terminal state, judge
 * that state (if it finished ok), fetch the diff for a worktree/in-place
 * target, and record the outcome exactly once. `opTokenRef` is checked after
 * every `await` so a transition (skill/harness change, New session, unmount)
 * that happens mid-run makes every later step in this call a no-op.
 */
async function runSkillTestFlow({
  skill,
  params,
  sourcePath,
  extraSkills,
  harness,
  token,
  opTokenRef,
  setActiveTarget,
  dispatchRunSession,
  run,
  waitForFinish,
  judge,
  addToast,
}: RunSkillTestFlowParams): Promise<void> {
  let target: SkillRunTargetInfo;
  try {
    target = await prepareSkillRunTarget({
      kind: params.targetKind,
      skill_name: skill.name,
      skill_folder: sourcePath,
      extra_skills: extraSkills,
      fixture: params.fixture ?? null,
      project_path: params.projectPath ?? null,
    });
  } catch (err) {
    addToast({
      type: "error",
      title: "Couldn't start the test",
      message: err instanceof Error ? err.message : "Unknown error",
    });
    dispatchRunSession({ type: "set_phase", phase: "idle" });
    return;
  }
  if (opTokenRef.current !== token) {
    // A transition raced us while `prepare` was in flight - the effect
    // that bumped the token already released whatever target was active
    // then, but this one was never assigned to `activeTargetRef`.
    discardSkillRunTarget(target.id).catch(() => {});
    return;
  }
  setActiveTarget(target);
  dispatchRunSession({ type: "set_phase", phase: "running" });

  // Registered before `run()` so a Finished event that arrives the instant
  // the run starts can't be missed.
  const finished = waitForFinish();
  // SAFETY: `harness` only ever holds a value from `HarnessSegmentedControl`,
  // which offers exactly the four `HarnessId` agents.
  await run({
    harness,
    prompt: params.prompt,
    cwd: target.cwd,
    skill_name: skill.name,
    write_access: "workspace",
  });
  if (opTokenRef.current !== token) return;
  const runState = await finished;
  if (opTokenRef.current !== token) return;

  if (runState.status !== "finished") {
    recordSkillRun(
      buildRunRecord(runState, harness, skill.name, "test", target.kind, undefined),
      runState.events,
    ).catch(() => {});
    dispatchRunSession({ type: "set_phase", phase: "done" });
    return;
  }

  dispatchRunSession({ type: "set_phase", phase: "judging" });
  const toolSummary = runState.events.reduce<string[]>((summaries, e) => {
    if (e.kind.kind === "tool_call") summaries.push(e.kind.summary);
    return summaries;
  }, []);
  const judgeFinished = judge.waitForFinish();
  // SAFETY: same as above.
  await judge.run({
    harness,
    prompt: buildSkillJudgePrompt({
      skillName: skill.name,
      description: skill.description,
      testPrompt: params.prompt,
      finalText: runState.finalText ?? "",
      toolSummary,
    }),
    cwd: target.cwd,
    skill_name: skill.name,
    write_access: "read_only",
  });
  if (opTokenRef.current !== token) return;
  const judgeState = await judgeFinished;
  if (opTokenRef.current !== token) return;

  const verdict =
    judgeState.status === "finished" && judgeState.finalText !== undefined
      ? parseJudgeVerdict(judgeState.finalText)
      : null;
  dispatchRunSession({ type: "set_verdict", verdict });

  if (target.kind !== "scratch") {
    dispatchRunSession({ type: "set_diff_busy", busy: true });
    try {
      dispatchRunSession({ type: "set_diff", diff: await skillRunTargetDiff(target.id) });
      dispatchRunSession({ type: "set_diff_busy", busy: false });
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't read the diff",
        message: err instanceof Error ? err.message : "Unknown error",
      });
      dispatchRunSession({ type: "set_diff_busy", busy: false });
    }
    if (opTokenRef.current !== token) return;
  }

  await recordSkillRun(
    buildRunRecord(runState, harness, skill.name, "test", target.kind, verdict ?? undefined),
    runState.events,
  ).catch(() => {});
  if (opTokenRef.current === token) dispatchRunSession({ type: "set_phase", phase: "done" });
}

interface UseRunDiffActionsParams {
  runTarget: SkillRunTargetInfo | null;
  activeTargetRef: RefObject<SkillRunTargetInfo | null>;
  setActiveTarget: (target: SkillRunTargetInfo | null) => void;
  dispatchRunSession: React.Dispatch<RunSessionAction>;
  addToast: (toast: Omit<Toast, "id">) => string;
}

/** The diff review step at the end of a worktree/in-place "Test" run: apply
 * or discard its changes, or (for a scratch target instead) open/delete its
 * folder. All four act on `runTarget`, so they share one hook rather than
 * four call sites each re-deriving it. */
function useRunDiffActions({
  runTarget,
  activeTargetRef,
  setActiveTarget,
  dispatchRunSession,
  addToast,
}: UseRunDiffActionsParams) {
  const handleApplyDiff = async () => {
    if (!runTarget) return;
    dispatchRunSession({ type: "set_diff_busy", busy: true });
    try {
      if (runTarget.kind === "worktree") {
        await applySkillRunTargetDiff(runTarget.id);
        addToast({ type: "success", title: "Applied to project" });
      }
      // InPlace's changes are already on disk - "Keep" just dismisses the diff.
      activeTargetRef.current = null;
      dispatchRunSession({ type: "clear_diff" });
      dispatchRunSession({ type: "set_diff_busy", busy: false });
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't apply the changes",
        message: err instanceof Error ? err.message : "Unknown error",
      });
      dispatchRunSession({ type: "set_diff_busy", busy: false });
    }
  };

  const handleDiscardDiff = async () => {
    if (!runTarget) return;
    dispatchRunSession({ type: "set_diff_busy", busy: true });
    try {
      await discardSkillRunTarget(runTarget.id);
      activeTargetRef.current = null;
      dispatchRunSession({ type: "clear_diff" });
      dispatchRunSession({ type: "set_diff_busy", busy: false });
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't discard the changes",
        message: err instanceof Error ? err.message : "Unknown error",
      });
      dispatchRunSession({ type: "set_diff_busy", busy: false });
    }
  };

  const handleOpenScratchFolder = () => {
    if (runTarget?.kind === "scratch") revealSkillRunTarget(runTarget.id).catch(() => {});
  };

  const handleDeleteScratchFolder = async () => {
    if (runTarget?.kind !== "scratch") return;
    try {
      await discardSkillRunTarget(runTarget.id);
      setActiveTarget(null);
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't delete the scratch folder",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

  return { handleApplyDiff, handleDiscardDiff, handleOpenScratchFolder, handleDeleteScratchFolder };
}

interface UseAssistantActionsParams {
  skill: InstalledSkill;
  rawContent: string | null;
  skillMdPath: string | undefined;
  isPluginManaged: boolean;
  harness: AgentId;
  prompt: string;
  state: SkillAgentRunState;
  run: (request: Parameters<ReturnType<typeof useSkillAgentRun>["run"]>[0]) => Promise<void>;
  waitForFinish: () => Promise<SkillAgentRunState>;
  judge: ReturnType<typeof useSkillAgentRun>;
  opTokenRef: RefObject<number>;
  ensureScratchDir: () => Promise<string | undefined>;
  activeTargetRef: RefObject<SkillRunTargetInfo | null>;
  setActiveTarget: (target: SkillRunTargetInfo | null) => void;
  dispatchRunSession: React.Dispatch<RunSessionAction>;
  auditStartContentRef: RefObject<string | null>;
  recordedRunIdRef: RefObject<string | undefined>;
  otherOwnSkills: InstalledSkill[];
  addToast: (toast: Omit<Toast, "id">) => string;
}

/**
 * Owns the panel's three run kinds - Ask, Audit, and Test. Pulled out of
 * `SkillAssistantPanel` since all three close over the same handful of
 * session refs/dispatchers, keeping the component's own body to its state
 * setup, effects, and render.
 */
function useAssistantActions({
  skill,
  rawContent,
  skillMdPath,
  isPluginManaged,
  harness,
  prompt,
  state,
  run,
  waitForFinish,
  judge,
  opTokenRef,
  ensureScratchDir,
  activeTargetRef,
  setActiveTarget,
  dispatchRunSession,
  auditStartContentRef,
  recordedRunIdRef,
  otherOwnSkills,
  addToast,
}: UseAssistantActionsParams) {
  const handleRun = async () => {
    if (!prompt.trim() || state.status === "running") return;
    const token = opTokenRef.current;
    const dir = await ensureScratchDir();
    if (!dir || opTokenRef.current !== token) return;

    dispatchRunSession({ type: "set_run_kind", kind: "ask" });
    recordedRunIdRef.current = undefined;
    try {
      // SAFETY: `harness` only ever holds a value from `HarnessSegmentedControl`,
      // which offers exactly the four `HarnessId` agents.
      await run({
        harness: harness as HarnessId,
        prompt,
        cwd: dir,
        skill_name: skill.name,
        write_access: "workspace",
        session_id: state.sessionId,
      });
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't start the run",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

  const handleAudit = async () => {
    if (rawContent === null || !skillMdPath || isPluginManaged || state.status === "running")
      return;
    const token = opTokenRef.current;
    const dir = await ensureScratchDir();
    if (!dir || opTokenRef.current !== token) return;

    auditStartContentRef.current = rawContent;
    dispatchRunSession({ type: "start_audit" });
    recordedRunIdRef.current = undefined;
    try {
      // SAFETY: `harness` only ever holds a value from `HarnessSegmentedControl`,
      // which offers exactly the four `HarnessId` agents.
      await run({
        harness: harness as HarnessId,
        prompt: buildSkillAuditPrompt({
          skillName: skill.name,
          skillMd: rawContent,
          harness,
          deployments: skill.deployments.map((d) => `${d.agent} · ${d.scope}`),
        }),
        cwd: dir,
        skill_name: skill.name,
        write_access: "read_only",
      });
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't start the audit",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

  /**
   * Validates a "Test" run's target inputs, then hands off to
   * `runSkillTestFlow` for the actual prepare/run/judge/diff/record
   * sequence - see that function for why `opTokenRef` gets threaded through.
   */
  const handleRunTest = async (params: SkillTestRunParams) => {
    if (state.status === "running") return;
    const token = opTokenRef.current;
    const sourcePath = sourceFolderPath(skill);
    if (!sourcePath) {
      addToast({
        type: "error",
        title: "Can't run the test",
        message: "This skill has no folder on disk to run against.",
      });
      return;
    }

    const extraSkills: [string, string][] = params.extraSkillNames
      .map((name): [string, string] | undefined => {
        const folder = otherOwnSkills.find((s) => s.name === name)?.deployments[0]?.path;
        return folder ? [name, folder] : undefined;
      })
      .filter((entry): entry is [string, string] => entry !== undefined);

    activeTargetRef.current = null;
    dispatchRunSession({ type: "start_test" });

    // SAFETY: `harness` only ever holds a value from `HarnessSegmentedControl`,
    // which offers exactly the four `HarnessId` agents.
    await runSkillTestFlow({
      skill,
      params,
      sourcePath,
      extraSkills,
      harness: harness as HarnessId,
      token,
      opTokenRef,
      setActiveTarget,
      dispatchRunSession,
      run,
      waitForFinish,
      judge,
      addToast,
    });
  };

  return { handleRun, handleAudit, handleRunTest };
}

interface TestJudgePanelProps {
  judgeState: SkillAgentRunState;
  runState: SkillAgentRunState;
  verdict: ReturnType<typeof parseJudgeVerdict>;
  harness: AgentId;
}

/** The "Test" run's judge transcript and pass/fail verdict, shown once the
 * judge harness has produced at least one event. */
function TestJudgePanel({ judgeState, runState, verdict, harness }: TestJudgePanelProps) {
  return (
    <>
      {(judgeState.events.length > 0 || judgeState.status !== "idle") && (
        <>
          <div className="text-caption font-semibold tracking-[0.04em] text-text-tertiary uppercase">
            Judge
          </div>
          <SkillAgentTranscript state={judgeState} />
        </>
      )}

      {verdict && (
        <div className="flex flex-col gap-1">
          <div className={`text-small ${verdict.passed ? "text-success" : "text-error"}`}>
            {[
              verdict.passed ? "Passed" : "Failed",
              `skill loaded: ${runState.skillLoaded ?? "unknown"}`,
              runState.durationMs !== undefined
                ? `${(runState.durationMs / 1000).toFixed(1)} s`
                : undefined,
              runState.costUsd !== undefined ? `$${runState.costUsd.toFixed(2)}` : undefined,
              `on ${HARNESS_LABELS.find(([id]) => id === harness)?.[1] ?? harness}`,
            ]
              .filter((segment): segment is string => Boolean(segment))
              .join(" · ")}
          </div>
          <p className="m-0 text-caption text-text-secondary">{verdict.sentence}</p>
        </div>
      )}
    </>
  );
}

interface AskComposerProps {
  textareaRef: RefObject<HTMLTextAreaElement | null>;
  prompt: string;
  onPromptChange: (value: string) => void;
  onKeyDown: (event: React.KeyboardEvent<HTMLTextAreaElement>) => void;
  isRunning: boolean;
  sessionId: string | undefined;
  onNewSession: () => void;
  onRun: () => void;
  onCancel: () => void;
}

/** The "Ask" tab's prompt box plus its session/Run/Cancel footer row. */
function AskComposer({
  textareaRef,
  prompt,
  onPromptChange,
  onKeyDown,
  isRunning,
  sessionId,
  onNewSession,
  onRun,
  onCancel,
}: AskComposerProps) {
  return (
    <div className="flex flex-col gap-1.5">
      <Textarea
        ref={textareaRef}
        className="resize-none rounded-sm border-border bg-bg-tertiary px-2.5 py-2 text-small text-text-primary focus-visible:border-border-focus focus-visible:ring-0"
        placeholder="Ask about this skill…"
        rows={2}
        value={prompt}
        onChange={(e) => onPromptChange(e.target.value)}
        onKeyDown={onKeyDown}
        disabled={isRunning}
      />
      <div className="flex items-center justify-between gap-2">
        {sessionId ? (
          <span className="text-caption text-text-tertiary">
            Continues the current session ·{" "}
            <button
              type="button"
              className="cursor-pointer border-0 bg-transparent p-0 text-caption text-accent transition-colors hover:text-accent-hover"
              onClick={onNewSession}
            >
              New session
            </button>
          </span>
        ) : (
          <span />
        )}
        {isRunning ? (
          <Button variant="outline" size="sm" onClick={onCancel}>
            Cancel
          </Button>
        ) : (
          <Button size="sm" onClick={onRun} disabled={!prompt.trim()}>
            Run
          </Button>
        )}
      </div>
    </div>
  );
}

/**
 * A per-skill assistant surface: a harness picker (defaulting to Claude Code
 * when it sees the skill, else the first harness that does) and an "Ask"
 * box that runs the selected harness against a scratch copy of this skill.
 * "Audit" runs the same harness read-only with a review prompt and, when it
 * proposes a rewrite, renders it as acceptable/rejectable hunks. "Test" runs
 * the skill against a scratch, worktree, or in-place copy, judges whether it
 * did what its description promises, and (for worktree/in-place) surfaces
 * the resulting diff.
 */
/** Loaded on demand: `@pierre/diffs` bundles Shiki, which would otherwise double the main chunk. */
const SkillProposedEdits = lazy(() =>
  import("./SkillProposedEdits").then((m) => ({ default: m.SkillProposedEdits })),
);
const SkillRunDiff = lazy(() =>
  import("./SkillRunDiff").then((m) => ({ default: m.SkillRunDiff })),
);

export function SkillAssistantPanel({
  skill,
  rawContent,
  skillMdPath,
  isPluginManaged,
  onApplied,
  onDiskChanged,
  showHistory,
  onCloseHistory,
}: SkillAssistantPanelProps) {
  const addToast = useAppStore((state) => state.addToast);
  const { snapshot } = useSkillSnapshot();
  const visibleAgents = visibleAgentsFor(skill);
  const defaultHarness: AgentId =
    (visibleAgents.includes("claude-code") ? "claude-code" : visibleAgents[0]) ?? "claude-code";
  const harnessSelectItemsForSkill = harnessSelectItems(visibleAgents);

  const [prompt, setPrompt] = useState("");
  // The file's content at the moment the current audit run started, so a
  // later save (from the editor, or another tab) doesn't get misattributed
  // to the run that reviewed the pre-save file.
  const auditStartContentRef = useRef<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const { run, cancel, reset, state, waitForFinish } = useSkillAgentRun();
  const judge = useSkillAgentRun();
  const recordedRunIdRef = useRef<string | undefined>(undefined);

  const {
    harness,
    scratchDir,
    isPreparing,
    showTestForm,
    setShowTestForm,
    runSession,
    dispatchRunSession,
    activeTargetRef,
    setActiveTarget,
    opTokenRef,
    ensureScratchDir,
    handleSelectHarness,
    handleNewSession,
  } = useAssistantRunSession({
    skill,
    skillMdPath,
    defaultHarness,
    cancel,
    judgeCancel: judge.cancel,
    reset,
    judgeReset: judge.reset,
    addToast,
  });
  const { runKind, proposal, runTarget, runDiff, isDiffBusy, testPhase, judgeVerdict } = runSession;

  useEffect(() => {
    return () => {
      if (scratchDir) removeSkillScratchDir(scratchDir).catch(() => {});
    };
  }, [scratchDir]);

  useEffect(() => {
    const el = textareaRef.current;
    // Re-measure only after the DOM reflects the latest prompt text.
    if (!el || el.value !== prompt) return;
    el.style.height = "auto";
    const lineHeight = parseFloat(getComputedStyle(el).lineHeight || "20");
    const maxHeight = lineHeight * MAX_TEXTAREA_ROWS;
    el.style.height = `${Math.min(el.scrollHeight, maxHeight)}px`;
  }, [prompt]);

  const otherOwnSkills = ownSkillsView(snapshot?.skills ?? []).filter((s) => s.name !== skill.name);

  const { handleRun, handleAudit, handleRunTest } = useAssistantActions({
    skill,
    rawContent,
    skillMdPath,
    isPluginManaged,
    harness,
    prompt,
    state,
    run,
    waitForFinish,
    judge,
    opTokenRef,
    ensureScratchDir,
    activeTargetRef,
    setActiveTarget,
    dispatchRunSession,
    auditStartContentRef,
    recordedRunIdRef,
    otherOwnSkills,
    addToast,
  });

  // Once an audit run finishes, pull the proposed rewrite (if any) out of its
  // final text and diff it against the file as it was when the run started.
  useEffect(() => {
    if (
      runKind !== "audit" ||
      state.status !== "finished" ||
      state.finalText === undefined ||
      !skillMdPath
    )
      return;
    const fileAtAuditStart = auditStartContentRef.current;
    if (fileAtAuditStart === null) return;
    const extracted = extractProposedSkillMd(state.finalText);
    if (extracted === null) return;
    const proposedText = normalizeProposalToOriginal(extracted, fileAtAuditStart);
    if (proposedText === fileAtAuditStart) return;
    dispatchRunSession({
      type: "set_proposal",
      proposal: {
        fileAtAuditStart,
        hunks: diffSkillMd(fileAtAuditStart, proposedText),
        skillMdPath,
      },
    });
  }, [runKind, state.status, state.finalText, skillMdPath, dispatchRunSession]);

  // Record every finished/errored Ask or Audit run once. Test records itself
  // once, inline, at the end of `runTest`.
  useEffect(() => {
    if (runKind === "test") return;
    if (state.status !== "finished" && state.status !== "error") return;
    if (!state.runId || recordedRunIdRef.current === state.runId) return;
    recordedRunIdRef.current = state.runId;
    recordSkillRun(
      // SAFETY: `harness` only ever holds a value from `HarnessSegmentedControl`,
      // which offers exactly the four `HarnessId` agents.
      buildRunRecord(state, harness as HarnessId, skill.name, runKind, undefined, undefined),
      state.events,
    ).catch(() => {});
  }, [runKind, state, skill.name, harness]);

  const handleKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      handleRun();
    }
  };

  const { handleApplyDiff, handleDiscardDiff, handleOpenScratchFolder, handleDeleteScratchFolder } =
    useRunDiffActions({
      runTarget,
      activeTargetRef,
      setActiveTarget,
      dispatchRunSession,
      addToast,
    });

  const isRunning = state.status === "running" || isPreparing;
  const hasTranscript = state.events.length > 0;
  const canAudit = rawContent !== null && !isPluginManaged && !isRunning;
  const candidateProjects = testCandidateProjects(skill, snapshot?.projects ?? []);
  const isTestRunning =
    testPhase === "preparing" || testPhase === "running" || testPhase === "judging";

  if (showHistory) {
    return <SkillRunHistory skillName={skill.name} onClose={onCloseHistory} />;
  }

  return (
    <div className="flex flex-col gap-3">
      <SelectControl
        ariaLabel="Harness"
        value={harness}
        onValueChange={(value) => {
          // SAFETY: `items` only ever holds a value from `HARNESS_LABELS`.
          handleSelectHarness(value as AgentId);
        }}
        items={harnessSelectItemsForSkill}
        leadingIcon={<HarnessIcon harness={harness} size={14} />}
      />

      {hasTranscript ? (
        <SkillAgentTranscript state={state} />
      ) : (
        <p className="m-0 text-pretty text-small leading-normal text-text-tertiary">
          Ask the harness anything about this skill. It runs in a scratch folder with only this
          skill installed.
        </p>
      )}

      {runKind === "test" && (
        <TestJudgePanel
          judgeState={judge.state}
          runState={state}
          verdict={judgeVerdict}
          harness={harness}
        />
      )}

      {proposal && rawContent !== null && skillMdPath && (
        <Suspense
          fallback={<p className="m-0 text-caption text-text-tertiary">Loading changes…</p>}
        >
          <SkillProposedEdits
            fileAtAuditStart={proposal.fileAtAuditStart}
            currentContent={rawContent}
            skillMdPath={skillMdPath}
            proposalSkillMdPath={proposal.skillMdPath}
            hunks={proposal.hunks}
            onHunksChange={(hunks) =>
              dispatchRunSession({ type: "set_proposal", proposal: { ...proposal, hunks } })
            }
            onApplied={(content) => {
              onApplied(content);
              dispatchRunSession({ type: "set_proposal", proposal: null });
            }}
            onDiscard={() => dispatchRunSession({ type: "set_proposal", proposal: null })}
            onDiskChanged={onDiskChanged}
          />
        </Suspense>
      )}

      {runTarget && runTarget.kind !== "scratch" && runDiff !== null && (
        <Suspense
          fallback={<p className="m-0 text-caption text-text-tertiary">Loading changes…</p>}
        >
          <SkillRunDiff
            projectLabel={projectBasename(runTarget.cwd)}
            targetKind={runTarget.kind}
            diff={runDiff}
            isBusy={isDiffBusy}
            onPrimary={handleApplyDiff}
            onSecondary={handleDiscardDiff}
          />
        </Suspense>
      )}

      {runTarget && runTarget.kind === "scratch" && state.status !== "running" && (
        <div className="flex gap-2">
          <Button variant="outline" size="sm" className="flex-1" onClick={handleOpenScratchFolder}>
            Open folder
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="flex-1"
            onClick={handleDeleteScratchFolder}
          >
            Delete scratch folder
          </Button>
        </div>
      )}

      <div id="skill-assistant-test-toggle-region">
        {showTestForm ? (
          <SkillTestForm
            skill={skill}
            otherOwnSkills={otherOwnSkills}
            candidateProjects={candidateProjects}
            isRunning={isTestRunning}
            onRun={handleRunTest}
          />
        ) : (
          <AskComposer
            textareaRef={textareaRef}
            prompt={prompt}
            onPromptChange={setPrompt}
            onKeyDown={handleKeyDown}
            isRunning={isRunning}
            sessionId={state.sessionId}
            onNewSession={handleNewSession}
            onRun={handleRun}
            onCancel={() => cancel()}
          />
        )}
      </div>

      <div className="flex gap-2">
        <Button
          variant="outline"
          size="sm"
          className="flex-1"
          onClick={handleAudit}
          disabled={!canAudit}
        >
          Audit
        </Button>
        <Button
          variant={showTestForm ? "default" : "outline"}
          size="sm"
          className="flex-1"
          onClick={() => setShowTestForm((open) => !open)}
          disabled={isRunning}
          aria-expanded={showTestForm}
          aria-controls="skill-assistant-test-toggle-region"
        >
          Test
        </Button>
      </div>
    </div>
  );
}

/** Projects where `skill` is deployed, else every tracked project. */
function testCandidateProjects(skill: InstalledSkill, allProjects: string[]): string[] {
  const deployed = [
    ...new Set(skill.deployments.map((d) => d.project_path).filter((p): p is string => Boolean(p))),
  ];
  return deployed.length > 0 ? deployed : allProjects;
}
