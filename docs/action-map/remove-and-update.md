# Remove and update

This area takes a skill off disk, or refreshes it in place, and checks whether a newer version exists.

Commands: remove_skill (Copy, dotagents, Fork, skills-sh variants), update_skill (dotagents, skills-sh), check_skill_updates_now, the update-check loop, restore_trashed_skill.
UI entry points: header Remove and Update, RemoveDeploymentsDialog, InstalledSkillLifecycleActions, Home inbox Update.

## Current state

| Command                  | Journal | Concurrency                              | Partial-state risk                                                        |
| ------------------------ | ------- | ---------------------------------------- | ------------------------------------------------------------------------- |
| remove_skill (skills-sh) | no      | ForkMutationLock                         | stale trial record when the trial drop fails after the CLI succeeded      |
| remove_skill (dotagents) | no      | ForkMutationLock                         | links left staged when the restore itself fails                           |
| remove_skill (Copy)      | no      | ForkMutationLock                         | crash between the deletes and the registry write, before rollback can run |
| remove_skill (Fork)      | no      | ForkMutationLock, not re-acquired inside | skill left under skills-trash when the restore fails                      |
| update_skill (skills-sh) | no      | ForkMutationLock                         | CLI partial writes stay on disk                                           |
| update_skill (dotagents) | no      | ForkMutationLock                         | CLI partial writes stay on disk                                           |
| check_skill_updates_now  | n/a     | in-progress guard shared with the loop   | none noted; failures are only logged                                      |
| restore_trashed_skill    | no      | ForkMutationLock                         | incomplete target folder after an entry-count mismatch                    |

`remove_skill` dispatches on the owner kind.
The skills-sh variant shells out to `npx skills remove`, drops the trial record, and requests a snapshot rebuild (commands.rs:1875 → 1961).
A trial-drop failure after a successful CLI removal is only printed, not surfaced (:2035).
The dotagents variant re-verifies every dependent Claude link, stages them into `~/.agents/skills-trash/.dotagents-link-remove-<pid>-<n>/`, runs the CLI, then deletes the staged links (commands.rs:1924 → 1735, :1822).
A stage or CLI failure restores what was already staged, but a restore failure is only reported in the error (:1834, :1850).
The Copy variant stages each target into `skills-trash`, fingerprint-checks it, deletes the live paths, then rewrites the registry (commands.rs:1965 → 1501, :1599).
A mid-loop delete failure restores everything staged, and a registry failure restores memory and disk, but a rollback failure compounds the error rather than resolving it (:1625, :1648).
The Fork variant renames the dir into `skills-trash`, fingerprint-checks it, drops the registry and trial records, then removes the backup and the fork snapshot (commands.rs:1907 → 2062, :2134).
A registry failure restores from the backup, but a restore failure leaves the skill sitting at the backup path (:2168).
None of the four variants writes a journal event.

`update_skill` (skills-sh) shells `npx skills update` in place, then runs a background update check for that owner and requests a snapshot rebuild (commands.rs:2402, :2485).
`update_skill` (dotagents) requires a ledger entry for the deployment and, for a pinned entry, a cached latest commit — otherwise it refuses with "run Check now first" (commands.rs:2428 → skill_lifecycle.rs:281, :307).
Both variants have no rollback if the CLI leaves a partial rewrite on disk.

`check_skill_updates_now` and the six-hour update-check loop share one in-progress guard (skill_update_check.rs:872/891).
The loop asks `gh api` or a git remote for the latest commit per source, writes `UpdateCheckState` and `update-check.json`, then requests a rebuild.
A network or `gh` failure is only logged with `eprintln`, and `check_skill_updates_now` has no frontend caller at all (skill_update_check.rs:891, lib.rs:164).

`restore_trashed_skill` copies a trashed skill back into `~/.agents/skills/<name>` and re-applies the Claude link rule.
It checks the copied entry count against the source but does not delete the trash copy or clean up a half-written target on mismatch (skill_trial.rs:985 → 937, :969, :970).

All five write commands hold `ForkMutationLock` for their whole call — one process-wide lock, not scoped to the skill being removed or updated.
On the frontend, the header Remove button shows "Remove {name}?" then, confusingly, reports success as "Updated N deployments" (SkillPageHeaderActions.tsx:89).
The cross-check notes this as a UI/feedback mismatch.
Home inbox Update (HomeInboxGroups.tsx:142) and Park (:184) call `update_skill` and `park_skill` directly with toast feedback.
`InstalledSkillLifecycleActions.tsx` drives Update and Remove from the store panel, with its own removal-preview confirm dialog (:267–312).
Tests cover each variant's argv construction, rollback paths, and (for Copy and Fork) registry-write-failure recovery.
The update-check loop's network-failure path has fake-lookup tests but no test asserts the toast a user would see.

