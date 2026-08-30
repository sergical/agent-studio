// ============================================================================
// SkillRunHistory - Lists the last runs recorded for a skill (Ask/Audit/Test,
// harness, outcome, relative time). Clicking a row loads its transcript
// read-only, with no follow-up prompt box.
// ============================================================================

import { useEffect, useState } from "react";
import { ArrowLeft } from "lucide-react";
import { Button } from "@skill-studio/ui";
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
      <div className="flex flex-col gap-2.5 rounded-md border border-border bg-bg-secondary p-4">
        <button
          type="button"
          className="inline-flex cursor-pointer items-center gap-1.5 self-start border-0 bg-transparent p-0 text-small text-text-secondary hover:text-text-primary"
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
          <p className="m-0 text-caption text-text-tertiary">Loading transcript…</p>
        )}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2.5 rounded-md border border-border bg-bg-secondary p-4">
      <div className="flex items-center justify-between">
        <span className="text-caption font-semibold tracking-[0.04em] text-text-tertiary uppercase">
          Runs
        </span>
        <Button variant="outline" size="sm" onClick={onClose}>
          Close
        </Button>
      </div>
      {runs === null ? (
        <p className="m-0 text-caption text-text-tertiary">Loading…</p>
      ) : runs.length === 0 ? (
        <p className="m-0 text-caption text-text-tertiary">No runs recorded yet.</p>
      ) : (
        <div className="flex flex-col gap-1.5">
          {runs.map((run) => (
            <button
              key={run.id}
              type="button"
              className="flex h-8 cursor-pointer items-center gap-2.5 rounded-sm border border-border-subtle bg-bg-tertiary px-2.5 text-left hover:bg-bg-hover"
              onClick={() => handleSelect(run)}
            >
              <span className="min-w-14 text-caption text-text-tertiary">
                {formatRelativeTime(run.started_at)}
              </span>
              <span className="flex-1 text-small text-text-secondary">
                {ACTION_LABEL[run.action]}
              </span>
              <HarnessIcon harness={run.harness} size={12} />
              <span
                className={`text-caption font-semibold ${
                  (run.judge ? run.judge.passed : run.ok) ? "text-success" : "text-error"
                }`}
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
