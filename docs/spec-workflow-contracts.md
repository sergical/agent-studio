# Existing workflow contracts

Status: Source-audited baseline for the shared-core migration. Runtime acceptance remains unverified.

Date: 2026-09-08.

This document accompanies [Shared core, headless access, performance, and observability](./spec-headless-performance-observability.md). It records the behavior that migration must preserve. It does not add every desktop operation to the first CLI release. Hosted MCP is out of scope.

Each migrated workflow requires a fixture that checks its eligible sources, destinations, ownership changes, file or link effects, failure behavior, and actual restore capability. Unsupported combinations remain unsupported unless a separate product change specifies them. Resource descriptions are qualitative, not measured memory or latency.

## How a skill can enter the application

Adding has four independent choices: source, install method, destination, and lifetime. Only supported combinations should appear in each adapter.

### Entry and source choices

| User workflow                                   | Current behavior and effects                                                                  | Why it is useful                                         | Main resource cost                                 |
| ----------------------------------------------- | --------------------------------------------------------------------------------------------- | -------------------------------------------------------- | -------------------------------------------------- |
| Browse the Skill Store                          | Fetch popular/list results, open details, select an existing skill to install                 | Discover a skill without knowing its repository          | Network response size, result and detail rendering |
| Search the Skill Store                          | Query the catalogue; install using the recorded source                                        | Find a named capability                                  | Network and search result rendering                |
| Paste a repository/source identifier            | Add sheet parses an existing source, then resolves available skills and methods               | Install a source outside catalogue browsing              | Repository metadata lookup or fetch                |
| GitHub owner/repository, subpath, tree/blob URL | Resolve repository, ref/path, and skill selection                                             | Pin or select a specific skill source                    | Network, archive extraction, staging               |
| skills.sh URL or supported Git URL              | Normalize the source and use a compatible install method                                      | Reuse a link someone shared                              | Source resolution and installer work               |
| Local absolute or home-relative path            | Use an existing local source through a supported method                                       | Install local development or offline content             | Directory reads/copies and validation              |
| Select several skills from a repository         | Start a batch operation with per-item outcomes; batch Copy can share one fetched snapshot     | Avoid repeated selection and repeated repository fetches | One repository snapshot plus per-skill writes      |
| Import a pack                                   | Resolve a pack and pinned source, preflight trust, then install its contents                  | Reproduce a curated skill set                            | Snapshot, validation, installer/copy work          |
| Install outside Skill Studio                    | Filesystem discovery finds skills from external installers, manual folders, and plugin caches | Adopt an existing setup                                  | Background discovery and classification            |

The current UI does not expose blank skill creation, pasting a complete SKILL.md body as a new skill, or drag-and-drop folder import. Editing an installed SKILL.md is available. Sources and methods are separate: accepting a URL does not mean every install method works for it. [Add sheet](/Users/sergiydybskiy/src/agent-studio/apps/desktop/src/components/AddSkill/AddSkillSheet.tsx:1197), [conventions](/Users/sergiydybskiy/src/agent-studio/docs/agent-skill-conventions.md)

### Install method and ownership

| Method           | Persistent effects                                                                            | Updates and later edits                                                          |
| ---------------- | --------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| Dotagents        | Runs the owning CLI and records its managed source/lock state; may add Claude per-skill links | Managed update/removal follows dotagents. Forking is a separate ownership change |
| skills.sh CLI    | Runs `npx skills`; uses its managed lock state and supported agent destinations               | Managed update/removal follows the owning CLI                                    |
| Copy             | Writes local files without an upstream install manager owning them                            | Independent edits are possible; do not promise managed update behavior           |
| Pack import      | Uses pack provenance and bundled content through its import operation                         | Imported items' actual ownership determines later update/removal behavior        |
| Plugin discovery | Reads a plugin's installed cache and provenance; no new skill installation occurs             | Plugin-owned content is treated as managed/read-only in the inspected UI         |

The dispatch and source-specific behavior live in [skill_add.rs](/Users/sergiydybskiy/src/agent-studio/apps/desktop/src-tauri/src/skills/skill_add.rs:1123). Destination arguments are built in [skill_install_plan.rs](/Users/sergiydybskiy/src/agent-studio/apps/desktop/src-tauri/src/skills/skill_install_plan.rs:27).

### Destination and lifetime

| Choice                   | Effect                                                                                                                |
| ------------------------ | --------------------------------------------------------------------------------------------------------------------- |
| Global Universal         | One shared skill under the user's shared root; agents that read that root see it. Claude may need a per-skill symlink |
| Project Universal        | Shared deployment belongs to the selected project rather than the global root                                         |
| Per-harness Copy         | Independent folders for selected harnesses. The plan requires at least one harness and permits Copy only              |
| Permanent                | No trial expiry record                                                                                                |
| Trial                    | Supported Universal installation with a trial record. Per-harness trials are rejected                                 |
| Keep a trial             | Removes the trial record while leaving the installation in place                                                      |
| Trial expiry             | Moves the skill to the app's skills-trash area and emits an expiry event                                              |
| Restore an expired trial | Copies it back as unmanaged content and restores applicable Claude visibility                                         |

