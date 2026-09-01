# Spec: Event store, restore, and per-harness materialize

Status: agreed 2026-08-31, revised same day after adversarial review
(crash-recovery protocol, recoverable forced restore, persistent
materialized-root reconciliation). Implements the "every destructive
change is recorded and recoverable" decision plus per-harness disable for
shared-folder skills.

## Goals

1. Every mutating operation Skill Studio performs is recorded in an
   append-only event log.
2. Any bytes a mutation destroys or overwrites (moved-aside folders,
   removed symlinks, rewritten SKILL.md frontmatter) are backed up before
   the mutation runs and can be restored from the UI.
3. A shared-folder skill can be disabled per harness by removing just that
   harness's link (materialize), and re-enabled by restoring it.
4. A crash at any point leaves the system in a state the app can
   recognize and report at next startup; no mutation path deletes the
   only copy of anything before its replacement exists.

Non-goals: no shadow git repo (opencode-style) — our events are coarse,
occasional folder/symlink operations, not continuous file diffing. No sync,
no multi-machine history.

## Storage

- SQLite via `rusqlite` (feature `bundled`, WAL mode).
- DB path: `<app_data_dir>/events.sqlite3` (Tauri `app_data_dir`).
  NOT in `~/.agents` — the app's only owned file there stays
  `skill-studio.json`.
- Backups: `<app_data_dir>/backups/<event-id>/` containing the preserved
  bytes plus `manifest.json` mapping original absolute paths to relative
  paths inside the backup dir. The manifest also records a content
  fingerprint (SHA-256 over a stable walk: relative path, symlink target
  or file bytes) per top-level backed-up path.

### Schema

```sql
CREATE TABLE events (
  id          TEXT PRIMARY KEY,       -- ULID, sortable by time
  ts          TEXT NOT NULL,          -- ISO 8601 UTC
  kind        TEXT NOT NULL,
  skill       TEXT NOT NULL,
  harness     TEXT,                   -- AgentId label when harness-scoped
  scope       TEXT,                   -- 'global' | 'project'
  project_path TEXT,
  payload     TEXT NOT NULL,          -- JSON: kind-specific forward data
  inverse     TEXT,                   -- JSON: how to undo; NULL = not restorable
  backup_dir  TEXT,                   -- relative backup dir when bytes were preserved
  status      TEXT NOT NULL,          -- 'pending' | 'done' | 'failed' | 'interrupted'
  reverted_by TEXT REFERENCES events(id)
);
CREATE INDEX idx_events_skill ON events(skill, ts DESC);

-- Desired state for harness roots Skill Studio has converted to
-- per-skill links (see Materialize). Source of truth for reconciliation;
-- the filesystem is a projection of it.
CREATE TABLE materialized_roots (
  root_path   TEXT PRIMARY KEY,       -- absolute path of the harness skills dir
  harness     TEXT NOT NULL,
  shared_root TEXT NOT NULL,          -- absolute path of the shared root it mirrors
  created_by  TEXT REFERENCES events(id)
);
CREATE TABLE materialized_disabled (
  root_path   TEXT NOT NULL REFERENCES materialized_roots(root_path),
  skill       TEXT NOT NULL,          -- skill dir name deliberately unlinked
  PRIMARY KEY (root_path, skill)
);
```

### Event kinds (v1)

`install`, `remove`, `update`, `park`, `unpark`, `harness_disable`,
`harness_enable`, `move_aside_disable`, `move_aside_restore`,
`invocation_change`, `fork`, `unlink_harness` (materialize-disable),
`relink_harness`, `explode_shared_dir`, `restore` (the undo of another
event, `payload.target_event`).

## Write path (Rust)

New module `skills/event_store.rs`:

```rust
pub fn allocate_id() -> EventId                              // ULID, no DB access
pub fn backup_paths(app_data, id, paths: &[PathBuf]) -> Result<BackupManifest>
pub fn record(conn, id, draft: EventDraft) -> Result<()>     // status='pending'
pub fn finish(conn, id, status: Done|Failed) -> Result<()>
pub fn list(conn, filter) -> Result<Vec<EventRow>>
pub fn restore(conn, app_data, id, force: bool) -> Result<EventId>
pub fn reconcile_at_startup(conn, app_data) -> Result<Vec<EventRow>> // pending -> interrupted
```

Every mutating command wraps its work in these durable phases, in order:

1. `allocate_id()` — the operation ID exists before anything touches disk,
   so the backup dir and the row always agree.
2. `backup_paths(id, ...)` for anything about to be destroyed or
   rewritten. The manifest (with fingerprints) is written and fsynced
   before phase 3; a backup dir with no event row is garbage-collectable.
3. `record(id, pending)` — the row carries pre-mutation fingerprints in
   `inverse`, so recovery can tell which side of the mutation the
   filesystem is on.
4. Perform the mutation. Filesystem changes are staged so no step deletes
   the only copy of anything: replacements are created (and verified)
   before their source is removed; directory moves use `rename` (atomic
   on the same filesystem); multi-step sequences order create-then-delete.
5. `finish(done)` on success; `finish(failed)` on error (backup kept for
   manual recovery, row visible as failed).

### Crash recovery

- `reconcile_at_startup` runs before the first scan: every `pending` row
  is flipped to `interrupted` (a crash is the only way a pending row
  survives a restart — commands are synchronous within one process).
- An `interrupted` row keeps its backup and renders in History with a
  warning and a Restore button; restore uses the recorded fingerprints to
  determine whether the mutation completed, partially applied, or never
  ran, and either re-applies the inverse or reports that the filesystem
  already matches the pre-event state.
