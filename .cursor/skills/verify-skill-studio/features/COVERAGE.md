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
| Home dashboard | `home-dashboard.md` | ✅ Fully Mapped | ⚠️ Unproven | Stat tiles, invocation/cost bars, inbox groups, filters, trial restore toast, linked-root dialog |
| Skills list | `skills-management.md` | ✅ Fully Mapped | ⚠️ Unproven | All filter kinds, search, selection mode, coverage matrix toggle, create pack |
| Plugins view | `plugins-view.md` | ✅ Fully Mapped | ⚠️ Unproven | Read-only plugin skills from Claude Code/Codex caches |
| Activity tracking | `activity-tracking.md` | ✅ Fully Mapped | ⚠️ Unproven | Heatmap (52-week grid), By Skill/Project tables, history section, restore operations |
| Packs management | `packs-management.md` | ✅ Fully Mapped | ⚠️ Unproven | Create, update, publish, import, delete |
| Learn sections | `learn-sections.md` | ✅ Fully Mapped | ⚠️ Unproven | 4 explainer sections (broken, invoke, cost, unused) |
| Settings | `settings.md` | ✅ Fully Mapped | ⚠️ Unproven | Editor preference picker, skills.sh API key, theme (sidebar), projects (filter bar) |

## Skill Detail & Operations

| Surface | File | Status | Proven | Notes |
|---------|------|--------|--------|-------|
| Skill detail page | `skill-detail.md` | ✅ Fully Mapped | ⚠️ Unproven | Header, locations card (per-deployment toggles), markdown editor (fork-before-save), compare dialog, repair card, assistant drawer |
| Add Skill sheet | `add-skill-sheet.md` | ✅ Fully Mapped | ⚠️ Unproven | All sources: dotagents, skills-sh, copy, pack, GitHub (single/multi), git, local |
| SkillStore browse | `skillstore-browse.md` | ✅ Fully Mapped | ⚠️ Unproven | Browse skills.sh catalog, search, pagination, install from detail |
| Skill operations | `skill-operations.md` | ✅ Fully Mapped | ⚠️ Unproven | Install, update, remove, fork, unfork, pull, park, unpark, enable/disable, invocation policy |

## Modals & Dialogs

| Surface | Documentation | Status | Notes |
|---------|---------------|--------|-------|
| Install progress modal | Covered in `skillstore-browse.md`, `add-skill-sheet.md` | ✅ | Shows during install, toast on completion |
| Remove confirmation | Covered in `skill-operations.md` | ✅ | Native Tauri dialog (not Playwright-dismissible) |
| Pack name prompt | Covered in `packs-management.md` | ✅ | Create pack dialog |
| Discard changes dialog | Covered in `skill-detail.md` | ✅ | Guards unsaved markdown edits |
| Skill compare dialog | Covered in `skill-detail.md` | ✅ | Compare multiple deployments of same skill |
| Repair link dialog | Covered in `skill-detail.md` | ✅ | Fix broken symlinks |
| Materialize root dialog | Covered in `home-dashboard.md` | ✅ | Convert linked root to per-skill links |
| Trial restore toast | Covered in `add-skill-sheet.md`, `home-dashboard.md` | ✅ | Toast with restore action after 24h expiry |

## Sub-Features & Interactions

| Feature | Documentation | Status | Notes |
|---------|---------------|--------|-------|
| Sidebar search | `sidebar-navigation.md` | ✅ | Jumps to Skills view with query |
| Theme toggle | `sidebar-navigation.md` | ✅ | Light/dark mode switcher |
| Stat tiles (Broken/Warnings/Updates) | `home-dashboard.md` | ✅ | Clickable filters with InfoPopover explainers |
| Invocation/cost bars | `home-dashboard.md` | ✅ | Segmented bars with click actions |
| Inbox groups (collapsed/expanded) | `home-dashboard.md` | ✅ | Collapsible groups with action buttons per row |
| Skill list filters | `skills-management.md` | ✅ | Scope, harness, source, issue, invocation, usage |
| Selection mode | `skills-management.md` | ✅ | Multi-select for create pack (shift-click range select) |
| Coverage matrix toggle | `skills-management.md` | ✅ | Alternative view in Skills (grid layout) |
| GitHub skill listing | `add-skill-sheet.md` | ✅ | Auto-discover multiple skills in repo |
| Agent target selector | `add-skill-sheet.md` | ✅ | Enable/disable harnesses for install |
| Scope selector (global/project) | `add-skill-sheet.md` | ✅ | Choose install scope |
| Trial mode checkbox | `add-skill-sheet.md` | ✅ | 24h auto-expire |
| Editor preference picker | `settings.md` | ✅ | Choose app for "Open in editor" (radio group) |
| skills.sh API key input | `settings.md` | ✅ | Developer override for direct API access |
| Skill locations card | `skill-detail.md` | ✅ | Per-deployment enable/disable toggles + Open/Reveal actions |
| Skill markdown card | `skill-detail.md` | ✅ | View/edit SKILL.md with Monaco editor, fork-before-save |
| Skill test form | `skill-detail.md` | ✅ | Test skill with Cloud Agent (noted as planned feature) |
| Skill assistant drawer | `skill-detail.md` | ✅ | AI-powered skill editor with apply flow |
| Activity heatmap | `activity-tracking.md` | ✅ | 52-week x 7-day GitHub-style grid |
| Activity history section | `activity-tracking.md` | ✅ | Event list with restore actions, drift guard |

## External Integrations

| Integration | Documentation | Status | Notes |
|-------------|---------------|--------|-------|
| skills.sh API | `skillstore-browse.md`, `skill-operations.md` | ✅ | Search, browse, install |
| npx skills CLI | `skill-operations.md`, `add-skill-sheet.md` | ✅ | Install/update/remove via CLI |
| dotagents CLI | `skill-operations.md`, `add-skill-sheet.md` | ✅ | Alternative install method |
| gh CLI (GitHub) | `packs-management.md` | ✅ | Publish packs |
| Native plugin caches | `plugins-view.md` | ✅ | Claude Code, Codex plugin discovery |
| Claude Code transcripts | `activity-tracking.md` | ✅ | Invocation history source (Codex/OpenCode/pi not tracked yet) |

## Coverage Summary

### By Documentation Status
- **Fully Mapped**: 14 surfaces (all primary views, skill detail, operations, sub-features)
- **Partially Mapped**: 0 (all surfaces expanded to full depth)
- **Not Mapped**: 0 (all surfaces documented)

### By Verification Status
- **Proven with Evidence**: 0 (none yet verified with Playwright)
- **Mapped but Unproven**: 14 (all documented, verification pending)

### Expansion Priority
**All primary surfaces now at full depth.** No expansion priorities remain for documentation.

Next priority: **Verification** - driving documented flows with Playwright and capturing evidence.

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
