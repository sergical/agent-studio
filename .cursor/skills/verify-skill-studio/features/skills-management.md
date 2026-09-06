# Skills Management

Unified, filterable list of all installed skills across all agents, scopes, and sources. Supports multi-select for pack creation and toggles between table and coverage matrix views.

## Sub-features

**Filter Bar** (SkillListFilterBar component):
- **Search input** (left, w-60) - Filter by skill name/description, debounced in SkillListTable (no API call, client-side filter)
- **Scope selector** (segmented control + dropdown):
  - All / Global buttons in ToggleGroup
  - Project dropdown (segmented trigger) with MenuControl:
    - Lists `userAddedProjects` from store (basename + short path)
    - "Stop tracking <name>…" action (with confirmation) calls `removeProject()`
    - "Add project…" action opens folder picker (Tauri `open()` dialog)
  - Parked scope (set from sidebar "Parked" row, not shown in this bar)
- **Filter menu** (ListFilter icon + badge count):
  - **Harness** radio group - Any / Claude Code / Codex / OpenCode / pi / shared (only harnesses present in snapshot)
  - **Source** radio group - Any / dotagents / skills.sh / in-repo / manual / fork (no "plugin" - those live in PluginSkillsView)
- **Result count** - "N skill(s)" (right-aligned, tabular nums)
- **Sort selector** - Dropdown: Name / Used (30d) / Cost (tokens), applied by SkillListTable
- **View toggle** (segmented icons) - List (table) / Coverage (LayoutGrid icon, matrix view)

**Active Filter Chips** (second row, shown when `activeFilterCount(filter) > 0`):
- One chip per active filter (scope, harness, source, issue, invocation, usage)
- Each chip: label + "×" button to clear that filter
- "Clear all" button (text-only, right-aligned) resets entire filter (calls `onReset()`)

**Selection Mode** (SkillListTable, behind `skill-packs` flag):
- **"Select" ghost button** (left of filter bar when not in selection mode) - enters mode, shows header checkbox
- **Header checkbox** - Select all visible / Select none (toggles all rows matching current filter)
- **Row checkboxes** - One per row, keyed by `deployment.path` (not skill name - handles duplicate names across scopes)
- **Shift-click range select** - Tracks `lastCheckedIndexRef`, selects all rows between last clicked and current
- **Selection bar** (replaces "Select" button when in mode):
  - "N selected" count
  - "Cancel" button → exits mode, clears selection
  - "Create pack" button → opens PackNamePrompt dialog, calls `createSkillPack()`
  - Escape key also exits mode

**SkillListTable**:
- **Rows** - One per skill, clicking row:
  - Selection mode OFF → calls `onSelectSkill(name, deploymentPath)`, opens detail
  - Selection mode ON → toggles checkbox
- **Columns** (no table headers, implicit layout):
  - Selection checkbox (only in mode, w-11 gutter)
  - Skill name (button, bold, clickable)
  - One-line description (truncated, text-secondary)
  - Location chips (SkillLocationCell: scope badges, harness icons, disabled states)
  - Invocation chip ("User only" / "Model only" if not default "both")
  - Usage count (30d window, tabular nums, text-tertiary)
  - Token count (SKILL.md description tokens, formatted, text-tertiary)
- **Sorting** - Applied via `sort` prop: "name" (localeCompare), "used" (30d descending), "cost" (tokens descending)
- **Empty states**:
  - No skills at all (`hasAnySkills: false`) → "You haven't added a skill yet" + "Add skill" button
  - Skills exist but filter matches none → "No skills match your filters" + "Clear filters" button

**Coverage Matrix Toggle** (SkillCoverageMatrix component, `showCoverage: true`):
- Replaces table with grid: rows = skills (names), columns = harnesses
- Cells show checkmark if skill deployed to that harness, empty if not
- Clicking cell opens skill detail (same as table row click)
- Same filter/search applies (only shows matching skills)

## How to get to it (user POV)

1. Click "Skills" in sidebar
2. Table loads with all installed skills

**From sidebar search**:
1. Type in sidebar search box → auto-navigates to Skills view with query applied
2. Filter bar search input mirrors sidebar query

**From Home inbox "Show all"**:
1. Home Broken/Warnings/Updates group → "Show all N" footer link
2. Navigates to Skills with matching filter preset (e.g. `issue: "any"`)

## Driving it with Playwright

