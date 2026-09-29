// ============================================================================
// Skill Studio - scanned-deployment tests
// Each layout is one the scanner meets on real machines. The builders must
// produce rows `scannerMismatches` accepts, and `scannerMismatches`
// must name a row the scanner cannot produce, so fixtures built on them stay
// honest.
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment } from "@skill-studio/lib";
import {
  perSkillLinkDeployment,
  realCopyDeployment,
  scannerMismatches,
  universalDeployment,
  wholeFolderDeployment,
} from "./scanned-deployment";
import { buildHarnessSnapshot } from "./skill-fixture";

const HOME = "/Users/demo";
const UNIVERSAL = `${HOME}/.agents/skills/find-bugs`;
const CLAUDE_ENTRY = `${HOME}/.claude/skills/find-bugs`;

function expectScannerProducible(layout: string, deployments: Deployment[]) {
  for (const deployment of deployments) {
    expect(
      scannerMismatches(deployment),
      `${layout}: ${deployment.agent} row at ${deployment.path}`,
    ).toEqual([]);
  }
}

describe("scanned-deployment layouts", () => {
  it("real_claude_skills_folder_with_a_per_skill_link_builds_scanner_shapes_or_names_the_field", () => {
    const universal = universalDeployment({ universalPath: UNIVERSAL });
    const claude = perSkillLinkDeployment({
      agent: "Claude Code",
      path: CLAUDE_ENTRY,
      universalPath: UNIVERSAL,
    });
    expectScannerProducible("per-skill link", [universal, claude]);
    expect(claude.backing).toEqual({ kind: "linked-to", deployment_id: universal.id });

    // A link to the Universal folder that forgot its target reads as an
    // independent entry to the scanner, so a linked-to backing is impossible.
    const withoutTarget = { ...claude, symlink_target: null, resolved_path: null };
    expect(scannerMismatches(withoutTarget).join("\n")).toMatch(/^backing is/m);
  });

  it("real_claude_copy_builds_scanner_shapes_or_names_the_field", () => {
    const universal = universalDeployment({ universalPath: UNIVERSAL });
    const claude = realCopyDeployment({ agent: "Claude Code", path: CLAUDE_ENTRY });
    expectScannerProducible("real copy", [universal, claude]);
    expect(claude.destination).toBe("per-harness");
    expect(claude.backing).toEqual({ kind: "independent" });

    // A real folder has no link fields; a copy claiming to link to the
    // Universal folder is a shape the scanner cannot produce.
    const claimsLink: Deployment = {
      ...claude,
      backing: { kind: "linked-to", deployment_id: universal.id },
      symlink_target: UNIVERSAL,
    };
    const problems = scannerMismatches(claimsLink).join("\n");
    expect(problems).toMatch(/^backing is/m);
    expect(problems).toMatch(/symlink_target is set on an entry that is not a link/);
  });

  it("universal_only_skill_with_no_claude_entry_builds_scanner_shapes_or_names_the_field", () => {
    const universal = universalDeployment(
      { universalPath: UNIVERSAL },
      { disabled_readers: ["claude-code"] },
    );
    expectScannerProducible("Universal only", [universal]);
    expect(universal).toMatchObject({ destination: "universal", backing: { kind: "canonical" } });

    // An id from an older format (no project or encoded path segments) is
    // one the scanner never writes.
    const oldId = { ...universal, id: "dep:v1/global/universal/find-bugs" };
    expect(scannerMismatches(oldId).join("\n")).toMatch(/^id is/m);
  });

  it("whole_folder_link_builds_scanner_shapes_or_names_the_field", () => {
    const universal = universalDeployment({ universalPath: UNIVERSAL });
    const claude = wholeFolderDeployment({
      agent: "Claude Code",
      path: CLAUDE_ENTRY,
      universalPath: UNIVERSAL,
    });
    expectScannerProducible("whole-folder link", [universal, claude]);
    expect(claude.backing).toEqual({ kind: "linked-to", deployment_id: universal.id });

    // The scanner reads the entry through its parent link, so the entry
    // itself is a real folder.
    const asLink = { ...claude, is_symlink: true, symlink_target: UNIVERSAL };
    const problems = scannerMismatches(asLink).join("\n");
    expect(problems).toMatch(/is_symlink is false/);
    expect(problems).toMatch(/has no symlink_target of its own/);
  });
});

describe("dev harness estate", () => {
  it("dev_harness_snapshot_has_only_scanner_shapes_or_names_the_row", () => {
    for (const skill of buildHarnessSnapshot().skills) {
      expectScannerProducible(skill.name, skill.deployments);
    }
  });
});
