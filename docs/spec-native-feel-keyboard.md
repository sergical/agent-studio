# Spec: Keyboard (step 4 of docs/spec-native-feel-iteration.md)

Read first: the "Keyboard (step 4)" requirements in
`docs/audit-motion-and-accessibility.md`, and `docs/spec-native-feel-lists.md`
(step 3 is applied: Skills and Home lists are `role="grid"` with `role="row"`
rows, the first row has `tabIndex={0}`, Enter opens, Space toggles, and
`rowGroup()` lives in `skill-row-state.tsx`).

Work in `/Users/sergiydybskiy/src/agent-studio/.claude/worktrees/shared-core-primitives`
(branch `dev/browser-harness`). Do not commit, do not touch any `package.json`
or the lock file (no new dependencies), do not start a dev server
(http://localhost:1420 is up), never use `git stash`.

Goal: every common action is reachable without the mouse, and nothing that
the keyboard triggers animates.

## 1. `Kbd` primitive (`packages/ui`)

Add `packages/ui/src/components/kbd.tsx` exporting `Kbd` (a `kbd` element,
`inline-flex h-4.5 min-w-4.5 items-center justify-center rounded-xs border
border-border-subtle px-1 font-sans text-caption text-text-tertiary`), and
export it from `packages/ui/src/index.ts`. Use the macOS glyphs ⌘ ⇧ ⌥ ↵ ↑ ↓.
If `DropdownMenuShortcut` exists, make it render its children the same way or
leave it and use `Kbd` inside it; do not keep two different looks.

## 2. Row cursor (`apps/desktop/src/hooks/useRowCursor.ts`)

One hook used by `SkillListTable.tsx` and the Home list.

- Input: the ordered list of visible row keys (skills in rendered order,
  skipping rows inside collapsed groups), and callbacks `onOpen(key)`,
  `onToggle(key)`, `onMenu(key)`.
- State: the cursor key. Roving `tabIndex`: the cursor row is 0, all others -1.
  The cursor survives re-sorts and filter changes when its key is still
  visible; otherwise it moves to the nearest row by previous index.
- Keys, handled on the grid element's `onKeyDown` (focus is inside the list):
  - `ArrowDown`/`j`, `ArrowUp`/`k`: move by one and focus the row.
  - `Home`/`End`: first/last row.
  - `Enter`: `onOpen`. `Space` and `x`: `onToggle`.
  - `Shift+ArrowDown`/`Shift+ArrowUp`: move and toggle the row moved onto
    into the selection (extend).
  - `.` or `Shift+F10`: `onMenu` opens that row's ⋯ menu.
  - `Escape`: clear the selection if any (existing behaviour).
- Window-level entry: when the active view is Skills or Home, no input,
  textarea, contenteditable, menu, listbox or dialog has focus, pressing
  `j`, `k`, `ArrowDown` or `ArrowUp` focuses the cursor row (first row if none).
- Movement uses `row.focus({ preventScroll: true })` then
  `row.scrollIntoView({ block: "nearest" })`. No smooth scrolling. No
  transitions on the focused or cursor row.
- Group headers: `ArrowLeft` on a row collapses its group and moves focus to
  the header button; `ArrowRight` on a collapsed header expands it. Tab from a
  row goes to the next focusable element after the grid, not through every row.
- Announce position through a visually hidden `role="status"` element in the
  list: "12 of 80". Debounce to the last move.

## 3. Command palette (`apps/desktop/src/components/CommandPalette/`)

- `⌘K` toggles it from anywhere, including inside inputs. Mount once in
  `App.tsx`. Store: `commandPaletteOpen: boolean` and
  `setCommandPaletteOpen(open)` in `appStore.ts`.
- Structure: kit `Dialog` (modal, `aria-label="Command palette"`) with every
  enter and exit animation disabled on overlay and popup (`animate-none`,
  `transition-none`), no backdrop blur. Width 560px, top offset 15vh,
  max 420px of results that scroll.
- Input: kit `Input`, `role="combobox"`, `aria-expanded="true"`,
  `aria-controls` the listbox, `aria-activedescendant` the highlighted option.
  Focus stays in the input. Placeholder "Type a command or skill".
- Results: `role="listbox"` with sections (`role="group"` with
  `aria-label`): **Actions**, **Go to**, **Skills**. Options are
  `role="option"` with `aria-selected` on the highlighted one. Highlight is
  instant (`bg-bg-active`), no transitions. Arrow keys and `⌃n`/`⌃p` move the
  highlight; Enter runs; mouse hover moves the highlight; Escape closes.
  Scroll the highlighted option with `scrollIntoView({ block: "nearest" })`.
- Items:
  - Actions: Add skill (`openAddSkillSheet()`), Sync (the Sidebar's refresh
    action, `requestRescan()`), Filter skills (go to Skills and
    `requestSkillSearchFocus()`), Toggle theme (the existing theme toggle).
  - Go to: Home, Skills, Plugins, Activity, Packs, Learn, Settings
    (`setActiveView`).
  - Skills: every installed skill by name, with its glyph and `rowGroup`
    label as secondary text; Enter opens it (`openSkill`).
- Matching: case-insensitive; rank prefix match, then word-start match, then
  substring, then by name. With an empty query show Actions and Go to, and
  the first 8 skills. No library.
- Result count announced via a polite `role="status"` ("5 results").
- Each command with a shortcut shows it with `Kbd` and has
  `aria-keyshortcuts`.
- Closing returns focus to the element that had it before opening (kit
  Dialog `finalFocus` or manual).
- `SkillPage`'s Escape handler must not fire while the palette is open.

## 4. Other shortcuts and hints

- `/` focuses the skill filter when the Skills view is active and focus is not
  in an editable element.
- `⌘N` opens Add skill; `⌘,` opens Settings. Block these when a dialog is open.
- Keep all shortcut handling in one module, `apps/desktop/src/lib/app-shortcuts.ts`
  (definitions: id, keys, label) consumed by the palette, tooltips, and menus,
  so hints and handlers cannot drift. The global listener lives in a hook,
  `hooks/useAppShortcuts.ts`, mounted once in `App.tsx`, and follows the
  `useNativeShell.ts` pattern.
- Hints:
  - Sidebar tooltips: Search skills `/`, Add skill `⌘N`, Settings `⌘,`.
    Extend `TooltipControl` to accept an optional `shortcut` prop rendered
    with `Kbd`, rather than baking text into the content.
  - Row ⋯ menu: Open `↵`, Select `X`.
  - Sidebar switcher row: add a third icon button "Command palette" (`⌘K`) only
    if it fits without crowding; otherwise put `⌘K` in the Search skills
    tooltip. Report which.

## Rules

`anti-slop` lint: no `title=`, no `satisfies`, `// SAFETY:` on every type
assertion, no dead exports, no raw form elements outside `packages/ui`.
Imports grouped React, external, internal, types. Comments only for the
non-obvious. No new CSS variables. No animation on anything in this step.

## Acceptance

```
npx tsc --noEmit -p apps/desktop/tsconfig.json
npx oxlint apps/desktop/src packages/ui/src
npx oxfmt --check <every file you changed>
```

Browser check with `agent-browser --session keys1` against
`http://localhost:1420/?f=1`. Screenshots and helper `.sh` scripts go in
`/private/tmp/claude-501/-Users-sergiydybskiy-src-agent-studio--claude-worktrees-shared-core-primitives/9f53962b-6417-4cd1-9df0-6b7fe02c976e/scratchpad`.
Write JS with quotes into a `.sh` file and run it with `sh`.

1. Skills view, click the page background, press `j` three times: report
   `document.activeElement` row name and `aria-rowindex`; screenshot
   `keys-cursor.png`. Press `k`: cursor moves up. Press `End`, then `Home`.
2. Press `x`, then `Shift+ArrowDown`: 2 rows selected. Escape clears.
3. Press `.`: the row menu opens; Escape closes it and focus returns to the row.
4. Press Enter: skill page opens. Escape: back to Skills with the cursor on
   the same row.
5. `⌘K`: palette open, screenshot `keys-palette.png`. Type `com`: report the
   option texts in order and the status text. ArrowDown, Enter: the chosen
   item runs. `⌘K` again, Escape: focus returns to the prior element.
6. Report computed `transition-duration` and `animation-name` on the palette
   popup and on a highlighted option (must be 0s / none).
7. `/` on Skills focuses "Filter skills". `⌘N` opens Add skill.
8. Console: report every error verbatim.

Report: files changed, check output verbatim on failure, eval values,
screenshot paths, the sidebar ⌘K choice, and anything skipped or uncertain.
