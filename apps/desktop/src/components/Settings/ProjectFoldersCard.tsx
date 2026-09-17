// ============================================================================
// ProjectFoldersCard - Settings' "Project folders" card: every folder
// discovery found or the user added by hand, the per-harness search
// switches that decide what discovery looks at, and the actions to stop
// tracking or remove a folder. Shares its add/stop-tracking/remove logic
// with the Skills filter bar through `useProjectFolderActions`, so both
// surfaces agree on one set of tracked folders.
// ============================================================================

import { useEffect, useEffectEvent, useRef, useState } from "react";
import type { RefObject } from "react";
import { AlertTriangle, Asterisk, ChevronDown, FolderOpen, Plus } from "lucide-react";
import {
  Button,
  buttonVariants,
  Collapsible,
  CollapsiblePanel,
  CollapsibleTrigger,
} from "@skill-studio/ui";
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
import { MenuControl, MenuItem } from "../ui/MenuControl";
import { HarnessIcon } from "../ui/HarnessIcon";
import { SwitchControl } from "../ui/SwitchControl";
import { ProjectFolderAddForm } from "./ProjectFolderAddForm";
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

/** The row's secondary line: a missing-folder warning, a pattern's match count, or a plain
 * folder's discovered/added label and skill count. Split out of `FolderRow` to keep it a single
 * flat branch instead of nested ternaries. */
