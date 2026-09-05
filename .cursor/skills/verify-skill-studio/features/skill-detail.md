# Skill Detail View

## Sub-features

The Skill Detail page shows comprehensive information about a single skill:

1. **Header** - Skill name, description, and action buttons:
   - **Update** - Pull latest version from upstream (if available)
   - **Remove** - Delete this skill deployment
   - **Back** - Return to previous view (Home or Skills list)

2. **Deployments Card** - Shows where this skill is installed:
   - **Scope** - Global, Project, or Parked
   - **Agent** - Claude Code, Codex, OpenCode, pi, or shared
   - **Path** - Full filesystem path to SKILL.md
   - **Status** - Enabled/Disabled toggle per deployment

3. **Skill Content** - Markdown rendering of SKILL.md:
   - **Frontmatter** - YAML metadata (name, description, invocation)
   - **Body** - Full markdown content with syntax highlighting
   - **Edit button** - Opens inline editor (Monaco)

4. **Test Panel** (optional) - If the skill has a test harness:
   - **Test Input** - Form to provide test parameters
   - **Run Test** - Execute the skill in test mode
   - **Test Results** - Output from test run

5. **Activity** - Invocation history for this skill:
   - **Recent runs** - Timestamps, durations, outcomes
   - **Heatmap** - Visual usage pattern over time

6. **Issues Panel** (if applicable) - Health warnings or errors:
   - **Spec violations** - SKILL.md format issues
   - **Missing dependencies** - External tools/packages the skill needs
   - **Update available** - Link to upstream changes

## How to get to it (user POV)

From **Home** or **Skills** view:
1. Click on any skill row in the list or inbox
2. The skill detail page opens, showing full information about that skill

Alternatively:
- Deep link via URL: `/skill/:name` (if the app supports routing)

## Driving it with Playwright

### Open Skill Detail

```typescript
import { test, expect } from '@playwright/test';

test('open skill detail from skills list', async ({ page }) => {
  await page.goto('http://localhost:1420');
  
  // Navigate to Skills view
  await page.click('text=Skills');
  await page.waitForSelector('table tbody tr');
  
  // Click the first skill row
  const firstRow = page.locator('table tbody tr').first();
  const skillName = await firstRow.locator('td').first().textContent();
  
  await firstRow.click();
  
  // Wait for detail page to load
  await page.waitForSelector('text=Deployments', { timeout: 5000 });
  
  // Verify skill name is shown in header
  await expect(page.locator('h1, h2').filter({ hasText: skillName || '' })).toBeVisible();
  
  // Take screenshot
  await page.screenshot({ 
    path: './evidence/skill-detail-page.png',
    fullPage: true 
  });
});
```

### View Deployments

```typescript
test('view skill deployments', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Skills');
  await page.waitForSelector('table tbody tr');
  
  // Open first skill
  await page.locator('table tbody tr').first().click();
  await page.waitForSelector('text=Deployments');
  
  // Find the deployments card
  const deploymentsCard = page.locator('text=Deployments').locator('..');
  
  // Verify it shows scope and path
  await expect(deploymentsCard).toContainText(/Global|Project|Parked/i);
  
  // Take screenshot
  await page.screenshot({ path: './evidence/skill-deployments.png' });
});
```

### Read Skill Markdown Content

```typescript
test('view skill markdown content', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Skills');
  await page.waitForSelector('table tbody tr');
  
  // Open first skill
  await page.locator('table tbody tr').first().click();
  await page.waitForSelector('text=Deployments');
  
  // Look for markdown rendering (code blocks, headings, etc.)
  const markdownContent = page.locator('article, .markdown-body, [data-markdown]');
  
  // Verify content is visible
  if (await markdownContent.isVisible()) {
    await page.screenshot({ path: './evidence/skill-markdown-content.png', fullPage: true });
  }
});
```

### Toggle Deployment Status

```typescript
test('toggle skill deployment enable/disable', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Skills');
  await page.waitForSelector('table tbody tr');
  
  // Open first skill
  await page.locator('table tbody tr').first().click();
  await page.waitForSelector('text=Deployments');
  
  // Find the enable/disable toggle switch
  const toggle = page.locator('input[type="checkbox"][role="switch"]').first();
  
  if (await toggle.isVisible()) {
    const wasChecked = await toggle.isChecked();
    
    // Toggle it
    await toggle.click();
    
    // Wait for state change
    await page.waitForTimeout(500);
    
    // Verify state changed
    const nowChecked = await toggle.isChecked();
    expect(nowChecked).not.toBe(wasChecked);
    
    await page.screenshot({ path: './evidence/skill-toggle-changed.png' });
  }
});
```

### Go Back to Previous View

```typescript
test('return to previous view with back button', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Skills');
  await page.waitForSelector('table tbody tr');
  
  // Open a skill
  await page.locator('table tbody tr').first().click();
  await page.waitForSelector('text=Deployments');
  
  // Click the Back button
  const backButton = page.locator('button:has-text("Back"), button[aria-label="Back"]');
  await backButton.click();
  
  // Verify we're back on Skills list
  await page.waitForSelector('table tbody tr', { timeout: 5000 });
  
  await page.screenshot({ path: './evidence/skill-back-to-list.png' });
});
```

## Gotchas

1. **No Skill Selected**: If you navigate directly to `/skill` without a name, the app might show an error or empty state. Always open from a list.

2. **Multiple Deployments**: A skill can have multiple deployments (Global + Project, or multiple projects). The Deployments card will show multiple rows. Make sure your test handles this case.

3. **Edit Mode**: Clicking "Edit" on the markdown content switches to an inline Monaco editor. The UI changes significantly — you'll need separate test logic for edit mode.

4. **Async Updates**: Toggling deployment status or clicking Update triggers async operations. Wait for toast notifications or loading states to disappear before asserting on results.

5. **Missing Skill**: If the skill was deleted or moved between loading the list and opening detail, you might see an error message. This is rare but possible in concurrent scenarios.

6. **Long Content**: SKILL.md files can be very long. Use `fullPage: true` for screenshots to capture everything, or scroll manually to specific sections.

7. **Test Panel Availability**: Not all skills have a test panel — it depends on whether the skill defines test parameters. Check for its presence before trying to interact with it.

## Observable End State

After driving this feature, you should observe:

- **Skill detail page is visible** with header showing skill name
- **Deployments card shows** scope, agent, path, and status
- **Markdown content renders** with proper formatting
- **Back button works** and returns to previous view (Skills list or Home)
- **Toggle changes** deployment status (observable via UI state change)
- **Screenshots captured** showing the detail page, deployments, and content

Success criteria:
- No JavaScript errors in browser console
- All sections (header, deployments, content) render correctly
- Navigation to/from detail page works
- Interactive elements (toggle, back button) respond correctly
- Evidence artifacts saved to `./evidence/skill-detail-*.png`
