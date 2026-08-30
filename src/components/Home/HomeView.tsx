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
    <span className="home-row-hicons">
      {badges.map(({ id, muted }) => (
        <span key={id} className="home-row-hicon">
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
    <div className="home-row">
      <span className={`dot ${severity ?? ""}`} />
      <span className="home-row-namecell">
        <button className="home-row-name" onClick={onOpen} title={skill.name}>
          {skill.name}
        </button>
        <HarnessBadges skill={skill} />
      </span>
      <span className="home-row-detail">{detail}</span>
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
    <button className="home-group-head" aria-expanded={isExpanded} onClick={onToggle}>
      <ChevronDown className="home-group-chev" size={14} />
      {label}
      <span className="home-group-count count-tabular">{count}</span>
      <span className="home-group-spacer" />
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
    <div className="home-more">
      <button className="info-popover-learn-more" onClick={onClick}>
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
    <button className="home-row-action" onClick={handlePull} disabled={isPulling}>
      {isPulling ? <span className="spinner" /> : "Pull latest"}
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
    <button className="home-row-action" onClick={handlePark} disabled={isParking}>
      {isParking ? <span className="spinner" /> : "Park"}
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
        <p className="home-empty">
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
      <div className="home-stats">
        <div className="home-stat-wrap">
          <button
            className={`home-stat ${broken.length > 0 ? "bad" : ""}`}
            aria-pressed={filter === "broken"}
            onClick={() => toggleFilter("broken")}
          >
            <span className="home-stat-label">Broken</span>
            <span className="home-stat-value count-tabular">{broken.length}</span>
          </button>
          <InfoPopover
            label="About broken"
            title="Broken and warnings"
            onLearnMore={() => setActiveView({ kind: "learn", section: "broken" })}
          >
            An agent loads nothing, or something you did not intend: a dead link, a SKILL.md the
            loader rejects, a parked skill that was reinstalled.
          </InfoPopover>
        </div>

        <div className="home-stat-wrap">
          <button
            className={`home-stat ${warnings.length > 0 ? "warn" : ""}`}
            aria-pressed={filter === "warn"}
            onClick={() => toggleFilter("warn")}
          >
            <span className="home-stat-label">Warnings</span>
            <span className="home-stat-value count-tabular">{warnings.length}</span>
          </button>
          <InfoPopover
            label="About warnings"
            title="Broken and warnings"
            onLearnMore={() => setActiveView({ kind: "learn", section: "broken" })}
          >
            Everything still loads, but the state drifted: copies that differ between harnesses,
            lock-file entries with no folder on disk.
          </InfoPopover>
        </div>

        <div className="home-stat-wrap">
          <button
            className="home-stat"
            aria-pressed={filter === "upd"}
            onClick={() => toggleFilter("upd")}
          >
            <span className="home-stat-label">Updates</span>
            <span className="home-stat-value count-tabular">{updates.length}</span>
          </button>
        </div>
      </div>

      <section className="home-lane">
        <div className="home-lane-row">
          <span className="home-lane-key">
            Who can invoke
            <b className="count-tabular">{invokeTotal}</b>
            <InfoPopover
              label="About invocation"
              title="Who can invoke a skill"
              onLearnMore={() => setActiveView({ kind: "learn", section: "invoke" })}
            >
              Read from SKILL.md frontmatter. Claude Code honours both limits, pi only the you-only
              one; Codex and OpenCode use their own config.
            </InfoPopover>
          </span>
          <div className="home-lane-bar" role="group" aria-label="Who can invoke">
            {inv.both > 0 && (
              <button
                className="home-lane-seg both"
                style={{ flex: `${inv.both} 0 auto` }}
                title="Open in Skills"
                onClick={() => goToInvocation("both")}
              >
                <span className="count-tabular">{inv.both}</span> you or the model
              </button>
            )}
            {inv.modelOnly > 0 && (
              <button
                className="home-lane-seg model"
                style={{ flex: `${inv.modelOnly} 0 auto` }}
                title="Open in Skills"
                onClick={() => goToInvocation("model-only")}
              >
                <span className="count-tabular">{inv.modelOnly}</span> model only
              </button>
            )}
            {inv.userOnly > 0 && (
              <button
                className="home-lane-seg user"
                style={{ flex: `${inv.userOnly} 0 auto` }}
                title="Open in Skills"
                onClick={() => goToInvocation("user-only")}
              >
                <span className="count-tabular">{inv.userOnly}</span> you only
              </button>
            )}
          </div>
        </div>

        <div className="home-lane-row">
          <span className="home-lane-key">
            Prompt cost
            <b className="count-tabular">{formatTokens(cost.totalTokens)}</b>
            <InfoPopover
              label="About prompt cost"
              title="Prompt cost"
              onLearnMore={() => setActiveView({ kind: "learn", section: "cost" })}
            >
              Tokens of name and description the model reads every turn. Only skills the model may
              invoke count; user-only skills cost nothing until you run them.
            </InfoPopover>
          </span>
          <div className="home-lane-bar" role="group" aria-label="Prompt cost">
            {cost.totalTokens === 0 ? (
              <span className="home-lane-seg idle" style={{ width: "100%" }}>
                No model-invocable skills
              </span>
            ) : (
              <>
                <button
                  className="home-lane-seg used"
                  style={{ width: `${(cost.usedTokens / cost.totalTokens) * 100}%` }}
                  title="Open in Skills"
                  onClick={() => goToSkills({ usage: "used-30d" })}
                >
                  <span className="count-tabular">{formatTokens(cost.usedTokens)}</span> ·{" "}
                  <span className="count-tabular">{cost.usedCount}</span> skills used in 30 days
                </button>
                <button
                  className="home-lane-seg idle"
                  style={{ width: `${(cost.idleTokens / cost.totalTokens) * 100}%` }}
                  aria-pressed={filter === "unused"}
                  title="Show the skills not used in 30 days"
                  onClick={() => toggleFilter("unused")}
                >
                  <span className="count-tabular">{formatTokens(cost.idleTokens)}</span> ·{" "}
                  <span className="count-tabular">{cost.idleCount}</span> skills not used in 30 days
                </button>
              </>
            )}
          </div>
        </div>
      </section>

      <div className="home-inbox">
        {filter && (
          <div className="home-filter-note">
            Showing one group ·{" "}
            <button className="info-popover-learn-more" onClick={() => setFilter(null)}>
              Show everything
            </button>
          </div>
        )}

        {allClear && !filter && <p className="home-empty">All clear. Nothing needs attention.</p>}

        {broken.length > 0 && isGroupVisible("broken") && (
          <section className="home-group" data-group="broken">
            <GroupHead
              label="Broken"
              count={broken.length}
              isExpanded={isGroupExpanded("broken", broken.length)}
              onToggle={() => toggleGroup("broken")}
            />
            {isGroupExpanded("broken", broken.length) && (
              <div className="home-group-rows">
                {broken.slice(0, MAX_ROWS_PER_GROUP).map((issue, i) => (
                  <InboxRow
                    key={`${issue.kind}-${issue.skill.name}-${i}`}
                    severity="error"
                    skill={issue.skill}
                    onOpen={() => onSelectSkill(issue.skill.name)}
                    detail={<span title={issue.detail}>{issue.detail}</span>}
                    action={
                      <button
                        className="home-row-action"
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
          <section className="home-group" data-group="warn">
            <GroupHead
              label="Warnings"
              count={warnings.length}
              isExpanded={isGroupExpanded("warn", warnings.length)}
              onToggle={() => toggleGroup("warn")}
            />
            {isGroupExpanded("warn", warnings.length) && (
              <div className="home-group-rows">
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
                          className="home-row-action"
                          onClick={() => openSkill(issue.skill.name, undefined, "compare")}
                        >
                          Compare
                        </button>
                      ) : (
                        <button
                          className="home-row-action"
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
          <section className="home-group" data-group="unused">
            <GroupHead
              label="Not used in the last 30 days"
              count={unused.length}
              isExpanded={isGroupExpanded("unused", unused.length)}
              onToggle={() => toggleGroup("unused")}
            />
            {isGroupExpanded("unused", unused.length) && (
              <div className="home-group-rows">
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
                            <span className="inv">user-only, not in the prompt</span>
                          )}
                        </span>
                      }
                      action={
                        modelInvocable ? (
                          <ParkButton skill={skill} />
                        ) : (
                          <button
                            className="home-row-action"
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
          <section className="home-group" data-group="rec">
            <GroupHead
              label="Recently used"
              count={recent.length}
              isExpanded={isGroupExpanded("rec", recent.length)}
              onToggle={() => toggleGroup("rec")}
            />
            {isGroupExpanded("rec", recent.length) && (
              <div className="home-group-rows">
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
                      <span className="home-row-action count-tabular">{usesIn30Days} uses</span>
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
    <section className="home-group" data-group="upd">
      <GroupHead
        label="Updates"
        count={updates.length}
        isExpanded={isExpanded}
        onToggle={onToggle}
        extra={
          updates.length > 1 && (
            <button
              className="home-group-update-all"
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
        <div className="home-group-rows">
          {updates.slice(0, MAX_ROWS_PER_GROUP).map((skill) => (
            <InboxRow
              key={skill.name}
              skill={skill}
              onOpen={() => onSelectSkill(skill.name)}
              detail={
                <>
                  {skill.content_hash && skill.update_commit && (
                    <span className="mono">
                      {shortSha(skill.content_hash)} → {shortSha(skill.update_commit)}
                    </span>
                  )}{" "}
                  <span className="mono">{formatTokens(skill.description_tokens)} tokens</span>
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
