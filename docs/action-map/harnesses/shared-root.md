# Shared root

The universal skills folder and the three ledgers that describe it. Read from the code on 2026-09-17. This is the one place every harness meets, so it is the deployment target for install and the anchor for park, fork, and outdated.

## Where it is

| Root      | Path                      | Scope   | Who reads it                                                                                                                                                                  | Evidence                            |
| --------- | ------------------------- | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------- |
| Universal | `~/.agents/skills`        | global  | Codex, OpenCode, and pi read it directly; Claude Code needs a link; Cursor and Grok Build have their own `.cursor/skills` and `.grok/skills` and reach it only through a link | agents.rs:358 to 381; harness files |
| Universal | `.agents/skills`          | project | same                                                                                                                                                                          | agents.rs:391 to 409                |
| Parked    | `~/.agents/skills-parked` | global  | nobody; a skill moved here is off everywhere                                                                                                                                  | agents.rs:388                       |

The app scans the universal root as its own `SkillRoot` labelled `shared`; no harness's path constant points at it (agents.rs:363 to 412). The full `AgentId` list has 42 harnesses, each with its own dot folder; six are first class for provenance: Claude Code, Codex, OpenCode, pi, Cursor, Grok Build (agents.rs:338 to 345).

## How a harness reaches it

The `npx skills` CLI in symlink mode makes each harness's own skills folder a link into `~/.agents/skills`, either one whole-folder link or one link per skill. Skill Studio does not create the top-level link; it detects it, for example `claude_reads_shared_folder` is true when `~/.claude/skills` is itself a symlink (add_method_defaults.rs:66 to 77, 104). The app's own per-skill links are byte-identical to what the CLI writes in symlink mode, verified against a real install (skill_materialize.rs:16 to 19).

`explode_shared_dir` turns a whole-folder link into a real folder of per-skill links so one skill can be turned off for one harness without touching the shared root (skill_materialize.rs:87 to 160). It refuses unless the canonical parent ends in `.agents/skills` (validate_materialize_root, :58 to 80). The record of which roots were exploded and which links were removed lives in the desktop event store, tables `materialized_roots` and `materialized_disabled`.

## The three ledgers

| Ledger         | File                                                                                            | Written by         | Fields                                                                                                                                                                                                                                                                                                                                                  | Code                                                                                                              |
| -------------- | ----------------------------------------------------------------------------------------------- | ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| skills.sh lock | `~/.agents/.skill-lock.json`, version 3                                                         | `npx skills`       | per skill: `source`, `sourceType`, `sourceUrl`, optional `skillPath`, `skillFolderHash` (a git tree SHA), `installedAt`, `updatedAt`                                                                                                                                                                                                                    | crates/skill-studio-core/src/lock_file.rs:22 to 67; a duplicate in apps/desktop/src-tauri/src/skills/lock_file.rs |
| dotagents      | `~/.agents/agents.toml` (declared) and `~/.agents/agents.lock` (resolved, with a pinned commit) | the dotagents tool | per skill: name, source, GitHub repo, path, installed commit, declared ref, whether a manifest row exists; missing files mean empty, not error                                                                                                                                                                                                          | dotagents_ledger.rs:17 to 33, 93                                                                                  |
| registry       | `~/.agents/skill-studio.json`, version 4                                                        | Skill Studio only  | `forks`, `trials`, `parked`, `harness_disabled` (skill, then harness, then the removed link), `packs`, `copies`, `server_url`, `preferred_editor`, `trusted_dotagents_sources`, `projects` (added and excluded folders and `~/src/*` patterns), `discovery` (harnesses whose folder search is off), plus a flatten catch-all so unknown keys round-trip | skill_fork_registry.rs:253 to 348; tracked_projects.rs:160; discovery_sources.rs:21                               |

Provenance precedence when a name is in more than one ledger: dotagents over plugin over skills.sh over manual, and a fork over any of them (skill_update_check.rs:417 to 425).

## What is not there yet

- **No first run.** No onboarding, welcome, or setup view exists; harness visibility comes from the fixed `FIRST_CLASS_AGENTS` list, and the only user choice is the per-harness folder-search switch in Settings under Project folders (SettingsView.tsx:19 to 20; ProjectFoldersCard.tsx:349, 406; discovery_sources.rs:51 to 64). See harness-detection.md for the signals a first run needs.
- **No preferred method or preferred harnesses.** `AddMethodDefaults` is computed on every open of the Add sheet from environment facts (npx on PATH, lock file exists, harness config folders present, Claude link kind) and never saved (add_method_defaults.rs:56 to 116). The registry has no field for a preference.
- **One lease for one file.** The registry, the lock file, and the ledgers are each read-modify-write with no shared lease; the host has `FileLease` but it is not wired to these writes (crates/skill-studio-host/src/lease.rs).
- **Two lock-file readers.** The core and the desktop each carry a copy of the lock-file parser.

## What the core must handle

- Treat `~/.agents/skills` as the install target and every harness folder as a link or a native switch on top of it.
- Read all three ledgers with one reader each, and write only the registry; the lock file and the dotagents files belong to their tools.
- Take the root lease before any write that touches the shared root or the registry.
- Keep `skillFolderHash` as the currency check for skills.sh installs (see TreeHash in plan.md) and the pinned commit for dotagents.
- Keep the flatten catch-all on the registry so an older build never drops a newer build's keys.
