# Claude Code

What the code knows about Claude Code, read on 2026-09-17. Each fact says whether it is verified against Claude Code documentation, observed from real transcripts, or assumed.

## How the app knows Claude Code is present

**Today.** There is no install probe. The roots `~/.claude/skills` and `.claude/skills` are scanned whether or not the binary exists (apps/desktop/src-tauri/src/skills/agents.rs:168, 217). The headless runner resolves the binary with `$SHELL -lc 'command -v claude'` only when a run starts (skill_agent_runner.rs; docs/agent-skill-conventions.md:350). A missing `~/.claude/projects` folder is treated as "never started", not an error (crates/skill-studio-host/src/skill_uses.rs:304; discovery.rs:396).

**Proper.** Four signals, per harness-detection.md:

| Signal         | Source                                                                                                                                 |
| -------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| Executable     | `claude` resolved on the login-shell PATH; native installs link `~/.local/bin/claude` into `~/.local/share/claude/versions/<version>/` |
| Version        | `claude --version`, which prints `2.1.211 (Claude Code)`                                                                               |
| Install method | `~/.claude.json` fields `installMethod` and `autoUpdates`, the vendor's own record                                                     |
| Configured     | `~/.claude/settings.json` or `~/.claude.json` parses                                                                                   |
| Used           | `~/.claude.json` field `numStartups` above zero, or any transcript under `~/.claude/projects`                                          |

`/Applications/Claude.app` is Claude Desktop, a different product, and says nothing about Claude Code. The `~/.claude/` folder is also written by the VS Code and JetBrains extensions, so it can exist with no CLI.

## Where skills live

| Root         | Path                                 | Scope                                          | Depth     | Evidence                                                             |
| ------------ | ------------------------------------ | ---------------------------------------------- | --------- | -------------------------------------------------------------------- |
| Own          | `~/.claude/skills`, `.claude/skills` | global, project                                | one level | harness.rs:461 to 475                                                |
| Universal    | `~/.agents/skills`                   | not read directly; Claude Code needs a symlink |           | harness.rs:484; docs/agent-skill-conventions.md:69, 128              |
| Plugin cache | `~/.claude/plugins/cache`, recursive | global                                         | recursive | verified, code.claude.com/docs/en/plugins-reference (harness.rs:476) |

The deployment into `~/.claude/skills` is either one whole-folder symlink to `~/.agents/skills` or a per-skill symlink `~/.claude/skills/<name>`. The code names these `ClaudeLinkState::WholeDir`, `PerSkill`, and `None` (skill_harness_disable.rs:292, 421, 577). Whether Claude Code follows either kind of link is `Unknown` in the facts table (harness.rs:485), even though the disable code relies on the per-skill kind. Hidden entries such as `.skill-studio-disabled/` are never reached because the reader is one level deep (harness.rs:236, 487).

Roots the code does not know: the enterprise managed-settings root, the synced root `~/.claude/skills/synced/`, and skills-directory plugins named `<name>@skills-dir` (docs/research/harness-primitives.md:739).

## How Claude Code loads a skill

- Frontmatter fields it reads: `name`, `description`, and the Claude-specific `disable-model-invocation`, `user-invocable`, `allowed-tools`, `disallowed-tools`, `context`, `agent`, `background`, `paths`, `model`, `effort`, `argument-hint`, `arguments`, `hooks`, `shell`, `when_to_use`, `metadata`, `license`, `compatibility`. Verified against code.claude.com/docs/en/skills on 2026-08-22 (docs/agent-skill-conventions.md:52).
- The doc notes nested `.claude/skills/` folders in subfolders as a discovery quirk (docs/agent-skill-conventions.md:122), but the core root spec marks Claude roots non-recursive (harness.rs:466). One of the two is wrong.

## Plugins

- The cache is enumerated recursively; the manifest is `.claude-plugin/plugin.json` (docs/research/harness-primitives.md:184).
- Enable, disable, and uninstall shell out to `claude plugin disable|enable <id> -s user` and `claude plugin uninstall <id> -s user -y` (skill_plugin_lifecycle.rs:14, 89). The switch is per plugin, so every skill in a plugin moves together (skill_plugin_lifecycle.rs:3).
- Watch events under the plugin cache are classified separately in the refresh loop (skill_refresh.rs:1639, 1736, 1972).

## How the app turns a skill off for Claude Code

