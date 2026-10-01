import { describe, expect, it } from "vitest";
import { rankItems, type PaletteItem } from "./command-palette-items";

function skillItem(label: string): PaletteItem {
  return { id: `skill:${label}`, section: "skills", label, run: () => {} };
}

describe("rankItems", () => {
  const items = ["ask-emil", "i-have-adhd", "find-bugs", "have-fun"].map(skillItem);
  const labels = (query: string) => rankItems(items, query).map((item) => item.label);

  it("finds a hyphenated skill name typed with spaces, ranked as a prefix match", () => {
    expect(labels("i have")).toEqual(["i-have-adhd"]);
  });

  it("matches words in any order after the in-order matches", () => {
    expect(labels("have")).toEqual(["have-fun", "i-have-adhd"]);
    expect(labels("adhd have")).toEqual(["i-have-adhd"]);
  });
});
