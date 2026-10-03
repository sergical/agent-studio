import { describe, expect, it } from "vitest";
import {
  chosenInstallHarnesses,
  harnessesKeptWithoutUniversal,
  installDestinationError,
  installDestinationFields,
  installHarness,
  installHarnessLockReason,
  installHarnessLocked,
  installMethodFor,
  offeredInstallHarnesses,
  toggleInstallHarness,
  universalDestinationPath,
  universalLockReason,
} from "./skill-install-destination";
import type { AgentId } from "./skill-types";

const ALL: readonly AgentId[] = ["claude-code", "codex", "open-code", "pi", "cursor", "grok-build"];

describe("Destination rows", () => {
  it("add skill: names the shared folder per scope, else the Universal row shows the wrong path", () => {
    expect(universalDestinationPath("global")).toBe("~/.agents/skills");
    expect(universalDestinationPath("project")).toBe(".agents/skills");
  });

  it("add skill: gives every harness its own folder per scope, else the caption promises a folder the copy does not write", () => {
    const folders = Object.fromEntries(ALL.map((id) => [id, installHarness(id)?.folder] as const));
    expect(folders).toEqual({
      "claude-code": { global: "~/.claude/skills", project: ".claude/skills" },
      codex: { global: "~/.codex/skills", project: ".codex/skills" },
      "open-code": { global: "~/.config/opencode/skills", project: ".opencode/skills" },
      pi: { global: "~/.pi/agent/skills", project: ".pi/skills" },
      cursor: { global: "~/.cursor/skills", project: ".cursor/skills" },
      "grok-build": { global: "~/.grok/skills", project: ".grok/skills" },
    });
  });

  it("add skill: offers Claude Code plus detected and kept harnesses in declaration order, else unknown ids leak in", () => {
    expect(offeredInstallHarnesses(["grok-build", "codex"], ["pi", "not-a-harness"])).toEqual([
      "claude-code",
      "codex",
      "pi",
      "grok-build",
    ]);
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

describe("Universal ticked: locked rows", () => {
  const lockedAt = (claudeReadsShared: boolean) =>
    ALL.filter((id) => installHarnessLocked(id, claudeReadsShared, true));

  it("add skill: locks every harness that reads the shared folder, else an unticked box would hide nothing", () => {
    expect(lockedAt(false)).toEqual(["codex", "open-code", "pi", "cursor", "grok-build"]);
  });

  it("add skill: also locks Claude Code when its folder points at the shared folder, else the box promises a link it cannot make", () => {
    expect(lockedAt(true)).toEqual([
      "claude-code",
      "codex",
      "open-code",
      "pi",
      "cursor",
      "grok-build",
    ]);
  });

  it("add skill: locks nothing once Universal is unticked, else a harness cannot get its own copy", () => {
    for (const claudeReadsShared of [false, true]) {
      expect(ALL.filter((id) => installHarnessLocked(id, claudeReadsShared, false))).toEqual([]);
    }
  });

  it("add skill: gives every locked row a hover reason and no other row one, else the user sees a dead checkbox", () => {
    for (const scope of ["global", "project"] as const) {
      for (const claudeReadsShared of [false, true]) {
        for (const id of ALL) {
          expect(installHarnessLockReason(id, claudeReadsShared, scope, true) !== null).toBe(
            installHarnessLocked(id, claudeReadsShared, true),
          );
        }
      }
    }
    expect(installHarnessLockReason("cursor", false, "global", true)).toBe(
      "Cursor reads ~/.agents/skills and can't hide one skill.",
    );
    expect(installHarnessLockReason("codex", false, "global", true)).toBe(
      "Codex reads ~/.agents/skills and can't hide one skill.",
    );
    expect(installHarnessLockReason("codex", false, "global", false)).toBeNull();
  });
});

describe("Universal ticked: ticks", () => {
  it("add skill: ticks every offered harness before the user picks, else the default hides a skill from a harness", () => {
    expect(chosenInstallHarnesses(ALL, null, false, true)).toEqual(ALL);
  });

  it("add skill: keeps locked harnesses ticked after a pick, else Cursor drops out of the harness list", () => {
    expect(chosenInstallHarnesses(ALL, ["claude-code"], false, true)).toEqual([
      "claude-code",
      "codex",
      "open-code",
      "pi",
      "cursor",
      "grok-build",
    ]);
  });
});

describe("Universal unticked: ticks", () => {
  it("add skill: keeps the Claude Code tick and clears every harness that reads the shared folder, else a harness gets a copy the user never asked for", () => {
    expect(harnessesKeptWithoutUniversal(ALL)).toEqual(["claude-code"]);
    expect(
      harnessesKeptWithoutUniversal(["codex", "open-code", "pi", "cursor", "grok-build"]),
    ).toEqual([]);
  });

  it("add skill: ticks only the picked harnesses, else a locked default leaks into the copy list", () => {
    expect(chosenInstallHarnesses(ALL, null, true, false)).toEqual([]);
    expect(chosenInstallHarnesses(ALL, ["codex", "pi"], true, false)).toEqual(["codex", "pi"]);
  });

  it("add skill: requires one ticked harness, else the install writes nothing", () => {
    expect(installDestinationError(false, [])).toBe("Select at least one harness.");
    expect(installDestinationError(false, ["codex"])).toBeNull();
    expect(installDestinationError(true, [])).toBeNull();
  });
});

describe("Install request", () => {
  it("add skill, Universal on: sends today's default agents, so pi and Grok Build, shown ticked, do not become link targets", () => {
    const chosen = chosenInstallHarnesses(ALL, null, false, true);
    expect(
      installDestinationFields({
        chosen,
        method: "skills-sh",
        universal: true,
      }),
    ).toEqual({
      method: "skills-sh",
      destination: "universal",
      agents: ["claude-code", "codex", "open-code", "cursor"],
      link_mode: "link",
    });
  });

  it("add skill, Universal off: sends per-harness Copy for the ticked harnesses only, else a copy lands in a folder the user left out", () => {
    const chosen = chosenInstallHarnesses(ALL, ["codex", "pi"], false, false);
    expect(
      installDestinationFields({
        chosen,
        method: "skills-sh",
        universal: false,
      }),
    ).toEqual({
      method: "copy",
      destination: "per-harness",
      agents: ["codex", "pi"],
      link_mode: "copy",
    });
  });
});

describe("Method interplay", () => {
  it("add skill: switches skills.sh and dotagents to Copy when Universal is off, else the CLI writes the shared folder anyway", () => {
    expect(installMethodFor("skills-sh", false)).toBe("copy");
    expect(installMethodFor("dotagents", false)).toBe("copy");
    expect(installMethodFor("copy", false)).toBe("copy");
  });

  it("add skill: keeps the picked method while Universal is on, else the user's method choice is overwritten", () => {
    expect(installMethodFor("skills-sh", true)).toBe("skills-sh");
    expect(installMethodFor("dotagents", true)).toBe("dotagents");
  });

  it("add skill: locks Universal only for a source with no Copy method, else a git URL loses its only way to install", () => {
    expect(universalLockReason(["dotagents"])).not.toBeNull();
    expect(universalLockReason(["skills-sh", "dotagents", "copy"])).toBeNull();
    expect(universalLockReason(["copy"])).toBeNull();
    expect(universalLockReason([])).toBeNull();
  });
});
