// ============================================================================
// skill-violation-text - turns the validator's own strings into one sentence a
// reader can take in at a glance. The raw list stacks as
// "missing required frontmatter field: name / missing required frontmatter
// field: description", which reads as machine output rather than as a problem
// the reader can act on.
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

/**
 * One human sentence per group of violations. Every missing frontmatter field
 * collapses into a single clause, since a reader fixing them opens the same
 * file once; anything else keeps its own sentence.
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
  return sentences.join(" ");
}
