// ============================================================================
// SkillHistorySection - Activity view's History list: one row per event
// store entry (docs/spec-event-store.md), with restore, drift-guard
// force-restore, and "Reveal in Finder" for backed-up events.
// ============================================================================

import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import {
  Archive,
  ArchiveRestore,
  FolderSymlink,
  History as HistoryIcon,
  Link2,
  Link2Off,
  Undo2,
} from "lucide-react";
import { formatRelativeTime } from "@skill-studio/lib";
import type { SkillEvent } from "@skill-studio/lib";
import { listSkillEvents, openSkillPath, restoreSkillEvent } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import { HARNESS_LABELS } from "../../lib/harness-labels";

const HARNESS_LABEL_BY_ID = new Map<string, string>(HARNESS_LABELS);

/** Icon per event kind - see the Materialize section of spec-event-store.md for what each kind does. */
function iconForKind(kind: string, className: string) {
  const props = { size: 14, className: `shrink-0 ${className}` };
  switch (kind) {
    case "restore":
      return <Undo2 {...props} />;
    case "unlink_harness":
    case "harness_disable":
      return <Link2Off {...props} />;
    case "relink_harness":
    case "harness_enable":
      return <Link2 {...props} />;
    case "explode_shared_dir":
    case "distribute_from_shared":
      return <FolderSymlink {...props} />;
    case "move_aside_disable":
      return <Archive {...props} />;
    case "move_aside_restore":
      return <ArchiveRestore {...props} />;
    default:
      return <HistoryIcon {...props} />;
  }
}

/** "unlink harness" from "unlink_harness", for kinds with no friendlier label. */
function kindLabel(kind: string): string {
  return kind.replace(/_/g, " ");
}

/** The backend's drift-guard refusal names the drifted path and ends in this phrase - see event_store.rs. */
function isDriftRefusal(message: string): boolean {
  return message.includes("changed since") || message.includes("drifted");
}

/** What a restore's confirm dialog names as "what will be put back" - the inverse of the event's own kind. */
function restoreDescription(event: SkillEvent): string {
  const skillPart = event.skill ? `${event.skill}` : (event.harness ?? "this item");
  switch (event.kind) {
    case "unlink_harness":
      return `Restore ${skillPart}'s link for ${event.harness ?? "its harness"}`;
    case "explode_shared_dir":
      return `Restore ${event.harness ?? "the harness"}'s whole-folder link`;
    case "distribute_from_shared":
      return `Move ${skillPart} back into the Universal folder and remove the per-harness copies`;
    case "move_aside_disable":
      return `Restore ${skillPart} to its original location`;
    default:
      return `Undo "${kindLabel(event.kind)}" for ${skillPart}`;
  }
}

