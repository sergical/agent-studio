// ============================================================================
// TelemetryCard - Settings' "Telemetry" card: the switch for crash reports,
// operation timings, and WebView errors. Off in the registry by default; the
// welcome screen offers it on and writes the user's explicit choice; this
// card lets the user change it later - see the Rust `telemetry_commands` and
// `crates/skill-studio-host/src/telemetry.rs` for what a report can and
// can't carry.
// ============================================================================

import { useEffect, useState } from "react";
import { Bug } from "lucide-react";
import { getTelemetryEnabled, setTelemetryEnabled } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import { SwitchControl } from "../ui/SwitchControl";
import { SettingsCard } from "./SettingsCard";

export function TelemetryCard() {
  const addToast = useAppStore((state) => state.addToast);
  const [enabled, setEnabled] = useState(false);
  const [isLoading, setIsLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    getTelemetryEnabled()
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
      await setTelemetryEnabled(next);
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
      description="Sends where in its own code a crash happened and how long each action took. Skill Studio removes everything sensitive first: no skill names, files, paths, or machine name. It uses this data only to improve the app."
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
