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

| Agent       | Explicit invocation | Restrict model auto-invoke                                                                     | Disable a skill (keep on disk)                                                                            |
| ----------- | ------------------- | ---------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| Claude Code | `/name [args]`      | frontmatter `disable-model-invocation: true` (user only); `user-invocable: false` (model only) | no native per-skill switch → Skill Studio removes its per-skill symlink, or parks it globally (see below) |
| Codex       | `$name`             | sidecar `agents/openai.yaml` → `policy.allow_implicit_invocation: false`                       | `~/.codex/config.toml` → `[[skills.config]] path = "…/SKILL.md" enabled = false`                          |
| OpenCode    | `skill` tool        | `permission.skill` in `opencode.json`: per-name pattern `allow` / `deny` / `ask`               | same: `"name": "deny"` (wildcards allowed, e.g. `internal-*`)                                             |
| pi          | `/skill:name`       | frontmatter `disable-model-invocation: true`                                                   | no per-skill disable → Skill Studio parks the folder globally                                             |
| Cursor      | unknown             | unknown                                                                                        | no per-skill disable → Skill Studio parks the folder globally                                             |
| Grok Build  | unknown             | unknown                                                                                        | no per-skill disable → Skill Studio parks the folder globally                                             |

Sources: https://code.claude.com/docs/en/skills · https://developers.openai.com/codex/skills ·
https://opencode.ai/docs/skills/ · https://pi.dev/docs/latest/skills

Frontmatter-based controls (`disable-model-invocation`, `user-invocable`) edit the
shared SKILL.md, so they apply to every agent that reads that same folder or symlink
target. Config-based controls (Codex, OpenCode) are per agent.

### Other Claude Code frontmatter fields

`allowed-tools`, `disallowed-tools`, `context: fork`, `agent`, `background`, `paths`
(globs that gate auto-loading), `model`, `effort`, `argument-hint`, `arguments`,
`hooks`, `shell`, `when_to_use`, `metadata`, `license`, `compatibility`.

### Parking (disable globally, for every harness)

Parking is Skill Studio's own global disable: it moves a skill's shared-folder
deployment from `~/.agents/skills/<name>` to `~/.agents/skills-parked/<name>`
(a rename, not a copy), removing a per-skill Claude Code symlink first if one
exists. Every harness that reads the shared folder loses the skill at once;
unparking reverses both steps. The registry (`~/.agents/skill-studio.json`)
records `parked: {name: {parked_at, source_kind, claude_link}}` so unparking
knows whether to recreate a Claude Code link.

Because parking only moves the shared folder, a `sync`/`update`/reinstall run
while a skill is parked can recreate `~/.agents/skills/<name>` on its own -
Skill Studio still shows the skill as parked (from the registry record), but
flags it as `parked-but-reinstalled` until it's unparked. Unparking then
reconciles the two copies: if the reinstalled copy is byte-identical to the
parked one, the parked copy is simply discarded; otherwise it's moved to
`~/.agents/skills-trash/<name>-<timestamp>` rather than silently overwriting
either copy.

### Per-harness disable (one harness at a time)

Separately from parking, a single harness can be turned off for one skill
while every other harness keeps seeing it, via that harness's own mechanism:

- **Codex**: `~/.codex/config.toml` → `[[skills.config]] path = "…/SKILL.md" enabled = false`,
  matched by the skill's canonical SKILL.md path. Skill Studio edits this with
  `toml_edit`, preserving all other content and comments byte-for-byte.
- **OpenCode**: `~/.config/opencode/opencode.json` → `permission.skill.<name-or-glob> = "deny"`.
  Skill Studio refuses to parse or write `opencode.jsonc`; a project with only
  that file is reported as unreadable rather than risking a write that drops
  comments.
- **Claude Code**: has no native per-skill switch, so Skill Studio tracks it in
  the registry's own `harness_disabled: {name: {"claude-code": {link_target}}}`
  map, and disables by removing (then later restoring) the skill's per-skill
  symlink under `~/.claude/skills/<name>`. This only works when the skill is
  deployed via that per-skill symlink; a skill deployed through the whole-directory
  `~/.claude/skills → ~/.agents/skills` symlink has no per-skill link to remove.
- **pi, Cursor, Grok Build**: no per-skill disable - these agents read the
  shared folder directly, so the only way to disable a skill for them is to
  park it (above), which disables it for every harness.

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
| parked      | n/a (global only)                     | `~/.agents/skills-parked/`                     | Skill Studio's own root for parked (disabled globally) skills - see "Parking" above; excluded from coverage     |

## Local data sources Skill Studio reads

