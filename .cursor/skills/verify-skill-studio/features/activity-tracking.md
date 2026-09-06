# Activity Tracking

Invocation analytics and event history: GitHub-style heatmap, per-skill/project breakdowns (configurable windows), and full restorable event log.

## Sub-features

**Invocation Heatmap** (InvocationHeatmap component):
- **52-week × 7-day grid** - GitHub contribution graph style, oldest week left, newest right
- **Cell color intensity** - 5 levels (0-4) relative to max day count in range:
  - 0% = bg-tertiary (no activity)
  - 1-25% = 25% accent mix
  - 26-50% = 50% accent mix
  - 51-75% = 75% accent mix
  - >75% = 100% accent
- **Hover tooltips** - "N invocation(s) · Month DD" (native title attr, no custom tooltip)
- **Month labels** - Above first week of each month, span width = weeks in that month
- **Weekday labels** - Left gutter: Mon / Wed / Fri (empty strings for others, spacing only)
- **Date range** - 364 days (52 weeks), anchored to `snapshot.scanned_at` UTC midnight, recalculates when UTC date key changes
- **Header count** - "N invocations in the last year" (sums same `dates` array as grid)
- **Data source** - Claude Code transcripts only (subtitle: "Codex, OpenCode and pi are not tracked yet.")

**By Skill Table**:
- **Window selector** (WindowSegmentedControl) - 24h / 7d / 30d (shared state: `usageWindow` in appStore)
- **Filters skills** - Only shows skills with invocations > 0 in current window (empty = "No invocations in the last N")
- **Sorting** - `topSkills()` helper: descending by usage count in window, then alphabetical by name
- **Columns** (grid layout, no headers):
  - Skill name (button, clickable → opens skill detail)
  - Last used (relative time, e.g. "2 hours ago", or "never" if `last_used` null)
  - Invocations (count for current window, tabular nums, right-aligned)
  - Projects (count of unique projects where skill was invoked in 30d, regardless of current window)
- **Row action** - Click anywhere on row → `onSelectSkill(name)` → skill detail page

**By Project Table (30d only)**:
- **Label** - "By project, 30 days" (not configurable window, always 30d)
- **Sorting** - Descending by count, then alphabetical by full path
- **Rows**:
  - Project label (basename of path, e.g. "/Users/x/my-app" → "my-app")
  - Count (total invocations across all skills in this project, 30d)
  - Tooltip (full path on hover, mono font)
- **No click action** - Rows are divs, not buttons (display-only)

**History Section** (SkillHistorySection component):
- **Event Log** - One row per event from event store (`~/.agents/.skill-event-store.json`), newest first (up to `MAX_EVENTS = 200` loaded)
- **Row data**:
  - Icon (per event kind: Undo2 for restore, Link2Off for unlink, Archive for move-aside, etc., colored by status: error=red, interrupted=yellow, success=tertiary)
  - Skill name or harness label (if event has no skill, shows harness or kind label)
  - Kind label (e.g. "unlink harness", "explode shared dir", "move aside disable")
  - Harness label (if applicable)
  - Relative time (e.g. "2 hours ago")
  - Status badge (only shown for failed/interrupted: "failed" red, "interrupted" yellow)
- **Row actions** (right side):
  - **Restore button** (restorable events only: unlink, explode, move_aside_disable, etc.) - Shows Undo2 icon, ghost style
  - **Reveal in Finder button** (events with `backup_path`) - Shows Folder icon, ghost style
- **Restore flow**:
  1. Click "Restore" → confirmation dialog (native Tauri `ask()`, shows restore description)
  2. If confirmed → backend runs `restoreSkillEvent(id, force=false)`
  3. If drift detected (file changed since backup) → shows drift warning dialog with "Restore anyway" option
  4. If "Restore anyway" → re-runs with `force=true` (backs up current content first, then restores)
  5. Success → toast "Restored", event gets new status, list reloads
  6. Failure → toast error
- **Empty state** - "No history yet" when event list empty
- **Collapsible** - History section can expand/collapse (default expanded)

**Empty/First-Run States**:
- **No invocations** → "No invocations recorded yet." (subtitle still mentions Claude Code only)
- **Heatmap all zeros** → Grid renders but all cells bg-tertiary
- **By Skill empty for window** → "No invocations in the last N" message
- **By Project always zero** → Section hidden (only shows if `byProject.length > 0`)
- **History empty** → "No history yet" message

## How to get to it (user POV)

1. Click "Activity" in sidebar
2. Page loads with heatmap (top), By Skill table (middle), By Project table (if applicable), History section (bottom)

**Direct navigation** - No deep links or filter presets (unlike Skills view from Home inbox)

## Driving it with Playwright

