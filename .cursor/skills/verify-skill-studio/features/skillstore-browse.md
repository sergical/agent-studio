# SkillStore Browse

The "Browse skills.sh" tab within the Add Skill sheet - discover, search, and install skills from the skills.sh catalog of 36,000+ community skills.

## Sub-features

- **Search bar** - Query skills.sh catalog (debounced 300ms)
- **Popular skills** - Browse most-installed skills when no search query
- **Pagination** - Load more results (50 per page)
- **Skill cards** - Name, description, install count, installed badge
- **Skill detail panel** - Slides in on card click with full SKILL.md/AGENTS.md body
- **Install from detail** - Agent selector, scope picker, install button
- **Installed vs Browse tabs** - Switch between browsing catalog and viewing installed skills
- **Install progress modal** - Shows when install completes

## How to get to it (user POV)

1. Click "Add skill" in sidebar
2. Click "Browse skills.sh" tab in the sheet
3. OR: Open Add Skill sheet which defaults to manual tab, then switch to Browse tab

## Driving it with Playwright

```typescript
// Open skill store
await page.click('button:has-text("Add skill")');
await page.click('[role="tab"]:has-text("Browse skills.sh")');

// Search
await page.fill('input[placeholder*="Search"]', 'database');
await page.waitForTimeout(350); // Debounce delay
// Results should update

// Browse popular skills (no search)
await page.fill('input[placeholder*="Search"]', '');
await page.waitForTimeout(350);
// Popular skills should show

// Load more (pagination)
const loadMoreButton = page.locator('button:has-text("Load more")');
if (await loadMoreButton.isVisible()) {
  await loadMoreButton.click();
  await page.waitForSelector('.skill-card', { timeout: 5000 });
}

// Open skill detail
await page.click('.skill-card:first-child');
await page.waitForSelector('[data-testid="skill-detail-panel"]');

// Install from detail
await page.click('button:has-text("Install")');
await page.waitForSelector('[role="dialog"]:has-text("Installing")');
```

Selectors:
- Add button: `button:has-text("Add skill")`
- Browse tab: `[role="tab"]:has-text("Browse skills.sh")`
- Search input: `input[placeholder*="Search"]` or by label
- Skill cards: `.skill-card` (CSS class, check actual component)
- Detail panel: Check if `SkillDetailPanel` has `data-testid`
- Install button: `button:has-text("Install")` within detail panel
- Load more: `button:has-text("Load more")`

## Gotchas

- **Search debounce** - 300ms delay before query fires (see `useDebounce` in `SkillSearchBar`)
- **Server dependency** - Browse requires Skill Studio server running (mode: "server") or a skills.sh API key (mode: "direct")
- **Failed fetch** - Shows `BrowseErrorEmptyState` instead of results if server unreachable
- **Empty search** - Search query < 2 chars shows popular skills instead
- **Pagination** - `has_more` from API determines if "Load more" button shows
- **Install progress** - Modal shows immediately, toasts on completion
- **Installed badge** - Skills already installed show checkmark badge
- **Tab switch** - Switching to "Installed" tab shows locally installed skills, not catalog

## Branches

### Happy path
1. User opens Add Skill sheet → defaults to "Add by source" tab
2. User clicks "Browse skills.sh" tab → fetches popular skills
3. User types search query → debounces 300ms → fetches search results
4. User clicks skill card → detail panel slides in
5. User clicks Install → progress modal shows → toast on success → sheet closes

### Empty / first-run states
- No search query → shows popular skills sorted by install count
- No results for query → "No skills found" message
- No installed skills yet → Installed tab shows empty state

### Duplicate / already installed
- Skill already installed → card shows checkmark badge, "Installed" chip
- Install button disabled or shows "Already installed" state

### Failure / error states
- **Server unreachable** → `BrowseErrorEmptyState` with retry button
- **Search API error** → toast notification, old results remain visible
- **Install failure** → error toast with message, modal closes
- **Load more failure** → error toast, pagination button re-enables

### Cancellation / close mid-flow
- Close sheet while browsing → state persists (search query, scroll position)
- Close sheet during install → install continues in background, toast shows result
- Cancel search mid-typing → debounce cancels pending fetch, old results stay

### Loading / progress states
- **Initial load** → "Loading skills…" skeleton cards
- **Search in progress** → spinner in search bar, old results stay visible
- **Loading more** → "Loading more…" text on button, button disabled
- **Install in progress** → progress modal with spinner, "Installing…" text

## Benchmarks & improvement

### Observable metrics
- **Search latency**: Debounce delay (300ms) + API roundtrip + render
  - Measure: Start typing → results appear
  - Target: < 1s total (300ms debounce + 500ms API + 200ms render)
- **Initial load time**: Popular skills fetch + render
  - Measure: Tab switch → results visible
  - Target: < 800ms
- **Pagination latency**: Load more click → new results appended
  - Measure: Button click → new cards rendered
  - Target: < 600ms
- **Install success rate**: Percentage of installs that complete without error
  - Measure: Track install attempts vs completions
  - Target: > 95% (failures usually network or auth issues)

### Current instrumentation
- `isLoading` / `isLoadingMore` flags track fetch state
- Toast notifications on success/failure (no timing logged)
- No per-operation latency metrics
- No search analytics (query frequency, null results, etc.)

### Suggested measurements for verification
- **Search responsiveness**: Measure time from last keystroke to results rendered
- **Pagination performance**: Time from "Load more" click to new cards visible
- **Install duration**: Track `npx skills add` command execution time (backend Rust span)
- **Cache hit rate**: Percentage of searches served from cache vs fresh API calls
- **Error rate by type**: Count server unreachable vs API errors vs install failures

### Improvement levers
- **Increase debounce** to 400-500ms (reduces API load, slightly worse UX for slow typers)
- **Virtual scrolling** for skill cards (currently renders all in DOM; 1000+ results could lag)
- **Prefetch popular skills** on sheet open (even before tab switch; ~300ms saved)
- **Cache search results** per query (currently only caches popular; frequent searches repeat API calls)
- **Optimistic UI for install** (show "Installed" badge immediately, rollback on failure)
- **Parallel install** for multiple selected skills (currently sequential; batched `addSkills` backend call exists but unused in Browse tab)
- **Lazy-load detail panel** (currently loaded on first card click; code-split could save ~20KB)

## Observable end state

After each interaction:

- **Search query**: Results update to match query, card count changes, pagination resets
- **Load more**: New cards append below existing ones, button shows "Load more" or hides if no more results
- **Open detail**: Panel slides in from right, shows skill name + description + full markdown
- **Install**: Progress modal shows → success toast → modal dismisses → sheet closes → sidebar shows incremented skill count
- **Tab switch**: Content area swaps between Browse (catalog) and Installed (local skills)
- **Server error**: `BrowseErrorEmptyState` replaces results, shows retry button

Invariants:
- Search query always visible in search bar (persists across tab switches)
- Installed badge only shows for skills present in `installedSkills` array
- Detail panel only shows for one skill at a time (clicking another card replaces content)
- "Load more" button only visible when `hasMore === true`
- Browse tab always shows skills.sh catalog, never local-only skills
