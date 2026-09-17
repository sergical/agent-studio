# Skill assistant runs

Deferred on 2026-09-17: agent runs are out of the first release scope (scope-decision in plan.md). The `ops` functions and their tests stay so the CLI and MCP can still drive them; nothing in user-stories.md depends on this file.

The skill assistant drawer runs Ask, Audit, and Test sessions against a real harness process (Claude, Codex, OpenCode, or pi) in a scratch folder or a project worktree, streams events to the UI, and records a bounded run history per skill.
The feature sits behind the `skill-assistant` flag, off by default.

Commands: start_skill_agent_run, cancel_skill_agent_run, create_skill_scratch_dir, remove_skill_scratch_dir, prepare_skill_run_target, skill_run_target_diff, apply_skill_run_target_diff, discard_skill_run_target, reveal_skill_run_target, record_skill_run, list_skill_runs, read_skill_run_events, the skill-agent://event stream.
UI entry points: SkillAssistantPanel (Ask, Audit, Test, harness picker, New session), SkillRunHistory, useSkillAgentRun.

## Current state

`start_skill_agent_run` (skill_agent_runner.rs:956) validates the client-supplied run_id, checks it is not already running, finds the harness binary with `command -v`, and spawns it in its own process group.
It writes nothing to disk directly; it streams skill-agent://event until a Finished event.
A duplicate run id, a missing binary, or a spawn failure all end in a Finished{ok:false} event rather than a command error (:1080).
The run entry is removed from the in-memory map when the task ends.
There is no spawn-level test; it needs a real binary.

`cancel_skill_agent_run` (:1270) is idempotent: an unknown run id is a silent no-op, and a second cancel on an already-cancelled run is a no-op through an AtomicBool swap.
It calls terminate_process_group inside the run's own task and never fails.

`create_skill_scratch_dir` (:1377) builds `<app_cache>/skill-studio/scratch/<timestamp>-<pid>`, copies each named skill in, best-effort `git init -q`, and adds `.claude/skills` and `.pi/skills` links.
A mid-sequence failure leaves a partially populated directory for remove_skill_scratch_dir to clean up later; there is no direct test for this command.
`remove_skill_scratch_dir` (:1438 → 1420) only accepts a path that canonicalizes to an immediate child of the scratch root.
The UI calls it fire-and-forget with `.catch(() => {})`, so a failure leaves the directory on disk with no retry.

`prepare_skill_run_target` (skill_run_target.rs:173) sets up one of three kinds before a Test run: Scratch (a cache dir), Worktree (`git worktree add --detach` plus a setup commit), or InPlace (nothing, but refuses a dirty tree).
A worktree created before a later failure is left behind until discard_skill_run_target runs.
`skill_run_target_diff` (:479) is a read: it runs `git add -N .`, `git diff`, then `git reset -q` in the target's working copy to produce a unified diff, and resets its own transient index change afterward, so a git failure mid-sequence can leave the index touched with no direct test to catch it.
`apply_skill_run_target_diff` (:607 → 532) is Worktree-only: it re-checks the project tree is clean and at the stored git_head, writes a patch to the system temp dir, applies it with `git apply --3way`, and on failure runs restore_after_failed_apply to check out and clean the touched paths.
`discard_skill_run_target` (:737 → 722) removes a Worktree directory, or for InPlace runs `git reset`, `git checkout`, and `git clean` per path bucket against current git status, not a stored baseline; a failure mid-sequence leaves it half-reverted.
None of the run-target commands journal.

`record_skill_run` (skill_run_history.rs:84 → 95) writes `<id>.json`, then `<id>.events.jsonl`, then last.json for Test, then trims history to the newest 20 pairs per skill by mtime.
A write failure between the .json and .events.jsonl steps leaves a half pair; there is no lock, so two concurrent calls for the same skill could race in the trim step.
`list_skill_runs` (:214) and `read_skill_run_events` (:252) are both reads with traversal-safe path checks; the UI swallows both commands' errors with `.catch(() => {})` and shows an empty list or "No runs recorded yet" instead.

The event stream is one skill-agent://event listener per mounted panel, filtered by run_id (useSkillAgentRun.ts:99-108).
Events for another run id are dropped.
After unmount the listener is gone, so a late event from a still-running backend process is lost, and cancel on unmount is only best effort.

Cleanup rules, summarized in the source map's table at line 1495: on panel unmount, cancel and scratch-dir removal both run fire-and-forget; a Scratch or Worktree target is discarded, but an InPlace target is only kept with a toast, never discarded.
A skill or deployment change clears state but does not remove the scratch dir from disk.
A Worktree target survives past "Keep" until a later discard call.
The source map calls out these as known leaks, not proposed fixes.

Tests: start_skill_agent_run's command-building logic has five named tests (:1477-1548); cancel_skill_agent_run has two; record_skill_run has six, including two path-traversal refusals; prepare_skill_run_target has five, including a fixture traversal refusal; apply_skill_run_target_diff has two; discard_skill_run_target has two (in_place_discard_*).
create_skill_scratch_dir, reveal_skill_run_target, and skill_run_target_diff have none.

