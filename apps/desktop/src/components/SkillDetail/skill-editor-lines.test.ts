import { describe, expect, it } from "vitest";
import { contentForSave, isContentDirty, lineRange } from "./skill-editor-lines";

describe("lineRange", () => {
  const content = "---\nname: a\n---\n";

  it("line_range_selects_the_requested_line_without_its_newline_or_jumps_to_the_wrong_row", () => {
    expect(lineRange(content, 1)).toEqual({ start: 0, end: 3 });
    expect(lineRange(content, 2)).toEqual({ start: 4, end: 11 });
    expect(content.slice(4, 11)).toBe("name: a");
  });

  it("line_range_covers_the_last_line_when_the_text_has_no_trailing_newline_or_drops_it", () => {
    expect(lineRange("a\nbc", 2)).toEqual({ start: 2, end: 4 });
  });

  it("line_range_returns_null_for_a_line_the_text_does_not_have_or_selects_garbage", () => {
    expect(lineRange("a\nb", 3)).toBeNull();
    expect(lineRange("a", 0)).toBeNull();
    expect(lineRange("a", 1.5)).toBeNull();
  });

  it("line_range_gives_an_empty_range_for_a_trailing_blank_line_or_overshoots", () => {
    expect(lineRange("a\n", 2)).toEqual({ start: 2, end: 2 });
  });

  it("line_range_selects_the_right_text_in_a_crlf_file_or_drifts_one_char_per_line", () => {
    const crlf = "---\r\nname: a\r\n---\r\n";
    expect(lineRange(crlf, 2)).toEqual({ start: 4, end: 11 });
    expect(lineRange(crlf, 4)).toEqual({ start: 16, end: 16 });
  });
});

describe("isContentDirty", () => {
  it("dirty_compare_treats_a_crlf_file_as_clean_after_an_edit_is_undone_or_stays_dirty", () => {
    const crlf = "a\r\nb\r\n";
    expect(isContentDirty("a\nb\n", crlf)).toBe(false);
    expect(isContentDirty("a\nb\nx", crlf)).toBe(true);
  });
});

describe("contentForSave", () => {
  it("save_content_restores_crlf_for_a_crlf_file_or_rewrites_it_as_lf", () => {
    expect(contentForSave("a\nb\n", "x\r\ny\r\n")).toBe("a\r\nb\r\n");
  });

  it("save_content_leaves_an_lf_file_untouched_or_adds_carriage_returns", () => {
    expect(contentForSave("a\nb\n", "x\ny\n")).toBe("a\nb\n");
  });
});
