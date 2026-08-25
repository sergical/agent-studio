// ============================================================================
// SkillAssistantPanel - Right-column card on the skill page: a harness
// picker, an "Ask about this skill" box that runs the local harness in a
// scratch folder containing only this skill, an "Audit" action that reviews
// SKILL.md and proposes a per-hunk rewrite via `SkillProposedEdits`, and a
// "Test" action that runs the skill against a scratch/worktree/in-place
// target, judges the result, and (for worktree/in-place) surfaces the diff
// ============================================================================

import { Suspense, lazy, useEffect, useRef, useState } from "react";
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
} from "../../lib/skill-agent-prompts";
import type { HarnessId } from "../../lib/skill-agent-types";
import { skillVisibleToAgent } from "../../lib/skill-coverage";
import type { SkillMdHunk } from "../../lib/skill-md-diff";
import { diffSkillMd } from "../../lib/skill-md-diff";
import { ownDeployments, ownSkillsView } from "../../lib/skill-plugin-partition";
import { recordSkillRun } from "../../lib/skill-run-history-api";
import type {
  SkillRunAction,
  SkillRunJudge,
  SkillRunRecord,
} from "../../lib/skill-run-history-types";
import {
  applySkillRunTargetDiff,
  discardSkillRunTarget,
  prepareSkillRunTarget,
  revealSkillRunTarget,
  skillRunTargetDiff,
} from "../../lib/skill-run-target-api";
import type { SkillRunTarget } from "../../lib/skill-run-target-types";
import { COMMON_AGENTS } from "../../lib/skill-types";
import type { AgentId, InstalledSkill } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";
import { HARNESS_LABELS, HarnessSegmentedControl } from "../ui/HarnessSegmentedControl";
import { SkillAgentTranscript } from "./SkillAgentTranscript";
import { SkillRunHistory } from "./SkillRunHistory";
import type { SkillTestRunParams } from "./SkillTestForm";
import { SkillTestForm } from "./SkillTestForm";

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
  targetKind: SkillRunTarget["kind"] | undefined,
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

  const [harness, setHarness] = useState<AgentId>(defaultHarness);
  const [prompt, setPrompt] = useState("");
  const [scratchDir, setScratchDir] = useState<string | undefined>(undefined);
  const [isPreparing, setIsPreparing] = useState(false);
  const [runKind, setRunKind] = useState<"ask" | "audit" | "test">("ask");
  const [showTestForm, setShowTestForm] = useState(false);
  const [proposal, setProposal] = useState<Proposal | null>(null);
  // The file's content at the moment the current audit run started, so a
  // later save (from the editor, or another tab) doesn't get misattributed
  // to the run that reviewed the pre-save file.
  const auditStartContentRef = useRef<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const { run, cancel, reset, state } = useSkillAgentRun();
  const judge = useSkillAgentRun();

  const [runTarget, setRunTarget] = useState<SkillRunTarget | null>(null);
  // The project a Worktree/InPlace target was prepared against - needed to
  // apply the diff back, since the target itself doesn't carry it.
  const [testProjectPath, setTestProjectPath] = useState<string | undefined>(undefined);
  const [runDiff, setRunDiff] = useState<string | null>(null);
  const [isDiffBusy, setIsDiffBusy] = useState(false);
  const [testPrompt, setTestPrompt] = useState("");
  const [judgeVerdict, setJudgeVerdict] = useState<ReturnType<typeof parseJudgeVerdict>>(null);
  const recordedRunIdRef = useRef<string | undefined>(undefined);

  useEffect(() => {
    // A new skill, or a different deployment's copy of the same skill, can't
    // reuse a previous one's scratch dir or transcript. Cancel any run
    // against the old skill/path before the scratch dir cleanup effect below
    // (keyed on `scratchDir`) removes the folder it runs in.
    let ignore = false;
    (async () => {
      await cancel();
      if (ignore) return;
      setScratchDir(undefined);
      setHarness(defaultHarness);
      setProposal(null);
      setRunTarget(null);
      setTestProjectPath(undefined);
      setRunDiff(null);
      setJudgeVerdict(null);
      setShowTestForm(false);
      reset();
      judge.reset();
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

  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    const lineHeight = parseFloat(getComputedStyle(el).lineHeight || "20");
    const maxHeight = lineHeight * MAX_TEXTAREA_ROWS;
    el.style.height = `${Math.min(el.scrollHeight, maxHeight)}px`;
  }, [prompt]);

  const handleSelectHarness = async (agent: AgentId) => {
    if (agent === harness) return;
    // Stop the previous harness's run before switching out from under it.
    await cancel();
    setHarness(agent);
    setProposal(null);
    setRunTarget(null);
    setTestProjectPath(undefined);
    setRunDiff(null);
    setJudgeVerdict(null);
    reset();
  };

  const handleNewSession = async () => {
    await cancel();
    setProposal(null);
    setRunTarget(null);
    setTestProjectPath(undefined);
    setRunDiff(null);
    setJudgeVerdict(null);
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
      return dir;
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't prepare a scratch folder",
        message: err instanceof Error ? err.message : "Unknown error",
      });
      return undefined;
    } finally {
      setIsPreparing(false);
    }
  };

  const handleRun = async () => {
    if (!prompt.trim() || state.status === "running") return;
    const dir = await ensureScratchDir();
    if (!dir) return;

    setRunKind("ask");
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
    const dir = await ensureScratchDir();
    if (!dir) return;

    setProposal(null);
    auditStartContentRef.current = rawContent;
    setRunKind("audit");
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

  const handleRunTest = async (params: SkillTestRunParams) => {
    if (state.status === "running") return;
    const sourcePath = sourceFolderPath(skill);
    if (!sourcePath) {
      addToast({
        type: "error",
        title: "Can't run the test",
        message: "This skill has no folder on disk to run against.",
      });
      return;
    }

    const otherOwn = ownSkillsView(snapshot?.skills ?? []).filter((s) => s.name !== skill.name);
    const extraSkills: [string, string][] = params.extraSkillNames
      .map((name): [string, string] | undefined => {
        const folder = otherOwn.find((s) => s.name === name)?.deployments[0]?.path;
        return folder ? [name, folder] : undefined;
      })
      .filter((entry): entry is [string, string] => entry !== undefined);

    setProposal(null);
    setRunDiff(null);
    setRunTarget(null);
    setTestProjectPath(undefined);
    setJudgeVerdict(null);
    setRunKind("test");
    setTestPrompt(params.prompt);
    recordedRunIdRef.current = undefined;

    try {
      const target = await prepareSkillRunTarget({
        kind: params.targetKind,
        skill_name: skill.name,
        skill_folder: sourcePath,
        extra_skills: extraSkills,
        fixture: params.fixture ?? null,
        project_path: params.projectPath ?? null,
      });
      setRunTarget(target);
      setTestProjectPath(params.projectPath);
      // SAFETY: `harness` only ever holds a value from `HarnessSegmentedControl`,
      // which offers exactly the four `HarnessId` agents.
      await run({
        harness: harness as HarnessId,
        prompt: params.prompt,
        cwd: target.cwd,
        skill_name: skill.name,
        write_access: "workspace",
      });
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't start the test",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

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
    setProposal({
      fileAtAuditStart,
      hunks: diffSkillMd(fileAtAuditStart, proposedText),
      skillMdPath,
    });
  }, [runKind, state.status, state.finalText, skillMdPath]);

  // Record every finished/errored Ask or Audit run once.
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

  // Once a "test" run finishes ok, start the judge turn as a fresh session
  // over the same working directory.
  useEffect(() => {
    if (runKind !== "test" || state.status !== "finished" || !runTarget) return;
    if (judge.state.status !== "idle") return;
    const toolSummary = state.events
      .filter((e) => e.kind.kind === "tool_call")
      .map((e) => (e.kind.kind === "tool_call" ? e.kind.summary : ""));
    judge
      .run({
        // SAFETY: `harness` only ever holds a value from `HarnessSegmentedControl`,
        // which offers exactly the four `HarnessId` agents.
        harness: harness as HarnessId,
        prompt: buildSkillJudgePrompt({
          skillName: skill.name,
          description: skill.description,
          testPrompt,
          finalText: state.finalText ?? "",
          toolSummary,
        }),
        cwd: runTarget.cwd,
        skill_name: skill.name,
        write_access: "read_only",
      })
      .catch(() => {});
  }, [runKind, state, runTarget, judge, harness, skill.name, skill.description, testPrompt]);

  // Once the judge finishes, fetch the diff (worktree/in place) and record
  // the whole test run, including the judge's verdict.
  useEffect(() => {
    if (runKind !== "test" || !runTarget) return;
    if (state.status !== "finished" && state.status !== "error") return;
    if (state.status === "finished" && judge.state.status === "running") return;
    if (!state.runId || recordedRunIdRef.current === state.runId) return;
    recordedRunIdRef.current = state.runId;

    const verdict =
      state.status === "finished" && judge.state.finalText !== undefined
        ? parseJudgeVerdict(judge.state.finalText)
        : null;
    setJudgeVerdict(verdict);
    const judgeRecord: SkillRunJudge | undefined = verdict ?? undefined;

    (async () => {
      if (runTarget.kind !== "scratch") {
        setIsDiffBusy(true);
        try {
          setRunDiff(await skillRunTargetDiff(runTarget));
        } catch (err) {
          addToast({
            type: "error",
            title: "Couldn't read the diff",
            message: err instanceof Error ? err.message : "Unknown error",
          });
        } finally {
          setIsDiffBusy(false);
        }
      }
      await recordSkillRun(
        // SAFETY: `harness` only ever holds a value from `HarnessSegmentedControl`,
        // which offers exactly the four `HarnessId` agents.
        buildRunRecord(
          state,
          harness as HarnessId,
          skill.name,
          "test",
          runTarget.kind,
          judgeRecord,
        ),
        state.events,
      ).catch(() => {});
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [runKind, state, judge.state, runTarget, skill.name, harness]);

  const handleKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      handleRun();
    }
  };

  const handleApplyDiff = async () => {
    if (!runTarget) return;
    setIsDiffBusy(true);
    try {
      if (runTarget.kind === "worktree") {
        if (!testProjectPath) throw new Error("No project path recorded for this run.");
        await applySkillRunTargetDiff(runTarget, testProjectPath);
        addToast({ type: "success", title: "Applied to project" });
      }
      // InPlace's changes are already on disk - "Keep" just dismisses the diff.
      setRunDiff(null);
      setRunTarget(null);
      setTestProjectPath(undefined);
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't apply the changes",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
      setIsDiffBusy(false);
    }
  };

  const handleDiscardDiff = async () => {
    if (!runTarget) return;
    setIsDiffBusy(true);
    try {
      await discardSkillRunTarget(runTarget);
      setRunDiff(null);
      setRunTarget(null);
      setTestProjectPath(undefined);
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't discard the changes",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
      setIsDiffBusy(false);
    }
  };

  const handleOpenScratchFolder = () => {
    if (runTarget?.kind === "scratch") revealSkillRunTarget(runTarget).catch(() => {});
  };

  const handleDeleteScratchFolder = async () => {
    if (runTarget?.kind !== "scratch") return;
    try {
      await discardSkillRunTarget(runTarget);
      setRunTarget(null);
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't delete the scratch folder",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

  const isRunning = state.status === "running" || isPreparing;
  const hasTranscript = state.events.length > 0;
  const canAudit = rawContent !== null && !isPluginManaged && !isRunning;
  const otherOwnSkills = ownSkillsView(snapshot?.skills ?? []).filter((s) => s.name !== skill.name);
  const candidateProjects = testCandidateProjects(skill, snapshot?.projects ?? []);
  const isTestRunning =
    runKind === "test" && (state.status === "running" || judge.state.status === "running");

  if (showHistory) {
    return <SkillRunHistory skillName={skill.name} onClose={onCloseHistory} />;
  }

  return (
    <div className="skill-assistant-panel">
      <div className="skill-assistant-panel-label">Assistant</div>

      <HarnessSegmentedControl
        selected={harness}
        visibleAgents={new Set(visibleAgents)}
        onSelect={handleSelectHarness}
      />

      {hasTranscript ? (
        <SkillAgentTranscript state={state} />
      ) : (
        <p className="skill-assistant-panel-empty">
          Ask the harness anything about this skill. It runs in a scratch folder with only this
          skill installed.
        </p>
      )}

      {runKind === "test" && (judge.state.events.length > 0 || judge.state.status !== "idle") && (
        <>
          <div className="skill-assistant-panel-label">Judge</div>
          <SkillAgentTranscript state={judge.state} />
        </>
      )}

      {runKind === "test" && judgeVerdict && (
        <div className="skill-test-result">
          <div className={`skill-test-result-line ${judgeVerdict.passed ? "" : "failed"}`}>
            {[
              judgeVerdict.passed ? "Passed" : "Failed",
              `skill loaded: ${state.skillLoaded ?? "unknown"}`,
              state.durationMs !== undefined
                ? `${(state.durationMs / 1000).toFixed(1)} s`
                : undefined,
              state.costUsd !== undefined ? `$${state.costUsd.toFixed(2)}` : undefined,
              `on ${HARNESS_LABELS.find(([id]) => id === harness)?.[1] ?? harness}`,
            ]
              .filter((segment): segment is string => Boolean(segment))
              .join(" · ")}
          </div>
          <p className="skill-test-result-sentence">{judgeVerdict.sentence}</p>
        </div>
      )}

      {proposal && rawContent !== null && skillMdPath && (
        <Suspense fallback={<p className="skill-assistant-panel-note">Loading changes…</p>}>
          <SkillProposedEdits
            fileAtAuditStart={proposal.fileAtAuditStart}
            currentContent={rawContent}
            skillMdPath={skillMdPath}
            proposalSkillMdPath={proposal.skillMdPath}
            hunks={proposal.hunks}
            onHunksChange={(hunks) => setProposal({ ...proposal, hunks })}
            onApplied={(content) => {
              onApplied(content);
              setProposal(null);
            }}
            onDiscard={() => setProposal(null)}
            onDiskChanged={onDiskChanged}
          />
        </Suspense>
      )}

      {runTarget && runTarget.kind !== "scratch" && runDiff !== null && (
        <Suspense fallback={<p className="skill-assistant-panel-note">Loading changes…</p>}>
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
        <div className="skill-assistant-panel-actions">
          <button type="button" className="skill-action-button" onClick={handleOpenScratchFolder}>
            Open folder
          </button>
          <button type="button" className="skill-action-button" onClick={handleDeleteScratchFolder}>
            Delete scratch folder
          </button>
        </div>
      )}

      {showTestForm ? (
        <SkillTestForm
          skill={skill}
          otherOwnSkills={otherOwnSkills}
          candidateProjects={candidateProjects}
          isRunning={isTestRunning}
          onRun={handleRunTest}
        />
      ) : (
        <div className="skill-assistant-ask">
          <textarea
            ref={textareaRef}
            className="skill-assistant-ask-input"
            placeholder="Ask about this skill…"
            rows={2}
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            onKeyDown={handleKeyDown}
            disabled={isRunning}
          />
          <div className="skill-assistant-ask-footer">
            {state.sessionId ? (
              <span className="skill-assistant-ask-session-note">
                Continues the current session ·{" "}
                <button
                  type="button"
                  className="skill-assistant-ask-new-session"
                  onClick={handleNewSession}
                >
                  New session
                </button>
              </span>
            ) : (
              <span />
            )}
            {isRunning ? (
              <button type="button" className="skill-action-button" onClick={() => cancel()}>
                Cancel
              </button>
            ) : (
              <button
                type="button"
                className="skill-action-button primary"
                onClick={handleRun}
                disabled={!prompt.trim()}
              >
                Run
              </button>
            )}
          </div>
        </div>
      )}

      <div className="skill-assistant-panel-actions">
        <button
          type="button"
          className="skill-action-button"
          onClick={handleAudit}
          disabled={!canAudit}
        >
          Audit
        </button>
        <button
          type="button"
          className={`skill-action-button ${showTestForm ? "active" : ""}`}
          onClick={() => setShowTestForm((open) => !open)}
          disabled={isRunning}
        >
          Test
        </button>
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
