// ============================================================================
// DoctorCard - Settings' "Doctor" card: on-demand or startup results from
// `ops::doctor`, the pass over all six lifecycle invariants
// (`docs/action-map/lifecycle-states.md`) - a link that doesn't resolve, a
// registry or lockfile entry with no folder, a skill parked and deployed at
// once, the quarantine directory over its cap, or a journal plan left open
// by a crash.
// ============================================================================

import { useEffect } from "react";
import { Stethoscope } from "lucide-react";
import { Button } from "@skill-studio/ui";
import type { DoctorViolation } from "@skill-studio/lib";
import { useDoctor } from "../../hooks/useDoctor";
import { useAppStore } from "../../store/appStore";
import { keyDoctorViolations } from "./doctor-violation-keys";
import { SettingsCard } from "./SettingsCard";

function ViolationRow({ violation }: { violation: DoctorViolation }) {
  return (
    <div className="flex flex-col gap-0.5 border-b border-border-subtle px-2 py-1.5 text-body text-text-secondary last:border-b-0">
      <span className="text-text-primary">{violation.detail}</span>
      <span className="truncate text-small text-text-tertiary">{violation.path}</span>
    </div>
  );
}

export function DoctorCard() {
  const addToast = useAppStore((state) => state.addToast);
  const { report, error, running, run } = useDoctor();

  useEffect(() => {
    if (error !== null) {
      addToast({ type: "error", title: "Doctor pass failed", message: error });
    }
  }, [error, addToast]);

  return (
    <SettingsCard
      icon={<Stethoscope size={15} className="text-text-tertiary" />}
      title="Doctor"
      description="Check every lifecycle invariant: broken links, orphaned registry or lockfile entries, skills parked and deployed at once, an over-cap quarantine, or a journal plan left open by a crash."
      action={
        <Button variant="secondary" size="sm" onClick={run} disabled={running}>
          {running ? "Running…" : "Run doctor"}
        </Button>
      }
    >
      {error !== null ? (
        <p className="m-0 text-small text-text-tertiary">Couldn't run doctor: {error}</p>
      ) : report === null ? (
        <p className="m-0 text-small text-text-tertiary">Not run yet.</p>
      ) : report.violations.length === 0 ? (
        <p className="m-0 text-small text-text-tertiary">
          No issues found across {report.checked} skill{report.checked === 1 ? "" : "s"}.
        </p>
      ) : (
        <div className="flex flex-col">
          {keyDoctorViolations(report.violations).map(({ key, violation }) => (
            <ViolationRow key={key} violation={violation} />
          ))}
        </div>
      )}
    </SettingsCard>
  );
}
