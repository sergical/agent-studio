# Skills.sh Fork creation

Fork an eligible Global Universal skills.sh deployment while preserving its local
files. The desktop detaches only the selected provider entry, restores the live
copy, saves the upstream merge base, and publishes Fork ownership. A durable event
supports restart recovery. Fork does not offer generic History Undo.

The implementation refuses changed ownership, conflicting data and redirected
provider paths. It targets `skills remove NAME --yes --global --agent universal`
and checks actual filesystem and lock effects instead of relying on the exit code.
The provider runs outside the write lease; an inherited, event-bound advisory lock
prevents recovery while it remains active. Its device/inode is recorded in the event
so replacing the lock file cannot bypass a running provider.

## Native acceptance — September 16, 2026

Packaged Tauri binary: `8b4aa708eeb394d6c66e436b643bb2288b0ad5889a71f39ee03571b0cd93bc13`.
Source identity: `b5aaf8d45ba5f433dd3cf825dc66a52dee05ce4104599a7f39f37d6d5c9a894f`
(sorted changed source paths, NUL, file bytes, NUL; comparison base `bad81c5`).
Native acceptance used commit `88b506e`. Linux CI subsequently required the
mechanical fixed-size slice iterator spelling `as_chunks::<2>().0.iter()` in the
hex parser; strict local Clippy passes after that change. Native results are reused
for this behavior-preserving lint correction. The final combined candidate still
requires its release acceptance.

The frontend source is unchanged from that base and uses its accepted compiled assets.
Native automation used the macOS application accessibility surface at `tauri://localhost`.
Fixture homes denied network access and access to the real user home.

| Case | Observed result |
| --- | --- |
| Provider deletes live folder but leaves lock entry | Original files, modes and links restored; provider lock, registry and sibling unchanged. Event `01M2NQB8G1GBJYQB7HDPFGDG1X` failed. |
| Successful retry without restarting | Fork ownership visible; local and sibling data preserved; event `01M2NQDH6DPJPBWVTBY3B0QR7C` done. Two total provider calls across both attempts. |
| Provider failure before effects | Files and ownership unchanged; event `01M2NQSRMEWVK4C31G98JEWS9T` failed. |
| Nonzero provider exit after exact detach | Fork completes from observed effects; event `01M2NQV1512Y459352743XSMCE` done. Local data, sibling and unknown JSON fields preserved. |
| Startup from a seeded partial live copy | Live tree matches immutable backup, staging removed, Home shows All clear. Event `01M2NQJBEQ2FKM4J8FVRNK373Q` done. Argument-level logs show only read-only gh commit queries; no removal replay. |
| Startup with changed saved lock evidence | Eleven recorded files remain unchanged; no live publication or removal replay. Home warns and View Activity opens interrupted event `01M2NQF1TPHMEBBKGFEHTCDA3M`. |

Activity shows Reveal without Undo for failed, done and interrupted Fork events.
Restart states were generated through production admission/evidence helpers and
then reopened by the native application. These are seeded restart checks, not
claims of killing the native app at every instruction boundary.

## Focused verification and review

Commands run offline with locked dependencies, two Cargo workers, one test thread,
and the shared Cargo target. No full local suite was run after each edit.

- `cargo test --manifest-path crates/skill-studio-core/Cargo.toml --features event-store --lib skill_skills_sh_fork_creation -- --test-threads=1`: 22 passed, one fixture generator ignored; 11.21 seconds.
- Same command with filter `skill_fork_creation`: 11 passed, one generator ignored; 11.73 seconds. Covers the shared baseline helper used by PR113.
- Same command with filter `atomic_marker_publication`: two passed; 0.05 seconds. Covers interrupted publication and stale temporary-name collisions.
- Desktop process filter: nine passed; 3.95 seconds.
- Core all-target and desktop lib/test Clippy with `-D warnings`, both Rust format checks, frontend typecheck and diff check passed.
- Native build: `cargo build --offline --locked --release --features tauri/custom-protocol --manifest-path apps/desktop/src-tauri/Cargo.toml`; 78.58 seconds, maximum process RSS 1,480,065,024 bytes, zero swaps. Native runtime and test process-tree peak memory were not measured.
- Frontend typecheck: 0.88 seconds, maximum RSS 339,525,632 bytes.

A fresh simplification pass and independent review were completed. Review corrections
cover attached/missing restoration, live/base stage replacement, partial-copy retry,
unrelated project-list drift, original provider-lock identity, atomic staging markers,
and stale temporary-name collisions. Final correction review reported no findings.

Real cached skills CLI 1.5.26 probes showed that default removal also deletes independent
same-name harness trees. Explicit Universal targeting limits that behavior, but can
return success without detaching if another harness retains the name. Admission
refuses known same-name deployment conflicts and completion checks actual effects.
The cached provider source SHA256 was
`ffad0abd0643fe1851d40e01cd14dfe8a6b53697e17216b479d95b15890c66bf`.

## Scope and remaining delivery

The native cases use controlled providers; the CLI contract probes are separate.
The provider lock has an exec-inheritance regression, not a native app-kill test
against every npm/skills version. CLI implementations that deliberately close inherited
file descriptors or leave independently running descendants need separate validation.
Native stale-source and every publication-instruction crash boundary are not claimed;
focused core tests supply the recorded conflict and partial-stage coverage.

Pull, Unfork and other owner/scope combinations are separate batches. Final combined
release acceptance remains open. The shared baseline-stage correction must be included
with PR113 before release. No merge or deployment is part of this evidence.

Reproduction scripts, concise result JSON and unresolved diagnostics are retained under
`/tmp/skill-studio-delivery/skills-sh-fork`. Disposable native bundles, consumed homes,
caches and superseded logs are removed after collecting the results. CI status and
integration reconciliation are tracked in the acceptance ledger.
