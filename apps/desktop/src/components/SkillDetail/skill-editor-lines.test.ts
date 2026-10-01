import { describe, expect, it } from "vitest";
import { lineRange, normalizeLineEndings } from "./skill-editor-lines";

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

  it("line_range_selects_the_right_text_in_a_crlf_file_once_normalized_or_drifts_one_char_per_line", () => {
    const crlf = "---\r\nname: a\r\n---\r\n";
    const lf = normalizeLineEndings(crlf);
    expect(lf).toBe(content);
    const range = lineRange(lf, 2);
    expect(range).toEqual({ start: 4, end: 11 });
    expect(lf.slice(range?.start, range?.end)).toBe("name: a");
  });
});
