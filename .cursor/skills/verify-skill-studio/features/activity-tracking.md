# Activity Tracking

## Sub-features

The Activity view shows invocation history and usage analytics:

1. **Invocation Heatmap** - Visual calendar showing daily skill usage:
   - **Grid view** - One cell per day, color-coded by invocation count
   - **Year view** - Full year at a glance (similar to GitHub contribution graph)
   - **Hover tooltips** - Show exact date and invocation count per day

2. **By Skill Table** - Breakdown of invocations per skill:
   - **Skill name** - Link to skill detail
   - **Total invocations** - Count for selected time window (24h/7d/30d)
   - **Prompt cost** - Total tokens used by this skill
   - **Last used** - Timestamp of most recent invocation

3. **By Project Table** - Breakdown of invocations per project:
   - **Project path** - Directory where skills were invoked
   - **Total invocations** - Count across all skills in that project
   - **Skills used** - List of skills invoked in this project

4. **Time Window Selector** - Toggle between:
   - **24 hours**
   - **7 days**
   - **30 days**

5. **Cost Summary** - Total prompt cost across all skills (may include token counts, estimated $$ cost)

## How to get to it (user POV)

1. Click **Activity** in the left sidebar
2. You're now on the Activity view

This view aggregates invocation data from all agents (Claude Code, Codex, OpenCode, pi) and shows when and how often skills are used.

## Driving it with Playwright

### Navigate to Activity View

```typescript
import { test, expect } from '@playwright/test';

test('navigate to activity view', async ({ page }) => {
  await page.goto('http://localhost:1420');
  
  // Wait for sidebar to load
  await page.waitForSelector('text=Activity', { timeout: 10000 });
  
  // Click Activity in sidebar
  await page.click('text=Activity');
  
  // Wait for activity page to load (look for heatmap or "Skill Activity" heading)
  await page.waitForSelector('text=Skill Activity, text=Activity, svg.heatmap', { timeout: 5000 });
  
  // Take screenshot
  await page.screenshot({ 
    path: './evidence/activity-view.png',
    fullPage: true 
  });
});
```

### Verify Heatmap Renders

```typescript
test('activity heatmap is visible', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Activity');
  await page.waitForTimeout(1000);
  
  // Look for SVG heatmap (similar to GitHub contribution graph)
  const heatmap = page.locator('svg, [data-testid="heatmap"], .heatmap');
  
  // Verify it's visible
  if (await heatmap.isVisible()) {
    await page.screenshot({ path: './evidence/activity-heatmap.png' });
  } else {
    // Might be empty state if no invocations yet
    await expect(page.locator('text=No activity yet, text=No invocations')).toBeVisible();
  }
});
```

### Check "By Skill" Table

```typescript
test('view invocations by skill', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Activity');
  await page.waitForTimeout(1000);
  
  // Look for "By Skill" section
  const bySkillHeading = page.locator('text=By Skill, text=Skill Breakdown');
  
  if (await bySkillHeading.isVisible()) {
    // Find the table
    const table = page.locator('table').filter({ has: bySkillHeading });
    
    // Verify table has rows
    const rowCount = await table.locator('tbody tr').count();
    console.log(`Found ${rowCount} skills with invocations`);
    
    // Take screenshot
    await page.screenshot({ path: './evidence/activity-by-skill.png' });
  }
});
```

### Switch Time Window

```typescript
test('switch activity time window', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Activity');
  await page.waitForTimeout(1000);
  
  // Find time window selector (24h, 7d, 30d)
  const window7d = page.locator('button:has-text("7d"), button:has-text("7 days")');
  
  if (await window7d.isVisible()) {
    await window7d.click();
    await page.waitForTimeout(500);
    
    // Verify the table/heatmap updates (data might change)
    await page.screenshot({ path: './evidence/activity-7d-window.png' });
  }
});
```

### Click on a Skill Name

```typescript
test('open skill detail from activity table', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Activity');
  await page.waitForTimeout(1000);
  
  // Find the first skill name in the "By Skill" table
  const firstSkillLink = page.locator('table tbody tr a, table tbody tr td:first-child').first();
  
  if (await firstSkillLink.isVisible()) {
    await firstSkillLink.click();
    
    // Wait for skill detail page to load
    await page.waitForSelector('text=Deployments', { timeout: 5000 });
    
    await page.screenshot({ path: './evidence/activity-skill-opened.png' });
  }
});
```

## Gotchas

1. **Empty State**: If no skills have been invoked yet, the Activity view shows an empty state:
   - Heatmap is blank or shows "No activity"
   - Tables are empty with a message like "No invocations yet"
   - This is normal on a fresh install

2. **Data Lag**: Invocation data comes from the backend's skill run database. There might be a slight delay between a skill running and it showing up in Activity. If testing after a fresh invocation, wait a few seconds or refresh.

3. **Time Window State**: The selected time window (24h/7d/30d) is stored in Zustand state and persists across views. If you switch to 7d, then go to Home and back to Activity, it will still be on 7d.

4. **Cost Calculation**: Prompt cost is calculated using tiktoken or a similar tokenizer. The cost might be an estimate, not exact billing amounts.

5. **Project Breakdown**: The "By Project" table only shows projects where skills have been invoked from project scope. Global skills won't have a project associated.

6. **Heatmap Interactivity**: Hovering over heatmap cells shows tooltips with date and count. These tooltips are rendered dynamically and might not be captured in screenshots.

7. **Long Tables**: If many skills have invocations, the tables can be long. Consider scrolling or using `fullPage: true` for screenshots.

## Observable End State

After driving this feature, you should observe:

- **Activity view is visible** with heading "Skill Activity" or similar
- **Heatmap renders** (or shows empty state if no invocations)
- **"By Skill" table shows** invocation counts and costs (or empty state)
- **Time window selector** is present and clickable
- **Clicking a skill name** navigates to Skill Detail view
- **Screenshots captured** showing the heatmap, tables, and time windows

Success criteria:
- No JavaScript errors in browser console
- Heatmap and tables render correctly (or show appropriate empty states)
- Time window switching updates the displayed data
- Navigation from Activity to Skill Detail works
- Evidence artifacts saved to `./evidence/activity-*.png`
