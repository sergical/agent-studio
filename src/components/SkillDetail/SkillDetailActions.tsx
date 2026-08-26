// ============================================================================
// SkillDetailActions - Remove/update controls for skills.sh skills, plus the
// update-only control for dotagents skills (reveal/open/copy/enable-disable
// live in InstalledSkillHeader; that header shows the "Update available"
// text but never its own button, so there's exactly one update control per
// skill).
// ============================================================================

import { useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { GitFork, RefreshCw } from "lucide-react";
import { InstallControls } from "../SkillStore/InstallControls";
import { forkSkill, pullForkUpstream, unforkSkill, updateSkill } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import type { InstalledSkill, SkillWithStatus } from "../../lib/skill-types";

interface SkillDetailActionsProps {
  skill: InstalledSkill;
  onRemoveComplete: () => void;
}

/** Wraps an `InstalledSkill` in the shape `InstallControls` expects. */
function toSkillWithStatus(skill: InstalledSkill): SkillWithStatus {
  return {
    id: skill.name,
    name: skill.name,
    installs: 0,
    is_installed: true,
    installed_info: skill,
  };
}

/**
 * Update-only control for a dotagents-managed skill: no lock-file metadata,
 * so `InstallControls`' remove flow doesn't apply, but the update flow
 * (`npx @sentry/dotagents add|install`) does.
 */
function DotagentsUpdateButton({ skill }: { skill: InstalledSkill }) {
  const [isUpdating, setIsUpdating] = useState(false);
  const addToast = useAppStore((state) => state.addToast);

  const handleUpdate = async () => {
    setIsUpdating(true);
    try {
      const result = await updateSkill(skill.name, true);
      if (result.success) {
        addToast({
          type: "success",
          title: "Skill updated",
          message: result.tool ? `Ran ${result.tool} for ${skill.name}` : undefined,
        });
      } else {
        addToast({ type: "error", title: "Update failed", message: result.error });
      }
    } catch (err) {
      addToast({
        type: "error",
        title: "Update failed",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
      setIsUpdating(false);
    }
  };

  return (
    <button
      className="skill-action-button primary"
      onClick={handleUpdate}
      disabled={isUpdating}
      title="Dotagents-managed: re-pins to the latest commit, or re-syncs if this skill has no pinned entry"
    >
      {isUpdating ? (
        <>
          <span className="spinner" />
          Updating...
        </>
      ) : (
        <>
          <RefreshCw size={16} />
          Update Skill
        </>
      )}
    </button>
  );
}

/**
 * The one deployment `fork_skill` will accept: the shared-folder copy at
 * `~/.agents/skills/<name>`, or the Claude Code whole-dir symlink to it.
 * `undefined` when neither is deployed, in which case the Fork button has
 * nothing forkable to point at and is hidden.
 */
function sharedFolderDeployment(skill: InstalledSkill) {
  return skill.deployments.find(
    (d) => d.path.includes("/.agents/skills/") || d.path.includes("/.claude/skills/"),
  );
}

/**
 * Detaches a dotagents- or skills.sh-managed skill from its ledger so local
 * edits survive `sync`/`update`. Refused (with an error toast, since the app
 * has no static way to know a dotagents wildcard entry ahead of time) for a
 * wildcard dotagents source.
 */
function ForkButton({ skill, path }: { skill: InstalledSkill; path: string }) {
  const [isForking, setIsForking] = useState(false);
  const addToast = useAppStore((state) => state.addToast);

  const handleFork = async () => {
    setIsForking(true);
    try {
      await forkSkill(skill.name, path);
      addToast({ type: "success", title: "Forked", message: `${skill.name} is now yours to edit` });
    } catch (err) {
      addToast({
        type: "error",
        title: "Fork failed",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
      setIsForking(false);
    }
  };

  return (
    <button
      type="button"
      className="skill-action-button"
      onClick={handleFork}
      disabled={isForking}
      title="Detach from its CLI so your edits are kept; Pull upstream merges later changes"
    >
      {isForking ? (
        <>
          <span className="spinner" />
          Forking...
        </>
      ) : (
        <>
          <GitFork size={16} />
          Fork
        </>
      )}
    </button>
  );
}

/** Pull upstream / Un-fork controls for a skill detached via Fork. Only one
 * of the two can run at a time - a mutation lock on the backend would just
 * reject the second call, so disable both while either is in flight instead
 * of surfacing that as an error. */
function ForkedSkillActions({ skill }: { skill: InstalledSkill }) {
  const [busy, setBusy] = useState<"pull" | "unfork" | null>(null);
  const addToast = useAppStore((state) => state.addToast);
  const origin = skill.fork?.origin_source ?? "its origin";

  const handlePull = async () => {
    setBusy("pull");
    try {
      const result = await pullForkUpstream(skill.name);
      if (result.message) {
        addToast({ type: "info", title: result.message });
      } else if (result.conflicts.length > 0) {
        addToast({
          type: "warning",
          title: `${result.conflicts.length} conflicts — open the editor to resolve`,
          message: result.conflicts.join(", "),
        });
      } else {
        const mergedCount = result.merged.length + result.added.length + result.removed.length;
        addToast({ type: "success", title: `Merged ${mergedCount} files` });
      }
    } catch (err) {
      addToast({
        type: "error",
        title: "Pull upstream failed",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
      setBusy(null);
    }
  };

  const handleUnfork = async () => {
    const confirmed = await ask(
      `Discard your changes and reinstall ${skill.name} from ${origin}?`,
      {
        title: "Un-fork skill",
        kind: "warning",
      },
    );
    if (!confirmed) return;

    setBusy("unfork");
    try {
      await unforkSkill(skill.name);
      addToast({
        type: "success",
        title: "Un-forked",
        message: `${skill.name} is reinstalled from ${origin}`,
      });
    } catch (err) {
      addToast({
        type: "error",
        title: "Un-fork failed",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="skill-detail-section skill-detail-actions-row">
      <button
        type="button"
        className="skill-action-button primary"
        onClick={handlePull}
        disabled={busy !== null || !skill.has_update}
        title={skill.has_update ? "Merge upstream changes into your fork" : "Nothing new upstream"}
      >
        {busy === "pull" ? (
          <>
            <span className="spinner" />
            Pulling...
          </>
        ) : (
          "Pull upstream"
        )}
      </button>
      <button
        type="button"
        className="skill-action-button danger"
        onClick={handleUnfork}
        disabled={busy !== null}
      >
        {busy === "unfork" ? (
          <>
            <span className="spinner" />
            Un-forking...
          </>
        ) : (
          "Un-fork"
        )}
      </button>
    </div>
  );
}

/**
 * Remove/update controls, via the shared `InstallControls`, for skills.sh
 * skills (which carry the lock-file metadata `InstallControls` needs); an
 * update-only control for a dotagents skill with an update available; or
 * Pull upstream / Un-fork for a forked skill. A dotagents or skills.sh skill
 * also gets a Fork button alongside its other controls. Manual and plugin
 * skills have no owning CLI to update or fork through, so they render
 * nothing here.
 */
export function SkillDetailActions({ skill, onRemoveComplete }: SkillDetailActionsProps) {
  if (skill.source_kind === "fork") {
    return <ForkedSkillActions skill={skill} />;
  }

  const forkPath = sharedFolderDeployment(skill)?.path;

  if (skill.source_kind === "skills-sh") {
    return (
      <div className="skill-detail-section skill-detail-actions-row">
        <InstallControls
          skill={toSkillWithStatus(skill)}
          resolvedTopSource={null}
          onInstallStart={() => {}}
          onInstallComplete={() => {}}
          onRemoveComplete={onRemoveComplete}
        />
        {forkPath && <ForkButton skill={skill} path={forkPath} />}
      </div>
    );
  }

  if (skill.source_kind === "dotagents") {
    return (
      <div className="skill-detail-section skill-detail-actions-row">
        {skill.has_update && <DotagentsUpdateButton skill={skill} />}
        {forkPath && <ForkButton skill={skill} path={forkPath} />}
      </div>
    );
  }

  return null;
}
