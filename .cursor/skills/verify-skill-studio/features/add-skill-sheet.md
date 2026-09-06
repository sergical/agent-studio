# Add Skill Sheet

Right-side drawer for adding skills from multiple sources - supports dotagents, skills.sh, manual copy, GitHub repos, git URLs, local paths, and skill packs. Includes method selection, harness targeting, scope (global/project), and trial mode.

## Sub-features

- **Source field** - Parse input live: GitHub owner/repo, full URLs, git URLs, local paths
- **Source validation** - Real-time parse feedback ("github · owner/repo" or error message)
- **GitHub skill listing** - Auto-discover skills in a repo (debounced 400ms)
- **Multiple skill selection** - Checkboxes when repo contains multiple SKILL.md files
- **Method selector** - dotagents / skills.sh / Copy / Pack (segmented control)
- **Method availability** - Gray out unavailable methods (e.g. dotagents when not installed)
- **Agent target selector** - Enable/disable harnesses (Claude Code link toggle separate)
- **Scope selector** - Global vs Project
- **Project directory picker** - Browse for project folder or select from remembered list
- **Trial mode** - "Try for 24 hours" checkbox (auto-remove after 24h unless kept)
- **Skill pack import** - Special flow for packs (imports bundled + referenced skills)
- **Validation** - Install button disabled until: source parses, project path set (if project scope), skills selected (if multi-skill repo)
- **Progress feedback** - Submitting → toast on success/failure → auto-close sheet on success

## How to get to it (user POV)

1. Click "Add skill" in sidebar
2. Default tab is "Add by source"
3. Fill in source, pick method, configure agents/scope, submit

## Driving it with Playwright

```typescript
// Open sheet
await page.click('button:has-text("Add skill")');
await page.waitForSelector('[aria-label="Add skill"]');

// Type source (GitHub owner/repo)
await page.fill('#add-skill-source', 'getsentry/skills');
await page.waitForTimeout(500); // Let parsing + listing complete

// Wait for skill listing to load
await page.waitForSelector('text=Skills', { timeout: 5000 });

// Select method (if multiple available)
await page.click('[aria-label="Install method"] [value="dotagents"]');

// Configure harnesses
const claudeCheckbox = page.locator('label:has-text("Claude Code") input[type="checkbox"]');
await claudeCheckbox.check();

// Set scope to project
await page.click('[value="project"]');

// Choose directory (requires folder picker dialog, hard to automate)
// OR select from remembered projects if any exist
const projectSelect = page.locator('select'); // ProjectDirectorySelect
if (await projectSelect.isVisible()) {
  await projectSelect.selectOption({ index: 0 });
}

// Enable trial mode
await page.click('label:has-text("Try for 24 hours") input[type="checkbox"]');

// Submit
await page.click('button:has-text("Add skill")');
await page.waitForSelector('[role="status"]:has-text("Added")');
```

Selectors:
- Sheet: `[aria-label="Add skill"]`
- Source input: `#add-skill-source`
- Method buttons: `[aria-label="Install method"] [value="dotagents"]`, etc.
- Harness checkboxes: `label:has-text("Claude Code") input`, `label:has-text("Codex") input`, etc.
- Scope toggle: `[value="global"]`, `[value="project"]`
- Trial checkbox: `label:has-text("Try for 24 hours") input`
- Submit button: `button:has-text("Add skill")` or `button:has-text("Install N skills")`
- Cancel button: `button:has-text("Cancel")`

## Gotchas

- **Source parsing** - Runs synchronously on every keystroke (no debounce), shows live feedback below field
- **GitHub listing** - Debounced 400ms after source stabilizes, fetches skill folders from repo
- **Multiple skills** - If repo contains 2+ SKILL.md files, shows checkbox list (all selected by default)
- **Method constraints** - Git URLs only support dotagents (if installed); local paths only support Copy
- **dotagents unavailable** - When `dotagents_installed === false`, dotagents method grayed out and unavailable
- **Scope dependency** - Project scope requires `projectPath !== null` (validation blocks submit)
- **Pack import** - Method="pack" targets the repo root's `agents.toml`, not individual skills
- **Claude Code link** - Separate checkbox "Link Claude Code's own skills dir" (only shows when Claude reads shared folder)
- **Trial expiry** - Trials auto-trash after 24h, emit `skills://trial-expired` event with restore action in toast
- **Prefill support** - Sheet can open with `prefill` string (e.g. from clipboard or drag-drop)
- **Fresh defaults** - Every open resets form to defaults, fetches `AddMethodDefaults` to seed harness toggles

## Branches

### Happy path: GitHub single skill
1. User types "owner/repo" → parses as GitHub
2. Listing fetches → finds 1 SKILL.md → shows as single row
3. User picks method (dotagents default if installed)
4. User leaves harnesses default (all readers enabled)
5. User clicks "Add skill" → `addSkill` command runs → toast success → sheet closes

### Happy path: GitHub multiple skills
1. User types GitHub URL → parses → listing finds 5 skills
2. All 5 checked by default → user unchecks 2
3. User clicks "Install 3 skills" → `addSkills` command runs → 3 succeed → toast success
4. Sheet closes → sidebar count increments

### Happy path: Git URL
1. User types `https://github.com/owner/repo.git` → parses as git
2. Method selector shows only dotagents (grayed if not installed)
3. User submits → `addSkill` with method: "dotagents" → success

