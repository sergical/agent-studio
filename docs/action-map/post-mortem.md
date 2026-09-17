# Post-mortem: why the two finished stacks missed the target

Read on 2026-09-16 from the session transcripts on disk. Claude Code sessions live under `~/.claude/projects/-Users-sergiydybskiy-src-agent-studio*/`. Codex sessions live under `~/.codex/sessions/2026/09/`. The question was: the prompts that produced pull requests #73 to #134 and #79 to #141 were written by the same person with the same goal, so why did neither stack reach the rules in the README?

## Short answer

Neither stack was asked for the rules. The prompts asked for module structure, a verification loop, type signatures, native plugin discovery, and "all of this end to end". No prompt named a journal event, a lock, an atomic write, a rollback, a trust prompt, a test target, or a rule that every command has a caller. Both models filled those gaps with the cheapest reading of "durable", and the tests they wrote encoded that reading. The one defect that was raised during the work, the empty inventory after a slow scan, was raised by the user running the app, not by a test.

## The Claude Code sessions

| Session  | Dates                    | Pull requests                                        |
| -------- | ------------------------ | ---------------------------------------------------- |
| f30c30ac | 2026-08-22 to 2026-09-03 | Strip down to skills, native plugin discovery        |
| 8dbae7bf | 2026-09-09 to 2026-09-13 | #73 core crate with CLI, MCP, and TUI skeletons, #74 |
| 9f53962b | 2026-09-13 to 2026-09-17 | #76 to #97, #105 to #112, #114 to #134               |
| 3fdf5f15 | 2026-09-17               | Review of bot fixes #61, #62, #68 to #72             |

What the prompts asked for, quoted:

- 2026-08-22: "i want to strip it down to just a great way to manage, sync and test skills... native support for understanding where skills came from."
- 2026-09-09: "we need to separate the functionality into proper modules." Also a verification loop, type signatures, and a worktree per task.
- 2026-09-13: "lets commit first and then yeah go through the rest" and "i want all of this end to end."

What the prompts never asked for: journal events on every write, a lease per scope, atomic registry updates, rollback for chained writes, a trust prompt on every install path, a test per command, or a caller for every IPC command.

The one defect raised during the work: on 2026-09-10 the user saw plugins vanish from the list and asked why. The cause was the two-second scan budget dropping the global roots. The fix reordered the scan so the global roots run first. It did not keep the last good snapshot on a failed scan, so the same class of failure is still open in skill_refresh.rs.

The other seven defect classes in the README, the 38 unjournaled writes, the 23 commands with no direct test, the 11 commands with no caller, the store install with no trust prompt, the chained writes with no rollback, the last-write-wins registry, and the unbounded backups, were never raised by the user or the assistant. The action-map read on 2026-09-16 found them all at once.

Root causes:

1. **The spec was written from the skeleton.** The design decisions D1 to D15 in the shared-core notes were written after the core crate's `todo!()` bodies existed. None of them mention journaling, locks, or atomicity. The ops tests passed against empty bodies.
2. **No design review gate.** The desktop scanner deleted 11 tests with the note "covered by core", and nobody checked the test names. Defects surfaced by lint or by a later adversarial review, not by a review of the design.
3. **Fixtures encode the author's assumptions.** Every test home lacked the state that triggered a real defect: the slow scan, the broken symlink, the home folder registered as a project. The binary name collision was invisible to a single-crate `cargo test -p`.

## The Codex sessions

The Codex rollouts from 2026-09-15 and 2026-09-16 contain very few user messages. The work ran from a task scope carried between sessions, not from chat prompts, so there is no prompt text to quote. Pull requests #78 to #80 and #88 to #92 were opened on 2026-09-15, #93 to #104, #109 to #113, #123, #128, and #133 on 2026-09-16, and #135, #136, and #141 on 2026-09-17.

What the Codex stack did well: it made the write side durable. Copy, remove, and repair got undo and redo, atomic staging, and a lock around mutations. This is why the 2026-09-16 comparison picked the Codex core to own writes.

What it did not do, read from the pull requests and the code, not from prompts:

- Reads and the UI were left behind. A scan past its budget still returns an empty list marked partial, and the desktop renders the empty state.
- The registry file `~/.agents/skill-studio.json` still has no lock and no version field.
- The mutation lock is one global try-acquire with a retry loop in the desktop, not a lease per scope.
- Add and remove still fork the `npx skills` CLI from desktop commands, so the desktop bypasses the core crate for the most common writes.
- Tests were written per pull request, not per command. No test drives all nine CLI operations through one gate. The two defects it did catch, the binary name collision and the home root discovery, were caught by running the binary by hand.

Root causes:

1. **Two cores from one base with no shared contract.** The Codex core started on 2026-09-15 from the same base as #73. The two read "durable" differently: recovery and undo on one side, atomic repair on the other. The read side was on neither side's list until the 2026-09-16 review.
2. **Durability read as write-side atomicity only.** Nothing in the task scope said "every read sees the durable state" or "a failed read keeps the last good state", so the model did not build it.
3. **Test coverage followed delivery cadence.** Each pull request shipped its own tests. No cross-operation invariant test exists, such as "watch announces a revision before the inventory changes" or "restore preserves ownership".

## What this changes

The README rules and definition-of-done.md exist because of this read. Three practices follow from it:

- **Write the rules before the code.** Every future prompt for this area points at the README rules and the definition-of-done checklist, so "durable" has one meaning.
- **Review the design, not only the diff.** A pull request that deletes tests names each test and says which new test covers it. A pull request that adds a write names its journal event and its lease.
- **Fixtures carry the real failures.** The fixture homes get the slow scan, the broken symlink, the home folder as a project, and a second process holding the lease. A defect found in the running app becomes a fixture before it becomes a fix.

Readers who want the raw reads can find them in the session scratchpad under `action-map/postmortem-claude.md` and `action-map/postmortem-codex.md`. Those files are not in the repository.
