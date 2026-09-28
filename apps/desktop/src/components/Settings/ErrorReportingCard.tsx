// ============================================================================
// ErrorReportingCard - Settings' "Telemetry" card: the switch that sends a
// sanitized panic to Sentry. Off in the registry by default; the welcome
// screen offers it on and writes the user's explicit choice here, and this
// card lets the user change that choice later - see the Rust
// `error_reporting` and `skill_studio_core::report_sanitizer` for what a
// report can and can't carry.
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
          title: "Couldn't read your Telemetry setting",
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
        title: "Couldn't save your Telemetry setting",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

  return (
    <SettingsCard
      icon={<Bug size={15} className="text-text-tertiary" />}
      title="Telemetry"
      description="When Skill Studio crashes, it sends only where in its own code the crash happened, never your skill names, files, or paths."
    >
      <label className="flex h-9 items-center gap-2 px-2 text-body text-text-secondary">
        <SwitchControl
          checked={enabled}
          onCheckedChange={toggle}
          disabled={isLoading}
          ariaLabel="Telemetry"
        />
        {enabled ? "On" : "Off"}
      </label>
    </SettingsCard>
  );
}
