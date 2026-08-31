// ============================================================================
// Sidebar - Left-hand navigation: places only (Home, Skills, Activity, Packs,
// Parked). Filters (scope, harness, source, issue) live in the Skills view's
// filter bar instead - see the design rule in spec-ux-1.md section B.
// ============================================================================

import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import {
  Activity as ActivityIcon,
  BookOpen,
  LayoutDashboard,
  Moon,
  Package,
  PackageOpen,
  Plus,
  Puzzle,
  RefreshCw,
  Search,
  Sun,
} from "lucide-react";
import { ownSkillsView, pluginSkillsView } from "@skill-studio/lib";
import { defaultSkillListFilter } from "@skill-studio/lib";
import { isFeatureEnabled } from "../../lib/feature-flags";
import { useAppStore } from "../../store/appStore";
import type { ActiveView } from "../../store/appStore";
import type { SkillSnapshot } from "@skill-studio/lib";

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
  const resolvedTheme = useAppStore((state) => state.resolvedTheme);
  const setTheme = useAppStore((state) => state.setTheme);
  const searchInputRef = useRef<HTMLInputElement>(null);

  const own = ownSkillsView(snapshot?.skills ?? []);
  const skillsCount = own.length;
  const parkedCount = own.filter((s) => s.parked).length;
  const pluginCount = pluginSkillsView(snapshot?.skills ?? []).length;
  // Parked is a sub-section of the skills list, so the Skills row is only
  // "current" when that partition isn't selected. Plugin skills are their
  // own place (see PluginSkillsView), not a filter on Skills.
  const inParked = skillListFilter.scope === "parked";
  const skillsActive = anchorView.kind === "skills" && !inParked;
  const packsEnabled = isFeatureEnabled("skill-packs");

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

  const itemClass = (active: boolean) =>
    `grid h-[30px] w-full cursor-pointer grid-cols-[15px_minmax(0,1fr)_auto] items-center gap-2 rounded-sm border-0 px-2.5 text-left text-body transition-colors ${
      active
        ? "bg-accent-soft text-text-primary"
        : "bg-transparent text-text-secondary hover:bg-bg-hover hover:text-text-primary"
    }`;
  const iconButtonClass =
    "flex size-6 cursor-pointer items-center justify-center rounded-sm border-0 bg-transparent text-text-tertiary transition-colors hover:bg-bg-hover hover:text-text-primary";

  return (
    <nav className="flex w-60 shrink-0 flex-col overflow-y-auto border-r border-border bg-bg-secondary">
      <div className="flex flex-col gap-px px-2.5 pb-2.5 first:pt-3">
        <input
          ref={searchInputRef}
          type="search"
          aria-label="Search skills"
          className="h-8 rounded-sm border border-border bg-bg-primary px-3 text-body text-text-primary transition-colors duration-150 placeholder:text-text-quaternary focus-visible:border-border-focus"
          placeholder="Search skills…"
          value={skillListFilter.query}
          onChange={(e) => handleSearchChange(e.target.value)}
          onKeyDown={handleSearchKeyDown}
        />
        <button
          className="mt-1.5 flex h-[30px] cursor-pointer items-center justify-center gap-1.5 rounded-sm border-0 bg-accent-soft text-body text-text-primary transition-colors hover:text-accent-hover"
          onClick={() => openAddSkillSheet()}
        >
          <Plus size={15} />
          <span>Add skill</span>
        </button>
      </div>

      <div className="flex flex-col gap-px px-2.5 pb-2.5 first:pt-3">
        <button
          className={itemClass(anchorView.kind === "home")}
          onClick={() => setActiveView({ kind: "home" })}
        >
          <LayoutDashboard size={15} />
          <span className="min-w-0 truncate">Home</span>
        </button>
        <button
          className={itemClass(skillsActive)}
          onClick={() => {
            if (inParked) setSkillListFilter(defaultSkillListFilter());
            setActiveView({ kind: "skills" });
          }}
        >
          <Search size={15} />
          <span className="min-w-0 truncate">Skills</span>
          {skillsCount > 0 && (
            <span className="text-right text-caption tabular-nums text-text-tertiary">
              {skillsCount}
            </span>
          )}
        </button>
        {pluginCount > 0 && (
          <button
            className={itemClass(anchorView.kind === "plugins")}
            onClick={() => setActiveView({ kind: "plugins" })}
          >
            <Puzzle size={15} />
            <span className="min-w-0 truncate">Plugins</span>
            <span className="text-right text-caption tabular-nums text-text-tertiary">
              {pluginCount}
            </span>
          </button>
        )}
        <button
          className={itemClass(anchorView.kind === "activity")}
          onClick={() => setActiveView({ kind: "activity" })}
        >
          <ActivityIcon size={15} />
          <span className="min-w-0 truncate">Activity</span>
        </button>
        {packsEnabled && (
          <button
            className={itemClass(anchorView.kind === "packs")}
            onClick={() => setActiveView({ kind: "packs" })}
          >
            <Package size={15} />
            <span className="min-w-0 truncate">Packs</span>
          </button>
        )}
      </div>

      {parkedCount > 0 && (
        <div className="flex flex-col gap-px px-2.5 pb-2.5 first:pt-3">
          <button
            className={itemClass(anchorView.kind === "skills" && inParked)}
            onClick={() => {
              setSkillListFilter({ ...defaultSkillListFilter(), scope: "parked" });
              setActiveView({ kind: "skills" });
            }}
          >
            <PackageOpen size={15} />
            <span className="min-w-0 truncate">Parked</span>
            <span className="text-right text-caption tabular-nums text-text-tertiary">
              {parkedCount}
            </span>
          </button>
        </div>
      )}

      <div className="mt-auto flex select-none items-center justify-between gap-2 border-t border-border-subtle px-2.5 py-2">
        <button
          type="button"
          className="flex h-6 cursor-pointer items-center gap-1.5 rounded-sm border-0 bg-transparent px-1.5 text-caption text-text-tertiary transition-colors hover:enabled:bg-bg-hover hover:enabled:text-text-primary disabled:cursor-default"
          onClick={handleRefresh}
          disabled={spinning}
          title="Rescan installed skills"
        >
          <RefreshCw size={13} className={spinning ? "animate-spin" : ""} />
          <span>
            {spinning ? "Scanning…" : `Scanned ${relativeScanTime(snapshot?.scanned_at)}`}
          </span>
        </button>
        <div className="flex items-center gap-0.5">
          <button
            type="button"
            className={`${iconButtonClass} aria-[current=page]:bg-accent-softer aria-[current=page]:text-accent`}
            onClick={() => setActiveView({ kind: "learn" })}
            aria-current={anchorView.kind === "learn" ? "page" : undefined}
            aria-label="Learn"
            title="Learn"
          >
            <BookOpen size={13} />
          </button>
          <button
            type="button"
            className={iconButtonClass}
            onClick={() => setTheme(resolvedTheme === "dark" ? "light" : "dark")}
            aria-label={resolvedTheme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
            title={resolvedTheme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
          >
            {resolvedTheme === "dark" ? <Moon size={13} /> : <Sun size={13} />}
          </button>
        </div>
      </div>
    </nav>
  );
}
