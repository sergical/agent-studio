import { describe, expect, it } from "vitest";
import {
  chosenInstallHarnesses,
  harnessesKeptWithoutUniversal,
  installDestinationError,
  installDestinationFields,
  installDisabledHarnesses,
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
  const lockedAt = (scope: "global" | "project", claudeReadsShared: boolean) =>
    ALL.filter((id) => installHarnessLocked(id, claudeReadsShared, scope, true));

  it("add skill, global: locks Cursor, pi and Grok Build, else an unticked box would hide nothing", () => {
    expect(lockedAt("global", false)).toEqual(["pi", "cursor", "grok-build"]);
  });

  it("add skill, global: also locks Claude Code when its folder points at the shared folder, else the box promises a link it cannot make", () => {
    expect(lockedAt("global", true)).toEqual(["claude-code", "pi", "cursor", "grok-build"]);
  });

  it("add skill, project: also locks Codex and OpenCode, else an unticked box offers a per-project off that only exists globally", () => {
    expect(lockedAt("project", false)).toEqual([
      "codex",
      "open-code",
      "pi",
      "cursor",
      "grok-build",
    ]);
  });

  it("add skill: locks nothing once Universal is unticked, else a harness cannot get its own copy", () => {
    for (const scope of ["global", "project"] as const) {
      for (const claudeReadsShared of [false, true]) {
        expect(
          ALL.filter((id) => installHarnessLocked(id, claudeReadsShared, scope, false)),
        ).toEqual([]);
      }
    }
  });

  it("add skill: gives every locked row a hover reason and no other row one, else the user sees a dead checkbox", () => {
    for (const scope of ["global", "project"] as const) {
      for (const claudeReadsShared of [false, true]) {
        for (const id of ALL) {
          expect(installHarnessLockReason(id, claudeReadsShared, scope, true) !== null).toBe(
            installHarnessLocked(id, claudeReadsShared, scope, true),
          );
        }
      }
    }
    expect(installHarnessLockReason("cursor", false, "global", true)).toBe(
      "Cursor reads ~/.agents/skills and can't hide one skill.",
    );
    expect(installHarnessLockReason("codex", false, "global", false)).toBeNull();
  });
});

describe("Universal ticked: ticks", () => {
  it("add skill: ticks every offered harness before the user picks, else the default hides a skill from a harness", () => {
    expect(chosenInstallHarnesses(ALL, null, false, "global", true)).toEqual(ALL);
  });

  it("add skill: keeps locked harnesses ticked after a pick, else Cursor drops out of the harness list", () => {
    expect(chosenInstallHarnesses(ALL, ["claude-code"], false, "global", true)).toEqual([
      "claude-code",
      "pi",
      "cursor",
      "grok-build",
    ]);
  });

  it("add skill, project: keeps Codex ticked after the user unpicks it, else the install turns it off globally", () => {
    expect(
      chosenInstallHarnesses(
        ["claude-code", "codex", "open-code"],
        ["claude-code"],
        false,
        "project",
        true,
      ),
    ).toEqual(["claude-code", "codex", "open-code"]);
  });
});

describe("Universal unticked: ticks", () => {
  it("add skill: keeps Claude Code, Codex and OpenCode ticks and clears Cursor, pi and Grok Build, else a harness gets a copy the user never asked for", () => {
    expect(harnessesKeptWithoutUniversal(ALL)).toEqual(["claude-code", "codex", "open-code"]);
    expect(harnessesKeptWithoutUniversal(["pi", "cursor", "grok-build"])).toEqual([]);
  });

  it("add skill: keeps a user's unticked Codex unticked, else unticking Universal re-adds a harness the user left out", () => {
    expect(harnessesKeptWithoutUniversal(["claude-code", "open-code", "pi", "cursor"])).toEqual([
      "claude-code",
      "open-code",
    ]);
  });

  it("add skill: ticks only the picked harnesses, else a locked default leaks into the copy list", () => {
    expect(chosenInstallHarnesses(ALL, null, true, "global", false)).toEqual([]);
    expect(chosenInstallHarnesses(ALL, ["codex", "pi"], true, "project", false)).toEqual([
      "codex",
      "pi",
    ]);
  });

  it("add skill: requires one ticked harness, else the install writes nothing", () => {
    expect(installDestinationError(false, [])).toBe("Select at least one harness.");
    expect(installDestinationError(false, ["codex"])).toBeNull();
    expect(installDestinationError(true, [])).toBeNull();
  });
});

describe("Install request", () => {
  it("add skill, Universal on: sends today's default agents, so pi and Grok Build, shown ticked, do not become link targets", () => {
    const chosen = chosenInstallHarnesses(ALL, null, false, "global", true);
    expect(
      installDestinationFields({
        offered: ALL,
        chosen,
        scope: "global",
        method: "skills-sh",
        universal: true,
      }),
    ).toEqual({
      method: "skills-sh",
      destination: "universal",
      agents: ["claude-code", "codex", "open-code", "cursor"],
      disabled_harnesses: [],
      link_mode: "link",
    });
  });

  it("add skill, Universal on: writes an off switch for an offered Codex the user unticked, even when it was not detected", () => {
    const offered = ["claude-code", "codex", "open-code"] as const;
    const chosen = chosenInstallHarnesses(
      offered,
      ["claude-code", "open-code"],
      false,
      "global",
      true,
    );
    expect(
      installDestinationFields({
        offered,
        chosen,
        scope: "global",
        method: "copy",
        universal: true,
      }).disabled_harnesses,
    ).toEqual(["codex"]);
  });

  it("add skill, Universal off: sends per-harness Copy for the ticked harnesses only, else a copy lands in a folder the user left out", () => {
    const chosen = chosenInstallHarnesses(ALL, ["codex", "pi"], false, "global", false);
    expect(
      installDestinationFields({
        offered: ALL,
        chosen,
        scope: "global",
        method: "skills-sh",
        universal: false,
      }),
    ).toEqual({
      method: "copy",
      destination: "per-harness",
      agents: ["codex", "pi"],
      disabled_harnesses: [],
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

describe("Off switches", () => {
  it("add skill: turns the skill off only for offered harnesses with an off switch, else unchecking Codex does nothing", () => {
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
});
