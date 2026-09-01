// ============================================================================
// sidebar-nav - Non-component helpers for the Sidebar: which nav row a view
// anchors to, and the "last scanned" relative-time label. Split out of
// Sidebar.tsx so that file only exports the component (Fast Refresh).
// ============================================================================

import type { ActiveView } from "../store/appStore";

/**
 * The row a sidebar item should highlight against: a skill page isn't a row
 * of its own, so it anchors to the view it was opened from (Home, Skills,
 * …), which keeps that row active while the page is open.
 */
export function sidebarAnchorView(activeView: ActiveView): ActiveView {
  return activeView.kind === "skill" ? activeView.from : activeView;
}

export function relativeScanTime(scannedAt: string | undefined): string {
  if (!scannedAt) return "never scanned";
  const ms = Date.now() - new Date(scannedAt).getTime();
  if (ms < 0 || Number.isNaN(ms)) return "just now";
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}
