# Work plan

How the target in the README gets built: the architecture in units, the rule for cutting work, the list of units, how each unit is baselined and measured, the test strategy, the lint set, and what done means. The GitHub epic mirrors this file; when they differ, fix both in one pull request.

## 1. Architecture as units

Four layers. Each layer has one job and one way to talk to the next.

```
Frontend (React)         renders the snapshot, sends one IPC call per user action, applies events
   |  IPC (Tauri)  |  stdio (CLI)  |  MCP
Adapters                 desktop commands, CLI subcommands, MCP tools: deserialize, call ops, serialize
   |
skill-studio-core        ops/      one function per user job, plan -> lease -> journal -> execute -> events
                         primitives/  Root Snapshot Stage Swap Link WriteFile Journal Lease TreeHash
                         harness/   one adapter per harness: facts, detect, roots, switch, usage reader
                         dto/       the types the frontend and the schema are generated from
   |  ports (traits)
skill-studio-host        real fs, real processes (npx skills, gh, git, claude plugin), SQLite, clock, file lease, watch
```

Rules that keep the layers apart:

- The core crate has no dependency on tauri, rusqlite, tokio, or reqwest. It touches the world only through the port traits in `ports.rs`. A test asserts this from `Cargo.toml`.
- `std::fs` and `std::process` appear only in the host crate. A test greps for it.
- A desktop command, a CLI subcommand, and an MCP tool for the same job call the same `ops` function with the same request type. No adapter has a branch the others do not.
- The frontend has no business rule. It cannot decide that a skill is outdated, disabled, or in conflict; it renders the field the core sent.
- Every write goes through one path. When a new path lands, the old one is deleted in the same pull request.

## 2. The rule for cutting work

Both, and the deliverable decides which.

| The deliverable is                                                    | Cut it as         | Test it with                                                                               | Ships UI                |
| --------------------------------------------------------------------- | ----------------- | ------------------------------------------------------------------------------------------ | ----------------------- |
| A guarantee, such as "a swap is atomic" or "one writer per root"      | a primitive       | a model test and a crash test                                                              | no                      |
| Knowledge of an external system, such as "how Codex disables a skill" | a harness adapter | a fixture home captured from a real machine, plus a doc citation per fact                  | no                      |
| A user job, such as "install a skill"                                 | a vertical slice  | one end-to-end test through the CLI on a temp home, one desktop smoke, a story marked done | yes, all three surfaces |
| A number, such as "scan takes 1.9 s"                                  | a baseline        | a bench run by hand, plus a deterministic invariant in CI (work counts, never wall-clock)  | no                      |

Order: baselines first, because nothing can claim faster or safer without a number. Then a tracer slice, park and unpark, through the whole new stack, because it is the smallest write and proves the shape. Then the primitives the tracer did not need, then harness adapters in parallel, then the slices in user-value order.

A unit is one to three days for one person. If a unit needs more, it is two units.

## 3. The units

Numbers are for the epic; they are not a strict order inside a group.

### Group 0. Baseline and the UI thread

| #   | Unit                                                                                                                                                                                                                                                                                                                                           | Done when                                                                                                                                                               |
| --- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0.1 | Timing in the core: every `ops` call and each step inside it records `elapsed_ms`; the CLI gets `--time`; the desktop writes a rolling log under the app data folder                                                                                                                                                                           | `skill-studio scan --time` prints per-step times; the log shows every command with its duration                                                                         |
| 0.2 | Bench estate: a deterministic generator for a 400-skill home that matches the measured shape (63% global, names p90 33 chars, descriptions p50 249 chars, four harnesses); criterion benches for scan, park, install plan, run by hand; CI checks one deterministic invariant: the scan reads each SKILL.md once and lists each directory once | `tests/scan_work_count.rs` states the contract and is checked in ignored with today's counts in the reason; `cargo bench` runs locally and nothing in CI reads its time |
| 0.3 | No blocking on the UI thread: every desktop command that does file, process, network, or SQLite work is `async` and runs that work in `spawn_blocking`; the list comes from performance.md                                                                                                                                                     | performance.md table shows zero "blocks UI: yes" rows; the dev overlay shows no IPC call over 16 ms on the main thread                                                  |
| 0.4 | Frontend timing: a dev-only overlay with IPC round trip and commit-to-paint per action, from `performance.mark`; the snapshot-to-rows step is a pure function with behaviour tests                                                                                                                                                             | overlay exists; grouping tests pass                                                                                                                                     |

