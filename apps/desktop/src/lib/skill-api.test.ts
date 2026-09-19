// ============================================================================
// Skill Studio - skill-api tests
// skill-api.ts is the one file that names a Tauri command; a wrapper's shape
// or command string only changes with a test here. These tests pin the
// wrapper-to-command map against `src-tauri/src/lib.rs`'s registration list
// read straight off disk, guard the leftover-name allowlist unit 4.4 (#175)
// left in place, and keep the dev harness mock answering every kept wrapper.
// ============================================================================

import { describe, expect, it } from "vitest";
import skillApiSource from "./skill-api.ts?raw";
import mockTauriSource from "../dev/harness/mock-tauri.ts?raw";
// `?raw` reads straight off disk, the same as the two source imports above - no `node:fs`, so
// this stays typed without a Node types dependency this frontend workspace doesn't otherwise need.
import libRsSource from "../../src-tauri/src/lib.rs?raw";

/**
 * One exported wrapper's shape:
 * - "command": routes through `callCommand`, an IPC round trip.
 * - "event": subscribes via `listen`, never calls `invoke`.
 * - "none": not IPC at all (`invokeErrorMessage` is a pure string helper).
 *
 * `registeredInLibRs: false` marks a wrapper whose command is NOT in
 * `generate_handler!` today - a pre-existing gap from unit 4.3 (#200), which
 * unregistered every pack command while `AddSkillSheet`'s "Pack" add method
 * still calls `importSkillPack`/`confirmSkillPackTrust`/`abandonPackImportTrust`.
 * Fixing that is out of unit 4.4's scope (no Rust change beyond removing
 * `set_deployment_enabled`, no component change beyond the two Harnesses
 * rail callers); recorded here so the map stays honest either way - flip it
 * to `true` the day someone re-registers the three pack commands.
 */
type WrapperEntry =
  | { kind: "command"; command: string; registeredInLibRs: boolean }
  | { kind: "event"; event: string }
  | { kind: "none" };

