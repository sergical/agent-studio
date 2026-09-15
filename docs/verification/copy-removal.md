# Atomic Copy removal

Review base: PR93, `5770c0986e071c500ef3edefeb4a6a3b67dd09fd`.
Source: integration worktree `codex/shared-core-design`.
Status: implementation, focused checks, native acceptance and independent review
complete for this removal batch. CI and final combined release acceptance remain.

## User outcome and scope

Remove exactly one independent Copy and its verified reader links. Preserve
independent same-name skills and unrelated ownership records. Record a durable
operation before effects; move the tree into quarantine rather than recursively
deleting its contents. Removal has no Undo. Quarantine supports failure recovery.

The desktop calls the shared core on a blocking worker. Existing desktop history
uses the shared event store, and document edits and History restores use the same
SKILL.md transaction mutex. Startup retries interrupted removals in journal order
using current configured roots. A conflict preserves the data and interrupted
History entry. Trial expiry starts after recovery; inventory refresh follows.
Startup recovery remains synchronous and can delay launch when work is pending.

Dependencies include bounded backup/tree-move support, registry comparison and
publication, event records/store/binding, and guarded journal statements. CLI,
MCP, cloud adapters and later lifecycle features are excluded. The dialog and
error helper are the exact reviewed PR78 files from `7c1b952`.

## Acceptance evidence

| Case | Evidence |
| --- | --- |
| Selected Copy and readers removed; sibling preserved | Core cases across global/project, universal/independent and disabled copies; native success |
| Stale owner, revision, content or unsafe holding directory | Core refusal tests |
| Plugin ownership appears after admission or in a lock gap | New checkpoint regression; no forward removal |
| Read-only parent | Core regression and native refusal; original files, registry and links preserved |
| Quarantine changes or source is replaced before publication | New checkpoint regression, including a byte-identical replacement; ownership not removed |
| Interrupted removal before/after quarantine, readers and publication | Core checkpoint tests; seeded native pre-publication restart rollback |
| Conflicting replacement during recovery | Core cases and native restart; replacement and original quarantine preserved, History interrupted |
| Successfully restored legacy event | SQL regression covers missing, pending, failed and completed restore; only completed restoration settles the original |
| History compatibility and no Undo | All eight original store tests retained among 17 compatibility tests; native History has no Undo for removal |
| Shared editor/restore lock | Adapter lock-identity regression and existing invocation transaction test; native direct Copy write |
| Useful refusal feedback | Final native dialog shows permission reason and rolled-back event ID |

Native checks used CUA against the packaged Tauri application at `tauri://localhost`,
not a browser approximation. The isolated disposable home contained a universal
Copy, its Codex link, a same-name independent Claude sibling, and a direct Cursor
Copy for the write smoke check. Sandbox denied network and real-home reads.
Restart tests seed actual journal/quarantine state; they are not OS kill injection.

## Checks and review

Use `CARGO_BUILD_JOBS=2`, one test thread, and the compatible shared target cache.
Core commands use `--manifest-path crates/skill-studio-core/Cargo.toml
--offline --locked --features event-store --lib`; desktop commands use
`--manifest-path apps/desktop/src-tauri/Cargo.toml --offline --locked --lib`.

- Core removal: 16 passing cases and one explicit macOS disk-image mount skip in
  the last module run. One new replacement fixture initially stopped at the earlier
  ownership guard; after making its contents byte-identical, its focused rerun
  passed and exercised the final publication check. No production change followed.
- Event-store compatibility: 17 passed; added restoration-gate regression passed.
- Desktop command tests: 30 passed after lock unification; two document-transaction
  tests and two startup adapter tests passed.
- Core Clippy: `--features event-store --all-targets -- -D warnings` passed.
- Desktop Clippy: `--lib --tests -- -D warnings` passed.
- `npm run build` passed; `vitest run apps/desktop/src/lib/error-message.test.ts
  --maxWorkers=1` passed all six cases. Node heap capped at 3072 MiB.
- Fresh simplification retained no edits: its sandbox could not use the shared
  Cargo target, so it reverted its candidate. Root verified all 29 pre-pass hashes.
- Independent review found three defects, fixed with the regressions above. Fresh
  follow-up reviewed those corrections and the shared document-write adapter,
  found no actionable defects, and checked six restoration states in SQLite.

## Resources and cleanup

| Check | Wall time | Maximum single-process RSS |
| --- | --- | --- |
| Corrected core Clippy | 3.53 s | 597,098,496 bytes |
| Corrected desktop command tests | 12.87 s | 967,442,432 bytes |
| Corrected desktop Clippy | 9.50 s | 785,760,256 bytes |
| Final native release build | 68.41 s | 1,378,009,088 bytes |

These measured checks reported zero swaps. Process-tree peak and native paint
latency were not measured. Final build preflight: 40% free memory, 58 GiB free disk.
Only one heavy workload ran at a time. Test app and build processes exited.
Original test launcher/binary restored and hash-verified; disposable homes, binary
backups and passing raw logs removed after recording conclusions. Concise review
reports and fixture preparation script remain under `/tmp/skill-studio-delivery`.
CI artifact retention is three days. No merge or deployment occurred.

## Remaining product work outside this removal batch

Native write smoke confirmed two existing lifecycle gaps to resolve in the later
Copy mutation batch: the shared-folder invocation UI can select Fork based on an
aggregate source kind, and direct Copy invocation writes do not update the stored
Copy hash, so discovery marks the changed Copy Unknown. The shared transaction
write succeeded; these complete mutation workflows are not claimed as accepted.
Final combined lifecycle, performance-report, monitoring and release acceptance
remain governed by `docs/architecture/EXECUTION-RESET.md` in the integration tree.
