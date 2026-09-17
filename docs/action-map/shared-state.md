# Shared state on disk

This area covers every file the app writes, who writes each one, and what stops two writers from colliding.

Files: `~/.agents/skill-studio.json` (registry), `~/.agents/.skill-lock.json` (lock file, not written by the app), `<app_data>/events.sqlite3` and its backups, `~/.agents/skills-trash/`, staging under `~/.agents`, `<app_data>/skill-studio/` scratch and run directories, per-harness sidecars (`~/.codex/config.toml`, `~/.config/opencode/opencode.json`), `~/.agents/skills-parked/`.

UI entry points: none directly. Every control that ends in a write, park, unpark, fork, install, remove, editor and API-key settings, trial expiry, harness disable, is covered in the lifecycle, install, and edit-and-enable sections of the source map, not repeated here.

## Current state

| Path                                                                                    | Writer                                                                                            | Holds                                                                                                                                                  |
| --------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `~/.agents/skill-studio.json`                                                           | `write_fork_registry` (`skill_fork_registry.rs:372`), tmp plus rename, unique tmp name per writer | forks, trials, parked, harness_disabled, packs, copies, `skills_sh_api_key`, `server_url`, `preferred_editor`, `trusted_dotagents_sources`, `projects` |
| `~/.agents/.skill-lock.json`, `agents.toml`, `agents.lock`                              | never written by this app                                                                         | Owned by the `npx skills` and dotagents CLIs; the app only reads them                                                                                  |
| `<app_data>/events.sqlite3`                                                             | SQLite WAL, `rusqlite` transactions                                                               | events, materialized_roots, materialized_disabled tables                                                                                               |
| `<app_data>/backups/<event-id>/`                                                        | files written in place; only `manifest.json` is fsynced                                           | one numbered copy per backed-up path, plus a manifest                                                                                                  |
| `~/.agents/skills-trash/`, `~/.agents/skills-parked/`, `<root>/.skill-studio-disabled/` | rename or copy                                                                                    | quarantine and park locations                                                                                                                          |
| `~/.codex/config.toml`, `~/.config/opencode/opencode.json`                              | tmp plus rename, no fsync                                                                         | native per-skill disable                                                                                                                               |

The registry is the busiest shared file.
Every writer goes through `write_fork_registry`, so each individual write is atomic: the rename either lands whole or not at all.
But the read-modify-write around that write is not locked.
`set_preferred_editor` (`commands.rs:2391`, `skill_editor.rs:75`) and `set_skills_sh_api_key` (`commands.rs:118`) are both marked "partial-state risk" in the source map.
Two concurrent writers can each read the same old registry, change a different field, and the second rename wins.
The first writer's change is lost, not corrupted; there is no crash, just a silently dropped edit.

`<app_data>/events.sqlite3` is SQLite in WAL mode, with foreign keys off, written through `rusqlite` transactions.
Event kinds are free strings: `restore`, `repair_skill_frontmatter`, `make_independent_copy`, `explode_shared_dir`, and others.
There is no retention policy and no size cap.
`<app_data>/backups/<event-id>/` mirrors this problem: one numbered copy per backed-up path, plus a `manifest.json` that is the only fsynced file in the directory.
Nothing in the app ever deletes an old backup.

Two locks guard mutation today, and both are process-global `Mutex<()>` values.
`ForkMutationLock` (`skill_fork.rs:949`, `try_acquire` at `:953`) gates most lifecycle writes: fork, park, unpark, restore, remove, editor and API-key settings, trial expiry.
`SKILL_MD_WRITE_LOCK` (`skill_md_write.rs:18`) separately serializes editor-style SKILL.md read-modify-write.
Neither lock is reentrant.
Neither is scoped to a skill or a project: a write to one skill blocks a write to an unrelated skill in a different project.
The registry's own read-modify-write, noted above, sits entirely outside both locks.

The core crate already has a different primitive that the desktop write path has not adopted.
`skill_studio_host::lease::FileLease` (`lease.rs:16-34`) implements the core's `LeaseProvider` port over one advisory-locked file per canonical root, keyed by a hash of the root path.
`core_scan_installed_skills` uses it for `scan`, as a shared lease.
Every desktop mutation command still goes through the single global `ForkMutationLock` instead of an exclusive `FileLease` scoped to the skill or project it touches.

Five background loops write to this shared state.
The refresh loop writes `SkillRefreshState.snapshot` in memory, and the invocation cache to `cache_path`.
The update-check loop (`skill_update_check.rs:872`, spawned at `lib.rs:132`) runs `check_now` every 6 hours: it writes `UpdateCheckState` in memory and `<app_data>/skill-studio/update-check.json`, then requests a rebuild.
The trial-expiry loop (`skill_trial.rs:828`, spawned at `lib.rs:133`) runs 15 s after startup and then every 5 minutes: it takes `ForkMutationLock`, moves expired trials to trash, and drops their registry records.
Startup event-store reconcile (`lib.rs:17,130`) flips every pending row to `interrupted`, then runs three filesystem repairers; failures are printed, never fatal.
Pack-import staging reconcile (`lib.rs:120`) runs synchronously before the loops start, and also prints and continues on failure.
None of the five loops records a last-run time or a last error anywhere a person or another process can read.

