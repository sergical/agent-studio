// ============================================================================
// HomeView - "What needs doing" for the user's own skills: stat tiles for
// broken/warnings/updates, a lane card for invocation and prompt cost, and a
// grouped inbox list (Broken, Warnings, Updates, Not used in 30 days,
// Recently used). See popover-spec.md for the markup and copy this is built
// from.
// ============================================================================

import { useState } from "react";
import type { ReactNode } from "react";
import { ChevronDown } from "lucide-react";
import { Collapsible, CollapsiblePanel, CollapsibleTrigger } from "@skill-studio/ui";
import {
  attentionGroups,
  homeInvocationCounts,
  homePromptCost,
  recentlyUsedSkills,
  unusedSkills,
} from "@skill-studio/lib";
import { parkSkill, pullForkUpstream, updateSkill } from "../../lib/skill-api";
import {
  lifecycleTargetForHarnessRoot,
  lifecycleTargetForPark,
  updateSkillOwners,
} from "../../lib/skill-lifecycle-target";
import { collectDashboardIssues } from "@skill-studio/lib";
import type { HealthIssue, HealthIssueKind } from "@skill-studio/lib";
import { defaultSkillListFilter } from "@skill-studio/lib";
import { ownSkillsView } from "@skill-studio/lib";
import { formatRelativeTime, formatTokens, shortSha } from "@skill-studio/lib";
import { skillsWithUpdates } from "@skill-studio/lib";
import type {
  AgentId,
  InstalledSkill,
  InvocationPolicy,
  LifecycleTarget,
  SkillListFilter,
  SkillSnapshot,
} from "@skill-studio/lib";
import { useAppStore } from "../../store/appStore";
import { PageShell } from "../Shell/PageShell";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";
import { InfoPopover } from "../ui/InfoPopover";
import { MaterializeRootDialog } from "../ui/MaterializeRootDialog";
import { TooltipControl } from "../ui/TooltipControl";

const RECENTLY_USED_COUNT = 5;
const MAX_ROWS_PER_GROUP = 6;

/** Text link style shared by every "Show all"/"Show everything"/"Learn more" affordance on Home. */
const LINK_CLASS =
  "inline-flex items-center gap-1 border-0 bg-none p-0 text-small text-accent hover:underline";

/** One inbox row's trailing action - a text button or, on the "Recently used" rows, a plain count. */
const ROW_ACTION_CLASS =
  "h-9 min-w-10 border-0 bg-none p-0 text-right text-small text-text-tertiary hover:text-accent";

/** The one filter that can be active at a time: a stat tile or the idle bar segment. */
type HomeFilter = "broken" | "warn" | "upd" | "unused";

/** Every inbox group, in display order - also the key `HomeFilter` narrows to. */
type GroupId = HomeFilter | "rec";

interface HomeViewProps {
  snapshot: SkillSnapshot | undefined;
  isLoading: boolean;
  onSelectSkill: (name: string) => void;
}

/** The harness marks for one skill's deployments - muted when every deployment for that harness is disabled or parked. */
function harnessBadges(skill: InstalledSkill) {
  const byHarness = new Map<AgentId | "shared", boolean[]>();
  for (const deployment of skill.deployments) {
    const id = harnessIdFromLabel(deployment.agent);
    if (!id) continue;
    const active = !deployment.disabled && deployment.scope !== "parked";
    byHarness.set(id, [...(byHarness.get(id) ?? []), active]);
  }
  return [...byHarness.entries()].map(([id, activeFlags]) => ({
    id,
    muted: !activeFlags.some(Boolean),
  }));
}

function HarnessBadges({ skill }: { skill: InstalledSkill }) {
  const badges = harnessBadges(skill);
  if (badges.length === 0) return null;
  return (
    <span className="inline-flex shrink-0 gap-[5px]">
      {badges.map(({ id, muted }) => (
        <span key={id} className="inline-flex text-text-tertiary">
          <HarnessIcon harness={id} size={13} muted={muted} />
        </span>
      ))}
    </span>
  );
}

