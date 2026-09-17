# OpenCode

What the code knows about OpenCode, read on 2026-09-17. Facts are verified against the code unless marked assumed or doc-only.

## How the app knows OpenCode is present

**Today.** There is no install probe. Presence is inferred from skill roots and session data on disk (apps/desktop/src-tauri/src/skills/agents.rs:363; crates/skill-studio-host/src/discovery.rs). A project counts as an OpenCode project when `.opencode/skills` or `.opencode/skill` exists (crates/skill-studio-core/src/tracked_projects.rs:18). The runner names the binary `opencode` with support Partial, because the v2 beta adds a second `opencode2` binary during migration (harness.rs:694, doc-only).

**Proper.** Four signals, per harness-detection.md. Rows marked "confirm v2" must be checked against https://opencode.ai/v2/docs before the code lands; v1 docs are not a source.

| Signal         | Source                                                                                                                                                           |
| -------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Executable     | `opencode` resolved on the login-shell PATH (curl installer, npm `opencode-ai`, or Homebrew tap); also look for `opencode2` during the v2 migration (confirm v2) |
| Version        | `opencode --version` (confirm v2)                                                                                                                                |
| Install method | inferred from the resolved path, reported as inferred                                                                                                            |
| Configured     | `~/.config/opencode/opencode.json` or `.jsonc` parses; `~/.config/opencode/package.json` pins the plugin runtime                                                 |
| Used           | rows in `~/.local/share/opencode/opencode.db` or a channel database, or legacy `storage/session/**`; `auth.json` proves a login (confirm v2)                     |

The config folder can exist from editing settings with no session ever run, so Used comes from the database only. The probe reads no values from `service.json` or `opencode.json` beyond the keys it needs; both can hold secrets.

## Where skills live

| Root          | Path                                            | Scope           | Depth                             | Role                               |
| ------------- | ----------------------------------------------- | --------------- | --------------------------------- | ---------------------------------- |
| Own           | `~/.config/opencode/skills`, `.opencode/skills` | global, project | any depth, also root-level `*.md` | harness.rs:600; agents.rs:169, 218 |
| Legacy v1     | `~/.config/opencode/skill`, `.opencode/skill`   | global, project | one level                         | harness.rs:645; agents.rs:373, 399 |
| Universal     | `~/.agents/skills`, `.agents/skills`            | global, project | any depth                         | harness.rs:617                     |
| Cross-harness | `~/.claude/skills`, `.claude/skills`            | global, project | any depth                         | harness.rs:605                     |

Whether OpenCode follows a per-skill or whole-folder symlink is Unknown (harness.rs:663).

## How OpenCode loads a skill

- v2 reads root-level `*.md` files and nested SKILL.md at any depth in every non-legacy source (harness.rs:600).
- Config file: `~/.config/opencode/opencode.json`. A sibling `opencode.jsonc` is detected but never parsed or written; the app returns "OpenCode's config is opencode.jsonc; edit permission.skill by hand" (opencode_skill_permission.rs:17, 40, 114).
- `permission.skill.<name-or-glob>` with value `deny` turns a skill off. Globs support one `*` (opencode_skill_permission.rs:76).
- Plugins: v2 plugins register skills through `ctx.skill` in code. There is no on-disk plugin cache to enumerate (harness.rs:687).

## How the app turns a skill off for OpenCode

| Mechanism   | File touched                                                                                                                            | Code                                                                               |
| ----------- | --------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| Native deny | `opencode.json`, `permission.skill.<name> = "deny"`, written through `.json.tmp` and rename, other keys kept, `$schema` added on create | opencode_skill_permission.rs:52 (read), :113 (write); skill_harness_disable.rs:624 |
| Park        | rename out of `~/.agents/skills`                                                                                                        | generic                                                                            |

The permission key is keyed by name only, so the app refuses to toggle when a name has more than one OpenCode deployment, for example project and global (skill_harness_disable.rs:130, 150). On rebuild the deny set marks a deployment disabled only when the skill has exactly one OpenCode deployment; a shared deployment gets `open-code` appended to `disabled_readers` instead (skill_refresh.rs:1249 to 1305). No all-skills-off switch is known (harness.rs:666).

## Activity: how the app sees a skill use

