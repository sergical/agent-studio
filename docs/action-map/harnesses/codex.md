# Codex

What the code knows about OpenAI Codex CLI, read on 2026-09-17. Each fact says whether it is verified against Codex documentation or source, or assumed from behaviour.

## How the app knows Codex is present

**Today.** There is no binary or version check. Presence is inferred from folders: `~/.codex/skills`, `.codex/skills` in a project, `~/.codex/plugins/cache`, and `~/.codex/config.toml`. A missing folder is treated as "not installed or nothing archived yet", not an error (crates/skill-studio-host/src/skill_uses.rs:427). The runner spec names the binary `codex` but nothing runs it (crates/skill-studio-core/src/harness.rs:588).

**Proper.** Four signals, per harness-detection.md:

| Signal                     | Source                                                                                                                                                    |
| -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Executable                 | `codex` resolved on the login-shell PATH, or the bundled copy at `/Applications/ChatGPT.app/Contents/Resources/codex` reported as "not on PATH"           |
| Version and install method | `codex --version` once per binary change; install method inferred from the resolved path (npm global, Homebrew, ChatGPT.app bundle), reported as inferred |
| Configured                 | `~/.codex/config.toml` parses                                                                                                                             |
| Used                       | any `~/.codex/sessions/**/rollout-*.jsonl`, or a `[projects."<path>"]` trust row                                                                          |

`~/.codex/version.json` is the update-check cache (`latest_version`, `last_checked_at`), not the installed version. Do not read it as one.

## Where skills live

| Root         | Path                                 | Scope           | Evidence                                                                                                |
| ------------ | ------------------------------------ | --------------- | ------------------------------------------------------------------------------------------------------- |
| Universal    | `~/.agents/skills`, `.agents/skills` | global, project | verified, learn.chatgpt.com/docs/build-skills (harness.rs:523)                                          |
| Own          | `~/.codex/skills`, `.codex/skills`   | global, project | inferred from github.com/openai/codex/issues/22590, not a doc (harness.rs:543, 550; agents.rs:175, 224) |
| Plugin cache | `~/.codex/plugins/cache`, recursive  | global          | doc link only, layout not parsed (harness.rs:552)                                                       |

Codex follows a per-skill symlink into a deployment (verified, harness.rs:561). Whether it follows a symlinked whole `skills` folder is unknown (harness.rs:562).

## How Codex loads a skill

- Folder name must equal the `name` field in SKILL.md frontmatter (docs/agent-skill-conventions.md:14). Codex's own `skills.read` tool derives the name from the folder that holds SKILL.md, and the code mirrors that in `codex_skill_name_from_package` (crates/skill-studio-core/src/skill_uses/codex.rs:203).
- `~/.codex/config.toml` carries `[[skills.config]]` rows with `path` and `enabled`, and `[projects."/abs/path"]` sections for recent projects (crates/skill-studio-host/src/discovery.rs:27). Verified against local data.
- A sidecar `agents/openai.yaml` next to SKILL.md holds `policy.allow_implicit_invocation` (apps/desktop/src-tauri/src/skills/skill_invocation.rs:182).

## Plugins

The app lists `~/.codex/plugins/cache` as a plugin root and nothing more. There is no Codex manifest parser and no enable or disable path. Codex plugins are managed with `/plugins` inside a Codex session, so skill_plugin_lifecycle.rs runs only the `claude` binary (skill_plugin_lifecycle.rs:6, 69).

## How the app turns a skill off for Codex

| Mechanism            | File touched                                                                                                                                                   | Code                                                                        |
| -------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| Native disable       | `~/.codex/config.toml`, a `[[skills.config]]` row with the absolute SKILL.md path and `enabled = false`, written with toml_edit through a temp file and rename | codex_skill_config.rs:23 (read), :172 (write); skill_harness_disable.rs:615 |
| Restrict auto-invoke | `<skill>/agents/openai.yaml`, `policy.allow_implicit_invocation: false`; file created or deleted as needed                                                     | skill_invocation.rs:193                                                     |
| Park                 | rename `~/.agents/skills/<name>` to `~/.agents/skills-parked/<name>`; hits Codex because it reads the universal root                                           | docs/agent-skill-conventions.md:79                                          |

The native disable refuses with "No Codex-visible deployment found" when the skill has no path Codex can see (skill_harness_disable.rs:616). There is no known "turn off all skills" switch (harness.rs:564).

## Activity: how the app sees a skill use

- Source: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` and `~/.codex/archived_sessions/**`, walked four levels deep (skill_uses.rs:406, 417).
- One `session_meta` line per file gives the session id and the project path. The parser carries that context through the file and across refreshes (codex.rs:85; skill_uses.rs:10).
- Three triggers count as a use (codex.rs:69):
  1. A user message whose text starts with `<skill>\n<name>X</name>\n<path>P</path>`, which is what typing `$X` produces (codex.rs:26).
  2. A `function_call` with namespace `skills` and name `read`; no local session has been seen calling it (codex.rs:164; docs/agent-skill-conventions.md:168).
  3. A shell call (`exec`, `exec_command`, or `shell`) whose command reads a SKILL.md the app knows (codex.rs:266, :41).
- A substring pre-filter skips lines without `session_meta`, `<skill>`, `SKILL.md`, or `"namespace":"skills"` before any JSON parse (codex.rs:16).
- Cost caps: 256 KiB per line, 16 MiB per file, 128 MiB per refresh, 256 MiB on-disk cache (skill_uses.rs:50 to 62). Files resume from a byte offset, so a refresh reads only what was appended.
- The `originator` field names the surface (Codex Desktop, codex exec) but no code reads it.

## What the core must handle for Codex

- Read and write `config.toml` without losing other tables or comments. Done today with toml_edit.
- Keep the SKILL.md path in a disable row in sync when a skill moves, is parked, or is forked. Not done: a park leaves a stale `[[skills.config]]` row.
- Treat the universal root as the primary install target, and `.codex/skills` as a secondary root whose status is inferred, not documented.
- Resume rollouts from an offset and tolerate a file that is still being written.

## Resolved by the docs on 2026-09-16

From https://learn.chatgpt.com/docs/build-skills and the config reference (sources.md):

- Documented roots, searched upward: `$CWD/.agents/skills`, `$REPO_ROOT/.agents/skills`, `~/.agents/skills`, `/etc/codex/skills`, plus bundled skills. `.codex/skills` is not in the list, so it stays inferred and the app must not offer it as a default install target.
- `CODEX_HOME` overrides `~/.codex` for config, sessions, and the SQLite state; every Codex path in the app must honour it.
- A project `.codex/config.toml` is read only when the project is trusted.
- Disabling through `[[skills.config]]` needs a Codex restart to take effect; the app should say so after a switch.

## Unknowns

- Whether Codex follows a symlinked whole skills folder.
- Plugin cache layout and manifest; no marketplace page exists, so the app enumerates the folder blind.
- Whether the `skills.read` tool is ever emitted by shipping Codex builds.
- `codex --version` output and the rollout format, both observed only.