/** One row of any inbox group: severity dot, name + harness marks, detail, one action. */
function InboxRow({
  severity,
  skill,
  detail,
  action,
  onOpen,
}: {
  severity?: "error" | "warning";
  skill: InstalledSkill;
  detail: ReactNode;
  action: ReactNode;
  onOpen: () => void;
}) {
  return (
    <div className="grid h-9 grid-cols-[6px_minmax(0,260px)_minmax(0,1fr)_auto] items-center gap-3 border-b border-border-subtle py-0 pr-3 pl-4">
      <span
        className={`size-1.5 shrink-0 rounded-full ${severity === "error" ? "bg-error" : severity === "warning" ? "bg-warning" : ""}`}
      />
      <span className="flex min-w-0 items-center gap-2">
        <button
          className="min-w-0 truncate border-0 bg-none p-0 text-left text-body text-text-primary hover:text-accent"
          onClick={onOpen}
          title={skill.name}
        >
          {skill.name}
        </button>
        <HarnessBadges skill={skill} />
      </span>
      <span className="truncate text-small text-text-tertiary">{detail}</span>
      {action}
    </div>
  );
}

/** A group's sticky header: chevron, label, count, spacer, optional extra action. Sits inside a `Collapsible`, whose `data-panel-open` drives the chevron. */
function GroupHead({ label, count, extra }: { label: string; count: number; extra?: ReactNode }) {
  return (
    <CollapsibleTrigger className="group/head sticky top-0 z-1 flex h-8.5 w-full items-center gap-2 border-t border-b border-border-subtle bg-bg-secondary px-3 text-left text-small font-semibold text-text-primary">
      <ChevronDown className="size-3.5 shrink-0 -rotate-90 text-text-quaternary transition-transform group-data-panel-open/head:rotate-0" />
      {label}
      <span className="font-normal text-text-tertiary tabular-nums">{count}</span>
      <span className="flex-1" />
      {extra}
    </CollapsibleTrigger>
  );
}

/** "Show all N" footer link for a group, navigating to the matching Skills/Activity filter. */
function ShowAllLink({
  count,
  label,
  onClick,
}: {
  count: number;
  label: string;
  onClick: () => void;
}) {
  return (
    <div className="flex h-9 items-center px-4">
      <button className={LINK_CLASS} onClick={onClick}>
        {label} {count}
      </button>
    </div>
  );
}

/**
 * "Pull latest" for one Updates row: a fork pulls upstream via
 * `pullForkUpstream`, any other managed skill re-syncs via `updateSkill`.
 */
function PullLatestButton({ skill }: { skill: InstalledSkill }) {
  const [isPulling, setIsPulling] = useState(false);
  const addToast = useAppStore((state) => state.addToast);

  const handlePull = async () => {
    setIsPulling(true);
    try {
      if (skill.source_kind === "fork") {
        const result = await pullForkUpstream(lifecycleTargetForPark(skill));
        addToast({ type: "success", title: result.message ?? `Merged ${skill.name}` });
      } else {
        const summary = await updateSkillOwners(skill, updateSkill);
        addToast({
          type: summary.failures.length > 0 ? "warning" : "success",
          title: `Updated ${summary.succeeded} of ${summary.attempted} deployments`,
          message: summary.failures.map((failure) => failure.message).join("; ") || skill.name,
        });
      }
      setIsPulling(false);
    } catch (err) {
      addToast({
        type: "error",
        title: "Update failed",
        message: err instanceof Error ? err.message : "Unknown error",
      });
      setIsPulling(false);
    }
  };

  return (
    <button className={ROW_ACTION_CLASS} onClick={handlePull} disabled={isPulling}>
      {isPulling ? (
        <span className="inline-block size-3 animate-spin rounded-full border-2 border-current border-t-transparent" />
      ) : (
        "Pull latest"
      )}
    </button>
  );
}

