# Agent skill conventions

Reference for how each first-class agent discovers, invokes, and controls skills,
and what the agentskills.io spec requires. Verified against the linked docs on
2026-08-22. Re-verify a row before relying on it if the agent shipped a major
release since then.

## agentskills.io SKILL.md spec

Source: https://agentskills.io/specification

| Field           | Required | Constraint                                                                                             |
| --------------- | -------- | ------------------------------------------------------------------------------------------------------ |
| `name`          | yes      | 1–64 chars; `a-z`, `0-9`, `-` only; no leading/trailing/consecutive hyphens; equals the directory name |
| `description`   | yes      | 1–1024 chars; what the skill does and when to use it                                                   |
| `license`       | no       | License name or a bundled license file reference                                                       |
| `compatibility` | no       | 1–500 chars; environment requirements                                                                  |
| `metadata`      | no       | Arbitrary string-keyed map                                                                             |
| `allowed-tools` | no       | Space-separated pre-approved tools (experimental)                                                      |

Directory layout: `SKILL.md` (required), optional `scripts/`, `references/`, `assets/`.
The spec has **no** invocation-control fields; those are agent extensions (below).
Reference validator: `skills-ref validate ./my-skill`.

Skill Studio enforces these rules in `src-tauri/src/skills/frontmatter.rs`
(`validate_skill`) and reports failures as `spec_violations`.

## Invocation control and disable, per agent

Claude Code, Codex, OpenCode and pi auto-invoke a skill by default when its
description matches the task; Cursor and Grok Build's invocation model isn't
verified yet (see "unknown" cells below).

| Agent       | Explicit invocation | Restrict model auto-invoke                                                                     | Disable a skill (keep on disk)                                                   |
| ----------- | ------------------- | ---------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| Claude Code | `/name [args]`      | frontmatter `disable-model-invocation: true` (user only); `user-invocable: false` (model only) | none native → Skill Studio parks the folder (see below)                          |
| Codex       | `$name`             | sidecar `agents/openai.yaml` → `policy.allow_implicit_invocation: false`                       | `~/.codex/config.toml` → `[[skills.config]] path = "…/SKILL.md" enabled = false` |
| OpenCode    | `skill` tool        | `permission.skill` in `opencode.json`: per-name pattern `allow` / `deny` / `ask`               | same: `"name": "deny"` (wildcards allowed, e.g. `internal-*`)                    |
| pi          | `/skill:name`       | frontmatter `disable-model-invocation: true`                                                   | none native → Skill Studio parks the folder                                      |
| Cursor      | unknown             | unknown                                                                                        | no per-skill disable known                                                       |
| Grok Build  | unknown             | unknown                                                                                        | no per-skill disable known                                                       |

Sources: https://code.claude.com/docs/en/skills · https://developers.openai.com/codex/skills ·
https://opencode.ai/docs/skills/ · https://pi.dev/docs/latest/skills

Frontmatter-based controls (`disable-model-invocation`, `user-invocable`) edit the
shared SKILL.md, so they apply to every agent that reads that same folder or symlink
target. Config-based controls (Codex, OpenCode) are per agent.

### Other Claude Code frontmatter fields

`allowed-tools`, `disallowed-tools`, `context: fork`, `agent`, `background`, `paths`
(globs that gate auto-loading), `model`, `effort`, `argument-hint`, `arguments`,
`hooks`, `shell`, `when_to_use`, `metadata`, `license`, `compatibility`.

### Parking (Skill Studio's disable for agents without a native switch)

Agents discover skills purely by directory presence. Skill Studio disables a skill
for Claude Code or pi by moving the folder or symlink to
`~/.agents/.disabled-skills/<agent>/<scope>/<name>` and moves it back on enable.
Parked skills are listed as disabled in the scanner.

## Discovery paths

