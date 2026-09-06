# Skill Detail View

Full-page view of an installed skill: header (name, actions, metadata), locations card (per-deployment toggles), markdown editor (with fork-before-save), repair/test cards, compare dialog, and AI assistant drawer.

## Sub-features

**Header** (InstalledSkillHeader component):
- **Back button** - ArrowLeft icon + text from `from` view ("Home", "Skills", etc.), triggers `onBack()`
- **Skill name** - H1, bold, text-primary
- **Primary action button** - Dynamic based on skill state:
  - "Pull latest" (updates available) → ghost button, runs `updateSkill()`
  - "Remove" (no updates) → destructive button, runs `removeSkill()` with confirmation
  - Disabled during operation (shows spinner icon + "Updating…" or "Removing…")
- **Assistant trigger** - "Ask assistant" button, ghost style, opens `SkillAssistantDrawer`
- **Overflow menu** (three-dot icon):
  - "Compare copies" (if multiple deployments) → opens `SkillCompareDialog`
  - "View history" → opens event history drawer (not implemented yet, menu item disabled)
  - "Open in Finder" → calls `openSkillPath(path, "reveal")`
  - "Duplicate" (fork) → runs `forkSkill()`, creates copy in manual scope
- **Chips** (below name):
  - Source badge (dotagents / skills.sh / manual / fork / in-repo / plugin)
  - Invocation policy chip ("User only" / "Model only" if not default "both")
  - Broken/Warnings badges (if issues exist)
- **Metadata line** - Last used (relative time) • Invocations (30d count) • Token count (description tokens)

**Locations Card** (SkillLocationsCard component):
- **Purpose** - Shows every deployment of this skill (global, project(s), parked)
- **Row per deployment**:
  - Harness icon + label (e.g. "Claude Code", "Codex")
  - Scope badge (Global / Project name / Parked)
  - Path (full filesystem path to SKILL.md, truncated with tooltip)
  - Status toggle (Enable/Disable switch) - grayed when disabled
  - "Open in editor" button (opens in preferred editor from Settings)
  - "Reveal in Finder" button (opens file manager to skill folder)
- **Enable/Disable toggle** - Per-deployment control:
  - **Enable** - Calls backend command to remove disable marker (`.disabled` file or native mechanism)
  - **Disable** - Calls backend command to add disable marker
  - Shows spinner during operation, toast on success/error
