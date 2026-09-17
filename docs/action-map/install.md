# Install

This area adds a skill to disk, through the skills.sh CLI, the dotagents CLI, or a plain copy.

Commands: add_skill (Copy, Dotagents, SkillsSh variants), start_add_skill_operation / start_add_skills_operation, get_add_skill_operation, confirm_add_skill_trust, cancel_add_skill_operation, list_github_skills, get_add_method_defaults.
UI entry points: AddSkillSheet, SkillStoreInstallFlow, Promote to global, Install again, SkillRepairCard reinstall.

## Current state

| Command                    | Journal            | Concurrency                                 | Partial-state risk                                                        |
| -------------------------- | ------------------ | ------------------------------------------- | ------------------------------------------------------------------------- |
| add_skill (SkillsSh)       | no                 | ForkMutationLock                            | CLI partial writes stay on disk                                           |
| add_skill (Dotagents)      | no                 | ForkMutationLock                            | CLI partial writes stay on disk                                           |
| add_skill (Copy)           | no                 | ForkMutationLock                            | crash between the renames and the registry write leaves an unowned folder |
| start_add_skill_operation  | no, in-memory only | operation state mutex plus ForkMutationLock | partial install reported, not repaired                                    |
| cancel_add_skill_operation | no                 | operation state mutex; shared AtomicBool    | worker may already be past a commit                                       |
| confirm_add_skill_trust    | no                 | state mutex released before the fs lock     | none noted beyond the retry chain                                         |
| list_github_skills         | n/a, read only     | TREE_CACHE mutex                            | none                                                                      |
| get_add_method_defaults    | n/a, read only     | none                                        | none                                                                      |

Three code paths install a skill, and the app picks one by method.
`add_skill` (SkillsSh) shells out to `npx skills add` and writes the shared folder and a Claude link (skill_add.rs:1349 → 450).
`add_skill` (Dotagents) checks `require_trusted_dotagents_source` before any write, then shells out to `npx -y @sentry/dotagents add` and diffs directory names before and after to find what the CLI created (skill_add.rs:1349 → 369, skill_trust_policy.rs:81).
`add_skill` (Copy) stages each target beside its destination as `.skill-studio-install-<pid>-<nonce>-<i>`, fetches or copies into the first stage, then renames each stage into place one at a time, and finally writes an ownership record to `~/.agents/skill-studio.json` (skill_add.rs:1349 → 756, :675, :742, :953).
None of the three writes a journal event.

`start_add_skill_operation` and its batch sibling wrap the same three methods in an in-memory state machine with phases Queued, Validating, Fetching, Installing, Reconciling, and a terminal state (skill_add_operation.rs:880/898).
The worker takes `ForkMutationLock` before it writes, and this is the only lock any of the three methods hold — a single global lock, not scoped to the skill or the target root.
An untrusted dotagents source stops the operation at `NeedsTrust` instead of failing.
`confirm_add_skill_trust` records the trust in `~/.agents/skill-studio.json` and starts a fresh operation for the retry (skill_add_operation.rs:1054 → 939, :1007).
`cancel_add_skill_operation` only sets an `AtomicBool` the worker polls, so a cancel can still land after a commit.
The operation is then reported "cancelled after committing" (skill_add_operation.rs:926, :211).

On failure, each method's own cleanup runs — `remove_install_paths` for Copy, none for SkillsSh or Dotagents — but CLI writes already on disk are never rolled back.
`start_add_skills_operation`'s batch worker (the UI's only entry to the same batch logic) applies each entry independently and reports a partial `Vec<AddSkillOutcome>` by design (skill_add.rs:1334 → `add_skills_with_progress`).

`list_github_skills` and `get_add_method_defaults` are reads with no writes.
`list_github_skills` fills an in-process tree cache keyed by repo and ref (github_skill_listing.rs:325).

On the frontend, the Add Skill sheet drives the operation state machine and shows a trust prompt when the operation reaches `needs-trust` (AddSkillSheet.tsx:1174–1180, 1116–1124).
The skills.sh store panel (SkillStoreInstallFlow.tsx:230–246) calls `add_skill` once, with no operation and no trust prompt — a different code path for what looks like the same action.
"Promote to global" (SkillLocationsCard.tsx:109) and "Install again" (skill-location-actions.ts:279) both call `add_skill` directly, also with no trust prompt and no progress state.

Tests cover each method's argv construction, cancellation timing, and partial-batch reconciliation (skill_add.rs, skill_add_operation.rs — see the full map for line numbers).
The pack-import trio has thinner or no direct test coverage.

## Changes in the Claude stack (#73 to #134)

