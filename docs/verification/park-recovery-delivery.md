# Park and Unpark recovery

Park and Unpark use shared-core operations with a durable pending event before
filesystem changes. Recovery checks the recorded trees, links and ownership before
continuing. Unpark preserves a recreated live skill and retains the parked original
in unique recovery storage. Registry transitions preserve unknown fields and
supported legacy parked records. Park suspends the selected Copy or Fork owner;
Unpark restores it only when the live state permits that restoration.

Desktop commands run off the UI thread. History records both actions without a
Restore button. Ordinary Park/Unpark remains the supported reversal. Reconciliation
does not replace the provider installation through History Undo. Redundant staged
reader links move to `.agents/skills-parked-links-retained/<name>-<event-id>` using
a guarded non-overwriting move. Retained content is not automatically deleted.

## Verification

The review candidate passes:

- 17 focused core tests: registry compatibility, exact owner/trial transitions,
  trial ambiguity, malformed intent refusal, reader reconciliation, and recovery.
  The checkpoint test covers whole-folder, duplicate per-skill and missing readers
  after tree and reader transitions. Other interruption checkpoints cover ordinary
  Park/Unpark and changed content refusal.
- 10 desktop tests, including real pending Park and Unpark events dispatched through
  startup recovery, existing ordered recovery checks, and trial expiry after reversal.
- Strict all-target core/desktop Clippy, scoped History lint/format, and diff checks.
- Fresh simplification (no edits) and independent correction review (no remaining
  actionable findings).

Commands from the repository root:

```sh
cargo test --locked --offline -j2 --manifest-path crates/skill-studio-core/Cargo.toml --features event-store --lib skill_park_ -- --test-threads=1
cargo test --locked --offline -j2 --manifest-path apps/desktop/src-tauri/Cargo.toml --lib -- skill_park::tests skill_startup_recovery::tests expiry_after_park_and_unpark_removes_the_restored_exact_claude_link --test-threads=1
cargo clippy --locked --offline -j2 --manifest-path crates/skill-studio-core/Cargo.toml --features event-store --all-targets -- -D warnings
cargo clippy --locked --offline -j2 --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
npm run tauri -- build --debug --bundles app
```

Builds use a shared external Cargo target, two workers, no incremental compilation,
no debug symbols and a 3 GB Node heap cap. The packaged provider runtime is verified
before desktop checks. The isolated review checkout uses its locked npm dependencies.

| Check | Wall time | Maximum process RSS |
| --- | ---: | ---: |
| Core tests | 33.49 s | 1,487,585,280 bytes |
| Desktop tests | 21.49 s | 1,066,450,944 bytes |
| Core Clippy | 9.54 s | 1,059,749,888 bytes |
| Desktop Clippy | 10.37 s | 788,217,856 bytes |
| Native build | 34.89 s | 1,051,033,600 bytes |

All measurements above reported zero swaps. They are single-process resource
measurements, not application peak-memory or total machine memory measurements.

## Native acceptance

Final review-branch binary SHA-256:
`7e62e7b1161d9c6d5c103e4c1d07f62936abda0fc02a4d2be604c7198d76e44c`.

Native controls were operated with CUA in an isolated HOME, with network denied,
real HOME reads denied and telemetry disabled. Fixtures contain document/resource
files, valid and dangling links, and an unrelated same-name project skill.

1. Park `park-identical`; externally recreate its exact tree and matching per-skill
   reader while the original reader remains staged; click Unpark. Exact content,
   recreated reader and project sibling remain unchanged. The old reader is retained,
   the parked record clears, and both UI controls return to enabled.
2. Park `park-divergent`; run packaged Dotagents against a local Git source with a
   changed document; click Unpark. The provider live tree and raw provider records
   remain unchanged. The whole-folder reader link keeps its target and inode. The
   old tree and staged reader are retained at operation-specific paths. The UI shows
   Dotagents Managed and enabled. All four History events are done and nonrestorable.

Earlier integration binaries additionally passed Manual/Copy ordinary round trips,
real skills installer reconciliation and a controlled completion failure followed
by native restart. Those observations are tied to their earlier binary identities;
the final candidate has automated checkpoint and startup coverage plus the two
native regression cases above. This is not final combined application acceptance.

## Evidence and limits

Concise results, source hashes and native reproduction scripts are retained in
`/tmp/skill-studio-delivery/park-recovery`: `review-results.json`,
`review-source-hashes.json`, `final-native-duplicate-reader.json`, and
`final-native-provider.json`. Native session 98996 exited successfully. Test/build
processes exited. Consumed fixtures and logs are cleaned after retaining summaries.

Provider commands used local Git fixtures; remote provider availability was not
verified. Discovery reported unrelated fixture warnings, so this packet does not
claim complete discovery acceptance. Signing, production telemetry, merge and
deployment are outside this batch. CI status is reported on the draft PR.
