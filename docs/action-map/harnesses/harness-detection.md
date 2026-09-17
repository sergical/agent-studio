# Harness detection

How the app should know that a harness is installed, which version, how it was installed, and whether it has ever run. Researched on 2026-09-16 against vendor docs and this machine. Folder presence alone is not detection; it is the weakest of four signals and the app today uses only that one (see each harness file).

OpenCode facts below that came from `opencode.ai/docs` are marked "confirm v2"; the app references only `https://opencode.ai/v2/docs`. See sources.md for the page list.

## The four signals, strongest first

| Signal                     | Means                               | How the app gets it                                                                                 | Cost                                                                       |
| -------------------------- | ----------------------------------- | --------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| Executable                 | the harness can run on this machine | resolve the binary name on the user's login-shell PATH, then confirm the file is executable         | one shell spawn per app start, cached                                      |
| Version and install method | what build it is and who updates it | run `<bin> --version` once per binary change; read the vendor's own install marker where one exists | one process per harness per binary change, cached by path, size, and mtime |
| Configured                 | the user has set it up              | the vendor config file exists and parses                                                            | one file read                                                              |
| Used                       | it has run at least once            | a session or transcript record exists, or the vendor's own startup counter is above zero            | one directory listing, already done by the Activity reader                 |

The app reports a state per harness from these signals:

| State      | Rule                                                                                                    |
| ---------- | ------------------------------------------------------------------------------------------------------- |
| Not found  | no executable, no config, no session data                                                               |
| Data only  | config or sessions exist but no executable on PATH; shown with "binary not on PATH", never as installed |
| Installed  | executable found; version Unknown until the probe runs                                                  |
| Configured | Installed and the config file exists                                                                    |
| Used       | Configured or Installed, and session evidence exists                                                    |

Unknown is a value, not an error. A row the app cannot prove prints Unknown; it never guesses from a folder name.

## PATH resolution

macOS launches a desktop app with the minimal `launchd` PATH, so `~/.local/bin`, `/opt/homebrew/bin`, and version-manager shims are missing. The app already runs the login shell once to read `$EDITOR` (apps/desktop/src-tauri/src/skills/skill_editor.rs, cached in a `OnceLock`). Detection reuses that single spawn: ask the login shell for `PATH`, then resolve each binary name in Rust against that PATH and check the executable bit. This avoids one shell per harness and avoids aliases and shell functions, which `command -v` would return as non-paths.

Fallback directories, checked when the PATH lookup fails: `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`, `~/.npm-global/bin`, `~/.volta/bin`, `~/.bun/bin`, `~/.nvm/versions/node/*/bin`, and `/Applications/*.app/Contents/Resources` for bundled CLIs. A hit from a fallback is reported as "not on PATH" so the user knows why the harness's own terminal may differ.

The probe runs in `spawn_blocking` with a two-second timeout per process. It never runs on the UI thread and never on the scan path.

## Per harness

### Claude Code

| Signal         | Source                                                                                                                      | Evidence                                                 |
| -------------- | --------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------- |
| Executable     | `claude` on PATH; native installs place `~/.local/bin/claude` as a symlink into `~/.local/share/claude/versions/<version>/` | documented, code.claude.com/docs/en/setup                |
| Version        | `claude --version` prints `2.1.211 (Claude Code)`; the versions folder name carries the same number for native installs     | documented                                               |
| Install method | `~/.claude.json` field `installMethod` (`native`, `brew`, `npm`, and others) and `autoUpdates`                              | observed on this machine: `native`, `autoUpdates: false` |
| Configured     | `~/.claude/settings.json` or `~/.claude.json` exists                                                                        | documented                                               |
| Used           | `~/.claude.json` field `numStartups` above zero; `~/.claude/projects/*/sessions-index.json` or any transcript `.jsonl`      | observed: `numStartups` 1230                             |
| Health         | `claude doctor` is read-only and reports install health; optional, on demand only                                           | documented                                               |

Pitfalls: `/Applications/Claude.app` is Claude Desktop, bundle `com.anthropic.claudefordesktop`, a different product; its presence says nothing about Claude Code. The VS Code extension, the JetBrains plugin, and the desktop app all write into `~/.claude/`, so that folder can exist with no CLI. A custom `~/.local/bin/claude` launcher breaks the installer's symlink and `claude doctor`.

### Codex

