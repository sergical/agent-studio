import { useState } from "react";
import { cancelDocumentOperation } from "../lib/skill-document-operation";
import { useAppStore } from "../store/appStore";

export function useDocumentCancellation() {
  const [operationId, setOperationId] = useState<string | null>(null);
  const [isCancelling, setIsCancelling] = useState(false);
  const addToast = useAppStore((state) => state.addToast);

  const cancel = async () => {
    if (!operationId || isCancelling) return;
    setIsCancelling(true);
    try {
      await cancelDocumentOperation(operationId);
    } catch (error) {
      setIsCancelling(false);
      addToast({ type: "error", title: "Could not request cancellation", message: String(error) });
    }
  };

  const reset = () => {
    setOperationId(null);
    setIsCancelling(false);
  };

  return {
    onStarted: setOperationId,
    canCancel: operationId !== null,
    isCancelling,
    cancel,
    reset,
  };
}
