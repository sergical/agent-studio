import { expect, it, vi } from "vitest";
import {
  cancelDocumentOperation,
  runDocumentOperation,
  DocumentOperationCancelled,
} from "./skill-document-operation";
import type {
  DocumentOperationRequest,
  DocumentOperationTransport,
} from "./skill-document-operation";

const restoreRequest: DocumentOperationRequest = {
  command: "restore_skill_event",
  args: { eventId: "source", force: false },
};

const repairRequest: DocumentOperationRequest = {
  command: "apply_skill_frontmatter_repair",
  args: {
    request: {
      target: { deployment_id: "selected" },
      proposal_id: "preview",
      expected_content_fingerprint: "before",
      mode: "apply-fix",
    },
  },
};

const saveRequest: DocumentOperationRequest = {
  command: "write_installed_skill_md_if_unchanged",
  args: { path: "/fixture/SKILL.md", expectedContent: "before", content: "after" },
};

it.each([restoreRequest, repairRequest, saveRequest])(
  "retains admission and matches cancellation for $command",
  async (request) => {
    const unlisten = vi.fn();
    let receive: (id: string) => void = () => {};
    let finish: () => void = () => {};
    const run = vi.fn(async (_request: DocumentOperationRequest, id: string) => {
      receive("someone-else");
      receive(id);
      await new Promise<void>((resolve) => {
        finish = resolve;
      });
    });
    const transport: DocumentOperationTransport = {
      subscribeStarted: async (listener) => {
        receive = listener;
        return unlisten;
      },
      run,
      cancel: async () => true,
    };
    const started = vi.fn();
    const pending = runDocumentOperation(request, started, transport);
    await vi.waitFor(() => expect(started).toHaveBeenCalledOnce());
    expect(unlisten).not.toHaveBeenCalled();
    expect(run).toHaveBeenCalledWith(request, started.mock.calls[0]?.[0]);
    expect(await cancelDocumentOperation(started.mock.calls[0]?.[0], transport)).toBe(true);
    expect(unlisten).not.toHaveBeenCalled();
    finish();
    await pending;
    expect(unlisten).toHaveBeenCalledOnce();
  },
);

it("cleans up a refused mutation and preserves its backend error", async () => {
  const unlisten = vi.fn();
  const transport: DocumentOperationTransport = {
    subscribeStarted: async () => unlisten,
    run: async () => {
      throw new Error("Document changed");
    },
    cancel: async () => false,
  };
  await expect(runDocumentOperation(restoreRequest, vi.fn(), transport)).rejects.toThrow(
    "Document changed",
  );
  expect(unlisten).toHaveBeenCalledOnce();
});

it("sends a cancellation request for the exact operation", async () => {
  const cancel = vi.fn(async () => false);
  const transport: DocumentOperationTransport = {
    subscribeStarted: async () => () => {},
    run: async () => {},
    cancel,
  };
  expect(await cancelDocumentOperation("finished", transport)).toBe(false);
  expect(cancel).toHaveBeenCalledWith("finished");
});

it.each([
  [{ code: "cancelled" }, true, "Document operation cancelled"],
  [
    { code: "failed", message: "Recovery remains unresolved after cancellation" },
    false,
    "Recovery remains unresolved after cancellation",
  ],
  ["directory coordination was cancelled", false, "directory coordination was cancelled"],
  [new Error("Document changed"), false, "Document changed"],
] as const)(
  "preserves the terminal error distinction for %j",
  async (failure, cancelled, message) => {
    const unlisten = vi.fn();
    const transport: DocumentOperationTransport = {
      subscribeStarted: async () => unlisten,
      run: async () => {
        throw failure;
      },
      cancel: async () => true,
    };
    const pending = runDocumentOperation(repairRequest, vi.fn(), transport);
    await expect(pending).rejects.toHaveProperty("message", message);
    if (cancelled) {
      await expect(pending).rejects.toBeInstanceOf(DocumentOperationCancelled);
    } else {
      await expect(pending).rejects.toBeInstanceOf(Error);
      await expect(pending).rejects.not.toBeInstanceOf(DocumentOperationCancelled);
    }
    expect(unlisten).toHaveBeenCalledOnce();
  },
);
