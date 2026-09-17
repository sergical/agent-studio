# Shared diagnosis and named refresh acceptance

Review branch: `codex/diagnosis-delivery`, based on PR142 commit `56fc9ad`.

Home and Skills use the same shared-core diagnosis instead of separate frontend
scans. Findings cover broken links, blocking frontmatter errors, differing copies,
linked skill roots, reinstalled parked skills and tracked records without files.
A missing record is confirmed absent only when the relevant scan coverage and
ownership inputs are complete. Pending, partial or unresolved findings prevent
Home from claiming all clear. Finding links select the affected deployment.

A named refresh replaces selected rows only after validating the complete change.
It preserves unrelated row metadata and findings. Invalid selections, partial rows,
conflicts or a missing diagnosis baseline refuse publication and request a full
refresh. This is a read-only reporting batch; existing lifecycle actions remain.
CLI, MCP and unrelated refresh scheduling changes are excluded.

## Focused verification

Use the shared Cargo target with incremental compilation disabled, dev/test debug
information disabled, two build workers, offline locked dependencies and one test
thread. Commands below use `cargo test --locked --offline -j2 --lib`; core enables
`event-store`, desktop enables `worker-repair`.

| Check | Result |
| --- | --- |
| Core `skill_diagnosis` | 5 passed |
| Core `skill_reconciliation` | 3 passed |
| Desktop `skills::skill_refresh::tests` | 33 passed |
| Vitest diagnosis presentation, health, list filter and Home recovery/preview | 39 passed across scoped runs |
| Desktop typecheck | Passed |
| Changed frontend file lint | Passed |
| Strict core and desktop all-target Clippy | Passed |
| Fresh simplification | No edits |

The new checkout required the existing pinned bundled runtime. Copied test setup
was adapted to the branch's snapshot-builder arguments and complete replacement
selection. These setup/test fixes did not change production behavior. No full
local suite was run. Passing checks were reused while relevant inputs stayed fixed.

Core diagnosis check: 14.84 s wall, 1,382,694,912-byte maximum single-process RSS.
Desktop refresh: 25.19 s and 1,060,569,088 bytes. Frontend tests: 1.33 s and
194,183,168 bytes. Desktop Clippy: 14.79 s and 781,434,880 bytes. These commands
reported zero swaps. Application peak memory was not measured. Concise results
and source identities are under `/tmp/skill-studio-delivery/diagnosis`.

## Native acceptance and review

Isolated Tauri acceptance used a separate home, denied network and real-home
access, and empty telemetry DSNs. First candidate acceptance covered YAML and
copy findings, exact Home deployment links, missing-record search, repair-triggered
refresh, malformed ownership input, and unreadable discovery roots. Partial scans
show qualified absence and suppress all-clear; restoring the input clears warnings.
The watcher can race the repair-triggered refresh, so named-only reconciliation is
proved by the focused desktop test, not inferred from the native observation.

Review corrected three issues: same-name findings leaking across project/harness
filters, lost retry demand after a failed background scan, and unbounded Home
ledger previews. Full refresh failures retain demand and retry after five seconds;
the controlled Rust regression covers throttling, success and mid-scan requests.
Home displays at most six combined warning/ledger rows and counts all findings.

Corrected native binary SHA-256:
`74988c0677e443749f63201b0a8fdcc27e72617c166886bcc53ce9366cd01c19`.
Home displayed three skill warnings plus three ledger rows with Show all 24.
Show all opened all findings. Filtering to healthy project-a produced zero rows;
project-b produced one row and opened its malformed document, not project-a's
same-name copy. Harness scoping is covered by frontend regressions.
Build: 31.84 s, 1,083,703,296-byte maximum single-process RSS, zero swaps.
The managed native app exited successfully. These are functional debug-build
checks, not release performance or production monitoring evidence.

Fresh correction simplification made no edits. Independent review found one further
parked-scope regression: active reinstallation evidence was filtered out. The three-file
correction preserves that evidence for parked skills and keeps normal parked row
navigation unchanged. Its 25 relevant frontend tests, lint and typecheck pass; other
unchanged checks are reused. Final independent correction review is clear.

Final native binary: `98aa7ac16c9a51658a8b2f7c422f85056d31150bfc5b16ed2927a40c06f7797a`.
The Parked sidebar displayed the reinstalled skill; its normal row opened the parked
copy. Home's reinstallation warning opened the active Codex copy. The warning preview
remained bounded. Build: 17.21 s, 1,083,162,624-byte maximum single-process RSS, zero
swaps. App exited cleanly. Consumed native bundles, homes and temp directories were
removed after retaining identities, results and fixture reproduction scripts.

CI and final combined release acceptance remain separate requirements.