A shared installation is not a claim that every agent has identical discovery or enable/disable behavior. Claude whole-directory links, native configuration formats, and independent project copies need distinct handling.

## Current user workflow inventory

This table lists user-facing capabilities and their effects. Every resource entry is qualitative. Per-workflow memory still needs measurement.

| Workflow                                           | What it does and changes                                                                               | User value                                          | Main resource driver                                   |
| -------------------------------------------------- | ------------------------------------------------------------------------------------------------------ | --------------------------------------------------- | ------------------------------------------------------ |
| Home dashboard                                     | Displays inventory, issues, usage and update projections                                               | Find the next action                                | Snapshot size and issue derivation                     |
| Skills list, query, filters, selection             | Selects from the current inventory by scope, agent, source or issue                                    | Locate an exact skill/deployment                    | Filtering and rendered row count                       |
| Coverage matrix                                    | Displays visibility across supported agents                                                            | Find uneven agent coverage                          | Skill-by-agent cells and DOM size                      |
| Add a tracked project                              | Remembers a project and registers it with discovery                                                    | Include a project not already discovered            | Added scan/watch roots                                 |
| Stop tracking a project                            | Excludes it from discovery presentation; not a skill uninstall                                         | Reduce clutter and scan scope                       | Updated preferences and next scan                      |
| Manual rescan                                      | Requests a new backend snapshot                                                                        | Reconcile external changes                          | Filesystem discovery, assembly and IPC                 |
| Skill details                                      | Reads content, provenance, deployments and status                                                      | Understand what is installed and who owns it        | Markdown/content bytes and details rendering           |
| Compare deployments                                | Shows a read-only SKILL.md diff between copies                                                         | Explain duplicates or drift                         | Both file contents and diff calculation                |
| Edit installed SKILL.md                            | Validates ownership/path and saves permitted edits; guarded save paths reject stale content            | Maintain local skills                               | File size, parsing and atomic write                    |
| Preview/apply frontmatter repair                   | Fixes the supported unquoted-colon scalar case with exact-target and proposal guards                   | Recover a malformed name/description                | Read, hash, backup, atomic write, rescan               |
| Remove a deployment                                | Removes an exact supported deployment through its ownership-aware path                                 | Uninstall the intended copy                         | Owning CLI or directory/link mutation                  |
| Check for updates                                  | Uses upstream commit lookups and saves update status                                                   | See available upstream changes                      | Network lookups and result cache                       |
| Update managed skill                               | Uses dotagents re-add/re-pin or skills.sh update                                                       | Receive managed upstream changes                    | External installer, network and writes                 |
| Fork a managed skill                               | Preserves content, removes managed ownership, records fork origin/base state                           | Customize while retaining an upstream reference     | Folder snapshot, ledger changes and disk space         |
| Pull upstream into fork                            | Uses upstream data and a three-way merge                                                               | Incorporate upstream changes into customization     | Fetch, merge work, possible conflicts                  |
| Un-fork                                            | Reinstalls the origin and drops fork metadata/snapshot                                                 | Return to managed ownership                         | Installer/network and replacement writes               |
| Park Global Universal skill                        | Moves it to the parked root; project/per-harness copies are independent                                | Stop global availability without losing the content | Folder move and ownership/history work                 |
| Unpark                                             | Validates recorded deployment and restores availability                                                | Resume use                                          | Folder/link changes and rescan                         |
| Native harness visibility                          | Uses supported native mechanisms such as Codex/OpenCode configuration or Claude links                  | Change availability for a particular agent          | Small config/link edits                                |
| Disable/restore an eligible independent deployment | Moves it to/from a sibling disabled area; shared-root and plugin targets are refused                   | Temporarily hide an independent copy                | Folder move and snapshot change                        |
| Change invocation policy                           | Changes supported frontmatter and Codex metadata                                                       | Control automatic versus explicit invocation        | Parse/rewrite small config files                       |
| Materialize a linked root                          | Replaces a whole-directory symlink with a real directory of per-skill links                            | Permit finer control over linked skills             | Enumerate entries, record intent, create links         |
| Make independent copy                              | Replaces an eligible shared-backed link with a local directory copy                                    | Customize only one deployment                       | Full skill-tree copy and disk usage                    |
| Relink a broken deployment                         | Repairs the link through the dedicated repair action                                                   | Recover visibility without reinstalling content     | Link validation and replacement                        |
| Remove a broken link                               | Removes the dangling link through the repair action                                                    | Clear an unusable deployment                        | Link mutation and rescan                               |
| Reveal folder/open editor                          | Opens the selected filesystem target in a native tool                                                  | Inspect supporting files or edit externally         | OS application launch; external editor memory          |
| Plugin skills view                                 | Displays discovered plugin skills and provenance                                                       | Explain skills installed by a plugin                | Plugin-cache discovery and rendering                   |
| Mutation history                                   | Lists recorded operations and outcomes                                                                 | Explain how the setup changed                       | SQLite query and list rendering                        |
| Restore a supported event                          | Applies a stored inverse after drift checks; force restore first preserves current drift               | Recover from a supported change                     | Backups, filesystem operations, SQLite and rescan      |
| Create pack from selected skills                   | Copies selected deployments, writes provenance/README, initializes and commits a repository            | Curate a shareable set                              | Full copies and git operations                         |
| Open/view pack                                     | Displays local pack information or opens its location                                                  | Inspect a saved collection                          | Local metadata and UI                                  |
| Update pack                                        | Runs the pack update operation                                                                         | Refresh a saved collection                          | Source-dependent copy/fetch/git work                   |
| Publish/push pack                                  | Publishes through the GitHub path with confirmation                                                    | Share a collection                                  | GitHub authentication, network, git                    |
| Delete pack                                        | Removes the local pack and registry entry                                                              | Remove a local collection                           | Directory removal; not remote repository deletion      |
| Ask assistant                                      | Runs selected local harness with workspace write access inside scratch                                 | Explore a skill with an agent                       | Child agent process, model latency/cost and transcript |
| Audit skill                                        | Runs a read-only review; plugin-managed targets are restricted                                         | Obtain proposed improvements                        | Model call and transcript                              |
| Accept/reject audit hunks                          | Selectively applies proposed edits using a content compare-and-swap guard                              | Review changes before writing                       | Diff, file validation and write                        |
| Discard audit proposal                             | Clears proposed changes                                                                                | Keep current installed content                      | UI state release                                       |
| Test skill                                         | Prepares target, runs task, judges result, collects diff and records history; can include extra skills | Check behavior beyond syntax                        | Agent processes, model usage, scratch/worktree I/O     |
| Choose test target                                 | Scratch, worktree or in-place; non-scratch targets expose a whole-tree diff                            | Match isolation to the task                         | Copy or worktree setup; selected target writes         |
| Apply/discard test changes                         | Uses the run-target operation for applicable targets                                                   | Control which test edits survive                    | Diff and target filesystem operations                  |
| Cancel agent run                                   | Stops its process with a termination/grace/kill path                                                   | Stop unwanted work                                  | Process cleanup and final event delivery               |
| Open/delete scratch folder                         | Opens or removes an app-owned scratch directory                                                        | Inspect or clean test artifacts                     | OS launch or directory removal                         |
| Run history/transcripts                            | Reads saved summaries and selected transcripts                                                         | Revisit results                                     | Disk read, JSON parsing and transcript DOM             |
| Invocation activity/heatmap                        | Indexes available invocation records and shows use over time, including plugin skills                  | Find usage patterns and unused skills               | Transcript indexing and aggregate cache                |
| Learn                                              | Explains broken skills, invocation, cost and unused skills                                             | Help users interpret dashboard findings             | Static content rendering                               |
| Settings and theme                                 | Changes display preferences and catalogue access configuration                                         | Adapt the app and choose proxy/direct API access    | Small preference writes; catalogue network mode        |