function FolderSecondaryText({
  folder,
  isDiscovered,
  isPattern,
  skillCount,
}: {
  folder: ProjectFolder;
  isDiscovered: boolean;
  isPattern: boolean;
  skillCount: number;
}) {
  if (folder.missing) {
    return (
      <>
        <AlertTriangle size={12} className="text-warning" aria-hidden="true" />
        Folder not found · Added by you
      </>
    );
  }
  if (isPattern) {
    return (
      <>
        Added by you
        {" · "}
        <span className="tabular-nums">
          {folder.matches === 0
            ? "No matching folders"
            : `${folder.matches} ${folder.matches === 1 ? "folder" : "folders"}`}
        </span>
      </>
    );
  }
  return (
    <>
      {isDiscovered ? "Found in harness history" : "Added by you"}
      {" · "}
      <span className="tabular-nums">
        {skillCount} {skillCount === 1 ? "skill" : "skills"}
      </span>
    </>
  );
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
  const isPattern = folder.matches != null;

  return (
    <li className="flex min-w-0 items-center gap-3 px-3 py-2">
      <div className="min-w-0 flex-1">
        <p
          className={`m-0 truncate text-body text-text-primary ${isPattern ? "font-mono" : ""}`}
          title={folder.path}
        >
          {displayPath}
        </p>
        <p className="m-0 flex items-center gap-1 overflow-hidden whitespace-nowrap text-small text-text-tertiary">
          <FolderSecondaryText
            folder={folder}
            isDiscovered={isDiscovered}
            isPattern={isPattern}
            skillCount={skillCount}
          />
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

/** The folder list, closed behind a one-line summary until opened. */
function FolderList({
  folders,
  sources,
  skillCounts,
  open,
  onOpenChange,
  onStopTracking,
  onRemove,
  addFormOpen,
  onAddTypedPath,
  onCloseAddForm,
  triggerRef,
}: {
  folders: ProjectFolder[] | null;
  sources: DiscoverySourceSetting[] | null;
  skillCounts: Map<string, number>;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onStopTracking: (path: string) => void;
  onRemove: (path: string) => void;
  addFormOpen: boolean;
  onAddTypedPath: (value: string) => Promise<string | null>;
  onCloseAddForm: () => void;
  /** Focused after the add form closes, so focus doesn't drop to the body. */
  triggerRef: RefObject<HTMLButtonElement | null>;
}) {
  if (folders === null || sources === null) {
    return (
      <p className="m-0 rounded-md border border-border-subtle px-3 py-2 text-small text-text-tertiary">
        Looking for project folders…
      </p>
    );
  }

  if (folders.length === 0 && !addFormOpen) {
    const enabledLabels: string[] = [];
    for (const source of sources) {
      if (source.enabled) enabledLabels.push(deploymentLabelFromAgentId(source.harness));
    }
    return (
      <p className="m-0 rounded-md border border-border-subtle px-3 py-2 text-small text-text-tertiary">
        {enabledLabels.length === 0
          ? "Search is off for every harness, so only folders you add appear here."
          : `No project folders with skills yet. Skill Studio searched the history of ${joinWithAnd(
              enabledLabels,
            )}. If your project is somewhere else, add it by hand.`}
      </p>
    );
  }

  const missingCount = folders.filter((folder) => folder.missing).length;
  // A plain folder counts as one; a pattern counts as however many of its matched folders made
  // the resolved set, so this line reads like "N folders" even though one pattern is one row.
  const folderCount = folders.reduce(
    (total, folder) => (folder.missing ? total : total + (folder.matches ?? 1)),
    0,
  );
  return (
    // No overflow-hidden here: the global :focus-visible outline sits 2px outside the trigger and
    // would be clipped.
    <Collapsible
      open={open}
      onOpenChange={onOpenChange}
      className="rounded-md border border-border-subtle"
    >
      <CollapsibleTrigger
        ref={triggerRef}
        className="group/folders flex h-9 w-full items-center gap-2 rounded-md px-3 text-left text-small text-text-secondary transition-colors hover:bg-bg-hover data-panel-open:rounded-b-none"
      >
        <ChevronDown
          aria-hidden
          className="size-3.5 shrink-0 -rotate-90 text-text-tertiary transition-transform motion-reduce:transition-none group-data-panel-open/folders:rotate-0"
        />
        <span className="font-medium tabular-nums">
          {folderCount} {folderCount === 1 ? "folder" : "folders"}
        </span>
        {missingCount > 0 && (
          <span className="flex items-center gap-1 text-text-tertiary">
            <span aria-hidden="true">·</span>
            <AlertTriangle size={12} className="text-warning" aria-hidden="true" />
            <span className="tabular-nums">{missingCount}</span> not found
          </span>
        )}
      </CollapsibleTrigger>
      <CollapsiblePanel>
        {addFormOpen && <ProjectFolderAddForm onAdd={onAddTypedPath} onClose={onCloseAddForm} />}
        <ul className="m-0 list-none border-t border-border-subtle p-0">
          {folders.map((folder) => (
            <FolderRow
              key={folder.path}
              folder={folder}
              skillCount={skillCounts.get(folder.path) ?? 0}
              onStopTracking={onStopTracking}
              onRemove={onRemove}
            />
          ))}
        </ul>
      </CollapsiblePanel>
    </Collapsible>
  );
}

export function ProjectFoldersCard({ snapshot }: ProjectFoldersCardProps) {
  const addToast = useAppStore((state) => state.addToast);
  const { addProject, addTypedPath, stopTracking, removeProject } = useProjectFolderActions();
  const [folders, setFolders] = useState<ProjectFolder[] | null>(null);
  const [sources, setSources] = useState<DiscoverySourceSetting[] | null>(null);
  const [foldersOpen, setFoldersOpen] = useState(false);
  const [addFormOpen, setAddFormOpen] = useState(false);
  const requestId = useRef(0);
  const foldersTriggerRef = useRef<HTMLButtonElement>(null);

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
    if (!added) return;
    setFoldersOpen(true);
    refetchFolders();
  };

  const handleStartTyping = () => {
    setFoldersOpen(true);
    setAddFormOpen(true);
  };

  // Cancel, Escape, and a successful Add all close the form through this one path, so focus
  // always lands back on the "N folders" trigger instead of dropping to the body.
  const closeAddForm = () => {
    setAddFormOpen(false);
    foldersTriggerRef.current?.focus();
  };

  const handleAddTypedPath = async (value: string): Promise<string | null> => {
    const error = await addTypedPath(value);
    if (!error) refetchFolders();
    return error;
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

  return (
    <SettingsCard
      icon={<FolderOpen size={15} className="text-text-tertiary" />}
      title="Project folders"
      description="Skill Studio finds project folders in the history of the harnesses below and shows the skills inside them. Add a folder it missed, or stop tracking one you don't need."
      action={
        <MenuControl
          triggerClassName={buttonVariants({ variant: "outline", size: "sm" })}
          triggerAriaLabel="Add folder"
          trigger={
            <>
              <Plus size={14} aria-hidden="true" />
              Add folder
              <ChevronDown size={12} aria-hidden="true" />
            </>
          }
        >
          <MenuItem closeOnClick onClick={handleAddFolder}>
            <FolderOpen size={14} aria-hidden="true" />
            Choose a folder…
          </MenuItem>
          <MenuItem closeOnClick onClick={handleStartTyping}>
            <Asterisk size={14} aria-hidden="true" />
            Type a path or pattern…
          </MenuItem>
        </MenuControl>
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

      <FolderList
        folders={folders}
        sources={sources}
        skillCounts={skillCounts}
        open={foldersOpen}
        onOpenChange={setFoldersOpen}
        onStopTracking={handleStopTracking}
        onRemove={handleRemove}
        addFormOpen={addFormOpen}
        onAddTypedPath={handleAddTypedPath}
        onCloseAddForm={closeAddForm}
        triggerRef={foldersTriggerRef}
      />
    </SettingsCard>
  );
}
