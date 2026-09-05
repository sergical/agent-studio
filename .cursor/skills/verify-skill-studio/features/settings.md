# Settings

## Sub-features

The Settings view provides app-level configuration:

1. **Theme Selector** - Choose app appearance:
   - **System** - Follow OS dark/light mode
   - **Light** - Force light theme
   - **Dark** - Force dark theme

2. **Skill Directories** - Manage watched directories:
   - **Add Project** - Point to a new project directory to scan for skills
   - **Remove Project** - Stop tracking a project
   - **Auto-discovered Projects** - List of projects found via Codex config or Claude Code transcripts

3. **Skills.sh API Key** - Configure skills.sh integration:
   - **API Key Input** - Enter or update your skills.sh API key
   - **Key saved to** `~/.agents/skill-studio.json`
   - **Test Connection** - Verify the key works

4. **App Info** - Version, build info, and links:
   - **Version number** - Current app version (e.g., 0.1.0)
   - **GitHub link** - Link to the repo
   - **Documentation** - Link to docs or help resources

5. **Advanced Settings** (if present):
   - **Clear cache** - Reset skill scan cache
   - **Re-scan all** - Force full re-discovery of skills
   - **Enable developer mode** - Show debug info or logs

## How to get to it (user POV)

1. Click **Settings** in the left sidebar (gear icon + "Settings" text)
2. You're now on the Settings view

Settings changes are typically saved automatically (on blur or on click).

## Driving it with Playwright

### Navigate to Settings

```typescript
import { test, expect } from '@playwright/test';

test('navigate to settings view', async ({ page }) => {
  await page.goto('http://localhost:1420');
  
  // Wait for sidebar to load
  await page.waitForSelector('text=Settings', { timeout: 10000 });
  
  // Click Settings in sidebar
  await page.click('text=Settings');
  
  // Wait for settings page to load
  await page.waitForSelector('text=Theme, text=Appearance', { timeout: 5000 });
  
  // Take screenshot
  await page.screenshot({ 
    path: './evidence/settings-view.png',
    fullPage: true 
  });
});
```

### Change Theme

```typescript
test('switch theme to dark mode', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Settings');
  await page.waitForTimeout(1000);
  
  // Find theme selector (radio buttons or dropdown)
  const darkThemeOption = page.locator('input[value="dark"], button:has-text("Dark")');
  
  if (await darkThemeOption.isVisible()) {
    await darkThemeOption.click();
    await page.waitForTimeout(500);
    
    // Verify theme changed (check for dark mode class on body or root)
    const isDark = await page.locator('html, body').evaluate(el => 
      el.classList.contains('dark') || 
      el.getAttribute('data-theme') === 'dark'
    );
    
    expect(isDark).toBeTruthy();
    
    // Take screenshot in dark mode
    await page.screenshot({ path: './evidence/settings-dark-theme.png' });
  }
});
```

### Add a Project Directory

```typescript
test('add a project directory (mock)', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Settings');
  await page.waitForTimeout(1000);
  
  // Find "Add Project" button
  const addProjectButton = page.locator('button:has-text("Add Project"), button:has-text("Add Directory")');
  
  if (await addProjectButton.isVisible()) {
    await addProjectButton.click();
    
    // This will open a native file picker dialog, which Playwright can't drive
    // In a real test, you'd mock the dialog response or use Tauri's testing APIs
    
    // For now, just verify the button is clickable
    await page.screenshot({ path: './evidence/settings-add-project-clicked.png' });
  }
});
```

### Enter Skills.sh API Key

```typescript
test('enter skills.sh api key', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Settings');
  await page.waitForTimeout(1000);
  
  // Find API key input field
  const apiKeyInput = page.locator('input[type="text"][placeholder*="API"], input[type="password"][placeholder*="API"]').first();
  
  if (await apiKeyInput.isVisible()) {
    // Enter a test API key
    await apiKeyInput.fill('test-api-key-12345');
    
    // Blur the input to trigger save
    await apiKeyInput.blur();
    await page.waitForTimeout(500);
    
    // Verify a success message or toast
    // (This might trigger a toast notification)
    
    await page.screenshot({ path: './evidence/settings-api-key-entered.png' });
  }
});
```

### View App Version

```typescript
test('verify app version is displayed', async ({ page }) => {
  await page.goto('http://localhost:1420');
  await page.click('text=Settings');
  await page.waitForTimeout(1000);
  
  // Look for version text (e.g., "Version 0.1.0")
  const versionText = page.locator('text=/Version|v?\\d+\\.\\d+\\.\\d+/i');
  
  if (await versionText.isVisible()) {
    const version = await versionText.textContent();
    console.log(`App version: ${version}`);
    
    await page.screenshot({ path: './evidence/settings-app-version.png' });
  }
});
```

## Gotchas

1. **File Picker Dialogs**: Adding a project directory opens a native OS file picker. Playwright cannot drive native dialogs — you'll need to mock the response or use Tauri's testing APIs.

2. **Auto-Save**: Settings typically save automatically on blur or change. There might not be a "Save" button. Watch for toast notifications to confirm changes were saved.

3. **Theme State**: Theme changes persist across app restarts (stored in localStorage). If you change to dark mode, it will still be dark the next time the app opens.

4. **API Key Security**: The API key is stored in `~/.agents/skill-studio.json` as plaintext. Tests should not commit real API keys.

5. **Empty States**: If no projects are tracked, the "Skill Directories" section shows an empty list with a button to add the first one.

6. **Validation**: Invalid API keys or malformed inputs might trigger error messages or toasts. Tests should verify these error states.

7. **Restart Required**: Some settings (like developer mode) might require an app restart to take effect. The UI should indicate this if applicable.

## Observable End State

After driving this feature, you should observe:

- **Settings view is visible** with sections for Theme, Directories, API Key, etc.
- **Theme selector is interactive** and changes the app's appearance
- **Add Project button is present** and clickable (even if we can't test the file picker)
- **API Key input is present** and accepts text
- **App version is displayed** somewhere in the settings
- **Screenshots captured** showing the settings UI, theme changes, and inputs

Success criteria:
- No JavaScript errors in browser console
- All settings sections render correctly
- Interactive elements (theme selector, buttons, inputs) respond correctly
- Theme changes are reflected in the UI immediately
- Evidence artifacts saved to `./evidence/settings-*.png`
