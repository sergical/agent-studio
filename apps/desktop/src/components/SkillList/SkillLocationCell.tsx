// ============================================================================
// SkillLocationCell - the shared Location group: where on disk the skill
// sits (global home folders, added project roots), independent of which
// harnesses reach it. The project chip's tooltip is exception-only: it only
// appears once the truncated name has actually clipped.
// ============================================================================

import { useLayoutEffect, useRef, useState } from "react";
import { FolderGit2, Globe } from "lucide-react";
import { RichTooltip } from "../ui/RichTooltip";
import type { DiskLocation } from "./skill-row-state";

function LocationItem({ location }: { location: DiskLocation }) {
  const nameRef = useRef<HTMLSpanElement>(null);
  const [truncated, setTruncated] = useState(false);
  const isProject = location.kind === "project";
  // Measured after layout (and again on resize) so the tooltip exists before the first hover.
  useLayoutEffect(() => {
    if (!isProject) return;
    const measure = () => {
      const el = nameRef.current;
      if (el) setTruncated(el.scrollWidth > el.clientWidth);
    };
    measure();
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, [isProject]);
  const chip = (
    <span
      className={`inline-flex items-center gap-1 ${isProject ? "min-w-0 max-w-[120px]" : "shrink-0"}`}
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
  if (!isProject || !truncated) return chip;
  return (
    <RichTooltip content={<span className="text-small">{location.name}</span>}>{chip}</RichTooltip>
  );
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
