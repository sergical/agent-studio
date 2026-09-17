// ============================================================================
// Sidebar - Left-hand navigation: places only (Home, Skills, Activity,
// Parked). Filters (scope, harness, source, issue) live in the Skills view's
// filter bar instead - see the design rule in spec-ux-1.md section B.
// ============================================================================

import { useEffect, useState } from "react";
import {
  Activity as ActivityIcon,
  BookOpen,
  Layers,
  LayoutDashboard,
  Moon,
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
import { SHORTCUTS } from "../../lib/app-shortcuts";
import {
  hasNewerSkillSnapshotEmission,
  rescanTooltip,
  sidebarAnchorView,
} from "../../lib/sidebar-nav";
import { useAppStore } from "../../store/appStore";
import type { ActiveView } from "../../store/appStore";
import { TooltipControl } from "../ui/TooltipControl";
import type { ResolvedTheme, Theme } from "../../lib/theme";
import type { SkillListFilter, SkillSnapshot } from "@skill-studio/lib";

interface SidebarProps {
  snapshot: SkillSnapshot | undefined;
  emittedSnapshotRevision: number | undefined;
  requestRescan: () => Promise<void>;
}

const itemClass = (active: boolean) =>
  `grid h-6.5 w-full grid-cols-[14px_minmax(0,1fr)_auto] items-center gap-2 rounded-sm px-2 text-left text-body ${active ? "bg-bg-active text-text-primary" : "text-text-secondary hover:bg-bg-hover hover:text-text-primary"} active:bg-bg-pressed`;
const iconButtonClass =
  "rounded-sm text-text-tertiary hover:text-text-primary active:bg-bg-pressed";

/**
 * Tracks one rescan at a time: `spinning` is fully derived from the pending
 * revision and the store's latest emission, so once a newer snapshot lands
 * this reads `false` on its own, with no effect needed to clear it back to
 * null.
 */
function useSidebarRescan(
  snapshotRevision: number | undefined,
  emittedSnapshotRevision: number | undefined,
  requestRescan: () => Promise<void>,
) {
  const [pendingRescanSnapshotRevision, setPendingRescanSnapshotRevision] = useState<number | null>(
    null,
  );
  const spinning =
    pendingRescanSnapshotRevision !== null &&
    !hasNewerSkillSnapshotEmission(pendingRescanSnapshotRevision, emittedSnapshotRevision);

  const handleRefresh = async () => {
    if (spinning) return;
    setPendingRescanSnapshotRevision(snapshotRevision ?? 0);
    try {
      await requestRescan();
    } catch {
      setPendingRescanSnapshotRevision(null);
    }
  };

  return { spinning, handleRefresh };
}

interface SidebarNavItemsProps {
  anchorView: ActiveView;
  skillsActive: boolean;
  skillsCount: number;
  pluginCount: number;
  inParked: boolean;
  setActiveView: (view: ActiveView) => void;
  setSkillListFilter: (patch: Partial<SkillListFilter>) => void;
}

/** The three places: Home, Skills (with an optional Plugins place beside it), Activity. */
function SidebarNavItems({
  anchorView,
  skillsActive,
  skillsCount,
  pluginCount,
  inParked,
  setActiveView,
  setSkillListFilter,
}: SidebarNavItemsProps) {
  return (
    <div className="flex flex-col gap-px pt-2.5">
      <Button
        variant="ghost"
        className={itemClass(anchorView.kind === "home")}
        aria-current={anchorView.kind === "home" ? "page" : undefined}
        onClick={() => setActiveView({ kind: "home" })}
      >
        <LayoutDashboard size={14} />
        <span className="min-w-0 truncate">Home</span>
      </Button>
      <Button
        variant="ghost"
        className={itemClass(skillsActive)}
        aria-current={skillsActive ? "page" : undefined}
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
          aria-current={anchorView.kind === "plugins" ? "page" : undefined}
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
        aria-current={anchorView.kind === "activity" ? "page" : undefined}
        onClick={() => setActiveView({ kind: "activity" })}
      >
        <ActivityIcon size={14} />
        <span className="min-w-0 truncate">Activity</span>
      </Button>
    </div>
  );
}

interface SidebarParkedSectionProps {
  anchorView: ActiveView;
  inParked: boolean;
  parkedCount: number;
  setActiveView: (view: ActiveView) => void;
  setSkillListFilter: (patch: Partial<SkillListFilter>) => void;
}

/** Parked is a sub-section of the skills list, shown only when non-empty. */
function SidebarParkedSection({
  anchorView,
  inParked,
  parkedCount,
  setActiveView,
  setSkillListFilter,
}: SidebarParkedSectionProps) {
  if (parkedCount === 0) return null;
  const active = anchorView.kind === "skills" && inParked;
  return (
    <div className="flex flex-col gap-px pt-3">
      <Button
        variant="ghost"
        className={itemClass(active)}
        aria-current={active ? "page" : undefined}
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
  );
}

interface SidebarFooterProps {
  anchorView: ActiveView;
  scannedAt: string | undefined;
  spinning: boolean;
  onRefresh: () => void;
  setActiveView: (view: ActiveView) => void;
  resolvedTheme: ResolvedTheme;
  setTheme: (theme: Theme) => void;
}

/** The snapshot's age and a manual rescan button, plus Learn, Settings, and the theme toggle. */
function SidebarFooter({
  anchorView,
  scannedAt,
  spinning,
  onRefresh,
  setActiveView,
  resolvedTheme,
  setTheme,
}: SidebarFooterProps) {
  return (
    <div className="mt-auto flex select-none items-center justify-between gap-2 px-2 py-1.5">
      <TooltipControl content={rescanTooltip(scannedAt)}>
        <Button
          variant="ghost"
          size="xs"
          className="shrink-0 gap-1.5 rounded-sm px-1.5 text-text-tertiary"
          onClick={onRefresh}
          aria-disabled={spinning}
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
        <TooltipControl content="Settings" shortcut={SHORTCUTS.settings.keys}>
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
            aria-label={resolvedTheme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
          >
            {resolvedTheme === "dark" ? <Moon size={13} /> : <Sun size={13} />}
          </Button>
        </TooltipControl>
      </div>
    </div>
  );
}

/**
 * Left-hand navigation: a search box that jumps into Skills with a query,
 * Add skill, the three places (Home, Skills, Activity), Parked (when
 * non-empty), and a footer with the snapshot's age and a manual rescan
 * button.
 */
export function Sidebar({ snapshot, emittedSnapshotRevision, requestRescan }: SidebarProps) {
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

  function goToSearch() {
    if (anchorView.kind !== "skills") setActiveView({ kind: "skills" });
    requestSkillSearchFocus();
  }

  const { spinning, handleRefresh } = useSidebarRescan(
    snapshot?.revision,
    emittedSnapshotRevision,
    requestRescan,
  );

  return (
    <nav className="flex w-60 shrink-0 flex-col overflow-hidden">
      <div data-tauri-drag-region className="h-9 shrink-0" />
      {/* 41px = the page panel's 1px top border plus PageShell's 40px header, so this title sits on
          the same line as the page title. */}
      <div className="flex h-[41px] shrink-0 items-center justify-between pr-1.5 pl-3.5">
        <span className="text-small font-semibold text-text-primary">Skill Studio</span>
        <div className="flex items-center gap-0.5">
          {/* The sidebar switcher row only fits two icons without crowding, so the command
              palette's ⌘K hint rides along on this tooltip instead of a third icon button. */}
          <TooltipControl
            content={["Search skills", "Command palette ⌘K"]}
            shortcut={SHORTCUTS.filterSkills.keys}
          >
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
          <TooltipControl content="Add skill" shortcut={SHORTCUTS.addSkill.keys}>
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

      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto pl-2 gutter-pr-2">
        <SidebarNavItems
          anchorView={anchorView}
          skillsActive={skillsActive}
          skillsCount={skillsCount}
          pluginCount={pluginCount}
          inParked={inParked}
          setActiveView={setActiveView}
          setSkillListFilter={setSkillListFilter}
        />
        <SidebarParkedSection
          anchorView={anchorView}
          inParked={inParked}
          parkedCount={parkedCount}
          setActiveView={setActiveView}
          setSkillListFilter={setSkillListFilter}
        />
      </div>

      <SidebarFooter
        anchorView={anchorView}
        scannedAt={snapshot?.scanned_at}
        spinning={spinning}
        onRefresh={handleRefresh}
        setActiveView={setActiveView}
        resolvedTheme={resolvedTheme}
        setTheme={setTheme}
      />
    </nav>
  );
}
