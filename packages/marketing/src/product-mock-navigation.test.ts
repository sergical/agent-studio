// ============================================================================
// Skill Studio marketing demo - detail navigation label tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { productMockDetailBackLabel } from "./product-mock-navigation";

describe("productMockDetailBackLabel", () => {
  it("returns Plugins for detail opened from Plugins", () => {
    expect(productMockDetailBackLabel("plugins")).toBe("Plugins");
  });

  it("keeps the existing labels for other root views", () => {
    expect(productMockDetailBackLabel("home")).toBe("Home");
    expect(productMockDetailBackLabel("skills")).toBe("Skills");
    expect(productMockDetailBackLabel("activity")).toBe("Activity");
  });
});
