# Desktop shared-core consolidation

Base: PR180 `bcdb8645dc05c95678b64c9d4b5ad6ff782d1d87`.

Desktop now uses the core implementations for ledger reading, plugin discovery,
provenance, assembly, candidates, deployments, discovery, fork registry and
ownership. Nine duplicate desktop modules are removed. Caller adapters use explicit
read scopes for content hashes and retain cancellation checks and read warnings.
Changing the preferred editor refuses malformed registry input without overwriting
ownership records. No new lifecycle command or IPC schema is introduced.

## Acceptance

Focused desktop tests pass: Add43, install-plan7, lifecycle9, editor5, refresh35.
Strict desktop Clippy (`-D warnings`), Rust format and `git diff --check` pass.
Exact commands, durations, process RSS and source identity are recorded in
`/tmp/skill-studio-delivery/core-cutover/focused-results.json` and
`source-hashes.json`. The largest measured focused test process RSS was
1,030,782,976 bytes. The initial test compile required adapting test-only
callers to the core interfaces; checks passed after those edits.

Native CUA acceptance used an isolated home, denied real-home access and network,
and disabled telemetry. Manual Sync showed pending then complete. Copy Park and
Unpark changed the UI and passed disk checks for exact file bytes, resource links,
reader links, ownership records and the same-name project sibling. The eight
fixture resource links produced eight discovery warnings; moving those links
outside scan roots removed that banner on the same binary. The mixed-ownership
label across the global Copy and project sibling was observed, not independently
accepted as correct by this run.

Native build:28.04s, maximum process RSS1,038,974,976 bytes, zero swaps.
Unchanged PR180 frontend output was reused with matching hashes. Application peak
memory and input latency were not measured. Both managed native sessions exited0;
the consumed fixture was removed after retaining identities, procedures and results.

## Limits

This is batch acceptance, not final combined release acceptance. Monitoring wiring,
remote telemetry receipt, source symbolication and release signing are separate.
CLI/MCP changes and alternative worker/operation implementations are excluded.
Native checks here do not repeat every previously accepted lifecycle case.

Fresh simplification found no worthwhile changes. Final independent review found no actionable correctness, security, compatibility,
regression or material test-gap findings. Source hashes matched the reviewed code.