```typescript
// Navigate to Skills
await page.goto('http://localhost:1420');
await page.click('button:has-text("Skills")');
await page.waitForSelector('table tbody tr, text=No skills'); // Table or empty state

// Search skills
const searchInput = page.locator('input[aria-label="Filter skills"]');
await searchInput.fill('database');
await page.waitForTimeout(100); // Client-side filter, near-instant

// Filter by scope (Global)
await page.click('button:has-text("Global")');
await expect(page.locator('table tbody tr').first()).toBeVisible();

// Filter by project
await page.click('button:has-text("Project")'); // Opens dropdown
await page.click('text=my-app'); // Project name
await expect(page.locator('[data-chip]:has-text("my-app")')).toBeVisible(); // Active chip

// Clear filter chip
await page.click('[data-chip]:has-text("my-app") button'); // × button
await expect(page.locator('[data-chip]:has-text("my-app")')).toBeHidden();

// Open filter menu
await page.click('button:has-text("Filter")');
await page.click('text=dotagents'); // Source filter
await page.click('button:has-text("Filter")'); // Close menu (click outside or ESC)

// Change sort
await page.click('button:has-text("Sort:")'); // Or by aria-label
await page.click('text=Used'); // Sort by usage
// Table should reorder

// Toggle coverage matrix
await page.click('[aria-label="Coverage matrix view"]');
await page.waitForSelector('.coverage-matrix, [data-coverage-grid]'); // Matrix renders
await page.click('[aria-label="List view"]'); // Back to table

// Enter selection mode
await page.click('button:has-text("Select")');
await expect(page.locator('input[type="checkbox"]').first()).toBeVisible(); // Header checkbox

// Select skills
await page.check('table tbody tr:nth-child(1) input[type="checkbox"]');
await page.check('table tbody tr:nth-child(2) input[type="checkbox"]');
await expect(page.locator('text=2 selected')).toBeVisible();

// Shift-select range
await page.click('table tbody tr:nth-child(1) input[type="checkbox"]'); // First
await page.click('table tbody tr:nth-child(5) input[type="checkbox"]', { modifiers: ['Shift'] }); // Fifth
// Rows 1-5 should be selected

// Create pack
await page.click('button:has-text("Create pack")');
await page.fill('input[placeholder*="pack name"]', 'my-favorites');
await page.click('button:has-text("Create")');
await page.waitForSelector('[role="status"]:has-text("Pack created")');

// Exit selection mode
await page.click('button:has-text("Cancel")'); // Or press Escape
await expect(page.locator('input[type="checkbox"]')).toBeHidden();
```

Selectors:
- Search: `input[aria-label="Filter skills"]`
- Scope buttons: `button:has-text("All")`, `button:has-text("Global")`, `button:has-text("Project")`
- Filter menu: `button:has-text("Filter")` (trigger), menu items by text
- Sort: `button:has-text("Sort:")` (trigger), items by text
- View toggle: `[aria-label="List view"]`, `[aria-label="Coverage matrix view"]`
- Active chips: `[data-chip]` or by text content with × button
- Select button: `button:has-text("Select")`
- Selection bar: `text=N selected`, `button:has-text("Cancel")`, `button:has-text("Create pack")`
- Table rows: `table tbody tr`
- Row checkboxes: `table tbody tr input[type="checkbox"]`
- Skill name buttons: `table tbody tr button` (first button in row)

## Gotchas

- **Selection keying** - Checkboxes keyed by `deployment.path`, not skill name (duplicate names across scopes have different paths)
- **Filter stacking** - All filters AND together (search + scope + harness + source all apply simultaneously)
- **Query in two places** - Sidebar search box and filter bar search input are synced via store (`skillListFilter.query`)
- **Scope vs filter** - Scope is always shown (All/Global/Project buttons), but other filters hide in Filter menu until applied
- **Parked scope** - Not in filter bar buttons, only accessible via sidebar "Parked" row (sets `scope: "parked"` directly in store)
- **Project dropdown edge case** - "Add project" requires native folder picker (not testable via Playwright without mocks)
- **"Stop tracking" confirmation** - Native Tauri `ask()` dialog, not dismissible via Playwright
- **Coverage matrix reuses filters** - Same filter/search/sort apply to matrix view (just different rendering)
- **Selection persists across filter** - Checked rows stay checked even if filtered out (selection keyed by path, not visibility)
- **Selection clears on view change** - Leaving Skills view (e.g. to Home) exits selection mode and clears `selectedSkillPaths`
- **Pack creation async** - "Create pack" calls backend, shows progress in button text ("Creating…"), toast on complete
- **Empty result vs no skills** - Different empty states: `hasAnySkills: false` shows "Add skill" CTA, filtered-to-empty shows "Clear filters"

## Branches

### Happy path: Browse, filter, select, create pack
1. User clicks Skills in sidebar → table loads with all skills
2. User types "data" in search → table filters to matching skills (client-side, instant)
3. User clicks "Global" → only global-scope skills shown
4. User clicks Filter menu → Source → dotagents → filter chip appears, table updates
5. User clicks "Clear all" → all filters reset, full list shows
6. User clicks "Select" → checkboxes appear, selection mode active
7. User checks 3 skills → "3 selected" count shows
8. User clicks "Create pack" → dialog opens, enters "utilities", submits
9. Backend creates pack → toast "Pack created" → pack appears in Packs view
10. Selection mode exits, checkboxes hidden

### Empty / first-run states
- **No skills installed** → Empty state: "You haven't added a skill yet. Install from skills.sh or add your own." + "Add skill" button
- **Skills exist but filtered to zero** → "No skills match your filters" + "Clear filters" button (calls `onClearFilters()`)
- **No projects tracked** → Project dropdown shows "No projects tracked yet" + "Add project…" item
- **Coverage matrix with no skills** → Empty matrix or message

