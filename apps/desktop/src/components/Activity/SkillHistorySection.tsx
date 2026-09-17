// ============================================================================
// SkillHistorySection - Activity view's History list: one row per event
// store entry (docs/spec-event-store.md), with restore, drift-guard
// force-restore, and "Reveal in Finder" for backed-up events.
// ============================================================================

import { useCallback, useEffect, useRef, useState } from "react";
import { useDocumentCancellation } from "../../hooks/useDocumentCancellation";
import { errorMessage } from "../../lib/error-message";
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
import {
  invalidateSkillEventReads,
  listSkillEvents,
  openSkillPath,
  restoreExpiredTrialBackup,
  restoreSkillEvent,
} from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import { HARNESS_LABELS } from "../../lib/harness-labels";
import { canRestoreSkillEvent, shouldOfferForceRestore } from "./skill-history-restore-policy";

const HARNESS_LABEL_BY_ID = new Map<string, string>(HARNESS_LABELS);
const HISTORY_PAGE_SIZE = 20;

/** Icon per event kind - see the Materialize section of spec-event-store.md for what each kind does. */
function iconForKind(kind: string, className: string) {
  const props = { size: 14, className: `shrink-0 ${className}` };
  switch (kind) {
    case "undo_copy_frontmatter":
    case "undo_copy_document":
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
    case "make_independent_copy":
      return <FolderSymlink {...props} />;
    case "move_aside_disable":
      return <Archive {...props} />;
    case "restore_expired_copy_trial_backup":
    case "move_aside_restore":
      return <ArchiveRestore {...props} />;
    default:
      return <HistoryIcon {...props} />;
  }
}

/** "unlink harness" from "unlink_harness", for kinds with no friendlier label. */
function kindLabel(kind: string): string {
  switch (kind) {
    case "edit_copy_document":
      return "Edited copy";
    case "undo_copy_document":
      return "Undid copy edit";
    case "redo_copy_document":
      return "Redid copy edit";
    case "repair_copy_frontmatter":
      return "Repaired copy";
    case "undo_copy_frontmatter":
      return "Undid copy repair";
    case "redo_copy_frontmatter":
      return "Reapplied copy repair";
    case "pull_fork_upstream":
      return "Pulled upstream";
    case "unfork_dotagents":
      return "Restored Dotagents management";
    case "unfork_skills_sh":
      return "Restored skills.sh management";
    case "expire_copy_trial":
      return "Copy trial expired";
    case "restore_expired_copy_trial_backup":
      return "Restored trial backup";
    default:
      return kind.replace(/_/g, " ");
  }
}

/** What a restore's confirm dialog names as "what will be put back" - the inverse of the event's own kind. */
function restoreDescription(event: SkillEvent): string {
  const skillPart = event.skill ? `${event.skill}` : (event.harness ?? "this item");
  switch (event.kind) {
    case "expire_copy_trial":
      return `Restore retained content for ${skillPart} as an untracked Global skill`;
    case "edit_copy_document":
    case "redo_copy_document":
      return `Undo the edit to ${skillPart}`;
    case "undo_copy_document":
      return `Redo the edit to ${skillPart}`;
    case "repair_copy_frontmatter":
    case "redo_copy_frontmatter":
      return `Undo the YAML repair for ${skillPart}`;
    case "undo_copy_frontmatter":
      return `Reapply the YAML repair for ${skillPart}`;
    case "unlink_harness":
      return `Restore ${skillPart}'s link for ${event.harness ?? "its harness"}`;
    case "explode_shared_dir":
      return `Restore ${event.harness ?? "the harness"}'s whole-folder link`;
    case "distribute_from_shared":
      return `Move ${skillPart} back into the Universal folder and remove the per-harness copies`;
    case "make_independent_copy":
      return `Restore ${skillPart}'s exact Universal link`;
    case "move_aside_disable":
      return `Restore ${skillPart} to its original location`;
    case "move_copy_deployment":
      return `${event.reversal_label ?? "Reverse visibility"} ${skillPart}`;
    default:
      return `Undo "${kindLabel(event.kind)}" for ${skillPart}`;
  }
}

