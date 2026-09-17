// ============================================================================
// SkillLocationCell - the shared Location group: where on disk the skill
// sits (global home folders, added project roots), independent of which
// harnesses reach it. The project chip's tooltip is exception-only: it only
// appears once the truncated name has actually clipped, checked lazily on
// hover/focus rather than measured for every row up front.
// ============================================================================

import { useRef, useState } from "react";
import { FolderGit2, Globe } from "lucide-react";
import { TooltipTrigger } from "@skill-studio/ui";
import { RichTooltip, useRichTooltipHandle } from "../ui/RichTooltip";
import type { DiskLocation } from "./skill-row-state";

function LocationItem({ location }: { location: DiskLocation }) {
  const nameRef = useRef<HTMLSpanElement>(null);
  const [truncated, setTruncated] = useState(false);
  const isProject = location.kind === "project";
  const handle = useRichTooltipHandle();

  /** Measured when a tooltip is about to show, not at mount - most project names never clip, so
   * most rows never pay for a measurement at all. */
  function isClipped() {
    const el = nameRef.current;
    return el !== null && el.scrollWidth > el.clientWidth;
  }
  function checkTruncated() {
    setTruncated(isClipped());
  }

  const chip = (
    <span
      className={`inline-flex items-center gap-1 ${isProject ? "min-w-0 max-w-[120px]" : "shrink-0"}`}
      // Outside a shared scope the chip only becomes a tooltip trigger once hovered and clipped.
      onPointerEnter={isProject && !handle ? checkTruncated : undefined}
      onFocus={isProject && !handle ? checkTruncated : undefined}
    >
      {isProject ? (
        <FolderGit2 size={13} className="shrink-0 text-text-tertiary" aria-hidden />
      ) : (
        <Globe size={13} className="shrink-0 text-text-tertiary" aria-hidden />
      )}
      <span ref={nameRef} className="truncate text-small text-text-secondary">
        {location.name}
      </span>
    </span>
  );
  // The Global chip is already the word "Global" - there's no fact left for a tooltip to add. A
  // project chip only gets one once its name has actually clipped.
  if (!isProject) return chip;
  const content = <span className="text-small">{location.name}</span>;
  // In a scope the chip is always a trigger - swapping elements on hover would miss the hover that
  // caused it - and the shared tooltip shows nothing when the name fits.
  if (handle) {
    return (
      <TooltipTrigger
        handle={handle}
        payload={() => (isClipped() ? content : null)}
        render={chip}
      />
    );
  }
  if (!truncated) return chip;
  return <RichTooltip content={content}>{chip}</RichTooltip>;
}

/** The "+N" chip's tooltip: one line per hidden location, its own kind icon and name. */
function OverflowTooltip({ hidden }: { hidden: DiskLocation[] }) {
  return (
    <div className="flex flex-col gap-1 text-small">
      {hidden.map((location) => (
        <span
          key={`${location.kind}-${location.path}`}
          className="flex items-center gap-1.5 text-text-tertiary"
        >
          {location.kind === "global" ? (
            <Globe size={13} aria-hidden />
          ) : (
            <FolderGit2 size={13} aria-hidden />
          )}
          {location.name}
        </span>
      ))}
    </div>
  );
}

/** Where the skill sits on disk: up to two locations, then a "+N" overflow. */
export function SkillLocationCell({ locations }: { locations: DiskLocation[] }) {
  if (locations.length === 0) {
    return <span className="text-small text-text-tertiary">Nowhere</span>;
  }
  const shown = locations.slice(0, 2);
  const hidden = locations.slice(2);
  return (
    <span className="inline-flex min-w-0 items-center gap-2">
      {shown.map((location) => (
        <LocationItem key={`${location.kind}-${location.path}`} location={location} />
      ))}
      {hidden.length > 0 && (
        <RichTooltip content={<OverflowTooltip hidden={hidden} />}>
          <span className="shrink-0 text-small text-text-tertiary">+{hidden.length}</span>
        </RichTooltip>
      )}
    </span>
  );
}
