import { describe, expect, it } from "vitest";
import { rowClickOpensSkill } from "./home-row-click";

/** A fake element: `inRow` says whether the row's DOM holds it, `control` whether it sits inside a button. */
function target(inRow: boolean, control: boolean) {
  return { inRow, closest: () => (control ? {} : null) };
}

const row = { contains: (node: { inRow: boolean }) => node.inRow };

describe("rowClickOpensSkill", () => {
  it("a_click_on_the_row_text_opens_the_skill_or_rows_stop_navigating", () => {
    // SAFETY: rowClickOpensSkill reads only `contains` and `closest`.
    expect(rowClickOpensSkill(row as never, target(true, false) as never)).toBe(true);
  });

  it("a_click_on_a_row_button_stays_on_home_or_pull_latest_jumps_into_the_skill", () => {
    // SAFETY: rowClickOpensSkill reads only `contains` and `closest`.
    expect(rowClickOpensSkill(row as never, target(true, true) as never)).toBe(false);
  });

  it("a_click_inside_a_portaled_dialog_stays_on_home_or_confirming_the_update_warning_navigates", () => {
    // SAFETY: rowClickOpensSkill reads only `contains` and `closest`.
    expect(rowClickOpensSkill(row as never, target(false, false) as never)).toBe(false);
  });
});
