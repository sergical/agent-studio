// ============================================================================
// skill-violation-text - turns the validator's own strings into one sentence a
// reader can take in at a glance. The raw list stacks as
// "missing required frontmatter field: name / missing required frontmatter
// field: description", which reads as machine output rather than as a problem
// the reader can act on. Each kind also gets one sentence on what the agents
// do with it (measured behaviour: docs/agent-skill-conventions.md).
// ============================================================================

/** "name", "name and description", "name, description, and license". */
function andList(items: string[]): string {
  if (items.length === 1) return items[0];
  if (items.length === 2) return `${items[0]} and ${items[1]}`;
  return `${items.slice(0, -1).join(", ")}, and ${items[items.length - 1]}`;
}

function asSentence(text: string): string {
  const trimmed = text.trim();
  const capitalized = trimmed.charAt(0).toUpperCase() + trimmed.slice(1);
  return /[.!?]$/.test(capitalized) ? capitalized : `${capitalized}.`;
}

/** What the agents do with one violation, or "" when there is nothing to add. */
export function describeSpecViolationImpact(violation: string): string {
  if (violation === "missing required frontmatter field: description") {
    return "Codex, OpenCode, and pi skip it. Claude Code still loads it.";
  }
  if (violation === "missing required frontmatter field: name") {
    return "OpenCode skips it without a warning. Claude Code, Codex, and pi use the folder name.";
  }
  if (violation.startsWith("invalid YAML frontmatter")) {
    return "pi skips it. Claude Code, Codex, and OpenCode repair it and load it.";
  }
  const mismatch = /^name "(.*)" does not match its directory name "(.*)"$/.exec(violation);
  if (mismatch) {
    return `Claude Code calls it "${mismatch[2]}". Codex, OpenCode, and pi call it "${mismatch[1]}".`;
  }
  if (violation.startsWith('name "') || violation === "description exceeds 1024 characters") {
    return "Every agent still loads it. pi shows a warning.";
  }
  if (
    violation === "compatibility exceeds 500 characters" ||
    violation === "SKILL.md exceeds recommended 500 lines"
  ) {
    return "Every agent still loads it.";
  }
  return "";
}

/**
 * One human sentence per group of violations, then one impact sentence per distinct kind of
 * violation. Every missing frontmatter field collapses into a single clause, since a reader
 * fixing them opens the same file once; anything else keeps its own sentence.
 */
export function describeSpecViolations(violations: string[]): string {
  const missingFields: string[] = [];
  const others: string[] = [];
  for (const violation of violations) {
    const field = /^missing required frontmatter field: (.+)$/.exec(violation)?.[1];
    if (field) missingFields.push(field);
    else others.push(violation);
  }
  const sentences: string[] = [];
  if (missingFields.length > 0) {
    sentences.push(`SKILL.md has no ${andList(missingFields)} in its frontmatter.`);
  }
  sentences.push(...others.map(asSentence));
  const impacts = new Set(violations.map(describeSpecViolationImpact).filter(Boolean));
  return [...sentences, ...impacts].join(" ");
}
