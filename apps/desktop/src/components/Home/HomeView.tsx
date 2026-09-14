// ============================================================================
// HomeView - "What needs doing" for the user's own skills: stat tiles for
// broken/warnings/updates, a lane card for invocation and prompt cost, and a
// grouped inbox list (Broken, Warnings, Updates, Not used in 30 days,
// Recently used). See popover-spec.md for the markup and copy this is built
// from.
// ============================================================================

import { useState } from "react";
import { Button } from "@skill-studio/ui";
import {
  defaultSkillListFilter,
  formatTokens,
  homeInvocationCounts,
  homePromptCost,
} from "@skill-studio/lib";
import type {
  HealthIssue,
  InstalledSkill,
  InvocationPolicy,
  LifecycleTarget,
  SkillListFilter,
  SkillSnapshot,
} from "@skill-studio/lib";
import { lifecycleTargetForHarnessRoot } from "../../lib/skill-lifecycle-target";
import { useAppStore } from "../../store/appStore";
import { PageShell } from "../Shell/PageShell";
import { InfoPopover } from "../ui/InfoPopover";
import { MaterializeRootDialog } from "../ui/MaterializeRootDialog";
import { TooltipControl } from "../ui/TooltipControl";
import { HomeInboxGrid } from "./HomeInboxGroups";
import { buildHomeRowPlan, computeHomeGroups } from "./home-inbox-data";
import type { HomeFilter } from "./home-inbox-data";
import { useHomeGroupVisibility } from "./useHomeGroupVisibility";
import { useHomeRowCursor } from "./useHomeRowCursor";

interface HomeViewProps {
  snapshot: SkillSnapshot | undefined;
  isLoading: boolean;
  onSelectSkill: (name: string) => void;
  /** Whether Home is the view on screen right now - `false` while it's kept mounted but hidden
   * behind an open skill's page, so its window-level keyboard shortcuts stay off. */
  active: boolean;
}

/**
 * First-scan loading state, mirroring the loaded layout's dimensions (stat
 * tiles, lane card, list rows) so the dashboard fades in without a layout
 * jump - and so "empty" text only ever means the scan really found nothing.
 */
function HomeSkeleton() {
  const bar = "animate-pulse rounded-xs bg-bg-tertiary motion-reduce:animate-none";
  return (
    <PageShell title="Home">
      <div className="grid grid-cols-3 gap-3" aria-hidden="true">
        {["Broken", "Warnings", "Updates"].map((label) => (
          <div
            key={label}
            className="flex flex-1 flex-col gap-1 rounded-md border border-border-subtle bg-bg-elevated px-4 py-3.5"
          >
            <span className="text-caption tracking-[0.06em] text-text-tertiary uppercase">
              {label}
            </span>
            <span className="text-display leading-[1.1] font-semibold">
              <span className={`inline-block h-[1em] w-7 align-middle ${bar}`} />
            </span>
          </div>
        ))}
      </div>
      <section
        className="flex flex-col gap-2 rounded-md border border-border-subtle bg-bg-elevated px-4 py-3.5"
        aria-hidden="true"
      >
        {["Who can invoke", "Prompt cost"].map((label) => (
          <div key={label} className="grid grid-cols-[210px_minmax(0,1fr)] items-center gap-3">
            <span className="text-small whitespace-nowrap text-text-secondary">{label}</span>
            <div className={`h-7 ${bar}`} />
          </div>
        ))}
      </section>
      <div className="flex flex-col gap-px pt-2" aria-hidden="true">
        {[0, 1, 2, 3].map((i) => (
          <div key={i} className="flex h-11 items-center gap-3 px-3">
            <div className={`size-5 shrink-0 rounded-full ${bar}`} />
            <div className={`h-3.5 ${bar}`} style={{ width: `${34 - i * 6}%` }} />
          </div>
        ))}
      </div>
      <span className="sr-only" role="status">
        Scanning installed skills…
      </span>
    </PageShell>
  );
}

