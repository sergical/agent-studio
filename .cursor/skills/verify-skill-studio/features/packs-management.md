# Packs Management

Create, update, publish, import, and delete skill packs - bundled collections of skills distributed via GitHub as dotagents-compatible repos (see `docs/agent-skill-conventions.md` Packs section).

## Sub-features

- **Packs list** - All locally created packs with skill counts, repo links, created dates
- **Create pack** - Bundle selected skills from Skills view into a new pack
- **Pack detail** - View pack's skill list, directory path, GitHub repo (if published)
- **Update pack** - Rebuild pack tree from skill list, commit if changed
- **Publish pack** - Push pack to GitHub (creates repo first time, pushes updates after)
- **Import pack** - Install all skills in a pack from a GitHub repo (via Add Skill sheet method="pack")
- **Delete pack** - Remove pack locally (GitHub repo untouched)

## How to get to it (user POV)

**List view**:
1. Click "Packs" in sidebar (hidden behind `skill-packs` feature flag)
2. View shows all packs with names, skill counts, repo status

**Create**:
1. In Skills view, enter selection mode (click "Select" button)
2. Check skills to bundle
3. Click "Create pack" button
4. Enter pack name in dialog
5. Pack created in `~/.agents/packs/<name>`

**Detail**:
1. From Packs list, click a pack row
2. Detail page shows: name, dir path, repo URL (if published), skill chips
3. Actions: Update, Publish/Push, Delete

**Import**:
1. Add Skill sheet, method="pack", source=GitHub repo with pack
2. Submit → imports bundled + referenced skills

## Driving it with Playwright

```typescript
// Navigate to Packs view
await page.click('button:has-text("Packs")');
await page.waitForSelector('text=Packs');

// Create pack from Skills view
await page.click('button:has-text("Skills")');
await page.click('button:has-text("Select")'); // Enter selection mode
await page.check('table tbody tr:first-child input[type="checkbox"]'); // Select a skill
await page.check('table tbody tr:nth-child(2) input[type="checkbox"]'); // Select another
await page.click('button:has-text("Create pack")');
await page.fill('input[placeholder*="pack name"]', 'my-pack');
await page.click('button:has-text("Create")');

// Open pack detail
await page.click('button:has-text("Packs")');
await page.click('button:has-text("my-pack")'); // Pack row
await page.waitForSelector('text=my-pack'); // Detail page

// Update pack
await page.click('button:has-text("Update pack")');
await page.waitForSelector('[role="status"]:has-text("Pack updated")');

// Publish pack (first time)
await page.click('button:has-text("Publish to GitHub")');
// GitHub confirmation dialog (backend)
// After confirmation, toast shows success

// Delete pack
await page.click('button:has-text("Delete")');
// Confirmation dialog
await page.click('[role="alertdialog"] button:has-text("Delete")');
```

Selectors:
- Packs button: `button:has-text("Packs")`
- Pack rows: `button:has-text("<pack-name>")` (list items are buttons)
- Create pack: `button:has-text("Create pack")` (in Skills view, selection mode)
- Pack name input: `input[placeholder*="pack name"]`
- Update button: `button:has-text("Update pack")`
- Publish button: `button:has-text("Publish to GitHub")` or `button:has-text("Push update")`
- Delete button: `button:has-text("Delete")`

## Gotchas

- **Feature flag** - Packs row hidden in sidebar unless `skill-packs` feature flag enabled (default off)
- **Selection mode** - Create pack requires entering selection mode in Skills view first
- **Pack structure** - Pack is a git repo in `~/.agents/packs/<name>` with skill subfolders and `agents.toml`
- **Publish confirmation** - Backend shows native dialog before pushing to GitHub (not dismissible via Playwright)
- **GitHub dependency** - Publish requires `gh` CLI installed and authenticated
- **Pack registry** - Packs tracked in `~/.agents/skill-studio.json`, not `.skill-lock.json`
- **Import vs install** - "Import pack" installs its skills; "Create pack" bundles existing skills
- **Update auto-commit** - Update pack commits only if tree changed (no-op if unchanged)

## Branches

### Happy path: Create pack
1. User enters Skills view → clicks Select → checks 3 skills
2. Clicks "Create pack" → enters name "my-favorites" → submits
3. Backend: creates `~/.agents/packs/my-favorites/`, copies 3 skill folders, writes `agents.toml`, `git init`, commits
4. Toast success → Packs list shows new pack

