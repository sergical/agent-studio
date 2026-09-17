# Copy trial expiry and retained-backup Restore

Base: df4480e (PR135). Candidate hashes are retained in the task temporary
copy-trial/candidate-source.json until the final commit identifies the candidate.

Due owned Copies expire through a durable backup/removal operation. Notification
and Activity restore the exact retained backup to Global. Restore retains backup,
does not recreate Copy/trial ownership or original project readers, and is not
Undo. Neither expiry nor Restore offers generic Undo or force overwrite.

## Automated checks

Run from this review checkout. Cargo uses --locked --offline -j2 and tests use
--test-threads=1. Environment: CARGO_BUILD_JOBS=2, CARGO_NET_OFFLINE=true,
CARGO_TARGET_DIR=/tmp/skill-studio-delivery/cargo-target, CARGO_INCREMENTAL=0,
CARGO_PROFILE_DEV_DEBUG=0, CARGO_PROFILE_TEST_DEBUG=0. Node heap ceiling3072MiB.
One task-owned heavy workload runs at a time. Measurements are maximum single-process
RSS, not application memory or aggregate process-tree peaks.

- Core `cargo test --manifest-path crates/skill-studio-core/Cargo.toml --features
  event-store --lib trial`:57 passed,1 explicit disk-image mount skip;115.27s tests,
  129.69s wall,RSS1,231,618,048bytes. Covers expiry admission/effect/recovery and
  Restore claim/cancellation/conflict/publication cases plus adjacent trial rules.
- Desktop `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib`
  with `skill_trial::tests`:28passed,0.26s tests,15.81s wall,RSS952,303,616bytes;
  `event_commands::tests`:14passed,0.14s tests,10.57s wall,RSS999,620,608bytes;
  `skill_startup_recovery::tests`:7passed,0.04s tests,1.06s wall,RSS139,231,232bytes.
  These include retained legacy cases; native acceptance must prove new IPC wiring.
- Strict all-target Clippy: core(event-store)16.06s/RSS852,099,072bytes;
  desktop33.07s/RSS747,749,376bytes. Both format checks pass.
- Desktop typecheck passes (0.51s,RSS381,124,608bytes). Scoped oxlint/oxfmt pass.
- `npm run test -w skill-studio -- src/lib/skill-api-trial-events.test.ts
  src/lib/skill-document-operation.test.ts --maxWorkers=1`:12passed,478ms Vitest,
  1.36s wall,RSS159,105,024bytes. Listener validation/disposal, toast identity,
  operation transport and cancellation. History policy `SkillHistorySection.test.ts`:
  3passed,134ms Vitest. Its peak memory was not collected.

Timed checks reported zero swaps. No full local repository suite was run.

## Extraction corrections

The older review branch needs the existing Copy registry revision helper and
fresh snapshot project list rather than later deployment/state APIs. The expiry
loop retains recovery-before-fresh-work, unresolved-work gating and one short
lock retry. Restore retains the document-write transaction and actionable recovery
errors. The frontend includes the Restore operation union, listener cleanup and
cancellation callback. Cloned workspace symlinks now resolve into this checkout.
An unused Copy removal-restore module was excluded during final review; the
active retained-trial restore uses skill_trial_restore and skill_trial_restore_event.
Linux CI remains required.

## Native acceptance and review

Partial native acceptance passed on binary
`c4a741b0c23691543763197f3954ca98d673904ab901b4dadc8f52109962707b`.
The initial not-due fixture retained both Copies and original bytes. A due
per-harness fixture was refused with a visible reason; the fixture was corrected
to the supported Universal layout without changing production source.
Global expiry removed the Copy and readers, and Activity Restore to Global
completed after its confirmation dialog. Both events are done:
`01M2PFYW733K0P69PGWZZQQ08N` (expiry) and
`01M2PG0FN0VGSNK41PGYQHC64T` (Restore). Original document/resource hashes and
relative resource link are preserved; backup remains; Copy/trial ownership is
absent; the original Codex reader remains absent; unknown registry data survives.
Neither event has a generic inverse. Project expiry and notification Restore subsequently passed on the same binary.
Exact content returned to Global; original project readers stayed absent; no Copy
or trial ownership returned. A third project fixture expired, then its source
project was unregistered before Activity Restore. An occupied Global destination
was refused with `Restore target or its durable staging entry already exists`;
the external file survived and no Restore event was added. Removing only that
fixture collision permitted retry.

A fixture SQLite trigger refused the final Restore status update. Native Activity
showed pending and the error explicitly required recovery; published content
matched the original. After stopping the app, removing the trigger and restarting,
the same event `01M2PG85JTZKAYVF5462PQ5940` settled as done. No duplicate Restore
was created; content and backup remained exact. This tests a durable finalization
failure, not a process kill at every instruction. Changed-evidence conflict and
cancellation are covered by focused core tests, not additional native fault cases.
All managed native sessions exited. Fixture results remain temporarily available
for independent review; the copied app and fixture will be removed after review.
Native fixtures/procedure are in /tmp/skill-studio-delivery/copy-trial.
The build uses empty desktop telemetry DSNs. No production release, remote telemetry
receipt, signing or combined final release acceptance is claimed. Fresh simplification extracted one identical Global/Project label helper in
skill_trial.rs. Post-change desktop trial tests: 28 passed, 0.15s test time,
25.58s wall, maximum single-process RSS995,606,528bytes, zero swaps. Formatting
and diff checks pass. Other source is unchanged from automated/native acceptance;
the native binary predates this behavior-preserving label extraction. Independent review found no active-flow correctness or security defect. Its one
scope finding was resolved by excluding the unused skill_copy_restore module and
export (integration work preserved). Activity action rendering and expiry DTO state
selection lack dedicated automated tests; native checks cover the current routing.
Linux CI and final combined acceptance remain required. Final review confirmed the scope finding resolved. After removing the unused export,
core check and formatting passed (3.51s wall, RSS607,469,568bytes, zero swaps).
