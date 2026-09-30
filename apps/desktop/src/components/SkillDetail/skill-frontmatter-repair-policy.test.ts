// ============================================================================
// Skill Studio - malformed frontmatter repair presentation policy tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, FrontmatterRepairPreview } from "@skill-studio/lib";
import {
  canOfferLocalQuote,
  frontmatterPreviewKey,
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

describe("frontmatterPreviewKey", () => {
  /**
   * Flow: the page re-renders with a new deployment object for the same file.
   * Expect: the same key, so no new backend preview starts.
   * Failure: one preview per render, which queued 15 rescans in 6 minutes.
   */
  it("is stable for the same id and content hash, and changes with the content", () => {
    const same = frontmatterPreviewKey({ id: "d1", content_hash: "h1" });
    expect(frontmatterPreviewKey({ id: "d1", content_hash: "h1" })).toBe(same);
    expect(frontmatterPreviewKey({ id: "d1", content_hash: "h2" })).not.toBe(same);
    expect(frontmatterPreviewKey({ id: "d2", content_hash: "h1" })).not.toBe(same);
    expect(frontmatterPreviewKey(undefined)).toBeNull();
  });
});

describe("canOfferLocalQuote", () => {
  const backendPreview = { deployment_id: "d1" } satisfies Partial<FrontmatterRepairPreview>;

  /**
   * Flow: the backend preview is still loading.
   * Expect: no local Quote button yet.
   * Failure: "Quote" appears, then turns into "Fix" when the preview lands.
   */
  it("waits while the backend preview is loading", () => {
    expect(canOfferLocalQuote({ isPreviewSettled: false, hasPreview: false })).toBe(false);
  });

  /**
   * Flow: the backend preview settled with a repair.
   * Expect: no local Quote button; the backend Fix takes over.
   * Failure: two competing repair buttons.
   */
  it("yields to a backend preview", () => {
    expect(
      canOfferLocalQuote({ isPreviewSettled: true, hasPreview: Boolean(backendPreview) }),
    ).toBe(false);
  });

  /**
   * Flow: the backend preview settled with an error or no repair.
   * Expect: the local Quote button may show.
   * Failure: a fixable file never gets a repair button.
   */
  it("offers Quote once the backend preview settled empty", () => {
    expect(canOfferLocalQuote({ isPreviewSettled: true, hasPreview: false })).toBe(true);
  });
});