| Agent       | Project                               | Global                                         | Notes                                                                                                           |
| ----------- | ------------------------------------- | ---------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| Claude Code | `.claude/skills/`                     | `~/.claude/skills/`                            | also plugin cache `~/.claude/plugins/cache`; nested `.claude/skills/` in subdirs                                |
| Codex       | `.codex/skills/`                      | `~/.codex/skills/`                             | also plugin cache `~/.codex/plugins/cache`                                                                      |
| OpenCode    | `.opencode/skills/` (legacy `skill/`) | `~/.config/opencode/skills/` (legacy `skill/`) | walks up to the git worktree root                                                                               |
| pi          | `.pi/skills/`                         | `~/.pi/agent/skills/`                          | root `.md` files with valid frontmatter count too                                                               |
| Cursor      | `.cursor/skills/`                     | `~/.cursor/skills/`                            | also reads .agents, .claude and .codex skill dirs; plugins in ~/.cursor/plugins/{cache,local}                   |
| Grok Build  | `.grok/skills/`                       | `~/.grok/skills/`                              | reads ~/.agents/skills; plugins in ~/.grok/plugins, marketplaces in ~/.grok/config.toml [[marketplace.sources]] |
| shared      | `.agents/skills/`                     | `~/.agents/skills/`                            | `npx skills` target; Codex, OpenCode, pi, Cursor and Grok Build read it natively; Claude Code needs a symlink   |

## Local data sources Skill Studio reads

| Purpose                         | Location                             | Shape                                                                                                                                                  |
| ------------------------------- | ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Installed-skill lock            | `~/.agents/.skill-lock.json`         | `{version, skills: {name: {source, sourceType, sourceUrl, skillFolderHash, installedAt, updatedAt}}}`                                                  |
| dotagents declared skills       | `~/.agents/agents.toml`              | `[[skills]]` rows: `{name, source, path, ref?}` - `ref` absent means unpinned; no `[[skills]]` row for a lock entry means a wildcard (`--all`) install |
| dotagents resolved skills       | `~/.agents/agents.lock`              | `[skills.<name>]` tables: `{source, resolved_path, resolved_commit}` - the commit actually on disk                                                     |
| Skill invocations (Claude Code) | `~/.claude/projects/*/*.jsonl`       | assistant `tool_use` with `"name":"Skill","input":{"skill":"<name>"}` + timestamp + `cwd`                                                              |
| Projects list (Codex)           | `~/.codex/config.toml`               | `[projects."/abs/path"]` sections                                                                                                                      |
| Projects list (Claude Code)     | `~/.claude/projects/<encoded-path>/` | `cwd` field inside the transcripts (dir name encoding is lossy)                                                                                        |
| skills.sh search                | `https://skills.sh/api/search?q=`    | `{skills: [{id, skillId, name, installs, source}]}` (`source`, not `topSource`)                                                                        |

Codex session logs mention every installed SKILL.md path on every turn (the skill
list in the instructions), so they are **not** an invocation signal. OpenCode keeps
sessions in `~/.local/share/opencode/opencode.db` (SQLite); pi in
`~/.pi/agent/sessions/**/*.jsonl` (`toolCall` records with `cwd`).

## Update check

`src-tauri/src/skills/skill_update_check.rs` compares each global-scope,
GitHub-backed skill's installed commit against the newest commit `gh api`
reports for its path, on a 6-hour timer plus a manual "Check now" (Issues
view). For a dotagents skill the installed commit comes straight from
`agents.lock`; for a skills.sh skill it's the newest commit at or before the
lock entry's `updatedAt` (cached until `updatedAt` changes, so a lock entry
that hasn't moved never re-queries its baseline). Results persist at
`<app data dir>/skill-studio/update-check.json` so a full snapshot rebuild can
read them without shelling out. Access is read-only (`gh api repos/.../commits`)
through the user's own `gh` CLI login; the app stores no tokens itself, and
"Update" runs the tool that owns the skill (`npx @sentry/dotagents add|install`
or `npx skills update`) rather than writing to `agents.toml`, `agents.lock`, or
`.skill-lock.json` directly.

## Headless runs

