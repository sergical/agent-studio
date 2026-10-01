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
  ownDeployments,
  parseYamlFrontmatterError,
  proposeFrontmatterQuoteRepair,
} from "@skill-studio/lib";
import type {
  Deployment,
  FrontmatterQuoteRepair,
  FrontmatterRepairKind,
  FrontmatterRepairPreview,
  InstalledSkill,
} from "@skill-studio/lib";
import { TooltipControl } from "../ui/TooltipControl";
import { canOfferLocalQuote, frontmatterRepairKindsFor } from "./skill-frontmatter-repair-policy";

interface InstalledSkillHeaderProps {
  skill: InstalledSkill;
  /** The deployment whose SKILL.md the page renders - the header's violation line follows it. */
  deployment?: Deployment;
  /** The previews the backend accepted, one per repair kind. */
  frontmatterRepairs?: FrontmatterRepairPreview[];
  /** False while the backend preview is pending; the local quote repair waits for it. */
  isFrontmatterPreviewSettled?: boolean;
  /** The rendered copy's SKILL.md text; the quote repair and the line hint are derived from it. */
  skillMdContent?: string | null;
  /** Omitted when the rendered copy cannot be edited in place (plugin-managed). */
  onQuoteRepair?: (repair: FrontmatterQuoteRepair) => void;
  onFixRepair: (kind: FrontmatterRepairKind) => void;
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
  frontmatterRepairs = [],
  isFrontmatterPreviewSettled = false,
  skillMdContent,
  onQuoteRepair,
  onFixRepair,
  onEditManually,
}: InstalledSkillHeaderProps) {
  // Notes from the skill's own copies only, as on list rows - a plugin copy's notes are not the
  // user's to fix. A plugin-only skill has no own copy, so it counts every copy.
  const own = ownDeployments(skill);
  const noteSources = own.length > 0 ? own : skill.deployments;
  const nonBlockingNotes = [
    ...new Set(
      noteSources.flatMap((d) => d.spec_violations.filter((v) => !isBlockingSpecViolation(v))),
    ),
  ];
  const nonBlockingCount = nonBlockingNotes.length;
  // The red violation line names only the deployment whose SKILL.md the page
  // actually renders - other deployments' violations show on their own
  // Locations rows instead (see `SkillLocationsCard`). Home's spec-violation
  // issue still relies on `skill.spec_violations` covering every copy.
  const renderedDeployment = deployment ?? skill.deployments.find((d) => d.content_hash);
  const blockingViolations = (renderedDeployment?.spec_violations ?? []).filter(
    isBlockingSpecViolation,
  );
  const yamlViolation = blockingViolations.find((violation) =>
    violation.startsWith("invalid YAML frontmatter at line "),
  );
  const hasMalformedYaml = yamlViolation !== undefined;
  const yamlLocation = yamlViolation ? parseYamlFrontmatterError(yamlViolation) : null;
  // Conflicting invocation keys are a non-blocking note with their own line and Fix;
  // every other repair belongs to the red violation line.
  const lineKind = frontmatterRepairKindsFor(renderedDeployment).find(
    (kind) => kind !== "invocation-conflict",
  );
  const lineRepair = frontmatterRepairs.find((repair) => repair.kind === lineKind);
  const conflictRepair = frontmatterRepairs.find((repair) => repair.kind === "invocation-conflict");
  // A backend "Fix" preview wins; the local quote repair covers what it declines.
  const canQuote = canOfferLocalQuote({
    isPreviewSettled: isFrontmatterPreviewSettled,
    hasPreview: Boolean(lineRepair),
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
          <TooltipControl content={nonBlockingNotes.join("; ")}>
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
          {(hasMalformedYaml || lineRepair) && (
            <Button
              size="sm"
              variant="outline"
              onClick={lineRepair ? () => onFixRepair(lineRepair.kind) : onEditManually}
            >
              {lineRepair ? "Fix" : "Edit manually"}
            </Button>
          )}
        </div>
      )}
      {conflictRepair && (
        <div className="flex items-center gap-2 text-small text-error">
          <AlertTriangle size={13} />
          <span>Both invocation keys are set, so nothing can run this skill.</span>
          <Button size="sm" variant="outline" onClick={() => onFixRepair("invocation-conflict")}>
            Fix
          </Button>
        </div>
      )}
    </header>
  );
}
