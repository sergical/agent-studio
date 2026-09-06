# Home Dashboard

Central command center showing skill health, usage analytics, and actionable inbox of issues, updates, and unused skills.

## Sub-features

**Stat Tiles (3 clickable filters)**:
- **Broken** (red accent when count > 0) - Dead links, rejected SKILL.md, parked-but-reinstalled
- **Warnings** (yellow accent when count > 0) - Copies differ, lock-file-only entries
- **Updates** (neutral) - Skills with newer upstream commits available
- Each tile: heading, count (large display font), InfoPopover with explainer + "Learn more" link to Learn view
- Clicking tile filters view (sets `filter` state, shows matching group only)

**Invocation/Cost Lane Card** (2 segmented bars):
- **Who can invoke** - Horizontal bar split by policy:
  - "You or the model" (accent-soft) - `invocation: "both"` (default)
  - "Model only" (accent-softer) - `invocation: "model-only"`
  - "You only" (bg-tertiary) - `invocation: "user-only"`
  - Each segment clickable → navigates to Skills view with invocation filter
  - Total count shown with InfoPopover
- **Prompt cost** - Horizontal bar split by usage:
  - Used in 30d (accent-soft) - Skills invoked recently, their description tokens
  - Not used in 30d (bg-tertiary) - Idle skills still in prompt
  - Shows token counts (formatted, e.g. "2.4k"), skill counts
  - Clicking "not used" segment toggles unused filter (sets `filter: "unused"`)
  - Clicking "used" segment → Skills view with `usage: "used-30d"`

**Inbox Groups** (5 collapsible sections with Collapsible component):
1. **Broken** - `severity: "error"` issues, each row: dot (error red), skill name + harness badges, issue detail, "Fix"/"Open" action
2. **Warnings** - `severity: "warning"` issues, actions: "Compare" (duplicates), "Convert to per-skill links" (linked root), "Open"
3. **Updates** - Skills with `update_commit` field set, shows commit range (e.g. `abc123 → def456`), token count, "Pull latest" button per row, "Update all" in group header
4. **Not used in 30 days** - Skills with no invocations in 30d AND `invocation !== "user-only"`, shows scope + install date, "Park" button (model-invocable only) or "Open"
5. **Recently used** - Last 5 skills (`recentlyUsedSkills()` helper, `RECENTLY_USED_COUNT = 5`), shows project label + relative time, usage count (not a button, just display text)

**Group Interactions**:
- Chevron icon rotates on expand/collapse (CSS transition)
- Collapsed groups remember state via `collapsedGroups` Set in component state (default: `new Set(["unused"])` - unused starts collapsed)
- Max 6 rows per group (`MAX_ROWS_PER_GROUP = 6`), "Show all N" footer link when truncated → navigates to Skills view with matching filter
- Groups hidden when empty UNLESS `filter` is set (filtered view shows even empty groups)

**Filter Interaction**:
- One stat tile active at a time (clicking same tile twice clears filter)
- Active filter shows "Showing one group" banner with "Show everything" link to clear
- Groups not matching filter are hidden (DOM removed, not just CSS hidden)
- "All clear" message when `broken.length === 0 && warnings.length === 0 && updates.length === 0` and no filter

**Dialogs/Toasts**:
- **MaterializeRootDialog** - Triggered from Warnings row action "Convert to per-skill links" for `kind: "linked-root"` issues, shows harness name + root path, runs `materializeHarnessRoot()` command, recorded in Activity, can be undone
- **Trial restore toast** - Emitted by backend on `skills://trial-expired` event, shows skill name + trash path, "Restore" action button calls `restoreTrashedSkill()`, 15s duration

**Empty/First-Run**:
- No skills → "No skill snapshot yet." or empty stat tiles (0/0/0), empty inbox, prompt to install
- All clear → "All clear. Nothing needs attention." message in main area

## How to get to it (user POV)

**Default view** - App launches to Home (no navigation needed).

From any other view:
1. Click "Home" in sidebar (house icon + text)
2. View loads with stat tiles, lane card, inbox

## Driving it with Playwright

