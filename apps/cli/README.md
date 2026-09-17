# skill-studio

The Skill Studio command line: a thin adapter over `skill-studio-core`. It
builds a `RuntimeScope` and a set of ports from flags and the environment,
calls one core operation, and prints the result. It holds no policy of its
own — issue derivation, capability facts, and exit statuses all live in the
core, so the CLI, the desktop app, and any future MCP server or TUI agree.

## Commands

```
skill-studio scan         [--home <dir>] [--project <dir>]... [--fixture <dir>] [--skill <name>]... [--timings] [--json]
skill-studio diagnose     (same flags)
skill-studio capabilities [--harness <id>]... [--observe] [--tool <name>]... [--json]
skill-studio watch        [--home <dir>] [--fixture <dir>] [--since <revision>] [--json]
skill-studio schema       [--out <dir>]
```

- `scan` inventories every installed skill.
- `diagnose` inventories and derives issues (broken links, spec violations,
  repairable frontmatter, drift, duplicates, parked and disabled skills).
- `capabilities` reports what each harness supports, per the built-in
  catalog; `--observe` also probes the machine (config presence, runner
  binary on `PATH`).
- `watch` polls the scope every 200ms and prints one JSON line per change,
  each carrying the new revision and the full inventory that revision names.
  The watcher already holds that inventory, so it sends it instead of making
  the reader run its own `scan`; a revision-only line would force that scan
  to race the watcher and label newer bytes with an older revision.
  `--since <revision>` suppresses the initial line when the revision already
  matches what the caller last saw. `Ctrl-C` exits 130, the same code a
  cancelled read or write command uses.
- `schema` writes one JSON Schema file per request/result DTO (via
  `schemars`) into `--out` (default `crates/skill-studio-core/schema`).

## Scope flags

- `--fixture <dir>` scans an in-memory-shaped fixture directory instead of
  the real machine (`RuntimeScope::fixture`); its lease root is
  `<dir>/.history/leases`.
- Without `--fixture`, the scope is `Live`, rooted at `--home` or the host's
  home directory. Its lease root is `<data_root>/leases`, where
  `data_root` is `$XDG_DATA_HOME/skill-studio` or
  `~/.local/share/skill-studio`. The core itself never reads `HOME` or
  calls `dirs`; this crate is where that happens.
- `--project <dir>` (repeatable) adds explicit projects. Without one,
  projects are discovered through `TranscriptProjectDiscovery`.

## Output

`--json` prints the `ResultEnvelope` as one compact JSON document with a
trailing newline, and exits with `envelope.exit_status()`. Without
`--json`, it prints a short human table (skills, deployments, issues) and
uses the same exit status. `--json` never prompts and never colors.

An error — including a scope that fails to normalize, such as
`--fixture /nonexistent` — still prints a full envelope with
`status: "error"` and `data: null` in JSON mode; the CLI never panics
bare. Every invocation carries a fresh ULID `correlation_id`.

### Exit statuses

| Exit | Meaning                                                                                                                                                |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 0    | `status: "ok"`, no issues (`scan`, or `diagnose` with no issues).                                                                                      |
| 1    | `status: "ok"`, but `diagnose` found an issue at `Warning` severity or above.                                                                          |
| 2    | `status: "error"`, a request- or scope-shaped error (`invalid_request`, `invalid_scope`, `ambiguous_target`, `unsupported`, `execution_failed`, `io`). |
| 3    | `status: "error"`, a concurrency conflict (`scope_busy`, `stale_proposal`, `drift_conflict`, `ownership_changed`, `already_reverted`).                 |
| 4    | `status: "partial"` — some scope roots were not read in the read-timeout budget; see `data.observations`.                                              |
| 130  | `status: "error"`, `code: "cancelled"`.                                                                                                                |

The mapping is exact: it is `ErrorCode::exit_status()` in the core, tested
there and exercised end-to-end in `apps/cli/tests/envelope.rs`.
