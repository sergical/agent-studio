import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { z } from "zod";
import type { FrontmatterRepairApplyMode, LifecycleTarget } from "@skill-studio/lib";

const documentSaveFailureSchema = z.discriminatedUnion("code", [
  z.object({ code: z.literal("cancelled") }).strict(),
  z.object({ code: z.literal("failed"), message: z.string() }).strict(),
]);

export class DocumentOperationCancelled extends Error {
  constructor() {
    super("Document operation cancelled");
    this.name = "DocumentOperationCancelled";
  }
}

export type DocumentOperationRequest =
  | {
      command: "apply_skill_frontmatter_repair";
      args: {
        request: {
          target: LifecycleTarget;
          proposal_id: string;
          expected_content_fingerprint: string;
          mode: FrontmatterRepairApplyMode;
        };
      };
    }
  | { command: "restore_skill_event"; args: { eventId: string; force: boolean } }
  | {
      command: "write_installed_skill_md_if_unchanged";
      args: { path: string; expectedContent: string; content: string };
    };

export interface DocumentOperationTransport {
  subscribeStarted: (receive: (operationId: string) => void) => Promise<() => void>;
  run: (request: DocumentOperationRequest, operationId: string) => Promise<void>;
  cancel: (operationId: string) => Promise<boolean>;
}

const desktopTransport: DocumentOperationTransport = {
  subscribeStarted: (receive) =>
    listen<string>("skills://document-operation-started", ({ payload }) => receive(payload)),
  run: ({ command, args }, operationId) => invoke(command, { ...args, operationId }),
  cancel: (operationId) => invoke("cancel_document_operation", { operationId }),
};

export async function runDocumentOperation(
  request: DocumentOperationRequest,
  onStarted?: (operationId: string) => void,
  transport = desktopTransport,
): Promise<void> {
  const operationId = crypto.randomUUID();
  const unlisten = onStarted
    ? await transport.subscribeStarted((startedId) => {
        if (startedId === operationId) onStarted(operationId);
      })
    : undefined;
  try {
    await transport.run(request, operationId);
  } catch (error) {
    const failure = documentSaveFailureSchema.safeParse(error);
    if (failure.success) {
      if (failure.data.code === "cancelled") throw new DocumentOperationCancelled();
      throw new Error(failure.data.message);
    }
    throw error instanceof Error ? error : new Error(String(error));
  } finally {
    unlisten?.();
  }
}

export async function cancelDocumentOperation(
  operationId: string,
  transport = desktopTransport,
): Promise<boolean> {
  return transport.cancel(operationId);
}