**`docs/spec-core-primitives.md`, promise versus what the map shows.**
The spec promises a lease-based write model: reads take a shared lease, writes take an exclusive lease, and write helpers take an `ExclusiveGuard` and a `ScopedPath`, so a write without the lease or outside the scope does not compile (D9).
It names the rejected alternative in the same line: "A `Mutex<()>` in app state (`ForkMutationLock`). It does not cover the CLI."
That alternative is exactly what runs in production today; the map shows `ForkMutationLock` still gating most writes, unreplaced.
The spec also scopes most of today's write commands, fork, park, update, packs, as "later phase" work in its section 6.3, not yet moved to the core; only `scan`, `diagnose`, `capabilities`, and the two frontmatter-repair and event operations have shipped as core operations so far.
So the spec is a design for where writes should go, largely still ahead of the app rather than behind it.

## Changes in the Claude stack (#73 to #134)

#73 introduced `ScopedPath`, the core's port traits, and the `skill-studio-core`/`skill-studio-host` split.
That split is where `FileLease` and the exclusive-versus-shared lease model live today, alongside the golden-fixture test harness that checks the extracted scan path against the old desktop output.
#105 moved tracked-project state, `TrackedProjects { added, excluded }`, into the core, and stored it under a `projects` key in the same `~/.agents/skill-studio.json` registry.
It added one discovery implementation in `skill-studio-host`, shared by the CLI, MCP server, and desktop, replacing the desktop's own `project_discovery.rs`.
It also imports the old localStorage folder list once, on first start after the change.

## Changes in the Codex stack (#79 to #141)

#79 moved the agent registry and the SKILL.md frontmatter parser into the core crate as exact file moves, with the desktop aliasing its old imports to the new locations; it added no new behavior or lease.
#80 made the desktop's lockfile read take an explicit home path, instead of resolving the process user's home, so a snapshot built for an isolated or fixture home cannot read the real user's `.skill-lock.json` by accident.
It also distinguishes a missing lockfile from a read or parse failure.
#94 made removal of a managed Copy atomic and recoverable: the tree and its verified reader links move into quarantine first, a durable event is recorded, and the operation either publishes the ownership change or restores the original paths on failure, with a startup retry for an interrupted removal.
#92 extracted a bounded Rust telemetry boundary, Sentry envelopes and a bounded queue with timed shutdown, for desktop monitoring; it does not touch skill state files.

## Desired state

Every file on disk should keep one owner module and one writer function.
`write_fork_registry` is already that for the registry, and `event_store.rs` is already that for `events.sqlite3`.
The gap is not ownership; it is the missing lock around that single writer's read-modify-write.

A per-scope lease should replace both global mutexes.
`skill_studio_host::FileLease` already exists, and is keyed by canonical root.
Desktop mutation commands should acquire an exclusive lease on the skill's or project's root through it, instead of the single `ForkMutationLock`, so a park in one project does not block a fork in another.
`SKILL_MD_WRITE_LOCK` should fold into the same mechanism, rather than stay a second, unrelated global lock.

The registry read-modify-write should happen under that same lease, not as an independent atomic rename standing alone.
`set_preferred_editor` and `set_skills_sh_api_key` are explicitly marked "partial-state risk" today.
A lease around the read-modify-write removes the risk, without changing the tmp-plus-rename write itself.

Backups and the event store need a retention limit.
`<app_data>/backups/<event-id>/` and `events.sqlite3` both grow without bound today.
A size or age cap, checked at startup or on a schedule, keeps disk use bounded without weakening the audit trail for recent events.

Background loops should be observable and cancellable.
Each of the five, refresh, update check, trial expiry, startup reconcile, pack staging reconcile, should record its last run time and its last error somewhere a settings or diagnostics view can read.
A long-running loop should be stoppable, rather than only killable by quitting the app.

## Gaps

- `skill_editor.rs:75` (`set_preferred_editor`) and `commands.rs:118` (`set_skills_sh_api_key`): registry read-modify-write is unlocked; a concurrent writer can lose a change. Marked "partial-state risk" in the source map.
- `skill_fork.rs:949` (`ForkMutationLock`) and `skill_md_write.rs:18` (`SKILL_MD_WRITE_LOCK`): both are single global `Mutex<()>` locks, not per-skill or per-project, and neither is reentrant.
- `crates/skill-studio-host/src/lease.rs`: a working per-scope `FileLease` exists for the core's `scan` path, but no desktop mutation command uses it.
- `<app_data>/backups/<event-id>/` and `<app_data>/events.sqlite3`: no retention limit or size cap on either.
- Five background loops, refresh, update check, trial expiry, startup reconcile, pack staging reconcile, expose no last-run or last-error state outside stderr, and none can be cancelled independently of the app.
- `docs/spec-core-primitives.md` D9 names the exact rejected alternative that is still in production: the global `ForkMutationLock`. Its own decision log already flags this as the gap to close.
- The spec's section 6.3 lists most of today's write commands, fork, park, update, packs, as work not yet moved to the core; the write path described in the spec is largely ahead of, not behind, the shipped app.
