# Home recovery status acceptance

Review base: PR109, `f38a05ef05b5ff485f58df080033d7c0173ab737`.

Home reports unresolved interrupted changes with a View Activity action. Loading
and unavailable recovery status cannot display All clear. A database existence
query covers full history; pending operations and completed restores are excluded.
Failed restores keep the original interrupted event visible. Snapshot revisions
refresh the async query without polling; bursts coalesce and stale replies cannot
replace current status.

## Native acceptance — September 16, 2026

Isolated Tauri app, real-home reads and network denied. Binary SHA256:
`3e9dd908d53135faa810a65fe3ecd898f3f00d10f80ca95c73ca3dc3877e4300`.

- Retained PR109 conflict fixture showed Some skill changes need review on Home,
  without All clear. View Activity opened the actual Interrupted history entry.
- With the app stopped, moved only the task fixture replacement aside. Native
  restart performed the existing recovery: restored original, removed quarantine,
  preserved replacement separately, and marked event failed. Home displayed All
  clear. Event `01M2N8CY9JQ8QVXYFWTDDE1PGD` was inspected before/after through a
  read-only SQLite connection. No fabricated event status was used.
- A directory at the fixture database path caused EventStore initialization to
  fail. Home displayed Could not check interrupted skill changes and Retry,
  without All clear. Retry retained unavailable feedback. AX click lookup failed
  twice; screenshot-coordinate click was used. Retry does not reopen a store that
  failed initialization; fixing that failure requires an application restart.

Fixture preparation: `/tmp/skill-studio-delivery/recovery-status/prepare.py`.
The conflict input originated from PR109's retained production-checkpoint fixture
hook; it is consumed by the successful recovery case. Recreate that input with
PR109's documented generator rather than reusing the resolved database.

## Focused verification

From the review worktree:

- `npm run test -w skill-studio -- --maxWorkers=1 src/hooks/useInterruptedSkillEvents.test.ts src/components/Home/HomeRecoveryStatus.test.tsx`: 6 passed, 348ms total.
- `npm run typecheck -w skill-studio`: passed.
- Scoped oxlint/oxfmt on changed frontend paths and cargo fmt: passed.
- `cargo test --manifest-path crates/skill-studio-core/Cargo.toml --features event-store --lib interrupted_status_ignores_history_limit_and_resolved_events -- --test-threads=1`: 1 passed, 4.00s compile / 0.10s test.
- `cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --lib -- -D warnings`: passed, 4.38s.

Root inspected exact-worktree logs after correcting the remount lifetime and test
fixture issues. Tests cover history beyond 200 rows, resolution exclusions, retry,
stale replies, remount lifetime and burst coalescing. Native acceptance covers
actual navigation and startup recovery wiring. No new recovery mutation or Undo
capability is introduced.

## Resources and review status

One heavy job at a time, two Cargo workers, one Vitest worker, Node heap3072MiB,
shared compatible Cargo cache. Preflight51% memory free,100GiB available disk,
10996/276480 open file handles. Frontend build3.85s / max single-process
RSS1,012,940,800bytes. Native build78.61s / RSS1,445,216,256bytes; zero swaps
reported by both builds. Test peak memory was not measured. Build measurements
are not application runtime memory measurements.

Native app exited. Fresh simplification made no edits and all supplied focused
checks passed. Pre-correction non-document source identity (sorted relative paths, NUL,
bytes, NUL) is `1f7aec2d59763085eb4909b830430c2cb75e4d0576aefffad4df467b91ac3f5e`.
Independent review found a missing warning when inventory had no snapshot. The
correction renders the recovery notice during loading and read failure too;
Home-level server-render tests cover both states (4 Home tests pass,755ms total).
Scoped lint/format and typecheck pass. Follow-up review found no new issue. The
small two-file correction did not trigger another simplification or full suite.
Final native build SHA256 is
`33013d7e776ee8a6bf8c9acac396708cf9c8c3281ef2b21d9b40a929779031ba`.
Native unavailable/Retry acceptance passes after the correction. Final source
identity is `c34aca2d2ac93f96744a1660ddf80a1436ca3685e96e9424907b5ca1aa9cfe8b`.
Frontend build3.56s/max RSS1,045,381,120bytes; native54.94s/max RSS
1,509,818,368bytes, zero swaps. The unchanged recovery paths retain prior native
evidence; the no-snapshot branch is covered by component
composition tests, not native fault injection.
Commit and draft PR follow this accepted review. CI and integration reconciliation
remain pending. Temporary binary, fixtures and raw logs are removed after retaining
concise results and reproduction inputs. Final combined release acceptance remains separate.
