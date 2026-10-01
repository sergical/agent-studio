// ============================================================================
// Skill Studio - Home row click
// Decides whether a click on a Home inbox row opens the skill
// ============================================================================

/**
 * True when a click on `row` should open its skill. React bubbles clicks from
 * portaled dialogs through the component tree, so a click inside a row
 * action's dialog reaches the row even though the dialog is not in the row's
 * DOM. Those clicks, and clicks on the row's own buttons, belong to the action.
 */
export function rowClickOpensSkill(row: Node, target: Element | null): boolean {
  if (target === null || !row.contains(target)) return false;
  return target.closest("button, a, [role='button']") === null;
}
