# SKILL.md editing

This area reads and writes the SKILL.md file itself: its body, its frontmatter, and the Codex invocation sidecar that mirrors one frontmatter key.

Commands: `read_installed_skill_md`, `write_installed_skill_md`, `write_installed_skill_md_if_unchanged`, `preview_skill_frontmatter_repair`, `apply_skill_frontmatter_repair`, `set_skill_invocation`, `get_skill_details`.

UI entry points: `SkillMarkdownCard.tsx` editor Save, `SkillProposedEdits.tsx` Apply, `SkillFrontmatterRepairDialog.tsx`, the invocation segmented control in `SkillLocationsCard.tsx`, `SkillCompareDialog.tsx`.

## Current state

`get_skill_details` (`commands.rs:147`) is a read that fetches a skill's file list and SKILL.md body from the local skills.sh proxy.
With a developer key it goes to skills.sh direct instead.
It has no direct test and no write.

`read_installed_skill_md` (`commands.rs:2241`) reads up to 2 MiB from a path.
That path must belong to a deployment in the current snapshot and resolve to a file literally named SKILL.md.
Both reads use no lock and no journal.

`write_installed_skill_md` (`commands.rs:2311`) has **no frontend caller**.
Every UI path uses its compare-and-swap sibling instead.
It checks the path is owned by the snapshot, is named SKILL.md, is at most 2 MiB, and the deployment is not plugin-owned.
Then it does one atomic replace: write a tmp file in the same directory, fsync, rename, fsync the directory (`skill_md_write.rs:133`).
It holds a process-wide `SKILL_MD_WRITE_LOCK` and does not journal.

`write_installed_skill_md_if_unchanged` (`commands.rs:2329`) is what `SkillMarkdownCard.tsx` Save and `SkillProposedEdits.tsx` Apply actually call.
It adds one precondition: the current disk content, read under the same lock, must equal an `expected_content` the caller supplies (`skill_md_write.rs:93`).
On drift it refuses without writing, and the UI calls `onDiskChanged()` so the caller can reload.
It does not journal.
The map notes "no test targets the compare-and-swap command directly" — only the shared policy tests at `commands.rs:395-438` and the atomic-replace primitive are covered.

`preview_skill_frontmatter_repair` (`skill_frontmatter_repair.rs:280`) is a read.
It forces a synchronous snapshot rebuild, revalidates the target deployment, and returns a `FrontmatterRepairPreview` with a proposal id and a content fingerprint.

`apply_skill_frontmatter_repair` (`skill_frontmatter_repair.rs:406`) is the write that preview feeds.
It holds `ForkMutationLock`, and requires the preview's fingerprint and proposal id still match.
It records a `repair_skill_frontmatter` event with a SKILL.md backup, runs the fork machinery for `ForkAndFix` mode, replaces SKILL.md atomically inside the held write transaction, patches the inverse fingerprint, and finishes the event.
Non-fork modes finish the event `Failed` and leave the original file on error.
`ForkAndFix` leaves the intent pending on a crash; a startup reconciler (`skill_frontmatter_repair.rs:355`) finishes or rolls it back by exact fork record and fingerprint.
This is one of the eight journaled writes, with nine tests at `skill_frontmatter_repair.rs:627-925`.

`set_skill_invocation` (`skill_invocation.rs:354`) rewrites only the invocation frontmatter keys, through the write transaction.
For a Codex deployment, it then patches `<skill_dir>/agents/openai.yaml` with its own tmp-plus-rename, deleting the file when it would be empty.
It does **not** journal.
The map flags it partial-state risk directly: "two files written under different locks; a crash between them leaves SKILL.md and the Codex sidecar out of step."
`SKILL_MD_WRITE_LOCK` covers the frontmatter write; nothing locks the sidecar write.
It has a large test suite (`skill_invocation.rs:473-794`) covering the frontmatter rewrite and the sidecar in isolation.
No test targets the two-file sequence's crash window.

On success, `SkillMarkdownCard` refreshes its content, or the Proposed Edits panel closes with "SKILL.md updated"; the invocation control moves.
On failure the user sees "Couldn't save SKILL.md", "SKILL.md changed on disk", or "Couldn't change invocation policy", and the control reverts.
The map's cross-check flags Save SKILL.md — fork, then write, no rollback of the fork if the write fails — as one of three UI paths that chain writes with no rollback.

## Changes in the Claude stack (#73 to #134)

