# User stories

One person doing one thing end to end. Each story says what happens today, what the target is, which primitives and harness facts it uses, and the check that marks it done. The stories are the vertical slices in plan.md, group 3, in the same order.

The user has several hundred skills (378 at the last count). Most (63%) are global. The skills reach four harnesses, Claude Code, Codex, OpenCode, and pi, through the shared `~/.agents/skills` root and each harness's own folder. The user wants the app to feel like a fast native tool: nothing waits on the UI thread, every write is safe to interrupt, and the app uses the tools already on the machine (the `npx skills` CLI, `git`, `gh`, the `claude plugin` CLI, the user's editor) instead of rebuilding them.

Deferred, kept usable through the API: Ask, Audit, and Test runs (assistant-runs.md) and packs (packs.md). Nothing here depends on them.

## Index

| #   | Story                                                                         | Slice       | Status                                                              |
| --- | ----------------------------------------------------------------------------- | ----------- | ------------------------------------------------------------------- |
| U1  | First run: the app finds my harnesses and asks what to track                  | 3.2         | not started; no first run exists                                    |
| U2  | Inventory: I see every skill, where it lives, and which harness reads it      | 3.3         | works; blank list on a slow scan fixed once, not guarded            |
| U3  | Activity: I see which skills each harness used, and when                      | 3.3         | works for four harnesses                                            |
| U4  | Outdated: I see which skills have a newer version, by how they were installed | 3.4         | partial; skills.sh and dotagents only, by commit, on a 6-hour timer |
| U5  | Install: I add a skill by my preferred method into my preferred harnesses     | 3.5         | works through `npx skills`; preference not saved                    |
| U6  | Update: I upgrade one skill or all of them                                    | 3.6         | works one at a time; blocks a worker thread                         |
| U7  | Park and unpark: I turn a skill off everywhere and back on                    | 3.1, tracer | works; no journal, no lease                                         |
| U8  | Fix: the app repairs a broken skill or tells me what it cannot                | 3.7         | partial; frontmatter repair and link repair exist                   |
| U9  | Conflicts: two copies differ, the app hands me my editor                      | 3.7         | partial; hash mismatch shown, no side-by-side, editor exists        |
| U10 | Turn off for one harness with that harness's own switch                       | 3.8         | partial; Claude link, Codex row, OpenCode deny; pi none             |
| U11 | Undo: any write can be reversed                                               | 3.8         | partial; 8 of 46 writes journaled                                   |
| U12 | Remove                                                                        | 3.9         | works through `npx skills`; blocks a worker thread                  |

## The primitives every story uses

Root, Snapshot, Stage, Swap, Link, WriteFile, Journal, Lease, TreeHash, plus Source and Fetch for anything that comes from the network. See primitives-and-call-stack.md for how each story composes them and system-overview.md for what each one guarantees.

## U1. First run

**Person.** Someone who just installed the app on a Mac with Claude Code and Codex, plus a few projects under `~/src`.

**Today.** There is no first run. The list opens against a fixed set of six harnesses whether or not they exist (agents.rs:338). The only choice is the per-harness folder-search switch under Settings, Project folders (ProjectFoldersCard.tsx:349).

**Target.** On first launch the app runs `detect_harnesses` (harness-detection.md) and shows one row per harness: Not found, Data only, Installed with version, Configured, Used with the last session date. The user keeps or removes rows, and chooses whether to search harness history for project folders and which folders or patterns to track. Nothing is guessed from a folder name; Unknown prints as Unknown. The choice is saved in the registry, once, under a new `harnesses` key. A later launch skips the screen and re-runs detection in the background.

**Uses.** `detect_harnesses`; registry WriteFile under the Lease; tracked_projects patterns.

**Done when.** A clean macOS account with Claude Code and Codex installed reaches the list with those two rows detected and pi and OpenCode as Not found; the shell probe runs once; no IPC call on the main thread over one frame.

## U2. Inventory

**Person.** The user opens the app after a week away.

**Today.** The background thread scans every root, builds one snapshot, and the frontend swaps it in when the revision is newer (skill_refresh.rs:208; useSkillSnapshot.ts:96). One earlier defect: an empty list was published when the first scan ran slow; fixed by reordering, not guarded by a test (post-mortem.md).

**Target.** The last good snapshot is shown at once from disk, the scan runs in the background, and the list updates in place. A scan over budget shows the last list and a "still scanning" note, never a blank. The snapshot carries per-phase timings.

**Uses.** Snapshot; the read budget in scope.rs; `timings_ms`.

**Done when.** A test drives the scan over budget on the bench estate and asserts the published list is the previous one; scan on the bench estate is under 100 ms.

## U3. Activity

**Person.** The user wants to know which skills Codex used this week and which have never been used.

**Today.** Works for four harnesses with byte-offset resume, byte caps, and a read-only SQLite open for OpenCode (each harness file, section Activity). The "Used" column is empty for 95% of skills, which is true, not a bug.

**Target.** Same reads, moved into the core behind one `usage_reader` per adapter, with the fixture homes as the test bed. A harness the user removed in U1 is not read.

**Done when.** Each adapter's usage test runs against its fixture home; the four parsers live in the core with no `std::fs`.

## U4. Outdated, by install method

**Person.** The user sees "update available" next to a skill and trusts it.

**Today.** Only dotagents and skills.sh skills are candidates; both compare an installed commit against the newest `gh api` commit for the path, on a 6-hour timer plus "Check now", with results in `update-check.json` (skill_update_check.rs:421 to 504). Manual and plugin skills are never checked (:419). One `gh` call per candidate, four at a time.

**Target.** Currency per install method, from harnesses/shared-root.md:

