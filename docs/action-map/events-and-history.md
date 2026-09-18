# Events and history

This area is the append-only event log every mutating command writes to.
It covers the backups the log keeps, the restore it offers, and the reconcile pass that repairs a crash.

Commands: `list_skill_events`, `restore_skill_event`, the event store (`<app_data>/events.sqlite3`, `<app_data>/backups/<event-id>/`), the startup reconcile loop, the eight-command journal rule.

UI entry points: `SkillHistorySection.tsx` (Restore, Reveal in Finder), `SkillActivityView.tsx` / `ActivityFilters.tsx` / `ActivityYear.tsx` / `ActivityDayDetails.tsx`.

## Current state

The event store is SQLite via `rusqlite`, in WAL mode, at `<app_data_dir>/events.sqlite3`.
It sits deliberately outside `~/.agents` (`docs/spec-event-store.md:26-31`).
One `events` table holds a ULID id, timestamp, kind, skill, harness, scope, a JSON payload, a JSON inverse (`NULL` means not restorable), a backup dir, a status (`pending`/`done`/`failed`/`interrupted`), and a `reverted_by` self-reference (`docs/spec-event-store.md:41-54`).
Backups live under `backups/<event-id>/`, with a `manifest.json` mapping original paths to relative backup paths and a SHA-256 fingerprint per top-level path (`docs/spec-event-store.md:32-36`).

Every journaling command follows the same five phases (`docs/spec-event-store.md:99-115`).
It allocates an id before touching disk.
It backs up anything about to be destroyed or rewritten, with the manifest fsynced first.
It records a pending row.
It performs the mutation with create-before-delete staging.
It finishes `done` or `failed`.

Eight commands across the codebase follow this: `make_independent_copy`, `repair_skill_link`, `set_deployment_enabled`, `set_shared_harness_skill_enabled`, `materialize_harness_root`, `materialize_harness_root_then_disable`, `apply_skill_frontmatter_repair`, and `restore_skill_event` itself.
Everything else in the map — `set_harness_enabled`, `set_plugin_enabled`, `set_skill_invocation`, `write_installed_skill_md_if_unchanged`, `park_skill`, `unpark_skill`, `update_skill`, `remove_skill` — writes without a journal row.

`list_skill_events` (`event_commands.rs:65`) is a plain read, guarded by the event store's own mutex.
It returns up to `limit` (default 200) events with restorability flags.
It has no direct test.

`restore_skill_event` (`event_commands.rs:84`) is the one command every History Restore button calls.
It takes `ForkMutationLock`.
It refuses an explode restore while any exploded skill is still disabled, and it always refuses `make_independent_copy` with `force`.
Inside the store (`event_store.rs:378-472`) it backs up the current destination under `backups/<restore_id>/`, inserts a new pending restore event, applies the inverse (`RecreateSymlink`, `RemoveSymlink`, `MoveBack`, `RestoreBackup`, or the multi-path `UndistributeFromShared`), finishes the row, and claims `reverted_by` on the original row it restored.
A restore is itself journaled — restore-of-restore is legal, and `force` never destroys the only copy of anything, because step 2 preserves the drifted bytes in the restore event's own backup (`docs/spec-event-store.md:140-150`).
Duplicate or concurrent restores are blocked by a transactional claim: `UPDATE events SET reverted_by = ?new WHERE id = ?target AND reverted_by IS NULL`; zero rows updated means another restore already won (`docs/spec-event-store.md:126-131`).

Startup reconcile (`open_event_store` at `lib.rs:17`, called from setup at `lib.rs:130`) opens the database and flips every `pending` row to `interrupted`.
That is the only way a pending row can survive, since commands are synchronous within one process.
Three repairers then read the store and fix the filesystem: `reconcile_interrupted_independent_copy` and its restore counterpart, `reconcile_interrupted_convert_then_disable`, and `reconcile_interrupted_frontmatter_repair` (`lib.rs:46-98`).
Failures are printed, never fatal.

An `interrupted` row keeps its backup and renders in History with a warning and a Restore button.
Restore uses the recorded fingerprints to tell whether the mutation completed, partially applied, or never ran (`docs/spec-event-store.md:121-125`).
A drift-guard refusal — the live content no longer matches the fingerprint recorded in the original event's inverse — surfaces as a dialog offering force-restore, and states that the drifted content will itself be backed up (`docs/spec-event-store.md:170-172`).