PR #73 moved scan, lifecycle, events, and DTOs out of the Tauri backend into the new `skill-studio-core` crate, with `skill-studio-host` as the real-world adapter, and added a Rust CLI, an MCP server, and a Node TUI on top of the same core.
It introduced the root Cargo workspace and the generated TypeScript types, but it did not change the install write path itself — Copy, Dotagents, and SkillsSh still write the same way after the extraction.
PR #71 fixed a frontend-only bug: adding a project directory from the Browse install drawer while a search was active silently replaced the search results with the popular-skills list, because the mount-only data fetch had picked up `projects` as a dependency.
The fix decouples that fetch from `projects` changes so a project pick during an active search no longer clobbers the result grid.
Neither PR added a journal, changed the lock, or touched the trust prompt gap between the sheet and the store panel.

## Changes in the Codex stack (#79 to #141)

PR #101 made Browse installation route through the existing background Add operation instead of a direct command, so it gains cancel support and real terminal-result feedback, and it fixed Project installs to run the provider inside the selected project directory instead of passing an unsupported `--cwd` flag.
PR #104 fixed Add-by-source so a newly chosen Project registers with the backend before it is saved, so it shows in inventory without a restart.
It also added a shared guard that refuses missing or empty Project paths before any dispatch, root scan, trust check, or provider work.
Both PRs are scoped to the desktop app and carry their own native-fixture verification.
Neither is a wholesale redesign of add_skill's write order, but both close feedback and correctness gaps between the UI action and the write.

## Desired state

Every install write — Copy's staged rename, Dotagents' CLI call, SkillsSh's CLI call — records a journal event with a backup and an inverse before it touches disk, the same way `make_skill_independent_copy` already does.
One write path in `skill-studio-core` implements Copy, Dotagents, and SkillsSh installs.
`add_skill`, `start_add_skill_operation`, and the CLI/MCP adapters call into it rather than each reimplementing the method dispatch.
A per-scope lease — keyed by the target root, not one process-wide `ForkMutationLock` — lets an install into one project proceed while an unrelated global install runs.
The registry read-modify-write in Copy's ownership step and in `confirm_add_skill_trust`'s trust write happens under that same lease, not a separate acquire.
`start_add_skill_operation`'s multi-step chain (fingerprint, method write, reconcile) is one transaction, or `reconcile_affected`'s failure path is a real compensating step instead of a full rebuild used as a catch-all.
The trust prompt appears on every install path: `SkillStoreInstallFlow` and "Promote to global" gain the same `NeedsTrust` handling the Add Skill sheet already has, instead of calling `add_skill` directly with no prompt.
Success feedback for `start_add_skill_operation` only fires after Reconciling completes, and a partial batch names which entries failed in the toast, not just in the operation record.
Backups written for the journal follow a retention limit, so they do not grow without bound.
`add_skill` (each method), `start_add_skill_operation`, and `confirm_add_skill_trust` each get a crash-window test — kill the process mid-rename, mid-CLI-call, or mid-registry-write and assert the reconcile step repairs it, the way `make_independent_copy`'s crash suite already does.
An IPC command with no caller is a maintenance cost with no user behind it; `add_skills` and `is_skill_installed`, the two this area had, were removed in unit 4.1.

## Gaps

- `add_skill` (Copy, Dotagents, SkillsSh) writes to disk with no journal event and no backup, so a crash mid-write is not automatically reconciled at startup.
- Three separate write implementations for Copy, Dotagents, and SkillsSh live in the desktop crate, not in `skill-studio-core`, so the CLI and MCP adapters cannot reuse them.
- `ForkMutationLock` is one process-wide lock; an install into project A blocks an unrelated install into project B.
- `start_add_skill_operation`'s fingerprint-write-reconcile chain has no transaction boundary; `reconcile_affected`'s error path does a full rebuild instead of a targeted compensating step.
- `SkillStoreInstallFlow`'s Install Skill button calls `add_skill` once with no trust prompt, so a dotagents install from the store panel can install from an untrusted source without confirmation.
- "Promote to global" and "Install again" call `add_skill` directly, bypassing the operation state machine, so they get no cancel, no progress, and no trust prompt either.
- `cancel_add_skill_operation` can race a commit and report "cancelled" for work that already landed; there is no compensating undo for that case.
- Backups and event rows have no retention limit (see `<app_data>/backups/<event-id>/` and `events.sqlite3` in the shared-state table), so a busy install history grows without bound.
- `add_skill` (Copy) has direct crash-window tests; `add_skill` (Dotagents) and `add_skill` (SkillsSh) do not, since neither stages before it calls the CLI.
