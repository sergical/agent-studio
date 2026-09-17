# Spec: Lists (step 3 of docs/spec-native-feel-iteration.md)

Read first: `docs/skill-row-design-decision.md` (Stack row) and the
"Requirements for steps 3-5 / Lists" section of
`docs/audit-motion-and-accessibility.md`. Foundations is already applied:
focus rings are accent, collapsibles open instantly.

Work in `/Users/sergiydybskiy/src/agent-studio/.claude/worktrees/shared-core-primitives`
(branch `dev/browser-harness`). Do not commit, do not touch any `package.json`
or the lock file, do not start a dev server (http://localhost:1420 is up),
never use `git stash`.

Goal: the Skills list reads like a Linear issue list: state groups with counts,
no column headers, 32px rows, selection without a mode switch. Home uses the
same row so the two pages look like one product.

## 1. Row height and columns (`SkillList/SkillRowCells.tsx`, `SkillListTable.tsx`)

- Row height `h-9` becomes `h-8` (32px). Update the "36px" note in
  `docs/skill-row-design-decision.md` to 32px with one line saying why
  (denser, Linear-like list).
- Remove the column header row (`SkillListTable.tsx`, the row using
  `HEADER_CELL_CLASS`) and `TokenPairHeader`. Remove `HEADER_CELL_CLASS` if
  nothing else uses it.
- Token sort: remove the `tokenSort` state. Check what `sortRows` does for the
  `"size"` sort mode. If "Largest" already sorts by full tokens, the token cell
  shows the pair (prompt, full) and sorts by full; nothing else changes. If
  "size" sorts by something else, report it and keep "Largest" meaning full
  tokens. Do not add sort options.
- Grid template: add a 20px checkbox gutter before the glyph column:
  `20px_var(--glyph-hit)_minmax(0,1fr)_160px_148px_104px`.

## 2. Selection without a mode switch

- The checkbox gutter renders `CheckboxControl` in every row. It is
  `opacity-0` and becomes visible on row hover, row `:focus-within`, when the
  row is checked, or whenever any row is checked. No transition.
- Checking a box calls the existing selection actions (`toggleSkillSelection`;
  enter selection mode if the store still requires it). Unchecking the last
  row exits selection mode.
- The selection action bar (Create pack, Cancel) shows when at least one row
  is checked; its label shows the count ("3 selected").
- Remove whatever entry point only served "enter select mode" (a menu item or
  button) if the checkbox now covers it; keep Escape clearing the selection.
- The ⋯ trigger that appeared on hover in the leading cell moves to the end of
  the row (after tokens, inside the tokens cell or a new 24px column). It stays
  hover and focus-within revealed and is also visible on the checked row.

## 3. State groups (Skills list only; not the coverage view)

- Buckets, in this order:
  - **Needs attention**: `isDecision(rowState(...))` is true.
  - **Healthy**: `rowState(...)` is null and the skill is not parked.
  - **Parked**: `skill.parked` (or `rowState` kind `parked`).
    Put the bucketing in one exported function next to `rowState` in
    `skill-row-state.tsx` (for example `rowGroup(skill, ...) => "attention" | "healthy" | "parked"`),
    so the palette in step 4 can reuse it.
- Sorting from the Sort dropdown applies inside each group.
- Hide empty groups. If only one group has rows, still show its header.
- Group header: a 28px row, `text-small font-medium text-text-secondary`,
  chevron (`text-text-tertiary`, rotates, `motion-reduce:transition-none`),
  label, count `tabular-nums text-text-tertiary`. Sticky at the top of the
  scroll area (`sticky top-0 z-1 bg-bg-primary`), with a `border-b
border-border-subtle`. Use the kit `Collapsible` like Home's `GroupHead`
  does; reuse `GroupHead` by moving it to a shared file in
  `components/SkillList/` if its shape fits, instead of writing a second one.
- The trigger is a real button with `aria-expanded`; its accessible name
  includes label and count ("Needs attention, 4 skills"). Collapsed panels
  are not rendered or are `hidden`.
- Collapsed state: all groups open by default, kept in component state (not
  persisted).

## 4. Semantics and focus (from the audit)

- The list container is `role="grid"` with `aria-label="Skills"` and
  `aria-rowcount`. Group headers are `role="row"` with one
  `role="gridcell"` holding the trigger button. Skill rows are `role="row"`
  with `aria-rowindex`, `aria-selected` when checked; cells are
  `role="gridcell"`.
- Skill rows are no longer bare `div onClick`: the row element has
  `tabIndex={-1}`, except the first skill row in the list, which has
  `tabIndex={0}` (keyboard navigation between rows is step 4). Enter on a
  focused row opens the skill (same handler as click). Space toggles its
  checkbox.
- Focused row: `focus-visible:outline-2 outline-accent -outline-offset-2`,
  distinct from the checked style.
- Row clicks on the checkbox, glyph, or ⋯ do not open the skill (stop
  propagation where the current code does).
- No transitions on row background, cursor, or checked state.

## 5. Home adopts the Stack row (`Home/HomeView.tsx`)

- Replace `InboxRow` with the shared row cells: glyph (from `rowState`, empty
  when null), name, location, harnesses. The last column shows the group's
  `detail` (`text-small text-text-tertiary truncate`) with the group's
  `action` node right-aligned after it. Same 32px height and the same grid
  structure as Skills, with the tokens column replaced by a
  `minmax(0,240px)` detail column and an `auto` action column.
- No checkbox gutter on Home.
- Home keeps its groups, `GroupHead`, and collapsed state.
- The severity dot goes (the glyph shows state); make sure the glyph or an
  `sr-only` span still names the severity.
- Row semantics on Home: same `grid`/`row`/`gridcell` pattern, Enter opens.

## Rules

`anti-slop` lint: no `title=`, no `satisfies`, `// SAFETY:` on every type
assertion, no dead exports, no raw form elements outside `packages/ui`.
Imports grouped React, external, internal, types. Comments only for the
non-obvious. No new CSS variables.

## Acceptance

```
npx tsc --noEmit -p apps/desktop/tsconfig.json
npx oxlint apps/desktop/src packages/ui/src
npx oxfmt --check <every file you changed>
```

Browser check with `agent-browser --session lists1` against
`http://localhost:1420/?f=1`. Screenshots and helper `.sh` scripts go in
`/private/tmp/claude-501/-Users-sergiydybskiy-src-agent-studio--claude-worktrees-shared-core-primitives/9f53962b-6417-4cd1-9df0-6b7fe02c976e/scratchpad`.
Write JS with quotes into a `.sh` file and run it with `sh`.

1. Skills: screenshot `lists-skills.png`. Report group header texts and counts
   (they must add up to the total), the `offsetHeight` of a skill row (32),
   and that no element has `role="columnheader"` or the old header text
   "HARNESSES".
2. Collapse "Healthy": screenshot `lists-collapsed.png`; its rows are gone
   from the DOM or `hidden`; `aria-expanded="false"`.
3. Hover a healthy row: checkbox visible. Check two rows: screenshot
   `lists-selected.png`; action bar shows "2 selected". Press Escape: selection
   clears.
4. Focus the first row with Tab, press Enter: the skill page opens. Go back.
5. Home: screenshot `lists-home.png`; rows are 32px and show glyph, name,
   location, harnesses, detail, action.
6. Coverage view still renders (toggle it once, screenshot `lists-coverage.png`).
7. Console: report every error verbatim.

Report: files changed, check output verbatim on failure, eval values,
screenshot paths, what `"size"` sorts by, which select-mode entry point you
removed, and anything skipped or uncertain.