| Signal         | Source                                                                                                                                                                            | Evidence                                                             |
| -------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| Executable     | `codex` on PATH from npm `@openai/codex`, Homebrew cask `codex`, or a GitHub release tarball placed by hand; also bundled at `/Applications/ChatGPT.app/Contents/Resources/codex` | documented, github.com/openai/codex; bundle observed on this machine |
| Version        | `codex --version`; output format not confirmed in primary docs, to be pinned by one local run and a test                                                                          | Unknown until run                                                    |
| Install method | no vendor marker; infer from the resolved path (npm global dir, Homebrew Cellar, ChatGPT.app bundle, other) and say "inferred from path"                                          | inferred                                                             |
| Configured     | `~/.codex/config.toml` exists                                                                                                                                                     | observed                                                             |
| Used           | any `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`; `[projects."<path>"]` trust rows in config.toml                                                                               | observed                                                             |

Pitfalls: `~/.codex/version.json` holds `latest_version` and `last_checked_at`, the update-check cache, not the installed version. The `Codex Computer Use.app` helper under `~/.codex/computer-use/` is not the CLI. A `~/.codex/` folder with only `version.json` is not proof of a session.

### OpenCode

| Signal         | Source                                                                                                                                                           | Evidence                                   |
| -------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------ |
| Executable     | `opencode` on PATH from the curl installer, npm `opencode-ai`, or Homebrew `anomalyco/tap/opencode`; the v2 beta may add `opencode2` during migration            | confirm v2                                 |
| Version        | `opencode --version`                                                                                                                                             | confirm v2                                 |
| Install method | infer from the resolved path; no vendor marker known                                                                                                             | inferred                                   |
| Configured     | `~/.config/opencode/opencode.json` (or `.jsonc`) exists; `~/.config/opencode/package.json` pins `@opencode-ai/plugin` and its `node_modules` carry real versions | observed                                   |
| Used           | `~/.local/share/opencode/opencode.db` or `opencode-<channel>.db` with rows, or legacy `storage/session/**`; `~/.local/share/opencode/auth.json` proves a login   | observed; auth.json documented, confirm v2 |

Pitfalls: the config folder can exist from editing settings with no session ever run, so Used must come from the database or the storage folder. `service.json` and `opencode.json` on a real machine can hold secrets; the probe reads only the keys it needs and never logs values. Honour `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, and OpenCode's own env overrides when resolving these paths.

### pi

| Signal         | Source                                                                                                                        | Evidence                                         |
| -------------- | ----------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------ |
| Executable     | `pi` on PATH from npm `@earendil-works/pi-coding-agent`; community forks publish under other scopes with the same binary name | documented, npm and github.com/earendil-works/pi |
| Version        | `pi --version`; no version file under `~/.pi/agent` was found                                                                 | Unknown until run                                |
| Install method | npm only; infer the global prefix from the resolved path                                                                      | inferred                                         |
| Configured     | `~/.pi/agent/settings.json` exists                                                                                            | observed                                         |
| Used           | any `~/.pi/agent/sessions/<slug>/*.jsonl`; `~/.pi/agent/trust.json` is written on the first run inside a project              | observed                                         |

Pitfalls: `~/.pi/agent/bin/` holds vendored helper binaries such as `fd`; they are not pi. `~/.pi/agent/npm` and `git` hold installed packages, evidence of use, not of the CLI version.

### Cursor and Grok Build

Both are in the app's agent list for install targets but are not first-class Activity sources.

- Cursor: the editor version is `CFBundleShortVersionString` in `/Applications/Cursor.app/Contents/Info.plist` (3.20.21 here); the separate Cursor CLI installs `agent` into `~/.local/bin` with `agent --version`. `~/.cursor/argv.json` and `~/.cursor/extensions/` prove the editor ran. Treat editor and CLI as two detections.
- Grok Build: documented install is the `x.ai/cli/install.sh` script only; binary name, version flag, and config folder are Unknown. `/Applications/Grok Bot.app` is the chat client, not Grok Build.

## What the core must handle

- One `detect_harnesses` op that returns, per harness: state, executable path, on-PATH flag, version, install method with its evidence kind, config path, and the newest session timestamp.
- Caching keyed by the executable's path, size, and mtime, so `--version` runs once per binary change, not per scan.
- Every probe off the UI thread, under a timeout, with Unknown as the result on timeout.
- No probe writes anything, and no probe reads a value it does not need from a config file that may hold secrets.

## Open items

- `codex --version`, `opencode --version`, and `pi --version` output formats: pin each with a local run and a parsing test.
- OpenCode rows marked "confirm v2" need the v2 docs page from sources.md.
- Grok Build binary name and config folder.
- The Codex desktop version signal when Codex ships as a standalone app rather than inside ChatGPT.app.
