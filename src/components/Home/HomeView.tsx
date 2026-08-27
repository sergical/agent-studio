// ============================================================================
// HomeView - "What needs doing" for the user's own skills: a one-line
// summary, needs-attention rows, updates, and recently-used skills. See the
// design rule in spec-ux-1.md: Home holds what needs doing, not everything
// there is to know about a skill (that's the skill page or the coverage
// column in Skills).
// ============================================================================

import { useMemo, useState } from "react";
import { RefreshCw } from "lucide-react";
import { homeSummaryCounts, recentlyUsedSkills } from "../../lib/home-summary";
import { pullForkUpstream, updateSkill } from "../../lib/skill-api";
import { collectDashboardIssues } from "../../lib/skill-health";
import { ownSkillsView } from "../../lib/skill-plugin-partition";
import { formatRelativeTime, formatTokens, shortSha } from "../../lib/skill-stats";
import { skillsWithUpdates } from "../../lib/skill-updates";
import type { InstalledSkill, SkillSnapshot } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";
import { PageShell } from "../Shell/PageShell";
import { TooltipControl } from "../ui/TooltipControl";
import { NeedsAttentionCard } from "./NeedsAttentionCard";

const RECENTLY_USED_COUNT = 5;

interface HomeViewProps {
  snapshot: SkillSnapshot | undefined;
  isLoading: boolean;
  onSelectSkill: (name: string) => void;
}

/**
 * "Pull latest" for one Updates row: a fork pulls upstream via
 * `pullForkUpstream`, any other managed skill re-syncs via `updateSkill` -
 * the same two actions `SkillDetailActions` calls, just without its full
 * remove/fork UI.
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
    <button className="home-updates-pull" onClick={handlePull} disabled={isPulling}>
      {isPulling ? <span className="spinner" /> : <RefreshCw size={13} />}
      Pull latest
    </button>
  );
}

/**
 * Home's "Updates" block: one row per skill with a newer commit available,
 * plus an "Update all" secondary button in the header once there's more
 * than one - sequential, ending in a single toast with the count and any
 * failures rather than one toast per skill.
 */
