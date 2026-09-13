// ============================================================================
// Sidebar - Left-hand navigation: places only (Home, Skills, Activity, Packs,
// Parked). Filters (scope, harness, source, issue) live in the Skills view's
// filter bar instead - see the design rule in spec-ux-1.md section B.
// ============================================================================

import { useEffect, useRef, useState } from "react";
import {
  Activity as ActivityIcon,
  BookOpen,
  Layers,
  LayoutDashboard,
  Moon,
  Package,
  PackageOpen,
  Plus,
  Puzzle,
  RefreshCw,
  Search,
  Settings as SettingsIcon,
  Sun,
} from "lucide-react";
import { Button } from "@skill-studio/ui";
import { ownSkillsView, pluginSkillsView } from "@skill-studio/lib";
import { defaultSkillListFilter } from "@skill-studio/lib";
import { isFeatureEnabled } from "../../lib/feature-flags";
import {
  hasNewerSkillSnapshotEmission,
  rescanTooltip,
  sidebarAnchorView,
} from "../../lib/sidebar-nav";
import { useAppStore } from "../../store/appStore";
import { TooltipControl } from "../ui/TooltipControl";
import type { SkillSnapshot } from "@skill-studio/lib";

interface SidebarProps {
  snapshot: SkillSnapshot | undefined;
  emittedSnapshotRevision: number | undefined;
  requestRescan: () => Promise<void>;
}

/**
 * Left-hand navigation: a search box that jumps into Skills with a query,
 * Add skill, the four places (Home, Skills, Activity, Packs), Parked (when
 * non-empty), and a footer with the snapshot's age and a manual rescan
 * button.
 */
