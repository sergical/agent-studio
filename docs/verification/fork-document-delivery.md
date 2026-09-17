# Fork document Repair and Restore

Global Universal Dotagents **Fork and fix** now uses the shared Rust core in the
default desktop build. It retains snapshots, ownership evidence and durable History
before publication. Other owner and scope routes preserve their existing behavior.
Activity Restore reverses document bytes; restoring that event reapplies them.
These actions keep Fork ownership and do not reattach provider management.

## Acceptance

- Exact deployment, owner, document and provider proposal checks precede writes.
  Cancellation before an event removes only its verified backup reservation.
  Uncertain publication and replaced directories retain recovery evidence.
- Snapshots preserve resources and valid/dangling symbolic links. Sibling skills
  and unrelated registry fields remain unchanged.
- Restore supports changed-content refusal, forced preservation, Redo and interrupted
  startup completion. It changes the document, not Fork ownership or provider state.
- Restore after Pull admits only a valid upstream commit change in the same Fork
  identity. It records the exact current row for pending execution and recovery.
  Legacy intents remain strict. Restore and Pull share commit-ID validation.
- Missing merge bases can be reconstructed only from an unambiguous matching Fork
  event. Historical snapshots cannot replace a newer upstream base.
- Clean/conflicting Pull, a second Pull, failed Unfork, successful provider
  publication and startup settlement have focused core/desktop coverage.

## Verification

Focused checks passed: core snapshot1, upstream9, Pull12, registry admission2,
backup manifest7, the parameterized Fork preparation scenario1; desktop lifecycle2,
History eligibility1 and Unfork5. Six optional Unfork fixtures and the Pull fixture
generator were explicitly skipped. The required real-provider Unfork/publication/
startup test was run explicitly despite its ignore flag and passed.

After final simplification, backup manifest7, preparation1 and default desktop
lifecycle2 passed again. Strict core lib/tests and desktop all-target Clippy,
Rustfmt, frontend typecheck and scoped History lint/format passed. The final small
review correction reuses the canonical commit validator and tests uppercase
rejection. No full local suite was repeated; PR CI provides integration checks.
Fresh simplification and independent review covered the batch and its corrections.

The focused Rust commands use this environment:

```sh
export CARGO_TARGET_DIR=/tmp/skill-studio-delivery/cargo-target
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true
cargo test --locked --offline -j2 --manifest-path crates/skill-studio-core/Cargo.toml --features event-store --lib registry_admission_ -- --test-threads=1
cargo test --locked --offline -j2 --manifest-path crates/skill-studio-core/Cargo.toml --features event-store --lib skill_backup_manifest::tests -- --test-threads=1
cargo test --locked --offline -j2 --manifest-path crates/skill-studio-core/Cargo.toml --features event-store --lib skill_fork_preparation::tests::publishes_prepared_sources_and_refuses_unplanned_drift_overlap_and_budget -- --test-threads=1
cargo test --locked --offline -j2 --manifest-path apps/desktop/src-tauri/Cargo.toml --lib desktop_native_fork_ -- --test-threads=1
cargo clippy --locked --offline -j2 --manifest-path crates/skill-studio-core/Cargo.toml --features event-store --lib --tests -- -D warnings
cargo clippy --locked --offline -j2 --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
```

The real-provider test uses the admitted Node24.19.0/Dotagents3.0.1 runtime through
`SKILL_STUDIO_RUNTIME_FIXTURE`. Run the desktop test filter
`skills::skill_fork_document_history::tests::desktop_real_provider_unfork_publication_and_startup`
with `--ignored --exact --test-threads=1`. A missing runtime is not a pass.

## Native acceptance and limits

The Tauri app ran with an isolated home and denied network access:

1. Fork-and-fix repaired the document and showed Fork ownership.
2. Activity Restore returned original bytes; Restore again returned repaired bytes.
3. Resources, links, sibling content and ownership stayed intact. History labels and
   confirmation explained that Restore keeps Fork ownership.
4. Pull reconstructed the merge base, advanced the commit and completed its event.
5. On the corrected build, post-Pull Restore and Redo both completed. Redo returned
   exact prior bytes; provider files and the advanced registry stayed byte-identical.
6. Offline Unfork failed visibly in History while retaining Fork content/ownership.

The post-Pull native binary SHA256 was
`abc6e504b9654137d84bd81406f46fdf578a8ab815c9426a52e2bc950997bbc5`.
Subsequent changes removed three redundant explicit drops and shared the existing
lowercase commit validator; affected automated checks cover those edits. Changed-
content force and post-Pull restart recovery are automated desktop evidence, not
claims from the UI sequence. Successful network reinstallation is not claimed by
the offline UI check; the real-provider fixture verifies publication and recovery
using local source input. Injected failures do not prove arbitrary process-kill
atomicity. Combined application acceptance and signing/notarization remain open.

## Resources and cleanup

One heavy workload ran at a time with two Cargo workers and one test thread.
After simplification: manifest15.56s/maxRSS1,447,198,720bytes;
preparation38.83s/55,132,160bytes; desktop45.00s/1,059,454,976bytes.
The real-provider check took24.78s/138,870,784bytes. Native build took31.95s/
1,071,906,816bytes. All reported zero swaps. These are maximum single-process/build
measurements; application peak memory was not measured.

All test, build and native app sessions exited. Concise results, source identities
and reproduction scripts are retained under `/tmp/skill-studio-delivery/fork-document`.
Consumed fixture cleanup is recorded there after review acceptance. CI artifacts
have finite retention. CLI, MCP and cloud-agent delivery are excluded.

## Combined integration acceptance — September 17

The selective port into `codex/shared-core-design` also passed native Fork-and-fix,
Pull, document Restore and Redo. Restore returned exact original bytes; Redo
returned exact post-Pull bytes. The advanced Fork registry, provider documents,
resources, valid/dangling links and sibling skill were preserved. Four History
rows completed, with Restore/Redo links. Native binary SHA256:
`56652bb9fe822f4903d1531018c29115f6df1c7a6295c880af01e62ae78fb104`.
The isolated offline app exited successfully and its consumed fixture was removed.

Post-green simplification removes one redundant JSON serialization in registry
admission. This core file is identical in the review and integration checkouts.
After that edit, admission2, parameterized preparation1 and strict core Clippy
passed (16.36s,38.47s,9.95s; largest single-process RSS1,533,657,088bytes).
The preceding native binary is identified separately; no UI behavior changed.
Application peak memory was not measured. Detailed source identities and concise
results remain under `/tmp/skill-studio-delivery/fork-document`.

The integration workspace has a separate optional no-event-store build failure in
skills.sh staged-source dependencies. This does not establish failure of this
review branch, whose optional configuration passed CI; final combined acceptance
must resolve that gap. Production monitoring and release acceptance remain open.
