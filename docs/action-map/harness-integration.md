# Harness integration

Skill Studio meets each harness by discovering its skill folders, classifying who owns each deployment, and, for a headless run, driving that harness's own CLI.

## Scope and destination, before the per-harness detail

Every install picks a scope and a destination separately (`docs/agent-skill-conventions.md:60-75`). Scope is Global (below `$HOME`) or Project (below one selected project). Destination is Universal (one canonical folder at `.agents/skills/<name>`, read directly by Codex, OpenCode, pi, Cursor, and Grok Build, with Claude Code needing a link) or Per harness (an independent copy in each selected harness's own folder, with no shared source of truth). This choice, made once at install time, decides which of the two disk layouts below a given skill gets.

## The harnesses

`AgentId` lists 42 agents (`apps/desktop/src-tauri/src/skills/agents.rs:17-61`), but only six are "first class": their folders are scanned for provenance, disable, and run support (`agents.rs:338-345`). The table below covers those six plus the shared `.agents` root. Paths are the first-class agent's own `project_path()`/`global_path()` methods (`agents.rs:166-261`), the single source of truth `skill_roots()` reads from (`agents.rs:363-412`).

| Harness            | Global path                                                                                        | Project path                                                           | Link or copy                                                                                                                      | Sidecar files                                                                        | Native disable                                                                                                                                        | How the app knows it's enabled                                                                                                             | How Activity reads "used"                                                                                                                                                                                                |
| ------------------ | -------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Claude Code        | `~/.claude/skills` (`agents.rs:217`)                                                               | `.claude/skills` (`agents.rs:168`)                                     | Per-skill symlink into `.agents/skills`, or a whole-dir symlink                                                                   | None on disk; Skill Studio's own registry records the link state                     | No native per-skill switch; Skill Studio removes the per-skill symlink or parks the skill globally (`docs/agent-skill-conventions.md:37`, `:108-113`) | `Deployment.disabled` + `disabled_by: "claude-link-removed"` (`packages/lib/src/skill-types.generated.ts:858-866`)                         | `~/.claude/projects/*/*.jsonl`, `tool_use` named `Skill` (`docs/agent-skill-conventions.md:167`; `crates/skill-studio-host/src/skill_uses.rs:818-829`)                                                                   |
| Codex              | `~/.codex/skills` (`agents.rs:224`)                                                                | `.codex/skills` (`agents.rs:175`)                                      | Reads `.agents/skills` directly                                                                                                   | `~/.codex/config.toml` `[[skills.config]]`, `agents/openai.yaml` (invocation policy) | `enabled = false` in `config.toml`, matched by SKILL.md path, written with `toml_edit` (`docs/agent-skill-conventions.md:38`, `:101-103`)             | `Deployment.disabled_by: "codex-config"`; `codex_implicit_invocation` read straight off `openai.yaml` (`skill-types.generated.ts:839-844`) | `~/.codex/sessions/**/*.jsonl` and `archived_sessions` (`docs/agent-skill-conventions.md:168`; `skill_uses.rs:829-844`)                                                                                                  |
| OpenCode           | `~/.config/opencode/skills` (`agents.rs:218`), legacy `~/.config/opencode/skill` (`agents.rs:376`) | `.opencode/skills`, legacy `.opencode/skill` (`agents.rs:169`, `:402`) | Reads `.agents/skills` directly                                                                                                   | `~/.config/opencode/opencode.json` (never `.jsonc`)                                  | `permission.skill.<name-or-glob> = "deny"` (`docs/agent-skill-conventions.md:39`, `:104-107`)                                                         | `Deployment.disabled_by: "opencode-permission"`                                                                                            | `~/.local/share/opencode/opencode.db` / `opencode-<channel>.db`, table `session_message` (`docs/agent-skill-conventions.md:169`; `skill_uses.rs:845-858`, read by `crates/skill-studio-host/src/skill_uses/opencode.rs`) |
| pi                 | `~/.pi/agent/skills` (`agents.rs:219`)                                                             | `.pi/skills` (`agents.rs:170`)                                         | Reads `.agents/skills` directly; scratch/worktree runs also get a `.pi/skills/<name>` symlink (`skill_agent_runner.rs:1395-1403`) | None native                                                                          | No per-skill switch; Skill Studio parks the skill globally (`docs/agent-skill-conventions.md:40`, `:114-116`)                                         | Parked state only (`InstalledSkill.parked`, see below)                                                                                     | `~/.pi/agent/sessions/**/*.jsonl` (`docs/agent-skill-conventions.md:170`; `skill_uses.rs:858-870`)                                                                                                                       |
| Cursor             | `~/.cursor/skills` (`agents.rs:171`... global via `global_path`)                                   | `.cursor/skills` (`agents.rs:171`)                                     | Reads `.agents/skills` directly                                                                                                   | None native                                                                          | No per-skill switch; parked globally (`docs/agent-skill-conventions.md:41`, `:114-116`)                                                               | Parked state only                                                                                                                          | `~/.cursor/projects/<name>/agent-transcripts/<session>/*.jsonl` (`docs/agent-skill-conventions.md:171`; `skill_uses.rs:871-882`)                                                                                         |
| Grok Build         | `~/.grok/skills` (`agents.rs:259`)                                                                 | `.grok/skills` (`agents.rs:210`)                                       | Reads `.agents/skills` directly                                                                                                   | None native                                                                          | Unknown model-side control (`docs/agent-skill-conventions.md:31`, `:42`); no per-skill switch, parked globally                                        | Parked state only                                                                                                                          | `~/.grok/sessions/<encoded-cwd>/<session-id>/updates.jsonl` (`docs/agent-skill-conventions.md:172`; `skill_uses.rs:883-895`)                                                                                             |
| shared (Universal) | `~/.agents/skills` (`agents.rs:381`)                                                               | `.agents/skills` (`agents.rs:407`)                                     | Canonical - every above harness except Claude Code reads it directly (`docs/agent-skill-conventions.md:65-70`)                    | `.agents/agents.toml`, `.agents/agents.lock`, `.agents/.skill-lock.json`             | Parking (below)                                                                                                                                       | `Deployment.backing.kind: "canonical"` (`skill-types.generated.ts:828-838`)                                                                | n/a - not a harness itself                                                                                                                                                                                               |

Claude Code's plugin cache (`~/.claude/plugins/cache`) and Codex's (`~/.codex/plugins/cache`) are also read for skills; a plugin-shipped skill is `owner_kind: "plugin"`, read-only (`docs/agent-skill-conventions.md:122-123`; `skill_ownership.rs:22-56`).

Discovery for a harness can also be switched off per source: the registry's `discovery: {harness: false}}` map turns off that harness's project history in the desktop, CLI, and MCP server alike; a missing key means on (`docs/agent-skill-conventions.md:138`). A switched-off harness's skill-use refresh is skipped outright, though its cached uses stay visible (`crates/skill-studio-host/src/skill_uses.rs:1045-1054`).

## What one installed skill looks like on disk, per harness

A skill named `example`, deployed Universal (global scope) with a Claude Code per-skill link:

```
~/.agents/skills/example/SKILL.md      # source of truth
~/.claude/skills/example -> ../../.agents/skills/example   # link (Skill Studio-owned)
# Codex, OpenCode, pi, Cursor, Grok Build read ~/.agents/skills/example directly - no link needed
~/.agents/.skill-lock.json             # app-owned ledger, records the install
~/.agents/agents.toml, agents.lock     # app-owned, only when installed via dotagents
```

Project scope, same skill, same layout rooted at the project instead of `$HOME`:

```
<project>/.agents/skills/example/SKILL.md   # source of truth
<project>/.claude/skills/example -> ../../.agents/skills/example  # link
<project>/.agents/.skill-lock.json          # app-owned ledger (per-project)
```

A Per harness install (Copy method) never writes `.agents/skills` and never links back to it; each selected harness gets its own independent copy with no shared source of truth (`docs/agent-skill-conventions.md:71-75`).

## What each harness state means

| State (app name)       | What's on disk                                                                                                                                                                                                         | Command                                                                          |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| enabled                | Deployment present, not parked, no native disable set                                                                                                                                                                  | default state after install                                                      |
| disabled (per-harness) | Codex: `enabled = false` row in `config.toml`; OpenCode: `permission.skill` deny; Claude Code: per-skill symlink removed, tracked in the registry's `harness_disabled` map (`docs/agent-skill-conventions.md:108-113`) | `skill_harness_disable.rs` (Tauri command wraps this)                            |
| parked                 | Universal folder moved `~/.agents/skills/<name>` -> `~/.agents/skills-parked/<name>` (rename, not copy); Claude Code per-skill link removed first (`docs/agent-skill-conventions.md:79-94`)                            | `skill_park.rs`; unpark reverses both steps                                      |
| linked                 | A per-skill symlink (Claude Code) or a whole-dir symlink pointing at Universal                                                                                                                                         | created during install or by `skill_materialize.rs`                              |
| materialized           | A whole-dir symlink root is "exploded" into per-skill symlinks, one per skill, so a per-skill disable becomes possible (`skill-types.generated.ts:923-929`, `explode_shared_dir`)                                      | `skill_materialize.rs`                                                           |
| independent copy       | `Deployment.backing.kind: "independent"` - a folder with the same name as a Universal deployment but no link relationship to it                                                                                        | Copy method of Add skill, or a Per harness install                               |
| trial                  | Registry `trials` map records a 24-hour expiry; folder installed like a normal Universal deployment until it expires                                                                                                   | `skill_trial.rs`, `keep_skill_trial` to cancel expiry                            |
| trashed                | Copied to `~/.agents/skills-trash/<name>-<timestamp>` before removal, so a failed removal or an accidental trial expiry doesn't lose the folder (`docs/agent-skill-conventions.md:336-343`)                            | `skill_trial::spawn_trial_expiry_loop`; `restore_trashed_skill` to bring it back |

## How a run drives a harness

Four harnesses are supported for headless runs (`HarnessId`, `skill_agent_runner.rs:43-48`): Claude Code, Codex, OpenCode, pi. Cursor and Grok Build are discovery/disable-only, not runnable.

`build_command` (`skill_agent_runner.rs:172-269`) constructs the argv per harness:

- **Claude Code**: `claude -p "<prompt>" --output-format stream-json --verbose --permission-mode <default|auto> [--resume <id>]` (`skill_agent_runner.rs:182-206`).
- **Codex**: `codex exec --json --skip-git-repo-check -s <read-only|workspace-write> -C <cwd> "<prompt>"`, or `codex exec resume <thread_id> --json "<prompt>"` for a follow-up (`skill_agent_runner.rs:207-227`).
- **OpenCode**: `opencode2 run --standalone --format json --auto [--session <id>] "<prompt>"` - binary is `opencode2`, the v2 CLI; `--standalone` avoids a wedged shared background service (`skill_agent_runner.rs:228-252`).
- **pi**: `pi -p --mode json [--tools read,grep,find,ls] [--session <id>] "<prompt>"` (`skill_agent_runner.rs:253-265`).

The binary itself is resolved once per harness via `$SHELL -lc 'command -v <bin>'` and cached, because the app's own `PATH` doesn't see the user's shell config (`skill_agent_runner.rs:808-835`).

The skill under test is made visible to the run through a prepared **run target** (`skill_run_target.rs:28-32`), one of three kinds:

- **Scratch**: a fresh cache dir under `<app cache>/skill-studio/scratch/<timestamp>`; the skill is copied to `.agents/skills/<name>` and symlinked from `.claude/skills/<name>` and `.pi/skills/<name>` (`skill_agent_runner.rs:1372-1416`; `skill_run_target.rs:263-285`).
- **Worktree**: a detached `git worktree add --detach <path> HEAD` off the project's own repo, same copy-plus-symlink layout, committed as a "skill-studio: test setup" commit so the later diff excludes the setup files themselves (`skill_run_target.rs:326-407`).
- **InPlace**: the project's own working tree, used directly - refused if it has uncommitted changes (`skill_run_target.rs:413-437`).

Ask, Audit, and Test are UI-level names for these runs; the backend itself only knows write access (`ReadOnly` vs `Workspace`, `skill_agent_runner.rs:65-70`) and run target kind. A read-only run passes each harness's own read-only flags (Claude Code `--allowedTools`, Codex `-s read-only`, pi `--tools read,grep,find,ls`); OpenCode has no enforced read-only mode - probing found `patch` still creates files regardless of permission config (`skill_agent_runner.rs:228-238`).

Every parsed stdout line (or lifecycle step) emits one `SkillAgentEvent` on `"skill-agent://event"` (`skill_agent_runner.rs:27`, `:837-845`), and the run always ends with exactly one `Finished` event, even on cancellation or a crash before output (`skill_agent_runner.rs:949-952`, `:1177-1231`). A Worktree run's transcript lives in the worktree's own commit history; its verdict (pass/fail, `skill_loaded`) is carried in the `Finished` event's fields and persisted by the caller into run history (`SkillRunSummary`, `skill-types.generated.ts:794-799`). The diff for a Worktree or InPlace run is read with `git diff` against the setup commit or the original HEAD (`skill_run_target.rs:466-487`).

Whether the skill under test actually loaded is inferred per harness, not reported uniformly, because none of the four CLIs share one signal for it:

- **Claude Code**: an assistant `tool_use` block named `Skill` whose `input.skill` matches the skill's name (`skill_agent_runner.rs:322-337`).
- **Codex**: a `command_execution` item whose command string contains `/<skill-name>/SKILL.md` (`skill_agent_runner.rs:433-441`).
- **pi**: a `read` tool call whose `args.path` ends with or contains `/<skill-name>/SKILL.md` (`skill_agent_runner.rs:517-523`).
- **OpenCode**: a `skill` tool part naming the skill, or a `read` tool part on that skill's `SKILL.md` (`skill_agent_runner.rs:658-671`).

When none of these fire, `skill_loaded` stays `Unknown` rather than `No` (`skill_agent_runner.rs:76-81`) - the app never claims a skill definitely didn't load just because it didn't see the expected signal.

## Where the harnesses differ and the app papers over it

- OpenCode reads both `skills/` and the older singular `skill/` at both scopes; `skill_roots()` lists both as separate roots (`agents.rs:373-377`, `:399-403`).
- pi's global path is under `agent/` (`~/.pi/agent/skills`, `agents.rs:219`), not `~/.pi/skills` like its project path (`agents.rs:170`) - `AgentId::global_path`/`project_path` hide the asymmetry behind one interface.
- Claude Code has no native per-skill disable at all; the app fakes it by removing the per-skill symlink and tracking that decision in its own registry (`docs/agent-skill-conventions.md:108-113`).
- A skill deployed through a whole-dir symlink (`~/.claude/skills -> ~/.agents/skills`) has no per-skill link to remove, so per-harness disable is impossible until the root is "materialized" (exploded) into per-skill links first (`skill_materialize.rs`; `skill-types.generated.ts:923-929`).
- OpenCode's read-only mode is not actually enforced by the CLI; the app documents this rather than claiming a guarantee it can't back up (`skill_agent_runner.rs:234-238`).
- Cursor and Grok Build have no runner support at all (`HarnessId` omits them, `skill_agent_runner.rs:43-48`), so "Test" simply doesn't offer them as a target.
- The wire format still uses the compatibility value `shared` for the Universal root in `Deployment.agent` and in event/relationship names, even though UI copy calls it "Universal" (`docs/agent-skill-conventions.md:250-257`; `skill-types.generated.ts:820-824`).

## Desired state

Much of this already exists in `crates/skill-studio-core/src/harness.rs`: a `HarnessCatalog` of `HarnessFacts`, one row per harness (`harness.rs:456-826`, built by `HarnessCatalog::builtin()`), each carrying its discovery roots, native-disable mechanism, invocation-control shape, plugin-cache path, usage-source shape, and runner support, every fact paired with `Evidence` (`harness.rs:27-49`, `:219-260`). `CapabilityReport::from_facts` (`harness.rs:378-433`) already derives per-operation support (`set_harness_enabled`, `set_claude_link`, `materialize_root`, `run_skill_test`, `observe_usage`, `set_invocation_policy`) from one `HarnessFacts` row - the shape the spec's "one adapter per harness" asks for.

What is not yet true: the catalog is consumed in only two places (`apps/desktop/src-tauri/src/skills/skill_refresh.rs` and `crates/skill-studio-host/src/builder.rs`). `skill_agent_runner.rs`, `skill_run_target.rs`, `skill_harness_disable.rs`, `skill_materialize.rs`, and `skill_park.rs` still hard-code harness lists, symlink targets, and CLI argv locally, duplicating facts the catalog already has. `AgentId` also exists twice: the desktop's own 42-variant enum (`agents.rs:17-61`) and the core crate's 6-value newtype (`identity.rs:24-38`), with no shared conversion beyond ad hoc `AgentId::from(...)` calls. Closing the gap means: every desktop harness-facing module reads `HarnessCatalog` instead of its own constants; `skill_agent_runner::build_command` becomes a method the catalog's `RunnerSpec` dispatches to; a conformance test suite runs the catalog's declared paths/link-policy/disable-mechanism against a fixture tree for every harness in `HarnessCatalog::builtin()`.

## Native disable versus parking

Two different "off" switches exist, and the app is careful not to conflate them (`docs/agent-skill-conventions.md:96-116`). A per-harness disable turns a skill off for one harness only, through that harness's own mechanism (Codex's `config.toml`, OpenCode's `opencode.json`, or Skill Studio's own Claude Code symlink trick) - every other harness still sees the skill. Parking is Skill Studio's own global disable: it moves the Universal folder itself, so every harness that reads `.agents/skills` loses the skill at once. A skill can be parked and separately have a per-harness disable recorded for one reader; unparking only restores the Universal folder, it does not touch a harness's own disable state.

