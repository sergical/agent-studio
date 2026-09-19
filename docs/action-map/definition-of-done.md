# Definition of done

Done means every gap in this folder is closed and every check below is green in CI, on a fresh clone, with no single-thread flag.

Scope note, 2026-09-17: assistant runs (Ask, Audit, Test) and packs are deferred from the first release. The six checks below do not change; the per-area checklists for those two areas are not part of the first-release gate. The definition of done per unit of work (primitive, harness adapter, vertical slice, baseline) is in plan.md section 9.

## The six checks

### 1. Model test

- What it proves: a random sequence of operations, run against an in-memory model of the primitives, never leaves the disk in a state the model calls invalid.
- How it runs: a `proptest` or hand-rolled random-sequence test in `crates/skill-studio-core/tests/`, for example `cargo test -p skill-studio-core model_invariants`.
- What exists today: nothing. No file in `crates/skill-studio-core` or `crates/skill-studio-host` imports `proptest` or defines a random-sequence test.
- Pass condition: every run, for every seed CI picks, ends with the disk state matching the model's invariants.

### 2. Crash test

- What it proves: killing the executor after any step of any plan, then running recovery, always restores the disk invariants.
- How it runs: a per-step kill loop in `crates/skill-studio-core/tests/`, for example `cargo test -p skill-studio-core crash_recovery`, one test per plan type.
- What exists today: seven files mention "crash" (`repair_and_restore.rs`, `ports.rs`, `lib.rs`, `skill_materialize.rs`, `skill_independent_copy.rs`, `skill_frontmatter_repair.rs`, `event_store.rs`), but they cover single commands by hand, not a per-step kill loop over a journaled plan. `make_skill_independent_copy` is the only command with a named crash-recovery suite.
- Pass condition: for every step k of every plan, killing after step k and running recovery leaves no invariant violation.

### 3. Contention test

- What it proves: a real second process writing the same root does not corrupt it and does not deadlock the first process.
- How it runs: an integration test that spawns a second OS process against the same root, for example `cargo test -p skill-studio-core contention_two_process -- --test-threads=1` is not allowed; the test itself manages the second process, the outer run stays parallel.
- What exists today: nothing. No file matches "second process" or "contention" in `crates/skill-studio-core` or `apps/desktop/src-tauri`.
- Pass condition: both processes finish, the lease serializes their writes, and no journal entry is left half-written.

### 4. Parity test with the nine CLI traces

- What it proves: our own install, update, and remove code produces the same folders, links, and lockfile entry as `npx skills` for the same input.
- How it runs: `cargo test -p skill-studio-core cli_parity`, one test per trace, replaying the recorded trace against our core and diffing the result tree against the CLI's recorded result tree.
- What exists today: nothing recorded yet. `apps/desktop/src-tauri/tests/core_scan_parity.rs`, `restore_parity.rs`, `fingerprint_parity.rs`, and `content_facts_parity.rs` check scan and read parity today, not install, update, or remove parity, and no trace file exists on disk.
- Pass condition: for each of the nine traces, our result tree and lockfile entry match the CLI's byte for byte, apart from timestamps.

### 5. Golden snapshots

- What it proves: a scan of a fixed fixture estate returns the same snapshot every time, so a scan regression shows as a diff, not a surprise.
- How it runs: `cargo test -p skill-studio-core --test scan_golden`.
- What exists today: `crates/skill-studio-core/tests/scan_golden.rs` (1 test), plus `diagnosis_golden.rs` (1 test) and `diagnose_next_action_agreement.rs` (1 test) cover diagnosis, not the full mutation surface this slice adds.
- Pass condition: the golden file byte-matches the fresh scan output; any diff fails the test and needs a reviewed golden update.

### 6. Doctor pass

- What it proves: a read-only pass over a real home directory finds every invariant violation before any change lands, so we know the starting state.
- How it runs: a CLI subcommand or test binary, for example `cargo run -p skill-studio-cli -- doctor --home <path>`, exit code non-zero on any violation.
- What exists today: `crates/skill-studio-core/src/doctor.rs` checks the six invariants from `lifecycle-states.md`; `ops::fix_skill` wires invariants 1-5 (6 stays startup-only). No `skill-studio doctor` CLI subcommand runs the pass on its own yet.
- Pass condition: the pass completes without a panic and reports zero unexplained violations against a known-good home.

CI runs clippy with default and all features, and runs tests in parallel. No single-thread flag on any of the six checks above.

## Budgets

