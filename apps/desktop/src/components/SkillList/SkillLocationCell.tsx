// ============================================================================
// SkillLocationCell - "Where does this skill really live, and who links to
// it": one chip for the shared-root truth, one per symlink into it, one per
// separate copy (drift risk), and the existing broken-link chip. Agents are
// shown as harness brand marks; the relation glyph beside the mark says how
// that agent reaches the skill (link, copy, broken), tooltips carry the words.
// ============================================================================

import { Copy, Link2, Unlink } from "lucide-react";
import {
  deploymentLinkTarget,
  deploymentRelationText,
  driftingCopies,
  locationSummary,
} from "@skill-studio/lib";
import { homeRelativePath } from "@skill-studio/lib";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";
import { TooltipControl } from "../ui/TooltipControl";

interface SkillLocationCellProps {
  skill: InstalledSkill;
}

/** The plain harness chip and each relation-to-shared-root chip share this base look. */
const LOCATION_CHIP_BASE =
  "inline-flex items-center gap-1 whitespace-nowrap rounded-sm border border-transparent px-1.5 py-0.5 text-caption tracking-[0.02em] text-text-secondary";
const LOCATION_CHIP_CLASS = `${LOCATION_CHIP_BASE} bg-bg-tertiary`;

/** The agent's brand mark, or its text label when no mark exists for it. */
function AgentMark({ agent }: { agent: string }) {
  const id = harnessIdFromLabel(agent);
  if (!id) return <>{agent}</>;
  return <HarnessIcon harness={id} size={13} />;
}

export function SkillLocationCell({ skill }: SkillLocationCellProps) {
  const summary = locationSummary(skill);
  const { truth, links, copies, broken } = summary;
  const drifting = driftingCopies(summary);
  const driftingPaths = new Set(drifting.map((d) => d.path));

  // No shared-root copy and exactly one own directory: nothing to compare it
  // against, so it's just the harness mark, no relation glyph.
  if (!truth && links.length === 0 && copies.length === 1 && broken.length === 0) {
    const [only] = copies;
    return (
      <span className="flex min-w-0 items-center gap-1 overflow-hidden">
        <TooltipControl content={`${only.agent} · ${homeRelativePath(only.path)}`}>
          <span className={LOCATION_CHIP_CLASS} aria-label={only.agent}>
            <AgentMark agent={only.agent} />
          </span>
        </TooltipControl>
      </span>
    );
  }

  return (
    <span className="flex min-w-0 items-center gap-1 overflow-hidden">
      {truth && (
        <TooltipControl content="Shared folder · source of truth">
          <span
            className={`${LOCATION_CHIP_CLASS} text-text-primary`}
            aria-label="shared, source of truth"
          >
            <HarnessIcon harness="shared" size={13} />
          </span>
        </TooltipControl>
      )}
      {links.map((d) => {
        const target = deploymentLinkTarget(d);
        return (
          <TooltipControl
            key={d.path}
            content={
              d.is_symlink
                ? `${d.agent} · symlink → ${target ? homeRelativePath(target) : "unknown target"}`
                : `${d.agent} · ${deploymentRelationText(d)}`
            }
          >
            <span
              className={LOCATION_CHIP_CLASS}
              aria-label={`${d.agent}, linked to the shared folder`}
            >
              <AgentMark agent={d.agent} />
              <Link2 size={10} className="text-text-tertiary" />
            </span>
          </TooltipControl>
        );
      })}
      {copies.map((d) => {
        const isDrifting = driftingPaths.has(d.path);
        return (
          <TooltipControl
            key={d.path}
            content={`${d.agent} · separate copy at ${homeRelativePath(d.path)} · ${isDrifting ? "content differs" : "same content"}`}
          >
            <span
              className={`${LOCATION_CHIP_BASE} ${isDrifting ? "bg-warning-soft" : "bg-bg-tertiary"}`}
              aria-label={`${d.agent}, separate copy${isDrifting ? ", content differs" : ""}`}
            >
              <AgentMark agent={d.agent} />
              <Copy size={10} className={isDrifting ? "text-warning" : "text-text-tertiary"} />
            </span>
          </TooltipControl>
        );
      })}
      {broken.map((d) => (
        <BrokenChip key={d.path} deployment={d} />
      ))}
    </span>
  );
}

function BrokenChip({ deployment }: { deployment: Deployment }) {
  return (
    <TooltipControl content={`${deployment.agent} · broken link`}>
      <span
        className={`${LOCATION_CHIP_CLASS} text-error`}
        aria-label={`${deployment.agent}, broken link`}
      >
        <AgentMark agent={deployment.agent} />
        <Unlink size={10} />
      </span>
    </TooltipControl>
  );
}
