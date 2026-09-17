// ============================================================================
// HomeInboxGroups - Home's five inbox groups (Broken, Warnings, Updates, Not
// used in 30 days, Recently used): the shared row layout, each group's
// header and rows, and the per-row actions (Fix, Compare, Park, Pull latest).
// ============================================================================

import { useState } from "react";
import type { CSSProperties, KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react";
import { Button, Collapsible, CollapsiblePanel } from "@skill-studio/ui";
import { formatRelativeTime, formatTokens, shortSha } from "@skill-studio/lib";
import type { HealthIssue, InstalledSkill, RecentlyUsedSkill } from "@skill-studio/lib";
import { parkSkill, pullForkUpstream, updateSkill } from "../../lib/skill-api";
import { lifecycleTargetForPark, updateSkillOwners } from "../../lib/skill-lifecycle-target";
import { useAppStore } from "../../store/appStore";
import { GroupHead } from "../SkillList/GroupHead";
import { DEFAULT_HARNESS_LIST, rowState, whereFacts } from "../SkillList/skill-row-state";
import { HarnessStack } from "../SkillList/HarnessStack";
import { ROW_CLASS, RowGlyph, SkillNameCell } from "../SkillList/SkillRowCells";
import { SkillLocationCell } from "../SkillList/SkillLocationCell";
import { RichTooltipScope } from "../ui/RichTooltip";
import { issueActionLabel, issueKey, MAX_ROWS_PER_GROUP, rowAt, skillKey } from "./home-inbox-data";
import type { GroupId, HomeFilter, HomeGroups, HomeRowPlan } from "./home-inbox-data";

/** Home's row's glyph hit box - the same size Skills uses, so the two lists line up. */
const HOME_GLYPH_HIT = 28;
const HOME_GLYPH_SIZE = 14;

/** Text link style shared by every "Show all"/"Show everything"/"Learn more" affordance on Home. */
export const LINK_CLASS = "h-auto gap-1 p-0 text-small";

/** One inbox row's trailing action - a text button or, on the "Recently used" rows, a plain count. */
const ROW_ACTION_CLASS =
  "h-9 max-w-full justify-end truncate p-0 text-right text-small text-text-tertiary hover:bg-transparent hover:text-accent";

/** The roving-cursor wiring every group's rows share, passed down from `HomeView`'s single
 * `useHomeRowCursor` call. */
interface RowCursorProps {
  rowRef: (key: string) => (el: HTMLDivElement | null) => void;
  tabIndexFor: (key: string) => 0 | -1;
}

/**
 * One row of any inbox group - the Stack row Skills uses, with the tokens
 * column replaced by the group's own detail text and action node. The state
 * glyph comes from `rowState(skill)`, not the group's own severity, so a
 * skill with no state (e.g. a plain "recently used" row) shows no glyph.
 */
function HomeRow({
  skill,
  detail,
  action,
  onOpen,
  rowIndex,
  rowRef,
  tabIndex,
}: {
  skill: InstalledSkill;
  detail: ReactNode;
  action: ReactNode;
  onOpen: () => void;
  rowIndex: number;
  /** Registers the row with `useRowCursor` so movement can focus and scroll it. */
  rowRef: (el: HTMLDivElement | null) => void;
  tabIndex: 0 | -1;
}) {
  return (
    <div
      ref={rowRef}
      role="row"
      aria-rowindex={rowIndex}
      tabIndex={tabIndex}
      onClick={onOpen}
      className={`${ROW_CLASS} grid-cols-[var(--glyph-hit)_minmax(0,1fr)_160px_148px_minmax(0,1fr)_88px] gap-x-3 px-3 hover:bg-bg-secondary focus-visible:outline-2 focus-visible:outline-accent -outline-offset-2`}
      style={
        // SAFETY: `--glyph-hit` is a custom property, not a known CSSProperties key; React
        // passes it through to the style attribute as-is.
        { "--glyph-hit": `${HOME_GLYPH_HIT}px` } as CSSProperties
      }
    >
      {/* Not `contents`: `RowGlyph` renders nothing when the skill has no state, and a `contents`
          wrapper around no children drops out of the grid, shifting every column after it. */}
      <div role="gridcell" className="flex items-center justify-center">
        <RowGlyph state={rowState(skill)} size={HOME_GLYPH_SIZE} />
      </div>
      <div role="gridcell" className="contents">
        <SkillNameCell skill={skill} />
      </div>
      <div role="gridcell" className="contents">
        <SkillLocationCell locations={whereFacts(skill, DEFAULT_HARNESS_LIST).locations} />
      </div>
      <div role="gridcell" className="contents">
        <HarnessStack skill={skill} harnessList={DEFAULT_HARNESS_LIST} />
      </div>
      <div role="gridcell" className="contents">
        <span className="select-text truncate text-small text-text-tertiary">{detail}</span>
      </div>
      {/* Not `contents`: a fixed-width, right-aligned box so the action column's size never
          depends on its own content - that's what let the row's `1fr` columns drift per group. */}
      <div role="gridcell" className="flex min-w-0 items-center justify-end overflow-hidden">
        {action}
      </div>
    </div>
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
      <Button variant="link" className={LINK_CLASS} onClick={onClick}>
        {label} {count}
      </Button>
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
        // Hoisted out of the `??`: the React Compiler can't compile a value block
        // (conditional/logical/optional-chaining) directly inside a try/catch statement.
        let title = result.message;
        if (!title) title = `Merged ${skill.name}`;
        addToast({ type: "success", title });
      } else {
        const summary = await updateSkillOwners(skill, updateSkill);
        let type: "success" | "warning" = "success";
        if (summary.failures.length > 0) type = "warning";
        let message = summary.failures.map((failure) => failure.message).join("; ");
        if (!message) message = skill.name;
        addToast({
          type,
          title: `Updated ${summary.succeeded} of ${summary.attempted} deployments`,
          message,
        });
      }
      setIsPulling(false);
    } catch (err) {
      let message = "Unknown error";
      if (err instanceof Error) message = err.message;
      addToast({ type: "error", title: "Update failed", message });
      setIsPulling(false);
    }
  };

  return (
    <Button variant="ghost" className={ROW_ACTION_CLASS} onClick={handlePull} disabled={isPulling}>
      {isPulling ? (
        <span className="inline-block size-3 animate-spin rounded-full border-2 border-current border-t-transparent" />
      ) : (
        "Pull latest"
      )}
    </Button>
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
    <Button variant="ghost" className={ROW_ACTION_CLASS} onClick={handlePark} disabled={isParking}>
      {isParking ? (
        <span className="inline-block size-3 animate-spin rounded-full border-2 border-current border-t-transparent" />
      ) : (
        "Park"
      )}
    </Button>
  );
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
      <Button variant="ghost" className={ROW_ACTION_CLASS} onClick={onCompare}>
        Compare
      </Button>
    );
  }
  if (issue.kind === "linked-root" && issue.harness && issue.root) {
    const { harness, root } = issue;
    const harnessLabel = issue.harnessLabel ?? harness;
    return (
      <Button
        variant="ghost"
        className={ROW_ACTION_CLASS}
        onClick={() => onConvertLinkedRoot(harness, harnessLabel, root)}
      >
        {issueActionLabel(issue.kind)}
      </Button>
    );
  }
  return (
    <Button variant="ghost" className={ROW_ACTION_CLASS} onClick={onOpen}>
      {issueActionLabel(issue.kind)}
    </Button>
  );
}

