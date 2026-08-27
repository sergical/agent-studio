// ============================================================================
// Sidebar - Left-hand navigation: places only (Home, Skills, Activity, Packs,
// Parked). Filters (scope, harness, source, issue) live in the Skills view's
// filter bar instead - see the design rule in spec-ux-1.md section B.
// ============================================================================

import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import {
  Activity as ActivityIcon,
  LayoutDashboard,
  Package,
  PackageOpen,
  Plus,
  RefreshCw,
  Search,
} from "lucide-react";
import { ownSkillsView } from "../../lib/skill-plugin-partition";
import { defaultSkillListFilter } from "../../lib/skill-list-filter";
import { useAppStore } from "../../store/appStore";
import type { ActiveView } from "../../store/appStore";
import type { SkillSnapshot } from "../../lib/skill-types";

/**
 * The row a sidebar item should highlight against: a skill page isn't a row
 * of its own, so it anchors to the view it was opened from (Home, Skills,
 * …), which keeps that row active while the page is open.
 */
export function sidebarAnchorView(activeView: ActiveView): ActiveView {
  return activeView.kind === "skill" ? activeView.from : activeView;
}

interface SidebarProps {
  snapshot: SkillSnapshot | undefined;
  isLoading: boolean;
  requestRescan: () => Promise<void>;
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

/**
 * Left-hand navigation: a search box that jumps into Skills with a query,
 * Add skill, the four places (Home, Skills, Activity, Packs), Parked (when
 * non-empty), and a footer with the snapshot's age and a manual rescan
 * button.
 */
export function Sidebar({ snapshot, isLoading, requestRescan }: SidebarProps) {
  const [isRescanning, setIsRescanning] = useState(false);
  // Forces the footer to re-render so "just now" ages into "1m ago" and
  // beyond without waiting for the next snapshot - relativeScanTime() itself
  // stays a pure function of scannedAt and the current clock.
  const [, forceTick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => forceTick((n) => n + 1), 30_000);
    return () => clearInterval(id);
  }, []);
  const activeView = useAppStore((state) => state.activeView);
  const anchorView = sidebarAnchorView(activeView);
  const setActiveView = useAppStore((state) => state.setActiveView);
  const skillListFilter = useAppStore((state) => state.skillListFilter);
  const setSkillListFilter = useAppStore((state) => state.setSkillListFilter);
  const openAddSkillSheet = useAppStore((state) => state.openAddSkillSheet);
  const searchInputRef = useRef<HTMLInputElement>(null);

  const own = ownSkillsView(snapshot?.skills ?? []);
  const skillsCount = own.length;
  const parkedCount = own.filter((s) => s.parked).length;

  // Typing while on another view switches to Skills directly in the change
  // handler, so the query always has somewhere to act - no effect-based
  // redirect, which would otherwise fire on every store update.
  function handleSearchChange(value: string) {
    setSkillListFilter({ query: value });
    if (anchorView.kind !== "skills") setActiveView({ kind: "skills" });
  }

  function handleSearchKeyDown(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Escape") {
      setSkillListFilter({ query: "" });
      searchInputRef.current?.blur();
    }
  }

  const handleRefresh = async () => {
    setIsRescanning(true);
    await requestRescan();
    // Cleared when the next snapshot lands and `isLoading` flips back to false.
  };

  const spinning = isRescanning && isLoading;

  return (
    <nav className="skill-sidebar">
      <div className="skill-sidebar-section">
        <input
          ref={searchInputRef}
          type="search"
          aria-label="Search skills"
          className="text-control"
          placeholder="Search skills…"
          value={skillListFilter.query}
          onChange={(e) => handleSearchChange(e.target.value)}
          onKeyDown={handleSearchKeyDown}
        />
        <button className="skill-sidebar-add-skill" onClick={() => openAddSkillSheet()}>
          <Plus size={15} />
          <span>Add skill</span>
        </button>
      </div>

      <div className="skill-sidebar-section">
        <button
          className={`skill-sidebar-item ${anchorView.kind === "home" ? "active" : ""}`}
          onClick={() => setActiveView({ kind: "home" })}
        >
          <LayoutDashboard size={15} />
          <span>Home</span>
        </button>
        <button
          className={`skill-sidebar-item ${anchorView.kind === "skills" ? "active" : ""}`}
          onClick={() => setActiveView({ kind: "skills" })}
        >
          <Search size={15} />
          <span>Skills</span>
          {skillsCount > 0 && (
            <span className="skill-sidebar-badge count-tabular">{skillsCount}</span>
          )}
        </button>
        <button
          className={`skill-sidebar-item ${anchorView.kind === "activity" ? "active" : ""}`}
          onClick={() => setActiveView({ kind: "activity" })}
        >
          <ActivityIcon size={15} />
          <span>Activity</span>
        </button>
        <button
          className={`skill-sidebar-item ${anchorView.kind === "packs" ? "active" : ""}`}
          onClick={() => setActiveView({ kind: "packs" })}
        >
          <Package size={15} />
          <span>Packs</span>
        </button>
      </div>

      {parkedCount > 0 && (
        <div className="skill-sidebar-section">
          <button
            className={`skill-sidebar-item ${
              anchorView.kind === "skills" && skillListFilter.scope === "parked" ? "active" : ""
            }`}
            onClick={() => {
              setSkillListFilter({ ...defaultSkillListFilter(), scope: "parked" });
              setActiveView({ kind: "skills" });
            }}
          >
            <PackageOpen size={15} />
            <span>Parked</span>
            <span className="skill-sidebar-badge count-tabular">{parkedCount}</span>
          </button>
        </div>
      )}

      <div className="skill-sidebar-footer">
        <span className="skill-sidebar-scanned-at">{relativeScanTime(snapshot?.scanned_at)}</span>
        <button
          className="skill-sidebar-refresh"
          onClick={handleRefresh}
          disabled={spinning}
          title="Rescan installed skills"
        >
          <RefreshCw size={13} className={spinning ? "skill-sidebar-refresh-spinning" : ""} />
        </button>
      </div>
    </nav>
  );
}
