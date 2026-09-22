// ============================================================================
// AppVersionCard - Settings' "Version" card: the running app's version and
// build commit, and a "What's new" toggle for the current release's
// changelog notes, bundled into the app at build time so it needs no
// network.
// ============================================================================

import { useEffect, useState } from "react";
import { Info } from "lucide-react";
import { Button } from "@skill-studio/ui";
import type { AppVersion, UpdateStatus } from "@skill-studio/lib";
import {
  appVersion,
  checkForUpdate,
  getUpdateStatus,
  installUpdate,
  invokeErrorMessage,
  onUpdateStatus,
} from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import { SettingsCard } from "./SettingsCard";

/** Settings' own label for each `UpdateStatus` - kept here rather than in `skill-types.ts` since
 * it's UI copy, not part of the wire shape. */
function updateStatusLabel(status: UpdateStatus): string {
  switch (status.status) {
    case "up-to-date":
      return "Up to date";
    case "checking":
      return "Checking for updates…";
    case "downloading":
      return `Downloading v${status.version}…`;
    case "ready-to-install":
      return `v${status.version} ready to install`;
    case "check-failed":
      return `Couldn't check for updates: ${status.message}`;
    case "error":
      return status.message;
  }
}

/** The colour for each `UpdateStatus`: the manual click's hard `Error` and a
 * failed install stay red, a background check that could not reach the
 * endpoint is a warning (visible, but not the user's fault), and the rest
 * are neutral. */
function updateStatusTone(status: UpdateStatus): string {
  switch (status.status) {
    case "error":
      return "text-error";
    case "check-failed":
      return "text-warning";
    default:
      return "text-text-secondary";
  }
}

export function AppVersionCard() {
  const addToast = useAppStore((state) => state.addToast);
  const [info, setInfo] = useState<AppVersion | null>(null);
  const [showNotes, setShowNotes] = useState(false);
  const [updateStatus, setUpdateStatus] = useState<UpdateStatus>({ status: "up-to-date" });
  const [installing, setInstalling] = useState(false);

  useEffect(() => {
    let cancelled = false;
    appVersion()
      .then((result) => {
        if (!cancelled) setInfo(result);
      })
      .catch((err) => {
        addToast({
          type: "error",
          title: "Couldn't read the app version",
          message: invokeErrorMessage(err),
        });
      });
    return () => {
      cancelled = true;
    };
  }, [addToast]);

  useEffect(() => {
    let cancelled = false;
    getUpdateStatus().then((result) => {
      if (!cancelled) setUpdateStatus(result);
    });
    const unlisten = onUpdateStatus((status) => {
      if (!cancelled) setUpdateStatus(status);
    });
    return () => {
      cancelled = true;
      unlisten.then((fn) => fn());
    };
  }, []);

  const handleCheckForUpdate = () => {
    checkForUpdate().catch((err) => {
      addToast({
        type: "error",
        title: "Couldn't check for updates",
        message: invokeErrorMessage(err),
      });
    });
  };

  const handleRestartToUpdate = () => {
    setInstalling(true);
    installUpdate().catch((err) => {
      setInstalling(false);
      addToast({
        type: "error",
        title: "Couldn't install the update",
        message: invokeErrorMessage(err),
      });
    });
  };

  if (!info) return null;

  const shortCommit = info.commit === "dev" ? "dev" : info.commit.slice(0, 7);
  const checking = updateStatus.status === "checking" || updateStatus.status === "downloading";

  return (
    <SettingsCard
      icon={<Info size={15} className="text-text-tertiary" />}
      title="Version"
      description="The version of Skill Studio you're running, and what changed in it."
    >
      <div className="flex items-center gap-2">
        <p className="m-0 flex-1 text-body text-text-secondary">
          v{info.version} ({shortCommit})
        </p>
        {info.notes && (
          <Button variant="ghost" onClick={() => setShowNotes((open) => !open)}>
            {showNotes ? "Hide what's new" : "What's new"}
          </Button>
        )}
      </div>
      {showNotes && info.notes && (
        <div className="whitespace-pre-wrap rounded-md bg-bg-secondary p-3 text-small text-text-secondary">
          {info.notes}
        </div>
      )}
      <div className="flex items-center gap-2">
        <p className={`m-0 flex-1 text-body ${updateStatusTone(updateStatus)}`}>
          {checking && (
            <span className="mr-2 inline-block size-3 animate-spin rounded-full border-2 border-current border-t-transparent align-middle" />
          )}
          {updateStatusLabel(updateStatus)}
        </p>
        {updateStatus.status === "ready-to-install" ? (
          <Button variant="secondary" onClick={handleRestartToUpdate} disabled={installing}>
            {installing ? "Restarting…" : "Restart to update"}
          </Button>
        ) : (
          <Button variant="ghost" onClick={handleCheckForUpdate} disabled={checking}>
            Check for updates
          </Button>
        )}
      </div>
    </SettingsCard>
  );
}
