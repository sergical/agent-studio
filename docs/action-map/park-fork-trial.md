# Park, fork, and trial

This area moves a skill out of active use without deleting it (park), detaches it from its provider so local edits survive an update (fork), and tracks a time-boxed install that expires on its own (trial).

Commands: park_skill, unpark_skill, fork_skill, unfork_skill, pull_fork_upstream, keep_skill_trial, the trial expiry loop and skills://trial-expired.
UI entry points: header Park/Unpark/Fork/Un-fork/Pull latest, Locations card Unpark, InstalledSkillHeader Keep, trial toast Restore.

## Current state

| Command            | Journal | Concurrency                                | Partial-state risk                                         |
| ------------------ | ------- | ------------------------------------------ | ---------------------------------------------------------- |
| park_skill         | no      | per-root lease                           | crash between the rename and the registry write            |
| unpark_skill       | no      | per-root lease                           | unparked skill with a missing Claude link                  |
| fork_skill         | no      | per-root lease                           | detached but empty skill dir if manual recovery is skipped |
| pull_fork_upstream | no      | per-root lease                           | crash between two renames inside the swap                  |
| unfork_skill       | no      | per-root lease                           | stale fork record pointing at a reinstalled skill          |
| keep_skill_trial   | no      | per-root lease                           | none noted, the registry write is atomic                   |
| trial expiry loop  | no      | per-root lease, taken by the loop itself | none noted beyond the trash-and-drop it performs           |

`park_skill` removes the per-skill Claude link, renames `~/.agents/skills/<name>` to `~/.agents/skills-parked/<name>`, inserts a `ParkedRecord`, retargets any trial, then writes the registry (skill_park.rs:463 → 210, :242, :254, :278).
A rename failure restores the link.
A registry-write failure renames the folder back and restores the link too, but that recovery uses `let _ =` — a best-effort write with no check that it succeeded (:244, :280).
`unpark_skill` compares the shared dir against the parked copy: an identical reinstall drops the parked copy, a divergent one moves it to `skills-trash` instead of overwriting it, and otherwise it renames the parked dir back and restores the Claude link, also best effort (skill_park.rs:498 → 308, :348, :352, :369, :385).

`fork_skill` fetches the upstream tree into `<app_data>/skill-studio/forks-snapshot/<name>`, writes a `ForkRecord` and drops trials, copies the live tree to a recovery dir, runs the ledger CLI removal, restores the shared dir from the recovery copy if the CLI wiped it, then removes the recovery dir (skill_fork.rs:960 → 780, :803, :836, :888, :918, :931, :939).
`rollback_fork_before_detach` undoes as much of this as it can depending on how far the call got.
A restore failure after a successful detach is only reported with the recovery path, not retried (:683, :932).
`pull_fork_upstream` rebuilds staging-live and staging-base, merges conflicting files with `git merge-file`, then swaps in the result: live renamed to a backup, staging-live moved in, old base renamed to a backup, staging-base moved in, registry written, both backups deleted (skill_fork.rs:1404 → 1236, :1286, :1311, :1157).
Each swap step rolls back the prior renames on failure, but this is manual step-by-step rollback, not a single transaction.
A crash between two renames is a named partial-state risk (:1181–1224).
`unfork_skill` reinstalls from origin — overwriting local edits — before it clears the fork and trial records (skill_fork.rs:1471 → 1443, :1456).
A registry-write failure after a successful reinstall leaves a stale fork record pointing at a skill that is no longer forked (:1458).

`keep_skill_trial` only rewrites `~/.agents/skill-studio.json` to drop the trial entries and never touches the folder (skill_trial.rs:843, :86).
The trial expiry loop runs 15 seconds after startup and then every 5 minutes: it takes `ForkMutationLock`, rebuilds the snapshot, moves expired trials to the trash, drops their registry records, and emits `skills://trial-expired` with the name and trash path (skill_trial.rs:828, lib.rs:133).
The frontend listens for that event and shows a 15-second warning toast with a Restore button that calls `restore_trashed_skill` (App.tsx:89).

None of park, unpark, fork, pull, or unfork writes a journal event — `make_skill_independent_copy` is the only command in the whole map that does.
All five hold the same process-wide `ForkMutationLock`.
On the frontend, the header exposes Park/Unpark and Fork/Un-fork with toast feedback and, for Un-fork only, a confirm dialog ("Discard your changes and reinstall from {origin}?") (SkillPageHeaderActions.tsx:68, 72).
The Locations card exposes a separate Unpark control (SkillLocationsCard.tsx:99).
The row menu exposes Park (SkillLocationMenu.tsx:874).
`InstalledSkillHeader`'s Keep button calls `keep_skill_trial` (:103).
Tests cover the rename-rollback and registry-rollback paths for park, unpark, fork, and pull in detail.
keep_skill_trial has one test.

