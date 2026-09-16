# Dotagents Fork creation acceptance

September 16, 2026. Review base: `0822f71982953a7fbd76ec6e2359a681ba96c50f`
(PR111). The first candidate passed the native cases below. Simplification was a
no-op. Independent review found gaps in fetched symlink preservation, fetch size
limits, saved-document evidence validation, and duplicate manifest row handling.
Corrections and independent review are complete; corrected native acceptance is
recorded below. PR113 code commit `4a6de23` passes CI; integration reconciliation
and independent port review are complete. Final combined release acceptance is open.

## Reviewed corrections — September 16

The four original findings and two follow-up findings are corrected. Independent
correction review found no remaining issues. The original no-op simplification
predates these corrections; no claim is made that the old native binary tests them.

- Controlled fetch caps archive stdout at64MiB and decoded tar data at256MiB,
  including archive metadata, with10,000 entries and depth32. Staging separately
  limits bytes/entries/depth. These are resource limits, not performance targets.
- Fetch/staging retain literal symlinks and file/directory permissions. Unsafe
  paths and traversal through archive symlinks are refused. Existing download
  entry points retain their bounded-default delegation.
- File output waits for its writer to finish before accepting success and checks
  late size/write failures. Diagnostic stderr retains its independent64KiB limit.
- Recovery compares saved documents/absence marker with intent before each prefix,
  including descriptor and directory-entry metadata checks for replacement races.
- Duplicate selected manifest rows are refused before fetch and before detach.

Final reviewed source identity:
`cfb9846437b219bd0f807fba7967ff50d7410779801f2f4be0f44dd37b011c54`.
Core Fork11 passed/1 ignored(9.77s); duplicate-row1 passed(0.01s); archive4 passed
on the final directory-mode correction(0.23s); staging/output regressions passed
(0.44s with one extra bounded-scope case); evidence-filter10 passed(3.07s, includes
other existing evidence cases). Strict desktop/core Clippy pass after final edits.
All root checks used offline locked resolution, two Cargo workers and one test
thread. Test peak RSS was not collected. Frontend source/assets remain unchanged
from the original candidate and its passing typecheck/build evidence is reused.

Corrected native acceptance passed on binary
`a3031d0237d116724d21c3daa763fb52e1d8951ea96dd64d12ed3c5dbb860b11`:

- Forward event `01M2NH5QR4NJW02GYS2VY9XX1Q`: native owner becomes Fork;
  valid/dangling links and exact upstream bytes remain in the baseline; live and
  sibling files unchanged; selected provider rows detached; unknown fields retained.
  Activity shows done with Reveal and no Undo.
- Real manifest-checkpoint fixture event `01M2NH7894C70ZNJJJT50G2F2N`:
  changing saved agents.lock evidence before startup refuses recovery; all recorded
  fixture files remain unchanged, event stays interrupted/non-restorable, and Home
  warns and links to Activity. The modified evidence remains for diagnosis.

