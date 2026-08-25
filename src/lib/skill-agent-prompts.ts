// ============================================================================
// Skill Studio - skill-agent-prompts
// Builds the "Audit" prompt sent to a headless harness run for one skill, and
// pulls the harness's proposed SKILL.md rewrite back out of its final text
// ============================================================================

import type { AgentId } from "./skill-types";

/** Input for `buildSkillAuditPrompt`. */
export interface SkillAuditPromptInput {
  skillName: string;
  skillMd: string;
  harness: AgentId;
  deployments: string[];
}

/** Heading the proposed rewrite, if any, follows in the harness's final text. */
const PROPOSED_HEADING = "## Proposed SKILL.md";

/** Matches every run of 3+ backticks or 3+ tildes in `text`. */
const FENCE_RUN_RE = /`{3,}|~{3,}/g;

/**
 * Returns a run of backticks one longer than the longest backtick run found
 * in `text`, minimum 4 - long enough that the caller can safely wrap `text`
 * in a fence without it being closed early by a fence already inside it.
 */
export function fenceForMarkdown(text: string): string {
  let longest = 0;
  for (const run of text.match(FENCE_RUN_RE) ?? []) {
    if (run[0] === "`" && run.length > longest) longest = run.length;
  }
  return "`".repeat(Math.max(longest + 1, 4));
}

/**
 * Builds the prompt for the "Audit" action: reviews `input.skillMd` against
 * the agentskills.io spec rules and asks for a findings list, a verdict, and
 * (only when worth it) a complete revised SKILL.md.
 */
export function buildSkillAuditPrompt(input: SkillAuditPromptInput): string {
  const deploymentsLine =
    input.deployments.length > 0 ? input.deployments.join(", ") : "no other deployments";
  const fence = fenceForMarkdown(input.skillMd);
  return `You are reviewing an agent skill (a SKILL.md file) for Skill Studio.

The skill is "${input.skillName}", running under ${input.harness} in this review, and otherwise deployed to: ${deploymentsLine}.

Judge it against two kinds of rules:

Specification rules (report as [spec]) - enforced by Skill Studio's validator, a violation blocks the skill from being marked spec-compliant:
- Frontmatter \`name\` is 1-64 characters, lowercase a-z0-9 and hyphens only, with no leading, trailing, or consecutive hyphens, and matches the skill's folder name exactly.
- Frontmatter \`description\` is required, non-empty, and at most 1024 characters.
- Frontmatter \`compatibility\`, when present, is at most 500 characters.
- The body stays under 500 lines.

Quality recommendations (report as [quality]) - not spec violations, but worth flagging:
- Frontmatter \`description\` is written in third person, states WHEN to use the skill and what it does, and includes trigger phrases a user or model would actually say.
- Anything approaching the 500-line body limit should split into linked files under the skill folder (progressive disclosure) rather than staying inlined.
- Prefer linking to files under the skill folder over inlining their contents.
- No secrets (API keys, tokens, credentials) anywhere in the file.
- Instructions are written for a model: imperative, specific, and unambiguous.
- Examples are included wherever the task is ambiguous without one.

Here is the SKILL.md to review:

${fence}markdown
${input.skillMd}
${fence}

Respond with exactly this structure:

## Findings

Up to 8 bullets, ordered by impact. Each bullet reads: "**[spec|quality] [high|medium|low]** what is wrong — why it matters".

## Verdict

One line summarizing whether this skill is ready to ship as-is.

${PROPOSED_HEADING}

Only include this section if changes are worth making. If so, follow the heading with ONE fenced block wrapping the complete revised SKILL.md - frontmatter included, nothing omitted, no placeholders. Wrap the proposed file in a fence of exactly ${fence.length} backticks (${fence}markdown … ${fence}), because the file itself contains shorter fences. If no change is worth making, write "No changes proposed." instead of the block.`;
}

/** Matches a fenced code block (3+ backticks or 3+ tildes, optionally tagged \`markdown\`/\`md\`) and captures the opening fence, the info string line, and the body. */
const FENCE_OPEN_RE = /^(`{3,}|~{3,})[ \t]*(?:markdown|md)?[ \t]*$/;

/**
 * Pulls the fenced SKILL.md rewrite out of a finished run's `finalText`, if
 * the harness proposed one. Returns null when there's no `## Proposed
 * SKILL.md` heading, no fence opener follows it, or the fence is never
 * closed (e.g. "No changes proposed.").
 *
 * The opener's fence character and length set the closer to match: an inner
 * fence of the same character but a shorter run doesn't close the block, so
 * a proposed file that itself contains ``` fences survives inside a longer
 * outer fence (see `fenceForMarkdown`).
 */
export function extractProposedSkillMd(finalText: string): string | null {
  const headingIndex = finalText.indexOf(PROPOSED_HEADING);
  if (headingIndex === -1) return null;
  const afterHeading = finalText.slice(headingIndex + PROPOSED_HEADING.length);
  const lines = afterHeading.split(/\r\n|\n/);

  let openerIndex = -1;
  let fenceChar = "";
  let fenceLength = 0;
  for (let i = 0; i < lines.length; i++) {
    const match = lines[i].match(FENCE_OPEN_RE);
    if (match) {
      openerIndex = i;
      fenceChar = match[1][0];
      fenceLength = match[1].length;
      break;
    }
  }
  if (openerIndex === -1) return null;

  const closerRe = new RegExp(`^${fenceChar}{${fenceLength},}[ \t]*$`);
  for (let i = openerIndex + 1; i < lines.length; i++) {
    if (closerRe.test(lines[i])) {
      return lines.slice(openerIndex + 1, i).join("\n");
    }
  }
  return null;
}

/**
 * Converts `proposal`'s line endings and trailing newline to match
 * `original`'s: LF unless `original` uses CRLF, and either exactly one
 * trailing newline (if `original` has one) or none (if it doesn't). Applied
 * before diffing a harness-proposed rewrite against the file it reviewed, so
 * a spurious line-ending or EOF-newline change doesn't show up as a hunk.
 */
export function normalizeProposalToOriginal(proposal: string, original: string): string {
  let normalized = proposal.replace(/\r\n/g, "\n");
  if (original.includes("\r\n")) {
    normalized = normalized.replace(/\n/g, "\r\n");
  }
  const newline = original.includes("\r\n") ? "\r\n" : "\n";
  const stripped = normalized.replace(/(?:\r\n|\n)+$/, "");
  return original.endsWith("\n") ? `${stripped}${newline}` : stripped;
}
