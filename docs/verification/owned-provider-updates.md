# Owned provider updates

September 16, 2026. Review base: `60bf50f` (Browse installation PR101).
Source identity for the 14 changed code/test files, sorted path + NUL + bytes + NUL:
`06b48b33b45b00f06b31c68f53f54957c6b88bd2835e795ff7744ce13dc67e38`.

## User outcome

Update uses the selected owner and scope, matches cached evidence to its current
repository/path/ref/baseline, and runs a bounded provider outside the async executor.
Provider completion, including failure, requests owner evidence and inventory refresh.
The document shown on screen refreshes when its content changes. Unknown evidence
cannot authorize Update; the store drawer shows the refusal reason.

## Verification

Focused Rust commands, from `apps/desktop/src-tauri`, used two Cargo workers,
one test thread, and the existing compatible target cache:

- `cargo test -q skill_update_check --lib -- --test-threads=1`: 28 pass; final 0.13 s test execution. Includes root-path forks, exact-source invalidation, independent same-name owners, project computed hashes, failure normalization and targeted merging.
- `cargo test -q skill_refresh --lib -- --test-threads=1`: 31 pass, 0.65 s test execution.
- Command filters `update_inputs_refuse_only_the_target_scope_and_shared_registry_failures` and `fresh_update_evidence_requires_a_source_matched_current_difference`: one each passes.
- `cargo test -q skill_process --lib -- --test-threads=1`: seven pass; timeout/descendant reaping, cancellation, shared deadline and output bound. 4.31 s wall, max single-process RSS 129,220,608 bytes, zero swaps.
- `cargo clippy -q -- -D warnings`, `cargo fmt --check`: pass.

Frontend direct Vitest uses `--maxWorkers=1`: lifecycle targets 20, caller selection
and rendered drawer five, source ledger 17 pass. The drawer rendering asserts a
disabled Update and the exact Unknown reason. Final drawer run 618 ms, source ledger
233 ms. `npm run typecheck`, scoped direct `oxlint`, `oxfmt --check`, and
`git diff --check` pass. Other test peak-memory measurements were not collected.

Fresh simplification removed a redundant borrow and duplicate comment. Independent
review found and resolved root-path evidence mismatch, app-data-directory fallback,
and hidden Unknown reasons. Final reviewer reported no remaining findings.

## Native acceptance

Actual Tauri app, isolated HOME and project; real-home reads and network denied.
Controlled local GitHub/provider fixtures exercise application integration, not the
live upstream providers. Project added with the native folder picker; the external
fixture ownership boundary was registered through Settings. Startup supplies the
background update check. No manual Check now UI is claimed.

Final binary SHA256:
`8e08d83c4b4dbcb4495a0bba697d3f007d6edc27cf8f28af5caec892debeb85e`.

- Pinned Global dotagents: correct owner/ref argv; Update disappears; metadata becomes Up to date and body becomes Updated content without reopening.
- Project dotagents with same-named Global owner: only Project remains actionable; provider cwd is the selected project and argv has `--project`; both scopes retain unrelated resource files.
- Unpinned dotagents: successful update without a ref argument; refreshed body/status.
- Global skills.sh: successful provider update, refreshed body and Up to date.
- Partial-write failure: failure message explicitly says files may have changed; changed body appears; busy state clears and retry remains available.
- Empty stderr/nonzero exit: useful `npx exited with code 7` diagnostic, no stuck busy state.
- Slow provider: Settings and Skills navigation work while the fixture PID is confirmed live; update subsequently completes.
- Project content hash: checker records UnknownWithReason (no proved Git commit baseline); native detail shows Unknown and no Update action.

The first native pass found stale document text and false update counts; those
were corrected and the final pass above repeated their acceptance cases.
Process-launch failure is handled by the shared runner but has not had a dedicated
native injection in this packet. No application-kill/OS-restart provider rollback
is claimed; refresh on next startup reads actual files and manager records.

## Limits and resources

Provider Update has no Undo or rollback guarantee. A declared dotagents ref uses
the existing re-pin-to-resolved-SHA behavior. A project content hash is not a Git
commit; that owner remains non-actionable. Ownership-input read failures make the
checker run Unknown without network lookup, including unrelated scopes, by the
current conservative policy. Fork/Pull behavior is preserved, not newly delivered.

Final frontend build: 3.53 s, max single-process RSS 1,051,803,648 bytes. Final
native build: 58.55 s, max single-process RSS 1,558,790,144 bytes. Both report zero
swaps. Preflight: 51% memory free and 58 GiB disk available. Node heap capped at
3072 MiB; one heavy workload at a time. These are build measurements, not a native
application memory benchmark.

App/provider/build processes exited. Disposable fixture cleanup and PR CI results
will be recorded before batch acceptance. No merge or deployment performed.
