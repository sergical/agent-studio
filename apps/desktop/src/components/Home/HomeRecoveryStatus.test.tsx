import type { SkillSnapshot } from "@skill-studio/lib";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { HomeRecoveryStatus, canShowAllClear } from "./HomeRecoveryStatus";
import { HomeView } from "./HomeView";

describe("Home recovery status", () => {
  it("warns and links to Activity when an interrupted event needs review", () => {
    const markup = renderToStaticMarkup(
      <HomeRecoveryStatus
        status={{ kind: "ready", hasInterrupted: true }}
        onRetry={() => undefined}
        onViewActivity={() => undefined}
      />,
    );

    expect(markup).toContain("Some skill changes need review.");
    expect(markup).toContain("View Activity");
    expect(canShowAllClear(false, { kind: "ready", hasInterrupted: true })).toBe(false);
  });

  it("withholds All clear while recovery status is loading or unavailable", () => {
    expect(canShowAllClear(false, { kind: "loading" })).toBe(false);
    expect(canShowAllClear(false, { kind: "unavailable", error: "store unavailable" })).toBe(false);
    expect(canShowAllClear(false, { kind: "ready", hasInterrupted: false })).toBe(true);
  });
});

describe("Home without an inventory snapshot", () => {
  it.each([true, false])("keeps recovery visible when isLoading is %s", (isLoading) => {
    const markup = renderToStaticMarkup(
      <HomeView
        snapshot={undefined}
        isLoading={isLoading}
        onSelectSkill={() => undefined}
        recoveryStatus={{ kind: "ready", hasInterrupted: true }}
        retryRecoveryStatus={() => undefined}
      />,
    );
    expect(markup).toContain("Some skill changes need review.");
    expect(markup).toContain("View Activity");
    expect(markup).not.toContain("All clear");
    expect(markup).toContain(isLoading ? "Scanning installed skills" : "No skill snapshot yet");
  });
});

describe("Home warning preview", () => {
  it("bounds missing-record rows and includes the full count in Show all", () => {
    const snapshot: SkillSnapshot = {
      revision: 1,
      skills: [],
      projects: [],
      invocations: [],
      heatmap: { days: {} },
      scanned_at: "before",
      last_test_by_skill: {},
      update_check: { checked_at: null, gh_status: "ok", message: null, updates_available: 0 },
      diagnosis: {
        scope: { home: "/fixture", projects: [], backing_roots: [], plugin_ownership_roots: [] },
        completeness: "complete",
        extent: "full",
        issues: Array.from({ length: 100 }, (_, index) => ({
          kind: "ledger-only",
          absence: "confirmed-absent",
          owner: {
            owner_id: `owner:${index}`,
            name: `missing-record-${index}`,
            scope: "global",
            project_path: null,
            owner_kind: "skills-sh",
            sources: [],
          },
        })),
      },
    };
    const markup = renderToStaticMarkup(
      <HomeView
        snapshot={snapshot}
        isLoading={false}
        onSelectSkill={() => undefined}
        recoveryStatus={{ kind: "ready", hasInterrupted: false }}
        retryRecoveryStatus={() => undefined}
      />,
    );
    expect(markup.match(/<details/g)).toHaveLength(6);
    expect(markup).toContain("missing-record-5");
    expect(markup).not.toContain("missing-record-6");
    expect(markup).toContain("Show all");
    expect(markup).toContain("100");
    expect(markup).not.toContain("All clear");
  });
});