| Budget                                              | Measurement method                                                                                                                                  | Where recorded                                                                                                 |
| --------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| Full scan under 100 ms without hashing              | A benchmark test times `snapshot(root)` against the golden fixture estate and asserts the median is under 100 ms.                                   | `crates/skill-studio-core/benches/` (does not exist yet; slice 7 adds it), result checked into CI job output.  |
| Any local mutation under 50 ms                      | A benchmark test times one plan execution (park, unpark, a single-file write) end to end and asserts the median is under 50 ms.                     | Same bench crate as above; result checked into CI job output.                                                  |
| The UI event within one frame of the journal commit | A desktop integration test measures the delay between `journal.done` and the `skill-agent://event` or snapshot-refresh event reaching the frontend. | `apps/desktop/src-tauri/tests/` (test does not exist yet; slice 7 adds it), result checked into CI job output. |

## Done criteria per area

Each item names a command, file, or grep. Items that repeat the same fix across areas (a journal, a shared lease, a shared core crate, a retention limit) are collected once in the shared-state subsection and marked "shared" in the area they came from.

### install

- [ ] `add_skill` (Copy, Dotagents, SkillsSh) records a journal event with a backup and an inverse before writing (shared: journal-every-write).
- [ ] Copy, Dotagents, and SkillsSh write paths move from `apps/desktop/src-tauri` into `skill-studio-core` (shared: one-write-path-in-core).
- [ ] `add_skill`'s install lease is per-scope, not the global `ForkMutationLock` (shared: per-scope-lease).
- [ ] `start_add_skill_operation`'s fingerprint-write-reconcile chain is one transaction, or `reconcile_affected`'s error path does a targeted compensating step instead of a full rebuild.
- [ ] `SkillStoreInstallFlow`'s Install Skill button shows the trust prompt before calling `add_skill`.
- [ ] "Promote to global" and "Install again" go through the operation state machine, with cancel, progress, and a trust prompt.
- [ ] `cancel_add_skill_operation` cannot report "cancelled" for a commit that already landed; a test proves it.
- [ ] Backups and event rows under `<app_data>/backups/<event-id>/` and `events.sqlite3` have a retention limit (shared: retention-limit).
- [ ] `add_skills` and `is_skill_installed` either get a frontend caller or are removed (shared: no-command-without-caller).
- [ ] `add_skill` (Dotagents) and `add_skill` (SkillsSh) each get a direct crash-window test, matching what Copy already has.

### remove-and-update

- [ ] `remove_skill`'s four variants and both `update_skill` variants record a journal event (shared: journal-every-write).
- [ ] The four remove implementations (Copy, dotagents, Fork, skills-sh) share one core implementation instead of duplicating stage-verify-commit-rollback (shared: one-write-path-in-core).
- [ ] Remove, update, park, fork, and pull each take a per-scope lease, not the shared `ForkMutationLock` (shared: per-scope-lease).
- [ ] skills-sh remove's trial-drop failure after a successful CLI removal shows a user-visible warning, not a print only.
- [ ] dotagents remove and Copy remove retry a failed rollback automatically instead of leaving staged links or paths in `skills-trash`.
- [ ] Fork remove's registry-write rollback failure names the backup path to the user, not just an error string.
- [ ] `update_skill`'s CLI-then-check-now-then-rebuild sequence is one transaction, or the check-now failure path has a compensating step.
- [ ] `check_skill_updates_now` gets a frontend caller, or the six-hour loop stays the documented only trigger and the map says so.
- [ ] The header Remove button's success toast reads "Removed," not "Updated N deployments."
- [ ] `restore_trashed_skill` cleans up a half-written target and the trash copy when the entry-count check fails; a test proves it.

### park-fork-trial

- [ ] `park_skill`, `unpark_skill`, `fork_skill`, `pull_fork_upstream`, and `unfork_skill` each record a journal event, matching `make_skill_independent_copy` (shared: journal-every-write).
- [ ] `park_skill` and `unpark_skill`'s rollback path checks the result of every recovery write instead of discarding it with `let _ =`.
- [ ] `fork_skill`'s restore-after-detach failure is retried or automatically repaired, not just reported.
- [ ] `pull_fork_upstream`'s four-step rename swap is one transaction; a crash-window test between any two renames passes.
- [ ] `unfork_skill`'s registry write has a compensating step if it fails after the CLI reinstall already discarded local edits.
- [ ] The six commands in this area take a per-skill or per-scope lease, not the shared `ForkMutationLock` (shared: per-scope-lease).
- [ ] Each command's registry read-modify-write is atomic with its own lease boundary, not just wrapped by the whole-call lock.
- [ ] The trial expiry loop records a journal event and gets a crash-window test, matching the foreground commands.
- [ ] `skills-trash` and fork snapshot directories have a retention limit; the app deletes old entries outside trial expiry too (shared: retention-limit).
- [ ] `keep_skill_trial`, `park_skill`, `unpark_skill`, `fork_skill`, `pull_fork_upstream`, and `unfork_skill` each get a crash-window test, matching `make_independent_copy`'s suite.