### Happy path: Publish pack
1. User opens pack detail → clicks "Publish to GitHub"
2. Backend: runs `gh repo create`, prompts for visibility (public/private), pushes
3. Toast success → pack detail shows repo URL ("getsentry/skill-studio-my-favorites")
4. Next publish → button says "Push update" (updates existing repo)

### Happy path: Import pack
1. User opens Add Skill sheet → types pack repo → selects method="pack"
2. Selects harnesses to import to → clicks "Import pack"
3. Backend: clones pack, runs `dotagents add <repo> --all`, installs bundled skills, installs referenced skills from other repos
4. Toast: "Imported N skills" → sheet closes

### Happy path: Update pack
1. User adds a new skill to their local skills
2. Opens pack detail → clicks "Update pack"
3. Backend: checks if pack's agents.toml references the new skill → no → "Already up to date" toast
4. User manually edits pack's agents.toml (outside app) → clicks Update again → "Pack updated" toast

### Empty / first-run states
- No packs created yet → Packs view shows "No packs yet. Select skills... to bundle them."
- Pack with 0 skills → Not possible (creation requires at least 1 skill selected)

### Duplicate / conflict
- Pack name already exists → Create fails with error toast "Pack name already exists"
- Pack directory exists on disk → Backend refuses, error toast

### Failure / error states
- **Create failure** → Error toast, stays in selection mode
- **Publish failure** → Error toast (no `gh` CLI, auth failure, network error), pack detail stays open
- **Update failure** → Error toast, pack detail stays open
- **Delete failure** → Error toast, pack detail stays open
- **Import failure** → Error toast listing which skills failed to install

### Cancellation / close mid-flow
- Cancel create pack dialog → selection mode persists, no pack created
- Close pack detail during update → update continues in background (Tauri command)
- Cancel publish confirmation (backend dialog) → error toast "Publish cancelled", no push

### Loading / progress states
- **Creating pack** → "Creating…" on dialog button
- **Updating pack** → "Updating…" on button
- **Publishing pack** → "Publishing…" on button (blocks until `gh` completes)
- **Deleting pack** → "Deleting…" on button

## Benchmarks & improvement

### Observable metrics
- **Create pack duration**: Selection → pack created in `~/.agents/packs/<name>` + committed
  - Measure: Dialog submit → toast shown
  - Target: < 2s for typical packs (< 10 skills)
- **Publish duration**: Button click → GitHub push complete
  - Measure: "Publishing…" → toast success
  - Target: < 10s (depends on `gh` CLI + git push)
- **Update duration**: Button click → pack tree rebuilt + committed (if changed)
  - Measure: "Updating…" → toast
  - Target: < 1s (fast if no change, up to 3s if large diff)
- **Import duration**: Install all bundled + referenced skills
  - Measure: Sheet submit → toast
  - Target: ~5s per skill (serial `dotagents add` calls)

### Current instrumentation
- `busy` state tracks which operation is in progress (no timing)
- Toast on success/failure (no duration logged)
- No pack creation/publish/import analytics

### Suggested measurements for verification
- **Pack creation rate**: How often users create packs (product metric)
- **Pack publish rate**: Percentage of created packs that get published
- **Pack size distribution**: Skill count per pack (typical 3-10, outliers 50+)
- **Import success rate**: Percentage of pack imports that complete without error
- **Time per skill in create/update**: Measure copy + commit overhead per skill

### Improvement levers
- **Parallel pack creation** (currently sequential skill copies; could parallelize)
- **Skip git commit on update if unchanged** (currently diffs tree, already optimized)
- **Batch-import pack skills** (currently serial `dotagents add` per skill; could batch via CLI)
- **Cache pack enumeration** (currently reads `~/.agents/skill-studio.json` on every Packs view load)
- **Stream publish progress** (currently blocks until `gh` completes; could emit events per step: create repo, push)

## Observable end state

After each interaction:

- **Create pack**: Packs view shows new pack row, detail page opens
- **Update pack**: Toast shows "Pack updated" or "Already up to date"
- **Publish pack**: Pack detail shows repo URL, button changes to "Push update"
- **Push update**: Toast confirms push, no UI change (repo already linked)
- **Import pack**: Toast shows "Imported N skills", sheet closes, sidebar skill count increments
- **Delete pack**: Pack row disappears from list, view returns to Packs list

Invariants:
- Pack name unique within `~/.agents/packs/` (backend enforces)
- Pack directory always a git repo (initialized on creation)
- Published packs always have `pack.repo` field set (persisted in `skill-studio.json`)
- Delete never touches GitHub repo (only local removal)