export function Sidebar({ snapshot, emittedSnapshotRevision, requestRescan }: SidebarProps) {
  const [pendingRescanSnapshotRevision, setPendingRescanSnapshotRevision] = useState<number | null>(
    null,
  );
  const activeRescanIdRef = useRef<symbol | null>(null);
  // Forces the footer to re-render so "just now" ages into "1m ago" and
  // beyond without waiting for the next snapshot - relativeScanTime() itself
  // stays a pure function of scannedAt and the current clock.
  const [, forceTick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => forceTick((n) => n + 1), 30_000);
    return () => clearInterval(id);
  }, []);
  useEffect(() => {
    return () => {
      activeRescanIdRef.current = null;
    };
  }, []);
  const activeView = useAppStore((state) => state.activeView);
  const anchorView = sidebarAnchorView(activeView);
  const setActiveView = useAppStore((state) => state.setActiveView);
  const skillListFilter = useAppStore((state) => state.skillListFilter);
  const setSkillListFilter = useAppStore((state) => state.setSkillListFilter);
  const openAddSkillSheet = useAppStore((state) => state.openAddSkillSheet);
  const resolvedTheme = useAppStore((state) => state.resolvedTheme);
  const setTheme = useAppStore((state) => state.setTheme);
  const requestSkillSearchFocus = useAppStore((state) => state.requestSkillSearchFocus);

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

  function goToSearch() {
    if (anchorView.kind !== "skills") setActiveView({ kind: "skills" });
    requestSkillSearchFocus();
  }

  useEffect(() => {
    if (
      activeRescanIdRef.current !== null &&
      hasNewerSkillSnapshotEmission(
        pendingRescanSnapshotRevision ?? undefined,
        emittedSnapshotRevision,
      )
    ) {
      activeRescanIdRef.current = null;
      setPendingRescanSnapshotRevision(null);
    }
  }, [emittedSnapshotRevision, pendingRescanSnapshotRevision]);

  const handleRefresh = async () => {
    if (activeRescanIdRef.current !== null) return;

    const requestId = Symbol("sidebar-rescan");
    activeRescanIdRef.current = requestId;
    setPendingRescanSnapshotRevision(snapshot?.revision ?? 0);

    try {
      await requestRescan();
    } catch {
      if (activeRescanIdRef.current !== requestId) return;
      activeRescanIdRef.current = null;
      setPendingRescanSnapshotRevision(null);
    }
  };

  const spinning = pendingRescanSnapshotRevision !== null;

  const itemClass = (active: boolean) =>
    `grid h-6.5 w-full grid-cols-[14px_minmax(0,1fr)_auto] items-center gap-2 rounded-sm px-2 text-left text-body ${active ? "bg-bg-active text-text-primary" : "text-text-secondary hover:bg-bg-hover hover:text-text-primary"}`;
  const iconButtonClass = "rounded-sm text-text-tertiary hover:text-text-primary";

  return (
    <nav className="flex w-60 shrink-0 flex-col overflow-hidden">
      <div data-tauri-drag-region className="h-9 shrink-0" />
      <div className="flex h-7 shrink-0 items-center justify-between pr-1.5 pl-3.5">
        <span className="text-small font-semibold text-text-primary">Skill Studio</span>
        <div className="flex items-center gap-0.5">
          <TooltipControl content="Search skills">
            <Button
              variant="ghost"
              size="icon-xs"
              className={iconButtonClass}
              aria-label="Search skills"
              onClick={goToSearch}
            >
              <Search size={14} />
            </Button>
          </TooltipControl>
          <TooltipControl content="Add skill">
            <Button
              variant="ghost"
              size="icon-xs"
              className={iconButtonClass}
              aria-label="Add skill"
              onClick={() => openAddSkillSheet()}
            >
              <Plus size={14} />
            </Button>
          </TooltipControl>
        </div>
      </div>

      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
        <div className="flex flex-col gap-px px-2 pt-2.5">
          <Button
            variant="ghost"
            className={itemClass(anchorView.kind === "home")}
            onClick={() => setActiveView({ kind: "home" })}
          >
            <LayoutDashboard size={14} />
            <span className="min-w-0 truncate">Home</span>
          </Button>
          <Button
            variant="ghost"
            className={itemClass(skillsActive)}
            onClick={() => {
              if (inParked) setSkillListFilter(defaultSkillListFilter());
              setActiveView({ kind: "skills" });
            }}
          >
            <Layers size={14} />
            <span className="min-w-0 truncate">Skills</span>
            {skillsCount > 0 && (
              <span className="text-right text-caption tabular-nums text-text-tertiary">
                {skillsCount}
              </span>
            )}
          </Button>
          {pluginCount > 0 && (
            <Button
              variant="ghost"
              className={itemClass(anchorView.kind === "plugins")}
              onClick={() => setActiveView({ kind: "plugins" })}
            >
              <Puzzle size={14} />
              <span className="min-w-0 truncate">Plugins</span>
              <span className="text-right text-caption tabular-nums text-text-tertiary">
                {pluginCount}
              </span>
            </Button>
          )}
          <Button
            variant="ghost"
            className={itemClass(anchorView.kind === "activity")}
            onClick={() => setActiveView({ kind: "activity" })}
          >
            <ActivityIcon size={14} />
            <span className="min-w-0 truncate">Activity</span>
          </Button>
          {packsEnabled && (
            <Button
              variant="ghost"
              className={itemClass(anchorView.kind === "packs")}
              onClick={() => setActiveView({ kind: "packs" })}
            >
              <Package size={14} />
              <span className="min-w-0 truncate">Packs</span>
            </Button>
          )}
        </div>

        {parkedCount > 0 && (
          <div className="flex flex-col gap-px px-2 pt-3">
            <Button
              variant="ghost"
              className={itemClass(anchorView.kind === "skills" && inParked)}
              onClick={() => {
                setSkillListFilter({ ...defaultSkillListFilter(), scope: "parked" });
                setActiveView({ kind: "skills" });
              }}
            >
              <PackageOpen size={14} />
              <span className="min-w-0 truncate">Parked</span>
              <span className="text-right text-caption tabular-nums text-text-tertiary">
                {parkedCount}
              </span>
            </Button>
          </div>
        )}
      </div>

      <div className="mt-auto flex select-none items-center justify-between gap-2 px-2 py-1.5">
        <TooltipControl content={rescanTooltip(snapshot?.scanned_at)}>
          <Button
            variant="ghost"
            size="xs"
            className="shrink-0 gap-1.5 rounded-sm px-1.5 text-text-tertiary"
            onClick={handleRefresh}
            disabled={spinning}
            aria-label={spinning ? "Syncing installed skills" : "Sync installed skills"}
          >
            <RefreshCw size={13} className={spinning ? "animate-spin" : ""} />
            <span className="whitespace-nowrap">{spinning ? "Syncing…" : "Sync"}</span>
          </Button>
        </TooltipControl>
        <div className="flex items-center gap-0.5">
          <TooltipControl content="Learn">
            <Button
              variant="ghost"
              size="icon-xs"
              className={`${iconButtonClass} aria-[current=page]:bg-accent-softer aria-[current=page]:text-accent`}
              onClick={() => setActiveView({ kind: "learn" })}
              aria-current={anchorView.kind === "learn" ? "page" : undefined}
              aria-label="Learn"
            >
              <BookOpen size={13} />
            </Button>
          </TooltipControl>
          <TooltipControl content="Settings">
            <Button
              variant="ghost"
              size="icon-xs"
              className={`${iconButtonClass} aria-[current=page]:bg-accent-softer aria-[current=page]:text-accent`}
              onClick={() => setActiveView({ kind: "settings" })}
              aria-current={anchorView.kind === "settings" ? "page" : undefined}
              aria-label="Settings"
            >
              <SettingsIcon size={13} />
            </Button>
          </TooltipControl>
          <TooltipControl
            content={resolvedTheme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
          >
            <Button
              variant="ghost"
              size="icon-xs"
              className={iconButtonClass}
              onClick={() => setTheme(resolvedTheme === "dark" ? "light" : "dark")}
              aria-label={
                resolvedTheme === "dark" ? "Switch to light theme" : "Switch to dark theme"
              }
            >
              {resolvedTheme === "dark" ? <Moon size={13} /> : <Sun size={13} />}
            </Button>
          </TooltipControl>
        </div>
      </div>
    </nav>
  );
}