/** "Park" for one unused, model-invocable row - the one-click fix that stops it costing prompt tokens. */
function ParkButton({ skill }: { skill: InstalledSkill }) {
  const [isParking, setIsParking] = useState(false);
  const addToast = useAppStore((state) => state.addToast);

  const handlePark = async () => {
    setIsParking(true);
    try {
      await parkSkill(lifecycleTargetForPark(skill));
      addToast({ type: "success", title: `Parked ${skill.name}` });
      setIsParking(false);
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't park skill",
        message: err instanceof Error ? err.message : "Unknown error",
      });
      setIsParking(false);
    }
  };

  return (
    <button className={ROW_ACTION_CLASS} onClick={handlePark} disabled={isParking}>
      {isParking ? (
        <span className="inline-block size-3 animate-spin rounded-full border-2 border-current border-t-transparent" />
      ) : (
        "Park"
      )}
    </button>
  );
}

/** The row-level action label for one health issue kind - see NeedsAttentionCard's former mapping. */
function issueActionLabel(kind: HealthIssueKind): string {
  switch (kind) {
    case "broken-symlink":
      return "Fix link";
    case "duplicate":
      return "Compare";
    case "linked-root":
      return "Convert to per-skill links";
    case "parked-but-reinstalled":
    case "spec-violation":
    case "lock-only":
      return "Open";
  }
}

