// ============================================================================
// Skill Studio - malformed frontmatter repair presentation policy tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, FrontmatterRepairPreview } from "@skill-studio/lib";
import {
  frontmatterRepairActionLabels,
  hasMalformedYamlWarning,
} from "./skill-frontmatter-repair-policy";

const deployment = {
  spec_violations: [
    "invalid YAML frontmatter at line 3, column 24: mapping values are not allowed",
  ],
} satisfies Pick<Deployment, "spec_violations">;

function preview(
  modes: FrontmatterRepairPreview["allowed_apply_modes"],
): Pick<FrontmatterRepairPreview, "allowed_apply_modes"> {
  return { allowed_apply_modes: modes };
}

describe("malformed frontmatter repair policy", () => {
  it("uses the selected deployment warning", () => {
    expect(hasMalformedYamlWarning(deployment)).toBe(true);
    expect(hasMalformedYamlWarning({ ...deployment, spec_violations: [] })).toBe(false);
  });

  it("presents only backend-authorized actions", () => {
    expect(frontmatterRepairActionLabels(preview(["fork-and-fix", "fix-installed-copy"]))).toEqual([
      "Fork and fix",
      "Fix installed copy",
    ]);
    expect(frontmatterRepairActionLabels(preview(["apply-fix"]))).toEqual(["Apply fix"]);
    expect(frontmatterRepairActionLabels(preview([]))).toEqual([]);
  });
});