### enable-and-links

- [ ] `set_harness_enabled` records a journal event covering the Codex multi-path write and the Claude link swap (shared: journal-every-write).
- [ ] `set_plugin_enabled` records a journal event, so a mid-CLI-call crash leaves a Skill Studio trace, not just Claude Code's own config.
- [ ] The nine commands in this area take a per-scope lease, not the single global `ForkMutationLock` (shared: per-scope-lease).
- [ ] `set_harness_enabled`'s Codex multi-path write is one transaction, or has a compensating step for a partial toggle.
- [ ] `materialize_harness_root`'s remove-then-rename window cannot strand the root in neither state; a test proves the rollback always succeeds or reports clearly.
- [ ] Backups and staging directories from `make_skill_independent_copy` and `materialize_harness_root` have a stated retention limit (shared: retention-limit).
- [ ] `set_harness_enabled` and the flagged read commands in this area get a direct test.
- [ ] `set_shared_harness_skill_enabled` gets a frontend caller, or is removed (shared: no-command-without-caller).
- [ ] A `set_skill_visibility` command exists, so pre-install visibility (`UniversalVisibilitySelector`) and post-install `enabled` state are one mechanism, not two.
- [ ] Failure toasts such as "Couldn't enable/disable" name which step and which path failed.

### skill-md-editing

- [ ] `write_installed_skill_md_if_unchanged` records a journal event with a backup and an inverse (shared: journal-every-write).
- [ ] `set_skill_invocation`'s frontmatter write and Codex sidecar write share one lock and one journal event, so a crash between them is repairable.
- [ ] `write_installed_skill_md_if_unchanged` gets a direct test.
- [ ] A crash-window test covers the gap between `set_skill_invocation`'s frontmatter write and its sidecar write.
- [ ] `write_installed_skill_md` gets a frontend caller, or is removed (shared: no-command-without-caller).
- [ ] `SkillMarkdownCard.tsx`'s Save flow rolls back the fork if the write step fails.
- [ ] `SKILL_MD_WRITE_LOCK` becomes a per-scope lease (shared: per-scope-lease).
- [ ] `write_installed_skill_md_if_unchanged` and `set_skill_invocation` move into `skill-studio-core`, matching `apply_skill_frontmatter_repair` (shared: one-write-path-in-core).
- [ ] `get_skill_details`, `read_installed_skill_md`, and `preview_skill_frontmatter_repair` each get a direct test.

### events-and-history

- [ ] Every mutating command journals, not just the current eight; `set_harness_enabled`, `set_plugin_enabled`, and `set_skill_invocation` close first (shared: journal-every-write).
- [ ] `park_skill`, `unpark_skill`, `update_skill`, and `remove_skill`'s success toast only fires after the journal confirms the write, closing the partial-state risk the map already names.
- [ ] `restore_skill_event` and every journaled command take a per-scope lease, not the shared `ForkMutationLock` (shared: per-scope-lease).
- [ ] Backups under `backups/<event-id>/` have a retention limit (shared: retention-limit).
- [ ] Startup reconcile has a named repairer for every journaled command, not a shared catch-all covering only the current three.
- [ ] `list_skill_events` gets a direct test.
- [ ] `docs/spec-event-store.md` names the `skill-studio-core` location, not the pre-#73 `skills/event_store.rs` path.
- [ ] Activity and History read from the same live event feed, or the map records the two as intentionally separate.

### packs

- [ ] `create_skill_pack` records a journal event with an inverse; a git failure after tree build does not leave an orphan directory (shared: journal-every-write).
- [ ] `update_skill_pack` records a journal event; a late git failure rolls back the tree instead of leaving it half-rewritten.
- [ ] `publish_skill_pack` compensates or warns the user when a registry write fails after `gh repo create` succeeds.
- [ ] `delete_skill_pack` shows a success toast and compensates when a registry write fails after directory removal.
- [ ] `import_skill_pack` and `confirm_skill_pack_trust` clean up CLI partial writes on a mid-install failure, with a journal event.
- [ ] The four packs.rs-native write commands take a per-pack lease, not the single global `ForkMutationLock` (shared: per-scope-lease).
- [ ] The four packs.rs-native write commands move into `skill-studio-core` (shared: one-write-path-in-core).
- [ ] `import_skill_pack` and `confirm_skill_pack_trust`'s named tests are identified in the source map by file and line.
- [ ] `publish_skill_pack`'s backend `private` visibility option gets a UI path in `PacksView`.

