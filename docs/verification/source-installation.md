# Add by source acceptance

September 16, 2026. Review base: PR103 head `0f49384`.

## Delivered behavior

Project requests without a path or with an empty path now fail before direct
single/batch provider dispatch and before background root scanning. All supported
methods and destinations use the same validation. Global behavior is preserved.
The Add Skill project picker now registers the selected project with the backend
before saving the selection. Failed registration shows its error and does not
save the rejected selection. Registration returns only accepted paths after
canonical home-directory filtering; the picker refuses home and its aliases
before persisting or selecting them. Existing startup batch filtering is retained.

## Evidence

- 63 focused Rust Add/operation tests pass (0.23s execution). The new matrix covers
  missing/empty Project paths for single and batch dotagents, skills.sh, Universal
  Copy and Per-harness Copy. It checks failed terminal state, no provider/fetch/
  lookup invocation and no temporary-home writes.
- Existing tests cover Copy collision/source ancestry refusal, registry refusal,
  publication/cancellation, trial cleanup warning, mixed-batch outcomes and exact
  ownership records. These unchanged tests remain valid.
- Strict all-target Clippy and scoped Rust formatting pass. One inherited Update
  test clone-to-slice warning was corrected mechanically.
- Frontend typecheck, scoped lint and formatting pass after the final picker correction.
- The focused backend registration test passes with home, home-dot, trailing-slash
  and symlink aliases in a mixed batch; only the valid project remains. Strict
  all-target Clippy and scoped rustfmt pass after the return-contract change.
- Fresh simplification removed duplicate validation; independent Rust review found
  no actionable issue. Final picker follow-up review found no actionable issue.

Native Tauri acceptance uses a disposable HOME/project and a controlled provider.
Network is denied; real-home reads are denied; writes are confined to the fixture
and its unique cache. This verifies routing, UI feedback and ownership, not live
upstream provider behavior.

| Case | Result |
| --- | --- |
| Global local Copy | Managed Copy appears with original content and expected Claude link. |
| Duplicate Copy | Exact destination-exists error; existing content preserved. |
| Global Per-harness Copy | Selected Claude/Codex destinations appear as separate managed copies. |
| Project Universal Copy | Correct Project files and ownership records; final picker fix makes location immediately visible without restart. |
| Project Per-harness Copy | Separate Claude/Codex directories match source; exact Project records; no Global independent copy created. |
| Untrusted dotagents | Trust prompt appears; provider log absent before confirmation. |
| Trust and retry | Confirmation persists normalized source identity and invokes provider; Global skill appears as managed dotagents. |
| Trusted Project dotagents | Provider cwd is selected project with --project; same-name Global installation remains; both locations visible. |

Initial native binary:
`c17df255621801f298b407b5d60d39b8d01c4947a2d53351ab959a4f9a3a50b0`.
Global Copy/collision/Global Per-harness results are from this candidate. Their
source and dispatch paths are unchanged by the later picker correction.
Project/dotagents candidate:
`e9706c1e69ca9d0c72a60861ec2663afcb7e28ad21001cc74e343af666090887`.
Project reconciliation, Project Per-harness and dotagents cases use this candidate.

Final home-refusal/Project-picker candidate:
`341d28517aa90dc38632a2b53ee0ba828a91fa1ed692dad07bf9605e20d352ac`.
Native home selection showed the scope refusal and preserved the previous Project.
Selecting a fresh project then installing Copy immediately showed both Project
locations. Disk checks confirm matching content, no Global Copy, and no home
entry in tracked projects. Backend tests cover canonical home aliases.

The first Project test exposed missing backend registration: files/registry were
correct but inventory lacked the project until restart. The fix was verified by
selecting a fresh project and observing its location immediately after install.

## Resource use and limitations

Two Cargo workers, one test worker, Node heap3072MiB and a compatible shared Rust
cache. Initial frontend build3.84s/max RSS871,596,032 bytes; no swaps. A later Vite
build failed with system-wide file-handle exhaustion (os error23, signal10).
No source fix or dependency reinstall was needed. After usage fell to9370 of276480
handles, verification resumed with reduced Rayon/UV concurrency. The temporary
fixture directory disappeared during interruption; recreated fixtures only after
confirming old process handles were missing and no build was running.

Project/dotagents native build60.72s/max RSS1,500,905,472 bytes.
Final correction native build57.52s/max RSS1,505,329,152 bytes, zero swaps. Test peak RSS was not collected.
The generated frontend was inspected to confirm it includes registration before
saving Project selection. Native app exited after acceptance. Reproduction inputs
are ordinary SKILL.md/resource files and a guarded dotagents shim writing a named
manifest/lock entry; no real repository or user skill was modified.

This batch does not add installation Undo or claim rollback of upstream provider
changes. Fault-injection tests establish partial-outcome/cleanup behavior; native
cases above establish UI wiring. Combined release acceptance remains separate.
