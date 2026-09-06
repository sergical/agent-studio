# Skill Studio Feature Map

Comprehensive documentation of every user-facing surface, workflow, and interaction in Skill Studio - a Tauri 2.x desktop app for managing AI coding assistant skills across Claude Code, Codex, OpenCode, and pi.

## Purpose

This feature map provides:
1. **Complete coverage** - Every view, modal, dialog, and interaction documented
2. **Playwright driving instructions** - Selectors and code examples for automated verification
3. **Branch documentation** - Happy paths, empty states, failures, cancellations, and edge cases
4. **Performance hooks** - Observable metrics, current instrumentation, improvement levers
5. **End-state verification** - Invariants and observable outcomes to prove correctness

## Feature Files

### Primary Views & Navigation

**[Sidebar Navigation](./sidebar-navigation.md)**
- Search box with auto-navigation to Skills view
- Add skill button
- View links (Home, Skills, Plugins, Activity, Packs, Parked)
- Footer controls: rescan, Learn, Settings, theme toggle

**[Home Dashboard](./home-dashboard.md)**
- Stat tiles: Broken, Warnings, Updates (clickable filters)
- Lane card: Invocation counts, prompt cost (segmented bars)
- Inbox groups: Broken, Warnings, Updates, Unused (30d), Recently Used
- Trial restore toasts
- Linked-root conversion dialog
- Filter interactions

**[Skills Management](./skills-management.md)**
- Filterable skill table (scope, harness, source, issue, invocation, usage)
- Search integration (sidebar query applies here)
- Selection mode (multi-select for create pack)
- Coverage matrix toggle (alternative table view)
- Quick actions per row

**[Plugins View](./plugins-view.md)**
- Read-only skills from native plugin caches
- Claude Code (`~/.claude/plugins/cache`)
- Codex (`~/.codex/plugins/cache`)
- No edit/fork/remove actions (plugin-managed)

**[Activity Tracking](./activity-tracking.md)**
- Invocation history (newest first, 200 event limit)
- Year heatmap by skill and project
- Usage breakdowns (30d window selector)
- Event restore operations (undo with drift guard)
- Filtering by skill

**[Packs Management](./packs-management.md)**
- Create pack (bundle selected skills)
- Update pack (rebuild tree from skill list)
- Publish pack (push to GitHub via `gh` CLI)
- Import pack (install bundled + referenced skills)
- Delete pack (local only, GitHub untouched)

**[Learn Sections](./learn-sections.md)**
- Deep-linkable explainer sections
- Broken and warnings (dead links, spec violations, copies differ)
- Who can invoke (per-harness invocation controls)
- Prompt cost (token accounting, user-only exemption)
- Not used in 30 days (transcript-based usage tracking)

**[Settings](./settings.md)**
- Editor preference (which app "Open in editor" uses)
- skills.sh API key (developer override for direct access)
- Theme preference (stored, future integration)
- Project tracking (add/remove project directories)

### Skill Detail & Operations

**[Skill Detail Page](./skill-detail.md)**
- Header: name, primary action, assistant trigger, overflow menu, metadata
- Locations card: per-deployment enable/disable, harness toggles, repair broken links
- Markdown card: view/edit SKILL.md, fork-before-save for managed skills
- Test form: invoke skill with Cloud Agent, capture results
- Compare dialog: side-by-side diff of multiple deployments
- Repair card: fix broken symlinks (remove or relink)
- Assistant drawer: AI-powered skill editor with audit proposals

**[Add Skill Sheet](./add-skill-sheet.md)**
- Source field with live parsing (GitHub owner/repo, URLs, git URLs, local paths)
- GitHub skill listing (auto-discover multiple SKILL.md files in repos)
- Multiple skill selection (checkboxes when repo contains 2+ skills)
- Method selector: dotagents / skills.sh / Copy / Pack
- Agent target selector (enable/disable harnesses)
- Scope selector (global vs project, with directory picker)
- Trial mode (24h auto-expire with restore action)
- Validation and progress feedback

**[SkillStore Browse](./skillstore-browse.md)**
- Search skills.sh catalog (36,000+ skills)
- Browse popular skills (install count sorted)
- Pagination (50 per page, "Load more" button)
- Skill cards with installed badges
- Detail panel with full SKILL.md/AGENTS.md body
- Install from detail (agent selector, scope picker)
- Tabs: Browse (catalog) vs Installed (local skills)

**[Skill Operations](./skill-operations.md)**
- **Install** - via Add Skill or SkillStore (dotagents/skills-sh/copy methods)
- **Update** - pull latest from upstream (dotagents sync, skills.sh re-install, fork pull)
- **Remove** - uninstall from global or project scope (confirmation dialog)
- **Fork** - detach from ledger to allow local edits (dotagents/skills-sh only)
- **Unfork** - discard fork, reinstall from origin
- **Pull upstream** - three-way merge for forked skills
- **Park** - move to `skills-parked/` (global disable)
- **Unpark** - restore from parked (collision reconciliation)
- **Enable/disable per-harness** - toggle via harness's own mechanism
- **Enable/disable deployment** - universal fallback via `.skill-studio-disabled/`
- **Set invocation policy** - rewrite SKILL.md frontmatter + Codex yaml

## Coverage Overview

