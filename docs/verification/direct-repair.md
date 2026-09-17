# Direct Repair and Restore acceptance

Review branch: `codex/direct-repair-delivery`, based on History PR141 (`48f9f68`).
Implementation, focused checks, native acceptance, simplification and independent
review are complete. CI and combined integration acceptance remain separate.

## User outcomes

Direct YAML Repair uses a shared-core preview and a private desktop event worker.
Activity Restore keeps exact backups and linked reversal events. Startup settles
matching persisted operations and retains conflicts with a Home warning.

Copy retains its existing in-process route. Fork-and-fix retains one legacy repair
transaction, with core ownership approval held through staging and revalidated
before ownership changes. Its live recovery copy preserves literal symbolic links.
Restoring a Fork-and-fix event restores the document; it does not Unfork or reattach
the provider. Public CLI/MCP hosts are excluded.

## Native evidence

Native CUA drove isolated copied debug apps with empty telemetry DSNs, denied
network access and denied real-home access. No installed app was replaced.

Direct Repair candidate:
`8ac823b3ebe5b1f2839531bd0512be15f5fcfabdb795d9e5a8d8b126d0445170`.

- Global skills.sh Repair and Activity Restore passed. Exact document bytes,
  unchanged provider locks and linked completed events were checked.
- Manual Cursor Repair and Restore returned exact original document bytes.
- Cancel preserved all three fixture documents and created no event.
- Repository-owned and managed-project previews offered no direct Apply/Fork.
- External edits refreshed the native preview. The new preview and applied repair
  preserved them; this does not prove submission of a stale GUI request.
- Restore detected edits made after Repair and made no change before force.
  Force retained those edits; restoring the force event recovered their exact bytes.
- Actual app startup settled a saved Restore marked interrupted with a cleared
  post-fingerprint. With conflicting bytes, it retained the interruption and file,
  showed a Home warning, and linked to the interrupted Activity entry.

These restart cases seed persisted state; they are not whole-app process-kill
experiments. Separate native SQLite tests cover child exit without SQLite close
and durable receipt recovery.

Final Fork-and-fix candidate:
`1b0e9a127d953822a5b707ae01c1ca70877dc07ddca921c5fdff1ff3bdf03669`.

A local archive and fake provider exercised actual desktop staging, provider
removal and restoration. YAML was fixed; only the selected provider row detached;
Fork ownership, local resource bytes and valid/dangling links were preserved;
the sibling was unchanged. One completed repair event appeared in Activity.
Activity Restore returned the original document, retained the links and Fork
ownership, and linked its completed event. Intentional dangling fixture links
produce discovery warnings.

The first run exposed an existing skip-links recovery copy. The final candidate
uses the existing non-following link-preserving helper for both snapshot and
restoration; the complete native case above was rerun after that correction.
The direct UI route is unchanged by this last helper correction. The corrected
packaged worker independently passed the Repair/Restore bridge before it.

## Focused checks and reproduction

Set `CARGO_TARGET_DIR=/tmp/skill-studio-delivery/cargo-target`,
`CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`,
`CARGO_BUILD_JOBS=2`, `CARGO_NET_OFFLINE=true`. Cargo commands use
`--locked --offline -j2`; tests use `-- --test-threads=1`.
Core commands enable `--features event-store`.

| Command target/filter | Result |
| --- | --- |
| Core lib `skill_event_worker` | 26 passed, 1 explicit child-fixture skip |
| Core lib `skill_event_native::tests` | 21 passed, 1 explicit child-fixture skip |
| Core tests history_worker_process, history_worker_frames, history_worker_bootstrap | 11 passed |
| Core lib `skill_event_file_authority` | 3 passed, 1 explicit child-fixture skip |
| Desktop lib `skill_frontmatter_repair::tests` after review corrections | 13 passed, 2 explicit fixture skips |
| Desktop lib `guarded_fork` | 2 passed |
| Core lib `repair_and_restore_preflight_the_exact_history_record_boundary` | 1 passed |
| Desktop lib `skill_fork::tests` after link correction | 30 passed |
| Desktop lib `skill_startup_recovery::tests` | 7 passed |
| Desktop lib `skill_copy_repair` | 4 passed |
| Strict core and desktop all-target Clippy | passed |
| Desktop `cargo check --no-default-features` | passed |
| Rustfmt and `git diff --check` | passed |

The ignored `desktop_release_worker_bridge` test was explicitly run with
`SKILL_STUDIO_RELEASE_WORKER` pointing to the copied corrected executable:
1 passed, 4.16 s test / 28.39 s wall. Its name does not imply a release build;
this was a debug executable. It covers stale selection, cancellation, Repair,
Restore, force/reversal and seeded recovery using the real private worker entry.

