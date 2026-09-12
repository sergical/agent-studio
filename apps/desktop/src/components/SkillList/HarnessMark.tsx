// ============================================================================
// HarnessMark - shared harness/Universal-folder marks: a fixed-size box that
// shows the harness's brand icon, dimmed to the `--color-icon-muted` tone
// when the harness doesn't reach the skill, badged with a small link glyph
// anchored to the icon when it reaches the skill only through the Universal
// folder. Every reached mark has a one-line tooltip naming the harness and
// how it reaches the skill, since the 11px badge is too small to carry that
// alone.
// ============================================================================

import { Bot, Layers, Link2, Link2Off } from "lucide-react";
import type { AgentId } from "@skill-studio/lib";
import { HarnessIcon } from "../ui/HarnessIcon";
import { RichTooltip } from "../ui/RichTooltip";
import type { HarnessReach, Universal } from "./skill-row-state";

/** The ids `HarnessIcon` has a brand SVG for. */
const KNOWN_HARNESS_IDS = new Set<AgentId | "shared">([
  "claude-code",
  "codex",
  "open-code",
  "pi",
  "cursor",
  "grok-build",
  "shared",
]);

/** `HarnessIcon` renders nothing for an id it has no brand SVG for; this
 * fallback draws lucide's `Bot` instead so every harness always has an icon
 * to show. */
export function HarnessGlyph({
  harness,
  size,
  muted,
}: {
  harness: AgentId;
  size: number;
  muted?: boolean;
}) {
  if (KNOWN_HARNESS_IDS.has(harness))
    return <HarnessIcon harness={harness} size={size} muted={muted} />;
  return (
    <Bot size={size} aria-hidden style={muted ? { color: "var(--color-icon-muted)" } : undefined} />
  );
}

function harnessMarkAriaLabel(reach: HarnessReach): string {
  const how =
    reach.how === "linked"
      ? "link to the Universal folder"
      : reach.how === "broken"
        ? "broken link"
        : reach.how === "universal"
          ? "source in the Universal folder"
          : "own copy";
  return `${reach.label} · ${how}`;
}

/** One line per reached harness: `<glyph> <label> · <how>[ · <state>]` - the
 * words the 11px badge cannot carry on its own. The how-word (linked /
 * source / own copy) always shows; a broken link replaces it in the error
 * tone, and disabled or parked follows it in the warning tone. */
function harnessMarkTooltip(reach: HarnessReach, parked: boolean) {
  const how =
    reach.how === "broken"
      ? { text: "link broken", tone: "text-error" }
      : reach.how === "linked"
        ? { text: "linked to Universal folder", tone: "text-text-tertiary" }
        : reach.how === "universal"
          ? { text: "source in Universal folder", tone: "text-text-tertiary" }
          : { text: "own copy", tone: "text-text-tertiary" };
  const state = parked ? "parked" : reach.disabled ? "disabled" : null;
  return (
    <span className="flex items-center gap-1.5 text-small">
      <HarnessGlyph harness={reach.harness} size={14} />
      <span>{reach.label}</span>
      <span className={how.tone}>· {how.text}</span>
      {state && <span className="text-warning">· {state}</span>}
    </span>
  );
}

/** One harness's mark: unreached is the harness's own icon dimmed to
 * `--color-icon-muted` at `opacity-60` - readable as a second state next to a
 * full-tone icon rather than "two shades" - with no tooltip, since there's
 * nothing to say about a harness that isn't there. Reached is the brand icon
 * at full tone, badged with a small link (or broken-link) glyph anchored to
 * the icon itself (not the slot) when the reach is only through the
 * Universal folder, so every harness's badge lands at the same offset from
 * its icon's corner. */
export function HarnessMark({
  reach,
  size = 13,
  parked = false,
}: {
  reach: HarnessReach;
  size?: number;
  /** The whole skill is parked: every disc takes the disabled treatment and says so. */
  parked?: boolean;
}) {
  if (!reach.reached) {
    return (
      <span
        className="relative inline-flex size-5 items-center justify-center opacity-60"
        aria-hidden
      >
        <HarnessGlyph harness={reach.harness} size={size} muted />
      </span>
    );
  }
  const badge = reach.how === "linked" || reach.how === "broken" ? reach.how : null;
  // A disabled or parked glyph is always muted.
  const glyphMuted = reach.disabled || parked;
  const mark = (
    <span
      className="group relative inline-flex size-5 items-center justify-center"
      aria-label={harnessMarkAriaLabel(reach)}
    >
      <span className="relative inline-flex" style={{ width: size, height: size }}>
        <HarnessGlyph harness={reach.harness} size={size} muted={glyphMuted} />
        {badge && (
          <span className="absolute -right-1 -bottom-1 inline-flex size-2.5 items-center justify-center rounded-full bg-bg-primary group-hover:bg-bg-secondary">
            {badge === "linked" ? (
              <Link2 size={7} className="text-text-secondary" aria-hidden />
            ) : (
              <Link2Off size={7} className="text-error" aria-hidden />
            )}
          </span>
        )}
      </span>
    </span>
  );
  return <RichTooltip content={harnessMarkTooltip(reach, parked)}>{mark}</RichTooltip>;
}

/** The Universal folder's own mark: present is the folder's `Layers` glyph
 * at full tone; absent is the same glyph dimmed to `--color-icon-muted` at
 * `opacity-60`. Present shows a one-line "Universal folder" tooltip so the
 * Layers glyph is named. */
export function UniversalMark({ universal, size = 13 }: { universal: Universal; size?: number }) {
  if (!universal.present) {
    return (
      <span
        className="relative inline-flex size-5 items-center justify-center opacity-60"
        aria-hidden
      >
        <Layers size={size} style={{ color: "var(--color-icon-muted)" }} />
      </span>
    );
  }
  return (
    <RichTooltip
      content={
        <span className="flex items-center gap-1.5 text-small">
          <Layers size={14} aria-hidden />
          Universal folder
        </span>
      }
    >
      <span
        className="relative inline-flex size-5 items-center justify-center"
        aria-label="Universal folder"
      >
        <Layers size={size} className="text-text-primary" aria-hidden />
      </span>
    </RichTooltip>
  );
}