Important boundaries:

- The current UI has no plugin install/disable manager and no visible pack member add/remove editor.
- Invocation observations are not complete proof of usage across all agents. The current refresh reads Claude project transcripts; missing observations must not be labeled as proof that a skill was never used.
- Model invocation costs and SKILL.md token counts are not application CPU/RAM metrics.
- History is not universal undo. Some recorded events have no safe inverse. The UI offers restore based on the event's actual capability.
- Pi, Cursor and Grok do not have a native per-harness visibility switch in the inspected implementation.
- Agent runners currently target Claude Code, Codex, OpenCode and pi. Discovery support for another agent does not establish support for launching it.

Entry-point references: [App routes](/Users/sergiydybskiy/src/agent-studio/apps/desktop/src/App.tsx:108), [coverage](/Users/sergiydybskiy/src/agent-studio/apps/desktop/src/components/SkillList/SkillsView.tsx:145), [location actions](/Users/sergiydybskiy/src/agent-studio/apps/desktop/src/components/SkillDetail/skill-location-actions.ts:144), [assistant](/Users/sergiydybskiy/src/agent-studio/apps/desktop/src/components/SkillDetail/SkillAssistantPanel.tsx:652), [proposal application](/Users/sergiydybskiy/src/agent-studio/apps/desktop/src/components/SkillDetail/SkillProposedEdits.tsx:62), [packs](/Users/sergiydybskiy/src/agent-studio/apps/desktop/src/components/Packs/PacksView.tsx:27), [history restore policy](/Users/sergiydybskiy/src/agent-studio/apps/desktop/src/components/Activity/SkillHistorySection.tsx:100).
