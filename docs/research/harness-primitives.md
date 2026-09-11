# Harness skill and plugin primitives

Research date: 2026-09-09. This document records what each harness supports for
skills and plugins. Each fact has a source and a confidence level.

| Level                | Meaning                                                         |
| -------------------- | --------------------------------------------------------------- |
| `verified-from-docs` | Read directly from the vendor's documentation or repository.    |
| `inferred`           | Derived from a secondary source, an issue thread, or a summary. |
| `unknown`            | Not found in any primary source. Treat as unverified.           |

Line references to `docs/agent-skill-conventions.md` are from the worktree
`/Users/sergiydybskiy/src/agent-studio/.claude/worktrees/shared-core-primitives`.

## 1. Agent Skills specification (agentskills.io)

The canonical spec is at https://agentskills.io/specification. The
`anthropics/skills` repo no longer hosts the spec text. Its
`spec/agent-skills-spec.md` reads only "The spec is now located at
https://agentskills.io/specification"
(https://raw.githubusercontent.com/anthropics/skills/main/spec/agent-skills-spec.md).
The repo is a reference implementation with 50+ example skills, a `/template`
skill, and a `skills-ref` validator. The spec is governed at
github.com/agentskills/agentskills (https://agentskills.io, verified-from-docs).

### Roots

| Item             | Fact                                                                          | Source                               | Confidence         |
| ---------------- | ----------------------------------------------------------------------------- | ------------------------------------ | ------------------ |
| Skill directory  | A directory with at least a `SKILL.md`.                                       | https://agentskills.io/specification | verified-from-docs |
| Optional folders | `scripts/` (code), `references/` (docs), `assets/` (templates, images, data). | https://agentskills.io/specification | verified-from-docs |
| Other files      | Any other files and directories are allowed.                                  | https://agentskills.io/specification | verified-from-docs |
| Root locations   | The spec defines no filesystem roots. Roots are harness-specific.             | https://agentskills.io/specification | inferred           |

### Discovery

The spec defines progressive disclosure in three stages
(https://agentskills.io, verified-from-docs):

1. Discovery. At startup the agent loads only `name` and `description` for
   every skill (about 100 tokens each).
2. Activation. When a task matches, the full `SKILL.md` body loads.
3. Execution. The agent follows the instructions. It can run bundled scripts
   or load referenced files.

Authoring guidance (https://agentskills.io/specification, verified-from-docs):

- Keep the `SKILL.md` body under about 5000 tokens and under 500 lines.
- Move detail to `references/` files.
- Keep file references one level deep from `SKILL.md`.
- Use relative paths from the skill root.

### Frontmatter

| Field           | Rule                                                                                                                                           | Source                               | Confidence         |
| --------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------ | ------------------ |
| `name`          | Required. 1-64 chars. Lowercase `a-z`, `0-9`, and hyphens. No leading, trailing, or consecutive hyphens. Must match the parent directory name. | https://agentskills.io/specification | verified-from-docs |
| `description`   | Required. 1-1024 chars. Should state what the skill does and when to use it. Should include keywords for matching.                             | https://agentskills.io/specification | verified-from-docs |
| `license`       | Optional. A license name or a reference to a bundled license file.                                                                             | https://agentskills.io/specification | verified-from-docs |
| `compatibility` | Optional. 1-500 chars. Describes environment requirements. The spec says most skills do not need it.                                           | https://agentskills.io/specification | verified-from-docs |
| `metadata`      | Optional. A map of string keys to string values for client-defined properties. Use unique key names.                                           | https://agentskills.io/specification | verified-from-docs |
| `allowed-tools` | Optional. Space-separated string of pre-approved tools, for example `Bash(git:*) Bash(jq:*) Read`. Experimental. Support varies by agent.      | https://agentskills.io/specification | verified-from-docs |

### Invocation control

The spec defines no invocation-control fields. Fields such as
`disable-model-invocation` and `user-invocable` are harness extensions
(https://agentskills.io/specification, inferred from the absence of such fields
in the frontmatter table).

### Disable

The spec defines no disable mechanism. Unknown beyond that.

### Plugins

The spec defines no plugin format. The `anthropics/skills` repo is a reference
implementation only (https://github.com/anthropics/skills, verified-from-docs).

### Config files

The spec defines no config file. It recommends the `skills-ref` validator:
`skills-ref validate ./my-skill`. The validator lives at
github.com/agentskills/agentskills/tree/main/skills-ref
(https://agentskills.io/specification, verified-from-docs).

### Versioning

The spec defines no version field. The only version example is a free-form
string under `metadata` (`metadata: {version: "1.0"}`), which is
client-defined (https://agentskills.io/specification, verified-from-docs).

### Observation of usage

The spec defines no usage observation. Unknown.

## 2. Claude Code

Sources: https://code.claude.com/docs/en/skills,
https://code.claude.com/docs/en/plugins,
https://code.claude.com/docs/en/plugins-reference,
https://code.claude.com/docs/en/discover-plugins.

### Roots

Skill discovery priority, highest first
(https://code.claude.com/docs/en/skills, verified-from-docs):

| Priority | Root                                                         | Note                                                                            |
| -------- | ------------------------------------------------------------ | ------------------------------------------------------------------------------- |
| 1        | Enterprise managed settings `.claude/skills/<name>/SKILL.md` |                                                                                 |
| 2        | Personal `~/.claude/skills/<name>/SKILL.md`                  |                                                                                 |
| 3        | Project `.claude/skills/<name>/SKILL.md`                     | Startup directory and all parent directories up to the repo root.               |
| 4        | Nested `<subdir>/.claude/skills/<name>/SKILL.md`             | Loads on first file access in that subdirectory, or via `/add-dir` (v2.1.257+). |
| 5        | `--add-dir` directories                                      |                                                                                 |
| 6        | Plugin `<plugin>/skills/<name>/SKILL.md`                     | Invoked as `/plugin-name:skill-name`.                                           |
| 7        | claude.ai account-synced skills                              | Download to `~/.claude/skills/synced/` when `CLAUDE_CODE_SYNC_SKILLS=1`.        |

Synced skills are skipped on a name conflict with any local command. They never
run shell injection or path substitution locally
(https://code.claude.com/docs/en/skills, verified-from-docs).

### Discovery

| Fact                                                                                                                                                                                                                                                                  | Source                                                                                     | Confidence         |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------ | ------------------ |
| Plugin skills are namespaced `/plugin-name:skill-name`. The bare `/skill-name` also works if no other skill has that name. The prefix is `name` in `plugin.json`, which can differ from the marketplace entry name.                                                   | https://code.claude.com/docs/en/plugins, https://code.claude.com/docs/en/discover-plugins  | verified-from-docs |
| A plugin with one skill can put `SKILL.md` at the plugin root. The invocation name comes from frontmatter `name`.                                                                                                                                                     | https://code.claude.com/docs/en/plugins                                                    | verified-from-docs |
| A folder in `~/.claude/skills/` or `.claude/skills/` that contains `.claude-plugin/plugin.json` auto-loads as a plugin named `<name>@skills-dir`. Project scope requires workspace trust. `claude plugin init <name>` scaffolds one under `~/.claude/skills/<name>/`. | https://code.claude.com/docs/en/plugins, https://code.claude.com/docs/en/plugins-reference | verified-from-docs |
| `/reload-plugins` applies plugin, skill, hook, and agent changes without a restart. Use `--force` if the reload would invalidate the prompt cache.                                                                                                                    | https://code.claude.com/docs/en/plugins, https://code.claude.com/docs/en/discover-plugins  | verified-from-docs |

### Frontmatter

All fields are optional. Only `description` is recommended. Full field set:
`name`, `description`, `when_to_use`, `argument-hint`, `arguments`,
`disable-model-invocation`, `user-invocable`, `model`, `effort`, `context`,
`agent`, `background`, `shell`, `allowed-tools`, `disallowed-tools`, `paths`,
`hooks`, `metadata`, `license`, `compatibility`
(https://code.claude.com/docs/en/skills, verified-from-docs).

`allowed-tools` and `disallowed-tools` apply only for the invocation turn. They
clear after the next user message. The skill content stays in context
(https://code.claude.com/docs/en/skills, verified-from-docs).

### Invocation control

| Field                            | Default | Effect                                                                                                                                                          | Source                                 | Confidence         |
| -------------------------------- | ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------- | ------------------ |
| `disable-model-invocation: true` | false   | Claude cannot auto-invoke. The description is not loaded into context, not preloaded into subagents, and not run by scheduled tasks. `/skill-name` still works. | https://code.claude.com/docs/en/skills | verified-from-docs |
| `user-invocable: false`          | true    | Hides the skill from the `/` menu. Typing `/skill-name` errors. Claude can still auto-invoke. Documented as a UI-only setting.                                  | https://code.claude.com/docs/en/skills | verified-from-docs |

### Disable

| Mechanism                                                                                                                     | Effect                                                                                                                                                 | Source                                            | Confidence         |
| ----------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------- | ------------------ |
| `settings.json` `skillOverrides: {"<name>": "on" \| "name-only" \| "user-invocable-only" \| "off"}`                           | Native per-skill toggle without editing `SKILL.md`. `"off"` hides the skill from Claude and from the `/` menu. Works at user, project, or local scope. | https://code.claude.com/docs/en/skills            | verified-from-docs |
| `settings.json` `disableBundledSkills: true`                                                                                  | Disables all Anthropic-bundled skills except `/doctor`.                                                                                                | https://code.claude.com/docs/en/skills            | verified-from-docs |
| `settings.json` `disableSkillShellExecution: true`                                                                            | Disables `` !`command` `` shell injection in user, project, plugin, and additional-directory skills. Bundled and managed skills are not affected.      | https://code.claude.com/docs/en/skills            | verified-from-docs |
| `claude plugin disable my-tool@skills-dir`                                                                                    | Disables a skills-directory plugin. Deleting its directory also works.                                                                                 | https://code.claude.com/docs/en/plugins-reference | verified-from-docs |
| `/plugin disable plugin-name@marketplace-name`, `/plugin enable`, `/plugin uninstall`, `/plugin list [--enabled\|--disabled]` | Plugin-level toggles. Scripting form: `claude plugin install/uninstall --scope <user\|project\|local>`, which does not open the interactive panel.     | https://code.claude.com/docs/en/discover-plugins  | verified-from-docs |

### Plugins

| Fact                                                                                                                                                                                                                                                                                                                                        | Source                                                                                    | Confidence         |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- | ------------------ |
| Cache layout: `~/.claude/plugins/cache/<marketplace-name>/<plugin-name>/<version>/`. Each version has its own directory with `node_modules/` and `.claude-plugin/`.                                                                                                                                                                         | https://code.claude.com/docs/en/plugins-reference                                         | verified-from-docs |
| `~/.claude/plugins/data/<plugin-id>/` is persistent data that survives updates. `${CLAUDE_PLUGIN_DATA}` resolves to it. `{id}` is the plugin id with non-alphanumeric characters replaced by `-`.                                                                                                                                           | https://code.claude.com/docs/en/plugins-reference                                         | verified-from-docs |
| Orphaned cached versions are pruned about 14 days after they become unreferenced.                                                                                                                                                                                                                                                           | https://code.claude.com/docs/en/plugins-reference                                         | verified-from-docs |
| Plugin root layout: `skills/`, `commands/` (legacy flat), `agents/`, `hooks/hooks.json`, `.mcp.json`, `.lsp.json`, `monitors/monitors.json`, `bin/` (added to Bash PATH), `settings.json` (only `agent` and `subagentStatusLine` keys; takes priority over `settings` in `plugin.json`). Only `plugin.json` lives inside `.claude-plugin/`. | https://code.claude.com/docs/en/plugins                                                   | verified-from-docs |
| `plugin.json` fields: `name` (unique id and skill namespace), `description`, `version` (optional, gates update visibility), `author`, optional `homepage`, `repository`, `license`.                                                                                                                                                         | https://code.claude.com/docs/en/plugins                                                   | verified-from-docs |
| `marketplace.json` plugin entries: required `name`, `source`, `sourceUri`; optional `displayName`, `description`, `version`, `author`, `keywords`, `defaultEnabled`. `source` is one of `npm`, `archive`, `github`, `command`, `url`.                                                                                                       | https://code.claude.com/docs/en/plugins-reference                                         | verified-from-docs |
| Official marketplaces: `claude-plugins-official` (auto-added on first interactive launch; install as `<name>@claude-plugins-official`) and `claude-community` (added via `/plugin marketplace add anthropics/claude-plugins-community`; install as `<name>@claude-community`).                                                              | https://code.claude.com/docs/en/discover-plugins, https://code.claude.com/docs/en/plugins | verified-from-docs |
| `claude --plugin-dir ./my-plugin` (repeatable, accepts `.zip`) loads a local plugin for one session. It overrides a same-named installed plugin unless managed settings force enable or disable. `claude --plugin-url <url>` fetches a hosted zip for one session.                                                                          | https://code.claude.com/docs/en/plugins, https://code.claude.com/docs/en/discover-plugins | verified-from-docs |

### Config files

| Key in `settings.json`                                                 | Purpose                                                                                                                                                                                                                                | Source                                                                                              | Confidence         |
| ---------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- | ------------------ |
| `enabledPlugins`                                                       | Map of `"plugin-name"` or `"plugin-name@marketplace-name"` to boolean. A missing entry falls back to the manifest's `defaultEnabled`. Persists across updates and reinstalls. User, project, or local scope.                           | https://code.claude.com/docs/en/plugins-reference                                                   | verified-from-docs |
| `extraKnownMarketplaces`                                               | Registers extra marketplaces (name + url, or a `{source:{source:"github",repo:...}}` object for team config in project settings).                                                                                                      | https://code.claude.com/docs/en/plugins-reference, https://code.claude.com/docs/en/discover-plugins | verified-from-docs |
| `blockedMarketplaces`                                                  | Restricts allowed marketplace sources, for example `["untrusted-marketplace", {"source":"skills-dir"}, {"source":"command"}]`.                                                                                                         | https://code.claude.com/docs/en/plugins-reference                                                   | verified-from-docs |
| `pluginConfigs`                                                        | Non-sensitive plugin `userConfig` values. Sensitive values go to macOS Keychain (fallback `~/.claude/.credentials.json`) or that file on other platforms. Only user settings, `--settings`, and managed settings are read (v2.1.207+). | https://code.claude.com/docs/en/plugins-reference                                                   | verified-from-docs |
| `skillOverrides`, `disableBundledSkills`, `disableSkillShellExecution` | See Disable above.                                                                                                                                                                                                                     | https://code.claude.com/docs/en/skills                                                              | verified-from-docs |

### Observation of usage

The transcript JSONL shape for a Skill invocation
(`~/.claude/projects/*/*.jsonl`, assistant `tool_use` with
`{"name":"Skill","input":{"skill":"<name>"}}`) is claimed in
`docs/agent-skill-conventions.md:137` and `docs/agent-skill-conventions.md:327`.
No page on code.claude.com/docs documents this shape (searched skills, plugins,
plugins-reference, discover-plugins). Confidence: unknown. Treat it as inferred
from observed transcripts, not as a documented format.

## 3. OpenAI Codex CLI

Sources: https://developers.openai.com/codex/skills (redirects to
https://learn.chatgpt.com/docs/build-skills),
https://learn.chatgpt.com/docs/config-file/config-reference,
https://developers.openai.com/plugins/build/plugins,
https://learn.chatgpt.com/docs/plugins.

### Roots

Official docs describe an upward scan from the current working directory
(https://learn.chatgpt.com/docs/build-skills, verified-from-docs):

| Scope      | Root                        |
| ---------- | --------------------------- |
| Repo level | `$CWD/.agents/skills`       |
| Parent     | `$CWD/../.agents/skills`    |
| Repo root  | `$REPO_ROOT/.agents/skills` |
| User       | `$HOME/.agents/skills`      |
| Admin      | `/etc/codex/skills`         |

Codex follows symlinked skill folders when it scans (verified-from-docs).

Other claims about roots:

| Claim                                                                                                                              | Source                                         | Confidence |
| ---------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------- | ---------- |
| A feature request for `.codex/skills` and `~/.codex/skills` as Codex-specific roots was closed as not planned (per a web summary). | https://github.com/openai/codex/issues/22590   | inferred   |
| A third-party blog says bundled skills live at `~/.codex/skills/.system` and users can drop skills into `~/.codex/skills/`.        | https://blog.fsck.com/2025/12/19/codex-skills/ | unknown    |

The three sources disagree. Re-verify against the Codex CLI source (codex-rs)
before you trust any one of them.

### Discovery

`SKILL.md` requires `name` and `description`. Codex loads only metadata (name,
description, file path) at first, capped at about 8,000 characters. The full
`SKILL.md` loads when the model selects the skill
(https://learn.chatgpt.com/docs/build-skills, verified-from-docs).

Restart Codex after you change `config.toml` or install or update a skill.
Metadata reloads only on restart (verified-from-docs).

### Frontmatter

`name` and `description` are required (verified-from-docs). No other
frontmatter fields were found in the input. Unknown beyond that.

### Invocation control

| Mechanism                                                  | Effect                                                           | Source                                      | Confidence         |
| ---------------------------------------------------------- | ---------------------------------------------------------------- | ------------------------------------------- | ------------------ |
| `$skill-name` in Codex CLI (`@skill-name` in ChatGPT)      | Explicit invocation.                                             | https://learn.chatgpt.com/docs/build-skills | verified-from-docs |
| Implicit invocation                                        | The model matches user intent to a skill description.            | https://learn.chatgpt.com/docs/build-skills | verified-from-docs |
| `allow_implicit_invocation: false` in `agents/openai.yaml` | Disables automatic triggering. Explicit `$` mentions still work. | https://learn.chatgpt.com/docs/build-skills | verified-from-docs |

### Disable

Per-skill enable and disable uses repeated `[[skills.config]]` tables in
`config.toml`. Each table has `path` (string, folder that contains `SKILL.md`)
and `enabled` (boolean). This works at the global `~/.codex/config.toml` level
(https://learn.chatgpt.com/docs/build-skills, verified-from-docs).

Caveats:

| Issue                                                                          | Source                                       | Confidence         |
| ------------------------------------------------------------------------------ | -------------------------------------------- | ------------------ |
| Open request for project-local `.codex/config.toml` `skills.config` filtering. | https://github.com/openai/codex/issues/20210 | verified-from-docs |
| Report that `[[skills.config]]` is ignored inside sub-agent TOML overrides.    | https://github.com/openai/codex/issues/14161 | verified-from-docs |

### Plugins

| Fact                                                                                                                                                                                                                                                                                           | Source                                                     | Confidence         |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- | ------------------ |
| `plugin.json` uses `$schema: https://agent-plugins.org/schemas/1.0.0/plugin.schema.json`. Fields: `name` (kebab-case), `version`, `description`, `author`, `homepage`, `repository`, `license`, `keywords`. OpenAI settings live under `extensions.com.openai` (`apps`, `hooks`, `interface`). | https://developers.openai.com/plugins/build/plugins        | verified-from-docs |
| Cache layout: `~/.codex/plugins/cache/$MARKETPLACE_NAME/$PLUGIN_NAME/$VERSION/`. Local plugins use `$VERSION = "local"`.                                                                                                                                                                       | https://developers.openai.com/plugins/build/plugins        | verified-from-docs |
| `marketplace.json` declares `name`, `interface.displayName`, and `plugins[]`. Each entry has `source` (relative `./` path, git-subdir, or npm), `policy.installation` (`AVAILABLE`, `INSTALLED_BY_DEFAULT`), `policy.authentication` (`ON_INSTALL`, `ON_FIRST_USE`), `category`.               | https://developers.openai.com/plugins/build/plugins        | verified-from-docs |
| A default marketplace may live at `.agents/plugins/marketplace.json`.                                                                                                                                                                                                                          | https://github.com/openai/codex/issues/19382 (web summary) | inferred           |
| A plugin directory reportedly has `.codex-plugin/plugin.json` plus optional `skills/`, `.app.json`, `.mcp.json`, `agents/`, `commands/`, `hooks.json`, `assets/`.                                                                                                                              | Web search summary, no fetched URL                         | unknown            |
| Plugins can bundle skills, MCP servers, browser extensions, and hooks.                                                                                                                                                                                                                         | https://learn.chatgpt.com/docs/plugins                     | verified-from-docs |

### Config files

| Item                        | Fact                                                                                                                                 | Source                                                      | Confidence         |
| --------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------- | ------------------ |
| `CODEX_HOME`                | Defaults to `~/.codex`. Personal defaults in `~/.codex/config.toml`. Project overrides in `.codex/config.toml` of a trusted project. | https://learn.chatgpt.com/docs/config-file/config-reference | verified-from-docs |
| `skills.max_context_tokens` | Positive integer. Skills-catalog token budget. Default 2% of the model context window, capped at 10,000 tokens.                      | https://learn.chatgpt.com/docs/config-file/config-reference | verified-from-docs |
| `[[skills.config]]`         | See Disable above.                                                                                                                   | https://learn.chatgpt.com/docs/build-skills                 | verified-from-docs |

### Observation of usage

Not covered by the input. Unknown.

## 4. OpenCode

Sources: https://opencode.ai/v2/docs/skills/,
https://opencode.ai/v2/docs/migrate-v1/,
https://opencode.ai/v2/docs/build/plugins, v1
https://opencode.ai/docs/skills.md, https://opencode.ai/changelog.

v2 is beta as of 2026-09-09. It installs as a separate `opencode2` binary
(`npm i -g @opencode-ai/cli@next`), alongside the v1 `opencode` binary. v1
stable is 1.18.x; since 1.18.24 it reads the v2 config fields it supports
(https://opencode.ai/changelog, verified-from-docs).

### Roots

Precedence, low to high (https://opencode.ai/v2/docs/skills/,
verified-from-docs):

| Priority | Root                           | Note                                                         |
| -------- | ------------------------------ | ------------------------------------------------------------ |
| 1        | Built-in                       |                                                              |
| 2        | `.claude/skills` (compat)      | Nearest ancestor.                                            |
| 3        | `.agents/skills` (compat)      | Nearest ancestor.                                            |
| 4        | `~/.config/opencode/skills`    | Global.                                                      |
| 5        | Project `.opencode/skills`     | Walked root to cwd.                                          |
| 6        | Explicit `skills` config array | Relative paths, `~/`, or HTTP(S) catalogs with `index.json`. |

Global compat roots also include `~/.claude/skills` and `~/.agents/skills`.
Each source reads root-level `*.md` and nested `SKILL.md` at any depth
(https://opencode.ai/v2/docs/skills/, verified-from-docs).

v1 accepted `.opencode/skill/` or `.opencode/skills/`. v2 canonical is
`.opencode/skills/<skill-id>/SKILL.md`
(https://opencode.ai/v2/docs/migrate-v1/, verified-from-docs). Skill ids
derive from the file path (kebab-case, 1-64 chars), not from frontmatter
`name`.

Symlink following and hidden-entry handling: unknown (not documented).

### Discovery

`SKILL.md` metadata (name, description) loads first; the full body loads on
selection through the built-in `skill` tool
(https://opencode.ai/v2/docs/skills/, verified-from-docs).

### Frontmatter

Not covered by the input beyond `description` (required),
`opencode/autoinvoke`, and `slash` (see Invocation control). Unknown beyond
that.

### Invocation control

| Field                        | Effect                                                                                   | Source                              | Confidence         |
| ---------------------------- | ---------------------------------------------------------------------------------------- | ----------------------------------- | ------------------ |
| `opencode/autoinvoke: false` | Omits the skill from model discovery.                                                    | https://opencode.ai/v2/docs/skills/ | verified-from-docs |
| `slash: false`               | Hides the skill from the slash-command catalog. Skills are `/skill` commands by default. | https://opencode.ai/v2/docs/skills/ | verified-from-docs |
| `description`                | Required.                                                                                | https://opencode.ai/v2/docs/skills/ | verified-from-docs |
| v1 `permission.skill` `ask`  | Per-agent prompt before invocation.                                                      | https://opencode.ai/docs/skills.md  | verified-from-docs |

No fields named `user-invocable` or `disable-model-invocation` exist.
Permission overrides can also be scoped per agent in custom agent
frontmatter or built-in agent config in `opencode.json`. Different agents
can see different skill sets (https://opencode.ai/v2/docs/skills/,
verified-from-docs).

### Disable

| Mechanism                                                                                                                        | Effect                                                                                                                      | Source                                             | Confidence         |
| -------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------- | ------------------ |
| v2 permission rule `{ "action": "skill", "resource": "<skill-id>", "effect": "allow" \| "deny" \| "ask" }` in `opencode.json(c)` | Ordered rules; the first matching resource wins.                                                                            | https://opencode.ai/v2/docs/skills/                | verified-from-docs |
| v1 `permission.skill` map in `opencode.json`                                                                                     | Map of name patterns (wildcards like `internal-*`) to `allow`, `deny` (hidden, access rejected), or `ask`.                  | https://opencode.ai/docs/skills.md                 | verified-from-docs |
| `tools: { skill: false }` in agent frontmatter or `opencode.json` (v1)                                                           | Removes the skill tool and all skills from that agent. The `<available_skills>` section is omitted. Not re-confirmed in v2. | https://opencode.ai/docs/skills.md                 | verified-from-docs |
| Reported bug: `"*": "deny"` did not hide installed skills in one user's test.                                                    | Possible enforcement bug in some version.                                                                                   | https://github.com/anomalyco/opencode/issues/29727 | unknown            |

### Plugins

v2 plugins are a `package.json` with an `exports` map and an optional
`./rpc` export, depending on `@opencode/plugin` (beta). Discovery is
`.opencode/plugin/` and `.opencode/plugins/` (plural preferred). A plugin
registers skills in `setup()` through `ctx.skill`:
`editor.add({ id, name, description, location, content })`
(https://opencode.ai/v2/docs/build/plugins, verified-from-docs). v1
plugins do not run in v2 beta (https://opencode.ai/v2/docs/migrate-v1/,
verified-from-docs).

No documented on-disk skill cache exists for v2. v1 npm plugins cache at
`~/.cache/opencode/node_modules/` (https://opencode.ai/docs/skills.md,
inferred). Community plugins such as `zenobi-us/opencode-skillful` and
`joshuadavidthomas/opencode-agent-skills` add custom skill-loading tools
on top of the v1 plugin API.

### Config files

`opencode.json`/`opencode.jsonc`: global under `~/.config/opencode/`,
project at the project root or under `.opencode/`. Schema
https://opencode.ai/config.json. A new v2 `~/.config/opencode/cli.json`
replaces `tui.json(c)`; it configures the terminal client only, not skills
(https://opencode.ai/v2/docs/migrate-v1/, verified-from-docs).

### Observation of usage

v1 layout: `~/.local/share/opencode/log/` and
`~/.local/share/opencode/storage/{message,part,session_diff,session}/`.
Skill calls appear as `skill` tool calls in message storage. Not
re-confirmed for v2 (https://opencode.ai/docs/troubleshooting/, inferred).

## 5. pi

Sources: https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/skills.md,
https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/packages.md,
https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/extensions.md,
https://github.com/badlogic/pi-skills, https://pi.dev/docs/latest/skills,
https://pi.dev/docs/latest/packages, https://pi.dev/docs/latest/settings,
https://pi.dev/docs/latest/sessions. Short names below: `pi-mono
docs/skills.md`, `pi-mono docs/packages.md`, `pi-mono docs/extensions.md`.
pi.dev content matches pi-mono docs as of 2026-09-09.

### Roots

| Scope   | Roots                                                                                               | Source                 | Confidence         |
| ------- | --------------------------------------------------------------------------------------------------- | ---------------------- | ------------------ |
| Global  | `~/.pi/agent/skills/`, `~/.agents/skills/`                                                          | pi-mono docs/skills.md | verified-from-docs |
| Project | `.pi/skills/`, `.agents/skills/` (cwd or ancestors, after project trust)                            | pi-mono docs/skills.md | verified-from-docs |
| Other   | `skills/` in npm packages, `pi.skills` in `package.json`, settings `skills` array, `--skill <path>` | pi-mono docs/skills.md | verified-from-docs |

### Discovery

| Fact                                                                                                                                                                    | Source                 | Confidence         |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------- | ------------------ |
| Direct `.md` files with skill frontmatter are recognized in `~/.pi/agent/skills/` and `.pi/skills/`. Directories with `SKILL.md` are discovered recursively everywhere. | pi-mono docs/skills.md | verified-from-docs |
| In `~/.agents/skills/` and `.agents/skills/`, nested `.md` files in grouping folders also count.                                                                        | pi-mono docs/skills.md | verified-from-docs |
| At startup pi puts only name and description into the system prompt (XML). The agent uses read or bash to load the full `SKILL.md`.                                     | pi-mono docs/skills.md | verified-from-docs |

### Frontmatter

Required: `name` (1-64 chars, lowercase alphanumerics and hyphens) and
`description` (max 1024 chars). Optional: `license`, `compatibility`,
`metadata`, experimental `allowed-tools`, and `disable-model-invocation`
(pi-mono docs/skills.md, verified-from-docs).

### Invocation control

| Mechanism                                | Effect                                                                                    | Source                 | Confidence         |
| ---------------------------------------- | ----------------------------------------------------------------------------------------- | ---------------------- | ------------------ |
| `/skill:name [args]`                     | Skills register as commands. Trailing args are appended to the content as `User: <args>`. | pi-mono docs/skills.md | verified-from-docs |
| `disable-model-invocation: true`         | Hides the skill from the system prompt. Users must invoke it with `/skill:name`.          | pi-mono docs/skills.md | verified-from-docs |
| `enableSkillCommands` in `settings.json` | Toggles `/skill:name` registration. Also settable in `/settings`.                         | pi-mono docs/skills.md | verified-from-docs |
| `--no-skills`                            | Disables all skill discovery. Explicit `--skill` paths still load.                        | pi-mono docs/skills.md | verified-from-docs |

Security note: skills can instruct the model to run any action and may include
executable code. Review skill content before use
(https://github.com/badlogic/pi-skills, verified-from-docs).

### Disable

`pi config` is an interactive command that enables or disables individual
extensions, skills, prompt templates, and themes from installed packages and
local directories. It writes to `~/.pi/agent/settings.json` (global) or
`.pi/settings.json` (project). `pi config -l` starts in project overrides with
inherited global resources shown dimmed. Tab switches scope
(pi-mono docs/packages.md, verified-from-docs).

The on-disk `settings.json` key for a per-skill toggle is not shown in the
fetched docs. Confidence for the schema: unknown. Exact `pi config` write
shape remains undocumented.

The `skills` settings array also accepts glob syntax: `!pattern` excludes,
`+path` force-includes, `-path` force-excludes
(https://pi.dev/docs/latest/settings, verified-from-docs). Per-package
skill filtering uses `{"source": ..., "skills": [...]}`
(https://pi.dev/docs/latest/packages, verified-from-docs).

### Plugins

| Fact                                                                                                                                                                                                                                               | Source                              | Confidence         |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------- | ------------------ |
| Extensions are pi's plugin mechanism. Auto-discovered from `~/.pi/agent/extensions` and `.pi/extensions` (project only after trust), or loaded with `--extension`.                                                                                 | pi-mono docs/extensions.md          | verified-from-docs |
| Packages (npm or git) bundle extensions, skills, prompt templates, and themes. Core packages go in `peerDependencies`. Other pi packages must be bundled via `dependencies` or `bundledDependencies` and referenced through `node_modules/` paths. | pi-mono docs/packages.md            | verified-from-docs |
| Package caches: `~/.pi/agent/npm/`, `~/.pi/agent/git/<host>/<path>` (global); `.pi/npm/`, `.pi/git/` (project). Manifest `package.json` uses a `"pi"` key.                                                                                         | https://pi.dev/docs/latest/packages | verified-from-docs |

### Config files

| File                        | Keys found                                                                                                                                              | Source                                           | Confidence         |
| --------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------ | ------------------ |
| `~/.pi/agent/settings.json` | `skills` array (extra roots, for example `["~/.claude/skills", "~/.codex/skills"]`), `enableSkillCommands`, per-resource toggles written by `pi config` | pi-mono docs/skills.md, pi-mono docs/packages.md | verified-from-docs |
| `.pi/settings.json`         | Project overrides written by `pi config -l`.                                                                                                            | pi-mono docs/packages.md                         | verified-from-docs |

### Observation of usage

Sessions are stored at `~/.pi/agent/sessions/` as JSONL, one file per
working directory. Override order:
`--session-dir` > `PI_CODING_AGENT_SESSION_DIR` > `sessionDir` setting.
A skill load is not a named entry type in the session format (unknown)
(https://pi.dev/docs/latest/sessions, verified-from-docs).

## 6. skills.sh CLI and the `.agents` root

Sources: https://raw.githubusercontent.com/vercel-labs/skills/main/README.md
(short name: `vercel-labs/skills README`), https://github.com/vercel-labs/skills,
https://deepwiki.com/vercel-labs/skills/5.9-skill-lock-file-system (short name:
`deepwiki lock-file page`), https://github.com/vercel-labs/skills/issues/399.

### Roots

| Fact                                                                                                                                                                                                            | Source                                | Confidence         |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------- | ------------------ |
| Default install creates symlinks from each target agent's skills directory to one canonical source location. `--copy` produces independent per-agent copies for when symlinks are not wanted or supported.      | vercel-labs/skills README             | verified-from-docs |
| `.agents/skills/` is a documented shared convention. Agents that read it directly include Cline, Dexto, Kimi Code CLI, Loaf, Sarvam Code, Warp, and Zed.                                                        | vercel-labs/skills README             | verified-from-docs |
| Supported agents include amp, antigravity, claude-code, clawdbot, codex, cursor, droid, gemini, gemini-cli, github-copilot, goose, kilo, kiro-cli, opencode, roo, trae, windsurf. The README states 75+ agents. | https://github.com/vercel-labs/skills | inferred           |

### Discovery

| Fact                                                                                                                                                                                                                                         | Source                                                    | Confidence         |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------- | ------------------ |
| `skillFolderHash` for GitHub sources uses the GitHub Trees API (recursive fetch, no clone). It uses the SHA of the entry whose path matches the skill folder. Auth order: unauthenticated, then `GITHUB_TOKEN` or `GH_TOKEN`, then `gh api`. | deepwiki lock-file page                                   | verified-from-docs |
| Sources accepted: `owner/repo` shorthand, full URLs, subpaths (`https://github.com/owner/repo/tree/main/skills/x`), GitLab URLs, generic git URLs, local paths. For GitHub it tries git credentials, then `gh repo clone`, then SSH.         | https://github.com/vercel-labs/skills/blob/main/README.md | inferred           |

### CLI commands

| Command                    | Flags                                                                                                         | Source                                                                                    | Confidence         |
| -------------------------- | ------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- | ------------------ |
| `npx skills add`           | `-g/--global`, `-a/--agent <agents...>`, `-s/--skill <skills...>`, `-l/--list`, `--copy`, `-y/--yes`, `--all` | vercel-labs/skills README                                                                 | verified-from-docs |
| `npx skills remove` (`rm`) | `-g/--global`, `-a/--agent`, `-s/--skill`, `-y/--yes`, `--all`                                                | vercel-labs/skills README                                                                 | verified-from-docs |
| `npx skills update`        | `-g/--global`, `-p/--project`, `-y/--yes`                                                                     | vercel-labs/skills README                                                                 | verified-from-docs |
| `npx skills list` (`ls`)   | `-g`, `-a`                                                                                                    | vercel-labs/skills README                                                                 | verified-from-docs |
| `npx skills use`           | Generates a prompt for one skill without installing it, or launches an agent with `--agent`.                  | vercel-labs/skills README                                                                 | verified-from-docs |
| `npx skills check`         | POSTs to `https://add-skill.vercel.sh/check-updates`. Read-only.                                              | https://dev.to/toyama0919/managing-ai-agent-skills-with-npx-skills-a-practical-guide-2an8 | inferred           |
| `npx skills find`          | Interactive search, keyword search, `--owner`.                                                                | https://github.com/vercel-labs/skills                                                     | inferred           |

No `-c` short alias for `--copy` was found in the README. Confidence that `-c`
exists: unknown.

### Frontmatter

Not covered by the input. The CLI installs spec-format skills. Unknown beyond
that.

### Invocation control

Not applicable. The CLI installs files. It does not control invocation.

### Disable

No disable command appears in the input. Unknown.

### Plugins

`dotagents` (`@sentry/dotagents`) is not part of the vercel-labs/skills or
skills.sh ecosystem. The term does not appear in the CLI README, AGENTS.md, or
indexed skills.sh material (vercel-labs/skills README, verified-from-docs).

### Config files

| Fact                                                                                                                                                                      | Source                                           | Confidence         |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------ | ------------------ |
| Global lock file: `~/.agents/.skill-lock.json` or `$XDG_STATE_HOME/skills/.skill-lock.json`. Project lock file: `skills-lock.json` in the repo, created on first install. | deepwiki lock-file page                          | verified-from-docs |
| Format `"version": 3`, keyed by skill name. Confirmed fields: `source`, `sourceType`, `skillFolderHash`, `installedAt`, `updatedAt`.                                      | deepwiki lock-file page                          | verified-from-docs |
| `sourceUrl` (present in `CLAUDE.md`) was not confirmed by the fetched upstream docs. Check `src/skill-lock.ts` upstream.                                                  | deepwiki lock-file page                          | unknown            |
| Older lock-file versions trigger a wipe and restart, not a migration.                                                                                                     | deepwiki lock-file page                          | verified-from-docs |
| Project-scoped skills are not tracked in the global lock file. `check` and `update` silently skip them. An empty `skills` object after install was reported on Windows.   | https://github.com/vercel-labs/skills/issues/399 | verified-from-docs |

### Observation of usage

Not covered by the input. Unknown.

## Cross-harness capability matrix

Cells: `yes`, `no`, `partial`, or `unknown`, with the source. "doc" means the
official documentation listed in the harness section above.

| Capability                           | Agent Skills spec                 | Claude Code                                                              | Codex                                                                                                                 | OpenCode                                                                                                 | pi                                                                                                                       | skills.sh CLI / `.agents`                          |
| ------------------------------------ | --------------------------------- | ------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------- |
| Per-skill link (symlinked skill dir) | unknown (spec silent)             | unknown (not in doc)                                                     | yes: follows symlinked skill folders (build-skills doc)                                                               | unknown (not in doc)                                                                                     | unknown (not in doc)                                                                                                     | yes: default install is per-skill symlink (README) |
| Whole-dir link (root is a symlink)   | unknown                           | unknown                                                                  | unknown                                                                                                               | unknown                                                                                                  | unknown                                                                                                                  | unknown                                            |
| Reads shared `.agents/skills`        | unknown (spec defines no roots)   | no: not in the priority list (skills doc)                                | yes: primary roots (build-skills doc)                                                                                 | yes: project and global (skills.md)                                                                      | yes: project and global (skills.md)                                                                                      | yes: documented shared convention (README)         |
| Native per-skill disable             | no (spec silent)                  | yes: `skillOverrides` in `settings.json` (skills doc)                    | yes: `[[skills.config]] enabled=false` in global `config.toml` (build-skills doc); project scope is open issue #20210 | yes: v2 permission rule `{action:"skill",resource,effect}` (v2 skills doc); v1 `permission.skill` `deny` | yes: `pi config` TUI (packages.md); `settings.json` `skills` exclusions (`!pattern`, `-path`), exact write shape unknown | no (installer only)                                |
| All-skills or tool-level disable     | no                                | partial: `disableBundledSkills` covers bundled skills only (skills doc)  | unknown                                                                                                               | partial: v1 `tools: { skill: false }` per agent, not re-confirmed in v2 (skills.md)                      | partial: `--no-skills` CLI flag only, no settings key (skills.md)                                                        | no                                                 |
| Invocation control (model vs user)   | no: no fields (spec)              | yes: `disable-model-invocation`, `user-invocable` (skills doc)           | yes: `allow_implicit_invocation` in `agents/openai.yaml`; `$name` explicit (build-skills doc)                         | yes: `opencode/autoinvoke: false`, `slash: false` (v2 skills doc); v1 per-agent `permission.skill` `ask` | yes: `disable-model-invocation`, `/skill:name`, `enableSkillCommands` (skills.md)                                        | no                                                 |
| Plugin cache on disk                 | no                                | yes: `~/.claude/plugins/cache/<mkt>/<plugin>/<ver>/` (plugins-reference) | yes: `~/.codex/plugins/cache/$MKT/$PLUGIN/$VER/` (plugins doc)                                                        | unknown: no documented on-disk skill cache in v2                                                         | yes: `~/.pi/agent/npm/`, `~/.pi/agent/git/<host>/<path>` (packages doc)                                                  | no                                                 |
| Plugin manifest                      | no                                | yes: `.claude-plugin/plugin.json` (plugins doc)                          | yes: `plugin.json` with agent-plugins.org schema (plugins doc); `.codex-plugin/` folder unknown                       | yes: v2 `package.json` with `exports` map, `@opencode/plugin` (build/plugins doc); v1 has no manifest    | yes: `package.json` with `pi.*` keys (packages.md)                                                                       | no                                                 |
| Usage observation                    | no                                | unknown: transcript JSONL shape not documented                           | unknown                                                                                                               | partial: v1 layout documented, not re-confirmed for v2 (troubleshooting doc)                             | partial: JSONL sessions per working directory; skill load is not a distinct entry type (sessions doc)                    | unknown                                            |
| Lock file                            | no                                | no                                                                       | no                                                                                                                    | no                                                                                                       | no                                                                                                                       | yes: `~/.agents/.skill-lock.json` v3 (deepwiki)    |
| Spec validator                       | yes: `skills-ref validate` (spec) | unknown                                                                  | unknown                                                                                                               | unknown                                                                                                  | unknown                                                                                                                  | unknown                                            |

## Conflicts with docs/agent-skill-conventions.md

A human must adjudicate each item. Line numbers refer to the worktree copy of
`docs/agent-skill-conventions.md`.

| #   | Harness      | Doc location                     | Doc claim                                                                                          | Research finding                                                                                                                                                                                                                                                                                             | Source                                                                                                                                    | Severity        |
| --- | ------------ | -------------------------------- | -------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------- | --------------- |
| 1   | Claude Code  | lines 36, 106-111                | Claude Code has no native per-skill switch. Skill Studio must remove a symlink to disable a skill. | `settings.json` `skillOverrides` (`on`, `name-only`, `user-invocable-only`, `off`) disables a skill per scope without touching the filesystem.                                                                                                                                                               | https://code.claude.com/docs/en/skills                                                                                                    | contradiction   |
| 2   | Claude Code  | Discovery paths table            | Claude Code roots are personal and project `.claude/skills/`.                                      | The table omits the enterprise managed-settings root (highest priority), the synced root `~/.claude/skills/synced/`, and skills-directory plugins (`<name>@skills-dir`).                                                                                                                                     | https://code.claude.com/docs/en/skills, https://code.claude.com/docs/en/plugins-reference                                                 | omission        |
| 3   | Claude Code  | lines 137, 327                   | Transcript JSONL Skill invocation shape `{"name":"Skill","input":{"skill":"<name>"}}`.             | No official page documents this shape. Mark it as inferred from observed transcripts.                                                                                                                                                                                                                        | Not found on code.claude.com/docs                                                                                                         | unverified      |
| 4   | Codex        | line 121                         | Codex discovery paths are `.codex/skills/` (project) and `~/.codex/skills/` (global).              | Official docs list only `.agents/skills` roots (cwd, parent, repo root, `$HOME`) plus `/etc/codex/skills`. Issue #22590 requesting `.codex/skills` roots was reportedly closed as not planned. A third-party blog claims `~/.codex/skills/` and `~/.codex/skills/.system`. Re-verify in the codex-rs source. | https://learn.chatgpt.com/docs/build-skills, https://github.com/openai/codex/issues/22590, https://blog.fsck.com/2025/12/19/codex-skills/ | contradiction   |
| 5   | OpenCode     | Discovery paths table            | Project `.opencode/skills/ (legacy skill/)`, global `~/.config/opencode/skills/ (legacy skill/)`.  | Resolved: the singular `skill/` path is the v1 legacy form, per the v2 migrate doc. v2 canonical is plural `.opencode/skills/`.                                                                                                                                                                              | https://opencode.ai/v2/docs/migrate-v1/                                                                                                   | resolved        |
| 6   | OpenCode     | Per-harness disable row          | Only `permission.skill` allow, deny, ask.                                                          | Docs also confirm `tools: { skill: false }`, which removes the skill tool for one agent (v1, not re-confirmed in v2). Addition, not a contradiction.                                                                                                                                                         | https://opencode.ai/docs/skills.md                                                                                                        | omission        |
| 14  | OpenCode     | Whole doc                        | No OpenCode v2 facts.                                                                              | The conventions doc has no OpenCode v2 facts: the new `opencode2` binary, the plural-only canonical path, the permission-rule shape, `autoinvoke`/`slash` frontmatter, and `cli.json`.                                                                                                                       | https://opencode.ai/v2/docs/skills/, https://opencode.ai/v2/docs/migrate-v1/                                                              | outdated        |
| 7   | pi           | line 39, line 112                | pi has no per-skill disable. Skill Studio parks the folder globally.                               | `pi config` enables or disables individual skills and writes to `~/.pi/agent/settings.json` or `.pi/settings.json`. The on-disk key is not shown in the docs. Likely outdated, not confirmed wrong.                                                                                                          | https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/packages.md                                                      | likely-outdated |
| 8   | pi           | Discovery paths table            | pi global root is `~/.pi/agent/skills/` only.                                                      | pi also reads `~/.agents/skills/` and project `.agents/skills/` as first-class roots. Consistent with the doc's Universal row. Completeness gap only.                                                                                                                                                        | https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/skills.md                                                        | omission        |
| 9   | skills.sh    | `CLAUDE.md` lock-file example    | Lock entries include `sourceUrl`.                                                                  | Upstream docs fetched do not confirm `sourceUrl`. Check `src/skill-lock.ts` upstream.                                                                                                                                                                                                                        | https://deepwiki.com/vercel-labs/skills/5.9-skill-lock-file-system                                                                        | unverified      |
| 10  | skills.sh    | Task assumption (not in the doc) | `-c` short flag for copy mode.                                                                     | Only `--copy` appears in the README. Verify with `npx skills add --help` before code depends on `-c`.                                                                                                                                                                                                        | https://raw.githubusercontent.com/vercel-labs/skills/main/README.md                                                                       | unverified      |
| 11  | skills.sh    | Neither local doc                | (none)                                                                                             | Project-scoped skills are not tracked in the global lock file. `check` and `update` skip them. Neither local doc mentions this.                                                                                                                                                                              | https://github.com/vercel-labs/skills/issues/399                                                                                          | omission        |
| 12  | skills.sh    | Provenance list                  | `dotagents` is a separate install method.                                                          | Consistent. `dotagents` is a Sentry tool, not part of skills.sh. Noted so a reader does not expect skills.sh docs to cover it.                                                                                                                                                                               | vercel-labs/skills README                                                                                                                 | no conflict     |
| 13  | Agent Skills | Frontmatter table                | (matches)                                                                                          | No conflicts found. Every checked field matches the current spec.                                                                                                                                                                                                                                            | https://agentskills.io/specification                                                                                                      | no conflict     |
