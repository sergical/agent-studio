# Fork Pull delivery

Review base: `04d21af` (skills.sh Fork, PR123). Review branch:
`codex/fork-pull-delivery`. Focused verification, fresh simplification, independent correction review, and
packaged native acceptance are complete. CI and integration reconciliation are
tracked separately in the acceptance ledger.

## User outcome

Pull upstream into a Global Universal Fork while keeping local changes. Both
Dotagents and skills.sh origins use the same core operation, including legacy
Fork records. Clean changes merge; text conflicts retain markers and binary
conflicts keep the local version. Conflicts are reported in the desktop. The
upstream baseline advances after successful publication, including conflicts.

A completed Pull records a non-restorable Activity event. Generic Undo is not
supported. Interrupted publication is recovered from verified local evidence;
recovery does not fetch or replay a provider command. Changed or replaced data
is preserved and leaves the operation unresolved for inspection.

## Implementation

The shared core prepares exact live/base/registry inputs, merges a bounded tree,
revalidates ownership and inputs, records durable intent, then publishes the live
tree, baseline, and selected registry row. Recovery recognizes each publication
prefix. Files, valid/dangling links, modes, empty directories, and unknown registry
fields are preserved according to the merge rules. Unsupported conflicts refuse
before publication. The desktop retains bounded fetch and `git merge-file` adapters
and executes Pull off the UI thread.

Home Pull latest and Update all report conflicts; detail Pull selects the exact
deployment. Activity labels the operation and offers no Undo. Startup routes the
new event through ordered recovery, and existing Home recovery status exposes
unresolved operations.

## Verification record

Initial candidate native results are recorded in
`/tmp/skill-studio-delivery/fork-pull/native-initial-results.json` with source and
binary identities. Home clean and conflicting Pull passed disk assertions for
live files, baseline, registry, unrelated data, links, modes, and empty directories.
Activity showed the completed event with Reveal and no Undo; Home displayed the
SKILL.md conflict. The real desktop git adapter produced the conflict markers.

The detail target mismatch was corrected to select the exact deployment. Review
also corrected unilateral node-type changes, cleanup on early preparation failures,
and root-mode conflict reporting. Independent correction review found no remaining
issue. Fresh simplification removed a temporary allocation without changing behavior.

Final source identity: `35029eae327c5c5ce40eafa3cb56c6ea23313a1eed7b4ff817baeccf770a6a98`.
Native binary SHA256: `89cbbfeafb21c0c33213e03dd67888ccfaf8998876151a8ec8d48cadbf56d248`.

Final packaged cases:

- Detail conflicting Pull completed (`01M2NWF4DPTAE9SPAXBQ8S86SE`): conflict markers,
  exact upstream baseline, registry and unrelated files verified; Activity done,
  Reveal only. The transient detail toast was not captured.
- Startup with canonical base absent between publication renames completed
  (`01M2NVN0NR1PT6AXVVRE8CVRG4`); live/base/registry agreed and Activity showed done.
- Startup after an external live edit preserved live/base/registry and remained
  interrupted (`01M2NWHJY7X8ABAK0WKRMX5ZNC`). Home warned and linked to Activity.
- Missing baseline refused before mutation: files/registry unchanged, no Pull event
  or provider invocation. The error currently uses OS wording: No such file or directory.
- Update all completed one clean and one conflicting Fork. UI showed Updated 2 of 2
  deployments and 1 conflicts need resolution. Clean live/base matched upstream
  exactly; conflict markers and both non-restorable done events were verified:
  `01M2NXE3HN6JHFHCAJ45YHCS41`, `01M2NXE4MMXJG2523XRRXGCD2V`.

Focused checks:12 core Pull tests passed/1 generator ignored (8.13s), tree exchange
1 passed, desktop Fork28 passed, process10 passed, startup7 passed. Strict desktop
Clippy passed (6.06s); frontend typecheck, changed-file lint/format, rustfmt and diff
checks passed. Unchanged process/startup checks were reused. Core tests cover both
origins, current/legacy records, durable prefixes, stale/replaced evidence, merge
rules and permission/link preservation. The real git adapter is exercised natively;
there is no separate direct automated adapter test.

