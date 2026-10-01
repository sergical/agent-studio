// ============================================================================
// Skill Studio - malformed frontmatter repair presentation policy tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, FrontmatterRepairPreview } from "@skill-studio/lib";
import {
  canOfferLocalQuote,
  frontmatterPreviewKey,
  frontmatterRepairCopy,
  frontmatterRepairKindFor,
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

describe("frontmatterRepairKindFor", () => {
  const kindFor = (...spec_violations: string[]) => frontmatterRepairKindFor({ spec_violations });

  /**
   * Flow: the scanner reports each fixable violation.
   * Expect: the matching repair kind.
   * Failure: a Fix button asks the backend for the wrong repair, which refuses it.
   */
  it("maps each fixable violation to its repair kind", () => {
    expect(kindFor("invalid YAML frontmatter at line 3, column 1: x")).toBe("colon-scalar");
    expect(kindFor('name "Foo" does not match its directory name "foo"')).toBe("name-mismatch");
    expect(
      kindFor('name "Foo" must be 1-64 lowercase a-z0-9 characters and hyphens, with no leading'),
    ).toBe("name-format");
    expect(kindFor("conflicting invocation keys")).toBe("invocation-conflict");
  });

  /**
   * Flow: a skill has a name problem and the invocation conflict.
   * Expect: the name fix first.
   * Failure: the conflict dialog opens while the name stays wrong.
   */
  it("offers the name fix before the invocation conflict", () => {
    expect(
      kindFor("conflicting invocation keys", 'name "a" does not match its directory name "b"'),
    ).toBe("name-mismatch");
  });

  /**
   * Flow: the skill has only violations no repair handles.
   * Expect: no kind, so no preview is requested.
   * Failure: every skill with a spec note triggers a backend preview.
   */
  it("returns null when no repair applies", () => {
    expect(kindFor("description exceeds 1024 characters")).toBeNull();
    expect(frontmatterRepairKindFor(undefined)).toBeNull();
  });
});

describe("frontmatterRepairCopy", () => {
  /**
   * Flow: a repair succeeds.
   * Expect: the toast names the thing fixed.
   * Failure: every kind says "YAML fixed".
   */
  it("names the fixed thing per kind", () => {
    expect(frontmatterRepairCopy("colon-scalar").success).toBe("YAML fixed");
    expect(frontmatterRepairCopy("name-mismatch").success).toBe("Name fixed");
    expect(frontmatterRepairCopy("name-format").success).toBe("Name fixed");
    expect(frontmatterRepairCopy("invocation-conflict").success).toBe("Invocation fixed");
  });
});
