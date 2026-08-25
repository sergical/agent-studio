// ============================================================================
// SkillAssistantPanel - Right-column card on the skill page: a harness
// picker, an "Ask about this skill" box that runs the local harness in a
// scratch folder containing only this skill, and three not-yet-wired
// actions (Audit, Test, Apply)
// ============================================================================

import { useEffect, useRef, useState } from "react";
import { useSkillAgentRun } from "../../hooks/useSkillAgentRun";
import { createSkillScratchDir, removeSkillScratchDir } from "../../lib/skill-agent-api";
import type { HarnessId } from "../../lib/skill-agent-types";
import { skillVisibleToAgent } from "../../lib/skill-coverage";
import { ownDeployments } from "../../lib/skill-plugin-partition";
import { COMMON_AGENTS } from "../../lib/skill-types";
import type { AgentId, InstalledSkill } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";
import { HarnessSegmentedControl } from "../ui/HarnessSegmentedControl";
import { SkillAgentTranscript } from "./SkillAgentTranscript";

interface SkillAssistantPanelProps {
  skill: InstalledSkill;
}

const MAX_TEXTAREA_ROWS = 8;

/** Every first-class agent that can actually see `skill`, in `COMMON_AGENTS` order. */
function visibleAgentsFor(skill: InstalledSkill): AgentId[] {
  return COMMON_AGENTS.filter((agent) => skillVisibleToAgent(skill, agent) !== "none");
}

/** `skill`'s own skill folder, for the scratch dir - a plugin-only skill has no folder to copy. */
function sourceFolderPath(skill: InstalledSkill): string | undefined {
  return ownDeployments(skill)[0]?.path ?? skill.deployments[0]?.path;
}

/**
 * A per-skill assistant surface: a harness picker (defaulting to Claude Code
 * when it sees the skill, else the first harness that does) and an "Ask"
 * box that runs the selected harness against a scratch copy of this skill.
 * Audit/Test/Apply are reserved for a coming update.
 */
export function SkillAssistantPanel({ skill }: SkillAssistantPanelProps) {
  const addToast = useAppStore((state) => state.addToast);
  const visibleAgents = visibleAgentsFor(skill);
  const defaultHarness: AgentId =
    (visibleAgents.includes("claude-code") ? "claude-code" : visibleAgents[0]) ?? "claude-code";

  const [harness, setHarness] = useState<AgentId>(defaultHarness);
  const [prompt, setPrompt] = useState("");
  const [scratchDir, setScratchDir] = useState<string | undefined>(undefined);
  const [isPreparing, setIsPreparing] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const { run, cancel, reset, state } = useSkillAgentRun();

  useEffect(() => {
    // A new skill can't reuse a previous one's scratch dir or transcript.
    // Cancel any run against the old skill before the scratch dir cleanup
    // effect below (keyed on `scratchDir`) removes the folder it runs in.
    let ignore = false;
    (async () => {
      await cancel();
      if (ignore) return;
      setScratchDir(undefined);
      setHarness(defaultHarness);
      reset();
    })();
    return () => {
      ignore = true;
    };
    // `defaultHarness`, `cancel`, and `reset` are recomputed every render;
    // only a skill change should re-run this.
  }, [skill.name]); // eslint-disable-line react-hooks/exhaustive-deps

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
    reset();
  };

  const handleNewSession = async () => {
    await cancel();
    reset();
  };

  const handleRun = async () => {
    if (!prompt.trim() || state.status === "running") return;

    let dir = scratchDir;
    if (!dir) {
      const sourcePath = sourceFolderPath(skill);
      if (!sourcePath) {
        addToast({
          type: "error",
          title: "Can't run the assistant",
          message: "This skill has no folder on disk to run against.",
        });
        return;
      }
      setIsPreparing(true);
      try {
        dir = await createSkillScratchDir([[skill.name, sourcePath]]);
        setScratchDir(dir);
      } catch (err) {
        addToast({
          type: "error",
          title: "Couldn't prepare a scratch folder",
          message: err instanceof Error ? err.message : "Unknown error",
        });
        return;
      } finally {
        setIsPreparing(false);
      }
    }

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

  const handleKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      handleRun();
    }
  };

  const isRunning = state.status === "running" || isPreparing;
  const hasTranscript = state.events.length > 0;

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

      <div className="skill-assistant-panel-actions">
        <button type="button" className="skill-action-button" disabled>
          Audit
        </button>
        <button type="button" className="skill-action-button" disabled>
          Test
        </button>
        <button type="button" className="skill-action-button" disabled>
          Apply
        </button>
      </div>

      <p className="skill-assistant-panel-note">Coming in the next update.</p>
    </div>
  );
}
