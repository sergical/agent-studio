// ============================================================================
// InstalledSkillHeader - Title, description, state chips, and any blocking
// violation for the rendered copy. The actions cluster (primary action,
// assistant toggle, ⋯ menu) lives in `PageShell`'s `actions` now; the source
// ledger's facts moved into the properties rail.
// ============================================================================

import { AlertTriangle } from "lucide-react";
import { Button } from "@skill-studio/ui";
import {
  describeFrontmatterErrorLine,
  describeFrontmatterRepair,
  isBlockingSpecViolation,
  parseYamlFrontmatterError,
  proposeFrontmatterQuoteRepair,
} from "@skill-studio/lib";
import type {
  Deployment,
  FrontmatterQuoteRepair,
  FrontmatterRepairPreview,
  InstalledSkill,
} from "@skill-studio/lib";
import { TooltipControl } from "../ui/TooltipControl";
import { canOfferLocalQuote } from "./skill-frontmatter-repair-policy";

interface InstalledSkillHeaderProps {
  skill: InstalledSkill;
  /** The deployment whose SKILL.md the page renders - the header's violation line follows it. */
  deployment?: Deployment;
  frontmatterRepair?: FrontmatterRepairPreview | null;
  /** False while the backend preview is pending; the local quote repair waits for it. */
  isFrontmatterPreviewSettled?: boolean;
  /** The rendered copy's SKILL.md text; the quote repair and the line hint are derived from it. */
  skillMdContent?: string | null;
  /** Omitted when the rendered copy cannot be edited in place (plugin-managed). */
  onQuoteRepair?: (repair: FrontmatterQuoteRepair) => void;
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
  isFrontmatterPreviewSettled = false,
  skillMdContent,
  onQuoteRepair,
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
  const yamlViolation = blockingViolations.find((violation) =>
    violation.startsWith("invalid YAML frontmatter at line "),
  );
  const hasMalformedYaml = yamlViolation !== undefined;
  const yamlLocation = yamlViolation ? parseYamlFrontmatterError(yamlViolation) : null;
  // A backend "Fix" preview wins; the local quote repair covers what it declines.
  const canQuote = canOfferLocalQuote({
    isPreviewSettled: isFrontmatterPreviewSettled,
    hasPreview: Boolean(frontmatterRepair),
  });
  const quoteRepair =
    yamlLocation && skillMdContent && canQuote && onQuoteRepair
      ? proposeFrontmatterQuoteRepair(skillMdContent, yamlLocation.line)
      : null;
  const lineHint =
    yamlLocation && skillMdContent && canQuote
      ? describeFrontmatterErrorLine(skillMdContent, yamlLocation.line)
      : null;
  const violationText = quoteRepair
    ? describeFrontmatterRepair(quoteRepair, yamlLocation?.column)
    : (lineHint ?? blockingViolations.join("; "));

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
          <span>{violationText}</span>
          {quoteRepair && (
            <Button size="sm" onClick={() => onQuoteRepair?.(quoteRepair)}>
              Quote the {quoteRepair.key}
            </Button>
          )}
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
