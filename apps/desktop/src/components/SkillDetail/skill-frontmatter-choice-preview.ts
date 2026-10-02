// ============================================================================
// Skill Studio - invocation-choice preview tracking for the repair dialog
// Each pick starts a backend preview. Only the latest pick may replace the
// shown proposal, and nothing can be applied while a pick is still pending.
// ============================================================================

import type {
  FrontmatterRepairKind,
  FrontmatterRepairPreview,
  InvocationConflictChoice,
  LifecycleTarget,
} from "@skill-studio/lib";

export interface ChoicePreviewState {
  /** The proposal on screen; the only one that may be applied. */
  preview: FrontmatterRepairPreview;
  /** The side the user picked last; `preview` catches up once its response lands. */
  selectedChoice: InvocationConflictChoice | null;
  isPending: boolean;
}

export function canApplyChoicePreview(state: ChoicePreviewState): boolean {
  return !state.isPending && state.preview.choice === state.selectedChoice;
}

export interface ChoicePreviewController {
  getState: () => ChoicePreviewState;
  subscribe: (listener: () => void) => () => void;
  choose: (choice: InvocationConflictChoice) => void;
}

type RequestPreview = (
  target: LifecycleTarget,
  kind: FrontmatterRepairKind,
  choice: InvocationConflictChoice,
) => Promise<FrontmatterRepairPreview>;

export function createChoicePreviewController(
  requestPreview: RequestPreview,
  target: LifecycleTarget,
  initial: FrontmatterRepairPreview,
  onError: (message: string) => void,
): ChoicePreviewController {
  let state: ChoicePreviewState = {
    preview: initial,
    selectedChoice: initial.choice,
    isPending: false,
  };
  let latestRequest = 0;
  const listeners = new Set<() => void>();
  const update = (next: ChoicePreviewState) => {
    state = next;
    listeners.forEach((listener) => listener());
  };

  return {
    getState: () => state,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    choose: (choice) => {
      const request = ++latestRequest;
      update({ ...state, selectedChoice: choice, isPending: true });
      requestPreview(target, initial.kind, choice)
        .then((preview) => {
          if (request !== latestRequest) return;
          update({ preview, selectedChoice: preview.choice, isPending: false });
        })
        .catch((error) => {
          if (request !== latestRequest) return;
          update({ ...state, selectedChoice: state.preview.choice, isPending: false });
          onError(error instanceof Error ? error.message : "Unknown error");
        });
    },
  };
}