Nothing in `SkillActivityView.tsx` calls a Tauri command directly; the heatmap and filters read the existing snapshot in memory.
`SkillHistorySection.tsx` is the component that calls `list_skill_events` and `restore_skill_event`.
A Restore success shows "Restored: {description}", a failure shows "Restore failed", and Reveal in Finder calls `open_skill_path(backup_path, "reveal")`.

## Changes in the Claude stack (#73 to #134)

PR #73 moved the event store itself — `event_store.rs`, the five-phase write path, and the reconcile logic — into `skill-studio-core`.
The desktop's `event_commands.rs` became a thin Tauri wrapper, and the new CLI and MCP server gained their own `restore`/events subcommands over the same core code.
`docs/spec-event-store.md` predates this split and still describes the pre-#73 module layout (`skills/event_store.rs`).
The schema, phases, and journal-kind list it documents did not change in the move, only their crate location.

## Changes in the Codex stack (#79 to #141)

PR #136, "Make Copy trial expiry and retained-backup Restore durable", routes Copy trial expiry through the shared core so it keeps a durable backup and removes the exact owned installation and its readers.
Activity and a notification can then restore that backup to Global as an untracked skill, including after the source project is unregistered.
An interrupted finalization settles the same event on restart.

PR #141, "Keep Activity History reads bounded and current", changes `list_skill_events`'s read path.
It moves onto a bounded background worker with at most two admitted reads; excess requests get `history_busy`.
The frontend shares identical pending requests, and results are invalidated when the snapshot changes or a restore finishes, so a stale read cannot overwrite current history.
Paging becomes 20 rows within the existing 200-event window.

PR #111, "show unresolved recovery status on Home", fixes a case this document's own current-state section implies is possible.
Home could show "All clear" while Activity still had an unresolved, interrupted-or-failed, unreverted operation.
The fix reads all history, not just the newest 200 rows, off the UI thread, and withholds the clear message while that read is loading or unavailable.

## Unit 1.2: the core Journal port

`skill-studio-core` now has a second, lower-level write-safety primitive alongside the SQLite event store above: the `Journal` port (`crates/skill-studio-core/src/ports.rs:627`) and its reference filesystem implementation, `FsJournal` (`crates/skill-studio-core/src/journal.rs:42`).
It journals `fsops`'s primitives (Stage, Swap, Link, WriteFile) directly, one row per `PlanRecord` (`ports.rs:602`) rather than per Tauri command, so a future `ops` function can compose several `fsops` calls into one plan and still get one journal row for the whole plan.

A plan is `Pending`, `Done`, `Failed`, `Reversed`, or `Interrupted` (`PlanStatus`, `ports.rs`).
`FsJournal::begin` writes the backup manifest, then the plan file, both through `ScopeFs` with an fsync-then-rename each, before the caller's first mutating step; the row starts `Pending`.
Each of `fsops`'s four primitives (`stage`, `swap`, `link`, `write_file`) now takes the open plan (a `&PlanWriter`) directly and records its own step _before_ it runs its own mutation, with everything reversal will need already computed and durable: `stage` records its temp path before creating it; `swap` records the exchange's chosen quarantine path (and the staged folder's pre-exchange device/inode, so reversal can later tell whether the exchange landed) before running it; `link` records the previous target (and the new one) before renaming the new link into place; `write_file` fsyncs its backup of the previous bytes before renaming the new ones into place. There is no separate `journaled_*` wrapper to call instead; a caller cannot run a primitive without a step landing for it (`crates/skill-studio-core/tests/architecture.rs` pins this).
The caller finishes the plan `Done` or `Failed` after the last step.

`reconcile` (`journal.rs`) is the startup pass: every plan still `Pending` is resolved.
A pending plan with no recorded steps means nothing was mutated yet, so it resolves `Failed`.
A pending plan with at least one recorded step means the process died mid-plan — and because every step is recorded before its mutation, that crash can only ever land between "recorded" and "mutation landed", never the other way around. `reconcile` inspects the disk for each step, in reverse order, to tell whether its mutation actually landed, and reverses only what did, through the same `ScopeFs`: stage's `remove_tree` is already a no-op when nothing landed; swap checks the live path's device/inode against the staged folder's pre-exchange identity, then exchanges the quarantined tree back (or, when the exchange landed but the follow-up move into quarantine did not, exchanges the staged path back and lets the paired stage step's own reversal clean it up) or removes what is at `path`; link checks whether the path is currently a symlink pointing at the new target before restoring (or removing) the previous one; write_file compares the live bytes against the backup before restoring them (or deleting the file if there was none). A fully reversed plan resolves `Reversed`; if reversal itself hits an I/O error (for example, its backup bytes are gone), the plan resolves `Interrupted` and is listed with that error for the caller to act on.
No plan is ever deleted.

