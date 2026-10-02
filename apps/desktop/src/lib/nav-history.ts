// ============================================================================
// nav-history - Back/forward stacks for the shell's route state. Pure
// functions over plain data, so the store, the shortcuts, and the tests share
// one definition of how a step moves between locations.
// ============================================================================

import type { ActiveView } from "../store/appStore";

/** Oldest entries fall off the back stack past this many. */
export const NAV_HISTORY_LIMIT = 50;

export interface NavHistory {
  back: ActiveView[];
  forward: ActiveView[];
}

export const EMPTY_NAV_HISTORY: NavHistory = { back: [], forward: [] };

/** Default deployment path per skill name: the copy a skill page shows when none was requested. */
export type DefaultDeploymentPaths = ReadonlyMap<string, string>;

/** The default copy a skill page settled on when it opened, kept so a rescan that changes which
 * copy carries warnings never moves the page (and its open editor) to another copy. The pin
 * resets when `openSkill` opens a page afresh (any open except back/forward), and when the page
 * shows another skill; see `repinDeployment`. */
export interface PinnedDeployment {
  skillName: string | undefined;
  path: string | undefined;
}

/** Whether two views are the same place. A skill page's `from` and one-shot `intent` don't
 * count: they describe how it was opened, not where it is. With `defaults`, "no path" and the
 * path of the default copy count as the same page, since both show the same SKILL.md. */
export function isSameLocation(
  left: ActiveView,
  right: ActiveView,
  defaults?: DefaultDeploymentPaths | null,
): boolean {
  if (left.kind !== right.kind) return false;
  if (left.kind === "skill" && right.kind === "skill") {
    const resolve = (view: typeof left) => view.deploymentPath ?? defaults?.get(view.name);
    return left.name === right.name && resolve(left) === resolve(right);
  }
  if (left.kind === "learn" && right.kind === "learn") return left.section === right.section;
  return true;
}

/** Stored entries drop `intent`, so stepping back onto a skill page never reopens its dialog. */
function toEntry(view: ActiveView): ActiveView {
  return view.kind === "skill" ? { ...view, intent: undefined } : view;
}

/** Records a user navigation from `current` to `next`. A move to the same place records nothing;
 * any real move clears the forward stack. */
export function recordNavigation(
  history: NavHistory,
  current: ActiveView,
  next: ActiveView,
  defaults?: DefaultDeploymentPaths | null,
): NavHistory {
  if (isSameLocation(current, next, defaults)) return history;
  return {
    back: [...history.back, toEntry(current)].slice(-NAV_HISTORY_LIMIT),
    forward: [],
  };
}

export interface NavStep {
  history: NavHistory;
  view: ActiveView;
}

/** Pops entries off `from` until one passes `exists`, dropping those that don't (a removed
 * skill). The location being left moves onto `to`. Null when nothing valid is left. */
function step(
  from: ActiveView[],
  to: ActiveView[],
  current: ActiveView,
  exists: (view: ActiveView) => boolean,
): { remaining: ActiveView[]; pushed: ActiveView[]; view: ActiveView } | null {
  let remaining = from;
  while (remaining.length > 0) {
    const view = remaining[remaining.length - 1];
    remaining = remaining.slice(0, -1);
    if (exists(view)) {
      return { remaining, pushed: [...to, toEntry(current)].slice(-NAV_HISTORY_LIMIT), view };
    }
  }
  return null;
}

export function stepBack(
  history: NavHistory,
  current: ActiveView,
  exists: (view: ActiveView) => boolean,
): NavStep | null {
  const result = step(history.back, history.forward, current, exists);
  if (!result) return null;
  return { history: { back: result.remaining, forward: result.pushed }, view: result.view };
}

export function stepForward(
  history: NavHistory,
  current: ActiveView,
  exists: (view: ActiveView) => boolean,
): NavStep | null {
  const result = step(history.forward, history.back, current, exists);
  if (!result) return null;
  return { history: { back: result.pushed, forward: result.remaining }, view: result.view };
}
