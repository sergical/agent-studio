// ============================================================================
// SkillLocationCell - "Where does this skill really live, and who links to
// it": one chip for the shared-root truth, one per symlink into it, one per
// separate copy (drift risk), and the existing broken-link chip.
// ============================================================================

import { Copy, FolderClosed, Link2, Unlink } from "lucide-react";
import { deploymentLinkTarget, driftingCopies, locationSummary } from "../../lib/skill-coverage";
import { homeRelativePath } from "../../lib/skill-path-format";
import type { Deployment, InstalledSkill } from "../../lib/skill-types";
import { TooltipControl } from "../ui/TooltipControl";

interface SkillLocationCellProps {
  skill: InstalledSkill;
}

export function SkillLocationCell({ skill }: SkillLocationCellProps) {
  const summary = locationSummary(skill);
  const { truth, links, copies, broken } = summary;
  const drifting = driftingCopies(summary);

  // No shared-root copy and exactly one own directory: nothing to compare it
  // against, so it's just a plain harness chip, no relation icon.
  if (!truth && links.length === 0 && copies.length === 1 && broken.length === 0) {
    const [only] = copies;
    return (
      <span className="skill-list-table-chips">
        <span className="skill-list-location-chip">{only.agent}</span>
      </span>
    );
  }

  return (
    <span className="skill-list-table-chips">
      {truth && (
        <span className="skill-list-location-chip truth" aria-label="shared, source of truth">
          <FolderClosed size={12} />
          shared
        </span>
      )}
      {links.map((d, i) => {
        const target = deploymentLinkTarget(d);
        return (
          <TooltipControl
            key={`link-${d.agent}-${i}`}
            content={
              d.is_symlink
                ? `Symlink → ${target ? homeRelativePath(target) : "unknown target"}`
                : `Linked folder → ${target ? homeRelativePath(target) : "unknown target"}`
            }
          >
            <span
              className="skill-list-location-chip link"
              aria-label={`${d.agent}, linked to the shared folder`}
            >
              <Link2 size={12} />
              {d.agent}
            </span>
          </TooltipControl>
        );
      })}
      {copies.map((d, i) => {
        const isDrifting = drifting.includes(d);
        return (
          <TooltipControl
            key={`copy-${d.agent}-${i}`}
            content={`Separate copy at ${homeRelativePath(d.path)} · ${isDrifting ? "content differs" : "same content"}`}
          >
            <span
              className={`skill-list-location-chip ${isDrifting ? "copy" : ""}`}
              aria-label={`${d.agent}, separate copy`}
            >
              <Copy size={12} />
              {d.agent}
            </span>
          </TooltipControl>
        );
      })}
      {broken.map((d, i) => (
        <BrokenChip key={`broken-${d.agent}-${i}`} deployment={d} />
      ))}
    </span>
  );
}

function BrokenChip({ deployment }: { deployment: Deployment }) {
  return (
    <TooltipControl content="Broken link">
      <span
        className="skill-list-location-chip broken"
        aria-label={`${deployment.agent}, broken link`}
      >
        <Unlink size={12} />
        {deployment.agent}
      </span>
    </TooltipControl>
  );
}
