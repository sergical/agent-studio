# Skill Studio - Feature Coverage Matrix

This matrix lists every user-facing surface in Skill Studio and its documentation/verification status.

## Coverage Legend

- ✅ **Fully Mapped** - Complete feature documentation with all H2 sections (sub-features, driving, gotchas, branches, benchmarks, end state)
- 📝 **Partially Mapped** - Feature documented but needs expansion (missing sub-features, branches, or benchmarks)
- ⚠️ **Mapped but Unproven** - Documentation exists but no Playwright verification or evidence captured
- ❌ **Not Mapped** - No documentation yet (should not appear in this final matrix)

## Primary Views & Navigation

| Surface | File | Status | Proven | Notes |
|---------|------|--------|--------|-------|
| Sidebar navigation | `sidebar-navigation.md` | ✅ Fully Mapped | ⚠️ Unproven | Search, add skill, view nav, theme toggle |
| Home dashboard | `home-dashboard.md` | 📝 Partially Mapped | ⚠️ Unproven | Has basic coverage, needs stat tiles, trial restore, linked-root dialog expansion |
| Skills list | `skills-management.md` | 📝 Partially Mapped | ⚠️ Unproven | Has basic coverage, needs filters, search, selection mode, create pack expansion |
| Plugins view | `plugins-view.md` | ✅ Fully Mapped | ⚠️ Unproven | Read-only plugin skills from Claude Code/Codex caches |
| Activity tracking | `activity-tracking.md` | 📝 Partially Mapped | ⚠️ Unproven | Has basic coverage, needs heatmap, restore operations expansion |
| Packs management | `packs-management.md` | ✅ Fully Mapped | ⚠️ Unproven | Create, update, publish, import, delete |
| Learn sections | `learn-sections.md` | ✅ Fully Mapped | ⚠️ Unproven | 4 explainer sections (broken, invoke, cost, unused) |
| Settings | `settings.md` | 📝 Partially Mapped | ⚠️ Unproven | Has basic coverage, needs theme, projects expansion |

## Skill Detail & Operations

| Surface | File | Status | Proven | Notes |
|---------|------|--------|--------|-------|
| Skill detail page | `skill-detail.md` | 📝 Partially Mapped | ⚠️ Unproven | Has basic structure, needs locations card, markdown editor, test, compare, repair, assistant expansion |
| Add Skill sheet | `add-skill-sheet.md` | ✅ Fully Mapped | ⚠️ Unproven | All sources: dotagents, skills-sh, copy, pack, GitHub (single/multi), git, local |
| SkillStore browse | `skillstore-browse.md` | ✅ Fully Mapped | ⚠️ Unproven | Browse skills.sh catalog, search, pagination, install from detail |
| Skill operations | `skill-operations.md` | ✅ Fully Mapped | ⚠️ Unproven | Install, update, remove, fork, unfork, pull, park, unpark, enable/disable, invocation policy |

## Modals & Dialogs

| Surface | Documentation | Status | Notes |
|---------|---------------|--------|-------|
| Install progress modal | Covered in `skillstore-browse.md`, `add-skill-sheet.md` | ✅ | Shows during install, toast on completion |
| Remove confirmation | Covered in `skill-operations.md` | ✅ | Native Tauri dialog (not Playwright-dismissible) |
| Pack name prompt | Covered in `packs-management.md` | ✅ | Create pack dialog |
| Discard changes dialog | Covered in `skill-detail.md` (needs expansion) | 📝 | Guards unsaved markdown edits |
| Skill compare dialog | Covered in `skill-detail.md` (needs expansion) | 📝 | Compare multiple deployments of same skill |
| Repair link dialog | Covered in `skill-detail.md` (needs expansion) | 📝 | Fix broken symlinks |
| Materialize root dialog | Covered in `home-dashboard.md` (needs expansion) | 📝 | Convert linked root to per-skill links |
| Trial restore toast | Covered in `add-skill-sheet.md`, `home-dashboard.md` (needs expansion) | 📝 | Toast with restore action after 24h expiry |

## Sub-Features & Interactions

