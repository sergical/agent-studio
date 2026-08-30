// ============================================================================
// SkillAgentTranscript - Renders one skill-agent run's streamed events as a
// scrolling transcript: assistant text (Markdown, deltas appended in place),
// collapsed tool-call/result rows, error blocks, and a footer summary once
// the run finishes
// ============================================================================

import { useEffect, useRef, useState } from "react";
import { ChevronRight } from "lucide-react";
import ReactMarkdown from "react-markdown";
import type { SkillAgentRunState } from "../../hooks/useSkillAgentRun";
import type { SkillAgentEvent, SkillLoaded } from "@skill-studio/lib";

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
    <div className="flex flex-col rounded-sm border border-border-subtle bg-bg-tertiary">
      <button
        type="button"
        className="flex h-7 cursor-pointer items-center gap-1.5 border-0 bg-transparent px-2 text-left transition-colors hover:bg-bg-hover"
        onClick={() => setIsOpen((open) => !open)}
        aria-expanded={isOpen}
      >
        <ChevronRight
          size={14}
          className={`shrink-0 text-text-tertiary transition-transform ${isOpen ? "rotate-90" : ""}`}
        />
        <span className="shrink-0 font-mono text-caption text-text-primary" title={block.name}>
          {block.name}
        </span>
        <span className="overflow-hidden text-ellipsis whitespace-nowrap text-caption text-text-secondary">
          {block.summary}
        </span>
      </button>
      {isOpen && block.detail && (
        <pre className="m-0 max-h-60 overflow-y-auto border-t border-border-subtle p-2 text-caption break-words whitespace-pre-wrap text-text-secondary">
          {block.detail}
        </pre>
      )}
    </div>
  );
}

/**
 * Scrolls inside its own `max-height: 60vh` box, sticking to the bottom as
 * new events arrive unless the user has scrolled up to read earlier output.
 */
export function SkillAgentTranscript({ state }: SkillAgentTranscriptProps) {
  const blocks = buildBlocks(state.events);
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickToBottomRef = useRef(true);

  useEffect(() => {
    const el = scrollRef.current;
    // `state.events.length`, not `blocks`, is the growing-content trigger:
    // every incoming event grows `events` by one, even a text delta that
    // appends into an existing block rather than pushing a new one. The
    // finished/error footer also grows the box's content without adding an
    // event, so a status flip to either is its own trigger.
    const hasNewContent =
      state.events.length > 0 || state.status === "finished" || state.status === "error";
    if (el && stickToBottomRef.current && hasNewContent) {
      el.scrollTop = el.scrollHeight;
    }
  }, [state.events.length, state.status]);

  const handleScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    stickToBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
  };

  return (
    <div
      className="flex max-h-[60vh] flex-col gap-2.5 overflow-y-auto pr-0.5"
      ref={scrollRef}
      onScroll={handleScroll}
    >
      {blocks.map((block) => {
        if (block.kind === "text") {
          return (
            <div key={block.id} className="skill-markdown text-body leading-normal">
              <ReactMarkdown>{block.text}</ReactMarkdown>
            </div>
          );
        }
        if (block.kind === "tool_call") {
          return <ToolCallBlock key={block.id} block={block} />;
        }
        if (block.kind === "tool_result") {
          return (
            <div
              key={block.id}
              className="flex h-7 items-center gap-1.5 rounded-sm border border-border-subtle bg-bg-tertiary px-2"
            >
              <span
                className="shrink-0 font-mono text-caption text-text-primary"
                title={block.name}
              >
                {block.name}
              </span>
              <span className="overflow-hidden text-ellipsis whitespace-nowrap text-caption text-text-secondary">
                {block.summary}
              </span>
            </div>
          );
        }
        return (
          <div
            key={block.id}
            className="rounded-sm bg-error-soft px-2.5 py-2 text-small break-words whitespace-pre-wrap text-error"
          >
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
        <div className="pt-1 text-caption text-text-tertiary">
          {footerLeadSegments(state).join(" · ")}
          {state.skillLoaded !== undefined && (
            <>
              {" · "}
              <span
                className={
                  state.skillLoaded === "no"
                    ? "text-warning"
                    : state.skillLoaded === "unknown"
                      ? "text-text-tertiary"
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
