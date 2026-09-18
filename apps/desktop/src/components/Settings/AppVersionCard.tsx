// ============================================================================
// AppVersionCard - Settings' "Version" card: the running app's version and
// build commit, and a "What's new" toggle for the current release's
// changelog notes, bundled into the app at build time so it needs no
// network.
// ============================================================================

import { useEffect, useState } from "react";
import { Info } from "lucide-react";
import { Button } from "@skill-studio/ui";
import type { AppVersion } from "@skill-studio/lib";
import { appVersion, invokeErrorMessage } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import { SettingsCard } from "./SettingsCard";

export function AppVersionCard() {
  const addToast = useAppStore((state) => state.addToast);
  const [info, setInfo] = useState<AppVersion | null>(null);
  const [showNotes, setShowNotes] = useState(false);

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

  if (!info) return null;

  const shortCommit = info.commit === "dev" ? "dev" : info.commit.slice(0, 7);

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
    </SettingsCard>
  );
}
