// ============================================================================
// skill-page-deployment.test - which copy a skill's page opens on when the
// caller names none: the copy that carries the spec warnings.
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import { resolveSkillPageDeployment } from "./skill-page-deployment";

const WARNING = "description exceeds 1024 characters";
const ERROR = 'name "Find Bugs" is not a valid skill name';

function fixtureDeployment(overrides: Partial<Deployment> = {}): Deployment {
  return {
    id: "dep:v1/global/universal/find-bugs",
    destination: "universal",
    owner_kind: "manual",
    mutability: "read-only",
    backing: { kind: "canonical" },
    agent: "shared",
    scope: "global",
    path: "/home/.agents/skills/find-bugs",
    is_symlink: false,
    symlink_is_broken: false,
    content_hash: "abc",
    disabled: false,
    codex_implicit_invocation: null,
    disabled_by: null,
    invocation: "both",
    spec_violations: [],
    shared_via_whole_dir_link: false,
    ...overrides,
  };
}

function fixtureSkill(deployments: Deployment[]): InstalledSkill {
  return {
    name: "find-bugs",
    source: "getsentry/find-bugs",
    source_type: "github",
    installed_at: "2026-01-01T00:00:00Z",
    has_update: false,
    source_kind: "dotagents",
    deployments,
    has_spec: true,
    spec_violations: deployments.flatMap((d) => d.spec_violations),
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
    update_owners: [],
    update_owner_ids: [],
    description: null,
    fork: null,
    parked_at: null,
    skill_path: null,
    source_url: null,
    update_commit: null,
    update_commit_at: null,
    updated_at: null,
  };
}

describe("resolveSkillPageDeployment", () => {
  it("a_clean_first_copy_and_a_warned_second_copy_opens_the_second_so_the_warning_is_visible_instead_of_hidden_on_the_first", () => {
    const skill = fixtureSkill([
      fixtureDeployment({ path: "/a/clean" }),
      fixtureDeployment({ path: "/b/warned", spec_violations: [WARNING] }),
    ]);
    expect(resolveSkillPageDeployment(skill, undefined).deployment?.path).toBe("/b/warned");
  });

  it("a_blocking_error_copy_outranks_a_warning_only_copy_even_when_the_warning_copy_is_first", () => {
    const skill = fixtureSkill([
      fixtureDeployment({ path: "/a/warned", spec_violations: [WARNING] }),
      fixtureDeployment({ path: "/b/errored", spec_violations: [ERROR] }),
    ]);
    expect(resolveSkillPageDeployment(skill, undefined).deployment?.path).toBe("/b/errored");
  });

  it("a_plugin_copy_with_errors_beats_a_clean_editable_copy_because_the_point_is_to_show_the_problem", () => {
    const skill = fixtureSkill([
      fixtureDeployment({ path: "/own/clean" }),
      fixtureDeployment({
        path: "/plugin/errored",
        spec_violations: [ERROR],
        plugin: { name: "p", version: null, harness: "Claude Code", marketplace: "m", id: "p@m" },
      }),
    ]);
    expect(resolveSkillPageDeployment(skill, undefined).deployment?.path).toBe("/plugin/errored");
  });

  it("an_explicit_deployment_path_wins_over_a_warned_copy_so_a_clicked_row_never_jumps_to_another_copy", () => {
    const skill = fixtureSkill([
      fixtureDeployment({ path: "/a/clean" }),
      fixtureDeployment({ path: "/b/warned", spec_violations: [ERROR] }),
    ]);
    expect(resolveSkillPageDeployment(skill, "/a/clean").deployment?.path).toBe("/a/clean");
    expect(resolveSkillPageDeployment(skill, "/gone").deploymentUnresolved).toBe(true);
  });

  it("all_clean_copies_keep_the_first_editable_pick_so_skills_without_warnings_open_as_before", () => {
    const skill = fixtureSkill([
      fixtureDeployment({ path: "/a/link", is_symlink: true }),
      fixtureDeployment({ path: "/b/physical" }),
    ]);
    expect(resolveSkillPageDeployment(skill, undefined).deployment?.path).toBe("/b/physical");
  });

  it("a_broken_copy_with_violations_is_not_preferred_over_a_readable_copy_with_violations_because_it_has_no_skill_md_to_show", () => {
    const skill = fixtureSkill([
      fixtureDeployment({ path: "/a/broken", symlink_is_broken: true, spec_violations: [ERROR] }),
      fixtureDeployment({ path: "/b/readable", spec_violations: [WARNING] }),
    ]);
    expect(resolveSkillPageDeployment(skill, undefined).deployment?.path).toBe("/b/readable");
  });

  it("a_skill_whose_only_violations_sit_on_a_broken_copy_falls_back_to_the_normal_pick", () => {
    const skill = fixtureSkill([
      fixtureDeployment({
        path: "/a/broken",
        is_symlink: true,
        symlink_is_broken: true,
        spec_violations: [ERROR],
      }),
      fixtureDeployment({ path: "/b/clean" }),
    ]);
    expect(resolveSkillPageDeployment(skill, undefined).deployment?.path).toBe("/b/clean");
  });
});