## Changes in the Claude stack (#73 to #134)

PR #73 extracted scan, lifecycle, events, and DTOs into `skill-studio-core` and `skill-studio-host`, and built the CLI, MCP server, and TUI on the same core.
Remove and update kept their existing per-owner-kind write order after the move.
The extraction changed where the code lives, not how it fails or rolls back.
No other Claude-stack PR in this range touches remove or update directly.

## Changes in the Codex stack (#79 to #141)

PR #78 kept a removal failure's reason visible inside the open Locations dialog and in the toast, instead of losing it, and wrapped long filesystem paths so they do not break the dialog layout; this is presentation only, with no backend change.
PR #94 made desktop Copy removal atomic and recoverable: it now moves the selected tree and its verified reader links into quarantine, records a durable event before the move, and either publishes the ownership change or recovers the original paths, with startup retrying an interrupted removal against current approved roots.
PR #102 made `update_skill` match the exact managed owner and its source evidence before running, moved the provider call onto a blocking worker with a bounded deadline, and refreshed owner evidence and inventory after completion or partial failure, distinguishing "Unknown" from "Up to date" in the UI.
PR #103 moved provider removal onto the same blocking worker, refreshed inventory after every terminal result, and reported provider and trial-cleanup failures through the desktop instead of showing a generic "Unknown error."
PR #109 made Fork removal durable: it records an event before it moves the exact global deployment into quarantine, then removes only its matching ownership and trial records, and startup recovery restores the original before ownership publication or finishes the removal afterward without touching a conflicting replacement.

## Desired state

`remove_skill` and `update_skill`, for every owner kind, record a journal event with a backup and an inverse before the first write — the stage-then-rename shape all four remove variants already use is most of the way there; what is missing is the event row and a startup reconciler like `reconcile_interrupted_independent_copy`.
One write path per operation lives in `skill-studio-core`: today Copy, dotagents, Fork, and skills-sh each reimplement stage-verify-commit-rollback with small variations, and a single parameterized implementation removes that duplication and the gaps between the variants' failure handling.
A per-scope lease replaces the single `ForkMutationLock`, so removing one skill does not block an unrelated update.
The registry read-modify-write in each remove and update variant runs under that lease, not a plain read-then-write with no lock around it (see `~/.agents/skill-studio.json` in the shared-state table).
Update's CLI call plus its background check-now plus its snapshot rebuild is one transaction, or the check-now step is a real compensating step rather than a best-effort follow-on that can silently fail.
Success feedback fires only once the last step (snapshot rebuild, in most cases) completes, and the header Remove button's "Updated N deployments" text is replaced with a message that matches the action.
Every remove and update variant gets a crash-window test, matching the recovery tests Copy and Fork removal already have for dotagents and skills-sh.
`check_skill_updates_now` either gets a caller in the UI (a "Check now" button, which the map notes does not exist) or is folded into the loop and dropped as a public command.

## Gaps

- None of `remove_skill`'s four variants or either `update_skill` variant writes a journal event; recovery logic is hand-rolled per variant instead of going through a shared reconciler.
- Four separate remove implementations (Copy, dotagents, Fork, skills-sh) duplicate stage-verify-commit-rollback logic in the desktop crate instead of sharing one core implementation.
- `ForkMutationLock` is one process-wide lock across remove, update, park, fork, and pull.
  Removing skill A blocks updating skill B.
- skills-sh remove's trial-drop failure after a successful CLI removal is only printed, leaving a stale trial record with no user-visible warning.
- dotagents remove and Copy remove can leave links or paths staged in `skills-trash` when their own rollback also fails, with no automatic second attempt.
- Fork remove's registry-write rollback can itself fail, leaving the skill at the backup path with only an error string to explain it.
- update_skill's CLI-then-check-now-then-rebuild sequence has no transaction boundary; a check-now failure after a successful CLI update leaves stale update evidence with no compensating step.
- `check_skill_updates_now` has no frontend caller.
  The six-hour loop is the only way the check ever runs.
- The header Remove button's success toast reads "Updated N deployments," which does not match the Remove action.
- `restore_trashed_skill` does not clean up a half-written target or the trash copy when the entry-count check fails.