| Purpose                            | Location                             | Shape                                                                                                                                                                                                                                                                                               |
| ---------------------------------- | ------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Installed-skill lock               | `~/.agents/.skill-lock.json`         | `{version, skills: {name: {source, sourceType, sourceUrl, skillFolderHash, installedAt, updatedAt}}}`                                                                                                                                                                                               |
| dotagents declared skills          | `~/.agents/agents.toml`              | `[[skills]]` rows: `{name, source, path, ref?}` - `ref` absent means unpinned; no `[[skills]]` row for a lock entry means a wildcard (`--all`) install                                                                                                                                              |
| dotagents resolved skills          | `~/.agents/agents.lock`              | `[skills.<name>]` tables: `{source, resolved_path, resolved_commit}` - the commit actually on disk                                                                                                                                                                                                  |
| Fork registry (Skill Studio-owned) | `~/.agents/skill-studio.json`        | `{version, forks: {name: {forked_at, origin_tool, origin_source, repo, path, declared_ref, base_commit}}, trials: {name: {started_at, expires_at, method, scope, project_path}}, parked: {name: {parked_at, source_kind, claude_link?}}, harness_disabled: {name: {"claude-code": {link_target}}}}` |
| Skill invocations (Claude Code)    | `~/.claude/projects/*/*.jsonl`       | assistant `tool_use` with `"name":"Skill","input":{"skill":"<name>"}` + timestamp + `cwd`                                                                                                                                                                                                           |
| Projects list (Codex)              | `~/.codex/config.toml`               | `[projects."/abs/path"]` sections                                                                                                                                                                                                                                                                   |
| Projects list (Claude Code)        | `~/.claude/projects/<encoded-path>/` | `cwd` field inside the transcripts (dir name encoding is lossy)                                                                                                                                                                                                                                     |
| skills.sh search                   | `https://skills.sh/api/search?q=`    | `{skills: [{id, skillId, name, installs, source}]}` (`source`, not `topSource`)                                                                                                                                                                                                                     |

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

## Fork / Pull upstream / Un-fork

A dotagents or skills.sh skill's local edits don't survive that CLI's own
lifecycle: dotagents `install` overwrites the folder outright (`sync`
preserves edits, but `install`/re-adding does not), and a folder `sync`
finds with neither a `[[skills]]` row nor a lock table gets silently adopted
as `source = "path:.agents/skills/<name>"` rather than kept as the skill the
user meant to edit. "Fork" (`src-tauri/src/skills/skill_fork.rs`) detaches a
skill from its owning ledger so edits stick: it snapshots the current folder,
removes the skill from its ledger (`npx @sentry/dotagents remove` or
`npx skills remove`), restores the folder if the removal deleted it, and
records a `ForkRecord` in `~/.agents/skill-studio.json`. Wildcard dotagents
entries (`name = "*"`, no per-skill manifest row) are refused in v1 - forking
one by name first requires adding a named row for it, which dotagents
doesn't offer without a fresh `add`.

"Pull upstream" three-way merges the skill's last-synced snapshot (`base`),
its current on-disk copy (`mine`), and a freshly fetched upstream copy at the
latest commit (`theirs`), file by file: unchanged-in-mine takes theirs;
unchanged-in-theirs keeps mine; a text file that differs on all three sides
runs through `git merge-file -p mine base theirs`, with its conflict markers
kept in place for the user to resolve in the editor; a binary file that
differs on all three sides keeps mine and is flagged. Files added or removed
upstream are added or removed locally when the local copy hadn't diverged.
The snapshot always advances to the new upstream commit afterward, even when
there were conflicts, so the fork's `base_commit` stays a true "last pulled"
marker. The upstream copy is fetched read-only via
`gh api repos/{owner}/{repo}/tarball/<sha>`, extracted with `tar`, and never
writes back to GitHub or to the owning CLI's own files.

"Un-fork" discards local edits and reinstalls the skill from its recorded
origin (`declared_ref`, if any, for a dotagents fork), then drops the fork
record and snapshot - the frontend confirms this destructively first.

## Add skill