The harness dropdown and Runs list add their own reset behavior on top of the commands above.
Switching harness cancels the current run, releases the active target, and resets local state (SkillAssistantPanel.tsx:1209-1217); a failure there shows a toast, "Couldn't discard the test run," but the panel proceeds with the switch regardless.
Runs list and run row failures are both swallowed, per the table above, so a user who hits a read error sees the same "No runs recorded yet" state as a skill with no history at all.

## Changes in the Claude stack (#73 to #134)

Only #68 touches this area: "fix: render cancelled skill-agent runs as Cancelled, not Failed."
It is a bot-authored UI fix that changes how the run history and transcript label a cancelled run, so a cancel no longer shows as a failure in SkillRunHistory or the live transcript.
It does not change any backend command, journaling, or the scratch/worktree cleanup logic.
No other PR in the #73-#134 range touches SkillAssistantPanel, the run-target commands, or run history.

## Changes in the Codex stack (#79 to #141)

No open PR in the Codex stack's range touches this area either.
`gh pr list --state open --limit 100 --search "agent run"` returns PRs about Activity, discovery, and core refactors that happen to contain the word "run" in unrelated contexts, for example #122, #124-#127, #130 are Activity skill-use PRs.
None of them names SkillAssistantPanel, skill_agent_runner.rs, skill_run_target.rs, or skill_run_history.rs.

## Desired state

Every write command here — create_skill_scratch_dir, remove_skill_scratch_dir, prepare_skill_run_target, apply_skill_run_target_diff, discard_skill_run_target, record_skill_run — records a journal event with a backup and an inverse before it touches disk, so a crash mid-worktree-setup or mid-diff-apply leaves a traceable, reversible state instead of an orphan directory or a half-reverted tree.
skill_agent_runner.rs, skill_run_target.rs, and skill_run_history.rs become one write path in skill-studio-core, shared by the desktop command layer, the CLI, and the MCP server, since all three already run harness sessions.
The single in-memory runs mutex and the single targets mutex become per-skill or per-target leases, so two skills' Test runs, or two targets for the same project, do not serialize on one lock or race with no lock at all, as record_skill_run's trim step does today.
prepare_skill_run_target, apply_skill_run_target_diff, and discard_skill_run_target become one transaction per target: either the whole chain (setup, patch, worktree removal, target-entry drop) commits, or a failure at any step leaves a compensating action queued, not a bare error string with the target left for manual retry.
Success feedback (the Finished transcript, the Keep/Discard toast, the Apply toast) fires only after the last step in its chain; list_skill_runs and read_skill_run_events stop swallowing errors and show the user a named partial result instead of an empty list indistinguishable from "no runs yet."
create_skill_scratch_dir and remove_skill_scratch_dir get direct tests, plus a crash-window test that kills the process mid-copy and asserts the leftover directory is listed for the user, not silently orphaned.
The same crash-window discipline applies to prepare_skill_run_target's worktree setup and apply_skill_run_target_diff's patch-then-apply sequence.
Scratch dirs and run targets are always either cleaned up or listed for the user: the InPlace-target-never-discarded gap and the Worktree-target-survives-Keep gap both get a visible entry in Settings or the assistant panel, rather than living only as an in-memory leak the source map documents but does not fix.

`skill_run_target_diff` fits the same transaction and testing principles: its `git add -N .` / `git diff` / `git reset -q` sequence becomes atomic, or covered by a crash-window test, so an interrupted diff never leaves a stray index entry in the target.

## Gaps

- create_skill_scratch_dir: no journal, no backup, no direct test; a mid-copy failure leaves a partial scratch dir.
- remove_skill_scratch_dir: fire-and-forget from the UI with a swallowed error; a failure leaks the scratch dir with no retry and no user-visible listing.
- prepare_skill_run_target (Worktree): a late failure leaves an orphan git worktree until a manual discard; no journal.
- apply_skill_run_target_diff: three-step chain (patch write, apply, worktree removal) with no transaction; a failed apply leaves the target for retry with only an error string.
- discard_skill_run_target (InPlace): reverts against live git status, not a stored baseline, in three ungrouped steps; a mid-sequence failure leaves the tree half-reverted.
- record_skill_run: no lock around the trim step; two concurrent calls for one skill can race; a write failure between .json and .events.jsonl leaves a half pair.
- list_skill_runs / read_skill_run_events: UI swallows both commands' errors, showing an empty list indistinguishable from "no runs yet."
- Cleanup: an InPlace target is never discarded on panel unmount, only flagged with a toast; a Worktree target survives "Keep" until a separate discard call; a late skill-agent://event after unmount is lost with the backend process only cancelled best effort.
- All commands: runs and targets are each a single mutex, not a per-skill or per-target lease.
- Logic lives in three separate desktop-only Rust files, not a shared core crate reusable by the CLI or MCP server.
- skill_run_target_diff: transient `git add -N .` index change is reset after the diff, but a git failure mid-sequence has no direct test and no journal to confirm the reset happened.
- No open PR in either stack currently plans to address any of the above; #68 is UI-label-only.
