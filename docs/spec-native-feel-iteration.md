# Native feel: next UX iteration (2026-09-12)

The app has the fundamentals of a native macOS app: overlay title bar, drag region,
system font with antialiasing, stable scrollbar gutter, focus rings, reduced-motion
support. It still reads as a web page inside an app shell. This plan closes that gap,
in the order that gives the most feel per change. Linear is the reference; its
screens were reviewed on Mobbin.

## Verification first: the browser harness

Tauri on macOS has no WebDriver or DevTools Protocol, so an agent cannot drive the
built app. The frontend therefore gets a harness mode:

- When the page runs without Tauri (`window.__TAURI_INTERNALS__` absent) in dev,
  `mockIPC` from `@tauri-apps/api/mocks` answers every `invoke` from a recorded
  snapshot fixture, and `mockWindows` supplies a window.
- The fixture is built by `apps/desktop/src/dev/harness/skill-fixture.ts`: about 30
  skills under a demo home, covering every row state (parked, disabled own copy,
  broken link, update, trial, drift, plugin, manual) and worst-case content. The
  marketing capture page shares the same fixture and mock.
- Mutations (park, unpark, remove, install, edit) update the in-memory snapshot and
  emit the same events the Rust side emits, so flows can be walked end to end.
- Later, issue #75 (transport-neutral client) swaps the fixture for the live core
  server, and the same harness becomes true end-to-end.
- Release regression runs on Linux CI with `tauri-driver` and WebdriverIO.

## 1. Shell behaviour (one PR)

- `select-none` on all chrome: sidebar, headers, toolbars, list rows, menus.
  `select-text` on content: descriptions, paths, code, markdown.
- Cursor: arrow on controls, hand only on links. Remove the global
  `cursor: pointer` on buttons.
- Suppress the WebKit context menu outside inputs and text areas.
- Block reload (Cmd+R), zoom (Cmd+plus/minus/0) and pinch zoom.
- `overscroll-behavior: none` on the app root.
- Markdown links open through the opener plugin, never inside the webview.
  Follow-up: no opener plugin is installed yet (only `tauri-plugin-dialog`);
  external `http(s)://` links currently render with `target="_blank"` instead.
  A proper opener requires the user to run
  `npm install @tauri-apps/plugin-opener`, which touches the lock file.

## 2. Frame

- Content is an inset, rounded, bordered panel on a slightly darker shell ground.
- Sidebar: switcher row with search and new-skill icons, small section headers,
  counts on the right, 26px items. The full-width search input and primary button
  go.
- Page header: one slim bar with breadcrumb and actions. Filters move to a thin tab
  row under it. No page-level h1.

## 3. Lists

- Home adopts the full Stack row (`docs/skill-row-design-decision.md`).
- Skills groups rows by state under collapsible headers with counts: Needs
  attention, Healthy, Parked.
- Column headers go. Rows tighten to 32px. Select mode appears on checkbox hover.

## 4. Keyboard

- Cmd+K palette over skills, views and actions.
- Arrow keys and j/k move the row cursor; Enter opens; Escape goes back.
- Shortcut hints in the row menu and in sidebar tooltips.

## 5. Detail

- Properties rail on the right: Location, Harnesses, Invocation, Source, Tokens,
  each editable in place. The assistant stays a drawer.

## 6. Settings

Decided on 2026-09-16 from the `#proto-settings` prototype, which had three variants.

- Direction: **Cards**. Each topic gets one bordered card with an icon, a title, one sentence that says what the setting does, and the control.
- Rejected: **Rows**. The whole page fit on one screen, but there was no room to explain a setting. The editor list also collapsed into a select, so you could not see which editors are installed.
- Rejected: **Sections**. It added a second navigation column beside the app sidebar, and with four sections that column was three-quarters empty.
- Project folders card, changed from the prototype:
  - Each row says where the folder came from: found in harness history, or added by you. The row action depends on this. A found folder offers "Stop tracking", because it would otherwise come back on the next scan. A folder you added offers "Remove".
  - A folder you added that no longer exists stays in the list, marked "Folder not found", so it does not disappear without a trace.
  - The empty state names the harness histories that were searched. Adding a folder by hand is the alternative, not the first step.
  - The per-harness search switches are on this card, because they decide what the list contains.
  - The row action is always visible, not only on hover, so keyboard users can reach it.