- Source: SQLite at `~/.local/share/opencode/opencode.db` and `opencode-<channel>.db`, regular files only, at most 16 (crates/skill-studio-host/src/opencode_db.rs:12, 22).
- Open: read-only. `mode=ro` when the `-wal` and `-shm` sidecars already exist, else `mode=ro&immutable=1` so the app never creates sidecars next to a closed database. Busy timeout 250 ms. A test proves no sidecar is created (opencode_db.rs:63, 103).
- Tables: `session_message` (v2) joined to `session_v2` or `session` for the directory; `part` (v1) joined to `session` (skill_uses/opencode.rs:127, 207). Presence checked in `sqlite_master` (opencode_db.rs:89).
- Triggers: a row of type `skill` is a User trigger; an assistant row yields one use per content item whose tool is `skill` (Agent) or `read` of a known SKILL.md (FileRead). Errored tool calls are skipped (core skill_uses/opencode.rs:81 to 155).
- Incremental read: a `time_updated` watermark per table, re-queried from the watermark minus 60 s of slack. If the new watermark is lower than the cached one the database was replaced and the table is re-read from the start (skill_uses/opencode.rs:17, 56). Rows that vanish are pruned by primary key (:71).
- Dedupe: the same session can appear in `opencode.db` and a channel database; a use counts once, databases visited in order (host skill_uses.rs:1294).
- Watch: the database file and its `-wal` sidecar trigger a refresh; `-shm` is excluded on purpose because the app's own read-only open touches it (host skill_uses.rs:256).
- A locked database skips that refresh cycle and leaves the cache unchanged (skill_uses/opencode.rs:27).
- Project discovery reads `project.worktree` from every channel database plus legacy `storage/project/<id>.json`, capped at 10,000 (discovery.rs:274).

## What the core must handle for OpenCode

- Read and write `opencode.json` without losing keys, and either parse `opencode.jsonc` or say clearly that it will not.
- Keep the deny key in step with a rename, park, or fork. A name-keyed switch cannot tell two deployments apart, so the core must refuse or ask, as it does today.
- Open the database read-only, never create sidecars, tolerate locks, and detect a replaced database.
- Honour `$XDG_DATA_HOME`, `OPENCODE_DB`, and `OPENCODE_CONFIG`. Not done; the app reads only the default paths (docs/agent-skill-conventions.md:144).

## From the v2 docs on 2026-09-16

From https://opencode.ai/v2/docs/skills/ and https://opencode.ai/v2/docs/config (sources.md). Only these two pages and the source repo are sources; v1 docs are not.

- Precedence: built in, then `.claude/skills` and `.agents/skills` (global, then ancestors), then `~/.config/opencode/skills`, then `.opencode/skills` from the project root toward the working folder, then the `skills` array in `opencode.json(c)`, which can name relative, `~/`, absolute, or HTTP catalog entries. The app does not read the `skills` array; a skill listed only there is invisible to it.
- Frontmatter OpenCode reads: `name`, `description`, `slash`, `metadata.opencode/slash`, `metadata.opencode/autoinvoke`.
- Two off switches: `metadata.opencode/autoinvoke: false` in the skill's frontmatter hides it from auto-discovery but keeps it loadable; a permission rule with `"effect": "deny"` blocks a skill id. The page describes deny as a rule with an `effect` field, not the `permission.skill.<name> = "deny"` key the app writes today (opencode_skill_permission.rs:113). Confirm the accepted shapes in the source under `packages/` before the adapter lands; if the old shape is gone, the app is writing a key OpenCode ignores.
- Config search: project `opencode.json(c)` or `.opencode/opencode.json(c)`, searched from the working folder up to the root, direct files merged before `.opencode` ones. The app reads only the global file.
- Env overrides: the fetched config page names only the fixed path; a search snippet from the same page names `XDG_CONFIG_HOME` and `OPENCODE_CONFIG_DIR`. Unresolved; read the source.

## Unknowns

- Symlink following, both kinds; the v2 skills page does not say.
- Whether v2 still reads `permission.skill.<name>`.
- An all-skills-off switch.
- The `opencode` versus `opencode2` binary during the v2 migration, and every install and version fact, since v2 has no install page.
