import { describe, expect, it } from "vitest";
import {
  describeFrontmatterRepair,
  parseYamlFrontmatterError,
  proposeFrontmatterQuoteRepair,
} from "./skill-frontmatter-repair";

const DESCRIPTION =
  "Fix a reported issue. Handles the full workflow: fetching the issue, finding the reproduction";
const FILE = `---\nname: fix-issue\ndescription: ${DESCRIPTION}\ndisable-model-invocation: true\n---\nBody.\n`;

/** JSON strings are valid YAML double-quoted scalars, so this is the value a YAML parser would read. */
function unquote(quoted: string): string {
  return quoted.slice(1, -1).replace(/\\(["\\])/g, "$1");
}

describe("proposeFrontmatterQuoteRepair", () => {
  /**
   * Flow: the user opens a skill whose description holds "workflow: fetching".
   * Expect: line 3 (file-relative, as the Rust message counts) is quoted and only that line changes.
   * Failure: the Quote button would write a file that still fails to parse, or edit other lines.
   */
  it("quotes a description with a colon and a space, keeping its value", () => {
    const repair = proposeFrontmatterQuoteRepair(FILE, 3);
    expect(repair).not.toBeNull();
    expect(repair?.key).toBe("description");
    expect(repair?.line).toBe(3);
    expect(repair?.before).toBe(`description: ${DESCRIPTION}`);
    expect(unquote(repair!.after.slice("description: ".length))).toBe(DESCRIPTION);
    expect(repair?.fixedContent).toBe(FILE.replace(repair!.before, repair!.after));
  });

  /**
   * Flow: the value holds double quotes and backslashes.
   * Expect: both are escaped, so the unquoted value equals the original.
   * Failure: the quoted scalar ends early or changes the text.
   */
  it("escapes backslashes and double quotes", () => {
    const value = 'Use "quotes" and C:\\dir: now';
    const repair = proposeFrontmatterQuoteRepair(`---\nname: a\ndescription: ${value}\n---\n`, 3);
    expect(repair).not.toBeNull();
    expect(unquote(repair!.after.slice("description: ".length))).toBe(value);
  });

  /**
   * Flow: the value has a space then "#", which YAML reads as a comment start.
   * Expect: the whole value is quoted so the "#" stays text.
   * Failure: the description is cut at the "#" after the repair.
   */
  it("quotes a value with a space before a hash", () => {
    const repair = proposeFrontmatterQuoteRepair(
      "---\nname: a\ndescription: Fix bug #12 today\n---\n",
      3,
    );
    expect(repair?.after).toBe('description: "Fix bug #12 today"');
  });

  /**
   * Flow: the value starts with a YAML indicator such as "*".
   * Expect: the value is quoted.
   * Failure: the file keeps a value YAML reads as an alias.
   */
  it("quotes a value that starts with an indicator character", () => {
    const repair = proposeFrontmatterQuoteRepair(
      "---\nname: a\ndescription: *bold* text\n---\n",
      3,
    );
    expect(repair?.after).toBe('description: "*bold* text"');
  });

  /**
   * Flow: the value is already quoted.
   * Expect: no repair.
   * Failure: the helper double-quotes a value and changes its text.
   */
  it("returns null for an already quoted value", () => {
    expect(
      proposeFrontmatterQuoteRepair('---\nname: a\ndescription: "Handles: things"\n---\n', 3),
    ).toBeNull();
  });

  /**
   * Flow: the value is a multi-line block scalar.
   * Expect: no repair.
   * Failure: the quotes would break the indented lines below the key.
   */
  it("returns null for a block scalar", () => {
    const content = "---\nname: a\ndescription: |\n  Handles: things\n  more\n---\n";
    expect(proposeFrontmatterQuoteRepair(content, 3)).toBeNull();
    expect(proposeFrontmatterQuoteRepair(content, 4)).toBeNull();
  });

  /**
   * Flow: a plain value continues on an indented next line.
   * Expect: no repair.
   * Failure: the helper quotes one line of a multi-line value.
   */
  it("returns null for a multi-line plain value", () => {
    expect(
      proposeFrontmatterQuoteRepair("---\nname: a\ndescription: Does: x\n  and y\n---\n", 3),
    ).toBeNull();
  });

  /**
   * Flow: the error line is a nested key, a comment, a body line, or out of range.
   * Expect: no repair.
   * Failure: the helper edits text that is not a top-level frontmatter field.
   */
  it("returns null when the error line is not a top-level frontmatter key", () => {
    const content = "---\nname: a\nmetadata:\n  note: a: b\n# c: d: e\n---\nBody: a: b\n";
    expect(proposeFrontmatterQuoteRepair(content, 4)).toBeNull();
    expect(proposeFrontmatterQuoteRepair(content, 5)).toBeNull();
    expect(proposeFrontmatterQuoteRepair(content, 7)).toBeNull();
    expect(proposeFrontmatterQuoteRepair(content, 1)).toBeNull();
    expect(proposeFrontmatterQuoteRepair(content, 99)).toBeNull();
  });

  /**
   * Flow: the value is plain and has no YAML trigger.
   * Expect: no repair, since quoting would not fix the reported error.
   * Failure: the button appears for an error it cannot fix.
   */
  it("returns null for a value with no trigger", () => {
    expect(
      proposeFrontmatterQuoteRepair("---\nname: a\ndescription: fine text\n---\n", 3),
    ).toBeNull();
  });

  /**
   * Flow: the file uses CRLF line endings.
   * Expect: the repair keeps CRLF on every line.
   * Failure: the write silently converts the whole file to LF.
   */
  it("keeps CRLF line endings", () => {
    const repair = proposeFrontmatterQuoteRepair(
      "---\r\nname: a\r\ndescription: x: y\r\n---\r\n",
      3,
    );
    expect(repair?.fixedContent).toBe('---\r\nname: a\r\ndescription: "x: y"\r\n---\r\n');
  });
});

describe("parseYamlFrontmatterError", () => {
  it("reads line and column from the Rust message", () => {
    expect(
      parseYamlFrontmatterError(
        "invalid YAML frontmatter at line 3, column 205: mapping values are not allowed in this context",
      ),
    ).toEqual({ line: 3, column: 205 });
    expect(parseYamlFrontmatterError("missing required frontmatter field: name")).toBeNull();
  });
});

describe("describeFrontmatterRepair", () => {
  /**
   * Flow: the page explains the example error.
   * Expect: it names the key, the line, and the fragment around the column.
   * Failure: the user sees no hint about what to edit.
   */
  it("names the colon fragment around the column", () => {
    const repair = proposeFrontmatterQuoteRepair(FILE, 3)!;
    const column = repair.before.indexOf(": fetching") + 2;
    const text = describeFrontmatterRepair(repair, column);
    expect(text).toContain("description on line 3");
    expect(text).toContain("colon followed by a space");
    expect(text).toContain("workflow: fetching");
  });
});