const EXPECTED_WRAPPER_COMMANDS = {
  invokeErrorMessage: { kind: "none" },
  previewSkillFrontmatterRepair: {
    kind: "command",
    command: "preview_skill_frontmatter_repair",
    registeredInLibRs: true,
  },
  applySkillFrontmatterRepair: {
    kind: "command",
    command: "apply_skill_frontmatter_repair",
    registeredInLibRs: true,
  },
  fixSkill: { kind: "command", command: "fix_skill", registeredInLibRs: true },
  openConflictPaths: { kind: "command", command: "open_conflict_paths", registeredInLibRs: true },
  searchSkills: { kind: "command", command: "search_skills", registeredInLibRs: true },
  getPopularSkills: { kind: "command", command: "get_popular_skills", registeredInLibRs: true },
  getSkillDetails: { kind: "command", command: "get_skill_details", registeredInLibRs: true },
  getInstalledSkills: {
    kind: "command",
    command: "get_installed_skills",
    registeredInLibRs: true,
  },
  getTrackedProjects: {
    kind: "command",
    command: "get_tracked_projects",
    registeredInLibRs: true,
  },
  registerSkillProjects: {
    kind: "command",
    command: "register_skill_projects",
    registeredInLibRs: true,
  },
  unregisterSkillProject: {
    kind: "command",
    command: "unregister_skill_project",
    registeredInLibRs: true,
  },
  removeSkillProject: {
    kind: "command",
    command: "remove_skill_project",
    registeredInLibRs: true,
  },
  getDiscoverySources: {
    kind: "command",
    command: "get_discovery_sources",
    registeredInLibRs: true,
  },
  setDiscoverySource: {
    kind: "command",
    command: "set_discovery_source",
    registeredInLibRs: true,
  },
  detectHarnesses: { kind: "command", command: "detect_harnesses", registeredInLibRs: true },
  getHarnessesChoice: {
    kind: "command",
    command: "get_harnesses_choice",
    registeredInLibRs: true,
  },
  saveHarnessesChoice: {
    kind: "command",
    command: "save_harnesses_choice",
    registeredInLibRs: true,
  },
  runDoctor: { kind: "command", command: "doctor", registeredInLibRs: true },
  onDoctorReport: { kind: "event", event: "skills://doctor" },
  listProjectFolders: {
    kind: "command",
    command: "list_project_folders",
    registeredInLibRs: true,
  },
  // Deferred from the shell's own UI (no Project Folders "Import" affordance calls it yet), but
  // the command is fully wired - see unit 4.4's four-name allowlist test below.
  importTrackedProjects: {
    kind: "command",
    command: "import_tracked_projects",
    registeredInLibRs: true,
  },
  removeSkill: { kind: "command", command: "remove_skill", registeredInLibRs: true },
  updateSkill: { kind: "command", command: "update_skill", registeredInLibRs: true },
  updateAllSkills: { kind: "command", command: "update_all_skills", registeredInLibRs: true },
  readInstalledSkillMd: {
    kind: "command",
    command: "read_installed_skill_md",
    registeredInLibRs: true,
  },
  writeInstalledSkillMdIfUnchanged: {
    kind: "command",
    command: "write_installed_skill_md_if_unchanged",
    registeredInLibRs: true,
  },
  openSkillPath: { kind: "command", command: "open_skill_path", registeredInLibRs: true },
  getEditorChoices: { kind: "command", command: "get_editor_choices", registeredInLibRs: true },
  setPreferredEditor: {
    kind: "command",
    command: "set_preferred_editor",
    registeredInLibRs: true,
  },
  commandHealth: { kind: "command", command: "command_health", registeredInLibRs: true },
  dataFolderStatus: { kind: "command", command: "data_folder_status", registeredInLibRs: true },
  getErrorReportingEnabled: {
    kind: "command",
    command: "get_error_reporting_enabled",
    registeredInLibRs: true,
  },
  setErrorReportingEnabled: {
    kind: "command",
    command: "set_error_reporting_enabled",
    registeredInLibRs: true,
  },
  forkSkill: { kind: "command", command: "fork_skill", registeredInLibRs: true },
  pullForkUpstream: { kind: "command", command: "pull_fork_upstream", registeredInLibRs: true },
  unforkSkill: { kind: "command", command: "unfork_skill", registeredInLibRs: true },
  // Not registered - see the module doc above.
  importSkillPack: { kind: "command", command: "import_skill_pack", registeredInLibRs: false },
  confirmSkillPackTrust: {
    kind: "command",
    command: "confirm_skill_pack_trust",
    registeredInLibRs: false,
  },
  abandonPackImportTrust: {
    kind: "command",
    command: "abandon_pack_import_trust",
    registeredInLibRs: false,
  },
  addSkill: { kind: "command", command: "add_skill", registeredInLibRs: true },
  startAddSkillOperation: {
    kind: "command",
    command: "start_add_skill_operation",
    registeredInLibRs: true,
  },
  startAddSkillsOperation: {
    kind: "command",
    command: "start_add_skills_operation",
    registeredInLibRs: true,
  },
  getAddSkillOperation: {
    kind: "command",
    command: "get_add_skill_operation",
    registeredInLibRs: true,
  },
  cancelAddSkillOperation: {
    kind: "command",
    command: "cancel_add_skill_operation",
    registeredInLibRs: true,
  },
  confirmAddSkillTrust: {
    kind: "command",
    command: "confirm_add_skill_trust",
    registeredInLibRs: true,
  },
  onAddSkillOperation: { kind: "event", event: "skills://add-skill-operation" },
  listGithubSkills: { kind: "command", command: "list_github_skills", registeredInLibRs: true },
  getAddMethodDefaults: {
    kind: "command",
    command: "get_add_method_defaults",
    registeredInLibRs: true,
  },
  installPreferences: {
    kind: "command",
    command: "install_preferences",
    registeredInLibRs: true,
  },
  // Trial is a deferred feature (see issue-4.4-followup-a.md); kept per unit 4.4's four-name
  // allowlist test below.
  keepSkillTrial: { kind: "command", command: "keep_skill_trial", registeredInLibRs: true },
  restoreTrashedSkill: {
    kind: "command",
    command: "restore_trashed_skill",
    registeredInLibRs: true,
  },
  onTrialExpired: { kind: "event", event: "skills://trial-expired" },
  parkSkill: { kind: "command", command: "park_skill", registeredInLibRs: true },
  unparkSkill: { kind: "command", command: "unpark_skill", registeredInLibRs: true },
  setHarnessEnabled: { kind: "command", command: "set_harness_enabled", registeredInLibRs: true },
  restoreMovedDeployment: {
    kind: "command",
    command: "restore_moved_deployment",
    registeredInLibRs: true,
  },
  setSkillInvocation: {
    kind: "command",
    command: "set_skill_invocation",
    registeredInLibRs: true,
  },
  setPluginEnabled: { kind: "command", command: "set_plugin_enabled", registeredInLibRs: true },
  uninstallPlugin: { kind: "command", command: "uninstall_plugin", registeredInLibRs: true },
  listSkillEvents: { kind: "command", command: "list_skill_events", registeredInLibRs: true },
  restoreSkillEvent: {
    kind: "command",
    command: "restore_skill_event",
    registeredInLibRs: true,
  },
  materializeHarnessRoot: {
    kind: "command",
    command: "materialize_harness_root",
    registeredInLibRs: true,
  },
  materializeHarnessRootThenDisable: {
    kind: "command",
    command: "materialize_harness_root_then_disable",
    registeredInLibRs: true,
  },
  makeSkillIndependentCopy: {
    kind: "command",
    command: "make_skill_independent_copy",
    registeredInLibRs: true,
  },
  repairSkillLink: { kind: "command", command: "repair_skill_link", registeredInLibRs: true },
  getSkillSnapshot: { kind: "command", command: "get_skill_snapshot", registeredInLibRs: true },
  requestSkillRescan: {
    kind: "command",
    command: "request_skill_rescan",
    registeredInLibRs: true,
  },
  onSkillSnapshot: { kind: "event", event: "skills://snapshot" },
  appVersion: { kind: "command", command: "app_version", registeredInLibRs: true },
} satisfies Record<string, WrapperEntry>;

