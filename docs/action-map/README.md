# Action map

This folder describes every action the desktop app can take, area by area. Each file answers four questions about one area: what the code does today, what the Claude stack changed, what the Codex stack changed, and where we want it to be.

Source: a read of `apps/desktop/src-tauri/src/skills/` and `apps/desktop/src` on 2026-09-16, on the branch `feat/activity-narrow-layout` at the tip of the Claude stack (#134). The full per-command table is published at https://claude.ai/artifact/K5s4TezGohKPfmiWVFP7ZP.

## Files

| File                                                 | Area                                                                               | Commands                                                                                                              |
| ---------------------------------------------------- | ---------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| [install.md](install.md)                             | Add a skill from GitHub, skills.sh, a folder, or a pack member                     | add_skill, start_add_skill_operation, confirm_add_skill_trust, list_github_skills, get_add_method_defaults            |
| [remove-and-update.md](remove-and-update.md)         | Remove or update an installed skill                                                | remove_skill, update_skill, restore_trashed_skill                                                                     |
| [park-fork-trial.md](park-fork-trial.md)             | Park, unpark, fork, un-fork, pull upstream, 24-hour trials                         | park_skill, unpark_skill, fork_skill, unfork_skill, pull_fork_upstream, keep_skill_trial                              |
| [enable-and-links.md](enable-and-links.md)           | Turn a deployment on or off, repair links, make independent copies, set visibility | set_deployment_enabled, set_harness_enabled, materialize_harness_root, repair_skill_link, make_skill_independent_copy |
| [skill-md-editing.md](skill-md-editing.md)           | Read, write, and repair SKILL.md and its frontmatter                               | read_installed_skill_md, write_installed_skill_md_if_unchanged, apply_skill_frontmatter_repair, set_skill_invocation  |
| [events-and-history.md](events-and-history.md)       | The event store, restore, backups, and the startup reconcile                       | list_skill_events, restore_skill_event                                                                                |
| [packs.md](packs.md)                                 | Skill packs, behind the `skill-packs` flag                                         | list_skill_packs, create_skill_pack, publish_skill_pack, import_skill_pack                                            |
| [assistant-runs.md](assistant-runs.md)               | Ask, Audit, and Test runs, scratch folders, run targets, run history               | start_skill_agent_run, prepare_skill_run_target, record_skill_run, list_skill_runs                                    |
| [settings-and-projects.md](settings-and-projects.md) | Tracked project folders, discovery, editor choice, the registry file               | register_skill_projects, list_project_folders, set_discovery_source, set_preferred_editor                             |
| [reads-and-snapshot.md](reads-and-snapshot.md)       | The skill snapshot, rescans, the skills.sh store reads, Activity reads             | get_skill_snapshot, request_skill_rescan, search_skills, get_skill_details                                            |
| [shared-state.md](shared-state.md)                   | Every file on disk, every lock, every background loop                              | none; this is the state the commands share                                                                            |

## Files that describe the whole

These files cut across the areas. Read them before the area files if you are new to the code.

| File                                                         | What it answers                                                                                                                                                                                                                                  |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| [system-overview.md](system-overview.md)                     | What the primitives are, how a write flows from a click to disk today and in the target design, the budgets and the failure modes                                                                                                                |
| [harness-integration.md](harness-integration.md)             | What Claude Code, Codex, OpenCode, pi, and the shared root each expect on disk, and how Ask, Audit, and Test runs drive each harness                                                                                                             |
| [lifecycle-states.md](lifecycle-states.md)                   | Every state one skill can be in, which command moves it, what a crash leaves behind, and the disk invariants a doctor pass must check                                                                                                            |
| [user-stories.md](user-stories.md)                           | The twelve user jobs U1 to U12 (detect harnesses, inventory, activity, outdated, install, update, park, fix, conflicts, turn off, undo, remove), what happens today, and when each is done                                                       |
| [primitives-and-call-stack.md](primitives-and-call-stack.md) | The eight primitives, how each user job composes them, the call stack from a click to disk, and where each harness enters                                                                                                                        |
| [plan.md](plan.md)                                           | The architecture as units of work, the rule for cutting work (primitives, harness adapters, vertical slices, baselines), the units in six groups, baselines, test strategy, lint set, simplicity rules, and the definition of done per unit kind |
| [frontend-keep.md](frontend-keep.md)                         | The Claude-stack frontend that stays (#73 to #134) by group with PRs and backend dependencies, the wrappers the rebuild rewires in `skill-api.ts`, and where each group is documented; protected by unit 4.4 (#175)                              |
| [performance.md](performance.md)                             | The Tauri threading model, which commands block today, the scan budget, the baseline plan, and the targets                                                                                                                                       |
| [definition-of-done.md](definition-of-done.md)               | The six checks, the budgets, a checklist per area and per slice, and how we keep this map true                                                                                                                                                   |
| [post-mortem.md](post-mortem.md)                             | Why the two finished work streams did not reach this target, read from the session prompts on disk                                                                                                                                               |
| [release-readiness.md](release-readiness.md)                 | What stands between this checkout and a signed, self-updating public build: updater, signing, CI, hosting on useskillstudio.com, marketing claims, CLI and MCP parity                                                                            |

## Per-harness files

One file per harness under [harnesses/](harnesses/). Each answers: how the app knows the harness is present, where skills live, how it loads a skill, how the app turns a skill off, how a use shows up in activity, and what the core must handle.

| File                                                             | Covers                                                                                                                                                   |
| ---------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [harnesses/harness-detection.md](harnesses/harness-detection.md) | The four detection signals, the five states, PATH resolution, and the per-harness probe tables                                                           |
| [harnesses/claude-code.md](harnesses/claude-code.md)             | Claude Code                                                                                                                                              |
| [harnesses/codex.md](harnesses/codex.md)                         | Codex                                                                                                                                                    |
| [harnesses/opencode.md](harnesses/opencode.md)                   | OpenCode, v2 docs only                                                                                                                                   |
| [harnesses/pi.md](harnesses/pi.md)                               | pi                                                                                                                                                       |
| [harnesses/shared-root.md](harnesses/shared-root.md)             | `~/.agents/skills`, the three ledgers, and what is missing                                                                                               |
| [harnesses/plugins.md](harnesses/plugins.md)                     | The Agent Skills spec at agentskills.io, the Agent Plugins spec at agent-plugins.org, and how each harness installs, caches, loads, and switches plugins |
| [harnesses/sources.md](harnesses/sources.md)                     | The documentation URLs to read for each harness when a question comes up; OpenCode references are https://opencode.ai/v2/docs, never v1                  |

## Scope for the first release

Assistant runs (Ask, Audit, Test) and packs are deferred. Their area files stay in this folder, and the core API must stay usable by them, but no unit in plan.md builds them. The scope decision is recorded in user-stories.md and plan.md.

## How to read one file

Each file has the same five sections.

1. **Current state.** What runs, in what order it writes, what lock it takes, whether it records a journal event, how it fails, what the code does on failure, what the user sees, and which tests exist. Lines cite `file:line` from the read on 2026-09-16.
2. **Changes in the Claude stack (#73 to #134).** What the open pull requests from #73 to #134 changed in this area. These are our own changes.
3. **Changes in the Codex stack (#79 to #141).** What the open pull requests from #79 to #141 changed in this area. This section reports only. It does not set the design.
4. **Desired state.** Where we want the area to be. Every file applies the same rules, listed below.
5. **Gaps.** One line per difference between current and desired, with the command name.

## The rules the desired state applies

- Every write records a journal event with a backup and an inverse before it touches disk.
- One write path lives in the core crate. The desktop command is a thin adapter over it.
- Writes take a per-scope lease, not one global try-acquire lock.
- The registry file `~/.agents/skill-studio.json` is read, changed, and written under that same lease.
- A chained write is one transaction, or each step has a compensating step.
- Success feedback appears only after the last step. Errors are never swallowed. Partial results are named to the user.
- Every install path shows the repository trust prompt.
- Backups have a retention limit.
- Every command has a direct test and a crash-window test.
- No IPC command exists without a caller.

## Headline numbers from the 2026-09-16 read

| Measure                                  | Count |
| ---------------------------------------- | ----- |
| Tauri commands                           | 62    |
| Commands that write                      | 46    |
| Writes that record a journal event       | 9     |
| Commands with no direct test             | 23    |
| Commands with a named partial-state risk | 35    |
| Commands with no frontend caller         | 2     |
| UI controls mapped                       | 110   |

## Keeping this map true

This map was read by hand. Nothing checks it against the code. When a command is added, removed, or changes its write order, edit the file for its area in the same pull request. The build does not fail when the map is stale, so the review has to catch it.
