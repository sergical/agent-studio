# Spec: Event store, restore, and per-harness materialize

Status: agreed 2026-08-31. Implements the "every destructive change is
recorded and recoverable" decision plus per-harness disable for
shared-folder skills.

## Goals

1. Every mutating operation Skill Studio performs is recorded in an
   append-only event log.
2. Any bytes a mutation destroys or overwrites (moved-aside folders,
   removed symlinks, rewritten SKILL.md frontmatter) are backed up before
   the mutation runs and can be restored from the UI.
3. A shared-folder skill can be disabled per harness by removing just that
   harness's link (materialize), and re-enabled by restoring it.

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
  paths inside the backup dir.

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
  status      TEXT NOT NULL,          -- 'pending' | 'done' | 'failed'
  reverted_by TEXT REFERENCES events(id)
);
CREATE INDEX idx_events_skill ON events(skill, ts DESC);
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
pub fn record(conn, draft: EventDraft) -> Result<EventId>   // status='pending'
pub fn finish(conn, id, status: Done|Failed) -> Result<()>
pub fn backup_paths(app_data, id, paths: &[PathBuf]) -> Result<PathBuf>
pub fn list(conn, filter) -> Result<Vec<EventRow>>
pub fn restore(conn, app_data, id) -> Result<EventId>
```

Every mutating command wraps its work:
1. `backup_paths` for anything about to be destroyed or rewritten.
2. `record` (pending).
3. Perform the mutation.
4. `finish(done)` on success; `finish(failed)` on error (backup kept for
   manual recovery, row visible as failed).

`restore(id)` dispatches on `kind`, applies the `inverse` op (recreate
symlink, move backup bytes back, rewrite frontmatter from backup), inserts
its own `restore` event, and sets `reverted_by` on the original. Restoring
an already-reverted event is an error. Restore refuses when the target
paths have been modified since the event (compare content hash recorded in
`inverse`) unless `force`.

## IPC

- `list_skill_events(limit, skill?) -> Vec<SkillEventDto>`
- `restore_skill_event(event_id, force: bool) -> Result<(), String>`
Snapshot rebuild is triggered after restore like any mutation.

## UI

- Activity view gains a History section: one row per event (kind icon,
  skill name, harness, relative time, status), a Restore button on
  restorable rows, confirm dialog naming exactly what will be put back.
- Failed events render with the error styling and keep their backup link
  (Reveal in Finder).

## Materialize: per-harness disable for shared skills

Case A — harness links to the shared root per skill (own symlink):
- Disable = `unlink_harness`: record symlink target in `inverse`, delete
  the symlink. Enable = `relink_harness` or restore of the event.

Case B — harness symlinks its whole skills dir at `~/.agents/skills`
(per-skill unlink impossible):
- One-time `explode_shared_dir`: replace the whole-dir symlink with a real
  directory containing one per-skill symlink for every skill in the shared
  root (backup = record of the dir-level link). After that, Case A applies.
- The Locations card explains this before doing it ("Claude Code links the
  whole folder — Skill Studio will convert it to per-skill links first").

UI: expanded shared-folder group rows get the same Enabled switch as copy
rows, routed through unlink/relink.

## Self-test plan

Rust (`cargo test`, colocated `#[cfg(test)]`, ALWAYS temp dirs / temp HOME
— never the real `~/.agents`, `~/.claude`, `~/.codex`, `~/.cursor`,
`~/.config/opencode`):

1. record → list roundtrip preserves all fields; ULIDs sort by insertion.
2. backup_paths + restore of a removed skill folder: byte-identical tree
   (hash compare).
3. unlink_harness then restore recreates the symlink with the same target.
4. explode_shared_dir converts a whole-dir symlink to per-skill links and
   its restore puts the dir-level symlink back.
5. restore of an already-reverted event fails; restore-of-restore fails.
6. A mutation error leaves the event `failed`, never `done`, and keeps
   the backup.
7. Drift guard: restore refuses when the current content hash differs from
   `inverse`'s recorded hash, succeeds with `force`.

Frontend: `npm run check` (typecheck + lint + fmt + clippy + cargo test)
is the acceptance gate. Manual smoke in the debug bundle: disable a
harness link from an expanded shared group, see the History row, restore
it, confirm the symlink is back.
