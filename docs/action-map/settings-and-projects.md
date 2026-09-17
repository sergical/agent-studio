# Settings, tracked projects, and editor choice

This area covers the Settings view's Project folders and Open in editor cards: which folders Skill Studio scans for skills, which discovery sources feed that list, and which editor opens a skill's files.
All of it reads and writes one shared registry file.

Commands: get_tracked_projects, register_skill_projects, unregister_skill_project, remove_skill_project, import_tracked_projects, list_project_folders, set_discovery_source, get_editor_choices, set_preferred_editor, get_preferred_editor, list_installed_editors, get_skills_sh_access, set_skills_sh_api_key, open_skill_path, and the registry file `~/.agents/skill-studio.json`.
UI entry points: ProjectFoldersCard, EditorCard, App.tsx startup import, project pickers in AddSkillSheet and SkillStoreInstallFlow.

This is the only area in the map where the same registry file, discovery function, and editor logic are already shared with the CLI and MCP server through skill-studio-core and skill-studio-host, so the desired state below builds on that groundwork rather than proposing a first extraction.

## Current state

The registry file backs two independent settings surfaces: which project folders feed the skill scan, and which app opens a skill's files.
Both surfaces write the same file through the same unlocked pattern, so a bug found in one (for example a lost concurrent write) applies equally to the other.

All project-folder commands share one helper, update_registry_section (skill_refresh.rs:270): read the registry with read_fork_registry, clone the field before the change, apply the change, and write back only if the field actually differs.
A repeated toggle or a no-op add never touches the file's mtime, but there is no lock around the read-modify-write: two commands racing on the same file can each read the pre-change value, and the second writer's change overwrites the first's.

`get_tracked_projects` (:290) is a plain read of registry.projects, no state change.
`register_skill_projects` (:319) drops any path equal to home, logged not errored, validates each remaining path or `*` pattern through tracked_projects::entry_to_save, and aborts the whole batch on the first invalid entry before anything is saved.
`unregister_skill_project` (:341) records an exclusion so discovery will not offer the folder again.
`remove_skill_project` (:362) is the counterpart for a hand-added folder: it removes it from added but records no exclusion, so discovery can offer it again later.
`import_tracked_projects` (:384) is a one-shot migration from the old localStorage-only list: it tracks the added paths and untracks the excluded ones in one call, so an interrupted migration cannot leave the file half migrated.
Each of the four writers calls state.mark_skills_dirty() only when the section actually changed, which schedules a full rebuild on the background thread, not an inline one.

`list_project_folders` (skill_project_folders.rs:136) is a read-only, spawn_blocking rescan: it runs discover_skill_projects fresh with no cache, reads the current TrackedProjects, and builds one row per discovered or added folder, one row per `*` pattern rather than one row per matched folder, and one row for a hand-added path that no longer resolves, marked "not found" instead of dropped.

`get_discovery_sources` (:432) reads the discovery map straight off disk, one row per harness in discovery_harnesses()'s display order.
`set_discovery_source` (:463 → 442) validates the harness name against that same list, flips it through update_registry_section, and triggers a rebuild only if the value changed; a harness with no entry, or a malformed discovery section, is treated as enabled.

`get_editor_choices` (commands.rs:2374) and `set_preferred_editor` (:2386 → skill_editor.rs:75) share the same unlocked registry read-modify-write pattern as the project-folder commands.
`set_preferred_editor` refuses an app name that is not in installed_editors; the write goes through write_fork_registry's tmp-plus-rename, so a rename failure leaves nothing partial, but a concurrent registry writer, for example set_discovery_source at the same instant, can still lose one side's change, since neither takes a lock.
`get_editor_choices` runs off the main thread because it may spawn the login shell to read $VISUAL/$EDITOR with a 3-second timeout.

`set_skills_sh_api_key` (commands.rs:118) writes the same registry file the same unlocked way; the key is never logged.
`open_skill_path` (:2346) is a pure proc command, Reveal in Finder and Open in editor: it checks the path belongs to a deployment in the current snapshot, then shells out to `open`, with no write of its own.

The registry file itself, `~/.agents/skill-studio.json` at map line 1247, is written tmp-plus-rename with a unique tmp name per writer, but with no lock around any read-modify-write.
It holds projects, discovery, preferred_editor, skills_sh_api_key, plus fork, trial, pack, and copy state that other commands in this document do not touch.

Tests: register_skill_projects_drops_only_the_home_directory (:2057); build_snapshot_excludes_stopped_tracking_project (:1980); set_discovery_source_at_* (five tests, :3216-3306); an_uninstalled_editor_is_refused_rather_than_saved and a_saved_choice_round_trips_and_clears (skill_editor.rs:139,149); save_skills_sh_api_key_* (three tests, commands.rs:1422-1438).
list_project_folders and get_discovery_sources have no direct test named in the map.

In the UI, every mutating control in ProjectFoldersCard and EditorCard follows the same pattern: update local state at once, call the command, then reload the list; on failure, show a toast and revert the optimistic change (ProjectFoldersCard.tsx:411-414, EditorCard.tsx:123-157).
App.tsx's startup effect (:1312) reads any legacy localStorage project list, calls import_tracked_projects if present, else get_tracked_projects, and clears localStorage only after a successful import.
A failure at startup shows the toast "Couldn't load project folders" and leaves the legacy localStorage list in place, so the next launch retries the same import rather than losing the user's old folder list.
The picker flows in AddSkillSheet and SkillStoreInstallFlow both call `register_skill_projects` after a Tauri `open()` folder pick, and both keep the picked folder selected in the form even when the save call fails, showing only a toast, "Couldn't save project folder," rather than reverting the selection.

## Changes in the Claude stack (#73 to #134)