/** Every `export (async )?function <name>` in `skill-api.ts`, source-order - the wrapper set the
 * pinning test below must exactly cover, neither more nor fewer than `EXPECTED_WRAPPER_COMMANDS`. */
function exportedWrapperNames(source: string): string[] {
  return [...source.matchAll(/^export (?:async )?function (\w+)/gm)].map((m) => m[1]);
}

/** One wrapper's body: from its `export function` line to the next top-level `export function`
 * (or EOF) - not a brace-matcher, but every wrapper here is a flat few lines, so the slice never
 * needs to look past the next sibling's start. */
function wrapperBody(source: string, name: string, allNames: string[]): string {
  const start = source.search(new RegExp(`^export (?:async )?function ${name}\\b`, "m"));
  if (start < 0) throw new Error(`wrapperBody: ${name} not found in source`);
  const rest = source.slice(start + 1);
  const nextStarts = allNames
    .filter((other) => other !== name)
    .map((other) => rest.search(new RegExp(`^export (?:async )?function ${other}\\b`, "m")))
    .filter((i) => i >= 0);
  const end = nextStarts.length > 0 ? Math.min(...nextStarts) : rest.length;
  return rest.slice(0, end);
}

/** Derives one wrapper's `WrapperEntry` from its body - the same shapes `EXPECTED_WRAPPER_COMMANDS`
 * hand-pins, read back out of the real source instead of trusted by hand. `fullSource` resolves a
 * `listen` call keyed by a module-level constant (`ADD_SKILL_OPERATION_EVENT`), declared outside
 * the wrapper's own body. */