function UpdatesSection({
  skills,
  onSelectSkill,
}: {
  skills: InstalledSkill[];
  onSelectSkill: (name: string) => void;
}) {
  const [isUpdatingAll, setIsUpdatingAll] = useState(false);
  const addToast = useAppStore((state) => state.addToast);

  if (skills.length === 0) return null;

  const handleUpdateAll = async () => {
    setIsUpdatingAll(true);
    let failures = 0;
    try {
      for (const skill of skills) {
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
      const succeeded = skills.length - failures;
      addToast({
        type: failures > 0 ? "warning" : "success",
        title: `Updated ${succeeded} of ${skills.length} skill${skills.length === 1 ? "" : "s"}`,
        message: failures > 0 ? `${failures} failed` : undefined,
      });
    } finally {
      setIsUpdatingAll(false);
    }
  };

  return (
    <div className="home-block">
      <div className="home-block-header">
        <span>
          Updates <span className="home-block-count count-tabular">{skills.length}</span>
        </span>
        {skills.length > 1 && (
          <button
            className="home-block-header-action"
            onClick={handleUpdateAll}
            disabled={isUpdatingAll}
          >
            {isUpdatingAll ? "Updating…" : "Update all"}
          </button>
        )}
      </div>
      <div className="home-updates">
        {skills.map((skill) => (
          <div key={skill.name} className="home-updates-row">
            <button className="home-updates-name" onClick={() => onSelectSkill(skill.name)}>
              {skill.name}
            </button>
            <span className="home-updates-commit count-tabular">
              {skill.content_hash && shortSha(skill.content_hash)}
              {skill.update_commit && ` → ${shortSha(skill.update_commit)}`}
            </span>
            <span className="home-updates-commit count-tabular" title="SKILL.md tokens">
              {formatTokens(skill.skill_md_tokens)}
            </span>
            <PullLatestButton skill={skill} />
          </div>
        ))}
      </div>
    </div>
  );
}

/** Home's "Recently used" block: the 5 skills with the latest invocation. */
function RecentlyUsedSection({
  snapshot,
  onSelectSkill,
  onSeeActivity,
}: {
  snapshot: SkillSnapshot;
  onSelectSkill: (name: string) => void;
  onSeeActivity: () => void;
}) {
  const rows = useMemo(
    () => recentlyUsedSkills(snapshot.skills, snapshot.invocations, RECENTLY_USED_COUNT),
    [snapshot],
  );

  if (rows.length === 0) return null;

  return (
    <div className="home-block">
      <div className="home-block-header">
        <span>Recently used</span>
      </div>
      <div className="home-recently-used">
        {rows.map(({ skill, lastUsed, projectLabel, usesIn30Days }) => (
          <button
            key={skill.name}
            className="home-recently-used-row"
            onClick={() => onSelectSkill(skill.name)}
          >
            <span className="home-recently-used-name">{skill.name}</span>
            <span className="home-recently-used-project">{projectLabel ?? ""}</span>
            <span className="home-recently-used-time">{formatRelativeTime(lastUsed)}</span>
            <span className="home-recently-used-count count-tabular">{usesIn30Days}</span>
          </button>
        ))}
      </div>
      <button className="home-block-footer-link" onClick={onSeeActivity}>
        See all activity
      </button>
    </div>
  );
}

/**
 * Home: a one-line summary, "Needs attention", "Updates" (hidden when
 * empty), and "Recently used" (hidden when there's no invocation history) -
 * four blocks in one column, nothing else. No coverage matrix, trend chart,
 * or window control here; those live in Skills and Activity.
 */
export function HomeView({ snapshot, isLoading, onSelectSkill }: HomeViewProps) {
  const setActiveView = useAppStore((state) => state.setActiveView);

  const own = useMemo(() => ownSkillsView(snapshot?.skills ?? []), [snapshot]);
  const issues = useMemo(() => collectDashboardIssues(own), [own]);
  const updates = useMemo(() => (snapshot ? skillsWithUpdates(snapshot) : []), [snapshot]);

  if (!snapshot) {
    return (
      <PageShell title="Home">
        <p className="home-empty">
          {isLoading ? "Scanning installed skills…" : "No skill snapshot yet."}
        </p>
      </PageShell>
    );
  }

  const counts = homeSummaryCounts(snapshot);
  const goToActivity = () => setActiveView({ kind: "activity" });
  const goToSkills = () => setActiveView({ kind: "skills" });
  const goToPacks = () => setActiveView({ kind: "packs" });

  return (
    <PageShell title="Home">
      <p className="home-summary count-tabular">
        <TooltipControl content="Skills you installed or wrote">
          <button className="home-summary-segment" onClick={goToSkills}>
            {counts.skillCount} skill{counts.skillCount === 1 ? "" : "s"}
          </button>
        </TooltipControl>
        {counts.pluginSkillCount > 0 && (
          <>
            {" · "}
            <TooltipControl content="Skills that ship inside a harness plugin">
              <button className="home-summary-segment" onClick={goToPacks}>
                {counts.pluginSkillCount} plugin skill{counts.pluginSkillCount === 1 ? "" : "s"}
              </button>
            </TooltipControl>
          </>
        )}
        {" · "}
        {counts.harnessCount} harness{counts.harnessCount === 1 ? "" : "es"}
        {" · "}
        {counts.projectCount} project{counts.projectCount === 1 ? "" : "s"}
        {" · "}
        {counts.usesIn30Days} use{counts.usesIn30Days === 1 ? "" : "s"} in 30 days
      </p>

      {issues.length > 0 ? (
        <div className="home-block">
          <div className="home-block-header">
            <span>
              Needs attention{" "}
              <span className="home-block-count count-tabular">{issues.length}</span>
            </span>
          </div>
          <NeedsAttentionCard issues={issues} onSelectSkill={onSelectSkill} />
        </div>
      ) : (
        <NeedsAttentionCard issues={issues} onSelectSkill={onSelectSkill} />
      )}

      <UpdatesSection skills={updates} onSelectSkill={onSelectSkill} />

      <RecentlyUsedSection
        snapshot={snapshot}
        onSelectSkill={onSelectSkill}
        onSeeActivity={goToActivity}
      />
    </PageShell>
  );
}
