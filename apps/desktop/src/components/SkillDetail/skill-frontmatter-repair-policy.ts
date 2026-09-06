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

export function frontmatterRepairActionLabels(
  preview: Pick<FrontmatterRepairPreview, "allowed_apply_modes">,
): string[] {
  return preview.allowed_apply_modes.map((mode) => {
    if (mode === "fork-and-fix") return "Fork and fix";
    if (mode === "fix-installed-copy") return "Fix installed copy";
    return "Apply fix";
  });
}
