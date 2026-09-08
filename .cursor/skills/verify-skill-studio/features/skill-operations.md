# Skill Operations

Core operations for managing skills: install, update, remove, fork, unfork, pull upstream, park, unpark, enable/disable (per-harness and deployment), invocation policy changes.

## Sub-features

- **Install** - Via Add Skill sheet (dotagents/skills-sh/copy methods) or SkillStore Browse tab
- **Update** - Pull latest version from upstream (dotagents `sync`, skills.sh re-install, or fork pull)
- **Remove** - Uninstall skill from global or project scope
- **Fork** - Detach dotagents/skills-sh skill from ledger to allow local edits
- **Unfork** - Discard fork, reinstall from origin
- **Pull upstream** - Three-way merge fork's snapshot + disk + fresh upstream
- **Park** - Move shared-folder deployment to `~/.agents/skills-parked/<name>` (global disable)
- **Unpark** - Restore from parked to `~/.agents/skills/<name>`
- **Enable/disable per-harness** - Toggle harness's own view of skill (Codex `config.toml`, OpenCode `opencode.json`, Claude Code symlink)
- **Enable/disable deployment** - Universal fallback via `.skill-studio-disabled/` holding directory
- **Set invocation policy** - Rewrite SKILL.md frontmatter (`disable-model-invocation`, `user-invocable`) + Codex `agents/openai.yaml`

## How to get to it (user POV)

- **Install**: Add Skill sheet (sidebar "Add skill" button) or SkillStore Browse tab
- **Update**: Home "Updates" group "Pull latest" button or skill detail overflow menu "Update"
- **Remove**: Skill detail overflow menu "Remove"
- **Fork**: Skill detail markdown card "Fork" button (when source is dotagents/skills-sh)
- **Unfork**: Skill detail overflow menu "Unfork" (when skill is forked)
- **Pull upstream**: Home "Updates" group for forked skills or detail "Pull latest"
- **Park**: Home "Not used in 30 days" group "Park" button or detail overflow menu "Park"
- **Unpark**: Skills view with `scope: "parked"` filter, detail "Unpark" action
- **Enable/disable**: Skill detail Locations card per-deployment toggles
- **Invocation policy**: Skill detail header dropdown "Who can invoke"

## Driving it with Playwright

```typescript
// Install (already covered in add-skill-sheet.md)
await page.click('button:has-text("Add skill")');
// ... fill in source, submit

// Update from Home
await page.click('button:has-text("Home")');
const updateRow = page.locator('text=skill-name').locator('..').locator('button:has-text("Pull latest")');
await updateRow.click();
await page.waitForSelector('[role="status"]:has-text("Skill updated")');

// Remove from detail
await page.click('table tbody tr:has-text("skill-name")'); // Open detail
await page.click('[aria-label="More actions"]'); // Overflow menu
await page.click('text=Remove');
await page.click('[role="alertdialog"] button:has-text("Remove")'); // Confirm

// Fork
await page.click('table tbody tr:has-text("managed-skill")');
await page.click('button:has-text("Fork")'); // In markdown card
await page.waitForSelector('[role="status"]:has-text("Forked")');

// Park from Home
await page.click('button:has-text("Home")');
const parkRow = page.locator('text=unused-skill').locator('..').locator('button:has-text("Park")');
await parkRow.click();
await page.waitForSelector('[role="status"]:has-text("Parked")');

// Enable/disable harness
await page.click('table tbody tr:has-text("skill-name")');
// Locations card has per-harness switches
const claudeSwitch = page.locator('text=Claude Code').locator('..').locator('[role="switch"]');
await claudeSwitch.click();

// Change invocation policy
await page.click('table tbody tr:has-text("skill-name")');
await page.click('button:has-text("Who can invoke")'); // Header dropdown
await page.click('text=You only');
await page.waitForSelector('[role="status"]:has-text("Policy updated")');
```

Selectors:
- Update button: `button:has-text("Pull latest")` or `button:has-text("Update")`
- Remove: Overflow menu `[aria-label="More actions"]` → `text=Remove`
- Fork: `button:has-text("Fork")` (in markdown card)
- Park: `button:has-text("Park")` (Home or detail)
- Harness switch: `[role="switch"]` near harness name
- Invocation dropdown: `button:has-text("Who can invoke")`

## Gotchas

- **Update tool varies** - dotagents uses `dotagents sync`, skills-sh uses `npx skills update`, forks use three-way merge
- **Remove confirmation** - Native dialog (Tauri `ask()`), not dismissible via Playwright
- **Fork restrictions** - Only dotagents/skills-sh skills can fork (manual/plugin/forked skills show no fork button)
- **Park restrictions** - Only skills deployed to shared folder (not project-scoped, plugin, or already parked)
- **Unpark collision** - If skill was reinstalled while parked, unpark reconciles (discards duplicate or trashes drift)
- **Enable/disable varies** - Per-harness uses harness's own mechanism (Codex toml, OpenCode json, Claude symlink); deployment-level uses `.skill-studio-disabled/` fallback
- **Invocation policy writes** - Modifies SKILL.md; writes Codex `agents/openai.yaml` only when the targeted deployment's agent is Codex
- **Sequential updates** - "Update all" button runs updates sequentially (avoids lock-file races)

## Branches

### Happy path: Install from Add Skill
1. User opens sheet → types source → picks method → submits
2. Backend runs `npx skills add <source>` or `dotagents add <source>` or copies folder
3. Toast success → sheet closes → sidebar count increments

