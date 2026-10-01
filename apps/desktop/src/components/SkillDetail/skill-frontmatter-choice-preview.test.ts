// ============================================================================
// Skill Studio - invocation-choice preview tracking tests
// ============================================================================

import { describe, expect, it, vi } from "vitest";
import type { FrontmatterRepairPreview, InvocationConflictChoice } from "@skill-studio/lib";
import {
  canApplyChoicePreview,
  createChoicePreviewController,
} from "./skill-frontmatter-choice-preview";

function proposal(choice: InvocationConflictChoice | null): FrontmatterRepairPreview {
  return {
    deployment_id: "d1",
    path: "/skills/sample",
    scope: "global",
    reason: "r",
    kind: "invocation-conflict",
    choice,
    expected_content_fingerprint: "f",
    proposal_id: `p-${choice}`,
    original_content: "original",
    proposed_content: `proposed-${choice}`,
    allowed_apply_modes: ["apply-fix"],
  };
}

function deferred() {
  let resolve!: (preview: FrontmatterRepairPreview) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<FrontmatterRepairPreview>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const target = { deployment_id: "d1", owner_id: null };

/** Each call to the fake backend returns the next queued, test-controlled response. */
function start(...responses: Promise<FrontmatterRepairPreview>[]) {
  const onError = vi.fn();
  const queue = [...responses];
  const controller = createChoicePreviewController(
    () => queue.shift() ?? Promise.reject(new Error("no response queued")),
    target,
    proposal(null),
    onError,
  );
  return { controller, onError };
}

describe("invocation choice preview", () => {
  /**
   * Flow: the user picks a side and the preview has not answered yet.
   * Expect: Apply stays disabled until the matching proposal shows.
   * Failure: the previous proposal stays applicable and is submitted.
   */
  it("blocks Apply while a pick is pending and allows it once the proposal matches", async () => {
    const pending = deferred();
    const { controller } = start(pending.promise);

    controller.choose("user-only");
    expect(canApplyChoicePreview(controller.getState())).toBe(false);

    pending.resolve(proposal("user-only"));
    await pending.promise;
    expect(canApplyChoicePreview(controller.getState())).toBe(true);
    expect(controller.getState().preview.proposed_content).toBe("proposed-user-only");
  });

  /**
   * Flow: the user picks user-only, then model-only; model-only answers first
   * and the slower user-only answer arrives afterwards.
   * Expect: the model-only proposal stays shown and applicable.
   * Failure: the late user-only answer overwrites the latest pick.
   */
  it("ignores an older response that arrives after the latest one", async () => {
    const first = deferred();
    const second = deferred();
    const { controller } = start(first.promise, second.promise);

    controller.choose("user-only");
    controller.choose("model-only");
    second.resolve(proposal("model-only"));
    await second.promise;
    first.resolve(proposal("user-only"));
    await first.promise;

    const state = controller.getState();
    expect(state.preview.choice).toBe("model-only");
    expect(state.selectedChoice).toBe("model-only");
    expect(canApplyChoicePreview(state)).toBe(true);
  });

  /**
   * Flow: the user picks user-only, then model-only; the older answer lands
   * while the latest is still pending.
   * Expect: Apply stays disabled and the old proposal is never shown.
   * Failure: the stale proposal becomes applicable under the model-only pick.
   */
  it("keeps Apply disabled when a stale response lands while the latest is pending", async () => {
    const first = deferred();
    const second = deferred();
    const { controller } = start(first.promise, second.promise);

    controller.choose("user-only");
    controller.choose("model-only");
    first.resolve(proposal("user-only"));
    await first.promise;

    const state = controller.getState();
    expect(state.preview.choice).toBeNull();
    expect(canApplyChoicePreview(state)).toBe(false);

    second.resolve(proposal("model-only"));
    await second.promise;
    expect(canApplyChoicePreview(controller.getState())).toBe(true);
  });

  /**
   * Flow: the latest pick fails to preview.
   * Expect: the selection returns to the shown proposal, the error is
   * reported, and Apply is not left stuck disabled.
   * Failure: the dialog shows one choice selected over another's proposal.
   */
  it("reverts the selection and reports the error when the latest preview fails", async () => {
    const failing = deferred();
    const { controller, onError } = start(failing.promise);

    controller.choose("user-only");
    failing.reject(new Error("refused"));
    await failing.promise.catch(() => undefined);

    expect(controller.getState().selectedChoice).toBeNull();
    expect(controller.getState().isPending).toBe(false);
    expect(onError).toHaveBeenCalledWith("refused");
  });
});
