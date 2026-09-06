# Settings

App-level preferences: preferred code editor for opening skill folders, skills.sh API key (developer override), and theme (synced via Zustand store).

## Sub-features

**Open in Editor** (EditorPicker component):
- **Purpose** - Controls which application the "Open in editor" button in skill Locations card launches
- **Automatic option** - First radio item: "Automatic (<first-found-editor>)", uses first editor in `listInstalledEditors()` result
- **Editor list** - Radio buttons for each installed code editor found in `/Applications/` and `~/Applications/`:
  - VS Code
  - Cursor
  - Sublime Text
  - TextMate
  - Nova
  - (others detected via backend `scan_applications()`)
- **Selection** - Radio buttons with checkmark icon on selected item, saves immediately on change (no "Save" button)
- **Backend call** - `setPreferredEditor(app_name)` writes to `~/.agents/skill-studio.json`'s `preferred_editor` field
- **Empty state** - "No known code editor was found in your Applications folders." (rare, but possible on fresh macOS)

**skills.sh API Key** (SkillsShKeySetting component):
- **Purpose** - Developer override to browse skills.sh directly instead of through Skill Studio server
- **Input** - Password field, placeholder "Developer override: skills.sh API key"
- **Save button** - Disabled when input empty or while saving
- **Backend call** - `setSkillsShApiKey(key)` writes to `~/.agents/skill-studio.json`'s `skills_sh_api_key` field
- **Status line** (below input):
  - With key: "Using a local skills.sh key (developer override)"
  - Without key: "Browsing through the Skill Studio server at <url>"
- **Security** - Key never refetched or displayed after save (input always starts empty)
- **Installation note** - "Installing by source never needs a key" (clarifies key is only for browsing)

**Theme Selector** (managed in Zustand store, UI not in SettingsView):
- **Options** - System / Light / Dark
- **Location** - Controlled from sidebar footer theme toggle (sun/moon icon), not SettingsView component
- **Storage** - `theme` field in appStore, persisted to localStorage, applied as `data-theme` attr on `<html>`
- **System mode** - Follows OS preference via `window.matchMedia('(prefers-color-scheme: dark)')`

**Projects Management** (in Skills filter bar, not SettingsView):
- **Add project** - "Add project…" in scope dropdown → native folder picker (Tauri `open()`)
- **Remove project** - "Stop tracking <name>…" in scope dropdown → confirmation dialog → removes from `userAddedProjects` array
- **Storage** - `userAddedProjects` array in appStore, persisted to localStorage

## How to get to it (user POV)

1. Click "Settings" in sidebar (gear icon + text)
2. Page loads with two sections: "Open in editor" and "skills.sh"

**Theme**: Click sun/moon icon in sidebar footer (not in Settings view)
**Projects**: Click "Project" dropdown in Skills filter bar → "Add project…" or "Stop tracking…"

## Driving it with Playwright

```typescript
// Navigate to Settings
await page.goto('http://localhost:1420');
await page.click('button:has-text("Settings")');
await page.waitForSelector('text=Open in editor');

// Verify editor options render
const editorSection = page.locator('text=Open in editor').locator('..');
await expect(editorSection).toBeVisible();
const radioCount = await editorSection.locator('input[type="radio"]').count();
console.log(`Found ${radioCount} editor options`);

// Select an editor (e.g. Cursor)
const cursorOption = page.locator('label:has-text("Cursor")');
if (await cursorOption.isVisible()) {
  await cursorOption.click();
  await page.waitForTimeout(200); // Saves immediately
  // Verify checkmark appears
  const checkmark = cursorOption.locator('svg');
  await expect(checkmark).toBeVisible();
}

// Enter skills.sh API key
const keyInput = page.locator('input[type="password"][placeholder*="skills.sh"]');
await keyInput.fill('sk_test_1234567890abcdef');
await page.click('button:has-text("Save")');
await page.waitForTimeout(500); // Backend save

// Verify status line updates
const statusLine = page.locator('text=Using a local skills.sh key');
await expect(statusLine).toBeVisible();

// Clear key (input is now empty after save, need to remove from backend)
// (No UI for removal - requires manual file edit or backend call)

// Change theme (via sidebar, not Settings page)
await page.click('button[aria-label="Toggle theme"]'); // Or by icon selector
// Verify theme changed (check html data-theme attr)
const theme = await page.locator('html').getAttribute('data-theme');
console.log(`Current theme: ${theme}`);
```

Selectors:
- Editor section: `text=Open in editor`
- Editor radio items: `label` within editor section, by text (e.g. `label:has-text("VS Code")`)
- Selected checkmark: `svg` within selected label
- skills.sh input: `input[type="password"][placeholder*="skills.sh"]`
- Save button: `button:has-text("Save")` (sibling of input)
- Status line: `text=Using a local skills.sh key` or `text=Browsing through the Skill Studio server`
- Theme toggle: `button[aria-label="Toggle theme"]` (in sidebar footer, not Settings)

## Gotchas