```typescript
// Navigate to Home
await page.goto('http://localhost:1420');
await page.waitForSelector('button:has-text("Home")');
await page.click('button:has-text("Home")');

// Verify stat tiles
const brokenTile = page.locator('button:has-text("Broken")').first();
await expect(brokenTile).toBeVisible();
const brokenCount = await brokenTile.locator('.text-display').textContent();
console.log(`Broken: ${brokenCount}`);

// Click stat tile to filter
await brokenTile.click();
await expect(page.locator('text=Showing one group')).toBeVisible();

// Clear filter
await page.click('button:has-text("Show everything")');

// Interact with invocation bar segment
const bothSegment = page.locator('button:has-text("you or the model")');
if (await bothSegment.isVisible()) {
  await bothSegment.click();
  // Should navigate to Skills view
  await page.waitForSelector('text=Skills');
}

// Expand/collapse inbox group
const brokenGroup = page.locator('button:has-text("Broken")').first(); // Group header
await brokenGroup.click(); // Toggle
await page.waitForTimeout(300); // Animation

// Click row action ("Pull latest" in Updates)
const updateRow = page.locator('text=my-skill').locator('..').locator('button:has-text("Pull latest")');
if (await updateRow.isVisible()) {
  await updateRow.click();
  await page.waitForSelector('[role="status"]:has-text("Skill updated")');
}

// Park skill from unused group
const parkButton = page.locator('button:has-text("Park")').first();
if (await parkButton.isVisible()) {
  await parkButton.click();
  await page.waitForSelector('[role="status"]:has-text("Parked")');
}

// Open skill from inbox row (click skill name button)
const skillName = page.locator('[data-group="broken"] button:has-text("my-skill")'); // Adjust selector
await skillName.click();
await page.waitForSelector('text=Locations'); // Skill detail page
```

Selectors:
- Stat tiles: `button:has-text("Broken")`, `button:has-text("Warnings")`, `button:has-text("Updates")`
- Lane segments: `button:has-text("you or the model")`, `button:has-text("model only")`, etc.
- Group headers: `button` within `[data-group="broken"]`, `[data-group="warn"]`, etc.
- Row skill names: `button` elements within inbox rows (rendered as buttons for click)
- Action buttons: `button:has-text("Pull latest")`, `button:has-text("Park")`, `button:has-text("Compare")`
- "Show all" links: `button:has-text("Show all")` within group footer
- Filter banner: `text=Showing one group`, `button:has-text("Show everything")`

## Gotchas

