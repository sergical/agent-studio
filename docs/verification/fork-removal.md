# Fork removal and startup recovery

Review base: PR104, `5120b9e`. Branch: `codex/fork-removal-delivery`.
Status: implementation, focused checks, fresh simplification, independent review
and final native acceptance complete. Review's ownership race was corrected and
follow-up review found no new issue. Commit/PR publication follows this packet.

Removing a global Universal Fork now records a durable, non-restorable event,
moves the exact tree to retained quarantine, then removes its exact Fork/trial
ownership. Unknown registry fields and other deployments stay unchanged. Both
SkillsSh and Dotagents records, including legacy IDs/paths, are supported.
Interrupted removal restores only to an absent original path before ownership
publication. After publication, recovery settles the event without touching a
replacement at that path. Conflicts preserve both trees and remain unresolved.
Removal has no Undo. Mutable merge bases and immutable History backups remain.
A later Fork clears the old merge base before fetching.

## Verification

Commands use two Cargo workers, one test thread and the shared compatible target.

- Core: `cargo test --offline --locked --manifest-path crates/skill-studio-core/Cargo.toml --features event-store skill_fork_removal -- --test-threads=1`:
  16 passed, one native fixture generator ignored; 3.61s compile, 5.30s execution.
  Covers both origins/current+legacy, durable reopen boundaries, replacements,
  trial addition/change/removal, registry drift, persisted event/order checks,
  ownership disappearance before moving and publication racing with rollback,
  current roots, quarantine overlap, unknown-field preservation and stale owners.
- Desktop: `cargo test --offline --locked --manifest-path apps/desktop/src-tauri/Cargo.toml skill_startup_recovery -- --test-threads=1`:
  five passed; 38.73s compilation, 0.02s execution. These retain existing ordered
  dispatcher cases; the new branch is exercised by native restart below.
- Strict all-target Clippy passes for desktop (19.63s) and core with event-store
  (3.23s after the final guard correction). An obsolete import was removed before the desktop Clippy pass.
- Wider core suite before the final event/trial guard edits: repair-agent output
  reports 567 passed, 15 ignored. Affected cases were rerun after the edits.
  This is not final combined-release acceptance.

Native binary SHA256:
`ec3952d1404a6ce3dea51d2eb1430d606d22fae11ffa5c85c1f4718dc33a7cd9`.
Built with `cargo build --offline --locked --release --features tauri/custom-protocol
--manifest-path apps/desktop/src-tauri/Cargo.toml`. Native build: 76.89s, maximum
single-process RSS 1,520,304,128 bytes, zero reported swaps. Unchanged PR104
frontend assets were reused after checking frontend source/dependency identity.
An initial frontend build attempt stopped because this checkout lacked
node_modules; it produced no accepted assets.

Native CUA acceptance used a disposable HOME, no network, denied real-home reads,
and fixture-only writes. Observed:

1. Global removal confirmation names one deployment and warns it cannot be undone.
   Removal leaves only the same-name Project location and its original body.
   Activity shows `done`, without Undo. Disk assertions confirm unknown registry
   data, immutable backup, mutable merge base and retained Fork quarantine.
2. Overlapping quarantine/ownership roots refuse removal with visible feedback,
   before effects. The fixture was corrected to a separate plugin root; application
   source did not change.
3. Actual TreeMoved checkpoint: native startup restores original SKILL.md/resources,
   preserves the registry and three trial rows, and Activity marks removal failed.
4. TreeMoved with a replacement: native startup preserves both trees; Activity
   shows Interrupted. No overwrite or false successful recovery was observed, including a second restart on the final candidate.
5. RegistryPublished with a replacement: native startup marks removal done,
   preserves replacement/quarantine and leaves only the unrelated trial row.

Checkpoints come from `generate_native_restart_fixture`, an explicitly ignored
core test using the production admission/event/move/publication path. Supply
`FORK_REMOVAL_FIXTURE_PARENT` (task-owned directory) and
`FORK_REMOVAL_CHECKPOINT=admitted|holding|tree|conflict|published`; run the test
with `--ignored --nocapture --test-threads=1`. It retains a unique directory and
checkpoint.json. Keep its absolute paths unchanged and launch the desktop with
that fixture HOME. This proves a separate native process can reopen the durable
state; it does not claim SIGKILL injection at arbitrary instruction boundaries.

## Remaining limitations and cleanup

Home still says All clear during unresolved recovery; Activity carries the
Interrupted status. Home recovery notification is an explicit remaining desktop
acceptance item, not closed by this batch. Retained quarantine has no automatic
expiration or user-facing restore action in this batch.

Final preflight: 47% memory free, 106GiB disk free, 11521/276480 handles.
Test peak RSS was not collected. The native app exited after acceptance.
Disposable bundle, successful fixtures, raw build/application logs and generated
cache inputs were removed. Keep the compact repro script, checkpoint test and
startup diagnostics under `/tmp/skill-studio-delivery/fork-removal`; the small
unresolved replacement fixture is retained for the Home notification follow-up.
The initial candidate was rebuilt after the review correction; final native
checks cover selected removal, rollback, published settlement and repeated
conflict refusal on the binary identified above. No production telemetry was
exported. No merge or deployment occurred.
