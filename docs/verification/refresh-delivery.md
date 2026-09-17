# Desktop refresh completion

Manual Sync now waits for the full scan requested by that action. A newer
invocation or targeted snapshot cannot falsely complete it. Full scans record an
instance-bound decimal generation; partial publications preserve earlier coverage
without advancing it. Requests made during a scan remain pending for the next scan.
Failed scans retain demand and use the existing retry backoff.

The UI shares one pending wait, handles an event arriving before its command
response, and uses two bounded cached reads for missed events. The 120-second
wait limit reports an error without cancelling background work. Disposal removes
wait timers and ignores late results. Repeated registration of unchanged projects
no longer requests another scan. Watch reconciliation checks physical paths and
file identities so replacing a root does not leave a stale registration.

## Acceptance

- 40 focused Rust tests pass, including concurrent demand, failure/retry,
  generation serialization, counter exhaustion, repeated registration, directory
  replacement, and existing diagnosis/targeted-refresh behavior.
- 16 frontend tests pass across the waiter, snapshot subscription and Sidebar
  helpers. Cases include partial coverage, concurrent callers, early/lost events,
  instance mismatch, timeout, disposal and request failure/retry.
- TypeScript, changed-file lint/format, strict all-target desktop Clippy and the
  native app build pass. Fresh simplification made no changes.
- The marketing capture mock publishes and returns matching receipt coverage. Its
  scoped lint/format and `tsc -p packages/marketing/capture/tsconfig.json --noEmit` pass.
- Native CUA observed Sync disabled while pending and enabled after completion.
  Replacing the watched root updated the list from 100 skills to two without Sync.
  Editing a nested document then updated its description automatically.

Native fixture binary SHA-256: `80b8d334c2d9abcf7f59042e5489837a3a643bb72f1c6039abd7db4122d48742`.
The fixture uses isolated HOME, denies network and real HOME reads, disables
telemetry, and is launched through a managed `open -W -n` process. Ad-hoc signing
and a fixture-only bundle identifier are not release signing/notarization.

## Reproduction and resource use

From the repository root:

```sh
cargo test --locked --offline -j2 --manifest-path apps/desktop/src-tauri/Cargo.toml --lib skill_refresh -- --test-threads=1
./node_modules/.bin/vitest run apps/desktop/src/lib/skill-refresh-waiter.test.ts apps/desktop/src/hooks/useSkillSnapshot.test.ts apps/desktop/src/components/Sidebar/Sidebar.test.ts --maxWorkers=2
npm run typecheck
cargo clippy --locked --offline -j2 --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
npm run tauri -- build --debug --bundles app
```

Runs used one heavy job at a time, two Cargo workers, one Rust test thread,
no incremental compilation or debug symbols, a shared external Cargo cache, and
3072 MiB Node heap cap. Core/desktop dependency and resource inputs were reused
from the prior reviewed branch; packaged runtime verification passed.

| Check | Wall time | Max process RSS |
| --- | ---: | ---: |
| Focused Rust | 28.62 s | 1,059,045,376 bytes |
| Frontend | 0.54 s | 117,538,816 bytes |
| TypeScript | 1.30 s | 352,272,384 bytes |
| Desktop Clippy | 15.43 s | 794,918,912 bytes |
| Native build | 34.21 s | 1,130,250,240 bytes |

All measured commands reported zero swaps. Application peak memory was not
measured. All test/build/native processes exited. Consumed fixture bundles, homes
and skill trees were deleted; compatible dependency caches remain for reuse.
Concise results, exact source hashes and native reproduction scripts remain under
`/tmp/skill-studio-delivery/refresh`.

## Limits

Forced native failure and event ordering were not injected into the UI; those
contracts have automated evidence. The native check proves directory replacement,
not all filesystem watcher behaviors. A missed event after the final cached read
can still cause a timeout. Old-instance receipts do not complete after a restart.
The ordinary root-link warning is expected in this fixture. Monitoring, release
signing, final combined application acceptance, merge and deployment remain separate.
Independent final review reports no remaining actionable findings. Remote CI status
is recorded on the PR and main ledger.
