// ============================================================================
// Skill Studio - Toast notifications
// Thin wrapper over sonner, keeping the `addToast({ type, title, message,
// duration, action })` call-site shape every caller already uses.
// ============================================================================

import { toast } from "sonner";
import type { Toast } from "@skill-studio/lib";

/** Sonner's own id type is `string | number`; ours is always a string. */
export function addToast(input: Omit<Toast, "id">): string {
  const id = `toast_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
  const { type, title, message, duration, action } = input;

  toast[type](title, {
    id,
    description: message,
    duration,
    action: action ? { label: action.label, onClick: action.onClick } : undefined,
  });

  return id;
}
