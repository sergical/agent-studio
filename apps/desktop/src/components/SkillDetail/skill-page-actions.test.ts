// ============================================================================
// Skill Studio - pullUpstreamToast tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { PullResult } from "@skill-studio/lib";

import { pullUpstreamToast, removeSuccessToast } from "./skill-page-actions";

function fixtureResult(overrides: Partial<PullResult> = {}): PullResult {
  return {
    from_commit: "aaa",
    to_commit: "bbb",
    merged: [],
    conflicts: [],
    added: [],
    removed: [],
    unchanged: 0,
    message: null,
    ...overrides,
  };
}

describe("pullUpstreamToast", () => {
  it("appends the F2 editor-open failure message to the conflict toast, or names the dropped message", () => {
    const result = fixtureResult({
      conflicts: ["skill.md"],
      message: "Conflict markers written to skill.md; could not open editor: no editor found",
    });

    const toast = pullUpstreamToast(result);

    expect(toast.type).toBe("warning");
    expect(toast.title).toBe("1 conflicts — open the editor to resolve");
    expect(toast.message).toBe(
      "skill.md Conflict markers written to skill.md; could not open editor: no editor found",
    );
  });

  it("shows the plain conflict list when the editor opened fine, or names the missing conflict path", () => {
    const result = fixtureResult({ conflicts: ["a.md", "b.md"] });

    const toast = pullUpstreamToast(result);

    expect(toast.message).toBe("a.md, b.md");
  });

  it("shows the already-up-to-date message when there are no conflicts, or names the lost message", () => {
    const result = fixtureResult({ message: "Already up to date" });

    const toast = pullUpstreamToast(result);

    expect(toast).toEqual({ type: "info", title: "Already up to date" });
  });

  it("counts merged, added, and removed files on a clean pull, or names the miscounted total", () => {
    const result = fixtureResult({ merged: ["a.md"], added: ["b.md"], removed: ["c.md"] });

    const toast = pullUpstreamToast(result);

    expect(toast).toEqual({ type: "success", title: "Updated 3 files" });
  });
});

describe("removeSuccessToast", () => {
  it("the_remove_success_toast_reads_removed_not_updated_n_deployments", () => {
    const toast = removeSuccessToast("find-bugs");

    expect(toast).toEqual({ type: "success", title: "Removed", message: "find-bugs" });
    expect(toast.title).not.toMatch(/updated/i);
    expect(toast.title).not.toMatch(/deployments/i);
  });
});
