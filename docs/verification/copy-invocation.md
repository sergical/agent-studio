# Copy invocation verification

Base: PR99 (`6bc3af3`). Status: focused checks, native acceptance, simplification,
and independent review with correction follow-up passed.
The procedure below defines the native cases. Results and limitations follow it.

The selected Copy must retain its ownership after changing invocation policy.
The change includes `SKILL.md`, the selected Codex sidecar when applicable, and
the Copy registry hash. Other deployments and unrelated resource files stay intact.

## Native acceptance procedure

Use the actual isolated Tauri app with disposable HOME and an outside-HOME project.
Deny network and real-home reads. Build once after focused checks and review fixes.
Record the tested source and binary identities; do not substitute browser rendering.

1. Prepare a global universal Copy and a same-name managed sibling. Change the
   Copy through Both → User only → Both. It must not prompt for Fork. After each
   refresh it must remain Copy with a matching folder hash; preserve the sibling.
   A universal Copy with a Codex reader link must not gain a Codex sidecar merely
   because that reader exists. The selected concrete Codex deployment controls
   sidecar writes; a same-name Codex sibling does not.
2. Add the outside-HOME project through the native picker. Repeat on its Copy;
   retain project registration and ownership after quit/relaunch.
3. On a Codex Copy, select User only. Verify `agents/openai.yaml` has the intended
   policy and preserves unrelated YAML fields. Returning to Both clears only the
   invocation override. Cover absent sidecar creation and removal in core tests.
4. Undo and Redo supported invocation events. Check exact document, sidecar
   presence/content, registry, disabled flag, resources, and sibling state.
5. Repeat a supported change on a disabled Copy. It must remain disabled.
6. Exercise a write refusal and an external-change conflict. Show a useful error;
   do not leave a completed event or a mismatched Copy registry hash.
7. Restart with an interrupted transaction and verify recovery or explicit
   conflict refusal. Label seeded states separately from real process-kill tests.
8. Confirm plugin and unknown/ambiguous targets remain unavailable for editing.

## Focused automated coverage

Core: sidecar create/update/remove, complete folder hashes, exact ownership,
resource/document/registry drift, interruption at each publication boundary,
rollback conflicts, old persisted document events without a sidecar, and reversal.
Desktop: exact target selection, durable outside-HOME scope, startup dispatch.
Frontend: aggregate managed source must not block or Fork an independently owned
Copy; confirmed manager and plugin restrictions still apply to their own targets.

## Results and resources

Source identity after scoped formatting and lint corrections: `1050b4738f117489f78b22be1ef1eb50c3409679bc41b12c264dcfb4c1470478`
(sorted changed/untracked Rust/TS/TSX relative path, NUL, bytes, NUL).

- Copy document module: 32 passed, one ignored, 102.76 s tests. This preceded the
  final receipt check and expanded History test. Later focused checks cover those
  edits; no full module rerun is claimed.
- Final receipt recovery regression: passed, 1.61 s (worker result).
- Expanded History regression: one test covering ten global/project combinations
  passed, 6.27 s compile and 13.88 s tests. Covers sidecar create/update/remove,
  unchanged absent sidecar and sidecar-only changes; exact Undo/Redo, fresh Copy
  ownership, registry values, resources and sibling preservation.
- Desktop transport regression: disabled global/project Copy invocation, repeat
  no-op, Undo/Redo passed, 19.20 s compile and 3.13 s tests.
- Strict core Clippy all targets/event-store passed, 6.05 s. Strict desktop
  Clippy lib/tests passed, 10.66 s. Two test-only owned path comparisons were
  corrected after the first core lint run.
- Earlier unchanged UI evidence: typecheck, scoped lint and 45 routing tests pass.

Core and desktop tests run with Cargo jobs2/test thread1 using compatible caches.
Preflight had 54% memory free and 62 GiB disk available. Test peak memory was not
measured. Listed test/lint sessions have exited. The disposable native fixture
exists but no new app build/launch or native acceptance occurred in this phase.

Use one heavy process, Cargo jobs2/test thread1, bounded frontend workers,
and compatible caches. Store disposable output in
`/tmp/skill-studio-delivery/copy-invocation`; retain concise revision-linked results
here and delete passing raw output after completion. Native latency and memory
are not measured unless explicitly recorded. This batch adds no performance budget.

## Native checkpoint — September 16

Initial native binary `60e1de807909acf4a2dbf40adab6768bde6f28b89f7cf157b93ba8ff6199c079`:
- Global Codex Copy: User only creates sidecar, retains Copy ownership and matching
  registry hash. Undo restores original document and removes sidecar. Quit/relaunch
  then Redo succeeds; History shows all three events done.
- Shared Copy beside an independent Claude sibling: User only → Both succeeds
  without Fork, leaves sibling unchanged, and creates no shared Codex sidecar.
- Outside-HOME project added through native picker: Copy stays owned through
  Both → User only → Model only → Both. Unrelated sidecar display_name survives.
- Read-only project folder: write is refused and document stays unchanged, but
  no persistent error is visible. A diagnostic repeat confirmed this UI gap.