### Happy path: Update from Home
1. User sees skill in "Updates" group → clicks "Pull latest"
2. Backend: dotagents skill runs `dotagents sync`, skills-sh skill runs `npx skills update <name>`
3. Toast success → skill disappears from Updates group

### Happy path: Fork skill
1. User opens dotagents-managed skill → clicks "Fork" in markdown card
2. Backend: removes skill from `agents.toml`, writes fork snapshot to `~/.agents/forks/<name>.json`
3. Toast success → skill detail shows "Forked" badge, "Fork" button → "Edit" button

### Happy path: Park skill
1. User sees unused skill in Home → clicks "Park"
2. Backend: moves `~/.agents/skills/<name>` → `~/.agents/skills-parked/<name>`, removes Claude Code symlink if exists
3. Toast success → skill disappears from Home, shows in Skills view with `scope: "parked"` filter

### Happy path: Enable/disable harness
1. User opens skill detail → Locations card shows harness rows
2. User toggles Claude Code switch OFF
3. Backend: removes `~/.claude/skills/<name>` symlink (Claude's per-skill disable)
4. Toast success → switch shows OFF state, row shows "Disabled" badge

### Empty / first-run states
- No updates available → "Updates" group hidden in Home
- No parked skills → Parked row hidden in sidebar
- No forked skills → no "Unfork" action in overflow menu

### Duplicate / already installed
- Install duplicate → Backend returns warning, toast shows warning message
- Unpark when reinstalled → Backend detects collision, discards duplicate or trashes drift

### Failure / error states
- **Install failure** → Error toast, sheet stays open, error message below form
- **Update failure** → Error toast, skill stays in Updates group
- **Remove failure** → Error toast, skill detail stays open
- **Fork failure** → Error toast, skill stays in original state
- **Park failure** → Error toast (e.g. skill not in shared folder, already parked)
- **Enable/disable failure** → Error toast, switch reverts to previous state

### Cancellation / close mid-flow
- Cancel remove confirmation → skill not removed, detail stays open
- Cancel fork → no fork created
- Close sheet during install → install continues in background (Tauri command)

### Loading / progress states
- **Installing** → "Adding…" on submit button
- **Updating** → Spinner inline in "Pull latest" button
- **Removing** → Removal happens immediately after confirmation (no progress indicator)
- **Forking** → Synchronous operation (no loading state, toast shows result)
- **Parking** → Synchronous file move (no loading state, toast shows result)
- **Enable/disable** → Switch toggles immediately, async backend call updates state

## Benchmarks & improvement

### Observable metrics
- **Install duration**: Add Skill submit → toast (measure per method: dotagents, skills-sh, copy)
  - Measure: "Adding…" starts → toast appears
  - Target: < 5s dotagents/copy, < 10s skills-sh (depends on `npx skills` network)
- **Update duration**: "Pull latest" click → toast
  - Measure: Button click → toast
  - Target: < 5s (depends on git fetch + CLI)
- **Remove duration**: Confirm → toast
  - Measure: Dialog confirm → toast
  - Target: < 2s (file deletion + ledger update)
- **Fork duration**: Fork button → toast
  - Measure: Button click → toast
  - Target: < 500ms (write snapshot JSON, remove from ledger)
- **Park duration**: Park button → toast
  - Measure: Button click → toast
  - Target: < 500ms (file move)
- **Enable/disable latency**: Switch toggle → backend confirms
  - Measure: Toggle → switch state settles
  - Target: < 200ms (fast file operations)

### Current instrumentation
- Toast on success/failure (no duration logged)
- `isSubmitting` / `isPulling` / `isParking` flags track state (no timing)
- No per-operation success rate or error categorization

### Suggested measurements for verification
- **Install success rate by method**: dotagents vs skills-sh vs copy (identify flakiest)
- **Update failure causes**: Network vs git vs lock-file contention (categorize errors)
- **Fork adoption rate**: Percentage of dotagents/skills-sh skills that get forked (product metric)
- **Park vs remove**: How often users park vs remove unused skills (UX metric)
- **Enable/disable usage**: How often per-harness toggles are used (feature adoption)

### Improvement levers
- **Parallel updates** (currently sequential to avoid lock-file races; could lock per skill instead of globally)
- **Optimistic park/unpark** (currently waits for backend; could update UI immediately and rollback on error)
- **Batch install** (Add Skill already batches GitHub multi-skill installs; could extend to other sources)
- **Cache fork snapshots** (currently writes JSON on every fork; could defer write until first edit)
- **Stream install progress** (currently blocks until complete; could emit events per step: fetch, copy, commit)

## Observable end state

After each interaction:

- **Install**: Sidebar skill count increments, Skills view shows new skill, Home updates
- **Update**: Skill disappears from Home "Updates" group, detail shows latest commit hash
- **Remove**: Skill disappears from all views, sidebar count decrements
- **Fork**: Skill detail shows "Forked" badge, overflow menu shows "Unfork" action
- **Park**: Skill moves to "Parked" scope, disappears from Home "Unused" group
- **Unpark**: Skill returns to global/project scope, visible in main Skills view
- **Enable/disable**: Locations card switch reflects new state, deployment shows enabled/disabled badge
- **Invocation policy**: Skill detail header shows new policy, SKILL.md frontmatter updated

Invariants:
- Install always increments skill count (no silent failures)
- Remove always decrements skill count (no orphaned entries)
- Fork converts `source_kind` from dotagents/skills-sh to "fork"
- Park moves skill to `skills-parked/`, never deletes
- Enable/disable per-harness uses harness's native mechanism (never generic for those)
- Invocation policy writes to SKILL.md; Codex yaml is written only when the targeted deployment is Codex