### Group 1. Primitives, in the core crate

| #   | Unit                                                                                                                                                                                          | Done when                                                                                                                                       |
| --- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| 1.1 | `fsops`: Root handle with confinement, Stage, Swap (quarantine then move in), Link (temp name then rename), WriteFile (temp, fsync, rename, refuse on stale read)                             | proptest model test over an in-memory fs; a crash test that kills after every step and checks the disk is either before or after, never between |
| 1.2 | Journal in the core: plan written and fsynced before step one, marked done after the last, reconciled on startup; the desktop event store becomes the host implementation of the journal port | 46 of 46 writes record a journal entry; startup after a simulated crash completes or reverses every plan                                        |
| 1.3 | Lease per root: `FileLease` wired to every write; a second holder gets `Busy` with the holder's pid and age                                                                                   | a real second process test; `ForkMutationLock` deleted                                                                                          |
| 1.4 | TreeHash: a git tree SHA over a skill folder that matches GitHub and the skills CLI lock file                                                                                                 | equal to the CLI's `skillFolderHash` for nine fixture skills                                                                                    |
| 1.5 | Snapshot: the scan reads each SKILL.md once and lists each directory once, whatever the number of harnesses, projects, or links; the harness adapters in Group 2 hand it their roots          | `tests/scan_work_count.rs` passes un-ignored; the real home scan finishes inside the read budget (today: 2.7 s, exit 4)                         |

### Group 2. Harness adapters

| #   | Unit                                                                                                                                                                    | Done when                                                                                                                                    |
| --- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| 2.1 | `HarnessAdapter` trait and the facts table with an Evidence value per row; detection per harness-detection.md (binary path, version, install method, ran-at-least-once) | `skill-studio harnesses` prints each harness with detected, version, install method, and evidence; unknown rows print Unknown, never a guess |
| 2.2 | Claude Code: per-skill links, `skillOverrides` in settings.json, plugin CLI, transcript reader                                                                          | fixture home tests for detect, roots, switch read and write, usage; the two Unknown link facts resolved by a real run                        |
| 2.3 | Codex: `config.toml` rows, `agents/openai.yaml`, rollout reader                                                                                                         | same shape of tests; a park updates the disable row                                                                                          |
| 2.4 | OpenCode: `opencode.json` and a decision on `.jsonc`, SQLite read-only reader, env overrides                                                                            | same; `XDG_DATA_HOME` honoured                                                                                                               |
| 2.5 | pi: settings exclusions once confirmed, session reader                                                                                                                  | same                                                                                                                                         |
| 2.6 | Shared root: `~/.agents/skills`, the lock file, the dotagents ledger, the registry, all read and written under the lease with a version field                           | registry writes are read-modify-write under the lease; a concurrent write test                                                               |
| 2.7 | Fixture homes: one per harness, captured from a real machine, secrets redacted, checked in under `fixtures/homes/`                                                      | every harness test reads from them; no synthetic transcript lines                                                                            |

### Group 3. Vertical slices, in user-value order

Each slice ships through the desktop, the CLI, and the MCP server from one `ops` function, and marks its story done in user-stories.md.

