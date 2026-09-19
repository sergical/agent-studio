import { describe, expect, it } from "vitest";
import { keyDoctorViolations } from "./doctor-violation-keys";
import type { DoctorViolation } from "@skill-studio/lib";

function violation(
  invariant: DoctorViolation["invariant"],
  path: string,
  detail = "",
): DoctorViolation {
  return { invariant, path, detail };
}

describe("keyDoctorViolations", () => {
  it("keys a single violation by its invariant and path", () => {
    const keyed = keyDoctorViolations([violation("quarantine_within_cap", "/home/.quarantine")]);

    expect(keyed.map((k) => k.key)).toEqual(["quarantine_within_cap:/home/.quarantine"]);
  });

  it("gives two violations sharing an invariant and path distinct keys, or names the collision", () => {
    const duplicate = violation("journal_has_no_open_plan", "/home/.agents/skills");
    const keyed = keyDoctorViolations([duplicate, duplicate]);

    const keys = keyed.map((k) => k.key);
    expect(new Set(keys).size).toBe(2);
    expect(keys).toEqual([
      "journal_has_no_open_plan:/home/.agents/skills",
      "journal_has_no_open_plan:/home/.agents/skills#2",
    ]);
  });

  it("keeps every violation paired with its own data after keying", () => {
    const first = violation("registry_entry_has_folder", "/home/a", "first");
    const second = violation("registry_entry_has_folder", "/home/a", "second");

    const keyed = keyDoctorViolations([first, second]);

    expect(keyed.map((k) => k.violation.detail)).toEqual(["first", "second"]);
  });
});
