// ============================================================================
// Skill Studio - theme token stylesheet guard
// Ensures no custom property in the app or kit theme sheets resolves to
// itself. Tailwind's @theme registry is keyed by name, so a bridge line like
// `--color-border: var(--color-border)` silently replaces the app's token
// with a direct cycle and the utility becomes invalid at runtime.
// ============================================================================

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const here = dirname(fileURLToPath(import.meta.url));

const THEME_SHEETS = [
  resolve(here, "../../../apps/desktop/src/App.css"),
  resolve(here, "../../ui/src/styles.css"),
];

const SELF_REFERENCE = /(--[\w-]+)\s*:\s*var\(\s*\1\s*[,)]/g;

describe("theme token sheets", () => {
  for (const sheet of THEME_SHEETS) {
    it(`${sheet.split("/").slice(-2).join("/")} has no self-referencing custom properties`, () => {
      const css = readFileSync(sheet, "utf8");
      const cycles = [...css.matchAll(SELF_REFERENCE)].map((m) => m[0]);
      expect(cycles).toEqual([]);
    });
  }
});