function EventRow({ event, onRestored }: { event: SkillEvent; onRestored: () => void }) {
  const addToast = useAppStore((state) => state.addToast);
  const [isRestoring, setIsRestoring] = useState(false);
  const isFailed = event.status === "failed";
  const isInterrupted = event.status === "interrupted";
  const icon = iconForKind(
    event.kind,
    isFailed ? "text-error" : isInterrupted ? "text-warning" : "text-text-tertiary",
  );
  const harnessLabel = event.harness
    ? (HARNESS_LABEL_BY_ID.get(event.harness) ?? event.harness)
    : null;

  const handleReveal = () => {
    if (!event.backup_path) return;
    openSkillPath(event.backup_path, "reveal").catch((err) => {
      addToast({
        type: "error",
        title: "Couldn't reveal in Finder",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    });
  };

  const runRestore = async (force: boolean) => {
    setIsRestoring(true);
    try {
      await restoreSkillEvent(event.id, force);
      addToast({ type: "success", title: "Restored", message: restoreDescription(event) });
      onRestored();
      setIsRestoring(false);
    } catch (err) {
      const message = err instanceof Error ? err.message : "Unknown error";
      if (!force && isDriftRefusal(message)) {
        const proceed = await ask(
          `${message}\n\nRestoring anyway will back up the current content first, so it stays restorable.`,
          { title: "Content has changed", kind: "warning" },
        );
        if (proceed) {
          await runRestore(true);
          setIsRestoring(false);
          return;
        }
      } else {
        addToast({ type: "error", title: "Restore failed", message });
      }
      setIsRestoring(false);
    }
  };

  const handleRestoreClick = async () => {
    const confirmed = await ask(`${restoreDescription(event)}?`, {
      title: "Restore",
      kind: "info",
    });
    if (!confirmed) return;
    await runRestore(false);
  };

  return (
    <div
      className={`flex min-h-9 items-center gap-3 border-b border-border-subtle px-2 py-1.5 last:border-b-0 ${
        isFailed ? "bg-error-soft" : isInterrupted ? "bg-warning-soft" : ""
      }`}
    >
      {icon}
      <span className="min-w-0 flex-1 truncate text-body text-text-primary" title={event.skill}>
        {event.skill || (event.harness ?? kindLabel(event.kind))}
      </span>
      <span className="text-small text-text-tertiary">{kindLabel(event.kind)}</span>
      {harnessLabel && (
        <span className="shrink-0 text-small text-text-tertiary">{harnessLabel}</span>
      )}
      <span className="shrink-0 text-small text-text-tertiary tabular-nums">
        {formatRelativeTime(event.ts)}
      </span>
      <span
        className={`shrink-0 text-caption font-semibold ${
          isFailed ? "text-error" : isInterrupted ? "text-warning" : "text-text-tertiary"
        }`}
      >
        {isInterrupted ? "Interrupted - the app was quit during this operation" : event.status}
      </span>
      {event.backup_path && (
        <button
          type="button"
          className="shrink-0 cursor-pointer border-0 bg-transparent p-0 text-small text-accent hover:underline"
          onClick={handleReveal}
        >
          Reveal in Finder
        </button>
      )}
      {event.restorable && (
        <button
          type="button"
          className="shrink-0 cursor-pointer rounded-sm border border-border-subtle bg-transparent px-2 py-1 text-small text-text-secondary transition-colors hover:bg-bg-hover disabled:opacity-50"
          onClick={handleRestoreClick}
          disabled={isRestoring}
        >
          Restore
        </button>
      )}
    </div>
  );
}

/**
 * The Activity view's History section: every event store row, newest first.
 * Fetched on mount and refetched after a successful restore - the backend
 * already re-emits `skills://snapshot` on its own after a mutating command,
 * so nothing else needs to be triggered here.
 */
export function SkillHistorySection() {
  const [events, setEvents] = useState<SkillEvent[] | null>(null);
  const addToast = useAppStore((state) => state.addToast);

  useEffect(() => {
    let cancelled = false;
    listSkillEvents()
      .then((rows) => {
        if (!cancelled) setEvents(rows);
      })
      .catch((err) => {
        if (!cancelled) setEvents([]);
        addToast({
          type: "error",
          title: "Couldn't load history",
          message: err instanceof Error ? err.message : "Unknown error",
        });
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const refresh = () => {
    listSkillEvents()
      .then(setEvents)
      .catch(() => {
        // A refetch failure after a successful restore isn't worth a second
        // toast - the row stays as it was until the next successful load.
      });
  };

  return (
    <div className="flex flex-col gap-3">
      <span className="text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
        History
      </span>
      {events === null ? (
        <p className="text-wrap-pretty text-body text-text-tertiary">Loading…</p>
      ) : events.length === 0 ? (
        <p className="text-wrap-pretty text-body text-text-tertiary">No events recorded yet.</p>
      ) : (
        <div className="flex flex-col">
          {events.map((event) => (
            <EventRow key={event.id} event={event} onRestored={refresh} />
          ))}
        </div>
      )}
    </div>
  );
}