function scopeLabel(event: SkillEvent): string {
  if (event.scope === "global") return "Global";
  if (event.scope === "project") {
    return event.project_path ? `Project · ${event.project_path}` : "Project · path not recorded";
  }
  return "Scope not recorded";
}

function EventRow({ event, onRestored }: { event: SkillEvent; onRestored: () => void }) {
  const addToast = useAppStore((state) => state.addToast);
  const [isRestoring, setIsRestoring] = useState(false);
  const cancellation = useDocumentCancellation();
  const isCopyRepair = [
    "repair_copy_frontmatter",
    "undo_copy_frontmatter",
    "redo_copy_frontmatter",
  ].includes(event.kind);
  const isCopyEdit = ["edit_copy_document", "undo_copy_document", "redo_copy_document"].includes(
    event.kind,
  );
  const isCopyChange = isCopyRepair || isCopyEdit;
  const restoresTrialBackup = event.recovery_action === "restore_trial_backup";
  const restoreLabel = restoresTrialBackup
    ? "Restore to Global"
    : (event.reversal_label ??
      (isCopyEdit
        ? event.kind === "undo_copy_document"
          ? "Redo edit"
          : "Undo edit"
        : event.kind === "undo_copy_frontmatter"
          ? "Redo repair"
          : isCopyRepair
            ? "Undo repair"
            : "Restore"));
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
    cancellation.reset();
    setIsRestoring(true);
    try {
      if (restoresTrialBackup) {
        await restoreExpiredTrialBackup(event.id, cancellation.onStarted);
      } else {
        await restoreSkillEvent(event.id, force, isCopyChange ? cancellation.onStarted : undefined);
      }
      addToast({
        type: "success",
        title: event.kind === "undo_copy_frontmatter" ? "Repair reapplied" : "Restored",
        message: restoreDescription(event),
      });
    } catch (err) {
      const message = errorMessage(err);
      if (!restoresTrialBackup && !force && shouldOfferForceRestore(event, message)) {
        const proceed = await ask(
          `${message}\n\nRestoring anyway will back up the current content first, so it stays restorable.`,
          { title: "Content has changed", kind: "warning" },
        );
        if (proceed) {
          await runRestore(true);
          return;
        }
      } else {
        addToast({ type: "error", title: "Restore failed", message });
      }
    } finally {
      setIsRestoring(false);
      cancellation.reset();
      onRestored();
    }
  };

  const handleRestoreClick = async () => {
    const detail = restoresTrialBackup
      ? "The trial, Copy ownership, and original reader links remain removed. The backup is kept."
      : scopeLabel(event);
    const confirmed = await ask(`${restoreDescription(event)}?${detail ? `\n\n${detail}` : ""}`, {
      title: restoreLabel,
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
      <div className="min-w-0 flex-1">
        <div className="truncate text-body text-text-primary" title={event.skill}>
          {event.skill || (event.harness ?? kindLabel(event.kind))}
        </div>
        <div className="truncate text-small text-text-tertiary" title={scopeLabel(event)}>
          {scopeLabel(event)}
        </div>
      </div>
      <span className="text-small text-text-tertiary">
        {event.history_label ?? kindLabel(event.kind)}
      </span>
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
      {isRestoring && (isCopyChange || restoresTrialBackup) && (
        <button
          type="button"
          onClick={cancellation.cancel}
          disabled={!cancellation.canCancel || cancellation.isCancelling}
          className="shrink-0 rounded-sm border border-border-subtle px-2 py-1 text-small disabled:opacity-50"
        >
          {cancellation.isCancelling ? "Stopping…" : "Stop restore"}
        </button>
      )}
      {(canRestoreSkillEvent(event) || restoresTrialBackup) && (
        <button
          type="button"
          className="shrink-0 cursor-pointer rounded-sm border border-border-subtle bg-transparent px-2 py-1 text-small text-text-secondary transition-colors hover:bg-bg-hover disabled:opacity-50"
          onClick={handleRestoreClick}
          disabled={isRestoring}
        >
          {restoreLabel}
        </button>
      )}
    </div>
  );
}

/**
 * The Activity view's History section: every event store row, newest first.
 * Backend mutations emit a fresh snapshot after their event reaches its final state.
 */
type HistoryState =
  | { kind: "loading" }
  | { kind: "failed" }
  | { kind: "ready"; events: SkillEvent[]; refreshFailed: boolean };

export function SkillHistorySection({ snapshotRevision }: { snapshotRevision?: number }) {
  const [history, setHistory] = useState<HistoryState>({ kind: "loading" });
  const [requestedPage, setRequestedPage] = useState(0);
  const request = useRef(0);
  const mounted = useRef(false);
  const refresh = useCallback(() => {
    if (!mounted.current) return;
    const current = ++request.current;
    setHistory((previous) => (previous.kind === "failed" ? { kind: "loading" } : previous));
    listSkillEvents()
      .then((events) => {
        if (mounted.current && current === request.current) {
          setHistory({ kind: "ready", events, refreshFailed: false });
        }
      })
      .catch(() => {
        if (!mounted.current || current !== request.current) return;
        setHistory((previous) =>
          previous.kind === "ready" ? { ...previous, refreshFailed: true } : { kind: "failed" },
        );
      });
  }, []);

  useEffect(() => {
    mounted.current = true;
    invalidateSkillEventReads();
    refresh();
    return () => {
      mounted.current = false;
    };
    // oxlint-disable-next-line react/exhaustive-effect-dependencies -- Completion snapshots invalidate independently stored event rows.
  }, [refresh, snapshotRevision]);

  const events = history.kind === "ready" ? history.events : [];
  const pageCount = Math.ceil(events.length / HISTORY_PAGE_SIZE);
  const page = Math.min(requestedPage, Math.max(0, pageCount - 1));
  const pageStart = page * HISTORY_PAGE_SIZE;

  return (
    <div className="flex flex-col gap-3">
      <span className="text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
        History
      </span>
      {history.kind === "loading" ? (
        <p className="text-wrap-pretty text-body text-text-tertiary">Loading…</p>
      ) : history.kind === "failed" ? (
        <button
          type="button"
          onClick={refresh}
          className="text-left text-body text-text-tertiary underline"
        >
          History could not load. Retry
        </button>
      ) : events.length === 0 ? (
        <p className="text-wrap-pretty text-body text-text-tertiary">No events recorded yet.</p>
      ) : (
        <div className="flex flex-col">
          {events.slice(pageStart, pageStart + HISTORY_PAGE_SIZE).map((event) => (
            <EventRow key={event.id} event={event} onRestored={refresh} />
          ))}
        </div>
      )}
      {history.kind === "ready" && history.refreshFailed && (
        <button
          type="button"
          onClick={refresh}
          className="text-left text-small text-text-tertiary underline"
        >
          History could not refresh. Retry
        </button>
      )}
      {pageCount > 1 && (
        <nav aria-label="History pages" className="flex items-center justify-between gap-3">
          <button type="button" disabled={page === 0} onClick={() => setRequestedPage(page - 1)}>
            Newer events
          </button>
          <span className="text-small text-text-tertiary">
            {pageStart + 1}–{Math.min(pageStart + HISTORY_PAGE_SIZE, events.length)} of{" "}
            {events.length} events
          </span>
          <button
            type="button"
            disabled={page === pageCount - 1}
            onClick={() => setRequestedPage(page + 1)}
          >
            Older events
          </button>
        </nav>
      )}
    </div>
  );
}