/** The Broken/Warnings/Updates stat-tile row - each a toggle for `HomeFilter`, the first two with an `InfoPopover`. */
function HomeStatTiles({
  broken,
  warnings,
  updates,
  filter,
  toggleFilter,
  onLearnMore,
}: {
  broken: HealthIssue[];
  warnings: HealthIssue[];
  updates: InstalledSkill[];
  filter: HomeFilter | null;
  toggleFilter: (id: HomeFilter) => void;
  onLearnMore: () => void;
}) {
  return (
    <div className="grid grid-cols-3 gap-3">
      <div className="group/stat relative flex">
        <Button
          variant="outline"
          className={`h-auto flex-1 flex-col items-stretch gap-1 rounded-md border-border-subtle bg-bg-elevated px-4 py-3.5 justify-start text-left active:translate-y-0 active:scale-98 aria-pressed:border-accent aria-pressed:bg-accent-softer ${
            broken.length > 0 ? "[&_.home-stat-value]:text-error" : ""
          }`}
          aria-pressed={filter === "broken"}
          onClick={() => toggleFilter("broken")}
        >
          <span
            className={`flex items-center gap-2 text-caption tracking-[0.06em] uppercase ${filter === "broken" ? "text-accent" : "text-text-tertiary"}`}
          >
            Broken
          </span>
          <span className="home-stat-value text-display leading-[1.1] font-semibold tracking-[-0.02em] tabular-nums">
            {broken.length}
          </span>
        </Button>
        <span className="absolute top-3.5 right-3.5 opacity-0 group-hover/stat:opacity-100 group-focus-within/stat:opacity-100 has-[[aria-expanded=true]]:opacity-100">
          <InfoPopover label="About broken" title="Broken and warnings" onLearnMore={onLearnMore}>
            An agent loads nothing, or something you did not intend: a dead link, a SKILL.md the
            loader rejects, a parked skill that was reinstalled.
          </InfoPopover>
        </span>
      </div>

      <div className="group/stat relative flex">
        <Button
          variant="outline"
          className={`h-auto flex-1 flex-col items-stretch gap-1 rounded-md border-border-subtle bg-bg-elevated px-4 py-3.5 justify-start text-left active:translate-y-0 active:scale-98 aria-pressed:border-accent aria-pressed:bg-accent-softer ${
            warnings.length > 0 ? "[&_.home-stat-value]:text-warning" : ""
          }`}
          aria-pressed={filter === "warn"}
          onClick={() => toggleFilter("warn")}
        >
          <span
            className={`flex items-center gap-2 text-caption tracking-[0.06em] uppercase ${filter === "warn" ? "text-accent" : "text-text-tertiary"}`}
          >
            Warnings
          </span>
          <span className="home-stat-value text-display leading-[1.1] font-semibold tracking-[-0.02em] tabular-nums">
            {warnings.length}
          </span>
        </Button>
        <span className="absolute top-3.5 right-3.5 opacity-0 group-hover/stat:opacity-100 group-focus-within/stat:opacity-100 has-[[aria-expanded=true]]:opacity-100">
          <InfoPopover label="About warnings" title="Broken and warnings" onLearnMore={onLearnMore}>
            Everything still loads, but the state drifted: copies that differ between harnesses,
            lock-file entries with no folder on disk.
          </InfoPopover>
        </span>
      </div>

      <div className="flex">
        <Button
          variant="outline"
          className="h-auto flex-1 flex-col items-stretch gap-1 rounded-md border-border-subtle bg-bg-elevated px-4 py-3.5 justify-start text-left active:translate-y-0 active:scale-98 aria-pressed:border-accent aria-pressed:bg-accent-softer"
          aria-pressed={filter === "upd"}
          onClick={() => toggleFilter("upd")}
        >
          <span
            className={`flex items-center gap-2 text-caption tracking-[0.06em] uppercase ${filter === "upd" ? "text-accent" : "text-text-tertiary"}`}
          >
            Updates
          </span>
          <span className="text-display leading-[1.1] font-semibold tracking-[-0.02em] tabular-nums">
            {updates.length}
          </span>
        </Button>
      </div>
    </div>
  );
}

