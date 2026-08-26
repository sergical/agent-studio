// ============================================================================
// Skill Studio - skill-coverage tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { AGENT_MATRIX_LABELS, agentIdFromDeploymentLabel } from "./skill-coverage";

describe("agentIdFromDeploymentLabel", () => {
  it("maps Cursor to cursor", () => {
    expect(agentIdFromDeploymentLabel("Cursor")).toBe("cursor");
  });

  it("maps Grok Build to grok-build", () => {
    expect(agentIdFromDeploymentLabel("Grok Build")).toBe("grok-build");
  });
});

describe("AGENT_MATRIX_LABELS", () => {
  it("includes Cursor and Grok Build alongside the original four", () => {
    expect(AGENT_MATRIX_LABELS).toEqual([
      "Claude Code",
      "Codex",
      "OpenCode",
      "pi",
      "Cursor",
      "Grok Build",
    ]);
  });
});
