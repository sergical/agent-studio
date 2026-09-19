// ============================================================================
// InstalledSkillHeader - Title, description, state chips, and any blocking
// violation for the rendered copy. The actions cluster (primary action,
// assistant toggle, ⋯ menu) lives in `PageShell`'s `actions` now; the source
// ledger's facts moved into the properties rail.
// ============================================================================

import { AlertTriangle } from "lucide-react";
import { Button } from "@skill-studio/ui";
import { isBlockingSpecViolation } from "@skill-studio/lib";
import type { Deployment, FrontmatterRepairPreview, InstalledSkill } from "@skill-studio/lib";
import { TooltipControl } from "../ui/TooltipControl";

interface InstalledSkillHeaderProps {
  skill: InstalledSkill;
  /** The deployment whose SKILL.md the page renders - the header's violation line follows it. */
  deployment?: Deployment;
  frontmatterRepair?: FrontmatterRepairPreview | null;
  onFixYaml: () => void;
  onEditManually: () => void;
}

/** "Parked · Aug 25, 2026" / "Parked" when the timestamp is missing or unparseable. */
function parkedChipLabel(parkedAt: string | null | undefined): string {
  if (!parkedAt) return "Parked";
  const date = new Date(parkedAt);
  if (Number.isNaN(date.getTime())) return "Parked";
  return `Parked · ${date.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" })}`;
}

/**
 * The installed skill header is identity only: title, description, and state
 * chips, followed by any blocking violation for the rendered copy. It shows
 * no controls, location, or invocation details.
 */
export function InstalledSkillHeader({
  skill,
  deployment,
  frontmatterRepair,
  onFixYaml,
  onEditManually,
}: InstalledSkillHeaderProps) {
  const nonBlockingCount =
    skill.spec_violations.length - skill.spec_violations.filter(isBlockingSpecViolation).length;
  // The red violation line names only the deployment whose SKILL.md the page
  // actually renders - other deployments' violations show on their own
  // Locations rows instead (see `SkillLocationsCard`). The union in
  // `skill.spec_violations` (and its "N spec notes" chip above) is unchanged:
  // Home's spec-violation issue still relies on it covering every copy.
  const renderedDeployment = deployment ?? skill.deployments.find((d) => d.content_hash);
  const blockingViolations = (renderedDeployment?.spec_violations ?? []).filter(
    isBlockingSpecViolation,
  );
  const hasMalformedYaml = blockingViolations.some((violation) =>
    violation.startsWith("invalid YAML frontmatter at line "),
  );

  return (
    <header className="flex flex-col gap-4">
      <div>
        <h2 className="text-heading-lg font-semibold text-text-primary">{skill.name}</h2>
        {skill.description && (
          <p className="mt-3 max-w-[65ch] select-text text-pretty text-body leading-[1.5] text-text-secondary">
            {skill.description}
          </p>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-1.5">
        {skill.parked && (
          <span className="inline-flex items-center gap-1 rounded-full bg-bg-tertiary px-2 py-0.5 text-caption text-warning">
            {parkedChipLabel(skill.parked_at)}
          </span>
        )}
        {skill.update_owner_ids.length > 0 && (
          <span className="inline-flex items-center gap-1 rounded-full bg-bg-tertiary px-2 py-0.5 text-caption text-accent">
            Update available
          </span>
        )}
        {nonBlockingCount > 0 && (
          <TooltipControl
            content={skill.spec_violations
              .filter((violation) => !isBlockingSpecViolation(violation))
              .join("; ")}
          >
            <span className="inline-flex items-center gap-1 rounded-full bg-bg-tertiary px-2 py-0.5 text-caption text-text-tertiary">
              {nonBlockingCount} spec note{nonBlockingCount !== 1 ? "s" : ""}
            </span>
          </TooltipControl>
        )}
      </div>

      {blockingViolations.length > 0 && (
        <div className="flex items-center gap-2 text-small text-error">
          <AlertTriangle size={13} />
          <span>{blockingViolations.join("; ")}</span>
          {hasMalformedYaml && (
            <Button
              size="sm"
              variant="outline"
              onClick={frontmatterRepair ? onFixYaml : onEditManually}
            >
              {frontmatterRepair ? "Fix" : "Edit manually"}
            </Button>
          )}
        </div>
      )}
    </header>
  );
}
