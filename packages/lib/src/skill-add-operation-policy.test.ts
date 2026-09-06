import { describe, expect, it } from "vitest";
import {
  addSkillFinishAction,
  addSkillOperationProgressCopy,
  addSkillOperationTerminalCopy,
  isAddSkillOperationCancellable,
  isAddSkillOperationTerminal,
  selectNewerAddSkillOperationEvent,
  shouldConsumeAddSkillOperation,
} from "./skill-add-operation-policy";
import type { AddSkillOperationEvent } from "./skill-add-operation-types";

function event(
  operationId: string,
  sequence: number,
  phase: AddSkillOperationEvent["phase"],
): AddSkillOperationEvent {
  return {
    operation_id: operationId,
    sequence,
    phase,
    message: phase,
  };
}

describe("selectNewerAddSkillOperationEvent", () => {
  it("keeps the first event for this operation", () => {
    const incoming = event("op-1", 1, "queued");
    expect(selectNewerAddSkillOperationEvent(undefined, incoming, "op-1")).toBe(incoming);
  });

  it("rejects a stale sequence", () => {
    const current = event("op-1", 4, "installing");
    expect(selectNewerAddSkillOperationEvent(current, event("op-1", 3, "validating"), "op-1")).toBe(
      current,
    );
  });

  it("rejects a foreign operation id", () => {
    const current = event("op-1", 2, "installing");
    expect(selectNewerAddSkillOperationEvent(current, event("op-2", 9, "completed"), "op-1")).toBe(
      current,
    );
  });

  it("accepts a later sequence", () => {
    const current = event("op-1", 2, "installing");
    const incoming = event("op-1", 3, "completed");
    expect(selectNewerAddSkillOperationEvent(current, incoming, "op-1")).toBe(incoming);
  });

  it("switches to a linked retry without mixing parent events", () => {
    const parent = event("op-parent", 4, "needs-trust");
    const retry = event("op-retry", 1, "queued");
    expect(selectNewerAddSkillOperationEvent(parent, retry, "op-retry")).toBe(retry);
    expect(selectNewerAddSkillOperationEvent(retry, parent, "op-retry")).toBe(retry);
  });
});

describe("add skill operation phase policy", () => {
  it("keeps cancel enabled only while work can still stop", () => {
    expect(isAddSkillOperationCancellable("queued")).toBe(true);
    expect(isAddSkillOperationCancellable("installing")).toBe(true);
    expect(isAddSkillOperationCancellable("needs-trust")).toBe(false);
    expect(isAddSkillOperationCancellable("completed")).toBe(false);
  });

  it("treats completed, failed, cancelled, and timed-out as terminal", () => {
    expect(isAddSkillOperationTerminal("completed")).toBe(true);
    expect(isAddSkillOperationTerminal("failed")).toBe(true);
    expect(isAddSkillOperationTerminal("cancelled")).toBe(true);
    expect(isAddSkillOperationTerminal("timed-out")).toBe(true);
    expect(isAddSkillOperationTerminal("needs-trust")).toBe(false);
  });

  it("consumes a terminal operation once", () => {
    const completed = event("op-1", 5, "completed");
    expect(shouldConsumeAddSkillOperation(completed, undefined)).toBe(true);
    expect(shouldConsumeAddSkillOperation(completed, "op-1")).toBe(false);
    expect(shouldConsumeAddSkillOperation(event("op-1", 4, "needs-trust"), undefined)).toBe(false);
  });

  it("shows batch item progress and distinct terminal copy", () => {
    expect(
      addSkillOperationProgressCopy({
        ...event("op-1", 3, "installing"),
        item: { current: 2, total: 5, name: "visual-recap" },
      }),
    ).toBe("Installing · 2 of 5");
    expect(addSkillOperationTerminalCopy(event("op-1", 6, "cancelled"))).toBe("Cancelled");
    expect(addSkillOperationTerminalCopy(event("op-1", 6, "timed-out"))).toBe("Timed out");
    expect(
      addSkillOperationTerminalCopy({
        ...event("op-1", 6, "failed"),
        error: "Copy destination already exists",
      }),
    ).toBe("Copy destination already exists");
  });

  it("opens the installed skill and reports a partial batch", () => {
    expect(
      addSkillFinishAction({
        ...event("op-1", 8, "completed"),
        result: {
          name: "find-bugs",
          tool: "dotagents",
          command: "npx",
          deployments_created: [],
        },
      }),
    ).toEqual({
      kind: "success",
      title: "Added find-bugs",
      message: undefined,
      openName: "find-bugs",
    });
    expect(
      addSkillFinishAction({
        ...event("op-1", 8, "completed"),
        outcomes: [
          {
            name: "other",
            error: "already exists",
          },
          {
            name: "visual-recap",
            result: {
              name: "visual-recap",
              tool: "copy",
              command: "copy",
              deployments_created: [],
            },
          },
        ],
      }),
    ).toEqual({
      kind: "success",
      title: "Added 1 skill",
      message: undefined,
      openName: "visual-recap",
      failedTitle: "1 skill failed",
      failedMessage: "other: already exists",
    });
  });
});