### Documentation Status
- ✅ **Fully Mapped**: 9 primary surfaces (sidebar, plugins, packs, learn, add-skill, skillstore, skill-ops)
- 📝 **Partially Mapped**: 5 surfaces (home, skills, activity, settings, skill-detail) - basic coverage exists, sub-features need expansion
- ❌ **Not Mapped**: 0 - every surface has documentation

See **[COVERAGE.md](./COVERAGE.md)** for the complete matrix of all surfaces, modals, sub-features, and their verification status.

### Verification Status
- **Proven**: 0 (none yet verified with Playwright + evidence)
- **Mapped but Unproven**: 14 (all documented, Playwright examples provided, verification pending)

## How to Use This Map

### For Verification Engineers
1. **Pick a feature file** - Each markdown file is one user-facing surface or workflow
2. **Read "How to get to it"** - User's perspective on accessing the feature
3. **Follow "Driving it with Playwright"** - Code examples and selectors
4. **Check "Branches"** - Test all documented paths (happy, empty, failure, cancel)
5. **Capture evidence** - Screenshots, logs, test results per "Observable end state"
6. **Validate benchmarks** - Measure latency, success rates per "Benchmarks & improvement"

### For Developers
1. **Reference when changing behavior** - Feature files document current state
2. **Update after refactors** - Keep selectors and flows in sync with code
3. **Add new features** - Follow existing H2 structure (sub-features, driving, gotchas, branches, benchmarks, end state)
4. **Check gotchas** - Understand edge cases and constraints before modifying

### For Product/QA
1. **Audit completeness** - COVERAGE.md shows which surfaces are documented vs gaps
2. **Review critical paths** - Home → Add Skill → Install is the most common flow
3. **Identify risk areas** - Complex flows (fork/unfork, trial restore, multi-skill install) need extra scrutiny
4. **Track verification progress** - "Proven" status tracks which features have been driven end-to-end

## Feature File Structure

Every feature file follows this template:

1. **Sub-features** - Breakdown of components and capabilities
2. **How to get to it (user POV)** - Navigation path from app launch
3. **Driving it with Playwright** - Code examples, selectors, click sequences
4. **Gotchas** - Edge cases, constraints, platform dependencies, feature flags
5. **Branches** - Happy path, empty/first-run, duplicate/conflict, failure, cancellation, loading states
6. **Benchmarks & improvement** - Observable metrics, current instrumentation, suggested measurements, improvement levers
7. **Observable end state** - Invariants and outcomes to verify correctness

## Critical Paths (Priority for Verification)

1. **Install flow** - Add Skill sheet → type source → submit → toast success → skill shows in sidebar
2. **Browse & install** - SkillStore Browse tab → search → click card → install from detail
3. **Update skill** - Home Updates group → "Pull latest" → toast success
4. **Park unused** - Home Unused group → "Park" → skill moves to parked
5. **Create pack** - Skills view → Select → check skills → "Create pack" → enter name → pack created
6. **Fork & edit** - Skill detail → "Fork" → edit markdown → save → changes persist

## Testing Strategy

### Phase 1: Smoke Test (Launch + Doctor)
- Verify Tauri dev server starts (`npm run tauri dev`)
- Doctor checks: port 1420 responds, tmux session alive, node process owns port
- Navigate to each primary view, capture one screenshot per view

### Phase 2: Core Workflows
- Install skill (dotagents, GitHub single, skills.sh)
- Update skill (Home "Pull latest")
- Remove skill (detail overflow menu)
- Park/unpark skill
- Search skills (sidebar + Skills view filter)

### Phase 3: Advanced Workflows
- Multi-skill GitHub repo install
- Fork, edit, unfork flow
- Create pack, publish pack
- Trial mode + restore after expiry
- Skill comparison
- Activity event restore

### Phase 4: Edge Cases & Failures
- Install duplicate skill (warning toast)
- Update without internet (error toast)
- Fork plugin skill (button hidden)
- Park project-scoped skill (error: not in shared folder)
- Enable harness with linked root (Convert dialog required)

## Expansion Priorities

Per COVERAGE.md, these surfaces need detailed sub-feature expansion:

1. **Home dashboard** - Stat tiles, inbox groups, filters, trial restore, linked-root dialog
2. **Skills management** - Filters (all 7 kinds), selection mode, coverage matrix view
3. **Skill detail** - Locations card, markdown editor, test form, compare dialog, repair flow, assistant drawer
4. **Activity tracking** - Heatmap details, restore operation branches
5. **Settings** - Theme preference, project tracking UI

## Related Documentation

- `../../SKILL.md` - Verification skill entry point (Launch, Doctor, Drive, Evidence, Cleanup)
- `../../../docs/agent-skill-conventions.md` - agentskills.io spec, per-agent discovery paths, invocation control
- `../../../apps/desktop/src-tauri/src/skills/` - Rust backend implementation
- `../../../apps/desktop/src/components/` - React frontend components

## Maintenance

- **On code changes**: Update feature files to match new behavior (selectors, flows, branches)
- **On new features**: Create new feature file or expand existing one, add to COVERAGE.md
- **On verification runs**: Mark features as "Proven" in COVERAGE.md, link to evidence
- **On refactors**: Re-verify affected features, update selectors if UI structure changed