The "Add skill" sheet (`src/components/AddSkill/AddSkillSheet.tsx`, backend
`skills/skill_add.rs`) accepts a free-text source - `owner/repo`,
`owner/repo/<path>`, a `github.com` URL (bare, `/tree/<ref>/<path>`, or
`/blob/<ref>/<path>/SKILL.md`), a `skills.sh/<owner>/<repo>/<skill>` URL,
`git:<url>` or a bare `*.git` URL, or an absolute/`~/` local path - parsed by
`src/lib/skill-source-parse.ts` into a `ParsedSkillSource`. One of three
methods installs it: **dotagents** (`npx -y @sentry/dotagents add`, tracked in
`agents.toml`/`agents.lock`), **skills.sh** (the existing `install_skill`
path, tracked in `.skill-lock.json`; github sources only), or **Copy**
(fetches a GitHub tarball or copies a local path straight into
`~/.agents/skills/<name>`, untracked - no ledger, no updates). Every method
writes only the shared folder; Claude Code reads it through `~/.claude/skills`
the same way a whole-dir symlink normally provides, so when that path is a
real directory (not the whole-dir symlink) and Claude Code is a selected
harness, `add_skill` creates one relative per-skill symlink
`~/.claude/skills/<name> -> ../../.agents/skills/<name>` - never for
`skills.sh`, since its own `--agent claude-code` deploys straight into
Claude Code's directory.

## Packs

A share pack (`src-tauri/src/skills/skill_pack.rs`) bundles a chosen set of
skill deployments into one dotagents-compatible repo under
`~/.agents/packs/<name>/`, for handing to another machine or another person.
Selecting rows in any `SkillListTable` and clicking "Create pack" in the
selection bar (`src/store/appStore.ts`'s `selectedSkillPaths`, keyed by each
row's deployment path) builds:

- `skills/<name>/` - a full bundled copy of **every** member, bundled from
  its exact selected deployment path - even one managed by dotagents,
  skills.sh, or a fork, so the pack still works if the origin repo moves or
  disappears.
- `agents.toml` - a `[[skills]]` row for provenance on every **managed**
  member (dotagents, skills.sh, or fork - only when its path is the shared
  `~/.agents/skills/<name>` root; a project deployment or plugin-cache copy
  is always treated as manual), with `source`, `path`, and `ref` (the fork's
  or skills.sh's resolved `installed_commit`, falling back to a dotagents
  declared ref, or omitted for an unpinned/wildcard entry).
- `README.md` - generated install instructions for both `npx -y
@sentry/dotagents add <owner>/<repo> --all` and `npx skills add
<owner>/<repo>`.

`create_skill_pack`/`update_skill_pack` commit the tree with `git`
locally only; `update_skill_pack` rebuilds from the pack's already-recorded
members and only commits when the tree actually changed. **Publishing is
never automatic**: `publish_skill_pack` confirms with a native
`tauri_plugin_dialog` message box (`PublishConfirm`) right before it shells
out to `gh repo create ... --push` (first publish) or `git push origin HEAD`
(every publish after `repo` is recorded) - the app never creates a repo or
pushes on its own, and a cancelled dialog returns `Err("Publish cancelled")`
before any `gh`/`git` call. `delete_skill_pack` only removes the local
registry entry and directory; it never touches GitHub.

Importing a pack (the Add-skill sheet's "Pack" method, shown for GitHub
sources) reads the repo's `agents.toml` read-only via `gh api -H "Accept:
application/vnd.github.raw" repos/<owner>/<repo>/contents/agents.toml`, then
validates every `[[skills]]` row (name, source, path, ref) before running any
command - one invalid row, or more than 200 rows, refuses the whole import
with nothing installed. It then runs `npx -y @sentry/dotagents add
<owner>/<repo> --all` for the bundled `skills/` tree, and only then one
`dotagents add <source> --name <name> --ref <ref>` per remaining `[[skills]]`
row - skipping any row whose name `--all` already bundled, since a pack now
bundles every managed member alongside its row. A row that fails to resolve
is reported but doesn't abort the rest of the import. None of this ever edits
`~/.agents/agents.toml`, `agents.lock`, or `.skill-lock.json` directly - a
pack only ever writes its own generated `agents.toml` under
`~/.agents/packs/<name>/`, and imports go through the same CLIs "Add skill"
already uses.

## Trials

Checking "Try for 24 hours" on the Add-skill sheet records a `TrialRecord` in
`~/.agents/skill-studio.json`'s `trials` map (`started_at`, `expires_at`
24 h later, `method`, `scope`, `project_path`). A background loop
(`skill_trial::spawn_trial_expiry_loop`, 15 s after startup then every 5 min)
copies each expired trial's folder to
`~/.agents/skills-trash/<name>-<YYYYMMDD-HHMMSS>/` **before** removing it
through its owning tool (or deleting it directly for Copy) - a failing
removal never loses the folder, since the trash copy already exists and the
trial record is kept for the next tick. The skill page's "Keep" button
(`keep_skill_trial`) just drops the trial record. Restoring a trashed skill
(`restore_trashed_skill`, wired to the "Restore" action on the
`skills://trial-expired` toast) copies it back to `~/.agents/skills/<name>` as
an untracked, manual skill and re-applies the Claude Code symlink rule.
Removing, un-forking, or forking a trial skill also drops its trial record.

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
