import type { SkillEvent } from "@skill-studio/lib";

/** True when Activity can offer the backend restore action for this event. */
export function canRestoreSkillEvent(event: SkillEvent): boolean {
  return event.restorable;
}

/** The backend's drift-guard refusal names the drifted path and ends in this phrase - see event_store.rs. */
function isDriftRefusal(message: string): boolean {
  return message.includes("changed since") || message.includes("drifted");
}

/** True only when the backend says bypassing this event's drift check is safe. */
export function shouldOfferForceRestore(event: SkillEvent, message: string): boolean {
  return event.force_restorable && isDriftRefusal(message);
}