| Method    | Installed side                       | Newest side                         | How                                                                         |
| --------- | ------------------------------------ | ----------------------------------- | --------------------------------------------------------------------------- |
| skills.sh | `skillFolderHash` in the lock file   | tree SHA of the source path at HEAD | TreeHash equals the CLI's hash; one `gh api` per source repo, not per skill |
| dotagents | pinned commit in `agents.lock`       | newest commit for the path          | as today                                                                    |
| plugin    | version in the plugin cache manifest | the marketplace manifest            | Claude Code only; Codex has no CLI                                          |
| manual    | none                                 | none                                | "Not tracked", never "up to date"                                           |

**Done when.** A skill whose lock hash differs from the tree SHA shows "update available" and one whose hash matches shows nothing; the check makes one network call per source; manual skills show "Not tracked".

## U5. Install by preferred method and harness

**Person.** The user found a skill on skills.sh and wants it in Claude Code and Codex, the way they always install.

**Today.** Works through `npx skills add` with a trust prompt, an operation you can cancel, and CLI trace parity for the symlink layout (skill_add_operation.rs:794; skill_materialize.rs:16). Method and harness defaults are recomputed from the environment on every open of the sheet and never saved (add_method_defaults.rs:111).

**Target.** The user's last choices are saved in the registry as `preferred_method` and `preferred_harnesses` and pre-fill the sheet. The install still runs the CLI; the app never re-implements the fetch. The trust prompt appears on every path, including a re-install. Success is reported only after the lock file, the links, and the journal entry are all on disk.

**Uses.** Source, Fetch through the CLI, Link, Journal, Lease.

**Done when.** CLI trace parity for `add` holds; the second install pre-fills the first install's choices; the operation runs in `spawn_blocking`.

## U6. Update

**Person.** The user clicks "Update all" on a Monday.

**Today.** One skill at a time through `npx skills update`, in an async command that blocks a Tokio worker for the whole run (commands.rs:2397, 2464).

**Target.** Update one or update all, off the UI thread, each skill its own journal entry, the list updating as each finishes. Undo restores the previous tree from the quarantine.

**Done when.** CLI trace parity for `update`; "Update all" on ten outdated fixtures ends with ten journal entries and zero main-thread calls over one frame.

## U7. Park and unpark, the tracer slice

**Person.** The user turns a noisy skill off everywhere for a week.

**Today.** Rename into `~/.agents/skills-parked`, plus link removal for Claude Code, in a sync command on the main thread with no journal and no lease (skill_park.rs:463).

**Target.** The first slice on the new stack: plan, Lease, Journal, Swap, Link, event, through the desktop, the CLI, and the MCP server from one `ops::park`. Codex's `[[skills.config]]` row and OpenCode's deny key are updated or left consistent (codex.md, "What the core must handle").

**Done when.** The crash test after each step leaves the disk in the before or the after state; a second process gets Busy; the old `skill_park.rs` path is deleted.

## U8. Fix

**Person.** A skill stopped loading in Codex after a rename.

**Today.** Frontmatter repair with a preview (skill_frontmatter_repair.rs:281, 407) and link repair (event_commands.rs:529). No single "fix" entry point; the user has to know which one applies.

**Target.** One "Fix" action per skill that runs the doctor checks for that skill (lifecycle-states.md) and offers the repairs it can do: name mismatch with the folder, missing frontmatter fields, a dangling link, a stale Codex row, a stale OpenCode deny key. Anything it cannot fix is named with the file path.

**Done when.** Each doctor invariant has a fixture that breaks it and a repair test that restores it.

## U9. Conflicts, handed to the editor

**Person.** The same skill exists in `~/.agents/skills` and in a project's `.claude/skills` and they differ.

**Today.** The DTO carries every distinct content hash (skill_dto.rs:333); the UI only hides the size field when they differ. "Pull upstream" on a fork runs a three-way merge and, on conflict, shows a toast that says to open the editor (skill-page-actions.ts:223). The editor picker and launcher exist (skill_editor.rs:191, 414).

**Target.** The app never merges by itself. It shows which files differ and a one-line summary per file, then opens both copies in the user's editor, side by side where the editor supports it. For a fork, conflict markers go in the file and the editor opens on it, as `git` does.

**Done when.** Two fixture copies that differ open in the chosen editor with both paths; the app writes nothing until the user acts.

## U10. Turn off for one harness

**Person.** A skill is useful in Claude Code but noisy in Codex.

**Today.** Claude Code: remove the per-skill link (refuses on a whole-folder link). Codex: `[[skills.config]]` row with `enabled = false`. OpenCode: `permission.skill.<name> = "deny"`, refused for `.jsonc` and for a name with two deployments. pi: none. See each harness file, "How the app turns a skill off".

**Target.** Each harness's own switch, through its adapter, with the registry recording the intent so a rescan can tell "off by the app" from "gone". Claude Code gains `skillOverrides` once its safety is confirmed; pi gains the settings exclusion once its format is confirmed.

**Done when.** The four adapter switch tests pass against the fixture homes; a park keeps the Codex row and the OpenCode key consistent.

## U11. Undo

**Person.** The user removed the wrong skill.

**Today.** 8 of 46 writes have a journal entry and a restore path (events-and-history.md). Quarantine exists for trials.

**Target.** Every write is a journal entry with a plan and a quarantine of what it replaced. `skill-studio undo` and the Activity view reverse the last entry. Retention is capped by size and age.

**Done when.** 46 of 46 writes journaled; a test reverses each kind of write from its journal entry.

## U12. Remove

**Person.** The user removes a skill they never use.

**Today.** `npx skills remove` in an async command that blocks a worker (commands.rs:1850, 1985).

**Target.** Same CLI, off the UI thread, journaled, with the removed tree in quarantine for undo.

**Done when.** CLI trace parity for `remove`; undo brings the tree back with the same TreeHash.
