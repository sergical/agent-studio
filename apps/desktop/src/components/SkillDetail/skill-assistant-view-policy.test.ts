// ============================================================================
// Skill Studio - Assistant view policy tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { nextSkillAssistantPanelMode } from "./skill-assistant-view-policy";

describe("nextSkillAssistantPanelMode", () => {
  it("resets Runs when the Assistant drawer closes or opens", () => {
    expect(nextSkillAssistantPanelMode("runs", "close-assistant")).toBe("assistant");
    expect(nextSkillAssistantPanelMode("runs", "open-assistant")).toBe("assistant");
  });

  it("opens Runs only for the explicit Runs action", () => {
    expect(nextSkillAssistantPanelMode("assistant", "open-runs")).toBe("runs");
    expect(nextSkillAssistantPanelMode("runs", "close-runs")).toBe("assistant");
  });
});
