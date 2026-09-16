# Browse installation acceptance

Browse uses the existing background Add operation for progress and cancellation.
It retains terminal feedback, refreshes installed skills after terminal outcomes,
and shows warnings when installation finishes with cleanup problems. Project
installation runs the provider in the selected directory. Global installation
ignores an incidental project selection.

The desktop owns subscription and cancellation through a controller. It registers
before starting, ignores foreign/stale events, retains the listener when catch-up
reads fail, and cancels a late-starting operation after its view unmounts.
Cancellation acknowledgement does not imply that the provider has stopped.

## Focused checks

- 23 tests across `skill-store-install-operation.test.ts`,
  `useAddSkillOperation.test.ts`, and `skill-add-operation-policy.test.ts` passed.
  Controller cases cover catch-up failure, pending registration/start disposal,
  stale callbacks, cancellation acknowledgement, terminal events, and refusal.
- 31 Rust tests selected by `skills_sh_` and seven install-plan tests passed.
  Project single/batch requests pass cwd, global requests leave it unset, and
  empty project paths are rejected before invoking the provider.
- Frontend typecheck, affected-file lint/format, Rust formatting, and strict
  all-target desktop Clippy passed.
- Fresh simplification removed one unnecessary `async`; relevant checks passed.

Resources: two Cargo workers, one Rust test thread, one Vitest worker, 3 GiB Node
heap limit. Rust test build: 34.87 s wall, maximum process RSS 1,209,237,504 bytes,
zero swaps. Frontend build: 3.44 s wall, RSS 1,002,700,800 bytes, zero swaps.
These are build measurements, not native runtime or process-tree peaks.

## Native acceptance

Passed in the actual packaged Tauri app with isolated home/project and a finite
loopback catalog/provider fixture:

- Global success: Added remains visible; closing shows Installed; count becomes 1.
- Cancellation: Cancel ends in Cancelled; no skill directory is created.
- Refusal: exact provider error remains in the modal; no installed entry is added.
- Partial failure: the modal warns that files changed and installation may be
  partial; closing shows the discovered files and Installed count becomes 2.
- Project success: native folder picker selects the fixture project; provider cwd
  matches it, files appear there, no global copy appears, and count becomes 3.
  The refusal fixture is configured to succeed for project scope for this case.
- Active modal Close: requests cancellation and retains the Cancelled result.
  Pending subscription/start unmount races are covered by controller tests;
  no native timing-injection claim is made.

Binary SHA-256: `442f57fbfee93f9a3210942919a01fd8750bdb460e7243b70baf70e46e69336b`.
Native build: 91.06 s wall, RSS 1,569,980,416 bytes, zero swaps. First fixture launch
failed because the sandbox requires `localhost`, not an IP literal; only the test
sandbox was corrected. The accepted app uses the same built binary. App/provider/
catalog processes have exited. Disposable bundle, home/project and logs are removed
once conclusions are recorded; compact catalog/provider reproduction scripts remain
under `/tmp/skill-studio-delivery/store-install`.
Fixture provider results prove UI/IPC/process behavior, not compatibility with a
new upstream provider release. No installation rollback or Undo is promised by
this change. CLI/MCP/cloud-agent delivery is outside this batch.

## Review

Independent review found no actionable findings. There is no mounted React test
for the complete terminal effect; native checks above exercise retained feedback,
list refresh, success and partial failure. No merge or deployment is included.
