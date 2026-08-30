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
import {
  attentionGroups,
  homeInvocationCounts,
  homePromptCost,
  recentlyUsedSkills,
  unusedSkills,
} from "../../lib/home-summary";
import { parkSkill, pullForkUpstream, updateSkill } from "../../lib/skill-api";
import { collectDashboardIssues } from "../../lib/skill-health";
import type { HealthIssue, HealthIssueKind } from "../../lib/skill-health";
import { defaultSkillListFilter } from "../../lib/skill-list-filter";
import { ownSkillsView } from "../../lib/skill-plugin-partition";
import { formatRelativeTime, formatTokens, shortSha } from "../../lib/skill-stats";
import { skillsWithUpdates } from "../../lib/skill-updates";
import type {
  AgentId,
  InstalledSkill,
  InvocationPolicy,
  SkillSnapshot,
} from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";
import { PageShell } from "../Shell/PageShell";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";
import { InfoPopover } from "../ui/InfoPopover";

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

/** A group's sticky header: chevron, label, count, spacer, optional extra action. */
function GroupHead({
  label,
  count,
  isExpanded,
  onToggle,
  extra,
}: {
  label: string;
  count: number;
  isExpanded: boolean;
  onToggle: () => void;
  extra?: ReactNode;
}) {
  return (
    <button
      className="sticky top-0 z-1 flex h-8.5 w-full items-center gap-2 border-t border-b border-border-subtle bg-bg-secondary px-3 text-left text-small font-semibold text-text-primary"
      aria-expanded={isExpanded}
      onClick={onToggle}
    >
      <ChevronDown
        className={`size-3.5 text-text-quaternary transition-transform ${isExpanded ? "" : "-rotate-90"}`}
      />
      {label}
      <span className="font-normal text-text-tertiary tabular-nums">{count}</span>
      <span className="flex-1" />
      {extra}
    </button>
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
        const result = await pullForkUpstream(skill.name);
        addToast({ type: "success", title: result.message ?? `Merged ${skill.name}` });
      } else {
        const result = await updateSkill(skill.name, true);
        if (result.success) {
          addToast({ type: "success", title: "Skill updated", message: skill.name });
        } else {
          addToast({ type: "error", title: "Update failed", message: result.error });
        }
      }
    } catch (err) {
      addToast({
        type: "error",
        title: "Update failed",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
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
      await parkSkill(skill.name);
      addToast({ type: "success", title: `Parked ${skill.name}` });
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't park skill",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
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
    case "parked-but-reinstalled":
    case "spec-violation":
    case "lock-only":
      return "Open";
  }
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

  if (!snapshot) {
    return (
      <PageShell title="Home">
        <p className="flex h-full items-center justify-center text-wrap-pretty text-text-tertiary">
          {isLoading ? "Scanning installed skills…" : "No skill snapshot yet."}
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

  const invokeTotal = inv.both + inv.modelOnly + inv.userOnly;
  const allClear = broken.length === 0 && warnings.length === 0 && updates.length === 0;

  const toggleFilter = (id: HomeFilter) => setFilter((cur) => (cur === id ? null : id));
  const isGroupVisible = (id: GroupId) => filter === null || filter === id;
  const isGroupExpanded = (id: GroupId, count: number) =>
    (id === "broken" ? count > 0 : !collapsedGroups.has(id)) || filter === id;
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
          <InfoPopover
            label="About broken"
            title="Broken and warnings"
            onLearnMore={() => setActiveView({ kind: "learn", section: "broken" })}
            className="absolute top-3.5 right-3.5 opacity-0 group-hover/stat:opacity-100 group-focus-within/stat:opacity-100 has-[[aria-expanded=true]]:opacity-100"
          >
            An agent loads nothing, or something you did not intend: a dead link, a SKILL.md the
            loader rejects, a parked skill that was reinstalled.
          </InfoPopover>
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
          <InfoPopover
            label="About warnings"
            title="Broken and warnings"
            onLearnMore={() => setActiveView({ kind: "learn", section: "broken" })}
            className="absolute top-3.5 right-3.5 opacity-0 group-hover/stat:opacity-100 group-focus-within/stat:opacity-100 has-[[aria-expanded=true]]:opacity-100"
          >
            Everything still loads, but the state drifted: copies that differ between harnesses,
            lock-file entries with no folder on disk.
          </InfoPopover>
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

      <section className="flex flex-col gap-2 rounded-md border border-border-subtle bg-bg-elevated px-4 py-3.5">
        <div className="grid grid-cols-[210px_minmax(0,1fr)] items-baseline gap-3">
          <span className="flex items-baseline gap-x-1 whitespace-nowrap text-small text-text-secondary">
            Who can invoke
            <b className="ml-1 font-semibold text-text-primary tabular-nums">{invokeTotal}</b>
            <InfoPopover
              label="About invocation"
              title="Who can invoke a skill"
              onLearnMore={() => setActiveView({ kind: "learn", section: "invoke" })}
            >
              Read from SKILL.md frontmatter. Claude Code honours both limits, pi only the you-only
              one; Codex and OpenCode use their own config.
            </InfoPopover>
          </span>
          <div className="flex h-7 gap-0.5" role="group" aria-label="Who can invoke">
            {inv.both > 0 && (
              <button
                className="inline-flex items-center gap-1 overflow-hidden rounded-xs bg-accent-soft px-2.5 text-small whitespace-nowrap text-text-primary transition-[filter] hover:brightness-115 aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent)]"
                style={{ flex: `${inv.both} 0 auto` }}
                title="Open in Skills"
                onClick={() => goToInvocation("both")}
              >
                <span className="tabular-nums">{inv.both}</span> you or the model
              </button>
            )}
            {inv.modelOnly > 0 && (
              <button
                className="inline-flex items-center gap-1 overflow-hidden rounded-xs bg-accent-softer px-2.5 text-small whitespace-nowrap text-text-secondary transition-[filter] hover:brightness-115 aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent)] aria-pressed:text-text-primary"
                style={{ flex: `${inv.modelOnly} 0 auto` }}
                title="Open in Skills"
                onClick={() => goToInvocation("model-only")}
              >
                <span className="tabular-nums">{inv.modelOnly}</span> model only
              </button>
            )}
            {inv.userOnly > 0 && (
              <button
                className="inline-flex items-center gap-1 overflow-hidden rounded-xs bg-bg-tertiary px-2.5 text-small whitespace-nowrap text-text-secondary transition-[filter] hover:brightness-115 aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent)] aria-pressed:text-text-primary"
                style={{ flex: `${inv.userOnly} 0 auto` }}
                title="Open in Skills"
                onClick={() => goToInvocation("user-only")}
              >
                <span className="tabular-nums">{inv.userOnly}</span> you only
              </button>
            )}
          </div>
        </div>

        <div className="grid grid-cols-[210px_minmax(0,1fr)] items-baseline gap-3">
          <span className="flex items-baseline gap-x-1 whitespace-nowrap text-small text-text-secondary">
            Prompt cost
            <b className="ml-1 font-semibold text-text-primary tabular-nums">
              {formatTokens(cost.totalTokens)}
            </b>
            <InfoPopover
              label="About prompt cost"
              title="Prompt cost"
              onLearnMore={() => setActiveView({ kind: "learn", section: "cost" })}
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
                <button
                  className="inline-flex items-center gap-1 overflow-hidden rounded-xs bg-accent-soft px-2.5 text-small whitespace-nowrap text-text-primary transition-[filter] hover:brightness-115"
                  style={{ width: `${(cost.usedTokens / cost.totalTokens) * 100}%` }}
                  title="Open in Skills"
                  onClick={() => goToSkills({ usage: "used-30d" })}
                >
                  <span className="tabular-nums">{formatTokens(cost.usedTokens)}</span> ·{" "}
                  <span className="tabular-nums">{cost.usedCount}</span> skills used in 30 days
                </button>
                <button
                  className="inline-flex items-center gap-1 overflow-hidden rounded-xs bg-bg-tertiary px-2.5 text-small whitespace-nowrap text-text-secondary transition-[filter] hover:brightness-115 aria-pressed:text-text-primary aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent)]"
                  style={{ width: `${(cost.idleTokens / cost.totalTokens) * 100}%` }}
                  aria-pressed={filter === "unused"}
                  title="Show the skills not used in 30 days"
                  onClick={() => toggleFilter("unused")}
                >
                  <span className="tabular-nums">{formatTokens(cost.idleTokens)}</span> ·{" "}
                  <span className="tabular-nums">{cost.idleCount}</span> skills not used in 30 days
                </button>
              </>
            )}
          </div>
        </div>
      </section>

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
          <section data-group="broken">
            <GroupHead
              label="Broken"
              count={broken.length}
              isExpanded={isGroupExpanded("broken", broken.length)}
              onToggle={() => toggleGroup("broken")}
            />
            {isGroupExpanded("broken", broken.length) && (
              <div className="flex flex-col">
                {broken.slice(0, MAX_ROWS_PER_GROUP).map((issue, i) => (
                  <InboxRow
                    key={`${issue.kind}-${issue.skill.name}-${i}`}
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
            )}
          </section>
        )}

        {warnings.length > 0 && isGroupVisible("warn") && (
          <section data-group="warn">
            <GroupHead
              label="Warnings"
              count={warnings.length}
              isExpanded={isGroupExpanded("warn", warnings.length)}
              onToggle={() => toggleGroup("warn")}
            />
            {isGroupExpanded("warn", warnings.length) && (
              <div className="flex flex-col">
                {warnings.slice(0, MAX_ROWS_PER_GROUP).map((issue: HealthIssue, i) => (
                  <InboxRow
                    key={`${issue.kind}-${issue.skill.name}-${i}`}
                    severity="warning"
                    skill={issue.skill}
                    onOpen={() => onSelectSkill(issue.skill.name)}
                    detail={<span title={issue.detail}>{issue.detail}</span>}
                    action={
                      issue.kind === "duplicate" ? (
                        <button
                          className={ROW_ACTION_CLASS}
                          onClick={() => openSkill(issue.skill.name, undefined, "compare")}
                        >
                          Compare
                        </button>
                      ) : (
                        <button
                          className={ROW_ACTION_CLASS}
                          onClick={() => onSelectSkill(issue.skill.name)}
                        >
                          {issueActionLabel(issue.kind)}
                        </button>
                      )
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
            )}
          </section>
        )}

        {updates.length > 0 && isGroupVisible("upd") && (
          <UpdatesGroup
            updates={updates}
            isExpanded={isGroupExpanded("upd", updates.length)}
            onToggle={() => toggleGroup("upd")}
            onSelectSkill={onSelectSkill}
            onShowAll={() => setActiveView({ kind: "skills" })}
          />
        )}

        {unused.length > 0 && isGroupVisible("unused") && (
          <section data-group="unused">
            <GroupHead
              label="Not used in the last 30 days"
              count={unused.length}
              isExpanded={isGroupExpanded("unused", unused.length)}
              onToggle={() => toggleGroup("unused")}
            />
            {isGroupExpanded("unused", unused.length) && (
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
            )}
          </section>
        )}

        {recent.length > 0 && isGroupVisible("rec") && (
          <section data-group="rec">
            <GroupHead
              label="Recently used"
              count={recent.length}
              isExpanded={isGroupExpanded("rec", recent.length)}
              onToggle={() => toggleGroup("rec")}
            />
            {isGroupExpanded("rec", recent.length) && (
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
            )}
          </section>
        )}
      </div>
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
    try {
      for (const skill of updates) {
        try {
          if (skill.source_kind === "fork") {
            await pullForkUpstream(skill.name);
          } else {
            const result = await updateSkill(skill.name, true);
            if (!result.success) failures += 1;
          }
        } catch {
          failures += 1;
        }
      }
      const succeeded = updates.length - failures;
      addToast({
        type: failures > 0 ? "warning" : "success",
        title: `Updated ${succeeded} of ${updates.length} skill${updates.length === 1 ? "" : "s"}`,
        message: failures > 0 ? `${failures} failed` : undefined,
      });
    } finally {
      setIsUpdatingAll(false);
    }
  };

  return (
    <section data-group="upd">
      <GroupHead
        label="Updates"
        count={updates.length}
        isExpanded={isExpanded}
        onToggle={onToggle}
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
      {isExpanded && (
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
      )}
    </section>
  );
}
