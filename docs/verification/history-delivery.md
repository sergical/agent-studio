# Bounded desktop History delivery

Base `4ba958c` (PR136). Native acceptance, simplification and independent review passed. Linux CI remains pending at publication.

Outcome: History summaries load on a blocking worker with two admitted reads.
Identical frontend requests coalesce; snapshot revisions invalidate in-flight reads.
The view ignores stale settlements, shows20rows per page within its existing
200-event window, and preserves existing restoration controls and refusal rules.
Recovery reads retain their separate off-thread path and shared database mutex.

Scope corrections found during root review: fixed the extracted usize limit;
invalidated reader requests on revision; replaced duplicated test-only DTO logic
with a conversion into production DTO projection; refuse legacy reversal when
required detail exceeds its bound; persist invalid intent into the test database
so assertions cover the new detail-read path. No generic force authorization added.

Focused checks use external CARGO_TARGET_DIR=/tmp/skill-studio-delivery/cargo-target,
CARGO_INCREMENTAL=0, CARGO_PROFILE_DEV_DEBUG=0, CARGO_PROFILE_TEST_DEBUG=0,
CARGO_BUILD_JOBS=2, CARGO_NET_OFFLINE=true; Cargo --locked --offline -j2,
tests --test-threads=1. One heavy workload at a time.

- Core cargo test --features event-store --lib skill_history::tests:4pass,
  0.01s tests/13.50s wall, maximum single-process RSS1,180,614,656bytes,zero swaps.
  Covers query limits, summary order/filter/projection, oversized detail refusal
  and rejection of a view masquerading as the event table.
- Desktop cargo test --lib event_commands::tests:16pass,0.13s tests/10.35s wall,
  RSS969,752,576bytes,zero swaps. Includes actual worker admission/excess rejection/
  permit release, Copy/legacy reversal and oversized-detail policy.
- Frontend reader/API/History-policy tests:6pass across3files,403ms. After changing
  API test transport to the existing Tauri mockIPC facility, the affected API test
  passed again170ms. Runtime peak memory was not collected for frontend tests.
- Desktop typecheck and scoped frontend lint/format passed.
- Strict all-target Clippy: desktop5.89s/RSS771,964,928bytes; core(event-store)
  7.50s/RSS890,355,712bytes; both passed with zero swaps.

A worker accidentally created a duplicate950MB local Cargo target during its early
check. No process was using it; it was removed. Final checks use the shared external
cache. Synthetic native fixtures live outside the repo, with network/real-home
access denied and telemetry export disabled. No production monitoring claim.

## Native acceptance

Final native build17.28s wall,maximum single-process RSS1,028,390,912bytes,zero swaps.
The first build exposed missing Global/project labels; the corrected build passed
scoped lint and its frontend typecheck/build. Paging on the final binary showed
20/20/5rows across45events, newest-first, correct disabled boundary buttons and
Global/project labels. Home/Activity navigation returned to newest entries.
Inserting fixture-refresh then native Sync showed the new event first without
leaving Activity. After restarting with a malformed pending Copy removal event,
Home displayed its review warning and View Activity showed the unresolved event
without an inverse control. SQLite read-back retained all47events, the unresolved
payload and no inverse. This seeded malformed event checks visibility/preservation,
not real interrupted removal (covered in its earlier delivery).

All managed native sessions exited. Fixture results remain temporarily for review.
Native app runtime peak memory was not measured. Binary identity: `a736455c474a7ebbbdc27c177c9257d1bbf5e998c8804d01ea04e5ee6da36866`.

## Post-green review

Simplification replaced two optional-expression chains in desktop DTO projection
with direct conditionals. Affected event_commands tests passed again:16tests,
0.15s tests/24.81s wall,RSS988,708,864bytes,zero swaps. Other production source
was unchanged from native acceptance at this point.


Independent review found that summaries still selected an entire explode payload.
The correction removes that unused projection and size-gates Copy visibility JSON
parsing. Required restoration detail remains separately bounded. A regression
covers oversized explode and Copy events. The reviewer confirmed the finding is
closed with no remaining concerns in the correction.

After that correction, core History tests passed (5 tests, 12.82s wall,
RSS 1,211,318,272 bytes); desktop event-command tests passed (16 tests, 16.62s wall,
RSS 1,013,137,408 bytes). Final strict all-target Clippy passed for desktop
(14.73s, RSS 772,341,760 bytes) and core with event-store (7.50s,
RSS 891,666,432 bytes). All four commands reported zero swaps.
The native binary predates the two expression simplifications and query correction;
unchanged UI acceptance is reused, while focused tests cover the corrected query.
No claim is made that this binary is the final combined release candidate.