- Duplicate/concurrent restores are prevented by a transactional claim:
  `UPDATE events SET reverted_by = ?new WHERE id = ?target AND
reverted_by IS NULL`; zero rows updated means another restore already
  claimed it, and the new restore aborts before touching the filesystem.
  (The in-process `ForkMutationLock` already serializes commands; the
  claim guards against crashed half-restores and future callers.)

### Restore

`restore(id, force)` dispatches on `kind` and applies the `inverse` op
(recreate symlink, move backup bytes back, rewrite frontmatter from
backup). Ordering per restore:

1. Claim the target event (compare-and-set above).
2. `allocate_id()` for the restore event and **back up whatever the
   restore is about to displace** — a restore is itself a mutation and
   follows the same five phases. This gives every restore a valid inverse,
   so restore-of-restore is legal and `force` never destroys the only
   copy of anything: the displaced (drifted) state lands in the restore
   event's own backup and can be restored back from History.
3. Drift guard: compare the current content fingerprint of the target
   paths against the fingerprint recorded in the original event's
   `inverse`. On mismatch, refuse with a message naming the drifted path
   — unless `force`, which proceeds because step 2 already preserved the
   drifted bytes.
4. Apply the inverse (create-before-delete, same staging rules), insert
   the `restore` event, `finish(done)`.

Restoring an event whose `reverted_by` is already set fails at the claim.

## IPC

- `list_skill_events(limit, skill?) -> Vec<SkillEventDto>`
- `restore_skill_event(event_id, force: bool) -> Result<(), String>`
  Snapshot rebuild is triggered after restore like any mutation.

## UI

- Activity view gains a History section: one row per event (kind icon,
  skill name, harness, relative time, status), a Restore button on
  restorable rows, confirm dialog naming exactly what will be put back.
- Failed and interrupted events render with the error styling and keep
  their backup link (Reveal in Finder). Interrupted rows say the app was
  quit mid-operation.
- A drift-guard refusal surfaces as a dialog offering force-restore, and
  states that the current (drifted) content will itself be backed up and
  restorable.

## Materialize: per-harness disable for shared skills

Case A — harness links to the shared root per skill (own symlink):

- Disable = `unlink_harness`: record symlink target in `inverse`, delete
  the symlink. Enable = `relink_harness` or restore of the event.

Case B — harness symlinks its whole skills dir at `~/.agents/skills`
(per-skill unlink impossible):

- One-time `explode_shared_dir`: replace the whole-dir symlink with a real
  directory containing one per-skill symlink for every skill in the shared
  root (backup = record of the dir-level link). The new directory is built
  complete at a temp path in the same parent and swapped in with `rename`
  after the original link is removed (link removal + rename, ordered so a
  crash leaves either the original link or the finished directory plus a
  recorded event).
- The conversion registers the root in `materialized_roots`. From then on
  Case A applies to that root, and per-skill unlinks are additionally
  recorded in `materialized_disabled` — the persistent record that a
  missing link means "deliberately disabled", not "never existed".
- The Locations card explains this before doing it ("Claude Code links the
  whole folder — Skill Studio will convert it to per-skill links first").

### Reconciliation

The filesystem under a materialized root is a projection of
`materialized_roots` + `materialized_disabled` + the shared root's
current contents. A reconcile pass runs:

- at startup (after `reconcile_at_startup`),
- after every install/remove/update that touches the shared root,
- when the background scan detects drift under a materialized root.

Reconcile creates a per-skill link for every shared-root skill not listed
in `materialized_disabled`, and removes links whose shared-root target no
longer exists. It never touches non-symlink entries (a real folder a user
dropped in stays theirs). This is what keeps a skill installed _after_
materialization visible to the materialized harness.

Un-materializing (restore of `explode_shared_dir`) refuses while
`materialized_disabled` has entries for the root, naming them — the
whole-dir link cannot represent per-skill disables.

### Frontend model

The deployment DTO gains `shared_via_whole_dir_link: bool` (backend
detects a deployment whose skills root is itself a symlink resolving into
the shared root). The Locations card uses it:

- The Enabled switch on such a deployment no longer routes to
  `set_deployment_enabled` (which deterministically refuses). It opens
  the explode/materialize confirmation, then performs the unlink.
- Until the user confirms conversion, the switch is shown in its enabled
  state with the explanation available; it never fires a doomed backend
  call.

## Self-test plan

Rust (`cargo test`, colocated `#[cfg(test)]`, ALWAYS temp dirs / temp HOME
— never the real `~/.agents`, `~/.claude`, `~/.codex`, `~/.cursor`,
`~/.config/opencode`):

1. record → list roundtrip preserves all fields; ULIDs sort by insertion.
2. backup_paths + restore of a removed skill folder: byte-identical tree
   (fingerprint compare).
3. unlink_harness then restore recreates the symlink with the same target.
4. explode_shared_dir converts a whole-dir symlink to per-skill links and
   its restore puts the dir-level symlink back; restore refuses while a
   materialized_disabled entry exists.
5. Restore of an already-reverted event fails at the claim.
   Restore-of-restore succeeds and round-trips the state.
6. A mutation error leaves the event `failed`, never `done`, and keeps
   the backup. A row left `pending` is flipped to `interrupted` by
   `reconcile_at_startup`.
7. Drift guard: restore refuses when the current fingerprint differs from
   the recorded one; `force` proceeds AND the drifted bytes land in the
   restore event's backup, restorable back.
8. Reconcile: after materialization, adding a new skill dir to the shared
   root and reconciling creates its link; a `materialized_disabled` entry
   keeps its link absent; a user-created real dir in the root is left
   untouched.

Frontend: `npm run check` (typecheck + lint + fmt + clippy + cargo test)
is the acceptance gate. Manual smoke in the debug bundle: disable a
harness link from an expanded shared group, see the History row, restore
it, confirm the symlink is back.
