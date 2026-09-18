# Enable and links

This area turns a skill deployment on or off, per harness or per reader.
It also repairs or converts the symlinks that carry a skill to disk.

Commands: `set_deployment_enabled`, `set_harness_enabled`, `set_reader_enabled`, `set_plugin_enabled`, `materialize_harness_root`, `materialize_harness_root_then_disable`, `repair_skill_link`, `make_skill_independent_copy`.
There is no `set_skill_visibility` command.
The closest control, `UniversalVisibilitySelector` in the Add Skill sheet, only holds local state until `add_skill` or `set_harness_enabled` runs.
See Gaps for what this gap means.

UI entry points: `SkillLocationsCard.tsx` (enabled switch per row), `SkillLocationMenu.tsx` (Relink, Remove link, Convert to per-skill links, Make independent copy, Enable/Disable the plugin), `MaterializeRootDialog.tsx`, `MakeIndependentCopyDialog.tsx`, `SkillRepairCard.tsx`.

## Current state

`set_deployment_enabled` (`skill_harness_disable.rs:811`) moves a deployment into or out of `<root>/.skill-studio-disabled/`.
For a Copy owner, it moves the Copy's directory and rewrites the copies map instead.
It takes `ForkMutationLock`, records a pending `move_aside_disable` or `move_aside_restore` event, does the move, and finishes the event.
It refuses a shared Universal deployment with "Park the skill instead", a plugin target, and a destination that already exists.
On a Copy-path registry failure it re-inserts the record and reverses the move.
The symlink case creates the new link before removing the old one.
It journals with a `MoveBack` inverse, so it is one of the eight write commands with a durable backup and inverse (`docs/spec-event-store.md:75-83`).
Tests at `skill_harness_disable.rs:1497-1731` cover the round trip, the shared-root refusal, and registry-failure rollback.

`set_harness_enabled` (`skill_harness_disable.rs:741`) is a different code path for the default per-harness toggle.
Codex writes `~/.codex/config.toml`.
OpenCode writes `~/.config/opencode/opencode.json`.
Claude Code writes a `harness_disabled` bucket in the registry, plus removes or recreates `~/.claude/skills/<name>`.
It holds `ForkMutationLock` only, and does **not** journal.
The map marks it partial-state risk: the Codex loop touches several `SKILL.md` paths with no cross-path transaction, so a crash can leave some toggled and some not.
A Claude Code registry failure recreates the removed link, or, when the slot is taken, reports "recreate manually" — a failure the user must fix by hand.
Sixteen tests cover it at `skill_harness_disable.rs:1023-1487`.

