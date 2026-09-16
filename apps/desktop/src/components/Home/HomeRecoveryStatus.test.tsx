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
