# Spec: Foundations (between step 2 Frame and step 3 Lists)

Source: `docs/audit-motion-and-accessibility.md` (read it first; line numbers
there are approximate, find code by content). This step applies the shared
fixes so steps 3-5 start on a correct base. It does NOT change row semantics,
list structure, keyboard navigation, or the detail layout; those are steps 3-5.

Work in `/Users/sergiydybskiy/src/agent-studio/.claude/worktrees/shared-core-primitives`
(branch `dev/browser-harness`). Do not commit, do not touch `package-lock.json`
or any `package.json`, do not start a dev server (http://localhost:1420 is
running), never use `git stash`.

## A. Focus rings and control contrast

1. One focus ring: 2px accent. In `apps/desktop/src/App.css`, set the focus
   colour so it reaches at least 3:1 on `bg-primary` and `bg-secondary` in both
   themes (the accent is 6.1:1 dark, 5.7:1 light). Prefer pointing
   `--color-border-focus` and the kit `--color-ring` at the accent over adding
   a new variable.
2. `packages/ui`: remove the `/50` alpha from `ring-ring/50` in `button.tsx`,
   `toggle.tsx`, `select.tsx`, `radio-group.tsx` and any other kit file with it
   (grep `ring-ring/`).
3. `SkillRowCells.tsx` `HIT_CLASS`: `focus-visible:outline-border` becomes the
   accent outline. `CheckboxControl.tsx`: focus outline becomes accent; the
   unchecked box edge uses a token that reaches 3:1 (e.g. `text-tertiary`
   colour as the border).
4. Unchecked `SwitchControl` track edge and segmented/toggle-group outlines:
   reach 3:1 against their background. Hairline separators stay as they are.
5. `text-quaternary` used for meaningful marks: `InfoPopover.tsx` trigger icon
   and the Home `GroupHead` chevron become `text-tertiary`. Leave decorative
   and disabled uses.

Do not change the dark accent fill in this step (a brand decision); report the
current white-on-accent ratio instead.

## B. Motion tokens and fixes

1. `App.css @theme`: add `--ease-out-quint: cubic-bezier(0.23, 1, 0.32, 1)`,
   `--ease-drawer: cubic-bezier(0.32, 0.72, 0, 1)`,
   `--default-transition-duration: 120ms`,
   `--default-transition-timing-function: var(--ease-out-quint)`.
   These are the only new CSS variables allowed.
2. Reduced-motion block: add `animation-iteration-count: 1 !important`.
   Add `motion-reduce:animate-none` to the pulse skeletons in
   `SkillMarkdownCard.tsx` and `AddSkillSheet.tsx`.
3. `apps/desktop/src/lib/theme.ts`: force a style flush
   (`void getComputedStyle(root).color`) after stamping the theme and before
   removing `.theme-switching`.
4. `transition-all` in `button.tsx`, `tabs.tsx`, `switch.tsx`, `badge.tsx`:
   name the properties that actually change (colors, background, border,
   box-shadow, and translate/scale where the component uses them).
5. Remove `transition-colors` from menu item highlight in `MenuControl.tsx`
   (DEFAULT_ITEM_CLASS) and the item class in `SkillListFilterBar.tsx`.
6. `drawer.tsx`: 240ms enter, 180ms exit, `ease-(--ease-drawer)`; backdrop uses
   the same timing. Keep the tw-animate keyframes approach unless Base UI's
   `data-starting-style`/`data-ending-style` transition is a small change;
   report which you chose.
7. `dialog.tsx`, `alert-dialog.tsx`: remove `backdrop-blur-*`; 150ms enter,
   100ms exit, `ease-(--ease-out-quint)`; overlay and popup share timing.
8. `tooltip.tsx`: add `data-instant:animate-none`; remove the dead
   `data-[state=delayed-open]` classes.
9. Toasts (`App.tsx` Toaster): shorten sonner's motion to about 220ms with a
   CSS override in `App.css` on `[data-sonner-toast]`, if sonner exposes it
   through CSS; otherwise report and skip.
10. `SkillLocationScope.tsx`: remove the per-row stagger and delay keyframe;
    at most a 120ms opacity transition with no delay. Replace `ease-in` on the
    harness stack fade with `ease-out`.
11. `collapsible.tsx`: height animation off by default (instant open/close);
    keep an opt-in prop only if a current caller needs animation (none should).
12. `HomeView.tsx` stat cards with `active:scale-98`: remove `translate-y-px`
    on those three cards so press feedback is one property.

