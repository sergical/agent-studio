# Learn Sections

Four explainer sections accessed from Home's "Learn more" links and sidebar's Learn button - deep-dive documentation on broken/warnings, invocation policy, prompt cost, and unused skills.

## Sub-features

- **Section navigation** - Left sidebar TOC with active indicator
- **Deep-linkable sections** - Can open at specific section (e.g. `{ kind: "learn", section: "invoke" }`)
- **Auto-scroll** - Opening at a section scrolls that heading into view and focuses it
- **Sections**:
  1. **Broken and warnings** - Dead links, rejected SKILL.md, parked-but-reinstalled, copies-differ, lock-only
  2. **Who can invoke** - Harness-by-harness table of explicit and model invocation controls
  3. **Prompt cost** - How name+description tokens add up, why user-only skills cost nothing
  4. **Not used in 30 days** - Transcript-based usage tracking, why only Claude Code is tracked

## How to get to it (user POV)

**From Home**:
1. Click "Learn more →" link below Broken/Warnings stat tiles → opens Learn at "broken" section
2. Click "Learn more →" link in invocation/cost card → opens Learn at "invoke" or "cost" section

**From sidebar**:
1. Click Learn button (book icon) → opens Learn at first section

**From Learn itself**:
1. Click TOC links to jump between sections
2. Back button (← Home) returns to Home view

## Driving it with Playwright

```typescript
// Open Learn from sidebar
await page.click('button[aria-label="Learn"]');
await page.waitForSelector('text=Learn'); // Page heading

// Navigate via TOC
await page.click('a[href="#learn-invoke"]');
await page.waitForSelector('#learn-invoke');

// Deep-link to specific section (programmatic)
// (Requires setting activeView in store, not drivable from UI alone)

// Back to Home
await page.click('button:has-text("← Home")');
await page.waitForSelector('text=Home');
```

Selectors:
- Learn button: `button[aria-label="Learn"]`
- Page heading: `text=Learn`
- TOC links: `a[href="#learn-broken"]`, `a[href="#learn-invoke"]`, `a[href="#learn-cost"]`, `a[href="#learn-unused"]`
- Section headings: `#learn-broken`, `#learn-invoke`, `#learn-cost`, `#learn-unused`
- Back button: `button:has-text("← Home")`

## Gotchas

- **Section focus** - When opened at a specific section, that heading gets `tabIndex={-1}` and focuses programmatically (for screen readers)
- **Active state** - TOC link active state driven by `section === key` (passed as prop, not URL hash)
- **No URL routing** - Sections are in-page anchors (`#learn-broken`), but app uses store-based routing, not URLs
- **Table styling** - "Who can invoke" section has a styled table with harness icons and code examples
- **Content verbatim** - Copy matches `popover-spec.md`'s LEARN object (single source of truth)

## Branches

### Happy path: From Home stat tile
1. User clicks "Learn more" on Broken tile → `setActiveView({ kind: "learn", section: "broken" })`
2. Learn view renders, scrolls to "Broken and warnings" heading, focuses it
3. User reads content, clicks TOC link to "Prompt cost"
4. View scrolls to that section (no route change, just scroll)

### Happy path: From sidebar
1. User clicks Learn button → opens at first section (no specific section specified)
2. View renders, TOC shows all 4 sections
3. User clicks ← Home → returns to Home view

### Empty / first-run states
- Learn always has content (static explanations, not data-driven)

### Duplicate / conflict
- N/A - no user-modifiable state

### Failure / error states
- N/A - static content, no API calls or data fetching

### Cancellation / close mid-flow
- Back button → returns to Home (no confirmation needed)

### Loading / progress states
- No loading states (content is static, pre-rendered)

## Benchmarks & improvement

### Observable metrics
- **View render time**: Learn button click → content visible
  - Measure: Button click → page heading rendered
  - Target: < 100ms (static content, no data fetch)
- **Section scroll time**: TOC click → section scrolled into view
  - Measure: Link click → `scrollIntoView` complete
  - Target: < 200ms (browser scrollIntoView + focus)
- **Deep-link open time**: From Home "Learn more" → section focused
  - Measure: Link click → section in viewport
  - Target: < 150ms (route change + scroll + focus)

### Current instrumentation
- No timing metrics
- Section focus via `headingRefs.current.get(section)?.focus()` (no logging)
- No analytics on which sections are visited most

### Suggested measurements for verification
- **Section visit distribution**: Track which sections users open (broken vs invoke vs cost vs unused)
- **Entry point distribution**: How often users arrive via Home vs sidebar
- **Time spent per section**: Track how long users stay on Learn view (proxy for engagement)
- **TOC usage**: Count in-page TOC clicks vs scrolling manually

### Improvement levers
- **Lazy-load table** in "Who can invoke" (currently renders on mount; could defer until scrolled into view)
- **Code-split Learn view** (currently loaded eagerly; ~5KB of static content)
- **Add search** within Learn sections (currently relies on browser Cmd+F; inline search could highlight matches)
- **Link to related flows** (e.g. "Prompt cost" could link to Park action, "Invoke" to invocation policy editor)

## Observable end state

After each interaction:

- **Open Learn**: Learn view renders, sidebar Learn button shows active state (`aria-current="page"`)
- **Deep-link to section**: View scrolls to section heading, heading focused (screen reader lands there)
- **TOC click**: Page scrolls to section, active link updates in TOC
- **Back button**: Returns to Home view

Invariants:
- Learn view always has all 4 sections (never partial or conditional)
- Section headings always match TOC labels
- Content is static (never changes based on user data or snapshot)
- Back button always goes to Home (never to previous view)