### assistant-runs

- [ ] `create_skill_scratch_dir` gets a journal event and a direct test; a mid-copy failure does not leave a partial scratch dir (shared: journal-every-write).
- [ ] `remove_skill_scratch_dir`'s failure surfaces to the user with a retry path, not a swallowed error.
- [ ] `prepare_skill_run_target` (Worktree) records a journal event; a late failure does not leave an orphan git worktree.
- [ ] `apply_skill_run_target_diff`'s three-step chain (patch write, apply, worktree removal) is one transaction, or each step has a compensating step.
- [ ] `discard_skill_run_target` (InPlace) reverts against a stored baseline, not live git status, in one grouped step.
- [ ] `record_skill_run` takes a lock around the trim step; a write failure between `.json` and `.events.jsonl` does not leave a half pair.
- [ ] `list_skill_runs` and `read_skill_run_events` surface their errors in the UI instead of showing an empty list indistinguishable from "no runs yet."
- [ ] An InPlace target is discarded on panel unmount; a Worktree target's "Keep" state and a late `skill-agent://event` after unmount are both handled, not lost.
- [ ] Runs and targets each take a per-skill or per-target lease, not a single mutex (shared: per-scope-lease).
- [ ] The logic in `skill_agent_runner.rs` and its sibling files moves into a shared core crate reusable by the CLI and MCP server (shared: one-write-path-in-core).
- [ ] `skill_run_target_diff`'s `git add -N .` index reset gets a direct test and a journal entry confirming the reset happened.

### settings-and-projects

- [ ] Every project-folder and editor/key writer records a journal event with a backup and an inverse (shared: journal-every-write).
- [ ] `update_registry_section` and `set_preferred_editor` / `set_skills_sh_api_key`'s registry read-modify-write takes a lock, so a concurrent writer cannot lose its change (shared: per-scope-lease).
- [ ] `list_project_folders` caches its harness-history rescan instead of rescanning on every call.
- [ ] `get_discovery_sources` and `list_project_folders` each get a direct test named in the map.
- [ ] `register_skill_projects`'s home-directory drop is a named partial result shown to the user, not only `eprintln!`.
- [ ] `get_preferred_editor` gets a frontend wrapper, or is removed in favor of `get_editor_choices` (shared: no-command-without-caller).
- [ ] `get_skills_sh_access` and `list_installed_editors` each get a direct test named in the map.

### reads-and-snapshot

- [ ] A scan failure keeps the last good snapshot instead of publishing an empty one (`skill_refresh.rs:1398-1404`, `:517-551`).
- [ ] `get_agent_targets` and `list_skill_projects` get a frontend caller, or are removed (shared: no-command-without-caller).
- [ ] `is_skill_installed` gets a frontend caller, or is removed (shared: no-command-without-caller).
- [ ] `request_skill_rescan`'s errors from the triggered rebuild surface to the user, not just a spinner that stops.
- [ ] `get_skill_snapshot`, `get_popular_skills`, `search_skills`, `get_skill_details`, `list_github_skills`, `read_installed_skill_md`, and the skill-use parsers' Tauri-facing read each get a direct test.
- [ ] The refresh loop, the update-check loop, and the invocation reconcile expose a last-run and last-error state for a settings or diagnostics view.

### shared-state

This area collects the fixes that repeat across every other area above. Closing these here closes the matching item in every area file that references it.

- [ ] journal-every-write: every write records a journal event with a backup and an inverse before it touches disk. Today only eight commands do, covered by `apps/desktop/src-tauri/src/skills/event_store.rs` (8 tests).
- [ ] per-scope-lease: `crates/skill-studio-host/src/lease.rs`'s `FileLease` (already used by `scan`) replaces `ForkMutationLock` (`skill_fork.rs:949`) and `SKILL_MD_WRITE_LOCK` (`skill_md_write.rs:18`), both single global `Mutex<()>` locks, for every mutation command.
- [ ] one-write-path-in-core: every write implementation listed above moves from `apps/desktop/src-tauri` into `skill-studio-core`, so the CLI and MCP server can reuse it.
- [ ] retention-limit: `<app_data>/backups/<event-id>/` and `<app_data>/events.sqlite3` each have a size cap or an age cap; a test asserts old entries are pruned.
- [ ] no-command-without-caller: `grep -rn '#\[tauri::command\]' apps/desktop/src-tauri/src/skills/commands.rs` cross-checked against every frontend `invoke()` call returns zero commands with no caller.
- [ ] The five background loops (refresh, update check, trial expiry, startup reconcile, pack staging reconcile) each expose a last-run and last-error state and can be cancelled independently of the app.
- [ ] `docs/spec-core-primitives.md` D9's rejected-alternative note (the global `ForkMutationLock`) is deleted once the lease lands, not left describing a still-shipping design.