#105 moved the tracked-project-folder list (TrackedProjects {added, excluded}) into skill-studio-core, stored under a new projects key in the registry, and consolidated project discovery, Codex config.toml and Claude Code transcripts, into one skill_studio_host function shared by the desktop app, the CLI, and the MCP server; it also changed discover-mode home-folder handling from an error to a silent drop, and deleted the desktop's own project_discovery.rs.
#106, #107, and #108 each add one more discovery source to that shared function — pi and Cursor, OpenCode including its v2 beta databases read read-only so no WAL files are created, and Grok Build — without touching the Tauri commands or the registry format.
#110 adds the per-harness on/off switch: a new discovery key in the registry, plus the get_discovery_sources and set_discovery_source commands, honored by the same shared discover_skill_projects function so the CLI and MCP server get the toggle for free.
#112 builds the Settings "Project folders" card itself: the folder list with Stop tracking, Remove, and Folder not found rows, per-harness search switches, and a shared useProjectFolderActions hook used by both the card and the skills filter bar; it also adds TrackedProjects::forget in core for the Remove action.
#131 adds typed-path and `~/src/*`-pattern entries to the same card, re-resolved on every refresh.
#132 splits "Open in editor" into its own EditorCard, adds a file-picker "Choose another app…" row and a $VISUAL/$EDITOR row read from the login shell, and moves all editor logic into skill_editor.rs with async Tauri commands so shell reads never block the window.

## Changes in the Codex stack (#79 to #141)

#93 ports desktop inventory, scoped discovery, ownership, full and named refresh, onto a shared Rust core crate, and extends Settings so a user can save additional linked-skill folders and mark a plugin's ownership boundary as complete; saved folders survive restart, refresh the inventory in the background, and can be removed without deleting skills.
It preserves unrelated lock-only entries during a named refresh, surfaces incomplete reads, and blocks document or invocation edits when a skill's ownership is unknown, while still allowing local document edits when only git ancestry, not a read boundary, is unclear.
The PR states its own limits: document-write admission still uses snapshot-time ownership, and coordinated writes, atomic removal, journal history, and startup recovery are explicitly left for later work.
It does not add discovery sources beyond Codex and Claude Code, per-harness toggles, or the editor picker; those are Claude-stack-only in the current PR queues.

`get_skills_sh_access` and `list_installed_editors` are reads that back the same two cards; `list_installed_editors` scans `/Applications` for known editor bundle identifiers, and `get_skills_sh_access` reports whether the app is running in proxy mode or direct mode based on whether `skills_sh_api_key` is set.
Neither writes, and neither is named by a frontend caller in the source map's cross-check, alongside `get_preferred_editor`, which the map lists as having no wrapper in skill-api.ts on this branch even though `get_editor_choices` already returns the same information.

## Desired state

Every registry write here — register_skill_projects, unregister_skill_project, remove_skill_project, import_tracked_projects, set_discovery_source, set_preferred_editor, set_skills_sh_api_key — records a journal event with a backup and an inverse before it writes the registry, so a lost race or a bad write is visible in the same history the fork and pack commands already use, not silent.
update_registry_section and its callers become one write path in skill-studio-core, already partly true after #93 and #105 move discovery and project tracking into core; the Tauri commands in skill_refresh.rs, skill_editor.rs, and commands.rs become thin adapters over that path, shared by the CLI and MCP server the way discovery already is.
The registry gets a per-scope lease, one lease per top-level section (projects, discovery, preferred_editor, skills_sh_api_key), instead of the current no-lock read-modify-write, so set_discovery_source and set_preferred_editor firing at the same instant cannot silently drop one change.
import_tracked_projects's track-then-untrack pair already behaves like one transaction; the same discipline extends to register_skill_projects's validate-then-track step, so a batch that fails validation partway never has a different partial effect than "nothing saved."
Success feedback in ProjectFoldersCard and EditorCard already waits for the command and the reload before showing the row change; that pattern extends to naming a partial result, for example when a saved pattern matches zero folders, or when list_project_folders's rescan itself partially fails for one discovery source, instead of only the current toast-plus-revert binary.
Every command above gets a direct test, most already have one, and a crash-window test: what the registry looks like if the process dies between the read and the tmp-plus-rename write, since today's tmp-plus-rename protects against a torn write but not against two writers racing.

`get_tracked_projects` and `get_discovery_sources` stay strict, unlocked reads, since they change nothing; the lease above only serializes writers against writers, not readers against writers, matching how `read_fork_registry` already treats a malformed file as a hard error rather than an empty default.
`get_preferred_editor` either gets a caller in skill-api.ts or is retired in favor of `get_editor_choices`, so the command list matches what the UI actually uses.

## Gaps

- All project-folder and editor/key writers: no journal, no backup, no inverse; a bad or interrupted write is only visible as a toast, not as a recoverable history entry.
- update_registry_section and set_preferred_editor / set_skills_sh_api_key's registry writes: no lock around read-modify-write; a concurrent writer can lose its change with no error.
- list_project_folders: full harness-history rescan every call, not cached, so a slow discovery source, for example a large OpenCode database or many pi sessions, delays every card open.
- get_discovery_sources and list_project_folders: no direct test named in the map.
- register_skill_projects: the home-directory drop is silent to the user, only `eprintln!`, not surfaced as a named partial result when a batch shrinks.
- `get_preferred_editor`: registered command with no frontend wrapper, duplicating what `get_editor_choices` already returns.
- `get_skills_sh_access`, `list_installed_editors`: reads with no direct test named in the map, though both back a visible Settings row.
- No PR in either stack's current queue adds journaling, a backup/inverse, or a per-scope lease to this area; #93's own description names coordinated writes and journal history as explicitly out of scope.