PR #73 is the only Claude-stack change touching this area.
It moved the frontmatter-repair write path into the shared `skill-studio-core` crate, as part of the general core/adapter split, alongside scan, lifecycle, and the event store.
The map notes this core write op is "unused by desktop" at the time of the split.
The desktop app kept its own call path into the same logic, rather than routing through a new core command surface.

No Claude-stack PR changes `write_installed_skill_md_if_unchanged`'s or `set_skill_invocation`'s locking or journaling.
PR #70, covered in enable-and-links.md, is the adjacent invocation-sidecar bug fix, not a change to this area's write path itself.

## Changes in the Codex stack (#79 to #141)

PR #98, "Repair owned Copy YAML with guarded Undo, Redo and recovery", rebuilds frontmatter repair for an owned Copy.
The repair records both a backup and a recoverable intent, updates the registry hash together with the document, and exposes guarded Undo/Redo in Activity.
It preserves resource files, independent same-name copies, and unrelated preferences, and runs off the UI thread with cancellation support.

PR #99, "Save owned Copy documents with Undo, Redo and recovery", does the equivalent for a plain Save.
SKILL.md and its ownership hash update together, interrupted writes recover through the journal, and the editor keeps an unsaved draft when a save is stopped or the file changes externally.

PR #100, "Preserve Copy ownership through invocation changes and history", fixes the same class of bug the Claude stack's #70 fixed for the sidecar, but from the ownership side.
Changing an owned Copy's invocation policy previously wrote the document without updating the ownership hash, which could mark the Copy Unknown on refresh.
The fix uses the shared core's journaled document transaction to update SKILL.md, the Codex sidecar when applicable, and the Copy record together, with Undo/Redo and interrupted-operation recovery.

All three PRs replace this area's "no journal, no rollback of the fork if the write fails" shape with one journaled transaction per write, for the Copy-owned case specifically.

## Desired state

`write_installed_skill_md_if_unchanged`, `apply_skill_frontmatter_repair`, and `set_skill_invocation` should converge on one write path.
Every SKILL.md mutation should record a journal event with a backup and an inverse before it touches disk, the way `apply_skill_frontmatter_repair` already does.

`set_skill_invocation`'s two-file sequence — frontmatter, then Codex sidecar — should become one transaction, or the sidecar write should be a named compensating step recorded in the same event.
That way a crash between them is detected and repaired at startup, instead of silently drifting.

The compare-and-swap write and the invocation write both currently live directly in `skill_md_write.rs` and `skill_invocation.rs`, behind the Tauri command.
They should move into `skill-studio-core` with the desktop command as a thin adapter, matching the shape PR #73 already gave `skill_frontmatter_repair.rs`.
That way a CLI or MCP caller edits SKILL.md through the same guarantees.

`SKILL_MD_WRITE_LOCK` is today a single process-wide lock for every SKILL.md on disk.
It should become a per-scope lease keyed by skill id, so editing skill A does not serialize behind an unrelated edit to skill B.
The invocation command's sidecar write should acquire the same lease as its frontmatter write.

The "Save SKILL.md" UI path — fork, then write, no rollback of the fork on write failure — needs either one transaction across both steps, or a compensating step that reverts the fork when the write fails, per the map's own finding.

`write_installed_skill_md` has no caller.
Per the "no IPC command without a caller" principle, remove it, or document why it must stay registered, for example as a future CLI or MCP direct-write path.

Every command here needs a direct test.
`write_installed_skill_md_if_unchanged`, `read_installed_skill_md`, `get_skill_details`, and `preview_skill_frontmatter_repair` currently have none.
`set_skill_invocation` has extensive unit coverage of each file in isolation, but no test of the two-file crash window.

## Gaps

- `write_installed_skill_md_if_unchanged` does not journal; a crash mid-write has no recorded backup or inverse.
- `set_skill_invocation` does not journal, and its frontmatter write and Codex sidecar write use different locks; a crash between them leaves the two files out of step with no repair.
- No test targets `write_installed_skill_md_if_unchanged` directly.
- No test covers the crash window between `set_skill_invocation`'s frontmatter write and its sidecar write.
- `write_installed_skill_md` has no frontend caller.
- The Save flow in `SkillMarkdownCard.tsx` chains a fork and a write, with no rollback of the fork if the write fails.
- `SKILL_MD_WRITE_LOCK` is one process-wide lock, not a per-scope lease.
- `write_installed_skill_md_if_unchanged` and `set_skill_invocation` still live directly behind the Tauri command, rather than in `skill-studio-core`, unlike `apply_skill_frontmatter_repair`.
- `get_skill_details`, `read_installed_skill_md`, and `preview_skill_frontmatter_repair` have no direct tests.
