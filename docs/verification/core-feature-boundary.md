# Core optional feature boundary

The combined desktop workspace could not compile the shared core without
`event-store`: skills.sh managed-source helpers imported event-gated copy and lock
modules. Those helpers do not need the event database.

This change exposes the existing pure helpers on Unix and moves the strict lock
JSON parser from Fork execution to lock transition. The old crate-internal parser
path remains available. Parsing behavior, duplicate-key rejection, the 8 MiB limit,
error text and managed-source APIs remain unchanged. The unchanged BackupCopyLimits
struct also moves into a private pure module; its original public path reexports
the same type. This avoids importing an event-gated backup implementation merely
for limits. The reviewed parent already
compiled without event-store; this fixes the dependency boundary required by its
later integration consumers.

Verification uses locked offline Cargo with two workers, one test thread,
incremental/debug output disabled and a shared target outside the repository.
The integration candidate passes no-default-features check, two lock-transition
tests, one nested duplicate-key parser test, and strict event-store core Clippy.
The review branch passes the same checks: 1.59s build check, 15.68s lock tests,
0.27s parser test and 10.05s Clippy. Largest single-process RSS was
1,439,973,376 bytes. No native rerun is needed
for unchanged parser behavior and module placement; prior desktop acceptance is
retained. No application memory measurement or production telemetry claim.

The mechanical code relocations use the mechanical-change simplification
exception. Independent review covers exact source and feature gates. Test logs,
source identities and resource results are kept briefly under
`/tmp/skill-studio-delivery/core-feature-boundary`; useful results are summarized
before disposable output is removed. Final combined release acceptance remains
separate.

Final correction review is clear. Four integration staged-source tests also passed
in 0.48s test execution. Optional builds retain dead-code warnings when consumers
are disabled (13 review-branch warnings, 15 integration warnings); this is a compile
check, not a warning-free optional-build claim. Event-store strict Clippy passes.
