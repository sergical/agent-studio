// ============================================================================
// Skill Studio - skill-page-actions tests
//
// `useSkillPageActions` is a React hook with Tauri side effects, so like the
// sibling `skill-location-status` module it exposes its testable logic as a
// pure function. `shouldOfferHeaderRemove` is the gate the header's "Remove"
// item relies on: it must surface Remove only for skills.sh skills that have
// a global deployment, since the header's Remove is global-only (the page
// has no scope picker) and `removeSkill(name, null)` can only target a global
// deployment. Project-only skills.sh skills must not get a header Remove -
// their removal lives in the Locations card's scope-aware flow instead.
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import { shouldOfferHeaderRemove } from "./skill-page-actions";

function fixtureDeployment(overrides: Partial<Deployment> = {}): Deployment {
  return {
    agent: "shared",
    scope: "global",
    path: "/home/.agents/skills/find-bugs",
    is_symlink: false,
    symlink_is_broken: false,
    content_hash: "abc",
    disabled: false,
    ...overrides,
  };
}

function fixtureSkill(overrides: Partial<InstalledSkill> = {}): InstalledSkill {
  return {
    name: "find-bugs",
    source: "getsentry/find-bugs",
    source_type: "github",
    installed_at: "2026-01-01T00:00:00Z",
    has_update: false,
    source_kind: "dotagents",
    deployments: [fixtureDeployment()],
    has_spec: true,
    spec_violations: [],
    skill_md_tokens: 0,
    description_tokens: 0,
    folder_bytes: 0,
    file_count: 0,
    content_hash: "",
    content_hashes: [],
    frontmatter_fields: {},
    folder_truncated: false,
    parked: false,
    invocation: "both",
    ...overrides,
  };
}

describe("shouldOfferHeaderRemove", () => {
  it("offers Remove for a skills.sh skill with a global deployment", () => {
    const skill = fixtureSkill({ source_kind: "skills-sh" });
    expect(shouldOfferHeaderRemove(skill)).toBe(true);
  });

  it("offers Remove for a skills.sh skill with both a global and a project deployment", () => {
    const skill = fixtureSkill({
      source_kind: "skills-sh",
      deployments: [
        fixtureDeployment({ scope: "global", path: "/home/.agents/skills/find-bugs" }),
        fixtureDeployment({
          scope: "project",
          project_path: "/repo",
          path: "/repo/.agents/skills/find-bugs",
        }),
      ],
    });
    expect(shouldOfferHeaderRemove(skill)).toBe(true);
  });

  it("finds the global deployment regardless of where it sits in the list", () => {
    const skill = fixtureSkill({
      source_kind: "skills-sh",
      deployments: [
        fixtureDeployment({
          scope: "project",
          project_path: "/repo",
          path: "/repo/.agents/skills/find-bugs",
        }),
        fixtureDeployment({ scope: "global", path: "/home/.agents/skills/find-bugs" }),
      ],
    });
    expect(shouldOfferHeaderRemove(skill)).toBe(true);
  });

  // The reported bug: a project-only skills.sh skill reached the page with
  // source_kind "skills-sh" and no global deployment, and the header surfaced
  // a Remove whose global-only operation could not remove it. The header must
  // now hide Remove for this case - the Locations card handles it instead.
  it("does not offer Remove for a project-only skills.sh skill (the bug case)", () => {
    const skill = fixtureSkill({
      source_kind: "skills-sh",
      deployments: [
        fixtureDeployment({
          scope: "project",
          project_path: "/repo",
          path: "/repo/.agents/skills/find-bugs",
        }),
      ],
    });
    expect(shouldOfferHeaderRemove(skill)).toBe(false);
  });

  it("does not offer Remove for a skills.sh skill installed across several projects but not globally", () => {
    const skill = fixtureSkill({
      source_kind: "skills-sh",
      deployments: [
        fixtureDeployment({
          scope: "project",
          project_path: "/repo-a",
          path: "/repo-a/.agents/skills/find-bugs",
        }),
        fixtureDeployment({
          scope: "project",
          project_path: "/repo-b",
          path: "/repo-b/.agents/skills/find-bugs",
        }),
      ],
    });
    expect(shouldOfferHeaderRemove(skill)).toBe(false);
  });

  it("does not offer Remove for a lock-only skills.sh skill (no deployments)", () => {
    const skill = fixtureSkill({ source_kind: "skills-sh", deployments: [] });
    expect(shouldOfferHeaderRemove(skill)).toBe(false);
  });

  it("does not offer Remove when the only deployment is a plugin-scoped one", () => {
    const skill = fixtureSkill({
      source_kind: "skills-sh",
      deployments: [
        fixtureDeployment({
          scope: "plugin",
          agent: "Codex",
          path: "/home/.codex/plugins/cache/openai-templates/skills/find-bugs",
        }),
      ],
    });
    expect(shouldOfferHeaderRemove(skill)).toBe(false);
  });

  it("does not offer Remove when the only deployment is parked", () => {
    const skill = fixtureSkill({
      source_kind: "skills-sh",
      deployments: [
        fixtureDeployment({ scope: "parked", path: "/home/.agents/skills-parked/find-bugs" }),
      ],
    });
    expect(shouldOfferHeaderRemove(skill)).toBe(false);
  });

  // A broken or unreadable global link is still a global deployment - the
  // backend's global removal targets the lock entry / install directory, not
  // the link's target, so the header Remove stays valid for it.
  it("still offers Remove when the global deployment is a broken symlink", () => {
    const skill = fixtureSkill({
      source_kind: "skills-sh",
      deployments: [
        fixtureDeployment({
          scope: "global",
          is_symlink: true,
          symlink_is_broken: true,
          path: "/home/.claude/skills/find-bugs",
        }),
      ],
    });
    expect(shouldOfferHeaderRemove(skill)).toBe(true);
  });

  // Every non-skills.sh source_kind must never get a header Remove, even with
  // a global deployment on disk - they are not lock-file tracked, so the
  // skills.sh removal the header triggers does not apply.
  it.each([["dotagents"], ["fork"], ["plugin"], ["in-repo"], ["manual"]] as const)(
    "does not offer Remove for a %s skill with a global deployment",
    (source_kind) => {
      const skill = fixtureSkill({
        source_kind,
        deployments: [fixtureDeployment({ scope: "global" })],
      });
      expect(shouldOfferHeaderRemove(skill)).toBe(false);
    },
  );

  it("does not mutate the skill passed in", () => {
    const skill = fixtureSkill({ source_kind: "skills-sh" });
    const snapshot = structuredClone(skill);
    shouldOfferHeaderRemove(skill);
    expect(skill).toEqual(snapshot);
  });
});