Build79.23s, maximum process RSS1,424,244,736bytes, zero swaps. Test/application
peaks unmeasured. Corrected native fixture generation passed1 case in0.74s. Task
native apps were stopped after verification. Two superseded builds were not launched.
Published as [PR113](https://github.com/sergical/agent-studio/pull/113), code
`4a6de23eb892128d300f347b10f5c444c98fd7d6`. Core CI35123157425 passes
(587 tests, 16 ignored; 143.88s tests, 3m41s job); frontend35123157407 and
GitGuardian pass. CI artifacts retain three days.

## Product outcome

Fork an eligible Global Universal Dotagents skill without changing its live files.
Save the fetched upstream tree as its merge baseline, detach only its provider
records, and record a durable History event. Restart completes an interrupted
publication when its evidence still matches. Conflicting edits remain untouched
and visible as an interrupted operation. Fork does not offer generic History Undo;
Unfork is a separate workflow with separate acceptance.

The desktop fetch is bounded and runs off the UI thread. The core rechecks the
selected deployment, owner revision and source after fetch. An owner-group request
is refused. Skills.sh keeps its existing route; this packet does not accept its
ordinary Fork durability, Pull, or Unfork.

## Earlier candidate native evidence

Earlier tested source identity (sorted changed/untracked source path NUL bytes NUL,
excluding docs):
`01114f8100d6e76d733a5b3143c60bc301bd14dcbaa37362694def63677dd54b`.
Earlier packaged binary SHA256:
`9d87f1fc51f4b421cdd84c199ca37c62da8d957d690997cf12615869f699f557`.

Actual Tauri windows were controlled with native accessibility tools. Fixtures
used isolated HOME/CFFIXED_USER_HOME, denied real-home reads and network, and
allowed writes only to task fixtures, the test bundle cache and `/dev/null`.
The gh shim supplied a local archive. No real provider installation was performed.

- **Forward:** native-dotfork changed from Dotagents to Fork in the UI. Exact live
  file sets and hashes remained unchanged. Only its provider rows were removed;
  sibling rows and unknown fields remained. Merge-base bytes matched the archive.
  Event `01M2NENHZP74MCK45N5FPCHQP0` is done/non-restorable. Activity exposes Reveal,
  with no Undo. No publication staging remained. This forward run used the same
  Rust code before the subsequent IPC error-message-only frontend correction.
- **Fetch failure:** the final binary showed “Fixture upstream is unavailable”,
  retained the sibling skill and manager rows, cleared its busy state, and created
  no additional event. The original Unknown error toast was corrected by normalizing
  Fork IPC errors with the existing error-message helper.
- **Restart:** the ignored core generator stopped the production operation after
  manifest detachment. Native startup completed event
  `01M2NEXPY9W043ZZJXA347QMEP`. Live files matched immutable live backups, the baseline
  matched immutable upstream, and provider bytes matched the recorded projection.
  Home showed no unresolved recovery; Activity showed done without Undo.
- **Conflicting restart:** a separate generated lock-detachment fixture received
  an external manifest edit before native startup. Event
  `01M2NEZV52G004YMFXBNMQ5XKK` remained interrupted. Recorded fixture-file hashes
  remained unchanged. Home showed “Some skill changes need review” and linked to
  Activity, which showed the interrupted operation without Undo.

The first forward attempt was a fixture failure: its sandbox prohibited `/dev/null`,
which tar output uses. A direct sandbox probe confirmed it; allowing that literal
path fixed the fixture. Direct executable launch did not register a controllable
native bundle, so subsequent runs used LaunchServices with the same sandbox launcher.
These were test setup changes, not product fixes or passing native runs.

## Focused checks

Use `CARGO_BUILD_JOBS=2` and the compatible shared Cargo target cache. Commands
run from the review checkout; each test command selected a nonzero number of tests.

- Core `cargo test --offline --locked --manifest-path crates/skill-studio-core/Cargo.toml --features event-store --lib skill_fork_creation -- --test-threads=1`: 10 passed, 1 ignored; final test time7.85s.
- Same core command with filter `skill_fork_transition`: 2 passed,0.02s.
- Desktop `cargo test --offline --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib skill_startup_recovery -- --test-threads=1`: 6 passed,0.02s (38.23s compile in recorded run).
- Same desktop command with filter `skills::skill_fork::tests::fork_command_requires_one_exact_deployment_target` and `--exact`: 1 passed. An earlier short-name filter selected zero and was rejected as evidence.
- Strict core Clippy with `--features event-store --all-targets -- -D warnings`, desktop Clippy with `--lib --tests -- -D warnings`, and rustfmt checks for both manifests pass.
- `NODE_OPTIONS=--max-old-space-size=3072 npm run build --workspace skill-studio`, scoped oxlint and oxfmt for `apps/desktop/src/lib/skill-api.ts` pass.

Core cases cover selected/sibling/trial projection, absent registry, stale revision,
competing ownership, cancellation after intent, changed immutable evidence,
external metadata conflicts, stale-baseline replacement, pre-journal failure
cleanup, and reopen recovery at outer and inner publication boundaries.

Generate native restart inputs with the ignored test
`skill_fork_creation::tests::generate_native_restart_fixture`, adding
`--exact --ignored --nocapture --test-threads=1` and explicit
`FORK_CREATION_FIXTURE_PARENT` under the task temporary directory plus
`FORK_CREATION_CHECKPOINT=manifest` or `lock`. The generator invokes the production
checkpoint path; no SQL intent is fabricated. Generator runs passed in0.77s/0.71s.
Local launcher and assertions reside in `/tmp/skill-studio-delivery/fork-creation`.

## Resources and remaining limits

Only one heavy job ran at a time. Native preflight reported52% free memory/92GiB
free disk initially, then45%/91GiB. The initial packaged build took80.35s and peaked
at1,452,867,584 bytes single-process RSS. After the error-message edit, frontend
build took3.79s/1,031,766,016 bytes RSS; final native build took57.98s/1,477,214,208
bytes RSS. Builds reported zero swaps. Test and application peak memory were not
measured; build RSS is not application memory. No numerical performance gate is
claimed. All native test processes were stopped and exit checked.

A failure inside backup reservation after directory creation but before its retained
handle is returned can leave an unjournaled directory. Definite failures after a
successful reservation now remove the owned reservation. Uncertain event writes
retain evidence. Both task test apps exited. Removed consumed app bundles, forward
and successful-restart fixtures, superseded build logs (59,815,172 bytes) and native
caches. Unresolved conflict/evidence fixtures, reproduction tools and current
diagnostics remain under the task temporary directory.

## Integration acceptance

Integration source identity:
`ed1dc31a86f1df4a6db9fcb7d2cc06dedc849322fc8a431741d18d37fb1d1c2c`.
The port adapts proposal accessors and reuses the existing bounded saved-document
reader, process runner and archive parser. It preserves existing receipt and
Pull/Unfork paths. Staging preserves links and modes; archive extraction retains
explicit directory modes. No integration Cargo dependency changed. Independent
port review found no issue.

Scoped checks pass: core Fork 11/1 ignored (9.60s), archive 5 (0.03s), startup 7
(1.42s), exact-target 1 and staging 1. Desktop check (5.71s), strict Clippy (8.46s),
frontend typecheck and scoped lint/format pass. Cargo used offline locked resolution,
two workers and one test thread; Node heap was bounded to 3072 MiB. Peak test RSS
was not measured. No native build or full suite was repeated for the port. Final
combined native acceptance remains open.

Consumed integration preimages were deleted after review. The retained manifest is
`/tmp/skill-studio-delivery/fork-creation/integration-acceptance.json`.