- **Editor selection saves immediately** - No "Save" button, clicking radio triggers backend call
- **No editor unselect** - Radio group always has one selected (can't go back to "none")
- **Automatic option dynamic** - Label shows first detected editor, e.g. "Automatic (VS Code)" (changes per machine)
- **skills.sh key hidden** - Input is password field, never refetched after save (always starts empty on reload)
- **Key removal requires file edit** - No "Clear key" button in UI, must delete from `~/.agents/skill-studio.json` manually
- **Theme not in Settings** - Theme control lives in sidebar footer, not SettingsView component
- **Projects not in Settings** - Project management lives in Skills filter bar, not SettingsView
- **Empty editor list edge case** - If no editors detected, shows message, no radio buttons
- **Editor picker loading state** - Shows "Looking for installed editors…" while loading (async backend call)
- **skills.sh mode auto-detection** - Backend `resolve_skills_sh_access()` checks key file, server env, falls back to default

## Branches

### Happy path: Change editor, save key
1. User clicks Settings → page loads, editor section shows list (or loading message)
2. User clicks "Cursor" radio → checkmark moves, backend call `setPreferredEditor("Cursor")` runs
3. Success → selection stays, next "Open in editor" uses Cursor
4. User types key into skills.sh input → "Save" button enables
5. User clicks "Save" → backend writes key, input clears, status line updates "Using a local skills.sh key"

### Empty / first-run states
- **No editors detected** → "No known code editor was found in your Applications folders." message, no radio buttons
- **No key saved** → Status line shows "Browsing through the Skill Studio server at <url>"
- **Automatic option when empty list** → "Automatic" (no editor name in label)

### Duplicate / conflict states
- **Multiple VS Code installs** → Backend dedupes by app name, shows once
- **Key already saved** → Input starts empty (security: never refetched), status line shows "Using a local key"

### Failure / error states
- **Editor save failure** → Toast "Couldn't save your editor: <error>", selection reverts to previous
- **Key save failure** → Toast "Couldn't save your skills.sh key: <error>", input stays filled, status line unchanged
- **Editor list load failure** → Toast "Couldn't read your editor setting: <error>", section shows error or empty

### Cancellation / close mid-flow
- **Navigate away after editor change** → Change already saved (no cancel needed)
- **Close after typing key but before Save** → Key not saved (input state lost)
- **Browser refresh** → Editor selection persists (saved immediately), unsaved key lost

### Loading / progress states
- **Initial editor load** → "Looking for installed editors…" message, no radio buttons yet
- **Editor selection saving** → No spinner (instant radio check move), backend call async in background
- **Key save in progress** → "Save" button shows "Saving…" or spinner, disabled

## Benchmarks & improvement

### Observable metrics
- **Settings page render time**: Nav click → sections visible
  - Measure: Route change → editor section + key section rendered
  - Target: < 200ms (two backend reads on mount)
- **Editor list load duration**: Component mount → radio buttons visible
  - Measure: `listInstalledEditors()` call → options rendered
  - Target: < 300ms (filesystem scan + parse)
- **Editor selection save latency**: Radio click → confirmed
  - Measure: Click → backend write complete (no UI feedback, async)
  - Target: < 100ms (JSON write to home dir)
- **Key save duration**: Button click → success state
  - Measure: Click → input clears + status line updates
  - Target: < 200ms (JSON write + re-read)

### Current instrumentation
- Loading states shown in UI ("Looking for installed editors…")
- Error toasts on failure (with error messages)
- No save duration logging, backend timing, or success rate tracking

### Suggested measurements for verification
- **Editor detection coverage**: Verify all common editors detected (VS Code, Cursor, Sublime, etc.)
- **Automatic fallback correctness**: Verify first editor in list matches "Automatic" label
- **Key save idempotency**: Save same key twice, verify no errors or duplicate writes
- **Key clear mechanism**: Test manual file edit → status line updates on reload
- **Theme persistence**: Verify theme survives app restart (localStorage roundtrip)

### Improvement levers
- **Cache editor list** (currently re-scans on every Settings mount; cache for 1h would speed)
- **Add "Clear key" button** (currently requires manual file edit; one-click clear would improve UX)
- **Prefetch editor list** (load on app start, not on Settings mount; Settings would show instantly)
- **Show save confirmation** (currently silent success; brief toast or checkmark would confirm)
- **Validate key format** (currently accepts any string; regex check for Vercel key pattern would catch typos early)
- **Theme preview** (currently requires sidebar toggle; inline preview in Settings would let users test without committing)

## Observable end state

After each interaction:

- **Navigate to Settings**: Page shows "Open in editor" section (loading → radio buttons) + "skills.sh" section (input + status line)
- **Select editor**: Checkmark moves to clicked option, backend saves immediately
- **Save key**: Input clears, status line updates "Using a local skills.sh key", next SkillStore browse uses key
- **Invalid key (wrong format)**: Backend accepts any string (no validation), key saved but may fail on use
- **Empty editor list**: Message "No known code editor was found" shows, no radio buttons
- **Automatic option**: Always present (top of list), label shows first editor or just "Automatic"

Invariants:
- Editor radio group always has one selected (Automatic or specific editor)
- skills.sh input never shows saved key (password field, never refetched)
- Status line reflects actual backend state (queries `getSkillsShAccess()` on mount)
- Editor selection saves on click (no "Save" button, immediate backend write)
- Key requires explicit "Save" click (input state not synced to backend until submitted)
- Theme state managed outside Settings (sidebar toggle, not SettingsView component)