- **Unresolved deployments** - Italic path, "Unresolved" badge, toggle disabled (backend couldn't locate SKILL.md)
- **Action buttons** - Ghost style, icon + text, each button triggers async operation

**Markdown Card** (SkillMarkdownCard component):
- **Display mode** (default):
  - Rendered markdown with syntax highlighting (code blocks), headings, lists, links
  - "Edit" button (top right) → enters edit mode
- **Edit mode**:
  - Monaco editor (VS Code's editor component)
  - Full markdown editing with syntax highlighting
  - "Discard" button → confirms if dirty, reverts to original content
  - "Save" button → runs `handleSave()`, writes `SKILL.md`, exits edit mode
  - Dirty indicator (not visible, but state tracked via `isEditorDirty`)
- **Fork-before-save** - dotagents/skills-sh skills fork to manual scope before save (preserves edit across updates)
- **Loading states**:
  - Initial load → skeleton (animated bars)
  - Load error → error message + "Retry" button
  - Saving → "Save" button shows spinner, disabled
- **Escape key** - Exits edit mode if clean, shows discard confirmation if dirty

**Discard Changes Dialog** (DiscardChangesDialog component):
- **Trigger** - Clicking "Discard" button or pressing Escape with unsaved changes
- **Content** - "Discard unsaved changes to <skill-name>?" + explanation
- **Actions** - "Cancel" (stay in edit mode) / "Discard" (confirm, exits edit mode, reverts content)
- **Async flow** - `pendingDiscard` state holds callback to run on confirm (e.g. `onBack()` when Escape pressed)

**Repair Card** (SkillRepairCard component):
- **Trigger** - Shown when skill has issues (spec violations, missing dependencies, etc.)
- **Content** - List of issues with fix suggestions
- **Actions** - Per-issue actions (e.g. "Fix spec violation", "Install dependency")
- (Not deeply documented in prior coverage - may need fuller research)

**Test Form** (future/planned, not currently implemented):
- Would show input fields for test parameters defined in SKILL.md
- "Run test" button → executes skill in test harness
- Test results display area

**Compare Dialog** (SkillCompareDialog component):
- **Trigger** - "Compare copies" from overflow menu, or navigating with `intent: "compare"`
- **Content** - Side-by-side diff of SKILL.md from multiple deployments
- **Purpose** - Shows which copy differs when "Copies differ" warning exists
- **Close** - Click outside, Escape key, or dialog close button

**Assistant Drawer** (SkillAssistantDrawer + SkillAssistantPanel):
- **Trigger** - "Ask assistant" button in header
- **Position** - Right-side overlay drawer, slides in from right
- **Content** - `SkillAssistantPanel` with AI chat interface
- **Actions**:
  - Ask questions about skill
  - Request edits to SKILL.md
  - Review/apply suggestions
- **Apply flow** - Assistant returns new SKILL.md content → "Apply" button → calls `handleApplied()` → updates `rawContent`, exits edit mode
- **Close** - Click outside drawer, Escape key, or close icon in drawer header

**Back Navigation**:
- **Back button** in header → `onBack()` → returns to `from` view (Home, Skills, etc.)
- **Escape key** → Back if no unsaved changes, otherwise shows discard dialog
- **Dirty guard** - Blocks navigation if editor dirty, requires confirmation

## How to get to it (user POV)

**From Home or Skills**:
1. Click any skill row in inbox or table
2. Skill detail page opens

**From URL** (deep link):
- `/skill/<name>` (if app supports routing)
- Optional `deploymentPath` query param to show specific deployment

**From Compare Intent**:
- Home Warnings row "Compare" button → navigates with `intent: "compare"` → auto-opens compare dialog

## Driving it with Playwright

```typescript
// Open skill detail from Skills list
await page.goto('http://localhost:1420');
await page.click('button:has-text("Skills")');
await page.waitForSelector('table tbody tr');
const firstRow = page.locator('table tbody tr').first();
const skillName = await firstRow.locator('span').first().textContent();
await firstRow.click();

// Verify detail page loaded
await page.waitForSelector('text=Locations');
await expect(page.locator(`h1:has-text("${skillName}")`)).toBeVisible();

// Check locations card
const locationsCard = page.locator('text=Locations').locator('..');
const deploymentCount = await locationsCard.locator('[data-deployment]').count();
console.log(`Deployments: ${deploymentCount}`);

// Toggle deployment enable/disable
const firstToggle = locationsCard.locator('button[role="switch"]').first();
const wasEnabled = await firstToggle.getAttribute('aria-checked') === 'true';
await firstToggle.click();
await page.waitForTimeout(500); // Backend operation
const nowEnabled = await firstToggle.getAttribute('aria-checked') === 'true';
expect(nowEnabled).toBe(!wasEnabled);

// Open in editor
const openButton = locationsCard.locator('button:has-text("Open in editor")').first();
await openButton.click();
// Native app opens (not verifiable via Playwright)

// Enter edit mode
await page.click('button:has-text("Edit")');
await page.waitForSelector('.monaco-editor'); // Monaco editor renders

// Edit content
await page.keyboard.type('\n\n## New Section\n\nTest content');
// Monaco content change tracked via isEditorDirty

// Try to escape with unsaved changes
await page.keyboard.press('Escape');
await page.waitForSelector('text=Discard unsaved changes'); // Discard dialog

// Cancel discard
await page.click('button:has-text("Cancel")');
await expect(page.locator('.monaco-editor')).toBeVisible(); // Still in edit mode

// Save changes
await page.click('button:has-text("Save")');
await page.waitForSelector('[role="status"]:has-text("Saved")'); // Toast
await expect(page.locator('button:has-text("Edit")')).toBeVisible(); // Back to display mode

// Open assistant
await page.click('button:has-text("Ask assistant")');
await page.waitForSelector('.assistant-drawer, [data-assistant-panel]'); // Drawer slides in

// Ask question (assistant chat)
const assistantInput = page.locator('.assistant-drawer textarea, input[placeholder*="Ask"]');
await assistantInput.fill('What does this skill do?');
await page.keyboard.press('Enter');
await page.waitForTimeout(2000); // AI response (mocked or real)

// Close assistant
await page.keyboard.press('Escape'); // Or click close button
await expect(page.locator('.assistant-drawer')).toBeHidden();

// Open compare dialog (overflow menu)
await page.click('button[aria-label="More actions"]'); // Three-dot menu
await page.click('text=Compare copies');
await page.waitForSelector('.compare-dialog, text=Compare deployments');

// Close compare
await page.keyboard.press('Escape');

// Back to previous view
await page.click('button:has-text("Back")'); // Or "Skills", "Home", etc.
await page.waitForSelector('table tbody tr, text=Home'); // Previous view
```

Selectors:
- Back button: `button:has-text("Back")` or `button[aria-label="Back to <view>"]`
- Skill name: `h1` (first heading)
- Primary action: `button:has-text("Pull latest")`, `button:has-text("Remove")`
- Assistant trigger: `button:has-text("Ask assistant")`
- Overflow menu: `button[aria-label="More actions"]` (three-dot icon)
- Locations card: `text=Locations` parent
- Deployment toggles: `button[role="switch"]` within locations card
- "Open in editor" buttons: `button:has-text("Open in editor")`
- Edit button: `button:has-text("Edit")`
- Monaco editor: `.monaco-editor`
- Save/Discard buttons: `button:has-text("Save")`, `button:has-text("Discard")`
- Discard dialog: `text=Discard unsaved changes`
- Assistant drawer: `.assistant-drawer` or `[data-assistant-panel]`
- Compare dialog: `.compare-dialog` or `text=Compare deployments`

## Gotchas

- **deploymentPath prop** - If provided, focuses on specific deployment (used for multi-deployment skills when clicked from filtered view)
- **Monaco editor async** - Takes ~500ms to load and render, wait for `.monaco-editor` selector
- **Fork-before-save only for dotagents/skills-sh** - Manual/in-repo skills save directly, no fork
- **Escape key blocked in Monaco** - Pressing Escape in editor exits edit mode (if clean) or shows discard dialog (if dirty), but typing in editor doesn't trigger navigation
- **Back button dynamic label** - Shows "Home" or "Skills" based on `from` view
- **Unresolved deployments** - Toggle disabled, italic path, can't edit (backend couldn't locate SKILL.md)
- **Compare dialog intent** - Auto-opens once when navigating with `intent: "compare"`, then clears intent (won't auto-open again without fresh intent)
- **Assistant drawer state** - Managed in appStore (`isAssistantOpen`), persists across skill switches (closing skill detail while assistant open → reopening same skill shows assistant)
- **Markdown card loading** - Initial load shows skeleton, copy-switch (changing deployment while staying on same skill name) keeps old content visible (no skeleton jump)
- **Primary action changes** - "Pull latest" when updates available, "Remove" otherwise (dynamic button text + handler)

## Branches

### Happy path: View, edit, save, toggle deployment
1. User clicks skill in list → detail page loads
2. Header shows name, chips, metadata; locations card shows deployments; markdown card shows content
3. User clicks "Edit" → Monaco editor renders
4. User types changes → `isEditorDirty` set to true
5. User clicks "Save" → fork runs (if dotagents/skills-sh), content writes to SKILL.md, toast "Saved", edit mode exits
6. User toggles deployment disable → backend runs, toast confirms, toggle updates
7. User clicks "Back" → returns to previous view

### Empty / first-run states
- **Skill has one deployment** - Locations card shows one row, no "Compare copies" in overflow menu
- **No issues** - No repair card shown
- **No invocations yet** - Metadata line shows "never" for last used, 0 invocations

### Duplicate / conflict states
- **Multiple deployments with different content** - "Copies differ" chip in header, "Compare copies" menu item enabled
- **Editing dotagents skill** - Fork runs before save, new deployment created in manual scope
- **Skill removed between views** - `skill` prop becomes `null`, shows error state or redirects

### Failure / error states
- **SKILL.md load failure** - Error message in markdown card + "Retry" button
- **Save failure** - Toast "Couldn't save SKILL.md: <error>", stays in edit mode, content preserved
- **Fork failure** - Toast "Couldn't fork before saving: <error>", save aborted, stays in edit mode
- **Toggle failure** - Toast "Couldn't disable deployment: <error>", toggle reverts to previous state
- **Open in editor failure** - Toast "Couldn't open editor: <error>" (editor not found, path invalid)
- **Reveal in Finder failure** - Toast "Couldn't reveal: <error>" (path invalid)
- **Compare load failure** - Dialog shows error message

### Cancellation / close mid-flow
- **Discard unsaved changes** - Clicking "Discard" in dialog or confirming Escape → reverts content, exits edit mode
- **Cancel discard** - Clicking "Cancel" in dialog → stays in edit mode, changes preserved
- **Close assistant drawer mid-chat** - Drawer closes, chat state may persist (depending on implementation)
- **Navigate away during save** - Save continues in background (async, no cancellation), toast may show after nav

### Loading / progress states
- **Initial page load** - Skeleton for markdown card (if not copy-switch), locations card renders immediately
- **SKILL.md loading** - Skeleton shows animated bars
- **Saving** - "Save" button shows spinner icon, disabled, text "Saving…"
- **Forking** - No separate spinner (happens during save), single "Saving…" covers fork + write
- **Toggle operation** - Switch shows intermediate state (animates position), disabled during backend call
- **Monaco editor loading** - Brief delay (< 500ms) as Monaco bundle loads and initializes

## Benchmarks & improvement

### Observable metrics
- **Detail page render time**: Click skill → page visible
  - Measure: Nav click → header + cards rendered
  - Target: < 400ms (includes SKILL.md read)
- **SKILL.md load duration**: Component mount → content displayed
  - Measure: `readInstalledSkillMd()` call → markdown rendered
  - Target: < 200ms for typical files (< 10KB)
- **Monaco editor initialization**: Enter edit mode → editor ready
  - Measure: "Edit" click → `.monaco-editor` usable
  - Target: < 500ms (Monaco lazy-loaded on first use)
- **Save duration**: Click "Save" → success toast
  - Measure: Button click → toast appears
  - Target: < 1s for direct save, < 3s for fork + save
- **Toggle deployment duration**: Click toggle → state updates
  - Measure: Click → backend complete + UI reflects change
  - Target: < 500ms (filesystem operation)

### Current instrumentation
- Loading states shown in UI (skeleton, spinners)
- Error toasts with messages
- `isLoading`, `isSaving`, `isEditorDirty` flags tracked
- No timing logs, save success rate, or fork duration metrics

### Suggested measurements for verification
- **Edit mode responsiveness**: Verify Monaco editor keybindings work (Cmd+Z, Cmd+F, etc.)
- **Fork correctness**: Verify forked skill appears in manual scope with all metadata preserved
- **Toggle idempotency**: Enable → disable → enable, verify final state matches initial
- **Discard guard accuracy**: Verify dirty flag only set when content actually changes (not just entering edit mode)
- **Compare dialog accuracy**: Verify diff shows exact character-level differences between deployments

### Improvement levers
- **Prefetch SKILL.md** (currently loads on mount; could prefetch while animating from list)
- **Memoize markdown rendering** (currently re-renders on every content change; memo keyed by content would skip redundant work)
- **Virtualize long markdown** (files > 50KB lag; syntax highlighter could chunk)
- **Debounce dirty flag** (currently sets on every keystroke; 200ms debounce would reduce re-renders)
- **Cache Monaco editor instance** (currently recreates on mode toggle; single persistent instance would init faster)
- **Batch deployment toggles** (currently one-at-a-time; "Disable all" / "Enable all" would speed bulk ops)
- **Optimistic toggle updates** (currently waits for backend; instant UI update + rollback on error would feel snappier)

## Observable end state

After each interaction:

- **Navigate to detail**: Page shows header (name, chips, actions), locations card (deployments), markdown card (content)
- **Toggle deployment**: Switch animates, backend runs, toast confirms, deployment disabled/enabled (removed from/added to prompt)
- **Enter edit mode**: Monaco editor renders, "Edit" button becomes "Save"/"Discard"
- **Edit content**: `isEditorDirty` set, dirty guard active (Escape/Back shows discard dialog)
- **Save**: Fork runs (if needed), SKILL.md writes, toast "Saved", edit mode exits, content updated
- **Discard**: Dialog appears → confirm → content reverts, edit mode exits
- **Open in editor**: Preferred editor opens to skill folder (not verifiable via Playwright)
- **Open assistant**: Drawer slides in from right, chat interface ready
- **Compare copies**: Dialog opens with side-by-side diff of deployments
- **Back navigation**: Returns to previous view (Home, Skills, etc.), dirty guard blocks if unsaved changes

Invariants:
- Markdown card content always matches selected `deploymentPath` (or first deployment if none specified)
- Fork-before-save only for dotagents/skills-sh sources (manual/in-repo skip fork)
- Edit mode and dirty state reset on skill switch (new skill = fresh state)
- Toggle state matches backend state (enable/disable reflected in SKILL.md presence)
- Primary action button text matches skill state (updates available = "Pull latest", else "Remove")
- Compare dialog auto-opens once per `intent: "compare"` navigation, then clears intent
- Escape key exits cleanly (no dirty changes) or triggers discard dialog (dirty changes)
- Back button label matches `from` view ("Home", "Skills", etc.)