| #   | Unit                                                                                                                              | Story    | Done when                                                                                           |
| --- | --------------------------------------------------------------------------------------------------------------------------------- | -------- | --------------------------------------------------------------------------------------------------- |
| 3.1 | Tracer: park and unpark on the new stack                                                                                          | U7       | journal, lease, swap, link, events; the old `skill_park.rs` path deleted                            |
| 3.2 | First run: detect harnesses, offer to track project folders                                                                       | U1       | a clean macOS account reaches the list with the right harnesses and no blank screen                 |
| 3.3 | Inventory and Activity: keep the last good snapshot; never publish an empty list on a slow scan                                   | U2, U3   | scan over budget shows the last list and a "still scanning" note                                    |
| 3.4 | Outdated per install method: skills.sh by lock hash against tree hash; plugin by cache version; dotagents by ledger; manual never | U4       | the list shows "update available" only when the hash differs; one network call per source per check |
| 3.5 | Install by preferred method and harness, through `npx skills`, trust prompt on every path                                         | U5       | CLI trace parity for add; the store install shows the trust prompt                                  |
| 3.6 | Update one and update all                                                                                                         | U6       | CLI trace parity for update; success only after the last step                                       |
| 3.7 | Fix: frontmatter repair, link repair, and conflicts handed to the user's editor                                                   | U8, U9   | two copies that differ open side by side in the chosen editor; the app never merges                 |
| 3.8 | Turn off per harness with the native switch, and undo for every write                                                             | U10, U11 | each harness file's switch is used; `skill-studio undo` reverses the last journal entry             |
| 3.9 | Remove                                                                                                                            | U12      | CLI trace parity for remove; quarantine with a retention cap                                        |

### Group 4. Surfaces and cleanup

| #   | Unit                                                                                                                 | Done when                                                                                          |
| --- | -------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| 4.1 | Delete the 11 IPC commands with no caller; delete each desktop write path as its slice lands                         | `#[tauri::command]` count equals the frontend wrapper count                                        |
| 4.2 | CLI and MCP parity: a subcommand and a tool for every write                                                          | the three lists match; the parity test compares the disk after each operation through each surface |
| 4.3 | Defer agent runs and packs: out of the navigation, the `ops` functions and tests kept, nothing behind a runtime flag | no feature flag in the code; the deferred module compiles and its tests run                        |

### Group 5. Quality gates

| #   | Unit                                                                                                                                            | Done when                                        |
| --- | ----------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------ |
| 5.1 | Lint set from section 7 in place; `npm run check` runs all of it                                                                                | CI green with the new set                        |
| 5.2 | Mutation testing: `cargo mutants` on core and host in CI for changed modules, weekly for all; tests that catch no mutant are deleted            | mutation score per module in the CI summary      |
| 5.3 | Doctor: `skill-studio doctor` checks every invariant in lifecycle-states.md and prints zero on a healthy machine; runs at startup and on demand | the six checks in definition-of-done.md all pass |
| 5.4 | CLI trace parity: nine recorded `npx skills` runs replayed against the core                                                                     | traces checked in; test green                    |

## 4. How each unit is baselined