| Feature | Documentation | Status | Notes |
|---------|---------------|--------|-------|
| Sidebar search | `sidebar-navigation.md` | ✅ | Jumps to Skills view with query |
| Theme toggle | `sidebar-navigation.md` | ✅ | Light/dark mode switcher |
| Stat tiles (Broken/Warnings/Updates) | `home-dashboard.md` (needs expansion) | 📝 | Clickable filters |
| Invocation/cost bars | `home-dashboard.md` (needs expansion) | 📝 | Segmented bars with click actions |
| Inbox groups (collapsed/expanded) | `home-dashboard.md` (needs expansion) | 📝 | Collapsible groups with action buttons |
| Skill list filters | `skills-management.md` (needs expansion) | 📝 | Scope, harness, source, issue, invocation, usage |
| Selection mode | `skills-management.md` (needs expansion) | 📝 | Multi-select for create pack |
| Coverage matrix toggle | `skills-management.md` (needs expansion) | 📝 | Alternative view in Skills |
| GitHub skill listing | `add-skill-sheet.md` | ✅ | Auto-discover multiple skills in repo |
| Agent target selector | `add-skill-sheet.md` | ✅ | Enable/disable harnesses for install |
| Scope selector (global/project) | `add-skill-sheet.md` | ✅ | Choose install scope |
| Trial mode checkbox | `add-skill-sheet.md` | ✅ | 24h auto-expire |
| Editor preference picker | `settings.md` (needs expansion) | 📝 | Choose app for "Open in editor" |
| skills.sh API key input | `settings.md` (needs expansion) | 📝 | Developer override for direct API access |
| Skill locations card | `skill-detail.md` (needs expansion) | 📝 | Per-deployment enable/disable toggles |
| Skill markdown card | `skill-detail.md` (needs expansion) | 📝 | View/edit SKILL.md with fork-before-save |
| Skill test form | `skill-detail.md` (needs expansion) | 📝 | Test skill with Cloud Agent |
| Skill assistant drawer | `skill-detail.md` (needs expansion) | 📝 | AI-powered skill editor |
| Activity heatmap | `activity-tracking.md` (needs expansion) | 📝 | Year view of invocations |
| Activity history section | `activity-tracking.md` (needs expansion) | 📝 | Event list with restore actions |

## External Integrations

| Integration | Documentation | Status | Notes |
|-------------|---------------|--------|-------|
| skills.sh API | `skillstore-browse.md`, `skill-operations.md` | ✅ | Search, browse, install |
| npx skills CLI | `skill-operations.md`, `add-skill-sheet.md` | ✅ | Install/update/remove via CLI |
| dotagents CLI | `skill-operations.md`, `add-skill-sheet.md` | ✅ | Alternative install method |
| gh CLI (GitHub) | `packs-management.md` | ✅ | Publish packs |
| Native plugin caches | `plugins-view.md` | ✅ | Claude Code, Codex plugin discovery |
| Claude Code transcripts | `activity-tracking.md` (needs expansion) | 📝 | Invocation history source |

## Coverage Summary

### By Documentation Status
- **Fully Mapped**: 9 surfaces (sidebar, plugins, packs, learn, add-skill, skillstore, skill-ops + 2 smaller)
- **Partially Mapped**: 5 surfaces (home, skills, activity, settings, skill-detail)
- **Not Mapped**: 0 (all surfaces documented)

### By Verification Status
- **Proven with Evidence**: 0 (none yet verified with Playwright)
- **Mapped but Unproven**: 14 (all documented, verification pending)

### Expansion Priority
1. **High Priority** (core workflows):
   - Home dashboard (stat tiles, inbox groups, filters)
   - Skills management (filters, selection mode)
   - Skill detail (locations card, markdown editor, test, compare, repair)
   
2. **Medium Priority** (supporting workflows):
   - Activity tracking (heatmap, restore operations)
   - Settings (theme, projects)
   
3. **Low Priority** (already comprehensive):
   - Sidebar, Plugins, Packs, Learn, Add Skill, SkillStore, Operations

### Testing Recommendations
1. **Launch/Doctor verification** - Verify Tauri dev server starts, app loads
2. **Smoke test each view** - Navigate to every primary view, capture screenshots
3. **Critical path verification**:
   - Add skill (dotagents source, GitHub single skill)
   - Install from SkillStore browse
   - Update skill from Home
   - Park unused skill
   - Create pack from selection
4. **Complex flows** (defer to dedicated verification runs):
   - Fork/unfork/pull upstream
   - Multi-skill GitHub repo install
   - Trial expiry + restore
   - Skill comparison
   - Activity event restore

## Notes

- All feature files follow the same H2 structure: Sub-features, How to get to it, Driving it with Playwright, Gotchas, Branches, Benchmarks & improvement, Observable end state
- Playwright selectors provided are best-effort; actual implementation may use data-testid attributes
- "Proven" status requires evidence (screenshots, test results, logs) captured during verification runs
- Feature flag dependencies noted (e.g. packs behind `skill-packs` flag)
- Platform dependencies noted (macOS-only features like native editor picker)
