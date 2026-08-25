// ============================================================================
// SkillAgentTranscript - Renders one skill-agent run's streamed events as a
// scrolling transcript: assistant text (Markdown, deltas appended in place),
// collapsed tool-call/result rows, error blocks, and a footer summary once
// the run finishes
// ============================================================================

import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronRight } from "lucide-react";
import ReactMarkdown from "react-markdown";
import type { SkillAgentRunState } from "../../hooks/useSkillAgentRun";
import type { SkillAgentEvent, SkillLoaded } from "../../lib/skill-agent-types";

interface SkillAgentTranscriptProps {
  state: SkillAgentRunState;
}

type TranscriptBlock =
  | { kind: "text"; id: string; text: string }
  | { kind: "tool_call"; id: string; name: string; summary: string; detail?: string }
  | { kind: "tool_result"; id: string; name: string; summary: string }
  | { kind: "error"; id: string; message: string };

/**
 * Folds a run's flat event log into renderable blocks: consecutive
 * `assistant_text` deltas append into the same growing block, a non-delta
 * `assistant_text` always starts a fresh one. `started`/`finished` carry no
 * block of their own - the command line is hidden, and `finished` renders
 * as the footer instead.
 */
function buildBlocks(events: SkillAgentEvent[]): TranscriptBlock[] {
  const blocks: TranscriptBlock[] = [];
  for (const event of events) {
    const id = `${event.run_id}-${event.seq}`;
    const kind = event.kind;
    if (kind.kind === "assistant_text") {
      const last = blocks[blocks.length - 1];
      if (kind.is_delta && last?.kind === "text") {
        last.text += kind.text;
      } else {
        blocks.push({ kind: "text", id, text: kind.text });
      }
    } else if (kind.kind === "tool_call") {
      blocks.push({
        kind: "tool_call",
        id,
        name: kind.name,
        summary: kind.summary,
        detail: kind.detail,
      });
    } else if (kind.kind === "tool_result") {
      blocks.push({ kind: "tool_result", id, name: kind.name, summary: kind.summary });
    } else if (kind.kind === "error") {
      blocks.push({ kind: "error", id, message: kind.message });
    }
  }
  return blocks;
}

const SKILL_LOADED_LABEL = {
  yes: "yes",
  no: "no",
  unknown: "unknown",
} as const satisfies Record<SkillLoaded, string>;

/** Footer segments before "skill loaded: …", e.g. ["Finished", "4.2 s", "$0.06"]. Omits parts the run didn't report. */
function footerLeadSegments(state: SkillAgentRunState): string[] {
  const segments = ["Finished"];
  if (state.durationMs !== undefined) segments.push(`${(state.durationMs / 1000).toFixed(1)} s`);
  if (state.costUsd !== undefined) segments.push(`$${state.costUsd.toFixed(2)}`);
  return segments;
}

function ToolCallBlock({ block }: { block: Extract<TranscriptBlock, { kind: "tool_call" }> }) {
  const [isOpen, setIsOpen] = useState(false);
  return (
    <div className="skill-agent-tool-row">
      <button
        type="button"
        className="skill-agent-tool-row-header"
        onClick={() => setIsOpen((open) => !open)}
        aria-expanded={isOpen}
      >
        <ChevronRight
          size={14}
          className={`skill-agent-tool-row-chevron ${isOpen ? "open" : ""}`}
        />
        <span className="skill-agent-tool-row-name">{block.name}</span>
        <span className="skill-agent-tool-row-summary">{block.summary}</span>
      </button>
      {isOpen && block.detail && <pre className="skill-agent-tool-row-detail">{block.detail}</pre>}
    </div>
  );
}

/**
 * Scrolls inside its own `max-height: 60vh` box, sticking to the bottom as
 * new events arrive unless the user has scrolled up to read earlier output.
 */
export function SkillAgentTranscript({ state }: SkillAgentTranscriptProps) {
  const blocks = useMemo(() => buildBlocks(state.events), [state.events]);
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickToBottomRef = useRef(true);

  useEffect(() => {
    const el = scrollRef.current;
    if (el && stickToBottomRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [blocks, state.status]);

  const handleScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    stickToBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
  };

  return (
    <div className="skill-agent-transcript" ref={scrollRef} onScroll={handleScroll}>
      {blocks.map((block) => {
        if (block.kind === "text") {
          return (
            <div key={block.id} className="skill-agent-text-block skill-markdown">
              <ReactMarkdown>{block.text}</ReactMarkdown>
            </div>
          );
        }
        if (block.kind === "tool_call") {
          return <ToolCallBlock key={block.id} block={block} />;
        }
        if (block.kind === "tool_result") {
          return (
            <div key={block.id} className="skill-agent-tool-row">
              <span className="skill-agent-tool-row-name">{block.name}</span>
              <span className="skill-agent-tool-row-summary">{block.summary}</span>
            </div>
          );
        }
        return (
          <div key={block.id} className="skill-agent-error-block">
            {block.message}
          </div>
        );
      })}

      {state.status === "running" && (
        <div className="skill-agent-pulse" aria-label="Running">
          <span />
          <span />
          <span />
        </div>
      )}

      {(state.status === "finished" || state.status === "error") && (
        <div className="skill-agent-footer">
          {footerLeadSegments(state).join(" · ")}
          {state.skillLoaded !== undefined && (
            <>
              {" · "}
              <span
                className={
                  state.skillLoaded === "no"
                    ? "skill-agent-footer-warning"
                    : state.skillLoaded === "unknown"
                      ? "skill-agent-footer-tertiary"
                      : undefined
                }
              >
                skill loaded: {SKILL_LOADED_LABEL[state.skillLoaded]}
              </span>
            </>
          )}
        </div>
      )}
    </div>
  );
}