| Mechanism                                                                                      | File touched                                                                            | State                                           | Code                                                                   |
| ---------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- | ----------------------------------------------- | ---------------------------------------------------------------------- |
| Remove the per-skill symlink                                                                   | `~/.claude/skills/<name>` removed; target kept in the registry under `harness_disabled` | implemented                                     | skill_harness_disable.rs:421 to 546, 577 to 604; skill_refresh.rs:1242 |
| `skillOverrides` in `~/.claude/settings.json` (values on, name-only, user-invocable-only, off) | `~/.claude/settings.json`                                                               | named in the facts table, never read or written | harness.rs:123, 154, 489; docs/research/harness-primitives.md:738      |
| Park                                                                                           | remove the per-skill link, then move the skill out of `~/.agents/skills`                | implemented                                     | docs/agent-skill-conventions.md:77                                     |

The symlink removal refuses to act when the deployment is a whole-folder link, because there is no per-skill link to remove (skill_harness_disable.rs:494). This is the one native switch Claude Code has, `skillOverrides`, and the app does not use it yet.

## Activity: how the app sees a skill use

- Source: `~/.claude/projects/<project>/*.jsonl` and `<session>/subagents/*.jsonl` (skill_uses.rs:64, 300). Missing folder means empty and complete; any other listing failure marks the source incomplete (skill_uses.rs:304).
- Parser: `parse_claude_code_uses` (crates/skill-studio-core/src/skill_uses.rs:519). A line is parsed only if it contains `"name":"Skill"`, `<command-name>`, or `SKILL.md` (:515). Each record needs a `timestamp`; `cwd` gives the project and `sessionId` the session (:538).
- Three triggers count as a use:
  1. An assistant `tool_use` block named `Skill`; the skill is `input.skill` (:566).
  2. An assistant `Read` of `.../<skill>/SKILL.md`, or a `Bash` command that reads one with cat, head, tail, nl, less, more, bat, or `sed -n` (:589; verb table :445).
  3. A user record whose text starts with `<command-message>` or `<command-name>`; the name is the text between the `command-name` tags with the slash stripped. Records with `isMeta: true` are skipped (:616).
- The record shape is observed from real transcripts, not documented by Anthropic (harness.rs:505; resolved by live runs on 2026-09-16 per docs/research/harness-primitives.md:740).
- Cost caps shared with the other harnesses: 256 KiB per line, 16 MiB per file per refresh, 128 MiB per run, 256 MiB cache (skill_uses.rs:46 to 62). Each file is cached by size and mtime with a resume offset and a 64-byte tail sample, so an append is resumed and a rewrite is detected (host skill_uses.rs:76). The cache file is `skill-uses.json` under the app data folder (skill_refresh.rs:855).
- Live refresh watches `~/.claude/projects` recursively (host skill_uses.rs:816).
- Usage is keyed to a skill by exact name match. No fuzzy match.

## What the core must handle for Claude Code

- Create and remove per-skill symlinks, and detect a whole-folder link and refuse to break it.
- Read and write `settings.json` `skillOverrides` without losing other keys, once the switch is adopted. Not done.
- Enumerate the plugin cache and call the `claude plugin` CLI for plugin switches.
- Resume transcripts from an offset, tolerate a file that is being appended, and detect rewrites.
- Know the managed-settings root and the synced root. Not done.

## Resolved by the docs on 2026-09-16

From https://code.claude.com/docs/en/skills (sources.md):

- A symlinked skill folder loads once, deduplicated by its target, so a per-skill link works and a whole-folder link to `~/.agents/skills` works too. The facts table row at harness.rs:485 can move from Unknown to verified.
- Nested `<subdir>/.claude/skills` folders are discovered when Claude touches files there, up to the repo root. The core root spec at harness.rs:466 is wrong to mark Claude roots non-recursive for project scope.
- `synced` under `~/.claude/skills` is reserved; the scanner must skip it.
- `CLAUDE_CONFIG_DIR` overrides `~/.claude`; the app must honour it when resolving every Claude path.
- The plugin cache layout is `~/.claude/plugins/cache/{marketplace}/{plugin}/{version}/`, with synced plugins at `~/.claude/plugins/synced/{name}/`.
- Off switches, in order of reach: `disableBundledSkills`, `skillOverrides`, `Skill(name)` permission rules, then the two frontmatter fields.

## Unknowns

- Whether `skillOverrides` is safe to write from outside Claude Code while it runs.
- Whether there is an all-skills-off switch beyond denying the `Skill` tool.
