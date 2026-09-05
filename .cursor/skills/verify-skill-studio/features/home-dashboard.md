# Home Dashboard

## Sub-features

The Home view is the default landing page and provides:

1. **Stat Tiles** - Three cards showing counts of:
   - **Broken** skills (red) - skills with errors or missing dependencies
   - **Warnings** skills (yellow) - skills with non-blocking issues
   - **Updates Available** (blue) - skills with upstream updates

2. **Usage Lane** - Two metric cards:
   - **Invocations** - Count of skill invocations (24h/7d/30d windows)
   - **Prompt Cost** - Total token cost across all skills

3. **Inbox Groups** - Collapsible sections:
   - **Broken** - Skills that need immediate attention
   - **Warnings** - Skills with minor issues
   - **Updates Available** - Skills with new versions
   - **Not used in 30 days** - Idle skills that might be candidates for removal
   - **Recently used** - Last 5 invoked skills

Each inbox row shows:
- Severity indicator (dot)
- Skill name + harness badges (Claude Code, Codex, etc.)
- Issue detail or usage timestamp
- Action button (Update, Pull, Review, etc.)

## How to get to it (user POV)

The Home view is the **default view** when you launch Skill Studio. No navigation needed.

If you're in another view:
1. Click **Home** in the left sidebar (house icon + "Home" text)
2. You're now on the dashboard

## Driving it with Playwright

### Navigate to Home

```typescript
import { test, expect } from '@playwright/test';

test('navigate to home dashboard', async ({ page }) => {
  await page.goto('http://localhost:1420');
  
  // Wait for sidebar to load
  await page.waitForSelector('text=Home', { timeout: 10000 });
  
  // Click Home in sidebar (if not already there)
  await page.click('text=Home');
  
  // Verify we're on the home view
  await expect(page.locator('h1, h2').filter({ hasText: /Home|Dashboard/i })).toBeVisible();
});
```

### Check Stat Tiles

```typescript
test('home shows stat tiles', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.waitForSelector('text=Home');
  
  // Look for stat tile indicators (text like "Broken", "Warnings", "Updates")
  const brokenTile = page.locator('text=Broken').first();
  const warningsTile = page.locator('text=Warnings').first();
  const updatesTile = page.locator('text=Updates').first();
  
  // Verify at least one tile is visible
  await expect(brokenTile.or(warningsTile).or(updatesTile)).toBeVisible();
  
  // Take screenshot
  await page.screenshot({ 
    path: './evidence/home-stat-tiles.png',
    fullPage: true 
  });
});
```

### Interact with Inbox Groups

```typescript
test('expand and collapse inbox groups', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.waitForSelector('text=Home');
  
  // Find a collapsible group (e.g., "Broken", "Recently used")
  const brokenGroup = page.locator('button:has-text("Broken")');
  
  if (await brokenGroup.isVisible()) {
    // Click to expand/collapse
    await brokenGroup.click();
    await page.waitForTimeout(500); // Animation delay
    
    // Take screenshot
    await page.screenshot({ path: './evidence/home-inbox-expanded.png' });
  }
});
```

### Click on a Skill from Inbox

```typescript
test('open skill from inbox row', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.waitForSelector('text=Home');
  
  // Find the first skill row in any inbox group
  const firstSkillRow = page.locator('[data-skill-name]').first();
  
  if (await firstSkillRow.isVisible()) {
    await firstSkillRow.click();
    
    // Wait for skill detail page to load
    await page.waitForSelector('text=Deployments', { timeout: 5000 });
    
    // Take screenshot of the opened skill
    await page.screenshot({ path: './evidence/home-skill-opened.png' });
  }
});
```

## Gotchas

1. **Empty State**: On a fresh install with no skills, the dashboard shows an empty state with a prompt to install skills. The stat tiles will show `0` and inbox groups will be empty.

2. **Data Loading**: The dashboard aggregates data from the Rust backend (`SkillSnapshot`). If the backend is slow or skills haven't been scanned yet, you might see a loading spinner. Wait for `isLoading` to become false before asserting on data.

3. **Collapsible State**: Inbox groups remember their open/closed state across sessions (likely stored in localStorage or Zustand). If a group is collapsed, you won't see its rows until you expand it.

4. **Row Actions**: Not all inbox rows have action buttons. "Recently used" rows show only a usage count, not an action button.

5. **Stat Tile Clicks**: Clicking a stat tile (Broken, Warnings, Updates) likely filters the Skills view, not the Home view. Verify the navigation behavior if you click a tile.

6. **Dynamic Counts**: Counts update live as you install/remove skills or as skills are invoked. Tests should not hardcode expected counts unless you're seeding test data.

## Observable End State

After driving this feature, you should observe:

- **Home view is visible** with heading "Home" or similar
- **Stat tiles render** showing numeric counts (may be 0)
- **Inbox groups render** (may be empty if no skills)
- **Clicking a skill row** navigates to the Skill Detail view
- **Screenshots captured** showing the dashboard layout

Success criteria:
- No JavaScript errors in browser console
- All UI elements render without visual glitches
- Navigation to/from Home works smoothly
- Evidence artifacts saved to `./evidence/home-*.png`