## C. Radius scale

`packages/ui/src/styles.css` (around line 93) chains `--radius-sm: var(--radius-xs)`,
`--radius-md: var(--radius-sm)`, and so on, so every `rounded-*` utility
resolves to 4px and `App.css`'s scale (xs 4, sm 6, md 10, lg 14, xl 20) never
applies. Do NOT change it. Report which radius each utility resolves to at
runtime (eval `getComputedStyle` on elements with `rounded-sm`, `rounded-md`,
`rounded-lg`) and what the chain appears to intend. The user decides.

## D. Labels, semantics, live regions

1. `HarnessMark.tsx` and `SkillCoverageMatrix.tsx`: every `span` with
   `aria-label` gets `role="img"`. Include disabled and parked state in the
   harness label text.
2. `StatusIcon.tsx`: when it conveys state, render an `sr-only` span with the
   state text instead of relying on `aria-hidden` plus tooltip. Home issue-row
   severity dot (`HomeView.tsx`) gets the same.
3. `HarnessStack.tsx` "+N": the hidden harness names are in an `sr-only` span.
4. `PackNamePrompt.tsx`: input `aria-label="Pack name"`; error has an id,
   `role="alert"`, linked via `aria-describedby`; `aria-invalid` when invalid.
5. `SkillSearchBar.tsx` clear button: `aria-label="Clear search"`.
   `WindowSegmentedControl.tsx`: `aria-label="Usage window"`.
6. `SkillListFilterBar.tsx`: the Project and Filter triggers keep their visible
   text in the accessible name (`Project: <name>`, `Filter, N active`).
   The result count gets `role="status"`.
7. `ScanPartialBanner.tsx`: `role="status"`.
8. `Sidebar.tsx`: every place item gets `aria-current="page"` when active.
   Sync keeps focus while running (`aria-disabled` instead of `disabled`, and
   ignore clicks while syncing).
9. `SkillPage.tsx` Escape handler: skip when `event.defaultPrevented`, or when
   the event target is inside `[role=menu]`, `[role=listbox]`, `[role=dialog]`,
   or an open popup.

## E. Hit areas (keep the visual size)

Expand to at least 24px with a pseudo-element (`relative` plus
`before:absolute before:-inset-[Npx] before:content-['']`), and widen gaps so
neighbouring hit areas do not overlap:

- Sidebar switcher and footer icon buttons (22-24px).
- `ScanPartialBanner.tsx` dismiss button.
- `InfoPopover.tsx` trigger (14px, expand to 28px).
- Token sort buttons in `SkillRowCells.tsx` (about 16px tall).

If the kit `Button` already has a hit-area pseudo-element that conflicts, use
`after:` or report it.

## Rules

`anti-slop` lint: no `title=`, no `satisfies`, `// SAFETY:` on every type
assertion, no dead exports, no raw form elements outside `packages/ui`.
Comments explain only what is non-obvious.

## Acceptance

```
npx tsc --noEmit -p apps/desktop/tsconfig.json
npx oxlint apps/desktop/src packages/ui/src
npx oxfmt --check <every file you changed>
```

Browser check with `agent-browser --session found1` against
`http://localhost:1420/?f=1`. Screenshots and helper `.sh` scripts go in
`/private/tmp/claude-501/-Users-sergiydybskiy-src-agent-studio--claude-worktrees-shared-core-primitives/9f53962b-6417-4cd1-9df0-6b7fe02c976e/scratchpad`.
Do not pass inline JS with quotes on the Bash command line; write a `.sh` file
and run it with `sh`.

1. Skills view: press Tab until a row glyph or ⋯ button has focus; screenshot
   `found-focus-row.png`. Eval its computed `outline-color` and report it.
2. Tab to a kit Button in the toolbar; screenshot `found-focus-button.png`;
   report computed `box-shadow` or outline.
3. Report the resolved `--default-transition-duration` on `:root`, and the
   `transition-property` of a kit Button.
4. Open a row ⋯ menu on the skill page, press Escape: the menu closes and the
   page stays (h1 unchanged). Press Escape again: goes back to Skills.
5. Home: screenshot `found-home.png`; toggle a group header, confirm it opens
   instantly (no `transition` on the panel height).
6. Section C radius values.
7. Console: report every error verbatim.

Report: files changed, check output verbatim on failure, eval values,
screenshot paths, choices made in B6 and B9, and anything skipped with why.
