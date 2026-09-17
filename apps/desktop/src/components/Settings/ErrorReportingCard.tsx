// ============================================================================
// ErrorReportingCard - Settings' "Error reporting" card: the opt-in switch
// that sends a sanitized panic or command failure to Sentry. Off by default -
// see the Rust `error_reporting` and `skill_studio_core::report_sanitizer`
// for what a report can and can't carry.
// ============================================================================

import { useEffect, useState } from "react";
import { Bug } from "lucide-react";
import { getErrorReportingEnabled, setErrorReportingEnabled } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import { SwitchControl } from "../ui/SwitchControl";
import { SettingsCard } from "./SettingsCard";

export function ErrorReportingCard() {
  const addToast = useAppStore((state) => state.addToast);
  const [enabled, setEnabled] = useState(false);
  const [isLoading, setIsLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    getErrorReportingEnabled()
      .then((result) => {
        if (!cancelled) setEnabled(result);
      })
      .catch((err) => {
        addToast({
          type: "error",
          title: "Couldn't read your error reporting setting",
          message: err instanceof Error ? err.message : "Unknown error",
        });
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [addToast]);

  const toggle = async (next: boolean) => {
    const previous = enabled;
    setEnabled(next);
    try {
      await setErrorReportingEnabled(next);
    } catch (err) {
      setEnabled(previous);
      addToast({
        type: "error",
        title: "Couldn't save your error reporting setting",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

  return (
    <SettingsCard
      icon={<Bug size={15} className="text-text-tertiary" />}
      title="Error reporting"
      description="Send a sanitized crash or a failed action to us. Off by default; never your skill names, file contents, or paths."
    >
      <label className="flex h-9 items-center gap-2 px-2 text-body text-text-secondary">
        <SwitchControl
          checked={enabled}
          onCheckedChange={toggle}
          disabled={isLoading}
          ariaLabel="Error reporting"
        />
        {enabled ? "On" : "Off"}
      </label>
    </SettingsCard>
  );
}
