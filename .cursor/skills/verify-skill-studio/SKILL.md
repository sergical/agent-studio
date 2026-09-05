---
name: verify-skill-studio
description: End-to-end verification of Skill Studio, a Tauri desktop app for managing AI coding assistant configurations. Use after code changes to verify the UI, navigation, and core workflows.
---

# verify-skill-studio

Verify Skill Studio's desktop UI and core functionality by launching the Tauri dev server, driving the app with Playwright, capturing evidence, and cleaning up.

## Surface

Skill Studio is a **Tauri 2.x desktop application** with:
- **Frontend**: React 19 + TypeScript + Tailwind CSS 4.x, running on `http://localhost:1420`
- **Backend**: Rust (Tauri 2.x)
- **Primary views**:
  - **Home**: Dashboard showing broken/warning/update stats, invocation counts, and skill inbox
  - **Skills**: Filterable list of installed skills across all agents and scopes
  - **Plugins**: Skills from native plugin caches (Claude Code, Codex)
  - **Activity**: Invocation history with heatmap and per-skill breakdowns
  - **Packs**: Skill pack management
  - **Settings**: App preferences and configuration

The app manages configuration files for Claude Code, OpenCode, Codex, pi, and other AI coding assistants.

## Prerequisites

Before running this skill:

```bash
# Install dependencies (if not already done)
cd /workspace && npm install

# Install Playwright browsers (first time only)
npx playwright install chromium

# Ensure Rust dependencies are available
cd apps/desktop/src-tauri && cargo build
```

## Launch

Start the Tauri development server in a dedicated tmux session:

```bash
SESSION_NAME="skill-studio-dev"
tmux -f /exec-daemon/tmux.portal.conf has-session -t "=$SESSION_NAME" 2>/dev/null || \
  tmux -f /exec-daemon/tmux.portal.conf new-session -d -s "$SESSION_NAME" -c /workspace

# Start the dev server
tmux -f /exec-daemon/tmux.portal.conf send-keys -t "$SESSION_NAME:0.0" \
  'cd /workspace && npm run tauri dev' C-m
```

**Ready signal**: Wait for both:
1. Vite dev server logs `Local:   http://localhost:1420/`
2. Tauri window opens (look for `WebView loaded` or similar in logs)

Typical startup time: 10-15 seconds for Vite, then 5-10 seconds for Tauri window.

**Port**: The Vite dev server uses port **1420**.

**Isolation**: Only one instance can run at a time (single dev server on port 1420). Never start a second instance while one is already running.

## Doctor

Verify the app is running and ready before driving:

```bash
# 1. Check if Vite dev server is responding
curl -s -o /dev/null -w "%{http_code}" http://localhost:1420/

# Expected: 200

# 2. Check tmux session is still alive
tmux -f /exec-daemon/tmux.portal.conf has-session -t "=skill-studio-dev" 2>/dev/null
echo $?

# Expected: 0 (session exists)

# 3. Verify port 1420 is owned by node
lsof -ti:1420 | xargs ps -p | grep node

# Expected: Shows the node process running Vite
```

If any check fails:
- **Port check fails (not 200)**: Server might still be starting or crashed. Check tmux logs.
- **Session missing**: Server was stopped or never started. Re-run Launch.
- **Port owned by different process**: Another process is blocking port 1420. Kill it or use a different port (update `tauri.conf.json`).

## Drive

Use **Playwright** to drive the Tauri app via the WebView's HTTP endpoint (`http://localhost:1420`).

### Setup (if not already done)

```bash
# Install Playwright in the workspace
cd /workspace && npm install -D @playwright/test
npx playwright install chromium
```

### Basic Test Pattern

Create test scripts in `/workspace/.cursor/skills/verify-skill-studio/tests/`:

```typescript
import { test, expect } from '@playwright/test';

test.describe('Skill Studio', () => {
  test.beforeEach(async ({ page }) => {
    // Navigate to the Tauri dev server
    await page.goto('http://localhost:1420');
    
    // Wait for app to be ready (check for sidebar presence)
    await page.waitForSelector('[data-testid="sidebar"]', { timeout: 10000 });
  });

  test('home view shows dashboard stats', async ({ page }) => {
    // Verify we're on the home view
    await expect(page.locator('text=Home')).toBeVisible();
    
    // Check for stat tiles (broken, warnings, updates)
    const statTiles = page.locator('[data-testid="stat-tile"]');
    await expect(statTiles).toHaveCount(3);
    
    // Take a screenshot for evidence
    await page.screenshot({ 
      path: '/workspace/.cursor/skills/verify-skill-studio/evidence/home-dashboard.png',
      fullPage: true 
    });
  });
});
```

### Key Selectors

Since the app doesn't have extensive `data-testid` attributes yet, use these selectors:

- **Sidebar navigation**: Look for text content (`text=Home`, `text=Skills`, etc.)
- **Skill list rows**: Table rows with skill names
- **Buttons**: Match by text (`button:has-text("Update")`, `button:has-text("Install")`)
- **Modal dialogs**: `[role="dialog"]` or by title text
- **Toast notifications**: Look for `.sonner-toast` or similar

### Driving Common Features

#### Navigate to Skills View

```typescript
await page.click('text=Skills');
await page.waitForSelector('text=Installed Skills', { timeout: 5000 });
await page.screenshot({ path: './evidence/skills-view.png' });
```

#### Open Skill Detail

```typescript
// Click on a skill row (assuming at least one skill exists)
const firstSkillRow = page.locator('table tbody tr').first();
await firstSkillRow.click();

// Wait for skill detail page to load
await page.waitForSelector('[data-testid="skill-detail"]', { timeout: 5000 });
await page.screenshot({ path: './evidence/skill-detail.png' });
```

#### Navigate to Activity View

```typescript
await page.click('text=Activity');
await page.waitForSelector('text=Skill Activity', { timeout: 5000 });
await page.screenshot({ path: './evidence/activity-view.png' });
```

## Evidence

Capture and preserve proof that the app works:

### Screenshot Evidence

Save screenshots to `/workspace/.cursor/skills/verify-skill-studio/evidence/`:

```bash
mkdir -p /workspace/.cursor/skills/verify-skill-studio/evidence
```

Required evidence for a complete verification run:
1. **Home view**: `home-dashboard.png` — shows stat tiles and skill inbox
2. **Skills list**: `skills-view.png` — shows the filterable skill table
3. **Skill detail**: `skill-detail.png` — opens a skill's full page view
4. **Navigation**: `sidebar-nav.png` — captures sidebar with all view options

### Console Logs

Capture browser console output to detect JavaScript errors:

```typescript
page.on('console', msg => {
  if (msg.type() === 'error') {
    console.error('Browser console error:', msg.text());
  }
});
```

### Test Results

Save Playwright test results and HTML report:

```bash
# Run tests with JSON reporter
npx playwright test --reporter=json,html

# Results saved to:
# - playwright-report/index.html (interactive HTML report)
# - test-results/ (per-test artifacts)
```

Copy the HTML report to evidence:

```bash
cp -r playwright-report /workspace/.cursor/skills/verify-skill-studio/evidence/
```

### Proof Standards

Evidence must demonstrate:
- **Real user path**: Navigate through the UI as a user would (no internal API calls)
- **Action + resulting state**: Show both the action (button click) and the outcome (new view loaded)
- **Side effects**: Verify DOM updates, route changes, and UI state changes
- **No mocks at app boundaries**: Only mock external services (skills.sh API, filesystem) if necessary

## Cleanup

After verification, tear down the dev server **only if YOU started it** in Launch:

```bash
# Kill the tmux session gracefully
SESSION_NAME="skill-studio-dev"
tmux -f /exec-daemon/tmux.portal.conf kill-session -t "=$SESSION_NAME" 2>/dev/null

# Verify port 1420 is released
lsof -ti:1420 || echo "Port 1420 released"
```

**Never kill by process name** (`pkill node`, `killall vite`) — other processes might be running.

**Never delete evidence** — screenshots and test artifacts must persist after cleanup for review.

**Evidence location after cleanup**: All artifacts remain in `/workspace/.cursor/skills/verify-skill-studio/evidence/` and can be committed to the repo or attached to a PR.

## Common Issues

### Port 1420 Already in Use

```bash
# Find what's using the port
lsof -ti:1420 | xargs ps -p

# If it's a stale dev server, kill it by PID
lsof -ti:1420 | xargs kill
```

### Tauri Window Won't Open

Check tmux logs for errors:

```bash
tmux -f /exec-daemon/tmux.portal.conf attach-session -t "=skill-studio-dev"
# Press Ctrl-C to stop, then investigate error messages
```

Common causes:
- **Missing GTK libraries**: Re-run system dependency install (see repo's README or AGENTS.md)
- **Rust build failed**: Run `cd apps/desktop/src-tauri && cargo build` to see errors

### Playwright Can't Connect

If Playwright times out connecting to `http://localhost:1420`:
1. Verify Vite server is running: `curl http://localhost:1420`
2. Check if Tauri window is visible: Look for the Skill Studio window
3. Increase timeout in test: `await page.goto('http://localhost:1420', { timeout: 30000 })`

### No Skills Found

The app shows an empty state if no skills are installed. To test with skills:
1. Install a skill manually via the UI (Skills tab → Install button)
2. OR: Seed test skills in `~/.claude/skills/` or `~/.agents/skills/`

## See Also

- **Feature map**: See `/workspace/.cursor/skills/verify-skill-studio/features/README.md` for detailed feature breakdown
- **Playwright docs**: https://playwright.dev/docs/api/class-test
- **Tauri docs**: https://tauri.app/v2/
