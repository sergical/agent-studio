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

Agent Studio enforces these rules in `src-tauri/src/skills/frontmatter.rs`
(`validate_skill`) and reports failures as `spec_violations`.

## Invocation control and disable, per agent

All four agents auto-invoke a skill by default when its description matches the task.

| Agent       | Explicit invocation | Restrict model auto-invoke                                                                     | Disable a skill (keep on disk)                                                   |
| ----------- | ------------------- | ---------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| Claude Code | `/name [args]`      | frontmatter `disable-model-invocation: true` (user only); `user-invocable: false` (model only) | none native → Agent Studio parks the folder (see below)                          |
| Codex       | `$name`             | sidecar `agents/openai.yaml` → `policy.allow_implicit_invocation: false`                       | `~/.codex/config.toml` → `[[skills.config]] path = "…/SKILL.md" enabled = false` |
| OpenCode    | `skill` tool        | `permission.skill` in `opencode.json`: per-name pattern `allow` / `deny` / `ask`               | same: `"name": "deny"` (wildcards allowed, e.g. `internal-*`)                    |
| pi          | `/skill:name`       | frontmatter `disable-model-invocation: true`                                                   | none native → Agent Studio parks the folder                                      |

Sources: https://code.claude.com/docs/en/skills · https://developers.openai.com/codex/skills ·
https://opencode.ai/docs/skills/ · https://pi.dev/docs/latest/skills

Frontmatter-based controls (`disable-model-invocation`, `user-invocable`) edit the
shared SKILL.md, so they apply to every agent that reads that same folder or symlink
target. Config-based controls (Codex, OpenCode) are per agent.

### Other Claude Code frontmatter fields

`allowed-tools`, `disallowed-tools`, `context: fork`, `agent`, `background`, `paths`
(globs that gate auto-loading), `model`, `effort`, `argument-hint`, `arguments`,
`hooks`, `shell`, `when_to_use`, `metadata`, `license`, `compatibility`.

### Parking (Agent Studio's disable for agents without a native switch)

Agents discover skills purely by directory presence. Agent Studio disables a skill
for Claude Code or pi by moving the folder or symlink to
`~/.agents/.disabled-skills/<agent>/<scope>/<name>` and moves it back on enable.
Parked skills are listed as disabled in the scanner.

## Discovery paths

| Agent       | Project                               | Global                                         | Notes                                                                                  |
| ----------- | ------------------------------------- | ---------------------------------------------- | -------------------------------------------------------------------------------------- |
| Claude Code | `.claude/skills/`                     | `~/.claude/skills/`                            | also plugin cache `~/.claude/plugins/cache`; nested `.claude/skills/` in subdirs       |
| Codex       | `.codex/skills/`                      | `~/.codex/skills/`                             | also plugin cache `~/.codex/plugins/cache`                                             |
| OpenCode    | `.opencode/skills/` (legacy `skill/`) | `~/.config/opencode/skills/` (legacy `skill/`) | walks up to the git worktree root                                                      |
| pi          | `.pi/skills/`                         | `~/.pi/agent/skills/`                          | root `.md` files with valid frontmatter count too                                      |
| shared      | `.agents/skills/`                     | `~/.agents/skills/`                            | `npx skills` target; Codex, OpenCode, pi read it natively; Claude Code needs a symlink |

## Local data sources Agent Studio reads

| Purpose                         | Location                             | Shape                                                                                                 |
| ------------------------------- | ------------------------------------ | ----------------------------------------------------------------------------------------------------- |
| Installed-skill lock            | `~/.agents/.skill-lock.json`         | `{version, skills: {name: {source, sourceType, sourceUrl, skillFolderHash, installedAt, updatedAt}}}` |
| Skill invocations (Claude Code) | `~/.claude/projects/*/*.jsonl`       | assistant `tool_use` with `"name":"Skill","input":{"skill":"<name>"}` + timestamp + `cwd`             |
| Projects list (Codex)           | `~/.codex/config.toml`               | `[projects."/abs/path"]` sections                                                                     |
| Projects list (Claude Code)     | `~/.claude/projects/<encoded-path>/` | `cwd` field inside the transcripts (dir name encoding is lossy)                                       |
| skills.sh search                | `https://skills.sh/api/search?q=`    | `{skills: [{id, skillId, name, installs, source}]}` (`source`, not `topSource`)                       |

Codex session logs mention every installed SKILL.md path on every turn (the skill
list in the instructions), so they are **not** an invocation signal. OpenCode keeps
sessions in `~/.local/share/opencode/opencode.db` (SQLite); pi in
`~/.pi/agent/sessions/**/*.jsonl` (`toolCall` records with `cwd`).