**Update (unit 3.8, issue #166, 2026-09-17):** the Tauri `set_harness_enabled` command is now a thin adapter over `skill-studio-core`'s `ops::set_harness_enabled` (`skill_harness_disable.rs:695`), the same shape `set_deployment_enabled`'s core-backed siblings use.
It parses the skill name out of `HarnessVisibilityTarget.deployment_id` and calls the core op directly; the wire contract (`HarnessVisibilityTarget` in, `Result<(), String>` out) is unchanged, so no frontend file needed to move.
The core op journals before the first write - a `HarnessEnable`/`HarnessDisable` event with a `SymlinkInverse` (Claude Code) or `RestoreBackupInverse` (Codex, OpenCode) - closing the "does not journal" gap below for this command specifically. What "restorable" means depends on how the row finished: a `Done` row restores normally. A `Failed`/`Interrupted` row - a crash partway through the Codex loop, say - restores too, but only when it carries both a `RestoreBackupInverse` and a `backup_dir` (a Claude Code `SymlinkInverse` row never does, so it stays unrestorable if the link write itself never lands); `restore` without `force` returns `DriftConflict` whenever the live file no longer matches what the inverse's `post_fingerprint` recorded, which a partial Codex write always trips, so `force` is required in practice. A `Pending` row is never restorable while it reads back pending - the next `MutationSession` promotes an abandoned one to `Interrupted` before anything can restore it.
Codex's multi-path loop still reports a partial toggle as "N of M" rather than rolling back (see Gaps), which the core's own module doc treats as an intentional narrowing versus a full cross-path transaction.
The CLI gained `skill-studio set-harness-enabled` and a general `skill-studio undo` (reverts the newest restorable event, across skills and write kinds) over the same core op; `set_deployment_enabled` and `set_plugin_enabled` were left as they were - see Gaps.

`set_reader_enabled` is not a Tauri command.
It is the frontend action name (`skill-location-actions.ts:209`) for the reader-toggle branch of one reducer.
Both branches of that reducer call `setHarnessEnabled`, which invokes the backend `set_harness_enabled` (`skill-location-actions.ts:194-211`).
So the "reader toggle" and the "harness toggle" share one backend command and its risk profile above.

`set_plugin_enabled` (`commands.rs:2528`, backed by `skill_plugin_lifecycle.rs:90`) runs `claude plugin disable|enable <plugin_id> -s user` for Claude Code only.
It checks the harness, takes `ForkMutationLock` around the CLI call, then requests a snapshot rebuild (`commands.rs:2511-2522`).
It applies to every skill the plugin ships, because Claude Code tracks `enabledPlugins` per plugin, not per skill.
It does not journal: the source of truth is Claude Code's own config file, not the Skill Studio event log.
Tests exist at `skill_plugin_lifecycle.rs:140-183`.

Unit 4.1 removed `set_shared_harness_skill_enabled`, which had no frontend caller; the UI reached its effect only indirectly, through the materialize dialog.
Toggling one per-skill link under an already-materialized root (enable relinks, disable unlinks) still goes through `skill_materialize::relink_harness`/`unlink_harness` - it just no longer has a standalone command.

`materialize_harness_root` (`event_commands.rs:273`) converts a whole-directory symlink into a real directory of per-skill links.
It is triggered from `MaterializeRootDialog.tsx:54`.
It builds the replacement at a staging path, records an `explode_shared_dir` event with a staged fingerprint, removes the original symlink, renames staging into place, then registers the root.
The map calls out a partial-state risk in the window between removing the symlink and renaming the directory in: a failed best-effort rollback can leave the root in neither state.
It journals with a `RecreateSymlink` inverse.
`materialize_harness_root_then_disable` (`event_commands.rs:314`) composes the same conversion with one link removal, as a single recorded operation.
A startup reconciler, `reconcile_interrupted_convert_then_disable`, handles a crash between the two phases.

`repair_skill_link` (`event_commands.rs:529`) relinks or removes a broken symlink found by `SkillRepairCard.tsx`.
It is a single syscall per action, delete or recreate.
It is recorded, journaled, and restorable.

`make_skill_independent_copy` (`event_commands.rs:390` → `skill_independent_copy.rs:81`) turns a link into an owned directory copy.
It is the longest write chain in this area: event recorded, optional whole-root explode, copy to a staging directory with a fingerprint check, event patched with a `RecreateSymlink` inverse, link renamed aside, staging renamed in, copies entry written, event finished, guards removed (`skill_independent_copy.rs:138-353`).
Every step has an explicit rollback closure.
A crash mid-copy is repaired at startup by `reconcile_interrupted_independent_copy`, using the recorded phase and fingerprints.
Five crash-recovery tests exist at `skill_independent_copy.rs:1962-2202`.

On success the switch moves, a row updates, or a dialog closes.
The map lists no confirming toast for most of these, besides "Re-linked to {path}" and "Removed broken link".
On failure the user sees a generic toast such as "Couldn't enable/disable", "Couldn't relink", or "Couldn't convert", with no detail about which step failed.

## Changes in the Claude stack (#73 to #134)

PR #73 built the shared `skill-studio-core` crate that this whole area now sits on.
Scan, lifecycle, and event-store code moved out of the Tauri backend into the core crate, with `skill-studio-host` as the real-world adapter, plus a CLI, an MCP server, and generated TypeScript types.

PR #70 is a bug fix on top of that split.
Editing the universal row of a symlink-shared Codex skill left an orphaned `agents/openai.yaml` sidecar, because sidecar reconciliation was gated on `deployment.agent == "Codex"` only.
The fix reconciles the sidecar that actually exists in the canonicalized skill directory.
It covers the independent-copies topology, so it does not over-clear a sidecar a sibling Copy still needs, and it adds three regression tests.

Neither PR changes the enable or link commands' write order, locking, or journaling.
PR #73 is the architectural move; PR #70 is a one-file correctness fix for invocation sidecars, which borders this area but belongs to skill-md-editing.

## Changes in the Codex stack (#79 to #141)

PR #135, "Make Copy visibility reversible and recoverable", adds an Enable/Disable pair for an independent Copy that runs through the shared core, off the UI thread.
It ships a guarded inverse in Activity and a startup settler for interrupted moves that does not overwrite conflicting external edits.
It explicitly withdraws the legacy Copy move-aside restore path, including with force, in favor of this new visibility control.
The PR claims 71 core tests, 22 visibility-adapter tests, and native acceptance of global reversal, stale-content refusal, and recovery after an injected failure.
This replaces the ad hoc rollback closures the Claude-stack `set_deployment_enabled` Copy path hand-writes with one durable, tested command.

## Desired state

`set_deployment_enabled`, `materialize_harness_root`, `materialize_harness_root_then_disable`, `repair_skill_link`, and `make_skill_independent_copy` already journal.
Keep their shape as the model.
`set_harness_enabled` and `set_plugin_enabled` should move to the same shape: a journal event with a backup and an inverse recorded before any file or config write.
That lets a Codex multi-path loop or a Claude Code link swap be replayed or rolled back the same way `move_aside_disable` is today.

All eight commands should share one write path in `skill-studio-core`, with the Tauri command in `commands.rs` or `event_commands.rs` reduced to argument parsing and DTO mapping.
Right now the logic lives directly in `skill_harness_disable.rs`, `skill_plugin_lifecycle.rs`, `skill_materialize.rs`, and `skill_independent_copy.rs`, behind the Tauri layer.
A CLI or MCP caller of the same operation should not have to re-implement the precondition checks.

`ForkMutationLock` is one global try-acquire lock shared by every command in this file.
It should become a per-scope lease keyed by skill id or deployment id, so relinking skill A does not block a concurrent visibility change to skill B.
The registry read-modify-write inside the Copy-path branches of `set_deployment_enabled` and `make_skill_independent_copy` should happen under that same lease, not as a separate unlocked step.

`set_harness_enabled`'s Codex loop over several `SKILL.md` paths needs either one transaction or a named compensating step per path.
That way a crash mid-loop is reported as "3 of 5 skills toggled", rather than left silent.
`materialize_harness_root`'s window between symlink removal and directory rename needs the same treatment.
Today's best-effort rollback is exactly the gap this principle closes.

Every command here should report success only after its last step.
A partial failure — Claude Code's "recreate manually" case, or a Codex loop that toggled some but not all paths — should name the affected paths to the user, instead of collapsing into one generic "Couldn't enable/disable" toast.

Backups already carry a manifest and fingerprint (`docs/spec-event-store.md:32-36`).
They need an explicit retention limit, since `make_skill_independent_copy` and `materialize_harness_root` both leave staging and backup directories behind on the happy path.

Every command needs a direct test and a crash-window test.
Several commands, like `set_harness_enabled`, note "no direct test" or rely on policy tests today; `make_skill_independent_copy` already has five crash-window tests and should be the model.

## Gaps

- `set_harness_enabled` does not journal; a crash mid-Codex-loop or mid-Claude-link-swap is not recorded or restorable. **Resolved by unit 3.8** — it now runs through `ops::set_harness_enabled` and journals with an inverse.
- `set_plugin_enabled` does not journal; Claude Code's own config is the only record, so a mid-CLI-call crash leaves no Skill Studio trace.
- `ForkMutationLock` is one global lock for all nine commands; no per-scope lease exists yet.
- `set_harness_enabled`'s Codex multi-path write still has no transaction or compensating step; a partial toggle is possible and reported as "N of M", not rolled back (unchanged by unit 3.8; see the core's `harness_switch.rs` module doc for the scope this narrows).
- `materialize_harness_root`'s remove-then-rename window can strand the root in neither state if the best-effort rollback itself fails.
- Backups and staging directories from `make_skill_independent_copy` and `materialize_harness_root` have no stated retention limit.
- `set_harness_enabled` and several read commands in this area have "no direct test" per the map; only policy or primitive tests exist.
- `set_shared_harness_skill_enabled` has no frontend caller.
- There is no `set_skill_visibility` command; `UniversalVisibilitySelector` is local-only until an Add Skill submit, so "visibility" before install and "enabled" after install are two different, disconnected mechanisms.
- Failure toasts, such as "Couldn't enable/disable" or "Couldn't convert", do not distinguish which step or which path failed.
