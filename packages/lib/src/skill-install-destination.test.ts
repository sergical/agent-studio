import { describe, expect, it } from "vitest";
import {
  chosenInstallHarnesses,
  defaultInstallHarnesses,
  installDestinationError,
  installDisabledHarnesses,
  installFolders,
  installFoldersPreview,
  installHarnessLocked,
  linkModeChoiceVisible,
  normalizeInstallHarnesses,
  offeredInstallHarnesses,
  toggleInstallHarness,
  universalDestinationPath,
} from "./skill-install-destination";

describe("skill install destination", () => {
  it("uses scope-aware Universal paths", () => {
    expect(universalDestinationPath("global")).toBe("~/.agents/skills");
    expect(universalDestinationPath("project")).toBe(".agents/skills");
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

describe("Destination selector", () => {
  it("add skill: hides Link/Copy when the choice writes only the shared folder, else a Copy choice would do nothing", () => {
    expect(linkModeChoiceVisible(["codex", "open-code", "cursor"], false)).toBe(false);
    expect(linkModeChoiceVisible([], false)).toBe(false);
    // Claude Code's whole folder already points at the shared folder: no second folder.
    expect(linkModeChoiceVisible(["claude-code"], true)).toBe(false);
  });

  it("add skill: shows Link/Copy once a harness with its own folder is chosen, else the user cannot ask for real copies", () => {
    expect(linkModeChoiceVisible(["claude-code"], false)).toBe(true);
    expect(linkModeChoiceVisible(["codex", "pi"], true)).toBe(true);
  });

  it("add skill: previews the shared folder first, then each own folder, else the user sees the wrong paths", () => {
    expect(installFolders(["claude-code", "codex", "pi"], "project", false)).toEqual([
      ".agents/skills",
      ".claude/skills",
      ".pi/skills",
    ]);
    expect(installFoldersPreview(["codex"], "global", "link", false)).toBe(
      "Writes ~/.agents/skills.",
    );
    expect(installFoldersPreview(["claude-code", "pi"], "global", "link", false)).toBe(
      "Writes ~/.agents/skills and links it in ~/.claude/skills and ~/.pi/agent/skills.",
    );
    expect(
      installFoldersPreview(["claude-code", "pi", "grok-build"], "project", "copy", false),
    ).toBe("Writes .agents/skills and copies it to .claude/skills, .pi/skills and .grok/skills.");
  });

  it("add skill: offers Claude Code plus detected and kept harnesses in declaration order, else unknown ids leak in", () => {
    expect(offeredInstallHarnesses(["grok-build", "codex"], ["pi", "not-a-harness"])).toEqual([
      "claude-code",
      "codex",
      "pi",
      "grok-build",
    ]);
  });

  it("add skill: starts without pi and Grok Build links, else a default install adds a second copy they already read", () => {
    expect(defaultInstallHarnesses(["claude-code", "codex", "pi", "cursor", "grok-build"])).toEqual(
      ["claude-code", "codex", "cursor"],
    );
  });

  it("add skill: locks Cursor, and Claude Code when it reads the shared folder, else an unchecked box would hide nothing", () => {
    expect(installHarnessLocked("cursor", false, "global")).toBe(true);
    expect(installHarnessLocked("claude-code", true, "global")).toBe(true);
    expect(installHarnessLocked("claude-code", false, "global")).toBe(false);
    expect(installHarnessLocked("codex", false, "global")).toBe(false);
    expect(installHarnessLocked("pi", false, "global")).toBe(false);
  });

  it("add skill at project scope: locks Codex and OpenCode, else an unchecked box offers a per-project off that only exists globally", () => {
    expect(installHarnessLocked("codex", false, "project")).toBe(true);
    expect(installHarnessLocked("open-code", false, "project")).toBe(true);
    expect(installHarnessLocked("pi", false, "project")).toBe(false);
    expect(installHarnessLocked("claude-code", false, "project")).toBe(false);
  });

  it("add skill: keeps locked harnesses in the request after a pick, else Cursor drops out of the harness list", () => {
    const offered = ["claude-code", "codex", "cursor"] as const;
    expect(chosenInstallHarnesses(offered, null, false, "global")).toEqual([
      "claude-code",
      "codex",
      "cursor",
    ]);
    expect(chosenInstallHarnesses(offered, ["codex"], true, "global")).toEqual([
      "claude-code",
      "codex",
      "cursor",
    ]);
  });

  it("add skill at project scope: keeps Codex in the request after the user unpicks it, else the install turns it off globally", () => {
    const offered = ["claude-code", "codex", "open-code"] as const;
    expect(chosenInstallHarnesses(offered, ["claude-code"], false, "project")).toEqual([
      "claude-code",
      "codex",
      "open-code",
    ]);
  });

  it("add skill: turns the skill off only for detected harnesses with an off switch, else unchecking Codex does nothing", () => {
    expect(
      installDisabledHarnesses(
        ["claude-code", "codex", "open-code", "pi"],
        ["claude-code"],
        "global",
      ),
    ).toEqual(["codex", "open-code"]);
  });

  it("add skill at project scope: turns nothing off, else a project install writes Codex and OpenCode's global off switch", () => {
    expect(
      installDisabledHarnesses(
        ["claude-code", "codex", "open-code", "pi"],
        ["claude-code"],
        "project",
      ),
    ).toEqual([]);
  });

  it("add skill: a toggle keeps the offered order, else the request order depends on click order", () => {
    const offered = ["claude-code", "codex", "pi"] as const;
    expect(toggleInstallHarness(offered, ["pi"], "claude-code", true)).toEqual([
      "claude-code",
      "pi",
    ]);
    expect(toggleInstallHarness(offered, ["claude-code", "pi"], "pi", false)).toEqual([
      "claude-code",
    ]);
  });
});
