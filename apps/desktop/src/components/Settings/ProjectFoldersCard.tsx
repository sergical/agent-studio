// ============================================================================
// ProjectFoldersCard - Settings' "Project folders" card: every folder
// discovery found or the user added by hand, the per-harness search
// switches that decide what discovery looks at, and the actions to stop
// tracking or remove a folder. Shares its add/stop-tracking/remove logic
// with the Skills filter bar through `useProjectFolderActions`, so both
// surfaces agree on one set of tracked folders.
// ============================================================================

import { useEffect, useEffectEvent, useRef, useState } from "react";
import { AlertTriangle, FolderOpen, Plus } from "lucide-react";
import { Button } from "@skill-studio/ui";
import {
  deploymentLabelFromAgentId,
  homeRelativePath,
  type AgentId,
  type DiscoverySourceSetting,
  type ProjectFolder,
  type SkillSnapshot,
} from "@skill-studio/lib";
import {
  getDiscoverySources,
  invokeErrorMessage,
  listProjectFolders,
  setDiscoverySource,
} from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import { useProjectFolderActions } from "../../hooks/useProjectFolderActions";
import { HarnessIcon } from "../ui/HarnessIcon";
import { SwitchControl } from "../ui/SwitchControl";
import { SettingsCard } from "./SettingsCard";

interface ProjectFoldersCardProps {
  snapshot: SkillSnapshot | undefined;
}