`trim_backups` (`journal.rs`) enforces a `BackupQuota` (max total bytes and max age) by trimming the oldest backups first — age violations before size violations — using each plan's `created_at` as its backups' age, since a backup has no timestamp of its own.

The desktop's `EventStore` (`apps/desktop/src-tauri/src/skills/event_store.rs`) is now the host implementation of this port directly: `impl Journal for EventStore` delegates every method to the `FsJournal` it already owns (rooted at `<app_data>/journal` over `skill_studio_host::RealFs`), rather than reimplementing plan storage on SQLite — the `EventStore::journal()` getter this used to be exposed through is gone. `run()`'s startup `spawn_blocking` now calls `skill_studio_core::journal::reconcile` against `&store` too, right after the SQLite five-phase reconciliation (`reconcile_core_journal_at_startup`, `lib.rs`), holding the same root `WriteLease` every other mutating command takes.
This is still additive: the SQLite five-phase write path described above is untouched, and no existing command has been rerouted through this `Journal` yet — that adoption is a later slice, once `ops` functions exist to call `fsops`'s primitives from. Today's startup reconcile against it is a no-op in practice for that reason; it exists so a crash mid-write is already covered on the day a command does route through it.

## Desired state

The eight-command journal rule should become the only rule: every mutating command records a journal event with a backup and an inverse before it touches disk, not eight of roughly twenty.
`set_harness_enabled`, `set_plugin_enabled`, and `set_skill_invocation` are the highest-value gaps, since the map already flags them partial-state risk without journaling.
`park_skill`, `unpark_skill`, `update_skill`, and `remove_skill` are named directly in the map's cross-check as success toasts hiding an unjournaled partial-state risk.

`list_skill_events` and `restore_skill_event` already live in `skill-studio-core` after #73.
Keep new event-store logic there, rather than growing a second copy in the desktop crate, so the CLI, MCP server, and desktop stay on one write path.

`ForkMutationLock` guards `restore_skill_event` the same way it guards every command in enable-and-links.md.
It should become the same per-scope lease described there, scoped to the event's target skill or deployment, so a restore of skill A does not block a concurrent journaled write to skill B.

Backups accumulate under `backups/<event-id>/` with no retention limit in the current design.
A retention policy, age- or count-bounded, is needed before the store grows unbounded on an active install.
`restore_skill_event`'s displaced-bytes backups should count the same as originals under that policy.

The startup reconcile's three repairers are each a compensating step for one journaled command.
As more commands journal, each needs its own repairer, named per the pattern `reconcile_interrupted_<kind>`, so a crash window is always covered by name rather than by the catch-all `pending -> interrupted` flip alone.

Restore feedback should keep naming the partial state precisely, the way the drift-guard dialog already does: "the current (drifted) content will itself be backed up and restorable".
That is the model for the "errors never swallowed, partial results named" principle, and it should extend to `set_harness_enabled` and `set_skill_invocation` once they journal.

`list_skill_events` needs a direct test.
Today only the DTO policy test at `event_commands.rs:640` and the restore-of-restore tests inside `event_store.rs` exercise it indirectly.

## Gaps

- Only eight of roughly twenty mutating commands journal; `set_harness_enabled`, `set_plugin_enabled`, and `set_skill_invocation` are the ones this map already flags as partial-state risk without a journal.
- `park_skill`, `unpark_skill`, `update_skill`, and `remove_skill` give a success toast that the map's own cross-check calls an unjournaled partial-state risk.
- `ForkMutationLock` is one global lock, not a per-scope lease, for `restore_skill_event` and every journaled command.
- Backups under `backups/<event-id>/` have no retention limit.
- Startup reconcile has three named repairers for the commands that journal today; new journaled commands need their own repairer, not a shared catch-all.
- `list_skill_events` has no direct test.
- `docs/spec-event-store.md` describes the pre-#73 module path (`skills/event_store.rs`) and should be updated to name the `skill-studio-core` location.
- Activity itself calls no Tauri command; History (`SkillHistorySection.tsx`) is a separate component from Activity, so a user reading Activity does not see the same live event feed History offers unless they navigate there.