## Changes in the Claude stack (#73 to #134)

PR #73 moved scan, lifecycle, events, and DTOs into `skill-studio-core` and `skill-studio-host`, and stood up the CLI, MCP server, and TUI on the same core.
Park, unpark, fork, pull, unfork, and the trial commands kept their existing rename-and-rollback write order through that move.
No other Claude-stack PR in this range is listed against this area, so the extraction is the only change here on that side.

## Changes in the Codex stack (#79 to #141)

PR #113 made Dotagents Fork durable: it detaches only the selected manager records, saves the exact upstream baseline including symlinks, records a durable event, bounds the fetch with deadlines and byte/entry/depth limits, and lets startup complete an interrupted publication when saved evidence still matches, leaving conflicting edits untouched.
PR #123 did the same for skills.sh Fork: it preserves local files, records a durable operation, restores the original skill if the provider deletes the folder before updating its lock, and uses an event-bound lock so restart recovery cannot race the provider.
PR #128 made Fork's Pull durable, publishing the live tree, baseline, and registry atomically and recovering an interrupted pull without replaying network or provider work, with Home, detail, and Update all targeting the correct deployment.
PR #133 made Unfork durable, restoring an eligible fork to its provider after confirmation through a durable publication record, with interruption recovery that either completes verified publication or preserves changed evidence for review.
PR #136 made Copy trial expiry and its Restore durable, retaining a backup through the shared core, refusing an occupied restore destination, and settling an interrupted finalization on the same event after restart.
All five PRs explicitly exclude CLI, MCP, and cloud environments and state that none of Fork, Pull, or Unfork offers a generic History Undo.

## Desired state

Park, unpark, fork, pull, and unfork each record a journal event with a backup and an inverse before the rename or CLI call that starts the operation, matching the pattern `make_skill_independent_copy` already uses.
A startup reconciler then repairs an interrupted run the way `reconcile_interrupted_independent_copy` does, instead of each command's own hand-rolled best-effort rollback.
One write path in `skill-studio-core` implements the stage-verify-swap shape that park, unpark, fork, and pull already approximate separately, so a crash-window fix in one does not need to be re-applied to the other three.
A per-scope lease, keyed to the skill being parked or forked, replaces the single `ForkMutationLock`, so parking one skill does not block forking another.
The registry read-modify-write inside each of these commands runs under that lease.
`pull_fork_upstream`'s four-rename swap becomes one transaction — or, short of that, its rollback closures are replaced by the same journal-and-inverse mechanism, so a crash between renames is repaired at startup instead of merely being a documented risk.
`unfork_skill` records its journal event, and takes the backup, before it runs the reinstall that discards local edits, so a registry-write failure after the reinstall does not leave a stale fork record — the event's inverse can at least tell the user what happened.
`park_skill` and `unpark_skill`'s best-effort recovery writes (`let _ =`) are replaced with checked writes that fall back to the journal's own repair path on failure, not a second silent write.
Success feedback for park, fork, and pull only shows once the registry write commits, not before.
Every one of these six commands gets a crash-window test matching the coverage `make_skill_independent_copy` already has, and the trial expiry loop — which already takes the lock, rebuilds the snapshot, and writes the registry in one pass — gets the same journal treatment as the rest, since it is a background writer with the same partial-state risk as the foreground ones.

## Gaps

- None of park_skill, unpark_skill, fork_skill, pull_fork_upstream, or unfork_skill writes a journal event.
  Only make_skill_independent_copy does, so only that command gets automatic startup repair.
- park_skill and unpark_skill's recovery writes on the rollback path use `let _ =`, discarding the result instead of checking it or repairing on failure.
- fork_skill's restore-after-detach failure is reported with a recovery path but not retried or automatically repaired.
- pull_fork_upstream's four-step rename swap is manual step-by-step rollback, not one transaction.
  A crash between two renames is a named, unrepaired partial-state risk.
- unfork_skill's registry write happens after the CLI reinstall already discarded local edits, so a write failure leaves a stale fork record with no compensating step.
- All six commands share one process-wide ForkMutationLock.
  There is no per-skill or per-scope lease.
- The registry read-modify-write inside each command has no lock beyond the whole-call ForkMutationLock, so the read and the write are not atomic with respect to the lease boundary described in the desired state.
- The trial expiry loop performs the same kind of multi-step write (move to trash, drop registry record) as the foreground commands but has no journal event and no direct crash-window test.
- Backups and quarantine directories (`skills-trash`, fork snapshots, recovery dirs) accumulate with no retention limit.
  The app never deletes trash entries except during trial expiry.
- keep_skill_trial, park_skill, unpark_skill, fork_skill, pull_fork_upstream, and unfork_skill have rollback-path tests but no crash-window (kill-mid-write, restart-and-reconcile) tests like make_independent_copy's crash-recovery suite.
