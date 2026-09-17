# Skill packs

Deferred on 2026-09-17: packs are out of the first release scope (plan.md, unit 4.3). The `ops` functions and tests stay; the `skill-packs` runtime flag goes away and the module is simply not registered with a surface.

A pack bundles named skills into one git repo under `~/.agents/packs/<name>`, so a user can create, update, publish, and re-import a set of skills together.
The feature sits behind the `skill-packs` flag, off by default.

Commands: list_skill_packs, create_skill_pack, update_skill_pack, publish_skill_pack, delete_skill_pack, import_skill_pack, confirm_skill_pack_trust, abandon_pack_import_trust, the pack import staging reconcile loop, the `skill-packs` flag in feature-flags.ts.
UI entry points: PacksView (row, Update, Publish, Delete), PackNamePrompt (Create), AddSkillSheet (Import pack, the trust footer).

## Current state

A pack row in the registry carries the pack's directory, its member skill names, and, once published, an `owner/repo` pointer.
Every write command in this document mutates either the pack's own git repo, that registry row, or both, and none of them share a common transaction boundary between the two.

The `skill-packs` flag defaults to off (feature-flags.ts:14); a user flips it per machine with `localStorage.setItem("feature:skill-packs", "on")`.
Every command below is dead in a default install.
`list_skill_packs` (skill_pack.rs:1738) reads the `packs` map from `~/.agents/skill-studio.json`; no lock, no write.

`create_skill_pack` (skill_pack.rs:1608 → 682) builds the pack tree under `~/.agents/packs/<name>`, runs `git init`, `git add -A`, `git commit`, then writes the registry entry, in that order (:703-715).
It takes ForkMutationLock and does not journal.
A git failure after the tree is built leaves an unregistered orphan directory on disk.

`update_skill_pack` (:1624 → 724) rebuilds the tree in place, then commits only when `git status --porcelain` is non-empty.
A late git failure leaves a partially rewritten tree with no rollback.

`publish_skill_pack` (:1639 → 761) needs `gh` installed and a confirm dialog accepted, then either `git push` or `gh repo create --source <dir> --remote origin --push`.
Only on first publish does the registry gain the owner/repo field.
A registry write failure after a successful `gh repo create` leaves the new GitHub repo unrecorded.

`delete_skill_pack` (:1660 → 810) removes the pack directory, then removes and rewrites the registry entry, and never touches GitHub.
A registry write failure after the directory removal leaves a dangling entry.

`import_skill_pack` (:1670 → 1397) validates the request, resolves and verifies the source commit, or fingerprints a local snapshot, and validates the agents.toml rows.
When every source identity is already trusted, it runs `npx -y @sentry/dotagents add <source>` plus one add per row.
When an identity is untrusted, it stages a local snapshot and returns a PendingPackTrust token with a TTL instead of installing.
`cleanup_prepared_pack_import` removes the staged snapshot on every early return, but a CLI call that starts and fails mid-way leaves its partial writes on disk.
ForkMutationLock covers only the execute branch; the trust token is behind its own mutex.

`confirm_skill_pack_trust` (:1699 → 1549) is a single-use follow-up: it checks the token, matches the stored request, revalidates for drift, records every identity as trusted, then runs the same CLI install as import_skill_pack.
`abandon_pack_import_trust` (:1727 → 1471) removes the token and its staged snapshot; nothing is installed.

The pack import staging reconcile loop (lib.rs:120) runs synchronously before the background loops start and cleans any pending staging left from a crash; a failure is printed and never blocks startup.
None of these seven write commands journal an event or take a backup before writing.
ForkMutationLock serializes them against each other and against fork operations, but a mid-sequence failure can still leave disk state ahead of the registry, or a registry entry ahead of disk state.

Tests: create_skill_pack has eight named tests (skill_pack.rs:2030-3681); update_skill_pack has three; publish_skill_pack has four; delete_skill_pack has two.
import_skill_pack, confirm_skill_pack_trust, and abandon_pack_import_trust have tests in the same file, but the source map does not name them.