Final release build80.19s/max process RSS1,533,116,416bytes/zero swaps; initial build
85.09s/RSS1,443,577,856bytes. Initial frontend5.82s/RSS896,548,864bytes; frontend was
rebuilt after the detail caller edit. Runtime/test peak memory was not measured.
Preflight before final native build:49% memory free,62GiB disk free, no competing
task build/app. All managed native sessions exited.

Verification corrections: launch the bundle through a managed `open -W -n` session
so native automation can resolve it. Frontend checks need checkout-local workspace
package links; ancestor packages gave unrelated type errors. Use the fully qualified
ignored fixture filter below: an earlier short filter also selected three unrelated
generators, which refused missing environment inputs; the Pull generator passed.
The conflict startup assertion expects interrupted, as set by the desktop dispatcher,
rather than the core fixture's initial pending status.

## Reproduction and limits

Build fresh frontend assets, then use `cargo build --offline --locked --release
--features tauri/custom-protocol --manifest-path apps/desktop/src-tauri/Cargo.toml`.
Use two Cargo workers and the absolute shared target directory. Native fixtures
and assertions are outside the repository under `/tmp/skill-studio-delivery/fork-pull`.
The ignored `skill_fork_pull::tests::generate_native_restart_fixture` core test creates a real pending
operation at a selected publication boundary. Set `FORK_PULL_FIXTURE_PARENT` to a
task-owned directory and `FORK_PULL_CHECKPOINT` to `intent`, `live`, `base-moved`,
`base`, or `registry`.

Native fixtures deny real-home reads and network access, and use a controlled gh
response. They exercise the actual Tauri IPC/core/filesystem/merge path, but do not
certify a live upstream service or process kill at every instruction boundary.
Final combined application acceptance remains separate. No merge or deployment.

Temporary evidence is summarized in `native-final-results.json` outside the repository.
Disposable bundles and superseded logs are removed after recording results.

CI correction: the first core job stopped at core-package Clippy; the earlier
local desktop invocation had not linted this dependency as a primary package.
The correction adds narrow enum-size/argument-count exceptions, combines identical
merge branches, removes one needless reference, and clarifies fixture conditionals.
Core `cargo clippy --offline --locked --features event-store --all-targets -- -D
warnings` now passes (7.14s); all12 focused Pull tests pass (8.19s). No persistence,
merge result, or desktop behavior changes; native evidence above is retained.
Frontend CI35145123422 passes (55s). The replacement core job remains pending.


## Integration acceptance — September 16

The combined `codex/shared-core-design` worktree retains later owner-update,
process-streaming and recovery code. Port review corrected Home/mixed-owner
routing to pass an exact Fork deployment ID. Native integration then found the
generic content fingerprint guard rejected dangling resource links. The Pull
adapter now uses fresh exact Fork identity/mutability checks; the unchanged core
retains its own no-follow snapshots, ownership revision and lease revalidation.
Generic lifecycle content guards remain unchanged. These integration corrections
are retained in the combined worktree for its final review/commit series.

Evidence:26routing tests,typecheck,lint,format;1adapter/core link regression
(0.68s tests,30.50s compile),strict desktopClippy7.81s and rustfmt. Fresh
simplification left no changes; independent correction review found no issue.
Packaged native Home single Pull and Update all(clean+conflict) passed. The latter
showed “Updated 2 of 2 deployments” and “1 conflicts need resolution”. Activity
showed all three operations done with Reveal and no Undo. Both clean live/base
trees equal upstream; conflict/local data, valid/dangling links, modes and unknown
registry fields were checked.

Source:`77bd093b77d03310a0adc49ced1ef809ace76430e646d77b37db9c5c23a4cf4b`.
Binary:`92f57cae9082689e3c01883f7c7caa42c2aa46ccd9008f6faa188b5620547b6d`.
Events:`01M2P07TT72W3MFZTMZ3KEE1BK`,`01M2P0AAF84H6SXYGDVY0JXYD8`,
`01M2P0ABKNXADXGN1S7W832Y9M`. Build62.16s,max processRSS1,550,188,544bytes,
zero swaps; runtime peak unmeasured. Managed sessions exited; the35,915,347-byte
disposable bundle was removed. Final results and reproduction tools are retained
under `/tmp/skill-studio-delivery/fork-pull/integration`. The initial refusal
preserved live trees and emitted no Pull event. Combined release acceptance
remains outstanding.