### Happy path: Local path
1. User types `/Users/me/my-skill` → parses as local
2. Method selector shows only Copy
3. User submits → `addSkill` with method: "copy" → success

### Happy path: Pack import
1. User types pack repo → parses as GitHub
2. User selects method="pack"
3. Harness selector shows (agents to import to)
4. User submits → `importSkillPack` runs → imports bundled + referenced skills → toast with count

### Trial mode
1. User checks "Try for 24 hours"
2. Install proceeds with `trial: true`
3. 24h later, backend emits `skills://trial-expired` event
4. Toast shows "Trial ended: <name> moved to skills-trash" with Restore button
5. User clicks Restore → skill copied back to `~/.agents/skills/<name>`

### Empty / first-run states
- No user-added projects → "Choose directory" button (no dropdown)
- No dotagents installed → dotagents method grayed out
- No skills.sh key → skills.sh method still works (server mode)
- Empty source field → placeholder "Paste a repo, URL, or path to get started."

### Duplicate / already installed
- Backend handles duplicates (returns warning message in `AddSkillResult.warning`)
- Toast shows warning: "Added <name>" with warning message as secondary text
- Sheet closes as normal (not an error)

### Failure / error states
- **Parse error** → Shows below source field in red once field loses focus (e.g. "Invalid GitHub URL")
- **Listing error** → "Could not reach GitHub" message with Retry button
- **Listing truncated** → Large repos show "showing the first N skills GitHub returned"
- **No skills found** → "No SKILL.md found in this repo or path."
- **Install failure** → Error toast, sheet stays open, error message shown below form
- **Partial failure (multi-skill)** → Success toast for installed count + error toast listing failed ones
- **Pack import failure** → Error toast with list of failed skills

### Cancellation / close mid-flow
- Click Cancel → sheet closes, form state discarded
- Click outside sheet → sheet stays open (requires explicit Cancel or Escape)
- Escape key → closes sheet if not mid-install
- Close during listing fetch → fetch cancelled, sheet closes
- Close during install → install continues in background (Tauri command runs to completion)

### Loading / progress states
- **Listing loading** → Skeleton placeholder ("Skills" label + animated bar)
- **Submitting** → Button shows "Adding…", disabled, spinner
- **Fresh open** → `getAddMethodDefaults` fetch runs (harness switches stay null until resolved)

## Benchmarks & improvement

### Observable metrics
- **Parse latency**: Keystroke → feedback text updates (< 10ms, synchronous)
- **Listing latency**: Source field stable → GitHub skills fetched (400ms debounce + API call)
  - Measure: Last keystroke → "Skills" section populated
  - Target: < 1.5s (400ms debounce + 1s GitHub API + 100ms render)
- **Install duration**: Button click → toast shown
  - Measure: "Adding…" starts → toast appears
  - Target: < 5s for dotagents/copy, < 10s for skills.sh (depends on `npx skills` CLI)
- **Multi-skill install**: Per-skill serial install time
  - Measure: Total duration / skill count
  - Target: ~3s per skill average

### Current instrumentation
- `isSubmitting` flag tracks install state (no timing)
- Toast shows success/failure (no duration logged)
- GitHub listing shows count + truncated flag
- No per-method install timing captured

### Suggested measurements for verification
- **Source parse rate**: Measure keystrokes that result in valid vs invalid parses (quality metric)
- **Listing cache hit rate**: How often same repo is requested (could reduce API calls)
- **Install success rate by method**: dotagents vs skills.sh vs copy (identify flakiest path)
- **Trial conversion rate**: Percentage of trials kept vs expired (product metric)
- **Error rate by source type**: GitHub vs git vs local (identify fragile paths)

### Improvement levers
- **Parallel multi-skill installs** (currently sequential; backend `addSkills` spawns parallel `npx` calls but waits for all)
- **Prefetch GitHub listing** on source field focus (saves ~400ms if user will type GitHub URL)
- **Cache GitHub listings** per repo/ref (currently no cache; same repo typed twice refetches)
- **Validate source on submit** instead of live (reduces parse thrashing during fast typing)
- **Persist form state** across opens (currently resets every time; could remember last method/scope)
- **Show install progress bar** instead of just "Adding…" (backend could emit progress events per skill)
- **Batch GitHub API calls** for multi-skill listings (currently one call per repo; could use GraphQL to fetch multiple in one roundtrip)

## Observable end state

After each interaction:

- **Source typed**: Parse feedback updates below field (green "github · owner/repo" or red error)
- **GitHub listing loaded**: "Skills" section shows skill names + paths, checkboxes if multiple
- **Method selected**: Segmented control highlights choice, caption text updates
- **Harness toggled**: Checkbox state changes, `enabledReaders` array updates
- **Scope switched**: Toggle updates, project picker shows/hides
- **Submit**: Button shows "Adding…" → toast appears → sheet closes (on success)
- **Cancel**: Sheet closes immediately, form state discarded

Invariants:
- Source field always reflects user input (no auto-correction or normalization)
- Method selection constrained by source type (git → dotagents only, local → copy only)
- Submit button disabled when validation fails (no source, no project path, no skills selected)
- Install always closes sheet on success (never stays open after success)
- Form always resets to defaults on next open (never carries over stale state)
