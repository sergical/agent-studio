# Copy document editing verification

Base: PR98, `08fc0ca`. This batch adds owned Copy document saving, guarded
Undo/Redo, and restart recovery through the desktop. It also persists explicitly
tracked project roots so the same scope remains available after restart.

Source identity before final native acceptance:
`69dbd0c15c90b01ed3b52088643ad3bc1020dff1b5b38d778dbc92785f13263f`.
This hashes each sorted changed Rust/TypeScript path, NUL, file bytes, NUL.

## Automated acceptance

Use `CARGO_BUILD_JOBS=2`, one test thread, compatible Cargo target caches, and
`NODE_OPTIONS=--max-old-space-size=3072`. No production telemetry is exported.

- Core `skill_copy_document_edit`: 27 passed; one directly skipped subprocess
  helper is invoked by its parent interruption test. Wall 109.78 s, tests 97.01 s,
  maximum single-process RSS 1,052,672,000 bytes, zero swaps. These execution
  assertions are supplemented by the changed-boundary checks below.
- Core `skill_ownership::tests`: 54 passed, compile 5.51 s, tests 1.10 s. Actual
  outside-home discovery and a registry loaded from disk establish Copy ownership;
  mismatched records stay Unknown. Confirmed plugins and failed reads remain guarded.
- Desktop `skill_document_save::tests`: eight passed, tests 12.02 s. Project fixtures
  are outside HOME. Covers exact target/alias refusal, Copy and direct saves,
  stale content, cancellation, Undo/Redo, production startup scope reconstruction,
  revoked project refusal, and disabled Copy recovery.
- Desktop `skill_project_authority::tests`: six passed, one subprocess helper
  skipped directly and invoked by its parent, tests 0.17 s. Covers persistence,
  malformed/nonregular/symlink settings, alias deletion after exclusion, and
  interprocess update locking.
- Desktop Copy History eligibility: one passed (4.73 s compile, 0.01 s tests).
  Save-error wire contract: one passed (6.13 s compile, under 0.01 s tests).
- Frontend operation transport/editor draft: eleven passed, 473 ms. Activity
  restore policy: three passed, 122 ms. These do not render the native UI.
- Final strict desktop Clippy passed (4.59 s). Scoped Rust format and diff checks
  passed. Frontend TypeScript/Vite build passed (3.62 s, maximum single-process
  RSS 998,653,952 bytes, zero swaps); scoped frontend lint passed.

Memory was not measured for the final focused tests or Clippy. Process-tree peak
and native interaction latency were not measured. No broad local suite was repeated.

## Native acceptance

Use the actual Tauri window, a disposable home, network denied, and real-home
reads denied. The initial candidate passed Save → Undo → Redo with exact document
and registry comparisons, resource/reader/sibling preservation, and unknown
registry-field preservation. Busy-database Stop save kept the draft and added no
History event. External document/ownership changes were refused with visible
conflict feedback and the draft retained. These UI controls have not changed.

Final binary:
`43f4085d5ef904f34c343fe42130d84b4051efbb03d6001e902b30ab5ed8e7ed`.
Release build: 73.17 s, maximum single-process RSS 1,549,434,880 bytes, zero swaps.
The native folder picker persisted the explicit project registration. The final
candidate recognizes the disabled Copy outside HOME, saves with a matching registry
hash, and preserves its disabled state and resource file. Native Undo restores the
exact original document/registry. After quit and relaunch, Redo restores the exact
saved document/registry. All three History events are done. No native process-kill
injection is claimed; core interruption and desktop startup regressions cover recovery.

App exit and absence of task-owned build/test processes were confirmed. The original
fixture launcher/binary were restored and hash-verified. Disposable home, project,
backup binaries, diagnostic scanner, and native log were removed. Passing raw logs
are discarded after retaining these results.

## Review and limits

Fresh simplification found no worthwhile changes. Independent review found and
resolved lost project scope, disabled recovery rejection, alias revocation, and
cross-process settings updates. Native diagnosis exposed outside-home plugin
ancestry overriding an exact Copy record; the ownership correction preserves
confirmed plugins, failed reads, and mismatched-record refusal. Final bounded
follow-up review reports no remaining defect.

The product target is macOS. Non-macOS legacy save behavior does not gain feature
parity. Copy invocation, other lifecycle batches, production monitoring, and
combined release acceptance remain outside this PR's completion claim.
