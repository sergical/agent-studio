// ============================================================================
// UninstallPluginDialog - the confirm step for a Locations row's
// "Uninstall the <name> plugin…" action. Uninstall moves every skill the
// plugin ships, not just the one whose card is open, so the body says so.
// ============================================================================

import { useState } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@skill-studio/ui";
import { uninstallPlugin } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import type { Deployment } from "@skill-studio/lib";

export function UninstallPluginDialog({
  deployment,
  onClose,
}: {
  /** The plugin deployment row the action was triggered from - carries `plugin.name`/`id`. */
  deployment: Deployment;
  onClose: () => void;
}) {
  const [isRemoving, setIsRemoving] = useState(false);
  const addToast = useAppStore((state) => state.addToast);
  const pluginName = deployment.plugin?.name ?? "plugin";

  const handleUninstall = () => {
    if (!deployment.plugin) return;
    setIsRemoving(true);
    uninstallPlugin(deployment.plugin.id, deployment.agent)
      .then(() => onClose())
      .catch((err) => {
        addToast({
          type: "error",
          title: "Couldn't uninstall",
          message: err instanceof Error ? err.message : "Unknown error",
        });
      })
      .finally(() => setIsRemoving(false));
  };

  return (
    <AlertDialog open onOpenChange={(open) => !open && onClose()}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Uninstall the {pluginName} plugin?</AlertDialogTitle>
          <AlertDialogDescription>
            This removes the {pluginName} plugin and all its skills from Claude Code. Claude Code
            deletes the cached files later.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel onClick={onClose} disabled={isRemoving}>
            Cancel
          </AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            onClick={handleUninstall}
            disabled={isRemoving || !deployment.plugin}
          >
            {isRemoving ? "Uninstalling…" : "Uninstall"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
