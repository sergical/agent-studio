// ============================================================================
// Skill Studio - Store install trust-prompt tests (review round 2, B1)
// ============================================================================

import { describe, expect, it, vi } from "vitest";
import {
  confirmStoreInstallTrust,
  declineStoreInstallTrust,
  parentProgressForPhase,
  startStoreInstall,
} from "./store-install-flow";
import type { AddSkillOperationEvent, AddSkillRequest } from "@skill-studio/lib";

// SAFETY: the fake `start`/`getOperation` below never read `request` - only the request
// object identity matters for these tests, not its fields.
const REQUEST = {} as AddSkillRequest;

function event(
  operationId: string,
  phase: AddSkillOperationEvent["phase"],
  extra: Partial<AddSkillOperationEvent> = {},
): AddSkillOperationEvent {
  return { operation_id: operationId, sequence: 1, phase, message: phase, ...extra };
}

describe("store install trust prompt", () => {
  it("store_install_of_an_untrusted_source_shows_the_trust_prompt_and_installs_after_confirm_or_names_the_skipped_prompt", async () => {
    const needsTrust = event("op-1", "needs-trust", {
      untrusted_source: { identity: "octocat/skills" },
    });
    const start = vi.fn(async () => needsTrust);
    const getOperation = vi.fn(async () => needsTrust);
    const started = await startStoreInstall("op-1", REQUEST, { start, getOperation });
    // Names the skipped prompt: a build that installs straight through instead of
    // pausing on `needs-trust` would return `completed` here, not this phase.
    expect(started.phase).toBe("needs-trust");
    expect(started.untrusted_source?.identity).toBe("octocat/skills");

    const completed = event("op-2", "completed", {
      result: {
        name: "visual-recap",
        tool: "skills-sh",
        command: "add",
        deployments_created: [],
        warning: null,
      },
    });
    const confirmTrust = vi.fn(async () => event("op-2", "queued"));
    const getRetryOperation = vi.fn(async () => completed);
    const settled = await confirmStoreInstallTrust("op-1", "op-2", "octocat/skills", {
      confirmTrust,
      getOperation: getRetryOperation,
    });
    expect(confirmTrust).toHaveBeenCalledWith("op-1", "op-2", "octocat/skills");
    expect(settled.phase).toBe("completed");
    expect(settled.result?.name).toBe("visual-recap");
  });

  it("store_install_declined_at_the_trust_prompt_writes_nothing_or_names_the_written_skill", async () => {
    const cancel = vi.fn(async () => event("op-1", "cancelled"));
    const declined = await declineStoreInstallTrust("op-1", cancel);
    expect(cancel).toHaveBeenCalledWith("op-1");
    // Names the written skill a regression would produce: decline must resolve to
    // `cancelled` with no `result`, never a `completed` event naming an install.
    expect(declined.phase).toBe("cancelled");
    expect(declined.result).toBeUndefined();
  });

  it("store_install_reaching_needs_trust_clears_the_parent_progress_or_names_the_phase_left_showing", () => {
    // Names the phase left showing: a build that leaves the `InstallProgressModal`
    // tracking would return "keep" here, not "clear" - the parent's spinner then sits
    // over the trust prompt with no way to dismiss it.
    expect(parentProgressForPhase("needs-trust")).toBe("clear");
  });

  it("store_install_declined_at_the_trust_prompt_clears_the_parent_progress_or_names_the_phase_left_showing", () => {
    // Decline settles the paused operation on `cancelled` (see the decline test
    // above); that phase must also clear the parent's modal.
    expect(parentProgressForPhase("cancelled")).toBe("clear");
  });
});
