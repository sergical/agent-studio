# Reads and snapshot

This area answers one question: what skills exist, and how are they used.
It covers the path from a disk scan to the published snapshot, and every screen that only reads.

Commands: `get_skill_snapshot`, `get_installed_skills`, `request_skill_rescan`, `get_popular_skills`, `search_skills`, `get_skill_details`, skill-use reads (core `skill_uses` parsers, host `SkillInvocationIndex`).

UI entry points: `useSkillSnapshot` (App.tsx), Sidebar Sync button (Sidebar.tsx:235), SkillStore search and list (SkillStore.tsx), Activity view (SkillActivityView.tsx), Home tiles (HomeView.tsx).

## Current state

The snapshot is the one object every read screen renders from.
It is a single `SkillSnapshot`, held in `SkillRefreshState.snapshot`, an `RwLock<Option<SkillSnapshot>>`.
The lock is `None` until the first rebuild finishes.

| Command                | Location               | What it does                                                                                                          |
| ---------------------- | ---------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `get_skill_snapshot`   | `skill_refresh.rs:274` | Returns the current snapshot, or `None`. Never rebuilds.                                                              |
| `get_installed_skills` | `commands.rs:161`      | Returns skills from the snapshot. Rebuilds first, in place, when the snapshot is stale or misses a requested project. |
| `request_skill_rescan` | `skill_refresh.rs:281` | Sets a dirty flag. Does not rebuild itself.                                                                           |
| `get_popular_skills`   | `commands.rs:136`      | Fetches one page of skills.sh listings over HTTP.                                                                     |
| `search_skills`        | `commands.rs:125`      | Fetches a search result over HTTP.                                                                                    |
| `get_skill_details`    | `commands.rs:147`      | Fetches one skill's files and body over HTTP.                                                                         |

`rebuild_snapshot_now` (`skill_refresh.rs:482`) is the only function that writes the snapshot.
A `rebuild_lock` mutex serializes every rebuild, so two rebuilds never race.
The background refresh loop (`skill_refresh.rs:720`, spawned at `lib.rs:117`) watches skill roots, plugin caches, the lock file, Codex config, and `~/.claude/projects`.
It uses a 750 ms debounce and a 200 ms poll.
A skills change triggers a full rebuild.
A transcript change triggers a lighter invocation-only rebuild, at most every 5 s, with a full rebuild after a 60 s backlog.
The loop also picks up the dirty flag that `request_skill_rescan` sets.

A rebuild calls `build_snapshot` (`skill_refresh.rs:1474`), which calls `core_scan_installed_skills` (`skill_refresh.rs:1349`).
That function runs `skill_studio_core::ops::scan` against a `RuntimeScope` built from `home` and the tracked project paths.
This is the one scan path in the app; the CLI and MCP server use the same operation.
`skill_assembly::assemble_installed_skills` (`skill_refresh.rs:1516`) turns the core's `InstalledSkillDto` list into desktop `InstalledSkill`/`Deployment` records.
Overlays for forks, trials, parked state, and update status are applied after assembly, not inside the core.

A push to the frontend happens only after `publish_skill_snapshot` (`skill_refresh.rs:524`) succeeds, over the `skills://snapshot` event.
Store reads (`get_popular_skills`, `search_skills`, `get_skill_details`) go over HTTP to the local proxy server on `127.0.0.1:8787`, or straight to skills.sh with a bearer key when `skills_sh_api_key` is set.
None of the three writes to disk.
SkillStore.tsx toasts on an HTTP failure (`SkillStore.tsx:194,209,238`).

Unit 4.1 removed `get_agent_targets` and `list_skill_projects`, which were registered but had no caller anywhere in `apps/desktop/src`.

Skill-use reads live mostly in the core and host, not behind a Tauri command.
`skill_studio_core::skill_uses` holds one parser per harness: `skill_uses/codex.rs`, `skill_uses/opencode.rs`, `skill_uses/pi.rs`, `skill_uses/cursor.rs`, `skill_uses/grok.rs`.
`SkillUseFilter` is the single place that decides whether a use counts: a harness switched off in Settings is excluded, and a `file_read` does not count when a `user` or `agent` use of the same skill exists in the same session.
`skill_studio_host::SkillInvocationIndex` reads the on-disk sources for every harness: Claude Code transcripts, OpenCode's SQLite databases, Codex rollout files, pi sessions, Cursor and Grok Build transcripts.
The result is folded into the snapshot inside `build_snapshot` (`skill_refresh.rs:1533-1539`), then cached to `cache_path`.
Activity has no Tauri command of its own.
Every stat and heatmap value on that page comes out of the published snapshot.