In the UI, Create, Update, Publish, and Delete each show a busy label and end in a toast on both outcomes (PacksView.tsx:146-154, PackNamePrompt.tsx:90-92), except Delete, which returns to the list with no success toast.
Delete's confirm dialog is a Tauri `ask()` that names the risk plainly: "Delete {name} locally? Its GitHub repo (if any) is left untouched."
Publish always targets `public` visibility; there is no private option surfaced in PacksView, even though the backend accepts a visibility argument.
The Add Skill sheet's pack import path shares the operation trust footer with regular skill installs: "Trust repository and retry" calls confirm_skill_pack_trust, and closing the footer calls abandon_pack_import_trust.
A second feature flag, `skill-assistant`, gates the unrelated assistant drawer in the same file; the two flags are independent, so a pack import can trigger the trust footer even when the assistant drawer is off.

## Changes in the Claude stack (#73 to #134)

No open PR in this stack's range touches packs.
`gh pr list --state open --limit 100 --search packs` returns no results.

## Changes in the Codex stack (#79 to #141)

No open PR in this stack's range touches packs either.
The pack commands and PacksView are untouched by both stacks' current PR queues.

## Desired state

Every pack write records a journal event with a backup and an inverse before it touches disk.
`create_skill_pack` journals the tree build so a git failure can roll the directory back; `update_skill_pack` backs up the pre-update tree; `publish_skill_pack` and `delete_skill_pack` do the same for the registry entry they touch.
`create_skill_pack`, `update_skill_pack`, `publish_skill_pack`, and `delete_skill_pack` become one write path in skill-studio-core, with the Tauri command as a thin adapter that only maps the skill-packs flag and the confirm dialog to core calls.
ForkMutationLock becomes a per-pack lease so two packs can be created or updated at once; the registry read-modify-write for the packs map runs under that same lease, not a separate unlocked step.
`create_skill_pack`'s three writes (tree, git commit, registry) become one transaction, or the registry write gets a compensating step that removes the orphan tree on failure.
The same applies to `delete_skill_pack`'s directory removal followed by the registry removal, and to `publish_skill_pack`'s `gh repo create` followed by the registry field write.
Every command's success toast fires only after its last write; Delete gains the success toast it is missing today, and any partial result is named to the user instead of just failing silently on the next refresh.
`import_skill_pack` and `confirm_skill_pack_trust` keep the trust prompt on every untrusted source, including a local path source, not only GitHub ones.
Backups for pack writes get the same retention limit as the rest of `<app_data>/backups/`, which today has none.
Every command above gets a direct test and a crash-window test, matching the pattern already used for create_skill_pack's named tests.
The pack import staging directory is either cleaned up on every path, including a crash between staging and CLI install, or listed for the user in Settings, the same way a leaked scratch dir should be.
Publish gains a real visibility choice in the UI to match the backend's existing `public`/`private` parameter, since a hardcoded public default is a silent privacy decision the user never makes.

## Gaps

- create_skill_pack: no journal, no backup, no inverse; a git failure after tree build leaves an orphan directory.
- update_skill_pack: no journal; a late git failure leaves a half-rewritten tree with no rollback.
- publish_skill_pack: a registry write failure after `gh repo create` leaves the new repo unrecorded, with no compensating step or user-facing warning.
- delete_skill_pack: no success toast; a registry write failure after directory removal leaves a dangling entry.
- import_skill_pack / confirm_skill_pack_trust: CLI partial writes on a mid-install failure stay on disk with no cleanup or journal.
- All four packs.rs-native writes: ForkMutationLock is one global lock, not a per-pack lease, so unrelated pack operations serialize against each other.
- All four: logic lives directly in the Tauri command layer, not in a shared core crate, so the CLI and MCP server cannot reuse it.
- import_skill_pack / confirm_skill_pack_trust: named tests exist but are not identified in the source map, so coverage cannot be confirmed without reading the file directly.
- publish_skill_pack: PacksView hardcodes `public` visibility, so the backend's `private` option has no UI path.
- No open PR in either stack currently plans to address any of the above.