/** Home's "Broken" group: one row per broken-symlink/parked-but-reinstalled/spec-violation issue. */
export function BrokenGroup({
  broken,
  start,
  isExpanded,
  onToggle,
  onSelectSkill,
  onShowAll,
  rowRef,
  tabIndexFor,
}: RowCursorProps & {
  broken: HealthIssue[];
  start: number;
  isExpanded: boolean;
  onToggle: () => void;
  onSelectSkill: (name: string) => void;
  onShowAll: () => void;
}) {
  return (
    <Collapsible data-group="broken" role="rowgroup" open={isExpanded} onOpenChange={onToggle}>
      <div role="row">
        <div role="gridcell">
          <GroupHead label="Broken" count={broken.length} groupId="broken" />
        </div>
      </div>
      <CollapsiblePanel>
        <div className="flex flex-col">
          {broken.slice(0, MAX_ROWS_PER_GROUP).map((issue, i) => {
            const key = issueKey("broken", issue);
            return (
              <HomeRow
                key={key}
                skill={issue.skill}
                rowIndex={rowAt(start, i)}
                rowRef={rowRef(key)}
                tabIndex={tabIndexFor(key)}
                onOpen={() => onSelectSkill(issue.skill.name)}
                detail={<span>{issue.detail}</span>}
                action={
                  <Button
                    variant="ghost"
                    className={ROW_ACTION_CLASS}
                    onClick={() => onSelectSkill(issue.skill.name)}
                  >
                    {issueActionLabel(issue.kind)}
                  </Button>
                }
              />
            );
          })}
          {broken.length > MAX_ROWS_PER_GROUP && (
            <ShowAllLink count={broken.length} label="Show all" onClick={onShowAll} />
          )}
        </div>
      </CollapsiblePanel>
    </Collapsible>
  );
}