**Bugbot finding, checked against source.**
`core_scan_installed_skills` (`skill_refresh.rs:1349-1404`) falls back to an empty `Vec` on any scan error: a held lease, or an unreadable root.
It marks `Completeness::Partial`, with the error as the single observation.
A comment at `skill_refresh.rs:1339-1342` confirms this is intentional: "there is no local classifier to fall back to anymore, so an empty snapshot is the only option."
`build_snapshot` (`skill_refresh.rs:1491-1505`) always assembles `skills` from `core_result.skills`, empty or not.
`publish_skill_snapshot`/`store_skill_snapshot` (`skill_refresh.rs:524-552`) then unconditionally replace `state.snapshot` with that result and bump the revision.
There is no branch that keeps the previous snapshot when `scan_partial` is true.
The frontend does show a `ScanPartialBanner` when `snapshot.scan_partial` is set (`SkillsView.tsx:109`), so the failure is visible, not silent.
But the list underneath the banner still goes empty, or loses entries, instead of continuing to show the last good inventory.
The specific defect Bugbot flagged on #73, an empty inventory on scan failure, is still present.
The fix since then adds a banner; it does not add a fallback to the last good snapshot.

## Changes in the Claude stack (#73 to #134)

#73 moved scanning out of the desktop crate.
`core_scan_installed_skills` now calls `skill_studio_core::ops::scan` through `skill_studio_host` ports, and the CLI, MCP server, and TUI read the same operation.
It added the golden-fixture and parity-test harness that checks the extracted scan path against the old desktop output.
#96 made the Skills list virtualize its rows with `@tanstack/react-virtual`, and kept the list mounted under an open skill page, cutting Home-to-Skills navigation from about 515 ms to about 90 ms on a 378-skill estate; it changed how much renders, not what is read.
#122 through #127 moved skill-use parsing into the core (`skill_uses` per harness, `SkillUseFilter`) and the reading of each harness's on-disk source into `skill_studio_host`, one PR per harness, each stacked on the last: OpenCode, Codex, pi and Cursor, then Grok Build.
#130 rebuilt the Activity page around the year heatmap, with harness and trigger filters and a docked day panel; #134 moved that panel above the lists on narrow windows.
Both are frontend changes over the existing snapshot, with no new command.

## Changes in the Codex stack (#93 to #141)

#93 pointed desktop inventory, scoped discovery, ownership, and full and named refreshes, at the same shared core.
It preserves unrelated lock-only entries during a named refresh, and withholds document or invocation edits when ownership is unknown.
#111 fixed a case where Home could report "All clear" while Activity still held an unresolved operation.
The recovery-status read now runs off the UI thread over the full event history, not only the newest 200 rows, and a loading or unavailable status also withholds the all-clear message.
#141 made Activity History reads bounded, through a background worker and shared core queries.
At most two concurrent reads are admitted; the rest get `history_busy`.
Identical pending requests are shared across callers, and invalidated when the snapshot changes or a restore finishes, so a stale response cannot overwrite current history.

## Desired state

A scan failure must never blank the snapshot.
When `core_scan_installed_skills` returns `Completeness::Partial`, `build_snapshot` should merge the failure into the previous published snapshot: keep the last good `skills` list, and set `scan_partial` and `scan_observations` on top of it.
That is the direct fix for the standing Bugbot finding above.

One read path should stay one read path.
`ops::scan` in the core is already the only scanner; the desktop command should stay a thin adapter that assembles DTOs and applies desktop-only overlays, nothing more.

Every background loop that feeds the snapshot, the refresh loop, the invocation reconcile, and the update-check loop, should expose its last run time and its last error.
Then a stuck or failing loop is visible somewhere other than stderr.
`request_skill_rescan` should surface "rebuild failed" to the caller, instead of letting the spinner just stop on the next snapshot with the error swallowed.

Every read command should carry a direct test, not only a test on the guard or the assembly step underneath it.
Today `get_skill_snapshot`, `get_popular_skills`, `search_skills`, `get_skill_details`, `list_github_skills`, `read_installed_skill_md`, and the skill-use reads through core and host all show "no direct test" in the source map.

## Gaps

- `skill_refresh.rs:1398-1404` and `:517-551`: a scan failure replaces the published snapshot with an empty one, instead of keeping the last good snapshot. Only a banner (`SkillsView.tsx:109`) tells the user.
- `request_skill_rescan`: errors from the triggered rebuild are swallowed; the Sync button spinner just stops on the next snapshot push, success or failure.
- No direct tests for `get_skill_snapshot`, `get_popular_skills`, `search_skills`, `get_skill_details`, `list_github_skills`, `read_installed_skill_md`, or the skill-use parsers' Tauri-facing read.
- The refresh loop, the update-check loop, and the invocation reconcile have no exposed last-run or last-error state, for a settings or diagnostics view to read.