The refusal finding prompted a focused UI correction: display the preserved Tauri
error string inline beside invocation controls and clear it on retry. Typecheck
and scoped lint pass; formatting was corrected. Refusal acceptance on the rebuilt
app is pending. The initial success/History evidence remains scoped to unchanged
transaction code; the final UI candidate still needs native verification.
Initial frontend build: 3.51 s, max single-process RSS 1,033,420,800 bytes.
Initial native build: 74.49 s, max single-process RSS 1,520,713,728 bytes; zero swaps.
No native interaction latency or process-tree memory peak was measured. App quit;
read-only fixture permissions restored. Binary/launcher backups and fixture are
retained for the remaining native cases and final cleanup.

## Final native acceptance before simplification

Refusal-feedback binary: `a99f81ff94111df16c73f515f64673301ea219cca83fb675707df6230ae00b1a`.
Source identity recorded during simplification: `0581ec0d5e9c18c835e20f3c1a96d25c99a7dff5be3b2a684b4ba31011991623`.

- Read-only project directory: inline alert and toast show Permission denied and
  the failed event ID. Both remains selected; History records failed, not done.
  Restoring permission and retrying clears the alert, applies User only, and retains
  Copy ownership. The project remained registered across the rebuild/restart.
- Disabled Copy: User only succeeds, disabled switch stays off, registry flag/path
  stay disabled. Undo restores original document and removes the sidecar.
- External document edit: automatic refresh changes owner to Unknown/Read-only,
  disables alternative policies and the location switch, and removes Edit.
  External bytes remain intact. Restoring the known fixture restores ownership.
- Seeded project interruption after document publication: quit the app, set the
  selected unreverted invocation event to interrupted, restore only its sidecar and
  registry from the verified backup, retain the proposed document, then relaunch.
  Startup restores the proposed sidecar with unrelated display_name intact and
  marks History done. All four Copy content hashes match fresh file hashing.
  This is seeded-state evidence, not a native SIGKILL claim.

Rebuild after the UI fix: 55.87 s, maximum single-process RSS 1,527,218,176 bytes,
zero swaps. Preflight 52% memory free, 62 GiB disk. No interaction-latency or
process-tree memory measurement. Native app exit confirmed; original test launcher
and binary restored and hash-checked, temporary binary backup removed. Small native
fixture remains for final review follow-up; disposable logs will be removed after
review. Fresh simplification is active; no commit or PR yet.

## Post-green review state

A fresh simplifier removed one unreachable guard after sidecar source selection.
Root inspected the change and reran the affected exact History matrix (14.23 s),
sidecar interruption recovery (1.60 s), and strict core Clippy (0.15 s); all passed.
The change cannot alter a reachable publication path, so unchanged native evidence
is reused. No native rebuild or full suite was repeated for that removal.
Source identity after simplification: `0581ec0d5e9c18c835e20f3c1a96d25c99a7dff5be3b2a684b4ba31011991623`.
Independent read-only review is active. No task-owned test/build/native process
remains. Peak memory for these focused reruns was not measured.

Build-log cleanup: retained summaries above and removed the passing raw native
and frontend build logs. Final frontend rebuild after inline feedback: 3.31 s,
maximum single-process RSS 1,050,902,528 bytes, zero swaps. The core module log
and small native fixture remain until review completes.

## Review corrections — September 16

Refuse wildcard-dotagents in invocation editability, ambiguous/wildcard ownership
in the backend guard, and Plugin ownership even when plugin metadata is absent.
The existing submit guard prevents read-only invocation files from being sent.
Reject present non-mapping Codex policy values instead of replacing them.

Focused checks: core invocation transform tests 3 passed (6.19 s compile,
under 0.01 s tests); desktop write refusal tests 4 passed (11.63 s compile,
0.01 s tests); UI routing tests 47 passed (0.756 s overall); frontend typecheck,
scoped oxlint, and diff whitespace check passed. Preflight: 54% free memory,
62 GiB available disk. Peak memory not measured. All these sessions exited.
Review follow-up is active; native checks for changed refusal paths remain.
The legacy non-Copy sidecar writer is unchanged by this correction.

## Final correction acceptance

Independent review follow-up accepted all three fixes, with no new finding.
Native binary `0f6a4b9b703c6e1c2ac9130e3449cc5f6082ba6325cf9658d6db9d97659f79e7`:
- Project Copy with a matching registry hash and `policy: custom`: selecting Both
  shows inline and toast `openai.yaml policy is not a YAML mapping`, keeps User
  only selected, and preserves document and sidecar bytes.
- Wildcard dotagents fixture: invocation alternatives are disabled. The existing
  source ledger labels this owner Ambiguous and lifecycle Read-only.

Release build: 74.16 s, max single-process RSS 1,526,644,736 bytes. Frontend build
2.56 s. Earlier successful native transaction/History cases remain valid for their
unchanged paths. No full suite rerun locally. Original launcher/binary were restored
and hash-checked after confirmed app exit. Modified fixture state restored; review
backup and wildcard fixture removed. Test peak/process-tree memory not measured.

Final source identity: `c34130caafef351c3ed0036f17299dee855d2df62812fff22fc9ce07e4f54332`.

## Linux CI correction

Initial CI found one formatting-only backup guard issue, corrected in `42ffa3c`.
Run 35059242763 then passed formatting and Clippy but failed sidecar creation:
cloning a retained directory handle and calling fsync produced EBADF on Linux.
Later failures included the resulting poisoned process coordination gate.
The creation path now opens `.` relative to the authorized directory before sync,
matching existing core write paths. The focused absent-sidecar creation test
passes on macOS (5.86 s compile, 0.06 s test). Linux CI remains required; macOS
native evidence alone does not prove the platform correction.
