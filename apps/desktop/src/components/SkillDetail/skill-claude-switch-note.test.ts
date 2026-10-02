import { describe, expect, it } from "vitest";
import type { Deployment } from "@skill-studio/lib";
import {
  CLAUDE_CODE_SWITCH_NOTE,
  claudeCodeProjectNames,
  claudeCodeSwitchToast,
} from "./skill-claude-switch-note";

function deployment(agent: string, projectPath: string | null): Deployment {
  // SAFETY: the note reads only `agent` and `project_path`, both set here.
  return { agent, project_path: projectPath } as Deployment;
}

describe("claudeCodeSwitchToast", () => {
  it("turning a skill off names each project that has its own Claude Code copy, once", () => {
    const toast = claudeCodeSwitchToast(
      {
        name: "tdd",
        deployments: [
          deployment("Claude Code", "/Users/me/src/alpha"),
          deployment("Claude Code", "/Users/me/src/alpha"),
          deployment("Claude Code", "/Users/me/src/beta"),
        ],
      },
      false,
    );
    expect(toast.title).toBe("tdd is off for Claude Code");
    expect(toast.message).toBe(
      "Claude Code turns skills off by name, so this also applies in alpha, beta.",
    );
  });

  it("a skill with no project copy gets the note without a project list", () => {
    const toast = claudeCodeSwitchToast(
      { name: "tdd", deployments: [deployment("Claude Code", null)] },
      true,
    );
    expect(toast.title).toBe("tdd is on for Claude Code");
    expect(toast.message).toBe(CLAUDE_CODE_SWITCH_NOTE);
  });
});

describe("claudeCodeProjectNames", () => {
  it("project copies for other harnesses are not named", () => {
    expect(
      claudeCodeProjectNames([
        deployment("Codex", "/Users/me/src/alpha"),
        deployment("Claude Code", "/Users/me/src/beta"),
      ]),
    ).toEqual(["beta"]);
  });
});
