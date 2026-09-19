// ============================================================================
// doctor-violation-keys - stable React keys for a `DoctorReport`'s
// violations list. `invariant:path` alone collides whenever two violations
// name the same invariant and path (two open journal plans rooted at the
// same directory, two registry entries naming one stale target), so each
// key gets a per-duplicate occurrence suffix instead of falling back to the
// array index react-doctor's `no-array-index-as-key` rule flags.
// ============================================================================

import type { DoctorViolation } from "@skill-studio/lib";

/** Pairs each violation with the React key `DoctorCard` renders it under. */
export function keyDoctorViolations(
  violations: DoctorViolation[],
): { key: string; violation: DoctorViolation }[] {
  const seen = new Map<string, number>();
  return violations.map((violation) => {
    const base = `${violation.invariant}:${violation.path}`;
    const occurrence = (seen.get(base) ?? 0) + 1;
    seen.set(base, occurrence);
    return { key: occurrence === 1 ? base : `${base}#${occurrence}`, violation };
  });
}