A parked-but-reinstalled skill is a known edge case: a `sync`/`update` run while a skill is parked can recreate `~/.agents/skills/<name>` on its own, since parking only ever moved the folder, not any install ledger. Skill Studio still shows the skill as parked, from its own registry record, but flags the mismatch as `parked-but-reinstalled` until the user unparks it, at which point the two copies are reconciled by content hash (`docs/agent-skill-conventions.md:87-94`).

## Gaps

- The harness catalog in `crates/skill-studio-core/src/harness.rs` is read by only two consumers, `skill_refresh.rs` and the host builder; `skill_agent_runner.rs`, `skill_run_target.rs`, `skill_harness_disable.rs`, `skill_materialize.rs`, and `skill_park.rs` each keep their own harness list, link targets, and CLI argv.
- No conformance test runs every harness row through the same install, disable, park, and run checks; each module tests its own copy of the facts.
- Grok Build's model-side auto-invoke control is recorded as unknown in the catalog, so the app cannot promise the invocation switch works there.
- A `sync` or `update` run while a skill is parked can recreate the shared folder; the app only flags `parked-but-reinstalled` and does not reconcile it.
- Provenance classification is split between `skill_ownership.rs` in the desktop crate and `identity.rs` in the core crate; one source of truth is needed before the CLI and MCP adapters can agree with the desktop.

## Notes on this read

- `apps/desktop/src-tauri/src/skills/provenance.rs` named in the task does not exist in this worktree; provenance classification now lives in `skill_ownership.rs` (`LifecycleOwnerKind`) and `crates/skill-studio-core/src/identity.rs` (`SourceKind`).
- `apps/desktop/src/lib/skill-types.ts` named in the task does not exist; the generated DTOs live in `packages/lib/src/skill-types.generated.ts`, produced from the Rust `#[derive(JsonSchema)]` types.
- Grok Build's model-side auto-invoke control is unverified (`docs/agent-skill-conventions.md:31`, `:42`) - the catalog records it as `Support::Unknown`, not `No`.
- `skill_agent_runner.rs` is 2018 lines; this report reads lines 1-1457 plus targeted tests past that point were not re-verified line-by-line.
