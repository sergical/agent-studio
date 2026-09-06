# Plugins View

List of skills discovered from native plugin caches - Claude Code (`~/.claude/plugins/cache`) and Codex (`~/.codex/plugins/cache`) plugins that ship skills per the agent-plugins.org manifest convention.

## Sub-features

- **Plugin skill list** - Table showing plugin-provided skills
- **Read-only deployments** - Plugin skills cannot be edited, updated, or removed via Skill Studio
- **Click to detail** - Opens skill detail page (read-only mode, no edit/fork actions)
- **Provenance badges** - "plugin" source kind shown in detail
- **Filter inheritance** - Search box in sidebar filters plugin skills too
- **Conditional visibility** - Plugins row in sidebar only shows when `pluginCount > 0`

## How to get to it (user POV)

1. Sidebar shows "Plugins" row only if plugin caches have skills
2. Click "Plugins" in sidebar
3. View loads with plugin-only skill list

## Driving it with Playwright

```typescript
// Navigate to plugins (only if row exists)
const pluginsButton = page.locator('button:has-text("Plugins")');
if (await pluginsButton.isVisible()) {
  await pluginsButton.click();
  await page.waitForSelector('text=Plugin Skills');
}

// Search plugins (via sidebar)
await page.fill('input[aria-label="Search skills…"]', 'my-plugin-skill');
// Should filter plugin skills table

// Click skill to open detail
const firstRow = page.locator('table tbody tr').first();
await firstRow.click();
await page.waitForSelector('[data-testid="skill-detail"]');
```

Selectors:
- Plugins button: `button:has-text("Plugins")`
- Page heading: `text=Plugin Skills` or similar
- Table rows: `table tbody tr`
- Search (sidebar): `input[aria-label="Search skills…"]`

## Gotchas

- **Plugin row hidden** - Sidebar only shows "Plugins" when `pluginCount > 0` (derived from `pluginSkillsView(snapshot.skills)`)
- **Read-only** - Plugin deployments have `deployment.plugin !== null`, so edit/fork/remove actions are disabled/hidden in detail
- **No update mechanism** - Plugins update when the parent plugin updates (not controllable from Skill Studio)
- **Sidebar search applies** - Typing in sidebar search filters plugin skills (same query applies to all views)
- **Native disable** - Plugin skills can still be disabled per-harness (Claude Code symlink removal, Codex `config.toml`, etc.)

## Branches

### Happy path
1. User has Claude Code or Codex with plugins that include skills
2. Sidebar shows "Plugins (N)" row
3. User clicks → PluginSkillsView renders table
4. User clicks row → detail page opens in read-only mode

### Empty / first-run states
- No plugins with skills → "Plugins" row not shown in sidebar
- Plugins exist but no skills → row shows "Plugins (0)" (verify if shown at all)
- Fresh install of Claude Code/Codex → plugin caches may not exist yet (row hidden)

### Duplicate / conflict
- Plugin skill name conflicts with user-installed skill → both show in their respective views
- Detail page shows provenance: plugin skills marked as `source_kind: "plugin"`

### Failure / error states
- Plugin cache read failure → handled during snapshot build (backend), no inline error in Plugins view
- Plugin cache inaccessible → skills simply don't appear (no error toast)

### Cancellation / close mid-flow
- Back from detail → returns to Plugins view (via `onBack()` in `SkillPage`)

### Loading / progress states
- Initial snapshot loading → entire view waits (handled by parent, not view-specific)
- No per-view loading state (relies on snapshot `isLoading`)

## Benchmarks & improvement

### Observable metrics
- **Plugin discovery time**: Part of snapshot scan (backend Rust `scan.rs` reads plugin caches)
- **View render time**: Table render for N plugin skills
  - Measure: View switch → table visible
  - Target: < 200ms for typical counts (< 50 skills)
- **Search filter time**: Sidebar query → filtered plugin table
  - Measure: Keystroke → table updates
  - Target: < 50ms (synchronous filter)

### Current instrumentation
- Plugin skills counted in snapshot (`pluginSkillsView()` helper)
- No per-plugin-skill timing or cache read latency logged
- No analytics on plugin skill usage vs user-installed skills

### Suggested measurements for verification
- **Plugin cache scan duration**: Time spent reading `~/.claude/plugins/cache` and `~/.codex/plugins/cache` (backend span)
- **Plugin skill count distribution**: How many users have 0, 1-5, 6-20, 21+ plugin skills (product metric)
- **Plugin skill click rate**: Percentage of users who open plugin skills vs ignore them
- **Read-only friction**: Count attempts to edit/fork plugin skills (blocked actions)

### Improvement levers
- **Cache plugin enumeration** (currently scans on every snapshot rebuild; could cache per plugin version hash)
- **Lazy-load plugin cache reads** (currently eager; could defer until Plugins view opened first time)
- **Virtual table** for large plugin lists (unlikely needed, typical counts < 50, but scales if popular plugins ship 100+ skills)
- **Expose plugin update mechanism** (currently opaque; could link to plugin's own update flow)

## Observable end state

After each interaction:

- **Navigate to view**: Plugins view renders, table shows plugin-provided skills, sidebar highlights "Plugins"
- **Search**: Table filters to matching skills, unmatched rows hidden
- **Click skill**: Detail page opens, shows skill content, edit/fork actions disabled or hidden
- **Back**: Returns to Plugins view

Invariants:
- Plugins row only visible when at least one plugin skill exists
- Plugin skills always have `source_kind: "plugin"` (never "dotagents", "skills-sh", "manual", "fork")
- Plugin skills cannot be forked, edited, or removed via Skill Studio (read-only)
- Sidebar search query applies to plugin skills (same filter as Skills view)