Every issue opens with a Measure table, filled before the work starts. These are hand-run measurements for the person doing the work, not a CI gate: CI only checks deterministic invariants (see performance.md's "How performance is checked"), never wall-clock time.

| Metric                                   | Today                                 | Target       | How measured                                                       |
| ---------------------------------------- | ------------------------------------- | ------------ | ------------------------------------------------------------------ |
| example: full scan, 400-skill estate     | 166.98 ms on disk, 1.9347 s in memory | under 100 ms | `cargo bench -p skill-studio-core --features testing --bench scan` |
| example: writes with a journal entry     | 8 of 46                               | 46 of 46     | count in docs/action-map/README.md                                 |
| example: commands blocking the UI thread | to fill from performance.md           | 0            | performance.md table                                               |

Sources of numbers: the bench file, the timing log, the action map counts, the test counts by kind, and the mutation score. A unit that cannot name a number gets a yes-or-no check instead, never "improved".

## 5. What, how, and how performant

Three questions, three tools.

- **What happened.** The journal is the record. `skill-studio events` lists it, with the plan, the steps, and the outcome. The desktop Activity view reads the same store. Every write names its journal id in its success message.
- **How it happened.** Tracing spans in the core, one per `ops` call and one per step, with the root, the harness, and the primitive. The host writes them to a rolling log. The CLI prints them with `--trace`.
- **How fast.** `elapsed_ms` on every span, the bench file with its CI threshold, and the frontend overlay. Budgets from definition-of-done.md: scan under 100 ms, any local mutation under 50 ms, UI event within one frame of the journal commit.

## 6. Test strategy

The cheapest test that can fail. Test through the CLI, not the window, because the three surfaces share the core.

| Layer            | Kind                                                                                       | Count             | Speed |
| ---------------- | ------------------------------------------------------------------------------------------ | ----------------- | ----- |
| Core primitives  | proptest model tests over an in-memory fs; crash tests that stop after step k              | many              | ms    |
| Core ops         | plan tests: given a snapshot, the plan is this exact list of steps                         | many              | ms    |
| Harness adapters | fixture home tests, files captured from real machines                                      | one file per fact | ms    |
| Host             | real fs and a real second process for the lease; SQLite read-only proof                    | few               | s     |
| Slices           | CLI end to end on a temp home, one per story; CLI trace parity                             | one per story     | s     |
| Desktop          | Vitest for the store reducer; one smoke that boots the window and loads the fixture estate | few               | s     |

No tautological tests. A test is tautological, and gets deleted, when any of these is true:

1. It asserts that a function returned what a mock was told to return.
2. The expected value is computed by the same code as the actual value.
3. It is a snapshot with no reviewed fixture behind it.
4. No single-line change in the code under test can make it fail. `cargo mutants` finds these; a test that catches no mutant after a full run is deleted.

Each test is named `<op>_<condition>_<outcome>` and its doc comment names the defect it guards, for example "guards: crash between the rename and the registry write". The pull request template asks: which bug does each new test catch?

Fixtures carry real failures. When a defect is found in the running app, the state that produced it goes into a fixture home before the fix lands.

## 7. Lint set

TypeScript, today: oxlint with `--deny-warnings`, the local anti-slop plugin (16 rules), react rules, `no-explicit-any`, oxfmt, react-doctor. Add:

- `knip` for dead exports, dead files, and unused dependencies.
- oxlint `no-restricted-imports` to enforce the layers: components import the store, the store imports `skill-api.ts`, nothing else imports `@tauri-apps/api`.
- Vitest as the test runner, wired into `npm run check`.

Rust, today: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, only for the desktop crate. Add a `[workspace.lints]` block so every crate gets the same set:

- `clippy::all` and `clippy::pedantic` at warn, with `module_name_repetitions` and `missing_errors_doc` allowed.
- Deny in the core crate: `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `dbg_macro`, `print_stdout`, `print_stderr`.
- `#![forbid(unsafe_code)]` in every crate.
- `cargo clippy --workspace --all-targets`, so tests are linted too.
- `cargo machete` for unused dependencies, `cargo deny` for licences and advisories, `cargo mutants` for tautology.
- Two architecture tests: the core crate's `Cargo.toml` has no tauri, rusqlite, tokio, or reqwest; `std::fs` and `std::process` are absent from the core crate's source.

`npm run check` runs all of it. A new rule lands with the fixes for every existing hit, never with allow-lists.

## 8. Keeping the code simple

- One write path per operation, in `ops`. The old path is deleted in the same pull request.
- The core's public surface is `ops.rs` and `dto`. Everything else is `pub(crate)`.
- A module is at most 500 lines. A test enforces it.
- No runtime feature flags. Deferred features are modules that compile and test but are not registered with a surface.
- One reader and one writer per file on disk: the registry, the lock file, the ledger, each harness config.
- No bare-role file names; the domain prefix rule in CLAUDE.md stays.
- The action map is the map of the code. A pull request that adds, removes, or reorders a write edits the area file in the same change.

## 9. Definition of done, by unit kind

**Primitive.** Model test and crash test pass. No `std::fs` in the core. Clippy clean under the new set. Mutation score at or above 80% for the module. system-overview.md names the primitive as present with a `file:line`.

**Harness adapter.** Facts table has an Evidence value per row and no guess. Fixture home checked in. Tests for detect, roots, switch read and write, and usage pass against the fixture. The harness file in `harnesses/` matches the code.

**Vertical slice.** Works through the desktop, the CLI, and the MCP server from one `ops` function. Every write in it has a journal entry, a lease, and a success message only after the last step. The end-to-end CLI test passes. The bench stays within budget. The story in user-stories.md says done and names the check. The old desktop path is deleted.

**Baseline.** The number is in the bench file or the log, with the command that produced it.

**The epic.** All six checks in definition-of-done.md pass, the headline numbers in the README read 46 of 46 journaled, 0 without a direct test, 0 without a caller, and the plain-words page at https://claude.ai/artifact/V7M7rcYQUzAYpjzg3z8ncG shows no "mess" pill.
