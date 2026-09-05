# Skills Management

## Sub-features

The Skills view provides a unified, filterable list of all installed skills:

1. **Skill List Table** - Shows all installed skills with columns:
   - **Name** - Skill name (clickable to open detail)
   - **Location** - Where the skill is deployed (Global, Project, Parked)
   - **Agent** - Which agent(s) use this skill (Claude Code, Codex, OpenCode, pi, shared)
   - **Status** - Health indicators (broken, warnings, updates available)
   - **Last Used** - Timestamp of most recent invocation

2. **Filter Bar** - Controls to narrow the list:
   - **Search box** - Filter by skill name
   - **Scope filter** - All / Global / Project / Parked
   - **Agent filter** - All / Claude Code / Codex / OpenCode / pi / shared
   - **Status filter** - All / Broken / Warnings / Updates / Unused

3. **Toolbar Actions**:
   - **Refresh** - Re-scan skill directories
   - **Install** - Add a new skill from skills.sh or GitHub
   - **Select** - Enter multi-select mode to create a pack

4. **Coverage Matrix** - Alternative view showing which agents have which skills (toggle via a switch)

## How to get to it (user POV)

1. Click **Skills** in the left sidebar
2. You're now on the Skills list view

The view shows all your installed skills across all agents and scopes in one unified table.

## Driving it with Playwright

### Navigate to Skills View

```typescript
import { test, expect } from '@playwright/test';

test('navigate to skills list', async ({ page }) => {
  await page.goto('http://localhost:1420');
  
  // Wait for sidebar to load
  await page.waitForSelector('text=Skills', { timeout: 10000 });
  
  // Click Skills in sidebar
  await page.click('text=Skills');
  
  // Wait for skills table to load
  await page.waitForSelector('table, text=Installed Skills', { timeout: 5000 });
  
  // Take screenshot
  await page.screenshot({ 
    path: './evidence/skills-list-view.png',
    fullPage: true 
  });
});
```

### Filter Skills by Search

```typescript
test('search for a skill by name', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Skills');
  await page.waitForSelector('table');
  
  // Find the search input
  const searchBox = page.locator('input[type="text"]').first();
  
  // Type a search query
  await searchBox.fill('test');
  
  // Wait for table to update
  await page.waitForTimeout(500);
  
  // Take screenshot of filtered results
  await page.screenshot({ path: './evidence/skills-search-filtered.png' });
});
```

### Filter by Scope

```typescript
test('filter skills by scope', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Skills');
  await page.waitForSelector('table');
  
  // Find scope filter buttons (All, Global, Project, Parked)
  const globalFilter = page.locator('button:has-text("Global")');
  
  if (await globalFilter.isVisible()) {
    await globalFilter.click();
    await page.waitForTimeout(500);
    
    // Verify table shows only global skills
    await page.screenshot({ path: './evidence/skills-global-filtered.png' });
  }
});
```

### Open a Skill Detail

```typescript
test('open skill detail from list', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Skills');
  await page.waitForSelector('table tbody tr');
  
  // Click the first skill row
  const firstRow = page.locator('table tbody tr').first();
  await firstRow.click();
  
  // Wait for skill detail page to load
  await page.waitForSelector('text=Deployments', { timeout: 5000 });
  
  // Take screenshot
  await page.screenshot({ path: './evidence/skill-detail-opened.png' });
});
```

### Refresh Skill List

```typescript
test('refresh skill list', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Skills');
  await page.waitForSelector('table');
  
  // Find and click the refresh button
  const refreshButton = page.locator('button[aria-label="Refresh"], button:has-text("Refresh")');
  
  if (await refreshButton.isVisible()) {
    await refreshButton.click();
    
    // Wait for refresh to complete (look for loading spinner to disappear)
    await page.waitForTimeout(1000);
    
    await page.screenshot({ path: './evidence/skills-refreshed.png' });
  }
});
```

## Gotchas

1. **Empty State**: If no skills are installed, the list shows an empty state with a message like "No skills found" and a button to install one.

2. **Loading State**: The table shows a loading spinner while the backend scans skill directories. Tests should wait for `isLoading` to be false before asserting on table content.

3. **Multi-Select Mode**: Clicking the "Select" button enters multi-select mode, showing checkboxes in each row. This changes the UI state — tests should account for this if clicking Select.

4. **Coverage Matrix Toggle**: There's a toggle to switch between table view and coverage matrix view. Make sure you're in the right view mode for your test.

5. **Skill Uniqueness**: A skill name can appear multiple times in the list if it's deployed to multiple scopes (Global, Project) or agents. Each row represents a unique deployment path.

6. **Filter Combinations**: Filters stack — you can search by name AND filter by scope AND filter by status all at once. The table updates dynamically.

7. **Table Sorting**: Clicking column headers might sort the table (if implemented). Tests should verify the sort state if this matters.

## Observable End State

After driving this feature, you should observe:

- **Skills list view is visible** with a table of skills (or empty state)
- **Filter bar is present** with search box and scope/agent/status filters
- **Clicking a skill row** navigates to Skill Detail view
- **Filtering works** — table updates to show only matching skills
- **Refresh button works** — triggers a re-scan (observable via loading state or updated data)
- **Screenshots captured** showing the list, filters, and filtered results

Success criteria:
- No JavaScript errors in browser console
- Table renders correctly with skill data
- Filters update the table without full page reload
- Navigation from list to detail works
- Evidence artifacts saved to `./evidence/skills-*.png`