Review corrections preflight exact serialized Repair/Restore records before any
backup, including one shared timestamp for Repair. Boundary regressions assert
no worker/event/backup on oversized requests. The 1 MiB preview input limit
remains; Apply can refuse a record exceeding the smaller recovery-read budget.
Platform cfg guards match their dependencies. Only macOS was compiled locally.

## Resources, review and cleanup

One heavy workload at a time; two Cargo workers and one test thread. Final Fork
suite: 11.04 s wall, 1,024,507,904-byte maximum single-process RSS. Final desktop
Clippy: 15.25 s / 780,189,696 bytes. Final native build: 31.97 s /
1,045,102,592 bytes. Corrected core Clippy: 9.07 s / 950,124,544 bytes. All reported
zero swaps. Native app peak memory and interaction timing were not collected.
These build measurements are not production performance claims.

Fresh simplification completed; independent review found ownership handoff and
oversized-record gaps, which were corrected and independently re-reviewed with
no remaining findings. The narrow native link correction also passed review.
All native/build/test process sessions have confirmed exit.

Local concise results, source identities and reproduction inputs are retained under
`/tmp/skill-studio-delivery/direct-repair`. They include `native/results.json`,
`native-fork-links/acceptance.json`, seed/package scripts and focused command
results. Eight superseded failure logs were removed after recording their causes.
Consumed test app bundles and fixture cleanup are recorded in the delivery ledger.
Final combined application acceptance, remaining Fork core migration and production
monitoring are not established by this packet.

## Linux CI correction

The first Linux CI run (35179969368) reported 615 passed, 168 failed and
23 ignored. The first failure was a child exiting before its completion receipt;
subsequent coordination tests failed against the poisoned process gate. The child
tried to sync a cloned capability directory descriptor, which is O_PATH on Linux.
The production SQLite directory callback had the same issue. Both now open "."
relative to the retained capability to obtain a syncable descriptor. The native
descriptor probe asserts that directory fsync succeeds.

After this correction, local core `cargo test --locked --offline -j2 --features
event-store --lib skill_event_ -- --test-threads=1` passed 74 tests with four
explicit child-fixture skips in 3.26 seconds. Strict all-target core Clippy passed
in 9.08 seconds; formatting and diff checks passed. These checks ran on macOS;
Linux CI35180649997 passed at `791c1b0`: 783 unit tests and 11 integration
tests, with 23 explicit fixture skips. Formatting and strict Clippy passed.
CI artifacts retain three days. Peak memory was not collected for the local
Linux-correction checks. No new native app was needed for that directory fix.

## Retained registry correction after integration

Combining the desktop repair selection with the newer registry writer exposed a
nested lock acquisition: Fork-and-fix refused before provider detach. Registry
publication now uses the existing selection lease. A visible publication failure
returns its rollback state instead of appearing to be a pre-publication failure.
The desktop either restores the selected raw registry records or retains the
upstream snapshot and reports recovery required. Unknown JSON fields survive.
An originally absent registry is removed only with a verified publication receipt.
A real failed creation without that receipt refuses rollback and retains evidence.

Integration validation: one core publication fault-injection test, one desktop
parameterized retained-selection test, 14 existing document-target tests, strict
desktop all-target Clippy, formatting, fresh simplification (no changes), and
independent review (no actionable findings). The desktop absent-registry injected
error occurs after successful creation; the actual creation error is covered by
the lower-level target test, not by that desktop parameter.

Final native integration binary:
`935153f388aa21ed9745c3575025d2938628dcb99c5eb366b0e752e81d17e80e`.
CUA Fork-and-fix and Activity Restore passed with file and read-only SQLite checks:
original document bytes and links restored, resources and sibling preserved,
only selected provider detached, Fork ownership retained, two linked done events.
This is integration-candidate evidence; the selectively ported PR candidate has
its own focused checks recorded below.

Integration check wall times: 22.77 s core fault test, 22.78 s desktop scenario,
16.05 s document targets, 16.03 s Clippy. Largest measured single-process RSS:
1,528,217,600 bytes. Native build: 38.95 s and 1,097,187,328-byte single-process RSS.
All measured commands reported zero swaps; application peak memory not measured.
All native sessions exited. Three consumed native app bundles, homes and temporary
directories were deleted after preserving concise results and reproduction inputs.

PR candidate verification after the selective port: desktop `--lib
skills::skill_fork::tests` with `--features worker-repair` passed 31 tests
(11.07 s wall, 1,024,737,280-byte maximum single-process RSS). Strict desktop
all-target Clippy passed (15.22 s, 781,598,720 bytes). Both reported zero swaps.
The initial test compilation found a missing test-local BTreeSet import; adding
that import resolved it. No production behavior changed during this port.
