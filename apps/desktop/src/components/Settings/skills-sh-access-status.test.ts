// ============================================================================
// Skill Studio - skills.sh access status tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { skillsShAccessStatusText } from "./skills-sh-access-status";

describe("skillsShAccessStatusText", () => {
  it("shows no authoritative status while loading", () => {
    expect(skillsShAccessStatusText({ kind: "loading" })).toBeNull();
  });

  it("shows an explicit unavailable state after a failed read", () => {
    expect(skillsShAccessStatusText({ kind: "unavailable" })).toBe(
      "skills.sh access status is unavailable.",
    );
  });

  it("keeps successful direct and server copy unchanged", () => {
    expect(
      skillsShAccessStatusText({
        kind: "available",
        access: { mode: "direct", server_url: null },
      }),
    ).toBe("Using a local skills.sh key (developer override)");
    expect(
      skillsShAccessStatusText({
        kind: "available",
        access: { mode: "server", server_url: "http://127.0.0.1:8787/api/v1" },
      }),
    ).toBe("Browsing through the Skill Studio server at http://127.0.0.1:8787/api/v1");
  });

  it("never formats an invalid server response as at null", () => {
    expect(
      skillsShAccessStatusText({
        kind: "available",
        access: { mode: "server", server_url: null },
      }),
    ).toBe("skills.sh access status is unavailable.");
  });
});
