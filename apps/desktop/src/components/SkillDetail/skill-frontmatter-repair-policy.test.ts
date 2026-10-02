// ============================================================================
// Skill Studio - malformed frontmatter repair presentation policy tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, FrontmatterRepairPreview } from "@skill-studio/lib";
import {
  canOfferLocalQuote,
  frontmatterPreviewKey,
  frontmatterRepairCopy,
  frontmatterRepairKindForViolation,
  frontmatterRepairKindsFor,
  frontmatterRepairActionLabels,
  fixLineFor,
  hasMalformedYamlWarning,
} from "./skill-frontmatter-repair-policy";
import violationMessages from "../../../../../crates/skill-studio-core/tests/fixtures/frontmatter-violation-messages.json";

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

describe("frontmatterRepairKindsFor", () => {
  const kindsFor = (...spec_violations: string[]) => frontmatterRepairKindsFor({ spec_violations });

  /**
   * Flow: the scanner reports each fixable violation.
   * Expect: the matching repair kind.
   * Failure: a Fix button asks the backend for the wrong repair, which refuses it.
   */
  it("maps each fixable violation to its repair kind", () => {
    expect(kindsFor("invalid YAML frontmatter at line 3, column 1: x")).toEqual(["colon-scalar"]);
    expect(kindsFor('name "Foo" does not match its directory name "foo"')).toEqual([
      "name-mismatch",
    ]);
    expect(
      kindsFor('name "Foo" must be 1-64 lowercase a-z0-9 characters and hyphens, with no leading'),
    ).toEqual(["name-format"]);
    expect(kindsFor("conflicting invocation keys")).toEqual(["invocation-conflict"]);
  });

  /**
   * Flow: a skill has a name problem and the invocation conflict.
   * Expect: both fixes, so the conflict stays reachable when the name fix is refused.
   * Failure: one priority-picked kind hides the other, and a refused name fix
   * leaves the conflict with no Fix at all.
   */
  it("offers the name fix and the invocation conflict fix together", () => {
    expect(
      kindsFor("conflicting invocation keys", 'name "a" does not match its directory name "b"'),
    ).toEqual(["invocation-conflict", "name-mismatch"]);
  });

  /**
   * Flow: broken YAML hides the other checks.
   * Expect: only the YAML fix.
   * Failure: a name or conflict fix is previewed against content that does not parse.
   */
  it("offers only the YAML fix when the frontmatter does not parse", () => {
    expect(
      kindsFor(
        "conflicting invocation keys",
        "invalid YAML frontmatter at line 3, column 1: x",
        'name "a" does not match its directory name "b"',
      ),
    ).toEqual(["colon-scalar"]);
  });

  /**
   * Flow: the skill has only violations no repair handles.
   * Expect: no kind, so no preview is requested.
   * Failure: every skill with a spec note triggers a backend preview.
   */
  it("returns no kinds when no repair applies", () => {
    expect(kindsFor("description exceeds 1024 characters")).toEqual([]);
    expect(frontmatterRepairKindsFor(undefined)).toEqual([]);
  });
});

describe("violation messages shared with the Rust matchers", () => {
  /**
   * Flow: `validate_skill` wording for each repairable violation, captured in
   * the fixture the Rust tests also assert against.
   * Expect: each message maps to its kind.
   * Failure: one side rewords a message and the Fix button silently disappears.
   */
  it.each(Object.entries(violationMessages))("maps the %s message to its kind", (kind, message) => {
    expect(frontmatterRepairKindForViolation(message)).toBe(kind);
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

describe("fixLineFor", () => {
  it("puts_the_name_fix_on_the_yellow_mismatch_line_not_the_red_missing_field_line_or_the_button_opens_a_preview_that_does_not_match_the_line", () => {
    expect(
      fixLineFor({ lineKind: "name-mismatch", hasMismatchLine: true, hasNameFormatNote: false }),
    ).toBe("warning");
  });

  it("puts_the_name_format_fix_on_the_grey_note_when_an_error_exists_and_no_mismatch_line_shows_or_the_error_line_steals_it", () => {
    expect(
      fixLineFor({ lineKind: "name-format", hasMismatchLine: false, hasNameFormatNote: true }),
    ).toBe("note");
  });

  it("puts_the_name_format_fix_on_the_mismatch_line_when_both_show_or_the_button_renders_twice", () => {
    expect(
      fixLineFor({ lineKind: "name-format", hasMismatchLine: true, hasNameFormatNote: true }),
    ).toBe("warning");
  });

  it("puts_the_yaml_fix_on_the_red_line_or_malformed_yaml_loses_its_repair", () => {
    expect(
      fixLineFor({ lineKind: "colon-scalar", hasMismatchLine: false, hasNameFormatNote: false }),
    ).toBe("error");
  });

  it("offers_no_fix_line_without_a_repair_kind_or_without_a_line_to_hold_it", () => {
    expect(
      fixLineFor({ lineKind: undefined, hasMismatchLine: true, hasNameFormatNote: true }),
    ).toBeNull();
    expect(
      fixLineFor({ lineKind: "name-mismatch", hasMismatchLine: false, hasNameFormatNote: false }),
    ).toBeNull();
  });
});