### harness-integration

`harness-integration.md` covers what Claude Code, Codex, OpenCode, pi, and the shared root each expect on disk, and how Ask, Audit, and Test runs drive each harness. Done for this area means:

- [ ] `skill_agent_runner.rs`, `skill_run_target.rs`, `skill_harness_disable.rs`, `skill_materialize.rs`, and `skill_park.rs` read harness paths, link targets, and CLI argv from `HarnessCatalog`; a grep for a hard-coded `.claude/skills` or `.codex/skills` string outside `harness.rs` and its tests returns zero.
- [ ] One conformance test runs every `HarnessCatalog` row through install, disable, park, unpark, and run, and passes for Claude Code, Codex, OpenCode, pi, and the shared root.
- [ ] `parked-but-reinstalled` is reconciled by the startup pass, not only flagged; a test parks a skill, recreates the shared folder, restarts, and asserts one folder in one state.
- [ ] Provenance classification has one implementation, in the core crate, and `skill_ownership.rs` calls it; the desktop, CLI, and MCP adapters return the same kind for the same folder in a parity test.

## Done criteria per slice

| Slice | Days | What lands                                                                  | Checks green at the end                                               | Area files it closes                                                           |
| ----- | ---- | --------------------------------------------------------------------------- | --------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| 0     | 1    | Nine CLI traces and the behavior contract                                   | none of the six yet; the traces are recorded for check 4 to use later | none                                                                           |
| 1     | 4    | Primitives crate with unit and crash tests                                  | model test, crash test                                                | shared-state (per-scope-lease, one-write-path-in-core groundwork)              |
| 2     | 2    | Scan through Snapshot, parity with current scan, swap the desktop read path | golden snapshots                                                      | reads-and-snapshot                                                             |
| 3     | 2    | First mutation end to end: park and unpark with journal, recovery, and UI   | crash test (park/unpark plan)                                         | park-fork-trial (park_skill, unpark_skill items)                               |
| 4     | 4    | Remove, then install with own fetch, tree hash, and lockfile, parity vs CLI | parity test (traces 1-5, 8-9), crash test (remove/install plans)      | install, remove-and-update                                                     |
| 5     | 5    | Update, fork, pull, unfork, invocation edit, harness disable as plans       | parity test (traces 6-7), crash test (all remaining plans)            | park-fork-trial (fork, pull, unfork items), skill-md-editing, enable-and-links |
| 6     | 2    | Delete old desktop mutation code, port CLI and MCP adapters                 | contention test                                                       | shared-state (one-write-path-in-core, closed), assistant-runs, packs           |
| 7     | 2    | Benchmark, doctor, and CI                                                   | doctor pass, all budgets measured                                     | events-and-history, settings-and-projects                                      |

## What is out of scope

- Packs stay behind the `skill-packs` flag; this work does not turn the flag on.
- The assistant runs (Ask, Audit, Test) keep their current runner process model; this work journals and leases the run-target and scratch-dir writes but does not rewrite the harness spawn path.
- The skills.sh proxy server (`apps/server`) is untouched; it holds the API key and stays a separate Node process.
- The marketing site (`packages/marketing`) is untouched.
- The CLI (`apps/cli`) and MCP server (`apps/mcp`) adapters gain the new core write path in slice 6, but their own command surfaces are not redesigned here.
- `docs/spec-core-primitives.md` section 6.3's list of not-yet-moved write commands sets the scope for this work; nothing outside that list is added mid-project.

## How we know the map is still true

The map is hand-read; nothing in the build checks it against the code. A CI step should diff the list of `#[tauri::command]` names in `apps/desktop/src-tauri/src/skills/commands.rs` (and any other file carrying the attribute) against the command names cited in this folder's area files, and fail the build on a mismatch: `grep -rn '#\[tauri::command\]' -A1 apps/desktop/src-tauri/src/skills/*.rs | grep -oP 'pub fn \K\w+'` gives the live list to compare. Until that step exists, each pull request that adds, removes, or reorders a command's writes must edit the matching area file in the same commit, and review is the only backstop.