```typescript
// Navigate to Activity
await page.goto('http://localhost:1420');
await page.click('button:has-text("Activity")');
await page.waitForSelector('text=Activity', { timeout: 5000 });

// Verify heatmap renders
const heatmap = page.locator('[role="img"][aria-label*="Invocations per day"]');
await expect(heatmap).toBeVisible();
const yearTotal = await page.locator('text=/\\d+ invocations in the last year/').textContent();
console.log(`Year total: ${yearTotal}`);

// Hover over heatmap cell (tooltip via title attr)
const firstCell = heatmap.locator('div[title]').first();
const cellTitle = await firstCell.getAttribute('title');
console.log(`First cell: ${cellTitle}`); // e.g. "5 invocations · Jan 3"

// Change usage window (By Skill table)
await page.click('button:has-text("7d")'); // Or "24h" or "30d"
await page.waitForTimeout(100); // Synchronous filter, no API call
const skillCount = await page.locator('text=By skill').locator('..').locator('button').count();
console.log(`Skills used in 7d: ${skillCount}`);

// Click skill row → opens detail
const firstSkillRow = page.locator('text=By skill').locator('..').locator('button').first();
const skillName = await firstSkillRow.locator('span').first().textContent();
await firstSkillRow.click();
await page.waitForSelector('text=Locations'); // Skill detail page
await expect(page.locator(`text=${skillName}`)).toBeVisible();

// Back to Activity
await page.click('button:has-text("Activity")');

// Check By Project table
const projectRow = page.locator('text=By project').locator('..').locator('div').first();
if (await projectRow.isVisible()) {
  const projectLabel = await projectRow.locator('span').first().textContent();
  console.log(`Top project: ${projectLabel}`);
}

// Expand History section (if collapsed)
const historyHeader = page.locator('button:has-text("History")');
if (await historyHeader.isVisible()) {
  await historyHeader.click(); // Toggle
}

// Restore an event
const restoreButton = page.locator('button[aria-label="Restore"]').first();
if (await restoreButton.isVisible()) {
  await restoreButton.click();
  // Native dialog appears (not controllable via Playwright without mocks)
  // If confirmed, wait for toast
  await page.waitForSelector('[role="status"]:has-text("Restored")');
}

// Reveal backup in Finder
const revealButton = page.locator('button[aria-label="Reveal in Finder"]').first();
if (await revealButton.isVisible()) {
  await revealButton.click();
  // Opens native file manager (not verifiable via Playwright)
}
```

Selectors:
- Heatmap: `[role="img"][aria-label*="Invocations per day"]`
- Heatmap cells: `div[title]` within heatmap
- Window buttons: `button:has-text("24h")`, `button:has-text("7d")`, `button:has-text("30d")`
- By Skill rows: `button` elements under "By skill" section
- By Project rows: `div` elements under "By project" section (not buttons)
- History rows: `[data-event-id]` or by skill name
- Restore button: `button[aria-label="Restore"]` within event row
- Reveal button: `button[aria-label="Reveal in Finder"]` within event row

## Gotchas