/** The Warnings group's per-row action - "Compare" for a duplicate, the Convert dialog opener for a linked root, else the generic Open. */
function WarningRowAction({
  issue,
  onCompare,
  onConvertLinkedRoot,
  onOpen,
}: {
  issue: HealthIssue;
  onCompare: () => void;
  onConvertLinkedRoot: (harness: string, harnessLabel: string, root: string) => void;
  onOpen: () => void;
}) {
  if (issue.kind === "duplicate") {
    return (
      <button className={ROW_ACTION_CLASS} onClick={onCompare}>
        Compare
      </button>
    );
  }
  if (issue.kind === "linked-root" && issue.harness && issue.root) {
    const { harness, root } = issue;
    const harnessLabel = issue.harnessLabel ?? harness;
    return (
      <button
        className={ROW_ACTION_CLASS}
        onClick={() => onConvertLinkedRoot(harness, harnessLabel, root)}
      >
        {issueActionLabel(issue.kind)}
      </button>
    );
  }
  return (
    <button className={ROW_ACTION_CLASS} onClick={onOpen}>
      {issueActionLabel(issue.kind)}
    </button>
  );
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
        <button
          className={`flex flex-1 flex-col gap-1 rounded-md border border-border-subtle bg-bg-elevated px-4 py-3.5 text-left transition-[border-color,background-color,transform] duration-150 hover:border-border hover:bg-bg-hover active:scale-98 aria-pressed:border-accent aria-pressed:bg-accent-softer aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent)] ${
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
        </button>
        <span className="absolute top-3.5 right-3.5 opacity-0 group-hover/stat:opacity-100 group-focus-within/stat:opacity-100 has-[[aria-expanded=true]]:opacity-100">
          <InfoPopover label="About broken" title="Broken and warnings" onLearnMore={onLearnMore}>
            An agent loads nothing, or something you did not intend: a dead link, a SKILL.md the
            loader rejects, a parked skill that was reinstalled.
          </InfoPopover>
        </span>
      </div>

      <div className="group/stat relative flex">
        <button
          className={`flex flex-1 flex-col gap-1 rounded-md border border-border-subtle bg-bg-elevated px-4 py-3.5 text-left transition-[border-color,background-color,transform] duration-150 hover:border-border hover:bg-bg-hover active:scale-98 aria-pressed:border-accent aria-pressed:bg-accent-softer aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent)] ${
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
        </button>
        <span className="absolute top-3.5 right-3.5 opacity-0 group-hover/stat:opacity-100 group-focus-within/stat:opacity-100 has-[[aria-expanded=true]]:opacity-100">
          <InfoPopover label="About warnings" title="Broken and warnings" onLearnMore={onLearnMore}>
            Everything still loads, but the state drifted: copies that differ between harnesses,
            lock-file entries with no folder on disk.
          </InfoPopover>
        </span>
      </div>

      <div className="flex">
        <button
          className="flex flex-1 flex-col gap-1 rounded-md border border-border-subtle bg-bg-elevated px-4 py-3.5 text-left transition-[border-color,background-color,transform] duration-150 hover:border-border hover:bg-bg-hover active:scale-98 aria-pressed:border-accent aria-pressed:bg-accent-softer aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent)]"
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
        </button>
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
              <button
                className="inline-flex items-center gap-1 overflow-hidden rounded-xs bg-accent-soft px-2.5 text-small whitespace-nowrap text-text-primary transition-[filter] hover:brightness-115 aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent)]"
                style={{ flex: `${inv.both} 0 auto` }}
                onClick={() => goToInvocation("both")}
              >
                <span className="tabular-nums">{inv.both}</span> you or the model
              </button>
            </TooltipControl>
          )}
          {inv.modelOnly > 0 && (
            <TooltipControl content="Open in Skills">
              <button
                className="inline-flex items-center gap-1 overflow-hidden rounded-xs bg-accent-softer px-2.5 text-small whitespace-nowrap text-text-secondary transition-[filter] hover:brightness-115 aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent)] aria-pressed:text-text-primary"
                style={{ flex: `${inv.modelOnly} 0 auto` }}
                onClick={() => goToInvocation("model-only")}
              >
                <span className="tabular-nums">{inv.modelOnly}</span> model only
              </button>
            </TooltipControl>
          )}
          {inv.userOnly > 0 && (
            <TooltipControl content="Open in Skills">
              <button
                className="inline-flex items-center gap-1 overflow-hidden rounded-xs bg-bg-tertiary px-2.5 text-small whitespace-nowrap text-text-secondary transition-[filter] hover:brightness-115 aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent)] aria-pressed:text-text-primary"
                style={{ flex: `${inv.userOnly} 0 auto` }}
                onClick={() => goToInvocation("user-only")}
              >
                <span className="tabular-nums">{inv.userOnly}</span> you only
              </button>
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
                <button
                  className="inline-flex items-center gap-1 overflow-hidden rounded-xs bg-accent-soft px-2.5 text-small whitespace-nowrap text-text-primary transition-[filter] hover:brightness-115"
                  style={{ flex: `${cost.usedTokens} 0 auto` }}
                  onClick={() => goToSkills({ usage: "used-30d" })}
                >
                  <span className="tabular-nums">{formatTokens(cost.usedTokens)}</span> ·{" "}
                  <span className="tabular-nums">{cost.usedCount}</span> skills used in 30 days
                </button>
              </TooltipControl>
              <TooltipControl content="Show the skills not used in 30 days">
                <button
                  className="inline-flex items-center gap-1 overflow-hidden rounded-xs bg-bg-tertiary px-2.5 text-small whitespace-nowrap text-text-secondary transition-[filter] hover:brightness-115 aria-pressed:text-text-primary aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent)]"
                  style={{ flex: `${cost.idleTokens} 0 auto` }}
                  aria-pressed={filter === "unused"}
                  onClick={() => toggleFilter("unused")}
                >
                  <span className="tabular-nums">{formatTokens(cost.idleTokens)}</span> ·{" "}
                  <span className="tabular-nums">{cost.idleCount}</span> skills not used in 30 days
                </button>
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
export function HomeView({ snapshot, isLoading, onSelectSkill }: HomeViewProps) {
  const setActiveView = useAppStore((state) => state.setActiveView);
  const setSkillListFilter = useAppStore((state) => state.setSkillListFilter);
  const openSkill = useAppStore((state) => state.openSkill);

  const [filter, setFilter] = useState<HomeFilter | null>(null);
  const [collapsedGroups, setCollapsedGroups] = useState<Set<GroupId>>(() => new Set(["unused"]));
  const [linkedRootDialog, setLinkedRootDialog] = useState<{
    target: LifecycleTarget;
    harness: string;
    harnessLabel: string;
    root: string;
  } | null>(null);

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

  const own = ownSkillsView(snapshot.skills);
  const issues = collectDashboardIssues(own);
  const { broken, warnings } = attentionGroups(issues);
  const updates = skillsWithUpdates(snapshot);
  const inv = homeInvocationCounts(own);
  const cost = homePromptCost(own, snapshot.invocations);
  const unused = unusedSkills(own, snapshot.invocations);
  const recent = recentlyUsedSkills(snapshot.skills, snapshot.invocations, RECENTLY_USED_COUNT);

  const allClear = broken.length === 0 && warnings.length === 0 && updates.length === 0;

  const toggleFilter = (id: HomeFilter) => setFilter((cur) => (cur === id ? null : id));
  const isGroupVisible = (id: GroupId) => filter === null || filter === id;
  const isGroupExpanded = (id: GroupId) => !collapsedGroups.has(id) || filter === id;
  const toggleGroup = (id: GroupId) =>
    setCollapsedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const goToSkills = (patch: Parameters<typeof setSkillListFilter>[0]) => {
    setSkillListFilter({ ...defaultSkillListFilter(), ...patch });
    setActiveView({ kind: "skills" });
  };
  const goToInvocation = (invocation: InvocationPolicy) => goToSkills({ invocation });

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

      <div className="flex flex-col">
        {filter && (
          <div className="flex h-9 items-center gap-2.5 px-3 text-small text-text-tertiary">
            Showing one group ·{" "}
            <button className={LINK_CLASS} onClick={() => setFilter(null)}>
              Show everything
            </button>
          </div>
        )}

        {allClear && !filter && (
          <p className="flex h-full items-center justify-center text-wrap-pretty text-text-tertiary">
            All clear. Nothing needs attention.
          </p>
        )}

        {broken.length > 0 && isGroupVisible("broken") && (
          <Collapsible
            data-group="broken"
            open={isGroupExpanded("broken")}
            onOpenChange={() => toggleGroup("broken")}
          >
            <GroupHead label="Broken" count={broken.length} />
            <CollapsiblePanel>
              <div className="flex flex-col">
                {broken.slice(0, MAX_ROWS_PER_GROUP).map((issue) => (
                  <InboxRow
                    key={`${issue.kind}-${issue.skill.name}-${issue.detail}`}
                    severity="error"
                    skill={issue.skill}
                    onOpen={() => onSelectSkill(issue.skill.name)}
                    detail={<span title={issue.detail}>{issue.detail}</span>}
                    action={
                      <button
                        className={ROW_ACTION_CLASS}
                        onClick={() => onSelectSkill(issue.skill.name)}
                      >
                        {issueActionLabel(issue.kind)}
                      </button>
                    }
                  />
                ))}
                {broken.length > MAX_ROWS_PER_GROUP && (
                  <ShowAllLink
                    count={broken.length}
                    label="Show all"
                    onClick={() => goToSkills({ issue: "any" })}
                  />
                )}
              </div>
            </CollapsiblePanel>
          </Collapsible>
        )}

        {warnings.length > 0 && isGroupVisible("warn") && (
          <Collapsible
            data-group="warn"
            open={isGroupExpanded("warn")}
            onOpenChange={() => toggleGroup("warn")}
          >
            <GroupHead label="Warnings" count={warnings.length} />
            <CollapsiblePanel>
              <div className="flex flex-col">
                {warnings.slice(0, MAX_ROWS_PER_GROUP).map((issue: HealthIssue) => (
                  <InboxRow
                    key={`${issue.kind}-${issue.skill.name}-${issue.detail}`}
                    severity="warning"
                    skill={issue.skill}
                    onOpen={() => onSelectSkill(issue.skill.name)}
                    detail={<span title={issue.detail}>{issue.detail}</span>}
                    action={
                      <WarningRowAction
                        issue={issue}
                        onCompare={() => openSkill(issue.skill.name, undefined, "compare")}
                        onConvertLinkedRoot={(harness, harnessLabel, root) =>
                          setLinkedRootDialog({
                            target: lifecycleTargetForHarnessRoot(issue.skill, harness, root),
                            harness,
                            harnessLabel,
                            root,
                          })
                        }
                        onOpen={() => onSelectSkill(issue.skill.name)}
                      />
                    }
                  />
                ))}
                {warnings.length > MAX_ROWS_PER_GROUP && (
                  <ShowAllLink
                    count={warnings.length}
                    label="Show all"
                    onClick={() => goToSkills({ issue: "any" })}
                  />
                )}
              </div>
            </CollapsiblePanel>
          </Collapsible>
        )}

        {updates.length > 0 && isGroupVisible("upd") && (
          <UpdatesGroup
            updates={updates}
            isExpanded={isGroupExpanded("upd")}
            onToggle={() => toggleGroup("upd")}
            onSelectSkill={onSelectSkill}
            onShowAll={() => setActiveView({ kind: "skills" })}
          />
        )}

        {unused.length > 0 && isGroupVisible("unused") && (
          <Collapsible
            data-group="unused"
            open={isGroupExpanded("unused")}
            onOpenChange={() => toggleGroup("unused")}
          >
            <GroupHead label="Not used in the last 30 days" count={unused.length} />
            <CollapsiblePanel>
              <div className="flex flex-col">
                {unused.slice(0, MAX_ROWS_PER_GROUP).map((skill) => {
                  const projectDeployment = skill.deployments.find((d) => d.project_path);
                  const scopeLabel = projectDeployment?.project_path
                    ? (projectDeployment.project_path.split("/").filter(Boolean).pop() ?? "Global")
                    : "Global";
                  const modelInvocable = skill.invocation !== "user-only";
                  return (
                    <InboxRow
                      key={skill.name}
                      skill={skill}
                      onOpen={() => onSelectSkill(skill.name)}
                      detail={
                        <span
                          title={`${scopeLabel} · installed ${formatRelativeTime(skill.installed_at)}`}
                        >
                          {scopeLabel} · installed {formatRelativeTime(skill.installed_at)} ·{" "}
                          {modelInvocable ? (
                            "description in every prompt"
                          ) : (
                            <span className="text-text-quaternary">
                              user-only, not in the prompt
                            </span>
                          )}
                        </span>
                      }
                      action={
                        modelInvocable ? (
                          <ParkButton skill={skill} />
                        ) : (
                          <button
                            className={ROW_ACTION_CLASS}
                            onClick={() => onSelectSkill(skill.name)}
                          >
                            Open
                          </button>
                        )
                      }
                    />
                  );
                })}
                {unused.length > MAX_ROWS_PER_GROUP && (
                  <ShowAllLink
                    count={unused.length}
                    label="Show all"
                    onClick={() => goToSkills({ usage: "unused-30d" })}
                  />
                )}
              </div>
            </CollapsiblePanel>
          </Collapsible>
        )}

        {recent.length > 0 && isGroupVisible("rec") && (
          <Collapsible
            data-group="rec"
            open={isGroupExpanded("rec")}
            onOpenChange={() => toggleGroup("rec")}
          >
            <GroupHead label="Recently used" count={recent.length} />
            <CollapsiblePanel>
              <div className="flex flex-col">
                {recent.map(({ skill, lastUsed, projectLabel, usesIn30Days }) => (
                  <InboxRow
                    key={skill.name}
                    skill={skill}
                    onOpen={() => onSelectSkill(skill.name)}
                    detail={
                      <span>
                        {projectLabel ?? "Global"} · {formatRelativeTime(lastUsed)}
                      </span>
                    }
                    action={
                      <span className={`${ROW_ACTION_CLASS} tabular-nums`}>
                        {usesIn30Days} uses
                      </span>
                    }
                  />
                ))}
                <ShowAllLink
                  count={0}
                  label="See all activity"
                  onClick={() => setActiveView({ kind: "activity" })}
                />
              </div>
            </CollapsiblePanel>
          </Collapsible>
        )}
      </div>

      {linkedRootDialog && (
        <MaterializeRootDialog
          target={linkedRootDialog.target}
          harness={linkedRootDialog.harness}
          harnessLabel={linkedRootDialog.harnessLabel}
          root={linkedRootDialog.root}
          onClose={() => setLinkedRootDialog(null)}
        />
      )}
    </PageShell>
  );
}

/** Home's "Updates" group: one row per skill with a newer commit, "Update all" in the header. */
function UpdatesGroup({
  updates,
  isExpanded,
  onToggle,
  onSelectSkill,
  onShowAll,
}: {
  updates: InstalledSkill[];
  isExpanded: boolean;
  onToggle: () => void;
  onSelectSkill: (name: string) => void;
  onShowAll: () => void;
}) {
  const [isUpdatingAll, setIsUpdatingAll] = useState(false);
  const addToast = useAppStore((state) => state.addToast);

  const handleUpdateAll = async () => {
    setIsUpdatingAll(true);
    let failures = 0;
    let attempted = 0;
    let succeeded = 0;
    for (const skill of updates) {
      try {
        if (skill.source_kind === "fork") {
          // react-doctor-disable-next-line react-doctor/async-await-in-loop -- update-all runs sequentially on purpose; concurrent `npx skills update` calls race on ~/.agents/.skill-lock.json
          await pullForkUpstream(lifecycleTargetForPark(skill));
          attempted += 1;
          succeeded += 1;
        } else {
          // react-doctor-disable-next-line react-doctor/async-await-in-loop -- update-all runs sequentially on purpose; concurrent `npx skills update` calls race on ~/.agents/.skill-lock.json
          const summary = await updateSkillOwners(skill, updateSkill);
          attempted += summary.attempted;
          succeeded += summary.succeeded;
          failures += summary.failures.length;
        }
      } catch {
        attempted += skill.source_kind === "fork" ? 1 : skill.update_owner_ids.length;
        failures += 1;
      }
    }
    addToast({
      type: failures > 0 ? "warning" : "success",
      title: `Updated ${succeeded} of ${attempted} deployment${attempted === 1 ? "" : "s"}`,
      message: failures > 0 ? `${failures} failed` : undefined,
    });
    setIsUpdatingAll(false);
  };

  return (
    <Collapsible data-group="upd" open={isExpanded} onOpenChange={onToggle}>
      <GroupHead
        label="Updates"
        count={updates.length}
        extra={
          updates.length > 1 && (
            <button
              className="border-0 bg-none p-0 text-small text-accent not-disabled:hover:underline disabled:text-text-quaternary disabled:cursor-not-allowed"
              onClick={(e) => {
                e.stopPropagation();
                handleUpdateAll();
              }}
              disabled={isUpdatingAll}
            >
              {isUpdatingAll ? "Updating…" : "Update all"}
            </button>
          )
        }
      />
      <CollapsiblePanel>
        <div className="flex flex-col">
          {updates.slice(0, MAX_ROWS_PER_GROUP).map((skill) => (
            <InboxRow
              key={skill.name}
              skill={skill}
              onOpen={() => onSelectSkill(skill.name)}
              detail={
                <>
                  {skill.content_hash && skill.update_commit && (
                    <span className="font-mono text-caption whitespace-nowrap text-text-tertiary">
                      {shortSha(skill.content_hash)} → {shortSha(skill.update_commit)}
                    </span>
                  )}{" "}
                  <span className="font-mono text-caption whitespace-nowrap text-text-tertiary">
                    {formatTokens(skill.description_tokens)} tokens
                  </span>
                </>
              }
              action={<PullLatestButton skill={skill} />}
            />
          ))}
          {updates.length > MAX_ROWS_PER_GROUP && (
            <ShowAllLink count={updates.length} label="Show all" onClick={onShowAll} />
          )}
        </div>
      </CollapsiblePanel>
    </Collapsible>
  );
}
