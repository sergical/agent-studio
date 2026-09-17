# pi

What the code knows about the pi coding agent, read on 2026-09-17. Facts are verified against the code unless marked assumed or doc-only.

## How the app knows pi is present

**Today.** There is no install probe. Presence is inferred from `.pi/skills` in a project (crates/skill-studio-core/src/tracked_projects.rs:23) or from session folders. The runner names the binary `pi`, inferred from a code survey, not a live check (harness.rs:764).

**Proper.** Four signals, per harness-detection.md:

| Signal         | Source                                                                                                                                    |
| -------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| Executable     | `pi` resolved on the login-shell PATH; installed from npm `@earendil-works/pi-coding-agent` or a community fork with the same binary name |
| Version        | `pi --version`; there is no version file under `~/.pi/agent`                                                                              |
| Install method | npm; the global prefix is inferred from the resolved path                                                                                 |
| Configured     | `~/.pi/agent/settings.json` parses                                                                                                        |
| Used           | any `~/.pi/agent/sessions/<slug>/*.jsonl`, or `~/.pi/agent/trust.json`, which pi writes on its first run inside a project                 |

`~/.pi/agent/bin/` holds vendored helpers such as `fd`, not pi. `~/.pi/agent/npm` and `git` hold installed packages and prove use, not the CLI version.

## Where skills live

| Root      | Path                                                                             | Scope           | Evidence                                           |
| --------- | -------------------------------------------------------------------------------- | --------------- | -------------------------------------------------- |
| Own       | `~/.pi/agent/skills`, `.pi/skills`                                               | global, project | harness.rs:709; agents.rs:170, 219                 |
| Universal | `~/.agents/skills`, `.agents/skills`                                             | global, project | harness.rs:717                                     |
| Packages  | `~/.pi/agent/npm`, `~/.pi/agent/git/<host>/<path>`; project `.pi/npm`, `.pi/git` | global, project | doc-verified, pi.dev packages doc (harness.rs:754) |

Root-level `.md` files with valid frontmatter count as skills, not only `SKILL.md` in a subfolder (docs/agent-skill-conventions.md:125). Whether pi skips hidden entries is Unknown and a unit test pins it as Unknown (harness.rs:735, 895). Symlink following is Unknown (harness.rs:733).

## How pi loads a skill

- pi reads the universal root directly, so a skill in `~/.agents/skills` reaches pi with no link (skill_harness_disable.rs:629).
- pi's `settings.json` has a `skills` array that accepts exclusion patterns (harness.rs:130; pi.dev settings doc). No code reads or writes it.
- Frontmatter `disable-model-invocation: true` and `user-invocable: false` apply to pi like every reader of SKILL.md, written by the shared skill_invocation.rs.

## How the app turns a skill off for pi

| Mechanism                         | File touched                                                                                 | State                        |
| --------------------------------- | -------------------------------------------------------------------------------------------- | ---------------------------- |
| Native per-skill switch           | none; the app returns "pi has no per-skill disable - it reads the Universal folder directly" | skill_harness_disable.rs:629 |
| `settings.json` skills exclusions | not implemented; the exact entry the interactive pi config writes is undocumented            | harness.rs:744               |
| All skills off                    | a CLI flag only, no settings key                                                             | harness.rs:737, doc-only     |
| Park                              | rename out of `~/.agents/skills`                                                             | generic                      |

## Activity: how the app sees a skill use

- Source: `~/.pi/agent/sessions/**/*.jsonl`, one folder per session, walked four levels deep (host skill_uses.rs:489). A missing root means "never installed".
- Read model: append-only, resumed by byte offset per file, same machinery as Claude Code and Codex (host skill_uses.rs:205, 863).
- The first record of type `session` gives the session id and `cwd`; the parser carries them through the file (core skill_uses/pi.rs:1, 83). The session folder name is a dashed encoding of the path and cannot be decoded reliably, so the header's `cwd` is used (discovery.rs:68).
- Triggers: a user message that starts with pi's skill block (name and location) is a User trigger. A plain typed `/skill:X` that pi did not load, for example in an untrusted folder, does not count, by design (pi.rs:25, 260). A `read` tool call whose path resolves to a known SKILL.md, or a `bash` call that reads one with head or cat, is a FileRead trigger (pi.rs:313). pi has no skill tool, so there is no Agent trigger (pi.rs:6).
- Tool arguments may arrive as a JSON string or an object; both are handled (pi.rs:54).
- Timestamps come from each record's RFC 3339 `timestamp` (pi.rs:102). Cost caps are the shared per-line, per-file, and per-run byte budgets.

## What the core must handle for pi

- Nothing to link: the universal root is the deployment. Install means "put it in `~/.agents/skills`".
- Read and write pi's `settings.json` exclusions once the entry format is confirmed. Not done.
- Resume transcripts by offset and read the header once.

## From the docs on 2026-09-16

From https://pi.dev/docs/latest/skills and the pi-mono repo (sources.md):

- Roots: `~/.pi/agent/skills`, `~/.agents/skills`, project `.pi/skills` and `.agents/skills` walked up to the git root, package `skills/` folders or `pi.skills` in `package.json`, and `--skill <path>`. Bare `.md` files with frontmatter count only in `~/.pi/agent/skills`; elsewhere `SKILL.md` folders are found recursively, so the facts table's one-level depth is wrong for project roots.
- Frontmatter: `name` up to 64 chars, `description` up to 1024, optional `license`, `compatibility`, `metadata`, `allowed-tools`, `disable-model-invocation`, modelled on agentskills.io.
- The only documented off switch is `disable-model-invocation: true` in frontmatter, after which `/skill:name` still works. No settings exclusion is documented, so the `settings.json` skills array in harness.rs:130 stays doc-unverified.

## Unknowns

- Whether a settings entry can exclude a skill at all.
- Hidden entry handling and symlink following.
- Whether pi has any usage record beyond file reads and typed skill blocks.
- `pi --version` output and any env override for `~/.pi/agent`.