function deriveWrapperEntry(body: string, fullSource: string): WrapperEntry {
  const commandMatch = body.match(/callCommand(?:<[^>]*>)?\(\s*"([a-z_]+)"/);
  if (commandMatch) return { kind: "command", command: commandMatch[1], registeredInLibRs: true };
  const literalEvent = body.match(/listen<[^>]*>\(\s*"([\w:./-]+)"/);
  if (literalEvent) return { kind: "event", event: literalEvent[1] };
  const constEvent = body.match(/listen<[^>]*>\(\s*([A-Z_][A-Z0-9_]*)/);
  if (constEvent) {
    const decl = fullSource.match(new RegExp(`${constEvent[1]}\\s*=\\s*"([\\w:./-]+)"`));
    if (decl) return { kind: "event", event: decl[1] };
  }
  return { kind: "none" };
}

/** `skills::mod::command_name,` -> `command_name` for every line inside `generate_handler![...]`
 * in `src-tauri/src/lib.rs`, read straight off disk - the ops surface the frontend may call. One
 * Rust path per line; takes the segment after the last `::`. */
function registeredCommandNames(libRs: string): Set<string> {
  const start = libRs.indexOf("generate_handler![");
  if (start < 0) {
    throw new Error("registeredCommandNames: could not find generate_handler![...] in lib.rs");
  }
  // A `]\s*\)` close, not a literal indentation string - rustfmt reformatting the
  // block's indentation must not break this parse.
  const closeMatch = /\]\s*\)/.exec(libRs.slice(start));
  if (!closeMatch) {
    throw new Error("registeredCommandNames: could not find generate_handler![...] in lib.rs");
  }
  const end = start + closeMatch.index;
  const body = libRs.slice(start, end);
  const names = new Set<string>();
  for (const line of body.split("\n")) {
    const trimmed = line.trim();
    if (trimmed.length === 0 || trimmed.startsWith("//")) continue;
    const m = trimmed.match(/^([\w:]+),$/);
    if (!m) continue;
    const segments = m[1].split("::");
    names.add(segments[segments.length - 1]);
  }
  return names;
}

describe("skill-api wrapper set", () => {
  it("skill_api_wrapper_set_matches_ops_commands_or_names_the_drift", () => {
    const wrapperNames = exportedWrapperNames(skillApiSource);
    const expectedNames = Object.keys(EXPECTED_WRAPPER_COMMANDS);

    // Every exported wrapper is pinned, and the pin list names nothing extra.
    expect(wrapperNames.sort()).toEqual([...expectedNames].sort());

    const registered = registeredCommandNames(libRsSource);

    for (const [name, expected] of Object.entries(EXPECTED_WRAPPER_COMMANDS)) {
      const derived = deriveWrapperEntry(
        wrapperBody(skillApiSource, name, wrapperNames),
        skillApiSource,
      );
      expect(derived, `${name}: derived ${JSON.stringify(derived)}`).toEqual(
        expected.kind === "command"
          ? { kind: "command", command: expected.command, registeredInLibRs: true }
          : expected,
      );
      if (expected.kind === "command") {
        const isRegistered = registered.has(expected.command);
        expect(
          isRegistered,
          `${name} -> "${expected.command}": generate_handler! registration is ${isRegistered}, EXPECTED_WRAPPER_COMMANDS says registeredInLibRs: ${expected.registeredInLibRs}`,
        ).toBe(expected.registeredInLibRs);
      }
    }
  });

  it("set_deployment_enabled_trial_wrappers_and_import_tracked_projects_are_removed_or_names_the_leftover", () => {
    // setDeploymentEnabled: unit 4.4 (#175) removed it end to end - park is the generic off
    // switch now, see docs/action-map/frontend-keep.md.
    expect(skillApiSource).not.toMatch(/\bsetDeploymentEnabled\b/);

    // The other four are an explicit allowlist, not oversights - each has a reason and a
    // follow-up:
    // - keepSkillTrial, restoreTrashedSkill, onTrialExpired: the trial feature is deferred
    //   whole, not this unit's cut - see issue-4.4-followup-a.md.
    // - importTrackedProjects: registered and reachable from `getTrackedProjects`' consumers'
    //   own import flow, just not the setDeploymentEnabled-adjacent code this unit touched.
    for (const kept of [
      "keepSkillTrial",
      "restoreTrashedSkill",
      "onTrialExpired",
      "importTrackedProjects",
    ]) {
      expect(skillApiSource, `${kept} should still be exported`).toMatch(
        new RegExp(`export (async )?function ${kept}\\b`),
      );
    }
  });

  it("dev_harness_mock_answers_every_command_the_kept_wrappers_call_or_names_the_gap", () => {
    const mockedCases = new Set(
      [...mockTauriSource.matchAll(/case "([a-z_]+)":/g)].map((m) => m[1]),
    );
    const missing = Object.values(EXPECTED_WRAPPER_COMMANDS)
      .filter((entry): entry is Extract<WrapperEntry, { kind: "command" }> => {
        return (
          entry.kind === "command" && entry.registeredInLibRs && !mockedCases.has(entry.command)
        );
      })
      .map((entry) => entry.command);
    expect(missing, `dev harness mock-tauri.ts has no case for: ${missing.join(", ")}`).toEqual([]);
  });
});
