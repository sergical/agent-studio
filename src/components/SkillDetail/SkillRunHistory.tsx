// ============================================================================
// SkillRunHistory - Lists the last runs recorded for a skill (Ask/Audit/Test,
// harness, outcome, relative time). Clicking a row loads its transcript
// read-only, with no follow-up prompt box.
// ============================================================================

import { useEffect, useState } from "react";
import { ArrowLeft } from "lucide-react";
import type { SkillAgentRunState } from "../../hooks/useSkillAgentRun";
import { listSkillRuns, readSkillRunEvents } from "../../lib/skill-run-history-api";
import type { SkillRunAction, SkillRunRecord } from "../../lib/skill-run-history-types";
import { formatRelativeTime } from "../../lib/skill-stats";
import { HarnessIcon } from "../ui/HarnessIcon";
import { SkillAgentTranscript } from "./SkillAgentTranscript";

interface SkillRunHistoryProps {
  skillName: string;
  onClose: () => void;
}

const ACTION_LABEL = {
  ask: "Ask",
  audit: "Audit",
  test: "Test",
} satisfies Record<SkillRunAction, string>;

/** Builds a read-only `SkillAgentRunState` from a recorded run, for `SkillAgentTranscript`. */
function stateFromRecord(
  record: SkillRunRecord,
  events: SkillAgentRunState["events"],
): SkillAgentRunState {
  return {
    status: record.ok ? "finished" : "error",
    runId: record.id,
    events,
    finalText: record.final_text,
    sessionId: undefined,
    costUsd: record.cost_usd,
    durationMs: record.duration_ms,
    skillLoaded: record.skill_loaded,
    errorMessage: undefined,
  };
}

/**
 * A "Runs" list opened from the skill page header's "Last test" line: rows
 * for every recorded run, newest first; clicking one loads its transcript in
 * place of the list.
 */
export function SkillRunHistory({ skillName, onClose }: SkillRunHistoryProps) {
  const [runs, setRuns] = useState<SkillRunRecord[] | null>(null);
  const [selected, setSelected] = useState<SkillRunRecord | null>(null);
  const [selectedState, setSelectedState] = useState<SkillAgentRunState | null>(null);

  useEffect(() => {
    let cancelled = false;
    listSkillRuns(skillName)
      .then((records) => {
        if (!cancelled) setRuns(records);
      })
      .catch(() => {
        if (!cancelled) setRuns([]);
      });
    return () => {
      cancelled = true;
    };
  }, [skillName]);

  const handleSelect = async (record: SkillRunRecord) => {
    setSelected(record);
    setSelectedState(null);
    try {
      const events = await readSkillRunEvents(skillName, record.id);
      setSelectedState(stateFromRecord(record, events));
    } catch {
      setSelectedState(stateFromRecord(record, []));
    }
  };

  if (selected) {
    return (
      <div className="skill-run-history">
        <button
          type="button"
          className="skill-run-history-back"
          onClick={() => {
            setSelected(null);
            setSelectedState(null);
          }}
        >
          <ArrowLeft size={14} />
          Runs
        </button>
        {selectedState ? (
          <SkillAgentTranscript state={selectedState} />
        ) : (
          <p className="skill-assistant-panel-note">Loading transcript…</p>
        )}
      </div>
    );
  }

  return (
    <div className="skill-run-history">
      <div className="skill-run-history-header">
        <span className="skill-assistant-panel-label">Runs</span>
        <button type="button" className="skill-action-button" onClick={onClose}>
          Close
        </button>
      </div>
      {runs === null ? (
        <p className="skill-assistant-panel-note">Loading…</p>
      ) : runs.length === 0 ? (
        <p className="skill-assistant-panel-note">No runs recorded yet.</p>
      ) : (
        <div className="skill-run-history-rows">
          {runs.map((run) => (
            <button
              key={run.id}
              type="button"
              className="skill-run-history-row"
              onClick={() => handleSelect(run)}
            >
              <span className="skill-run-history-row-time">
                {formatRelativeTime(run.started_at)}
              </span>
              <span className="skill-run-history-row-action">{ACTION_LABEL[run.action]}</span>
              <HarnessIcon harness={run.harness} size={12} />
              <span
                className={`skill-run-history-row-outcome ${run.judge ? (run.judge.passed ? "pass" : "fail") : run.ok ? "pass" : "fail"}`}
              >
                {run.judge ? (run.judge.passed ? "Passed" : "Failed") : run.ok ? "OK" : "Failed"}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
