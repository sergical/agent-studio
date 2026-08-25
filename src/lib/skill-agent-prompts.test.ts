// ============================================================================
// Skill Studio - skill-agent-prompts tests
// ============================================================================

import { describe, expect, it } from "vitest";
import {
  extractProposedSkillMd,
  fenceForMarkdown,
  normalizeProposalToOriginal,
  parseJudgeVerdict,
} from "./skill-agent-prompts";

const HEADING = "## Proposed SKILL.md";

describe("fenceForMarkdown", () => {
  it("returns 4 backticks for text with no fences", () => {
    expect(fenceForMarkdown("plain text, no fences")).toBe("````");
  });

  it("returns one more backtick than the longest backtick run", () => {
    expect(fenceForMarkdown("some `````` run of five backticks")).toBe("`".repeat(7));
  });

  it("ignores tilde runs when counting backticks", () => {
    expect(fenceForMarkdown("a ~~~~~~ tilde fence")).toBe("````");
  });
});

describe("extractProposedSkillMd", () => {
  it("extracts a proposal whose body contains a shorter ``` fence inside a longer outer fence", () => {
    const inner = "```js\nconsole.log(1);\n```";
    const finalText = `${HEADING}\n\n\`\`\`\`markdown\n---\nname: foo\n---\n\n${inner}\n\`\`\`\``;
    expect(extractProposedSkillMd(finalText)).toBe(`---\nname: foo\n---\n\n${inner}`);
  });

  it("extracts a proposal wrapped in ~~~ fences", () => {
    const finalText = `${HEADING}\n\n~~~markdown\n---\nname: foo\n---\nbody\n~~~`;
    expect(extractProposedSkillMd(finalText)).toBe("---\nname: foo\n---\nbody");
  });

  it("accepts the `md` info-string tag", () => {
    const finalText = `${HEADING}\n\n\`\`\`\`md\ncontent\n\`\`\`\``;
    expect(extractProposedSkillMd(finalText)).toBe("content");
  });

  it("handles CRLF input", () => {
    const finalText = `${HEADING}\r\n\r\n\`\`\`\`markdown\r\nline one\r\nline two\r\n\`\`\`\``;
    expect(extractProposedSkillMd(finalText)).toBe("line one\nline two");
  });

  it("returns null when the heading is missing", () => {
    expect(extractProposedSkillMd("no heading here\n\n```markdown\ncontent\n```")).toBeNull();
  });

  it("returns null when the fence is never closed", () => {
    const finalText = `${HEADING}\n\n\`\`\`\`markdown\nno closer here`;
    expect(extractProposedSkillMd(finalText)).toBeNull();
  });
});

describe("normalizeProposalToOriginal", () => {
  it("converts an LF proposal to CRLF when the original uses CRLF", () => {
    const original = "line one\r\nline two\r\n";
    const proposal = "line one\nline two\n";
    expect(normalizeProposalToOriginal(proposal, original)).toBe("line one\r\nline two\r\n");
  });

  it("adds a trailing newline when the original has one", () => {
    const original = "line one\nline two\n";
    const proposal = "line one\nline two";
    expect(normalizeProposalToOriginal(proposal, original)).toBe("line one\nline two\n");
  });

  it("strips a trailing newline when the original has none", () => {
    const original = "line one\nline two";
    const proposal = "line one\nline two\n\n";
    expect(normalizeProposalToOriginal(proposal, original)).toBe("line one\nline two");
  });

  it("round-trips an identical file to zero hunks via diffSkillMd", async () => {
    const { diffSkillMd } = await import("./skill-md-diff");
    const original = "line one\r\nline two\r\n";
    const normalized = normalizeProposalToOriginal("line one\nline two\n", original);
    expect(diffSkillMd(original, normalized)).toEqual([]);
  });
});

describe("parseJudgeVerdict", () => {
  it("parses a PASS verdict and its sentence", () => {
    expect(parseJudgeVerdict("PASS\nIt did the thing.")).toEqual({
      passed: true,
      sentence: "It did the thing.",
    });
  });

  it("parses a FAIL verdict and its sentence", () => {
    expect(parseJudgeVerdict("FAIL\nIt never called the tool.")).toEqual({
      passed: false,
      sentence: "It never called the tool.",
    });
  });

  it("is case-insensitive on the verdict line", () => {
    expect(parseJudgeVerdict("pass\nGood.")).toEqual({ passed: true, sentence: "Good." });
  });

  it("skips leading and trailing blank lines", () => {
    expect(parseJudgeVerdict("\n\nPASS\n\nGood.\n\n")).toEqual({
      passed: true,
      sentence: "Good.",
    });
  });

  it("returns null when the first line isn't PASS or FAIL", () => {
    expect(parseJudgeVerdict("Maybe\nUnclear.")).toBeNull();
  });

  it("returns null when there's no sentence line", () => {
    expect(parseJudgeVerdict("PASS")).toBeNull();
  });
});