/** Home's lane card: "Who can invoke" and "Prompt cost" segmented bars, each segment a filter/nav shortcut. */
function InvocationCostCard({
  inv,
  cost,
  filter,
  onLearnMoreInvoke,
  onLearnMoreCost,
  goToInvocation,
  goToSkills,
  toggleFilter,
}: {
  inv: ReturnType<typeof homeInvocationCounts>;
  cost: ReturnType<typeof homePromptCost>;
  filter: HomeFilter | null;
  onLearnMoreInvoke: () => void;
  onLearnMoreCost: () => void;
  goToInvocation: (invocation: InvocationPolicy) => void;
  goToSkills: (patch: Partial<SkillListFilter>) => void;
  toggleFilter: (id: HomeFilter) => void;
}) {
  const invokeTotal = inv.both + inv.modelOnly + inv.userOnly;
  return (
    <section className="flex flex-col gap-2 rounded-md border border-border-subtle bg-bg-elevated px-4 py-3.5">
      <div className="grid grid-cols-[210px_minmax(0,1fr)] items-baseline gap-3">
        <span className="flex items-baseline gap-x-1 whitespace-nowrap text-small text-text-secondary">
          Who can invoke
          <b className="ml-1 font-normal text-text-primary tabular-nums">{invokeTotal}</b>
          <InfoPopover
            className="self-center"
            label="About invocation"
            title="Who can invoke a skill"
            onLearnMore={onLearnMoreInvoke}
          >
            Read from SKILL.md frontmatter. Claude Code honours both limits, pi only the you-only
            one; Codex and OpenCode use their own config.
          </InfoPopover>
        </span>
        <div className="flex h-7 gap-0.5" role="group" aria-label="Who can invoke">
          {inv.both > 0 && (
            <TooltipControl content="Open in Skills">
              <Button
                size="sm"
                className="gap-1 overflow-hidden rounded-xs bg-accent-soft px-2.5 text-small whitespace-nowrap text-text-primary hover:brightness-115 aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent)]"
                style={{ flex: `${inv.both} 0 auto` }}
                onClick={() => goToInvocation("both")}
              >
                <span className="tabular-nums">{inv.both}</span> you or the model
              </Button>
            </TooltipControl>
          )}
          {inv.modelOnly > 0 && (
            <TooltipControl content="Open in Skills">
              <Button
                variant="secondary"
                size="sm"
                className="gap-1 overflow-hidden rounded-xs bg-accent-softer px-2.5 text-small whitespace-nowrap text-text-secondary hover:brightness-115 aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent)] aria-pressed:text-text-primary"
                style={{ flex: `${inv.modelOnly} 0 auto` }}
                onClick={() => goToInvocation("model-only")}
              >
                <span className="tabular-nums">{inv.modelOnly}</span> model only
              </Button>
            </TooltipControl>
          )}
          {inv.userOnly > 0 && (
            <TooltipControl content="Open in Skills">
              <Button
                variant="secondary"
                size="sm"
                className="gap-1 overflow-hidden rounded-xs bg-bg-tertiary px-2.5 text-small whitespace-nowrap text-text-secondary hover:brightness-115 aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent)] aria-pressed:text-text-primary"
                style={{ flex: `${inv.userOnly} 0 auto` }}
                onClick={() => goToInvocation("user-only")}
              >
                <span className="tabular-nums">{inv.userOnly}</span> you only
              </Button>
            </TooltipControl>
          )}
        </div>
      </div>

      <div className="grid grid-cols-[210px_minmax(0,1fr)] items-baseline gap-3">
        <span className="flex items-baseline gap-x-1 whitespace-nowrap text-small text-text-secondary">
          Prompt cost
          <b className="ml-1 font-normal text-text-primary tabular-nums">
            {formatTokens(cost.totalTokens)}
          </b>
          <InfoPopover
            className="self-center"
            label="About prompt cost"
            title="Prompt cost"
            onLearnMore={onLearnMoreCost}
          >
            Tokens of name and description the model reads every turn. Only skills the model may
            invoke count; user-only skills cost nothing until you run them.
          </InfoPopover>
        </span>
        <div className="flex h-7 gap-0.5" role="group" aria-label="Prompt cost">
          {cost.totalTokens === 0 ? (
            <span
              className="inline-flex items-center gap-1 overflow-hidden rounded-xs bg-bg-tertiary px-2.5 text-small whitespace-nowrap text-text-secondary"
              style={{ width: "100%" }}
            >
              No model-invocable skills
            </span>
          ) : (
            <>
              <TooltipControl content="Open in Skills">
                <Button
                  size="sm"
                  className="gap-1 overflow-hidden rounded-xs bg-accent-soft px-2.5 text-small whitespace-nowrap text-text-primary hover:brightness-115"
                  style={{ flex: `${cost.usedTokens} 0 auto` }}
                  onClick={() => goToSkills({ usage: "used-30d" })}
                >
                  <span className="tabular-nums">{formatTokens(cost.usedTokens)}</span> ·{" "}
                  <span className="tabular-nums">{cost.usedCount}</span> skills used in 30 days
                </Button>
              </TooltipControl>
              <TooltipControl content="Show the skills not used in 30 days">
                <Button
                  variant="secondary"
                  size="sm"
                  className="gap-1 overflow-hidden rounded-xs bg-bg-tertiary px-2.5 text-small whitespace-nowrap text-text-secondary hover:brightness-115 aria-pressed:text-text-primary aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent)]"
                  style={{ flex: `${cost.idleTokens} 0 auto` }}
                  aria-pressed={filter === "unused"}
                  onClick={() => toggleFilter("unused")}
                >
                  <span className="tabular-nums">{formatTokens(cost.idleTokens)}</span> ·{" "}
                  <span className="tabular-nums">{cost.idleCount}</span> skills not used in 30 days
                </Button>
              </TooltipControl>
            </>
          )}
        </div>
      </div>
    </section>
  );
}

