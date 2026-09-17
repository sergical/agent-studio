// ============================================================================
// CommandHealthCard - Settings' "Command health" card: the last 7 days of
// `timing.jsonl`, folded by `crates/skill-studio-core/src/health.rs` into one
// row per command - how often it ran, how often it failed, and how long it
// took (p50/p95).
// ============================================================================

import { useEffect, useState } from "react";
import { Activity } from "lucide-react";
import type { CommandHealth } from "@skill-studio/lib";
import { commandHealth } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import { SettingsCard } from "./SettingsCard";

const COLUMNS = ["Command", "Count", "Failures", "p50 ms", "p95 ms", "Last error"] as const;

function HealthRow({ row }: { row: CommandHealth }) {
  return (
    <div className="grid grid-cols-[2fr_1fr_1fr_1fr_1fr_2fr] items-center gap-2 px-2 py-1.5 text-body text-text-secondary">
      <span className="truncate text-text-primary">{row.command}</span>
      <span className="tabular-nums">{row.count}</span>
      <span className="tabular-nums">{row.failures}</span>
      <span className="tabular-nums">{row.p50_ms}</span>
      <span className="tabular-nums">{row.p95_ms}</span>
      <span className="truncate text-text-tertiary">{row.last_error ?? "–"}</span>
    </div>
  );
}

export function CommandHealthCard() {
  const addToast = useAppStore((state) => state.addToast);
  const [rows, setRows] = useState<CommandHealth[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    commandHealth()
      .then((result) => {
        if (!cancelled) setRows(result);
      })
      .catch((err) => {
        addToast({
          type: "error",
          title: "Couldn't read command health",
          message: err instanceof Error ? err.message : "Unknown error",
        });
      });
    return () => {
      cancelled = true;
    };
  }, [addToast]);

  return (
    <SettingsCard
      icon={<Activity size={15} className="text-text-tertiary" />}
      title="Command health"
      description="How often each command ran, failed, and how long it took, over the last 7 days."
    >
      {rows === null ? (
        <p className="m-0 text-small text-text-tertiary">Loading…</p>
      ) : rows.length === 0 ? (
        <p className="m-0 text-small text-text-tertiary">
          No commands recorded in the last 7 days.
        </p>
      ) : (
        <div className="flex flex-col">
          <div className="grid grid-cols-[2fr_1fr_1fr_1fr_1fr_2fr] gap-2 border-b border-border-subtle px-2 pb-1.5 text-small font-medium text-text-tertiary">
            {COLUMNS.map((column) => (
              <span key={column}>{column}</span>
            ))}
          </div>
          {rows.map((row) => (
            <HealthRow key={row.command} row={row} />
          ))}
        </div>
      )}
    </SettingsCard>
  );
}