/** Home's "Warnings" group: one row per duplicate/linked-root/lock-only issue. */
export function WarningsGroup({
  warnings,
  start,
  isExpanded,
  onToggle,
  onSelectSkill,
  onShowAll,
  openSkill,
  onConvertLinkedRoot,
  rowRef,
  tabIndexFor,
}: RowCursorProps & {
  warnings: HealthIssue[];
  start: number;
  isExpanded: boolean;
  onToggle: () => void;
  onSelectSkill: (name: string) => void;
  onShowAll: () => void;
  openSkill: (name: string, deploymentPath: string | undefined, intent: "compare") => void;
  onConvertLinkedRoot: (
    skill: InstalledSkill,
    harness: string,
    harnessLabel: string,
    root: string,
  ) => void;
}) {
  return (
    <Collapsible data-group="warn" role="rowgroup" open={isExpanded} onOpenChange={onToggle}>
      <div role="row">
        <div role="gridcell">
          <GroupHead label="Warnings" count={warnings.length} groupId="warn" />
        </div>
      </div>
      <CollapsiblePanel>
        <div className="flex flex-col">
          {warnings.slice(0, MAX_ROWS_PER_GROUP).map((issue, i) => {
            const key = issueKey("warn", issue);
            return (
              <HomeRow
                key={key}
                skill={issue.skill}
                rowIndex={rowAt(start, i)}
                rowRef={rowRef(key)}
                tabIndex={tabIndexFor(key)}
                onOpen={() => onSelectSkill(issue.skill.name)}
                detail={<span>{issue.detail}</span>}
                action={
                  <WarningRowAction
                    issue={issue}
                    onCompare={() => openSkill(issue.skill.name, undefined, "compare")}
                    onConvertLinkedRoot={(harness, harnessLabel, root) =>
                      onConvertLinkedRoot(issue.skill, harness, harnessLabel, root)
                    }
                    onOpen={() => onSelectSkill(issue.skill.name)}
                  />
                }
              />
            );
          })}
          {warnings.length > MAX_ROWS_PER_GROUP && (
            <ShowAllLink count={warnings.length} label="Show all" onClick={onShowAll} />
          )}
        </div>
      </CollapsiblePanel>
    </Collapsible>
  );
}

/** Home's "Not used in the last 30 days" group: one row per idle skill, with a one-click Park. */
export function UnusedGroup({
  unused,
  start,
  isExpanded,
  onToggle,
  onSelectSkill,
  onShowAll,
  rowRef,
  tabIndexFor,
}: RowCursorProps & {
  unused: InstalledSkill[];
  start: number;
  isExpanded: boolean;
  onToggle: () => void;
  onSelectSkill: (name: string) => void;
  onShowAll: () => void;
}) {
  return (
    <Collapsible data-group="unused" role="rowgroup" open={isExpanded} onOpenChange={onToggle}>
      <div role="row">
        <div role="gridcell">
          <GroupHead label="Not used in the last 30 days" count={unused.length} groupId="unused" />
        </div>
      </div>
      <CollapsiblePanel>
        <div className="flex flex-col">
          {unused.slice(0, MAX_ROWS_PER_GROUP).map((skill, i) => {
            const projectDeployment = skill.deployments.find((d) => d.project_path);
            const scopeLabel = projectDeployment?.project_path
              ? (projectDeployment.project_path.split("/").filter(Boolean).pop() ?? "Global")
              : "Global";
            const modelInvocable = skill.invocation !== "user-only";
            const key = skillKey("unused", skill);
            return (
              <HomeRow
                key={key}
                skill={skill}
                rowIndex={rowAt(start, i)}
                rowRef={rowRef(key)}
                tabIndex={tabIndexFor(key)}
                onOpen={() => onSelectSkill(skill.name)}
                detail={
                  <span>
                    {scopeLabel} · installed {formatRelativeTime(skill.installed_at)} ·{" "}
                    {modelInvocable ? (
                      "description in every prompt"
                    ) : (
                      <span className="text-text-quaternary">user-only, not in the prompt</span>
                    )}
                  </span>
                }
                action={
                  modelInvocable ? (
                    <ParkButton skill={skill} />
                  ) : (
                    <Button
                      variant="ghost"
                      className={ROW_ACTION_CLASS}
                      onClick={() => onSelectSkill(skill.name)}
                    >
                      Open
                    </Button>
                  )
                }
              />
            );
          })}
          {unused.length > MAX_ROWS_PER_GROUP && (
            <ShowAllLink count={unused.length} label="Show all" onClick={onShowAll} />
          )}
        </div>
      </CollapsiblePanel>
    </Collapsible>
  );
}