Skill Studio's local harness runner (`src-tauri/src/skills/skill_agent_runner.rs`)
starts each harness as a one-shot subprocess and parses its streaming JSON-lines
stdout. Binaries are resolved with `$SHELL -lc 'command -v <bin>'` (fallback
`/bin/zsh`) and cached per harness, since the app's own `PATH` doesn't see the
user's shell config.

- **Claude Code**: `claude -p "<prompt>" --output-format stream-json --verbose
--permission-mode <mode> [--resume <id>]`. Lines:
  `{"type":"system","subtype":"init",...,"session_id"}`;
  `{"type":"assistant","message":{"content":[{"type":"text","text":...} |
{"type":"tool_use","name":"Skill","input":{"skill":"say-banana"}} |
{"type":"thinking",...}]}}`;
  `{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":...,"content":"Launching skill: say-banana"}]}}`;
  `{"type":"result","subtype":"success","is_error":false,"result":"BANANA","session_id":"…","total_cost_usd":0.061,"duration_ms":…}`.
  Ignore `system/hook_*`, `rate_limit_event`, `thinking`. Skill loaded = a
  `tool_use` named `Skill` whose `input.skill` equals the skill name.
- **Codex**: `codex exec --json --skip-git-repo-check -s <read-only|workspace-write>
[-C <cwd>] "<prompt>"`; resume: `codex exec resume <thread_id> --json "<prompt>"`
  (no `-C`; set the process cwd instead). No `-a` flag. Lines:
  `{"type":"thread.started","thread_id":"…"}`, `turn.started`,
  `{"type":"item.started"|"item.completed","item":{"id","type":"agent_message","text"}
| {"type":"command_execution","command","aggregated_output","exit_code"} |
{"type":"reasoning"} | {"type":"file_change",...} | {"type":"error","message"}}`,
  `{"type":"turn.completed","usage":{...}}`, `turn.failed`. Skill loaded = any
  `command_execution.command` containing `/<skill-name>/SKILL.md`, else unknown.
  Final text = last completed `agent_message`.
- **pi**: `pi -p --mode json "<prompt>"` (cwd = process cwd; resume `--session <id>`).
  Lines: `{"type":"session","id":"…","cwd"}`, `message_update` with
  `assistantMessageEvent.type=="text_delta"` and `.delta`, `tool_execution_start`
  `{toolName, args}` / `tool_execution_end` `{toolName, result}`, `turn_end` with
  `message.content[]` (text blocks), `agent_end`, `agent_settled`. Skill loaded = a
  `read` tool whose `args.path` ends with or contains `/<skill-name>/SKILL.md`, else
  unknown. Final text = concatenated text blocks of the last `turn_end`.
- **OpenCode**: `opencode2 run --standalone --format json --auto [--session <id>]
"<prompt>"` (binary is `opencode2`, the v2 CLI; no `--dir` flag, cwd is the process
  cwd; `--standalone` bypasses the shared `opencode2 serve --service` background
  service, which hangs every run with no output when wedged). JSONL `type` values:
  `step_start | text | tool | step_finish`, each carrying a `part`. Session id comes
  from the top-level `sessionID`, first seen. `type=="text"` → assistant text from
  `part.text`. `type=="tool"` (`part.type=="tool"`) → one tool call per
  `part.callID`/`part.id`, deduped across the CLI's pending/running/completed
  re-prints of the same part; skill loaded = `part.tool=="skill"` naming this run's
  skill, or `part.tool=="read"` of that skill's `SKILL.md`. `type=="step_finish"` →
  `part.cost`/`part.tokens`, best effort, feeds the run's `Finished` cost. Read-only
  is not enforced for OpenCode: nine live probes of v0.0.0-beta-17498 found the CLI
  ignores `OPENCODE_CONFIG`, and cwd-scoped permission denies for `edit`/`write`/
  `patch`/`multiedit`/`apply_patch` never blocked `patch` from creating a file, so
  every OpenCode run gets workspace write access regardless of the requested mode.

Every run emits one `SkillAgentEvent` per parsed line (or per lifecycle step) on
`"skill-agent://event"`, and always exactly one terminating `Finished` event, even
on cancellation or a crash before any output.
