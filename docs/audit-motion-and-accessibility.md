# Audit: motion and accessibility (2026-09-13)

Read-only audits of `apps/desktop/src` and `packages/ui/src` against the
emil-animations and emil-touch-and-accessibility rules. The app is a macOS
desktop app: mouse, trackpad, keyboard, VoiceOver. Touch-only rules do not
apply; the hit-area minimum is 24px (WCAG 2.5.8), 28px preferred.
Line numbers are from commit b758cc4 and shift with later edits.

## Foundations (shared edits, do before steps 3-5)

Focus and contrast
- Focus rings are too faint. `HIT_CLASS` (`SkillRowCells.tsx:34`) uses
  `outline-border` (1.3:1). The kit `ring-ring/50` (`button.tsx:14`, `toggle.tsx:12`,
  `select.tsx:45`, `radio-group.tsx:27`) is about 1.9:1 dark, 1.6:1 light.
  `--color-border-focus` (`App.css:34,125`) is 2.85:1 in light.
  `CheckboxControl.tsx:34` uses `border-strong` (1.7:1). Fix: one accent focus
  ring (6.1:1 dark, 5.7:1 light) everywhere, 2px.
- Unchecked checkbox, switch and segmented outlines are below 3:1; use a stronger edge.
- `text-quaternary` fails contrast everywhere (2.0-2.6:1). Use it only for
  disabled or decorative marks; `InfoPopover` and the `GroupHead` chevron use it
  meaningfully.
- Dark theme: white text on accent Button is 3.21:1; darken the accent fill.

Motion
- No duration or easing tokens. Add to `App.css @theme`:
  `--ease-out-quint: cubic-bezier(0.23,1,0.32,1)`,
  `--ease-drawer: cubic-bezier(0.32,0.72,0,1)`,
  `--default-transition-duration: 120ms`,
  `--default-transition-timing-function: var(--ease-out-quint)`.
- Reduced motion (`App.css:233`) leaves infinite spin and pulse loops running
  at 0.01ms, so they flicker. Add `animation-iteration-count: 1 !important`.
- Theme switch (`lib/theme.ts:57`) removes `.theme-switching` before a style
  flush; force a flush (`getComputedStyle(root).color`) first.
- `transition-all` in `button.tsx:14`, `tabs.tsx:57`, `switch.tsx:20`,
  `badge.tsx:13`: name the properties.
- Menu item highlight fades under the keyboard (`MenuControl.tsx:24`,
  `SkillListFilterBar.tsx:50`): remove `transition-colors`.
- Drawer (`drawer.tsx:34,66`): keyframes, same speed both ways, not
  interruptible. Use 240ms in, 180ms out on `--ease-drawer`.
- Dialog (`dialog.tsx:34,54`, `alert-dialog.tsx:29,44`): drop backdrop blur,
  150ms in, 100ms out.
- Tooltip (`tooltip.tsx:45`): `data-instant:animate-none`, drop the dead
  `delayed-open` classes.
- Sonner toasts use about 400ms; override to 220ms.
- `SkillLocationScope.tsx:62-65`: remove the row stagger and delay;
  line 96 uses `ease-in` alone.
- `collapsible.tsx:23` animates `height`; Home groups should open instantly.
- `motion` is in `apps/desktop/package.json` but never imported.

Labels and semantics
- `aria-label` on a plain `span` is ignored: `HarnessMark.tsx:115,160`,
  `SkillCoverageMatrix.tsx:75-90`. Add `role="img"` or `sr-only` text.
- Harness label omits disabled and parked (`HarnessMark.tsx:47-57`).
- Status dots are colour-only or tooltip-only: `StatusIcon.tsx:39-47`,
  `HomeView.tsx:85-87`, `HarnessStack.tsx:88-90` (+N), token and spec-note tooltips.
- Missing labels: `PackNamePrompt.tsx:70-78` input (and its error),
  `SkillSearchBar.tsx:77-84` clear button, `WindowSegmentedControl.tsx:19`.
- `SkillCoverageMatrix.tsx:99-123`: `tr role="button"` flattens the table.
- `aria-label` hides visible text: `SkillListFilterBar.tsx:184,237`.
- Sidebar place items lack `aria-current="page"`.
- No live regions for the filter result count, scan banner, or sync finish.
- `SkillPage.tsx:259-280` Escape goes back even when a menu or listbox is open.
- Hit areas below 24px: sidebar icon buttons, `ScanPartialBanner.tsx:34`,
  `InfoPopover.tsx:40` (14px), token sort buttons (`SkillRowCells.tsx:226`).
  Expand with a pseudo-element; keep the visual size.

## Requirements for steps 3-5

Lists (step 3)
- The list is a `grid` or `listbox` with one tab stop. Rows are not bare
  `div onClick` (`SkillListTable.tsx:265`): they get focus, Enter opens, and
  an accent focus ring separate from the selected style.
- Group headers are `button aria-expanded aria-controls`, named with label and
  count. Collapsed panels are `hidden`, not height 0. Open instantly.
- Row actions stay reachable without hover.

Keyboard (step 4)
- Palette: no open or close animation, instant highlight, no smooth scroll.
  Modal dialog with a combobox input (`aria-activedescendant`) over a
  `listbox`; focus returns to the prior element on close; polite result count;
  `aria-keyshortcuts` on commands.
- Row cursor: no transition on the cursor; `scrollIntoView({block:"nearest"})`.
  j/k and arrows move, Enter opens, x or Space toggles selection, Home/End jump,
  Esc clears. Keys are ignored in inputs and while a menu or the palette is open.
  `.` or Shift+F10 opens the row menu.

Detail (step 5)
- Rail is `aside aria-label="Properties"`. Each value is a button named
  "Property: value, edit". Enter commits, Esc cancels, focus returns to the
  value. Saves and errors are announced. Esc in the rail does not go back.
