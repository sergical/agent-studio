# Core inventory desktop acceptance

Review base: PR80, commit `0d4b48bcafe37f0869c3076b1decba33fa44f637`.

## Delivered behavior

Desktop full and named refreshes use shared core discovery and ownership. They
retain desktop project discovery, update overlays and invocation statistics.
Named refresh preserves unrelated lock-only records. Incomplete reads produce a
visible warning; unsafe named reads retain the prior snapshot. Unknown ownership
blocks document/invocation edits and location controls. Existing lifecycle adapters
remain where called; coordinated writes, Copy removal and journal/startup recovery
are separate batches.

Settings saves linked-skill folders and complete plugin ownership boundaries in
`.agents/skill-studio-scope.json` under the provided home. Reads are capped at
64 KiB. Saves validate at most 64 absolute non-root directories per list, validate
physical targets, then write a new temporary file and rename it atomically. Async
IPC keeps file operations off the UI thread and requests a background full refresh.
`SKILL_STUDIO_SCOPE` overrides saved preferences and makes these controls read-only.
Backing roots permit linked content reads; they do not establish plugin absence.

Git ancestry truncated at a declared boundary remains Unknown source. Complete
plugin/manager inputs can still establish Manual ownership and permit local edits.
Metadata failures and invalid Copy records retain conservative refusal. The existing
Read-only lifecycle label refers to manager actions, even when local editing is
available; that wording remains a known clarity issue.

## Native acceptance

All checks used the actual Tauri window through native Computer Use, a fixture HOME
and CFFIXED_USER_HOME, denied network and real-home reads, and fixture-only writes.
No browser approximation was used.

- Global/project/plugin inventory and parent-plugin ownership appeared correctly.
- Plugin-owned and Unknown-owned skills refused direct edits and invocation changes.
- Malformed lock feedback appeared automatically; Home did not report All clear.
  Restoring valid records restored supported controls automatically.
- A home without `.git` and no manager entry allowed User only, followed by Both.
  The skill returned to its original SHA256:
  `7a439dd45d17c187aedcec21c5f29ba9649d34c3f8e55c15a8bfc0b24258884c`.
- Saving/removing an ownership folder refreshed inventory without deleting skills.
  Restart retained it. Invalid relative paths preserved the previous saved value.
  The native folder chooser opened and cancellation left settings unchanged.
- Adding a backing folder made linked content readable while incomplete ownership
  remained Unknown. An empty launch override excluded saved roots and disabled the
  Settings controls with an explanation.
- Synthesized reader rows refused unknown-owner changes. Their menus offered Reveal.
- Saving an already-watched project as an ownership root upgraded its watch; editing
  a plugin manifest then updated its displayed name without a manual rescan.
- Linked and direct disabled manager rows refused changes with malformed manager
  records. Repairing the registered project's lock restored local edit controls.

## Focused verification and reproducibility

Use two Cargo workers and one test thread, with compatible cached targets. Core:
`cargo test --offline --locked --manifest-path crates/skill-studio-core/Cargo.toml
--lib skill_ownership::tests -- --test-threads=1`; substitute the desktop manifest
and `skill_refresh::tests` for refresh coverage. Clippy uses `--lib --tests -- -D warnings`.
Build frontend assets before the packaged native command:
`cargo build --offline --locked --release --features tauri/custom-protocol
--manifest-path apps/desktop/src-tauri/Cargo.toml`.

Earlier checkpoints passed 409 core and 660 desktop tests. These predate corrections
and are not evidence of final combined acceptance. Relevant focused checks passed:
102 discovery tests (one explicit skip), 3 write-refusal, 21 invocation, 6 settings,
16 frontend ledger-model tests, frontend typecheck and scoped lint/format. Subsequent
ownership, refresh and location checks are recorded below. No full local suite was
repeated after minor edits; CI covers the final core candidate with three-day log
retention. Desktop integration CI is not configured in this review base.

Settings source identity before correction review:
`f4f88e8f6e5a4df266e84e6c7fb787231d6c74965b2548b01051777d9f31ae96`.
Settings tests: 8.60 s build / 0.40 s tests; desktop Clippy 5.26 s.
Frontend build: 4.03 s, maximum single-process RSS 975,699,968 bytes.
Its native build: 57.35 s, maximum single-process RSS 1,406,418,944 bytes.
Rebuilds followed source edits. Test peak memory was not collected.

## Review corrections and current evidence

Whole-extraction simplification and review completed. Findings about named refresh
and Unknown wire labels were corrected. A separate Settings/guard review found
unread backing-manager records, synthesized reader guards and watch mode upgrades.
Those findings were corrected, tested and natively checked below.

Corrections are now natively verified at source identity
`477fc63fe65d9429199dcacb0f45e2b3132a511ab9ff8ca1b909ed7f1cbc4120`
(excluding docs). Native testing exposed two further variants of the unread manager
record issue: disabled targets behind links and direct disabled project rows. The
ownership lookup now finds the enclosing `.agents/skills` root for both forms.
Regression coverage includes normal, disabled, nested and nested-manager targets.

Final focused checks: 53 ownership tests passed (1.13 s tests), core Clippy lib/tests
passed (2.59 s), and 31 desktop refresh tests passed (8.09 s build, 0.62 s tests).
Unchanged reader/watch corrections retain 45 passing frontend location tests and
native acceptance: generated reader switches refuse changes and menus offer only
Reveal; saving an already-watched project as an ownership root upgrades its watch,
and a later plugin-manifest edit updates its heading automatically.

Final native acceptance on September 15: with a registered project's malformed lock,
the linked global row and direct disabled project row both show Unknown ownership,
no Edit, and disabled change controls. Restoring a valid empty lock automatically
restores Manual ownership, Edit and invocation controls. The temporary Mac lock
was resolved; native acceptance is no longer blocked.

Latest packaged build: 60.83 s, maximum single-process RSS 1,461,501,952 bytes,
zero reported swaps. This is a build measurement, not application peak memory.
Native preflight: 40% memory free, 59 GiB disk free, no competing build process.
Test peak memory and native interaction timings were not collected. No full suite
rerun. App exit confirmed; original launcher, binary, locks and project registration
restored; added fixture directories and the 33 MB binary backup removed. Compact
native diagnostics and source baselines remain in `/tmp/skill-studio-delivery`.

Fresh correction simplification completed with no edits. Root recomputed source
identity `477fc63fe65d9429199dcacb0f45e2b3132a511ab9ff8ca1b909ed7f1cbc4120`
and confirmed the no-op. A separate fresh read-only correction review completed with no actionable introduced
defects and no edits. Root inspected its result. Delivered in draft PR #93; CI remains pending. Whole-extraction review is already complete; these
passes cover the five corrected files against the saved pre-correction baseline.

## CI follow-up

The first Ubuntu CI run (35013222751) passed formatting but Rust 1.98 Clippy rejected
a large channel-send error returned by a test thread. The test now maps that error
to unit; the existing double unwrap still fails on thread or send errors. No
production behavior changed. The FIFO regression passed locally (5.57 s build,
0.01 s test); formatting passed. Test peak memory was not measured. Preflight:
42% memory free, 59 GiB disk free, no competing task-owned process. The next CI
run checks the final core suite. No native rebuild is required for this test-only fix.