/**
 * Home: stat tiles, a lane card for invocation and prompt cost, and a
 * grouped inbox list - the columns any of these three surfaces would
 * otherwise leave the user to reconstruct by hand.
 */
export function HomeView({ snapshot, isLoading, onSelectSkill, active }: HomeViewProps) {
  const setActiveView = useAppStore((state) => state.setActiveView);
  const setSkillListFilter = useAppStore((state) => state.setSkillListFilter);
  const openSkill = useAppStore((state) => state.openSkill);

  const [linkedRootDialog, setLinkedRootDialog] = useState<{
    target: LifecycleTarget;
    harness: string;
    harnessLabel: string;
    root: string;
  } | null>(null);

  // Every Hook below must run on every render (Home's skeleton/empty states return early, further
  // down, only after they've all been called), so the derived data they depend on falls back to
  // empty rather than gating on `snapshot` here.
  const groups = computeHomeGroups(snapshot);
  const { broken, warnings, updates, inv, cost } = groups;

  const {
    filter,
    setFilter,
    toggleFilter,
    setCollapsedGroups,
    isGroupVisible,
    isGroupExpanded,
    toggleGroup,
  } = useHomeGroupVisibility();

  const goToSkills = (patch: Parameters<typeof setSkillListFilter>[0]) => {
    setSkillListFilter({ ...defaultSkillListFilter(), ...patch });
    setActiveView({ kind: "skills" });
  };
  const goToInvocation = (invocation: InvocationPolicy) => goToSkills({ invocation });

  // Row semantics match Skills: one continuous `aria-rowindex` across every visible group, and
  // `tabIndex={0}` on only the very first rendered row.
  const { starts, visibleKeys, openByKey } = buildHomeRowPlan({
    groups,
    isGroupVisible,
    isGroupExpanded,
    onSelectSkill,
  });

  const { rowRef, containerRef, tabIndexFor, onGridKeyDown, statusText } = useHomeRowCursor({
    visibleKeys,
    openByKey,
    active,
    setCollapsedGroups,
  });

  if (!snapshot) {
    if (isLoading) {
      return <HomeSkeleton />;
    }
    return (
      <PageShell title="Home">
        <p className="flex h-full items-center justify-center text-wrap-pretty text-text-tertiary">
          No skill snapshot yet.
        </p>
      </PageShell>
    );
  }

  return (
    <PageShell title="Home">
      <HomeStatTiles
        broken={broken}
        warnings={warnings}
        updates={updates}
        filter={filter}
        toggleFilter={toggleFilter}
        onLearnMore={() => setActiveView({ kind: "learn", section: "broken" })}
      />

      <InvocationCostCard
        inv={inv}
        cost={cost}
        filter={filter}
        onLearnMoreInvoke={() => setActiveView({ kind: "learn", section: "invoke" })}
        onLearnMoreCost={() => setActiveView({ kind: "learn", section: "cost" })}
        goToInvocation={goToInvocation}
        goToSkills={goToSkills}
        toggleFilter={toggleFilter}
      />

      <HomeInboxGrid
        groups={groups}
        starts={starts}
        filter={filter}
        onClearFilter={() => setFilter(null)}
        isGroupVisible={isGroupVisible}
        isGroupExpanded={isGroupExpanded}
        toggleGroup={toggleGroup}
        onSelectSkill={onSelectSkill}
        onShowAllIssues={() => goToSkills({ issue: "any" })}
        onShowAllUpdates={() => setActiveView({ kind: "skills" })}
        onShowAllUnused={() => goToSkills({ usage: "unused-30d" })}
        onShowAllRecent={() => setActiveView({ kind: "activity" })}
        openSkill={openSkill}
        onConvertLinkedRoot={(skill, harness, harnessLabel, root) =>
          setLinkedRootDialog({
            target: lifecycleTargetForHarnessRoot(skill, harness, root),
            harness,
            harnessLabel,
            root,
          })
        }
        containerRef={containerRef}
        onGridKeyDown={onGridKeyDown}
        rowRef={rowRef}
        tabIndexFor={tabIndexFor}
      />
      {/* Visually-hidden live region: announces the cursor's position, debounced to the last move. */}
      <div role="status" aria-live="polite" className="sr-only">
        {statusText}
      </div>

      {linkedRootDialog && (
        <MaterializeRootDialog
          target={linkedRootDialog.target}
          harness={linkedRootDialog.harness}
          harnessLabel={linkedRootDialog.harnessLabel}
          root={linkedRootDialog.root}
          intent={{ kind: "convert-only" }}
          onClose={() => setLinkedRootDialog(null)}
        />
      )}
    </PageShell>
  );
}