/** Home's "Recently used" group: one row per skill used in the last 30 days, newest first. */
export function RecentGroup({
  recent,
  start,
  isExpanded,
  onToggle,
  onSelectSkill,
  onShowAll,
  rowRef,
  tabIndexFor,
}: RowCursorProps & {
  recent: RecentlyUsedSkill[];
  start: number;
  isExpanded: boolean;
  onToggle: () => void;
  onSelectSkill: (name: string) => void;
  onShowAll: () => void;
}) {
  return (
    <Collapsible data-group="rec" role="rowgroup" open={isExpanded} onOpenChange={onToggle}>
      <div role="row">
        <div role="gridcell">
          <GroupHead label="Recently used" count={recent.length} groupId="rec" />
        </div>
      </div>
      <CollapsiblePanel>
        <div className="flex flex-col">
          {recent.map(({ skill, lastUsed, projectLabel, usesIn30Days }, i) => {
            const key = skillKey("rec", skill);
            return (
              <HomeRow
                key={key}
                skill={skill}
                rowIndex={rowAt(start, i)}
                rowRef={rowRef(key)}
                tabIndex={tabIndexFor(key)}
                onOpen={() => onSelectSkill(skill.name)}
                detail={
                  <span>
                    {projectLabel ?? "Global"} · {formatRelativeTime(lastUsed)}
                  </span>
                }
                action={
                  <span className={`${ROW_ACTION_CLASS} tabular-nums`}>{usesIn30Days} uses</span>
                }
              />
            );
          })}
          <ShowAllLink count={0} label="See all activity" onClick={onShowAll} />
        </div>
      </CollapsiblePanel>
    </Collapsible>
  );
}

/** Home's "Updates" group: one row per skill with a newer commit, "Update all" in the header. */
export function UpdatesGroup({
  updates,
  isExpanded,
  onToggle,
  onSelectSkill,
  onShowAll,
  start,
  rowRef,
  tabIndexFor,
}: RowCursorProps & {
  updates: InstalledSkill[];
  isExpanded: boolean;
  onToggle: () => void;
  onSelectSkill: (name: string) => void;
  onShowAll: () => void;
  /** This group's offset into the page's continuous `aria-rowindex` sequence. */
  start: number;
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
    <Collapsible data-group="upd" role="rowgroup" open={isExpanded} onOpenChange={onToggle}>
      <div role="row">
        <div role="gridcell">
          <GroupHead
            label="Updates"
            count={updates.length}
            groupId="upd"
            extra={
              updates.length > 1 && (
                <Button
                  variant="link"
                  className="h-auto p-0 text-small disabled:text-text-quaternary"
                  onClick={(e) => {
                    e.stopPropagation();
                    handleUpdateAll();
                  }}
                  disabled={isUpdatingAll}
                >
                  {isUpdatingAll ? "Updating…" : "Update all"}
                </Button>
              )
            }
          />
        </div>
      </div>
      <CollapsiblePanel>
        <div className="flex flex-col">
          {updates.slice(0, MAX_ROWS_PER_GROUP).map((skill, i) => {
            const key = skillKey("upd", skill);
            return (
              <HomeRow
                key={key}
                skill={skill}
                rowIndex={rowAt(start, i)}
                rowRef={rowRef(key)}
                tabIndex={tabIndexFor(key)}
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
            );
          })}
          {updates.length > MAX_ROWS_PER_GROUP && (
            <ShowAllLink count={updates.length} label="Show all" onClick={onShowAll} />
          )}
        </div>
      </CollapsiblePanel>
    </Collapsible>
  );
}

/**
 * Home's grid: the filter banner, the all-clear empty state, and the five inbox groups (each shown
 * only while visible under the active filter) - pulled out of `HomeView` since this is where all of
 * its per-group branching lived.
 */
