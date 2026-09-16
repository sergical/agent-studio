# Provider removal acceptance

September 16, 2026. Review base: `4c93ce4` (PR102).

Removal now runs ownership, filesystem and provider work on a blocking worker.
Both provider paths use the existing controlled process runner. Every terminal
result requests an inventory rebuild, including provider and trial-record cleanup
failures. Desktop removal callers receive readable native errors.

Provider removal has no Undo or rollback guarantee. Dotagents restores staged
dependent links on provider failure; that does not reverse provider file changes.
Copy removal retains its existing core transaction and recovery behavior. Fork
removal semantics are unchanged; broader Fork recovery is a separate delivery.

## Native acceptance

Actual Tauri release app, isolated HOME and project, fake provider executable,
network denied, real home reads denied, writes confined to disposable fixtures.
The fake provider verifies its HOME, selected name and scope before changing files.
This proves desktop routing and feedback, not live upstream provider behavior.

Final binary SHA256:
`e3a17c3225f3a78bdf38ddc45895513ee69fa506c6e4b24ed0e2461b0761c475`.

| Case | Observed result |
| --- | --- |
| Global dotagents success | Selected canonical directory and exact Claude link removed; same-name project retained. |
| Project dotagents success | Native project picker and ownership boundary registration; project cwd and `--project` verified; project directory/link removed and same-name Global directory/link retained. |
| Partial dotagents failure | Changed content refreshed; dependent link restored; native toast displays provider failure and warns that files may have changed. |
| Missing executable and retry | Exact launch error in removal dialog; dependent link restored; retry succeeds after restoring executable without restarting app. |
| Global skills.sh success | Directory and lock entry removed; list count changed from five to four. |
| Silent provider exit | Exit7 becomes `npx exited with code 7`; original skill remains; action returns from busy state. |
| Trial-record cleanup failure | Provider removed skill then corrupted disposable registry; UI explicitly says removal occurred but trial cleanup failed. Inventory removes the skill and displays incomplete ownership warning. Registry restored after test. |
| Slow dotagents removal | Settings rendered while fixture provider PID50546 was independently confirmed live after navigation. |
| Copy wrapper regression | Exact managed Copy and Codex link removed through core; independent same-name Claude directory and content retained; dialog declares no Undo and closes on completion. |

The first partial-failure run exposed an `Unknown error` toast. Normalizing Tauri
string rejection at `removeSkill` fixed all Error-based removal consumers; the
corrected message was verified on the final native binary. One rebuild omitted
`tauri/custom-protocol` and produced a blank test window; the test process was
stopped and the build command corrected. No product change was needed for that.

## Focused checks and review

- `cargo test -q commands::tests::dotagents_ --lib -- --test-threads=1`: 11 passed.
  Covers exact dependent links, same-name independent collisions, project failure
  restoration and scope argv. Test execution0.28s.
- `cargo test -q commands::tests::fork_remove_registry_failure --lib -- --test-threads=1`:
  one passed,0.01s. Existing fork rollback regression retained.
- `cargo check -q`, `cargo clippy -q -- -D warnings`, `cargo fmt --check`: passed.
- `vitest run apps/desktop/src/lib/skill-removal-api.test.ts apps/desktop/src/lib/error-message.test.ts --maxWorkers=1`:
  11 passed,0.277s total. Uses official Tauri mockIPC; checks exact arguments,
  native string/Error rejection and unchanged success/false results.
- Frontend typecheck, changed-file oxlint/oxfmt and diff whitespace checks: passed.
- Fresh simplification and independent review completed; narrow IPC follow-up
  review found no actionable issues.

The process runner is unchanged from PR102. Its seven passing checks, including
timeout, descendant reaping and bounded output, remain applicable; no full-duration
native timeout was repeated. Ownership refusal and staging boundaries retain their
existing focused test coverage. Native observations above do not replace those tests.

## Resource use and reproduction

One heavy local job at a time; two Cargo workers, one test worker, Node heap3072MiB,
shared compatible Rust cache. Preflight:50% memory free,58GiB disk available.
Frontend build3.40s, max process RSS1,034,895,360 bytes; final native build57.98s,
max process RSS1,405,304,832 bytes; zero swaps. Test peak memory was not collected.

Fixture and build scripts are retained temporarily under
`/tmp/skill-studio-delivery/provider-removal`; the build command is
`cargo build --release --features tauri/custom-protocol` after the frontend build.
Raw output and the native bundle are disposable after final evidence is recorded.
CI retains test reports for three days. Merge, deployment and final combined
release acceptance remain separate.
