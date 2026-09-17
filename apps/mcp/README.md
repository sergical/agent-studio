# skill-studio-mcp

A stateless local MCP server over `skill-studio-core`, stdio transport. Each
tool call builds a fresh `RuntimeScope` and `Ports`, re-reads disk, runs one
core operation, and drops everything — restarting the process between two
calls gives identical results (see `apps/mcp/tests/mcp_server.rs`,
`restart_gives_the_same_envelope_as_the_first_run`). No sessions, no
subscriptions, no cache between calls (MCP revision 2026-07-28, decision
D13).

## Tools

One tool per core operation, named `snake_case` from `Operation`. Every
tool's input schema is the core's own request DTO's `schemars` output,
unchanged; every tool's output is the same `ResultEnvelope` JSON
`skill-studio` (the CLI) prints, as `structured_content`, with `is_error`
set to match `envelope.status == "error"`.

| Tool                         | Core op                           | Request DTO            |
| ---------------------------- | --------------------------------- | ---------------------- |
| `scan`                       | `ops::scan`                       | `ScanRequest`          |
| `diagnose`                   | `ops::diagnose`                   | `ScanRequest`          |
| `capabilities`               | `ops::capabilities`               | `CapabilitiesRequest`  |
| `preview_frontmatter_repair` | `ops::preview_frontmatter_repair` | `RepairPreviewRequest` |
| `apply_frontmatter_repair`   | `ops::apply_frontmatter_repair`   | `RepairApplyRequest`   |
| `list_events`                | `ops::list_events`                | `ListEventsRequest`    |
| `restore_event`              | `ops::restore_event`              | `RestoreRequest`       |

`apply_frontmatter_repair`, `list_events`, and `restore_event` open a real
history store and take the exclusive lease the same way `skill-studio
apply-repair`/`events`/`restore` do; if another process is already holding
it, the call returns the ordinary `scope_busy` error envelope (bounded
wait, never an indefinite hang), the same as the CLI.

A core error is never a panic and never a bare string: it comes back as
`structured_content` carrying the envelope's `errors` array, with
`is_error: true`.

## Progress

A call whose `_meta.progressToken` is set receives at least one
`notifications/progress` message (one at the start, one at completion) over
the call's `peer`. A call with no progress token receives none. The core
does not currently emit granular per-op progress; this is start/done
bracketing around the call, not a step counter.

## Environment variables

Scope resolution follows the scope, never the process: an explicit home or
fixture derives its own history root and lease root from itself; only the
true default (neither variable set) resolves against the real machine. The
core itself never reads the environment — `apps/mcp/src/scope.rs` is where
that happens, mirroring `apps/cli/src/scope.rs`'s flag-driven version.

| Variable               | Effect                                                                                                               |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------- |
| `SKILL_STUDIO_FIXTURE` | Fixture scope rooted at this directory. Lease root: `<dir>/.history/leases`.                                         |
| `SKILL_STUDIO_HOME`    | Live scope rooted at this directory instead of the real home. Lease root: `<dir>/.skill-studio/leases`.              |
| `SKILL_STUDIO_PROJECT` | A `PATH`-style list of explicit project directories (`:` on Unix, `;` on Windows). Optional with either scope above. |

Neither variable set: the real host home (`dirs::home_dir()`), with the
ambient `$XDG_DATA_HOME/skill-studio` (or `~/.local/share/skill-studio`)
data root — the same rule `apps/cli/src/scope.rs::data_root` uses.

## Example client configuration

```json
{
  "mcpServers": {
    "skill-studio": {
      "command": "/path/to/skill-studio-mcp",
      "env": {
        "SKILL_STUDIO_HOME": "/Users/you"
      }
    }
  }
}
```

Point `SKILL_STUDIO_FIXTURE` at a materialized fixture directory instead,
for a read-only sandboxed session against a known tree rather than the real
machine.

## Testing

`apps/mcp/tests/mcp_server.rs` spawns the built binary as a real child
process over stdio (via `rmcp`'s `TokioChildProcess` client, and, for the
one case that client cannot exercise — a call with no progress token at
all — a raw JSON-RPC exchange over the same stdio pipes) and covers:
restart equivalence, statelessness within one process (a file added between
two calls on the same server), error mapping for an invalid scope, progress
notifications with and without a token, that `SKILL_STUDIO_HOME` never
writes outside itself, and that `skill-studio watch` and a fresh MCP `scan`
agree on state after a mutation applied through the CLI.