- **Claude Code only** - Invocations from Codex/OpenCode/pi not tracked (subtitle warning, but heatmap doesn't distinguish)
- **Heatmap date range** - 364 days from `scanned_at` midnight UTC, not "from now" (can lag if snapshot stale)
- **Window selector shared** - Changing window in Activity also changes window in Home lane card (same store slice)
- **By Project always 30d** - No window selector for projects, hardcoded to 30d (different from By Skill)
- **History MAX_EVENTS** - Only loads last 200 events (older ones still in JSON but not shown)
- **Restore confirmation native** - Tauri `ask()` dialog, not dismissible via Playwright (use mocks for automated tests)
- **Drift guard dialog** - Second confirmation if content changed, also native (blocks restore unless forced)
- **Restore button conditional** - Only shows for restorable events (some event kinds like "relink" are not restorable)
- **Reveal button conditional** - Only shows if `backup_path` set (not all events have backups)
- **Event status colors** - Failed = red bg, interrupted = yellow bg, success = transparent (status badge also shows text)
- **History reload after restore** - Component refetches event list after successful restore (no optimistic update)

## Branches

### Happy path: View analytics, change window, restore event
1. User clicks Activity → heatmap renders, By Skill shows 30d (default), By Project shows 30d, History shows events
2. User hovers heatmap cell → tooltip shows date + count
3. User clicks "7d" window → By Skill filters to last 7 days, table updates (some skills may disappear)
4. User clicks skill row → skill detail opens
5. User clicks Back → returns to Activity
6. User clicks "Restore" on event → confirmation dialog → confirms → backend restores → toast success → event status updates

### Empty / first-run states
- **No invocations** → Heatmap empty (all zeros), By Skill empty, By Project hidden, History may have non-invocation events
- **No invocations in window** → "No invocations in the last N" message under By Skill
- **No events** → History shows "No history yet"
- **By Project zero** → Section not rendered (conditional in component)

### Duplicate / conflict states
- **Same project basename** → By Project rows use full path as key, different rows for `/Users/a/app` and `/Users/b/app`
- **Skill invoked in multiple projects** → By Skill "Projects" column sums unique projects across all invocations
- **Restore already-restored event** → Backend error "Event already restored", toast error

### Failure / error states
- **Restore failure (file missing)** → Toast "Restore failed: backup not found"
- **Restore failure (drift detected, user cancels)** → Dialog dismissed, no restore, no error toast
- **Restore failure (filesystem error)** → Toast "Restore failed: <error>"
- **Reveal failure (path invalid)** → Toast "Couldn't reveal in Finder: <error>"
- **History load failure** → Empty state or error message (backend returns empty array on error)

### Cancellation / close mid-flow
- **Cancel restore confirmation** → Dialog dismissed, no restore, no backend call
- **Cancel drift warning** → Dialog dismissed, no force-restore
- **Navigate away during restore** → Restore continues in background (async, no cancellation), toast may show after nav

### Loading / progress states
- **Initial snapshot loading** → Parent `isLoading` flag, Activity shows skeleton or spinner
- **Restore in progress** → "Restore" button shows spinner icon, disabled, text "Restoring…"
- **History loading** → Brief loading state on mount (< 200ms typically), no dedicated spinner
- **Window change** → Synchronous filter (no loading state, instant table update)

## Benchmarks & improvement

### Observable metrics
- **Activity page render time**: Nav click → heatmap + tables visible
  - Measure: Route change → all sections rendered
  - Target: < 400ms for typical snapshot (< 100 skills, < 1000 invocations)
- **Heatmap render time**: Component mount → 364 cells visible
  - Measure: Dates prop received → grid complete
  - Target: < 150ms (simple div grid, no complex rendering)
- **Window change latency**: Button click → table updates
  - Measure: Click → By Skill rows reflow
  - Target: < 50ms (synchronous filter, no API call)
- **Restore duration**: Button click → success toast
  - Measure: Confirmation → toast appears
  - Target: < 1s for simple restore (file move), < 3s for force-restore (backup + restore)
- **History load duration**: Component mount → event rows visible
  - Measure: Backend call → list rendered
  - Target: < 300ms for 200 events

### Current instrumentation
- Heatmap total count visible in UI ("N invocations in the last year")
- Window selector state persisted in store
- Restore button shows spinner during operation
- No timing logs, per-event restore metrics, or heatmap render perf

### Suggested measurements for verification
- **Heatmap cell count accuracy**: Verify 364 cells (52 weeks × 7 days) always render
- **Window filter correctness**: Compare table counts vs raw invocation data for each window
- **History pagination boundary**: Load exactly 200 events, verify oldest cutoff
- **Restore success rate**: Track restore vs drift-guard refusal vs other errors
- **Drift guard accuracy**: Verify false-positive rate (file changed but restore should work)

### Improvement levers
- **Virtualize heatmap** (currently renders 364 divs; 52 × 7 grid is fast, but 100-week grids would lag; not needed yet)
- **Memoize heatmap intensity** (currently recalculates level for all 364 cells on every render; useMemo keyed by dates + heatmap)
- **Cache By Skill sorting** (currently re-sorts on every render; memo sorted array)
- **Debounce window changes** (currently synchronous; rapid clicks would thrash; 50ms debounce would smooth, but not needed for 3 buttons)
- **Lazy-load History** (currently loads all 200 events on mount; could defer until section expanded)
- **Batch restore operations** (currently one-at-a-time; multi-select + "Restore selected" would speed bulk ops)
- **Prefetch backup metadata** (currently reads on Restore click; could preload existence/size during render for better UX)

## Observable end state

After each interaction:

- **Navigate to Activity**: Heatmap renders with 52 × 7 grid, year total shown, By Skill table shows skills used in current window, By Project shows 30d totals, History section shows event log
- **Hover heatmap cell**: Tooltip shows "N invocation(s) · Month DD" (native title attr)
- **Click window button**: By Skill table filters to new window, some skills may appear/disappear, count updates
- **Click skill row**: Skill detail page opens, showing selected skill
- **Click Restore**: Confirmation dialog → confirm → spinner shows → toast success → event status updates to "restored" or similar
- **Drift warning**: Second dialog appears → "Restore anyway" → force-restore runs → success toast
- **Cancel restore**: Dialog dismissed, no backend call, event unchanged
- **Click Reveal**: Native file manager opens to backup path (not verifiable via Playwright)

Invariants:
- Heatmap year total === sum of all `heatmap.days` values in `dates` range (header, grid, aria-label all use same range)
- By Skill window filter === store `usageWindow` (shared with Home lane card)
- By Project always 30d (no window selector, hardcoded logic)
- History shows max 200 events, newest first
- Restore button only on restorable events
- Reveal button only when `backup_path` exists
- Event row colors match status (failed=red bg, interrupted=yellow bg, else transparent)
- Clicking skill name in By Skill → opens detail for that exact skill name (not filtered by scope)