/** "A", "A and B", "A, B and C" - the empty state's list of searched harnesses. */
function joinWithAnd(items: string[]): string {
  if (items.length === 0) return "";
  if (items.length === 1) return items[0];
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/** One skill per project path its deployments touch, counted once even when a skill deploys to
 * the same project through more than one harness - matches `SkillsView.tsx`'s own scope match. */
function countSkillsByProject(snapshot: SkillSnapshot | undefined): Map<string, number> {
  const counts = new Map<string, number>();
  for (const skill of snapshot?.skills ?? []) {
    const projectPaths = new Set(
      skill.deployments.map((d) => d.project_path).filter((path): path is string => Boolean(path)),
    );
    for (const path of projectPaths) {
      counts.set(path, (counts.get(path) ?? 0) + 1);
    }
  }
  return counts;
}

function FolderRow({
  folder,
  skillCount,
  onStopTracking,
  onRemove,
}: {
  folder: ProjectFolder;
  skillCount: number;
  onStopTracking: (path: string) => void;
  onRemove: (path: string) => void;
}) {
  const displayPath = homeRelativePath(folder.path);
  const isDiscovered = folder.source === "discovered";

  return (
    <li className="flex min-w-0 items-center gap-3 px-3 py-2">
      <div className="min-w-0 flex-1">
        <p className="m-0 truncate text-body text-text-primary" title={folder.path}>
          {displayPath}
        </p>
        <p className="m-0 flex items-center gap-1 overflow-hidden whitespace-nowrap text-small text-text-tertiary">
          {folder.missing ? (
            <>
              <AlertTriangle size={12} className="text-warning" aria-hidden="true" />
              Folder not found · Added by you
            </>
          ) : (
            <>
              {isDiscovered ? "Found in harness history" : "Added by you"}
              {" · "}
              <span className="tabular-nums">
                {skillCount} {skillCount === 1 ? "skill" : "skills"}
              </span>
            </>
          )}
        </p>
      </div>
      <Button
        variant="ghost"
        size="sm"
        className="shrink-0"
        aria-label={`${isDiscovered ? "Stop tracking" : "Remove"} ${displayPath}`}
        onClick={() => (isDiscovered ? onStopTracking(folder.path) : onRemove(folder.path))}
      >
        {isDiscovered ? "Stop tracking" : "Remove"}
      </Button>
    </li>
  );
}

export function ProjectFoldersCard({ snapshot }: ProjectFoldersCardProps) {
  const addToast = useAppStore((state) => state.addToast);
  const { addProject, stopTracking, removeProject } = useProjectFolderActions();
  const [folders, setFolders] = useState<ProjectFolder[] | null>(null);
  const [sources, setSources] = useState<DiscoverySourceSetting[] | null>(null);
  const requestId = useRef(0);

  const refetchFolders = () => {
    const id = ++requestId.current;
    listProjectFolders()
      .then((rows) => {
        if (id === requestId.current) setFolders(rows);
      })
      .catch((err) => {
        if (id !== requestId.current) return;
        addToast({
          type: "error",
          title: "Couldn't list project folders",
          message: invokeErrorMessage(err),
        });
      });
  };
  // Effect Event: reads the latest `refetchFolders` without becoming an effect dependency
  // itself, so the effect below reruns only when `projectsKey` changes.
  const refetchFoldersOnProjectsChange = useEffectEvent(refetchFolders);

  useEffect(() => {
    getDiscoverySources()
      .then(setSources)
      .catch((err) => {
        addToast({
          type: "error",
          title: "Couldn't read discovery settings",
          message: invokeErrorMessage(err),
        });
      });
  }, [addToast]);

  const projectsKey = (snapshot?.projects ?? []).join("\n");
  useEffect(() => {
    refetchFoldersOnProjectsChange();
    // `projectsKey` is not read here - it stays a dependency purely to retrigger the fetch when a
    // scan adds or removes a tracked project.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies -- retrigger-only dependency, not read in the body
  }, [projectsKey]);

  const skillCounts = countSkillsByProject(snapshot);

  const handleAddFolder = async () => {
    const added = await addProject();
    if (added) refetchFolders();
  };

  const handleStopTracking = async (path: string) => {
    await stopTracking(path);
    refetchFolders();
  };

  const handleRemove = async (path: string) => {
    await removeProject(path);
    refetchFolders();
  };

  const handleToggleSource = async (harness: string, enabled: boolean) => {
    const previous = sources;
    setSources((current) =>
      (current ?? []).map((s) => (s.harness === harness ? { ...s, enabled } : s)),
    );
    try {
      const updated = await setDiscoverySource(harness, enabled);
      setSources(updated);
      refetchFolders();
    } catch (err) {
      setSources(previous);
      addToast({
        type: "error",
        title: "Couldn't change discovery setting",
        message: invokeErrorMessage(err),
      });
    }
  };

  const isLoading = folders === null || sources === null;
  const enabledLabels: string[] = [];
  for (const source of sources ?? []) {
    if (source.enabled) enabledLabels.push(deploymentLabelFromAgentId(source.harness));
  }

  return (
    <SettingsCard
      icon={<FolderOpen size={15} className="text-text-tertiary" />}
      title="Project folders"
      description="Skill Studio finds project folders in the history of the harnesses below and shows the skills inside them. Add a folder it missed, or stop tracking one you don't need."
      action={
        <Button variant="outline" size="sm" onClick={handleAddFolder}>
          <Plus size={14} />
          Add folder…
        </Button>
      }
    >
      <div className="flex flex-col gap-2">
        <p className="m-0 text-small font-medium text-text-tertiary">Search history from</p>
        <div className="grid grid-cols-1 gap-x-6 sm:grid-cols-2">
          {(sources ?? []).map((source) => {
            const label = deploymentLabelFromAgentId(source.harness);
            // SAFETY: `source.harness` is always one of `discovery_harnesses()`'s six ids, which
            // are exactly `AgentId`'s wire values minus "shared".
            const harnessId = source.harness as AgentId;
            return (
              <label
                key={source.harness}
                className="flex h-8 cursor-pointer items-center gap-2 text-body text-text-secondary"
              >
                <HarnessIcon harness={harnessId} size={14} />
                <span className="flex-1">{label}</span>
                <SwitchControl
                  checked={source.enabled}
                  onCheckedChange={(enabled) => handleToggleSource(source.harness, enabled)}
                />
              </label>
            );
          })}
        </div>
      </div>

      <div className="rounded-md border border-border-subtle divide-y divide-border-subtle">
        {isLoading ? (
          <p className="m-0 px-3 py-2 text-small text-text-tertiary">
            Looking for project folders…
          </p>
        ) : folders.length === 0 ? (
          <p className="m-0 px-3 py-2 text-small text-text-tertiary">
            {enabledLabels.length === 0
              ? "Search is off for every harness, so only folders you add appear here."
              : `No project folders with skills yet. Skill Studio searched the history of ${joinWithAnd(
                  enabledLabels,
                )}. If your project is somewhere else, add it by hand.`}
          </p>
        ) : (
          <ul className="m-0 list-none p-0">
            {folders.map((folder) => (
              <FolderRow
                key={folder.path}
                folder={folder}
                skillCount={skillCounts.get(folder.path) ?? 0}
                onStopTracking={handleStopTracking}
                onRemove={handleRemove}
              />
            ))}
          </ul>
        )}
      </div>
    </SettingsCard>
  );
}