- **Stat tile vs group** - Clicking tile sets filter, clicking group header expands/collapses (different behaviors)
- **Updates "Update all"** - Button in group header runs sequential updates (can take 10-30s for many skills), shows "Updating…" text
- **Park restrictions** - Only model-invocable skills (`invocation !== "user-only"`) show "Park" button; user-only skills show "Open" instead
- **Trial restore timing** - Toast appears 24h after install (backend timer), not immediately controllable from UI
- **Materialize confirmation** - Backend shows native dialog before converting linked root (Tauri `ask()`, not dismissible via Playwright)
- **Filter persistence** - Filter state (`HomeFilter`) resets on view change (not persisted across nav)
- **Group expansion** - `collapsedGroups` state survives filter changes (doesn't auto-expand when filtering)
- **Row actions async** - "Pull latest", "Park", "Compare" all trigger async operations, wait for toast before asserting result
- **Recently used "action"** - Not a button, just displays count (e.g. "5 uses"), no click handler

## Branches

### Happy path: View dashboard, filter, take action
1. User lands on Home → stat tiles show counts (may be 0), lane bars render, inbox groups load
2. User clicks "Broken" tile → `filter` set to "broken", only Broken group visible, banner shows "Showing one group"
3. User clicks "Show everything" → filter cleared, all groups visible
4. User expands "Updates" group → chevron rotates, rows visible
5. User clicks "Pull latest" on skill → spinner shows, toast "Skill updated" appears, skill disappears from Updates group
6. User clicks "Park" on unused skill → toast "Parked <name>", skill moves to parked scope, disappears from Unused group

### Empty / first-run states
- **No skills installed** → Stat tiles show 0/0/0, inbox empty, "No skill snapshot yet." or empty state prompt
- **All healthy** → Broken=0, Warnings=0, Updates=0, "All clear" message, only Recently Used group shows (if any invocations exist)
- **No invocations yet** → Recently Used group empty, shows "No activity yet" or hidden entirely

### Duplicate / conflict states
- **Parked-but-reinstalled** → Shows in Broken group with "Parked skill reinstalled" detail, action "Open" (to inspect)
- **Copies differ** → Shows in Warnings group with "Copies differ" detail, action "Compare" (opens SkillCompareDialog)
- **Linked root + skill-specific toggle** → Warning row "Convert to per-skill links" opens MaterializeRootDialog

### Failure / error states
- **Update failure** → Toast "Couldn't update skill: <error>", skill stays in Updates group
- **Park failure** → Toast "Couldn't park skill: <error>" (e.g. skill not in shared folder, already parked)
- **Restore failure** → Toast "Couldn't restore skill: <error>" (trash path invalid, already exists)
- **Materialize failure** → Toast "Couldn't convert root: <error>", dialog closes, no change
- **Update all partial failure** → Toast "Updated N of M skills: X failed", groups remain for failed skills

### Cancellation / close mid-flow
- **Cancel materialize dialog** → Native dialog "Cancel" button, no conversion, no Activity event
- **Close during update all** → Updates continue in background (no cancellation mechanism), toasts show results
- **Filter then navigate away** → Filter state discarded (not persisted to store)

### Loading / progress states
- **Initial snapshot loading** → `isLoading: true`, skeleton shows (see HomeSkeleton component: animated bars for tiles/lane/rows)
- **Update in progress** → "Pull latest" button shows spinner icon, text "Updating…"
- **Update all in progress** → Header button shows "Updating…", disabled, updates run sequentially
- **Park in progress** → "Park" button shows spinner icon
- **Snapshot rebuilding** → Brief loading state as Home data recalculates (usually <200ms, not visible unless slow scan)

## Benchmarks & improvement

### Observable metrics
- **Home render time**: Navigate to Home → all groups visible
  - Measure: Route change → stat tiles + lane card + inbox rendered
  - Target: < 300ms for typical install (< 100 skills)
- **Stat tile click latency**: Click → filtered view shown
  - Measure: Tile click → groups update (DOM reflow)
  - Target: < 50ms (synchronous filter, no API call)
- **Update skill duration**: "Pull latest" click → toast
  - Measure: Button click → success/error toast
  - Target: < 5s for dotagents `sync`, < 10s for skills-sh re-install, < 3s for fork pull
- **Update all duration**: Header button click → all toasts complete
  - Measure: Click → last success/error toast
  - Target: ~5s per skill average (sequential, no parallelization)
- **Park duration**: Button click → toast
  - Measure: Click → success toast
  - Target: < 500ms (file move)

### Current instrumentation
- `isLoading` flag tracks initial snapshot load (no timing logged)
- Toast on success/failure (no duration metrics)
- Update all shows final count in toast ("Updated N of M")
- No per-group render timing or expansion/collapse latency

### Suggested measurements for verification
- **Stat tile responsiveness**: Measure click → `filter` state update → DOM reflow (track with React DevTools or perf trace)
- **Group expansion timing**: Measure click → CSS animation complete (should be ~200-300ms per `transition-transform`)
- **Action success rate by type**: Track Pull/Park/Compare actions, categorize errors (network, git, filesystem)
- **Update all completion rate**: Percentage of skills updated successfully in batch operation
- **Trial expiry rate**: How many trials expire vs get kept (product metric for trial feature)

### Improvement levers
- **Parallel update all** (currently sequential to avoid lock-file races; could lock per-skill or batch via backend)
- **Virtualize long inbox groups** (currently renders all rows; 100+ broken skills would lag; use `react-window` for groups > 20 rows)
- **Memoize group computations** (currently recalculates `attentionGroups()`, `skillsWithUpdates()`, etc. on every render; memo selectors)
- **Debounce filter changes** (currently synchronous; rapid tile clicks could thrash; 50ms debounce would smooth)
- **Prefetch linked-root data** (MaterializeRootDialog could preload harness root info while dialog animates in)
- **Cache recently-used calculation** (currently rescans invocations on every render; cache per snapshot)

## Observable end state

After each interaction:

- **Navigate to Home**: Stat tiles show counts, lane card shows bars with counts, inbox groups render (collapsed/expanded per state)
- **Click stat tile**: Filter banner appears ("Showing one group"), only matching group visible, tile shows active state
- **Clear filter**: Banner disappears, all groups visible, no tile highlighted
- **Expand group**: Chevron rotates 90°, rows visible, animation smooth
- **Pull latest**: Button shows spinner → toast success → skill disappears from Updates group → stat tile count decrements
- **Park skill**: Button shows spinner → toast success → skill disappears from Unused group → appears in Skills view with `scope: "parked"`
- **Update all**: Header button shows "Updating…" → series of toasts (one per skill or batched) → final summary toast
- **Compare copies**: SkillCompareDialog opens, shows side-by-side diff of multiple deployments
- **Convert linked root**: MaterializeRootDialog opens → native confirmation → toast success → harness root materialized → warning disappears

Invariants:
- Stat tile count === number of items in corresponding group (Broken tile count matches Broken group row count)
- Lane bar segment width proportional to skill count (CSS flex with dynamic ratios)
- Groups never show duplicate rows (each skill appears once per inbox section max)
- Filter clears on view change (Home → Skills → Home resets filter)
- Group expansion state survives filter toggle (expanding Broken, filtering to Warnings, clearing filter → Broken still expanded)
- Action buttons disable during operation (no double-click races)
