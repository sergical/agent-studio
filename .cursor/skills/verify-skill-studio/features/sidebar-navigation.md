# Sidebar Navigation

Primary navigation chrome for Skill Studio - the left sidebar with search, add skill, view links, and footer controls.

## Sub-features

- **Search box** - Jump to Skills view with a query filter
- **Add skill button** - Opens the Add Skill sheet
- **View navigation** - Home, Skills, Plugins (conditional), Activity, Packs (conditional)
- **Parked section** - Appears when parked skills exist
- **Footer controls**:
  - Rescan button with last-scanned timestamp
  - Learn button
  - Settings button
  - Theme toggle (light/dark)

## How to get to it (user POV)

Always visible - the sidebar is the app's primary chrome, pinned to the left edge.

## Driving it with Playwright

```typescript
// Search
await page.fill('input[aria-label="Search skills…"]', 'my-query');
await page.keyboard.press('Enter');
// Should navigate to Skills view with query applied

// Add skill
await page.click('button:has-text("Add skill")');
await page.waitForSelector('[aria-label="Add skill"]'); // Sheet opens

// Navigate to view
await page.click('button:has-text("Home")');
await page.click('button:has-text("Skills")');
await page.click('button:has-text("Activity")');

// Parked (only if parked skills exist)
await page.click('button:has-text("Parked")');

// Footer actions
await page.click('button:has-text("Scanned")'); // Rescan
await page.click('button[aria-label="Learn"]');
await page.click('button[aria-label="Settings"]');
await page.click('button[aria-label="Switch to dark theme"]'); // Theme toggle

// Clear search with Escape
await page.fill('input[aria-label="Search skills…"]', 'query');
await page.keyboard.press('Escape');
```

Selectors:
- Search input: `input[aria-label="Search skills…"]`
- Add button: `button:has-text("Add skill")`
- View buttons: `button:has-text("Home")`, `button:has-text("Skills")`, etc.
- Parked: `button:has-text("Parked")`
- Rescan: `button:has-text("Scanned")` or `:has-text("Scanning…")`
- Learn: `button[aria-label="Learn"]`
- Settings: `button[aria-label="Settings"]`
- Theme: `button[aria-label="Switch to dark theme"]` or `[aria-label="Switch to light theme"]`

## Gotchas

- **Plugins row** only shows when `pluginCount > 0` (Claude Code/Codex plugin caches have skills)
- **Parked row** only shows when `parkedCount > 0` (at least one skill is parked)
- **Packs row** hidden behind `skill-packs` feature flag (default off)
- Search automatically switches to Skills view when typing (no explicit nav needed)
- Escape in search clears query and blurs field
- Theme button reflects current `resolvedTheme` (light/dark), not the stored `theme` (which can be "system")
- "Scanned X ago" text updates every 30s via a timer (`forceTick`)
- Rescan button shows "Scanning…" with spinning icon while `isRescanning && isLoading`

## Branches

### Happy path
1. User clicks a view → `setActiveView` updates store → `App.tsx` renders new main content
2. User types search → store updates → switches to Skills view with filter
3. User clicks Add skill → sheet opens
4. User clicks theme toggle → theme switches, localStorage persists

### Empty / first-run states
- No parked skills → Parked row not rendered
- No plugins → Plugins row not rendered
- Packs disabled by flag → Packs row not rendered
- Fresh install → "Scanned just now" shows immediately after first scan completes

### Duplicate / conflict
- N/A - sidebar has no duplicate-handling logic

### Failure / error states
- Rescan fails → error toast shown by parent (no inline failure in sidebar)
- Search with no results → handled by Skills view, not sidebar
- Theme toggle failure → silent (localStorage write wrapped in try-catch)

### Cancellation / close mid-flow
- Escape during search → clears query, blurs input, stays on current view
- Search while on non-Skills view → switches to Skills (no cancel mechanism)

### Loading / progress states
- Rescan in progress → "Scanning…" text, spinning `RefreshCw` icon, button disabled
- Initial scan → handled by parent (`isLoading`), sidebar shows "Scanned X ago" once snapshot lands

## Benchmarks & improvement

### Observable metrics
- **Navigation latency**: Time from click to new view rendering (measure via React DevTools or trace `setActiveView` → main render)
- **Search responsiveness**: Keystroke to filter applied in Skills view (< 50ms expected, synchronous store update)
- **Rescan duration**: "Scanning…" start to next snapshot event (backend Rust scan + IPC latency)
- **Theme toggle latency**: Click to CSS variables applied (< 16ms expected, synchronous DOM mutation in `stampTheme`)

### Current instrumentation
- Rescan timing visible in footer ("Scanned X ago" relative to `snapshot.scanned_at`)
- No per-nav or per-interaction timing logged
- Snapshot age updates every 30s (polling interval)

### Suggested measurements for verification
- Capture time from navigation click to `activeView` store update
- Measure search filter propagation time (input change → Skills view render with filtered results)
- Track rescan wall time (start button click → snapshot event received)
- Count navigation interactions per session (to validate most-used paths)

### Improvement levers
- **Debounce search input** (currently applies on every keystroke; 150ms debounce would reduce filter thrashing on fast typing)
- **Virtual scrolling** for Parked list if user has 100+ parked skills (not currently an issue, but unbounded)
- **Memoize harness counts** in sidebar (currently recalculates `ownSkillsView` / `pluginSkillsView` on every render; snapshot change should be the only trigger)
- **Lazy-load view components** (all views imported eagerly; code-split Home/Skills/Activity for faster initial load)
- **Cache "Scanned X ago" string** (currently recomputes `relativeScanTime()` every 30s tick; only needs to update when format changes, e.g. "1m" → "2m")

## Observable end state

After each interaction:

- **View navigation**: Main content area shows the selected view (Home/Skills/Activity/etc.), sidebar highlights active item with `bg-accent-soft text-text-primary`
- **Search**: Skills view renders with `skillListFilter.query` set, matching rows visible, sidebar search input retains value
- **Escape in search**: Input cleared, no value in field, filter cleared in store
- **Add skill**: Sheet opens on right side (`AddSkillSheet` visible)
- **Rescan**: Button shows "Scanning…", eventually updates to "Scanned just now"
- **Theme toggle**: App root `data-theme` attribute changes, all CSS vars update, button label switches
- **Learn/Settings**: Respective view opens, footer button shows active state (`aria-current="page"`)

Invariants:
- Exactly one view highlighted in sidebar at a time
- Search query persists across view switches (except when manually cleared)
- Snapshot age never regresses (monotonically increasing, resets on rescan)
- Theme persisted to localStorage, survives reload
