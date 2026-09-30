// ============================================================================
// Skill Studio - malformed frontmatter repair presentation policy
// ============================================================================

import type { Deployment, FrontmatterRepairPreview } from "@skill-studio/lib";

export function hasMalformedYamlWarning(
  deployment: Pick<Deployment, "spec_violations"> | undefined,
): boolean {
  return Boolean(
    deployment?.spec_violations?.some((violation) =>
      violation.startsWith("invalid YAML frontmatter at line "),
    ),
  );
}

/** One backend preview per file state: the deployment plus the bytes last scanned for it. */
export function frontmatterPreviewKey(
  deployment: Pick<Deployment, "id" | "content_hash"> | undefined,
): string | null {
  return deployment ? `${deployment.id}\u0000${deployment.content_hash}` : null;
}

/**
 * The local "Quote" button waits for the backend preview to settle and yields
 * to it, so the user never sees "Quote" turn into "Fix".
 */
export function canOfferLocalQuote(state: {
  isPreviewSettled: boolean;
  hasPreview: boolean;
}): boolean {
  return state.isPreviewSettled && !state.hasPreview;
}

export function frontmatterRepairActionLabels(
  preview: Pick<FrontmatterRepairPreview, "allowed_apply_modes">,
): string[] {
  return preview.allowed_apply_modes.map((mode) => {
    if (mode === "fork-and-fix") return "Fork and fix";
    if (mode === "fix-installed-copy") return "Fix installed copy";
    return "Apply fix";
  });
}
