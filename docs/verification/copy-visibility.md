# Copy enable/disable delivery

Review base: e3f5b47. Candidate source identity is recorded with the final result.

An independent Copy can be moved into its harness holding directory and enabled
again without losing its ownership record. History offers Enable or Disable for
a completed unclaimed visibility change. The inverse refuses changed content or
ownership and never offers force overwrite. Startup settles saved valid moves;
conflicting evidence remains available for recovery. Ordinary harness-native
switches keep their existing mechanisms.

Scope: Copy move transition/intent/execution, guarded event claims, desktop async
visibility commands, History labels/reversal and startup dispatch. Trial expiry,
Restore to Global, worker transport, CLI and MCP are separate.

## Focused verification

Run from the review checkout, with CARGO_TARGET_DIR set to the shared integration
`apps/desktop/src-tauri/target`, CARGO_BUILD_JOBS=2, CARGO_NET_OFFLINE=true and
NODE_OPTIONS=--max-old-space-size=3072. Cargo commands use --locked --offline -j2;
Rust tests use --test-threads=1.

- Core `cargo test --manifest-path crates/skill-studio-core/Cargo.toml --features
  event-store --lib skill_copy_`:71 passed,2 explicit skips;148.68s tests,
  185.71s wall,maximum process RSS1,338,966,016bytes,zero swaps. This includes
  adjacent Copy histories because shared guarded event methods changed.
- Desktop `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib`
  with `event_commands::tests`:13 passed,0.02s tests;59.30s wall including compile,
  RSS1,179,254,784bytes. New History test covers pending/completed/claimed/malformed
  intent, exact inverse label and refusal to force.
- Same desktop command, `skill_harness_disable::tests`:22 passed,0.07s tests,
  1.09s wall,RSS137,986,048bytes; `skill_startup_recovery::tests`:7 passed,0.03s
  tests,0.42s wall,RSS138,723,328bytes. These retain existing adapter/dispatcher
  cases; they do not replace native Copy startup acceptance.
- Strict all-target Clippy passes for desktop and core(event-store). Both Rust
  format checks pass. Core Clippy12.62s/RSS983,138,304bytes.
- Desktop typecheck passes. Existing History restore-policy tests:3 passed,
  118ms Vitest duration. No component-render test suite is claimed. Scoped
  frontend lint/format and diff whitespace checks pass.

Initial desktop compile failed because async adapter extraction omitted the Tauri
Manager trait import. The one-line import correction preceded the passing desktop
checks; core code did not change. Preliminary worker checks are superseded.

## Native acceptance

Packaged debug binary SHA256:
`59f297714e532a3c73c6126f211406694be296e270813b4eca39a5262142c1d2`.
Both desktop telemetry DSNs were empty. Native app automation operated a fresh
HOME/CFFIXED_USER_HOME with network and real-home reads denied by sandbox.

- Global Cursor Copy: native Disable, History Enable, History Disable all completed.
  UI switch and History updated; claimed events lost their inverse. Exact document
  and resource bytes, relative link, other Copy and unknown registry field survived.
- Editing the disabled Copy resource made History Enable refuse with
  “Copy visibility ownership is ambiguous, unsupported or changed”. External bytes
  remained, Copy stayed disabled and no force option appeared. Fixture bytes were
  restored after recording this result.
- Project Cursor Copy: a fixture SQLite trigger refused the final done update.
  Files/registry were disabled correctly, Activity retained a pending event, and
  dirty refresh showed the new disabled path. After clean app exit, removing the
  trigger and restarting completed the SAME event without a duplicate. History
  Enable then restored the exact project Copy. Event:
  `01M2PBVNGXX2MZH9VT4HGSNWCD`; inverse `01M2PBXBE9G5Y524F95STR0KDX`.
- A separate seeded interrupted published Global event plus an external resource
  edit remained interrupted on restart. External bytes survived; Home displayed
  “Some skill changes need review.” Its View Activity action opened the preserved
  event, which offered no inverse. This is seeded conflict coverage, not an actual
  process kill at every publication instruction. Core tests cover move prefixes.

Global round-trip events: `01M2PBRDMMG9NNVR29EJWBQZD4`,
`01M2PBS74F633ZVPEBEPE9KFGV`, `01M2PBSQ64DP66D8J84SC69VMY`.
The last event was reused only for the explicitly seeded conflict case.
The transient completion-failure toast was not captured; pending Activity state,
filesystem/registry result, restart settlement and subsequent inverse were verified.
Ordinary harness behavior has retained focused adapter checks, not a new native
Cartesian sweep across every harness.

Build69.56s,maximum single-process RSS1,453,244,416bytes,zero swaps. Application
peak memory was not collected. All three managed native sessions exited. The
216,330,657-byte copied test bundle was removed. Retained concise event/results,
fixture conflict and reproduction scripts live in `/tmp/skill-studio-delivery/copy-visibility`;
the generated shared build target is reused. No production telemetry was sent.

## Upgrade compatibility correction

Independent review found that old Copy visibility events used filesystem-only
MoveBack inverses. Their restore descendants use RestoreBackup inverses. Replaying
either could move/delete content without repairing the Copy registry. The desktop
now withholds these unsafe legacy History actions and refuses backend execution,
including force. Users can use the Copy's current Enable/Disable control instead.
The guard checks both move endpoints, backup destinations and bounded restore
ancestry, including a registry still pointing to the old path. Missing/malformed
ancestry or registry evidence refuses the affected restore. Manual moves retain
normal restore behavior. New durable Copy History inverses are unchanged.

A regression creates legacy disable/enable rows and actual restore descendants
through EventStore::restore. It covers ownership at either endpoint, normal/forced
refusal without file/registry/event-claim effects, malformed registry refusal and
successful ordinary manual restore. Final affected desktop History suite:14passed,
0.12s tests,26.90s wall,RSS1,119,338,496bytes. Other passing checks remain valid for
unchanged code. Correction native acceptance passed on binary
`b23756fc0b90ef79884de11da709aa7621b7809affb67a75bfb618a508a799f0`:
legacy move-aside History remained visible without Restore; current Copy Enable
worked, retained Copy ownership and exact fixture files/registry, and left the legacy
event unclaimed. New event `01M2PCV3YCYDV28PDWXTFJA7Y5` is done. App exited and its
copied bundle was removed. Strict desktop Clippy also passes after this correction.
Independent correction review closed with no remaining findings.

## Review status

Fresh simplification made no changes; source hashes remain identical to the tested
candidate. Independent review and its compatibility correction review are complete with no
remaining findings. No production signing,
notarization, merge, deployment or final combined release is claimed.
