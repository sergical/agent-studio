# Copy frontmatter repair: review evidence

This change lets a user preview and repair malformed YAML in one owned Copy,
then undo or redo the repair from Activity. The document and Copy ownership hash
change together. Resource files, independent same-name siblings, reader links,
unrelated preferences and unknown registry fields must remain intact.

The review base is `2e42948c451a7c1ae4f661b69ecda6c0704fd0bf` (PR #94).
This evidence covers the Copy repair batch, not the full shared-core delivery.
The authoritative delivery ledger remains `docs/architecture/EXECUTION-RESET.md`
in the integration worktree. Native acceptance, simplification and final independent review passed. Both final review findings were corrected and accepted.

## Implementation boundaries

- Core preparation binds the selected deployment, content and ownership revision.
  Execution records the intent and both backups before publication.
- Dedicated Copy undo/redo operations conditionally claim their source events.
  Generic force restore is unavailable for these records.
- Desktop repair and History commands run blocking work off the UI thread and
  expose operation cancellation. Exiting requests cancellation and waits for the
  active document operation to release its state.
- Startup processes the oldest unresolved event first and stops on conflict or
  unsupported recovery. Legacy handlers share this ordering. Scope configuration
  is loaded only when a core handler needs it.
- The repair preview follows stable deployment identity, content and ownership
  fields. A snapshot refresh alone does not trigger another preview request.

## Focused automated evidence

Run from the review worktree. Use `CARGO_BUILD_JOBS=2`, one Rust test thread,
compatible target caches and one heavy job at a time.

| Check | Result | Wall time | Maximum single-process RSS |
| --- | --- | --- | --- |
| Core `skill_repair_execution`, event-store feature | 30 passed; 7 child entry points skipped directly and invoked by parent tests | 74.82 s | 917 MB |
| Desktop `skills::skill_copy_repair::tests`, including disabled Copies | 4 passed | 18.97 s | 861 MB |
| Desktop `skill_startup_recovery`, after lazy scope fix | 5 passed | 5.75 s | 952 MB |
| Desktop strict Clippy, library and tests | Passed | 8.62 s | 701 MB |
| Document operation transport | 8 passed | 268 ms | Not collected |
| Frontend typecheck and build, including dialog fixes | Passed | 4.09 s | 1,030 MB |
| Final native release build, after review fixes | Passed | 72.85 s | 1,501 MB |

Reported RSS uses decimal MB. Measured Rust/build runs reported zero swaps.
Process-tree peak memory was not measured. No performance target is inferred.

The four desktop adapter tests cover enabled and disabled global and project copies, repeated undo/redo,
cancellation before mutation and while SQLite is busy, failed journal completion,
and startup recovery. Core tests cover publication checkpoints, external drift,
changed recovery evidence, conditional history updates and process interruption.

Representative commands:

```sh
cargo test --offline --locked --manifest-path crates/skill-studio-core/Cargo.toml --features event-store --lib skill_repair_execution -- --test-threads=1
cargo test --offline --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib skills::skill_copy_repair::tests -- --test-threads=1
cargo test --offline --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib skill_startup_recovery -- --test-threads=1
cargo clippy --offline --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib --tests -- -D warnings
```

## Native evidence and its limits

CUA operated the actual Tauri app in an isolated home with real-home access and
network access denied. The fixture contained a malformed owned Copy, its Codex
reader symlink, one resource file and a healthy independent Claude Code sibling
with the same name. The registry also contained an unknown preference field.

Preview made no writes. Permission refusal preserved the document, registry,
resource and sibling. Apply → Undo → Redo passed through the native controls and
confirmation dialogs. Filesystem checks confirmed original/repaired document
bytes, matching ownership hashes and preserved unrelated data. History exposed
the next supported action and no restore action for failed attempts.

A seeded Redo interruption left the repaired document present, restored the old
registry from its backup and marked the journal event pending. Relaunch completed
the registry update, settled the event and exposed Undo. This was a seeded state,
not a native process-kill test. Core tests exercise actual process interruption.

Those native checks preceded the final startup ordering correction. The current
release binary is SHA-256
`c9f1c199fc60a541e292a458bbddcb87f5fe0506e9797406f0b7ad6f03032920`.
On this binary, native checks confirmed the visible permission error and dialog
layout, cancellation while SQLite was busy, and successful Apply after cancellation.
No intent was recorded for the cancelled operation; all baseline files and registry
entries remained unchanged. One earlier busy attempt timed out before the Stop
click and is recorded as refusal evidence, not cancellation evidence.

A second seeded interruption replaced the document externally. Restart preserved
the replacement and registry and showed the event as interrupted without a restore
action on that record. After the fixture's expected document was restored, restart
completed the same event and exposed Undo. Resource, sibling and reader remained
unchanged. These checks exercised the corrected startup dispatcher.

The fresh simplification pass centralized History's completion-state reset in
`finally`. Typecheck and scoped lint passed afterward. Frontend build passed in
3.39 s (RSS 1,040 MB), and native build passed in 55.29 s (RSS 1,456 MB), both
with zero swaps. Final native binary SHA-256:
`47c3ed67c0ec5c90e1d3720f0831a54b8afb00fa761a56f063f672144eb425fb`.
Undo → Redo passed again on this binary: document and registry matched their
expected baselines, History settled, busy controls cleared and the next action
became available. Unchanged core/recovery evidence is reused. Final independent
review found two issues: disabled Copies were offered repair but refused by core guards, and legacy History actions exposed an unsupported Stop control. Both were fixed; follow-up review found no remaining issues.

Final review-fix binary SHA-256:
`dea9eadcea3362b37c2b58fd255614a5f113b02b28923d3bf8c29db6cb4279c1`.
Native disabled Copy Apply → Undo → Redo passed on this binary. The UI retained
Copy ownership and the disabled state, cleared the repair warning, and settled
History with the next action. File checks verified the matching folder hash,
exact original/repaired document and registry states, and unchanged resource data.
Additional focused checks: three core transition tests and default-feature core
check passed. Frontend typecheck and scoped History lint passed after review fixes.

The native process exited. Original fixture launcher and binary were restored and
hash-verified. Disposable home, binary backups and native log were removed.
Passing intermediate build/test logs were summarized and removed. Native runtime
latency and process-tree peak memory were not measured.
