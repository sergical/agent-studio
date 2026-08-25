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

/**
 * Builds the prompt for the "Audit" action: reviews `input.skillMd` against
 * the agentskills.io spec rules and asks for a findings list, a verdict, and
 * (only when worth it) a complete revised SKILL.md.
 */
export function buildSkillAuditPrompt(input: SkillAuditPromptInput): string {
  const deploymentsLine =
    input.deployments.length > 0 ? input.deployments.join(", ") : "no other deployments";
  return `You are reviewing an agent skill (a SKILL.md file) for Skill Studio.

The skill is "${input.skillName}", running under ${input.harness} in this review, and otherwise deployed to: ${deploymentsLine}.

Judge it against the agentskills.io SKILL.md spec:
- Frontmatter \`name\` is kebab-case, at most 64 characters, and matches the skill's folder name.
- Frontmatter \`description\` is at most 1024 characters, written in third person, states WHEN to use the skill and what it does, and includes trigger phrases a user or model would actually say.
- Frontmatter may also carry \`license\`, \`compatibility\`, \`metadata\`, and \`allowed-tools\`, each optional.
- The body stays under 500 lines; anything longer should split into linked files under the skill folder (progressive disclosure) rather than staying inlined.
- Prefer linking to files under the skill folder over inlining their contents.
- No secrets (API keys, tokens, credentials) anywhere in the file.
- Instructions are written for a model: imperative, specific, and unambiguous.
- Examples are included wherever the task is ambiguous without one.

Here is the SKILL.md to review:

\`\`\`markdown
${input.skillMd}
\`\`\`

Respond with exactly this structure:

## Findings

Up to 8 bullets, ordered by impact. Each bullet reads: "**[high|medium|low]** what is wrong — why it matters".

## Verdict

One line summarizing whether this skill is ready to ship as-is.

${PROPOSED_HEADING}

Only include this section if changes are worth making. If so, follow the heading with ONE fenced block tagged \`markdown\` containing the complete revised SKILL.md - frontmatter included, nothing omitted, no placeholders. If no change is worth making, write "No changes proposed." instead of the block.`;
}

/** Matches a fenced code block (\`\`\` or ~~~, optionally tagged \`markdown\`/\`md\`) and captures its body. */
const FENCE_BLOCK_RE = /^(`{3,}|~{3,})[ \t]*(?:markdown|md)?[ \t]*\r?\n([\s\S]*?)\r?\n\1[ \t]*$/m;

/**
 * Pulls the fenced SKILL.md rewrite out of a finished run's `finalText`, if
 * the harness proposed one. Returns null when there's no `## Proposed
 * SKILL.md` heading, or no fenced block follows it (e.g. "No changes
 * proposed.").
 */
export function extractProposedSkillMd(finalText: string): string | null {
  const headingIndex = finalText.indexOf(PROPOSED_HEADING);
  if (headingIndex === -1) return null;
  const afterHeading = finalText.slice(headingIndex + PROPOSED_HEADING.length);
  const match = afterHeading.match(FENCE_BLOCK_RE);
  if (!match) return null;
  // Normalize CRLF line endings inside the block so the result is safe to
  // diff against `rawContent`, which is always read as LF.
  return match[2].replace(/\r\n/g, "\n");
}