export function HomeInboxGrid({
  groups,
  starts,
  filter,
  onClearFilter,
  isGroupVisible,
  isGroupExpanded,
  toggleGroup,
  onSelectSkill,
  onShowAllIssues,
  onShowAllUpdates,
  onShowAllUnused,
  onShowAllRecent,
  openSkill,
  onConvertLinkedRoot,
  containerRef,
  onGridKeyDown,
  rowRef,
  tabIndexFor,
}: RowCursorProps & {
  groups: HomeGroups;
  starts: HomeRowPlan["starts"];
  filter: HomeFilter | null;
  onClearFilter: () => void;
  isGroupVisible: (id: GroupId) => boolean;
  isGroupExpanded: (id: GroupId) => boolean;
  toggleGroup: (id: GroupId) => void;
  onSelectSkill: (name: string) => void;
  onShowAllIssues: () => void;
  onShowAllUpdates: () => void;
  onShowAllUnused: () => void;
  onShowAllRecent: () => void;
  openSkill: (name: string, deploymentPath: string | undefined, intent: "compare") => void;
  onConvertLinkedRoot: (
    skill: InstalledSkill,
    harness: string,
    harnessLabel: string,
    root: string,
  ) => void;
  containerRef: (el: HTMLDivElement | null) => void;
  onGridKeyDown: (e: ReactKeyboardEvent) => void;
}) {
  const { broken, warnings, updates, unused, recent, allClear } = groups;

  return (
    <RichTooltipScope>
      <div
        ref={containerRef}
        className="flex flex-col"
        role="grid"
        aria-label="Home"
        onKeyDown={onGridKeyDown}
      >
        {filter && (
          <div className="flex h-9 items-center gap-2.5 px-3 text-small text-text-tertiary">
            Showing one group ·{" "}
            <Button variant="link" className={LINK_CLASS} onClick={onClearFilter}>
              Show everything
            </Button>
          </div>
        )}

        {allClear && !filter && (
          <p className="flex h-full items-center justify-center text-wrap-pretty text-text-tertiary">
            All clear. Nothing needs attention.
          </p>
        )}

        {broken.length > 0 && isGroupVisible("broken") && (
          <BrokenGroup
            broken={broken}
            start={starts.broken}
            isExpanded={isGroupExpanded("broken")}
            onToggle={() => toggleGroup("broken")}
            onSelectSkill={onSelectSkill}
            onShowAll={onShowAllIssues}
            rowRef={rowRef}
            tabIndexFor={tabIndexFor}
          />
        )}

        {warnings.length > 0 && isGroupVisible("warn") && (
          <WarningsGroup
            warnings={warnings}
            start={starts.warn}
            isExpanded={isGroupExpanded("warn")}
            onToggle={() => toggleGroup("warn")}
            onSelectSkill={onSelectSkill}
            onShowAll={onShowAllIssues}
            openSkill={openSkill}
            onConvertLinkedRoot={onConvertLinkedRoot}
            rowRef={rowRef}
            tabIndexFor={tabIndexFor}
          />
        )}

        {updates.length > 0 && isGroupVisible("upd") && (
          <UpdatesGroup
            updates={updates}
            isExpanded={isGroupExpanded("upd")}
            onToggle={() => toggleGroup("upd")}
            onSelectSkill={onSelectSkill}
            onShowAll={onShowAllUpdates}
            start={starts.upd}
            rowRef={rowRef}
            tabIndexFor={tabIndexFor}
          />
        )}

        {unused.length > 0 && isGroupVisible("unused") && (
          <UnusedGroup
            unused={unused}
            start={starts.unused}
            isExpanded={isGroupExpanded("unused")}
            onToggle={() => toggleGroup("unused")}
            onSelectSkill={onSelectSkill}
            onShowAll={onShowAllUnused}
            rowRef={rowRef}
            tabIndexFor={tabIndexFor}
          />
        )}

        {recent.length > 0 && isGroupVisible("rec") && (
          <RecentGroup
            recent={recent}
            start={starts.rec}
            isExpanded={isGroupExpanded("rec")}
            onToggle={() => toggleGroup("rec")}
            onSelectSkill={onSelectSkill}
            onShowAll={onShowAllRecent}
            rowRef={rowRef}
            tabIndexFor={tabIndexFor}
          />
        )}
      </div>
    </RichTooltipScope>
  );
}
