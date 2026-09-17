# skill-studio-host

Real-world adapters for `skill-studio-core`. The core crate defines port
traits (`ScopeFs`, `Clock`, `IdSource`, `LeaseProvider`, `HistoryOpener`,
`EventSink`, ...) and never touches the operating system itself; this crate
gives each of them a real implementation.

## What is here

- `RealFs` — `ScopeFs` over `std::fs`. Writes go through a temp file in the
  same directory and a rename, preserving the file's mode. `read_capped`
  rejects a file over the byte cap instead of truncating it.
- `SystemClock` — `Clock` over `chrono::Utc::now` and a process-local
  `Instant` for the monotonic side.
- `UlidIds` — `IdSource` over a monotonic `ulid::Generator`, so two ids
  handed out back to back still sort in generation order.
- `FileLease` — `LeaseProvider` over one advisory-locked file per canonical
  root, using `std::fs::File::{lock, lock_shared, try_lock, try_lock_shared}`
  (stable since Rust 1.89). Locks release when the returned handle drops.
- `NoopSink` / `StderrSink` — `EventSink` that discards notices, or prints
  one JSON line per notice to stderr.
- `NoHistoryOpener` — `HistoryOpener` with no store behind it. Fine for
  read-only operations like a scan; a mutation asking for read-write access
  fails with `ErrorCode::Unsupported`.
- `SqliteHistoryOpener` / `SqliteHistoryStore` — the real event log. SQLite
  via `rusqlite` (`bundled`), schema and pragmas byte-compatible with the
  desktop's `events.sqlite3` (`apps/desktop/src-tauri/src/skills/event_store.rs`):
  same `events`, `materialized_roots`, `materialized_disabled` tables and
  indexes, WAL mode, `foreign_keys` off. `HistoryAccess::ReadIfExists` never
  creates the database file; `HistoryAccess::ReadWrite` creates the parent
  directory and the schema on first use. Byte backups displaced by a
  mutation live under `<db_path's directory>/backups/<event-id>/`. An
  unknown `kind` string loads and renders as not restorable rather than
  failing (spec decision D11); it never blocks `list`/`get`.
- `default_ports` — wires the adapters above into one `Ports` for the common
  case, leaving the process spawner, project discovery, and `PATH` lookup
  unset, with `NoHistoryOpener` for history.
- `default_ports_with_history` — `default_ports`, with `SqliteHistoryOpener`
  bound to a given `db_path` in place of `NoHistoryOpener`.

## What is not here

No policy, no Tauri types, no async runtime. This crate depends only on
`std`, `skill-studio-core`, `ulid`, `chrono`, `serde`, `serde_json`, and
`rusqlite`.