### Duplicate / conflict states
- **Duplicate skill names** → Each deployment is a separate row (e.g. same skill global + project shows twice, different paths)
- **Selected skill then filtered** → Skill hidden but still in `selectedSkillPaths`, count includes hidden selections
- **Pack name collision** → Backend refuses, error toast "Pack name already exists"

### Failure / error states
- **Create pack failure** → Error toast, dialog stays open, error message shown
- **Add project failure** → Error toast (e.g. permission denied, path invalid)
- **Stop tracking failure** → Error toast, project stays in list
- **Search no results** → Table shows "No skills match" empty state (not an error, just zero results)

### Cancellation / close mid-flow
- **Cancel selection mode** → "Cancel" button or Escape key → exits mode, clears selection (`exitSelectionMode()`)
- **Cancel pack creation dialog** → Click outside or Escape → dialog closes, selection mode persists, no pack created
- **Cancel project picker** → Native dialog cancel → returns `null`, no project added
- **Cancel "Stop tracking" confirmation** → Native dialog cancel → project stays tracked

### Loading / progress states
- **Initial snapshot loading** → `isLoading: true` from parent, table shows loading skeleton or spinner
- **Creating pack** → "Create pack" button shows "Creating…" text, disabled
- **Snapshot refresh** → Brief reflow as table data updates (< 200ms typically)
- **Search typing** → No loading state (client-side filter, synchronous)

## Benchmarks & improvement

### Observable metrics
- **Table render time**: Skills nav click → table visible
  - Measure: Route change → `tbody tr` elements rendered
  - Target: < 300ms for 100 skills, < 800ms for 500 skills
- **Filter latency**: Filter change → table updates
  - Measure: Filter click → DOM reflow complete
  - Target: < 100ms for simple filters (scope, harness), < 200ms for complex (search + multi-filter)
- **Search responsiveness**: Keystroke → filtered results
  - Measure: Input change → table rows update
  - Target: < 50ms (synchronous client-side filter)
- **Selection toggle latency**: Checkbox click → visual state change
  - Measure: Click → checkbox checked state + count update
  - Target: < 30ms (store update + re-render)
- **Create pack duration**: Button click → toast
  - Measure: "Create pack" → "Pack created" toast
  - Target: < 2s for typical packs (< 10 skills)

### Current instrumentation
- `isLoading` flag from parent (no per-table timing)
- Selection count visible in UI ("N selected")
- Filter active count badge on Filter menu trigger
- No render timing, filter performance metrics, or search latency logged

### Suggested measurements for verification
- **Table virtualization threshold**: Measure render time for 100, 500, 1000 skills (identify where it slows)
- **Filter combination performance**: Test search + scope + harness + source all active (worst case)
- **Selection shift-click range**: Measure time to select 1-100 rows via shift-click (should be linear, no lag)
- **Coverage matrix vs table render**: Compare render time for same dataset in both views
- **Pack creation size limit**: Test creating pack with 50, 100, 200 skills (identify slow/fail threshold)

### Improvement levers
- **Virtualize table rows** (currently renders all; 500+ rows lag on scroll; use `react-window` or `@tanstack/virtual`)
- **Debounce search input** (currently applies every keystroke; 50-150ms debounce for 1000+ skills would smooth)
- **Memoize filter predicates** (currently recreates filter functions on every render; useMemo would cache)
- **Lazy-load coverage matrix** (currently computes full matrix on mount; could load visible rows only)
- **Batch selection updates** (shift-click currently updates store N times; could batch into single call)
- **Persist selection in URL** (currently store-only; URL params would survive page refresh)
- **Cache sorted arrays** (currently re-sorts on every render; memoize sorted results keyed by sort mode + skills array)

## Observable end state

After each interaction:

- **Navigate to Skills**: Table renders with all skills, filter bar shows "N skills", no active chips
- **Search**: Table filters instantly, only matching rows visible, search input shows query
- **Scope filter**: Active chip appears, table shows only matching scope, button shows selected state
- **Filter menu**: Active filters show badge count on trigger, chips appear in second row
- **Clear chip**: Chip disappears, table expands to include previously-filtered skills
- **Clear all**: All chips disappear, table shows full list
- **Sort change**: Table reorders (Name = alphabetical, Used = descending by 30d count, Cost = descending by tokens)
- **Coverage matrix toggle**: View switches from table to grid, same filters apply
- **Enter selection mode**: Checkboxes appear, selection bar replaces "Select" button, Escape exits
- **Select rows**: Count updates ("N selected"), checkboxes show checked state
- **Shift-select**: Range of rows between clicks all toggle checked
- **Create pack**: Dialog opens → submit → toast → pack created in `~/.agents/packs/<name>` → selection clears → mode exits
- **Exit selection mode**: Checkboxes disappear, selection cleared, "Select" button returns

Invariants:
- Result count always matches visible row count (not total skills, only filtered)
- Active chips always reflect current filter state (no stale chips)
- Selection keyed by path, not name (same skill name in multiple scopes = separate selections)
- Coverage matrix and table share same filter logic (no divergence)
- Filter state synced with sidebar search (typing in sidebar updates filter bar)
- Selection clears on view change (never carries over to Home or other views)
- Escape always exits selection mode (no other key bindings)
