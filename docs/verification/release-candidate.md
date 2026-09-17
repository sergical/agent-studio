# Combined candidate: desktop monitoring integration

This candidate combines reviewed desktop/core PR181, API PR89, marketing PR90,
Rust telemetry PR92 and release preparation PR91. It adds desktop monitoring
startup/shutdown, React error capture and correlated inventory/History reads.
CLI, MCP and cloud-agent work is excluded. No production merge or deployment
has occurred.

## Review

Fresh simplification found no worthwhile change. Independent final review was
clear for startup/shutdown, IPC tracing, bounded History reads, React privacy
filters and dependency changes. Source hashes remained unchanged, so green
checks were reused. Production monitoring and full release acceptance remain open.

## Verification on this candidate

| Check | Result | Wall time | Maximum process RSS |
| --- | --- | --- | --- |
| Desktop Rust tests | 632 pass, 0 fail, 9 ignored | 75.69 s | 1,069,907,968 bytes |
| Desktop strict Clippy, all targets | Pass | 12.87 s | 788,938,752 bytes |
| All four workspace test suites | 440 pass, 0 fail | 33.92 s | 213,303,296 bytes |
| Desktop frontend build | Pass | 4.38 s | 1,063,796,736 bytes |
| Marketing build | Pass | 3.71 s | 429,735,936 bytes |
| API build / production-build test | Pass | 0.39 / 0.88 s | 171,851,776 / 97,157,120 bytes |
| Desktop and shared UI lint | Pass | 0.72 s | 202,342,400 bytes |
| Native app build | Pass | 39.28 s | 1,085,784,064 bytes |
| Native Sync, Copy Park/Unpark, History, quit | Pass with limits below | Not timed | Not measured |

Commands ran sequentially, with two Rust build jobs, one test thread/worker and a
3 GiB Node heap limit. Reported swaps were zero. RSS is a per-process measurement,
not total system or application peak memory. Logs and source identities were
recorded under `/tmp/skill-studio-delivery/release-candidate`.

The first Rust run exposed an incomplete test adapter: the Dotagents project
fixture did not declare a complete plugin-ownership boundary. Commit `8565647`
adds that explicit fixture boundary and asserts the expected Dotagents owner.
The original linked-target assertion remains; production behavior is unchanged.
The subsequent complete desktop run passed.

Nine ignored tests are not counted as passes. Two are subprocess helpers;
one requires the built release worker, five require explicit native/provider
fixtures, and one needs GitHub access. Their previous batch evidence does not
substitute for combined release acceptance.

Native verification used a packaged macOS app with isolated HOME, real-home
access denied and network denied. CUA operated Sync, Park, Activity, Unpark and
quit. History showed both operations complete. Disk checks verified exact files,
resource links, Copy ownership, unknown registry fields and a separate project
copy. Tested native binary SHA-256: `398271c0ae5460102a6f01125d1935d5e281f1645e9222a94594257443c99f3e`. The managed app exited and its 219,398,502-byte disposable fixture was
removed. Eight expected discovery issues from resource/dangling links and
existing mixed-owner labels limit this to the stated cases.

## Reproduce

Local toolchain: Node 26.8.2, npm 11.19.1, rustc/cargo 1.92.0, macOS arm64.
Disable telemetry export for normal checks. Run one heavy command at a time:

```sh
npm run test --workspaces --if-present -- --maxWorkers=1
npm run build
npm run build -w @skill-studio/marketing
npm run build -w @skill-studio/server
npm run test:production-build -w @skill-studio/server
npx --no-install oxlint apps/desktop/src packages/lib/src packages/ui/src --deny-warnings
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --locked -j2 --all-targets -- -D warnings
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --locked -j2 --lib -- --test-threads=1
npm run tauri -- build --debug --bundles app
```

Set `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_BUILD_JOBS=2` and
`NODE_OPTIONS=--max-old-space-size=3072`. Offline flags were used locally because
dependencies were cached. Native fixture recipes and concise evidence are retained
under the temporary directory above; no background server is required.

## Remaining release gates

- Complete the fixed lifecycle acceptance matrix on this combined candidate.
  This packet proves only the cases listed above, not every previous batch case.
- Verify production telemetry receipt, redaction, matching source maps/symbols,
  applicable logs/metrics and test retention in the four `sergtech` projects.
  Local fixture-transport tests do not establish production receipt.
- Resolve PR92's GitGuardian finding on the synthetic authenticated-DSN test.
  It has not been dismissed or counted as a passing security check.
- Confirm production API/site destinations, desktop API configuration, signing,
  notarization, distribution targets and rollback rehearsal. See
  [release preparation](../release-readiness.md).
- Finish classification of preserved integration-only changes and remaining
  disposable-artifact cleanup. Existing implementation is retained.

The performance baseline remains complete; this candidate adds no new targets
or extended optimization campaign.
