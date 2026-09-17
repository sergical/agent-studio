# Spec: Detail (step 5 of docs/spec-native-feel-iteration.md)

Read first: the "Detail (step 5)" requirements in
`docs/audit-motion-and-accessibility.md`. Steps 2-4 are applied: `PageShell`
has `parent`, `actions` and `toolbar`; `Kbd` and `TooltipControl`'s
`shortcut` prop exist.

Work in `/Users/sergiydybskiy/src/agent-studio/.claude/worktrees/shared-core-primitives`
(branch `dev/browser-harness`). Do not commit, do not touch any `package.json`
or the lock file, do not start a dev server (http://localhost:1420 is up),
never use `git stash`. Do not add Rust commands or new Tauri IPC.

Goal: the skill page reads like a Linear issue: the content on the left, a
properties rail on the right, actions in the header bar.

## 1. Layout (`SkillDetail/SkillPage.tsx`)

- Move the actions cluster from `InstalledSkillHeader` (primary action,
  assistant toggle, `MenuControl` ⋯) into `PageShell`'s `actions`. Remove the
  now-empty actions row from `InstalledSkillHeader`.
- Under the header bar, a two-column layout: main column `minmax(0,1fr)`,
  rail `260px`, `gap-8`. Below 900px window width the rail stacks above the
  main column. Use `width="default"` on `PageShell`.
- Main column, in order: title (`h2`) and description, state chips (parked,
  trial, update, spec notes) and the blocking-violation alert, then
  `SkillLocationsCard`, then `SkillMarkdownCard` / `SkillRepairCard`.
- The rail is `aside aria-label="Properties"`, `sticky top-5 self-start`.
- `InstalledSkillSourceLedger` leaves the header; its facts move into the rail
  (section 2). Delete the component if nothing else uses it.
- The assistant drawer is unchanged.

## 2. Properties rail (`SkillDetail/SkillPropertiesRail.tsx`, new)

Each property is one row: label `text-small text-text-tertiary` (96px column)
and value `text-small text-text-primary`. Row min height 28px, `gap-1`.
A value that can be edited is a ghost `Button` (full width, left-aligned,
`h-7 px-1.5 -mx-1.5`) whose accessible name is "<Label>: <value>, edit".
Read-only values are plain text, selectable (`select-text`).

| Property             | Value shown                                                                    | Edit in place                                                                                                                                                                                                            |
| -------------------- | ------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Location             | "Global", project names, or "Global + 2 projects"; drift shows a warning glyph | No mutation exists. The value is a button that scrolls `SkillLocationsCard` into view and focuses its heading. Name: "Location: …, show locations".                                                                      |
| Harnesses            | `HarnessStack` plus count                                                      | Popover listing every harness the skill can reach, each with a `Switch` calling `setHarnessEnabled` (the same call `SkillLocationScope` uses). Disabled while pending; errors toast.                                     |
| Invocation           | "Both", "User only", "Model only", or "Mixed"                                  | If all deployments share one policy: a `Select` calling `setSkillInvocation` for each deployment path (the same call `SkillLocationsCard` uses). If policies differ: plain text "Mixed" plus a button to show locations. |
| Source               | repo in mono (`getsentry/skills`), provenance label under it                   | Read-only. If an update is available, an "Update" ghost button calls `updateSkill` (reuse the existing handler).                                                                                                         |
| Lifecycle            | owner and managed/unmanaged                                                    | Read-only.                                                                                                                                                                                                               |
| Tokens               | prompt and full, `tabular-nums`                                                | Read-only.                                                                                                                                                                                                               |
| Installed / Modified | dates                                                                          | Read-only.                                                                                                                                                                                                               |

Rules for editable values (from the audit):

- Click or Enter opens the editor and focuses it. Enter or selecting commits;
  Escape cancels. Focus returns to the value button afterwards.
- Saves announce "Saved" and errors announce the message through one
  `role="status"` / `role="alert"` region in the rail.
- Escape inside the rail (popover, select) must not trigger `SkillPage`'s back
  navigation.
- Nothing is editable only on hover.
- Popover and Select use the kit primitives; no new animation.

Reuse the existing data derivations: find where `SkillLocationsCard`,
`SkillLocationScope` and the source ledger compute scope groups, harness reach,
invocation policy and token counts, and call the same helpers. Do not
duplicate that logic; move a helper to a shared module if the rail needs it.

## 3. The "no longer installed" branch

Keep it inside `PageShell` with the breadcrumb and no rail.

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

Browser check with `agent-browser --session detail1` against
`http://localhost:1420/?f=1`. Screenshots and helper `.sh` scripts go in
`/private/tmp/claude-501/-Users-sergiydybskiy-src-agent-studio--claude-worktrees-shared-core-primitives/9f53962b-6417-4cd1-9df0-6b7fe02c976e/scratchpad`.
Write JS with quotes into a `.sh` file and run it with `sh`.

1. Open `commit`: screenshot `detail-commit.png`. Report the rail row labels
   and values, and that the ⋯ menu trigger is inside the header bar.
2. Harnesses: open the popover, toggle one harness off; the harness stack
   updates (the harness mock handles `set_harness_enabled`). Screenshot
   `detail-harness.png`. Escape closes the popover and the page stays.
3. Invocation: change it with the keyboard (Enter, arrow, Enter); report the
   status text and that focus is back on the value button.
4. Location: activate it; `SkillLocationsCard` heading has focus.
5. Resize the viewport to 800px wide: screenshot `detail-narrow.png`; the rail
   stacks above the content.
6. Open a skill with an update available in the fixture; the Update button
   shows in Source.
7. Console: report every error verbatim.

Report: files changed, check output verbatim on failure, eval values,
screenshot paths, helpers moved, and anything skipped or uncertain.
