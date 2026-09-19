import { describe, expect, it } from "vitest";
import {
  installDestinationError,
  normalizeInstallHarnesses,
  perHarnessDestinationPath,
  universalDestinationPath,
} from "./skill-install-destination";

describe("skill install destination", () => {
  it("uses scope-aware Universal paths", () => {
    expect(universalDestinationPath("global")).toBe("~/.agents/skills");
    expect(universalDestinationPath("project")).toBe(".agents/skills");
  });

  it("covers every selectable Per harness path", () => {
    expect(perHarnessDestinationPath("claude-code", "project")).toBe(".claude/skills");
    expect(perHarnessDestinationPath("codex", "global")).toBe("~/.codex/skills");
    expect(perHarnessDestinationPath("open-code", "global")).toBe("~/.config/opencode/skills");
    expect(perHarnessDestinationPath("pi", "project")).toBe(".pi/skills");
    expect(perHarnessDestinationPath("cursor", "project")).toBe(".cursor/skills");
    expect(perHarnessDestinationPath("grok-build", "global")).toBe("~/.grok/skills");
  });

  it("keeps only Claude as an optional Universal link", () => {
    expect(normalizeInstallHarnesses("universal", ["codex", "claude-code"])).toEqual([
      "claude-code",
    ]);
  });

  it("keeps selected Per harness copies independent and in display order", () => {
    expect(normalizeInstallHarnesses("per-harness", ["pi", "claude-code", "pi"])).toEqual([
      "claude-code",
      "pi",
    ]);
  });

  it("requires at least one Per harness copy", () => {
    expect(installDestinationError("per-harness", [])).toBe("Select at least one harness.");
    expect(installDestinationError("per-harness", ["codex"])).toBeNull();
    expect(installDestinationError("universal", [])).toBeNull();
  });
});
