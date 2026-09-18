//! Shared read-only opener for `OpenCode`'s `SQLite` databases, used by project
//! discovery (`discovery.rs`) and by the `OpenCode` skill-uses reader
//! (`skill_uses/opencode.rs`) so both follow the same "never write next to
//! the user's database" rule.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension};

/// `OpenCode`'s data dir at its default `$XDG_DATA_HOME` location.
pub(crate) const OPENCODE_DATA_ROOT: &str = ".local/share/opencode";

/// `OpenCode`'s database is `opencode.db` on the latest, beta, and prod
/// channels and `opencode-<channel>.db` on every other channel (`next` for
/// the v2 beta, `local` for source builds), so one machine can hold several.
const MAX_OPENCODE_DATABASES: usize = 16;

/// Opening a FIFO blocks, and a symlink can point anywhere, so a database is
/// only opened when it is a regular file.
fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

/// True when `name` is one of `OpenCode`'s own database file names:
/// `opencode.db`, or `opencode-<channel>.db` for a non-default channel.
pub(crate) fn is_opencode_database_name(name: &str) -> bool {
    name == "opencode.db"
        || (name.starts_with("opencode-") && Path::new(name).extension() == Some("db".as_ref()))
}

/// `OpenCode`'s data directory: `$XDG_DATA_HOME/opencode` when
/// `XDG_DATA_HOME` is set and non-empty, else `<home>/.local/share/opencode`.
/// Matches `packages/core/src/global.ts` (`anomalyco/opencode`, commit
/// `83452558f70207ddaeaffce68b36ebac77019fae` on `dev`): `Global.Path.data`
/// joins the `xdg-basedir` package's `xdgData` (which itself falls back to
/// `~/.local/share`) with `"opencode"`.
///
/// The single resolver every `OpenCode` data-dir reader shares: the database
/// lookup (`opencode_databases`), the project-worktree scan
/// (`discovery::opencode_worktrees`), and the skill-use reader's root and
/// disk watch (`skill_uses::opencode_root`, `SOURCES`'s `OPEN_CODE` watch).
pub(crate) fn opencode_data_dir(home: &Path) -> PathBuf {
    match std::env::var_os("XDG_DATA_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("opencode"),
        _ => home.join(OPENCODE_DATA_ROOT),
    }
}

/// Lists the database(s) to read: `OPENCODE_DB` (an absolute path, or a
/// filename joined onto the data dir) names exactly one file outright and
/// skips the directory listing entirely; `:memory:` never exists on disk, so
/// it yields nothing to read. Otherwise every `opencode.db` and
/// `opencode-*.db` under the data dir (honouring `XDG_DATA_HOME`), regular
/// files only, sorted, capped at `MAX_OPENCODE_DATABASES`. Source:
/// `packages/core/src/database/database.ts` `path()`, same commit as
/// [`opencode_data_dir`].
pub fn opencode_databases(home: &Path) -> Vec<PathBuf> {
    let data_dir = opencode_data_dir(home);
    if let Some(over) = std::env::var_os("OPENCODE_DB") {
        if over.is_empty() || over == ":memory:" {
            return Vec::new();
        }
        let path = Path::new(&over);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            data_dir.join(path)
        };
        return if is_regular_file(&path) {
            vec![path]
        } else {
            Vec::new()
        };
    }

    let Ok(entries) = fs::read_dir(&data_dir) else {
        return Vec::new();
    };
    let mut databases: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_opencode_database_name)
        })
        .filter(|path| is_regular_file(path))
        .collect();
    databases.sort();
    databases.truncate(MAX_OPENCODE_DATABASES);
    databases
}

/// Opens `database` read-only without ever creating or changing a sidecar
/// file next to it. `OpenCode` keeps its database in WAL mode, and a plain
/// read-only open creates the `-wal` and `-shm` files when they are missing.
/// So the database is opened as immutable unless both files already exist,
/// which is the case while `OpenCode` runs and rows that are only in the WAL
/// must still be seen. `None` when the path can't be turned into a file URI
/// or the database can't be opened.
pub(crate) fn open_opencode_database(database: &Path) -> Option<Connection> {
    let live = ["-wal", "-shm"].iter().all(|suffix| {
        let mut sidecar = database.as_os_str().to_owned();
        sidecar.push(suffix);
        Path::new(&sidecar).exists()
    });
    let mut uri = url::Url::from_file_path(database).ok()?;
    uri.set_query(Some(if live {
        "mode=ro"
    } else {
        "mode=ro&immutable=1"
    }));
    let conn = Connection::open_with_flags(
        uri.as_str(),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let _ = conn.busy_timeout(Duration::from_millis(250));
    Some(conn)
}

/// Whether `table` exists in `conn`, checked against `sqlite_master`. A busy
/// or corrupt database is an `Err`, not "no such table": `SQLite` only checks
/// the file header on the first query, so this is where a bad file shows up.
pub(crate) fn table_exists(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |_| Ok(()),
    )
    .optional()
    .map(|found| found.is_some())
}

/// Serializes every test in the crate that touches `XDG_DATA_HOME` or
/// `XDG_CONFIG_HOME`: both are process-global and cargo runs tests on
/// multiple threads, so without this an unrelated test's
/// `opencode_databases(home)`/`opencode_config_dir(home)` call can read the
/// override set by a mutating test and look at the wrong directory. Mirrors
/// `core_scan_parity.rs`'s `home_env_lock` for `HOME`. Shared across
/// `discovery.rs` and `skill_uses.rs` so every `OpenCode` XDG test in the
/// crate serializes on the same lock.
#[cfg(test)]
pub(crate) fn xdg_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flow: a database closed after a WAL write (no live `-wal`/`-shm`
    /// sidecars) is opened read-only by the adapter.
    /// Expectation: the immutable open reads it correctly and creates no
    /// `-wal` or `-shm` sidecar.
    /// Failure here would mean Skill Studio writes next to a database
    /// `OpenCode` itself might still open, corrupting or confusing it.
    #[test]
    fn opencode_sqlite_reader_creates_no_wal_or_shm_sidecar_on_a_closed_database_or_names_the_created_file(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let database = tmp.path().join("opencode.db");
        {
            let conn = Connection::open(&database).unwrap();
            conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t (id INTEGER);")
                .unwrap();
            conn.execute("INSERT INTO t (id) VALUES (1)", []).unwrap();
        }
        // Closing the writer removes the WAL-mode sidecars (checkpointed on
        // close), so the fixture starts with none - the case a plain
        // read-only open would otherwise regenerate.
        let wal = tmp.path().join("opencode.db-wal");
        let shm = tmp.path().join("opencode.db-shm");
        assert!(!wal.exists());
        assert!(!shm.exists());

        let conn = open_opencode_database(&database).expect("open");
        let value: i64 = conn
            .query_row("SELECT id FROM t LIMIT 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, 1);
        drop(conn);

        assert!(!wal.exists());
        assert!(!shm.exists());
    }
}
