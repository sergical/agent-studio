//! Connection validation for the compatibility event store. Checks use metadata
//! and SQLite's existing handle; no additional database descriptor is opened.
use crate::skill_scope::SkillReadScope;
use rusqlite::{ffi, Connection};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub struct EventConnectionBinding<'connection> {
    connection: &'connection Connection,
    scope: SkillReadScope,
    database: PathBuf,
    device: u64,
    inode: u64,
}

impl<'connection> EventConnectionBinding<'connection> {
    /// Validates an already-open main connection against an authorized existing
    /// state root. This cannot undo effects performed while opening the store.
    pub fn bind(connection: &'connection Connection, root: &Path) -> Result<Self, String> {
        if !root.is_absolute() {
            return Err("Event state root must be absolute".into());
        }
        let scope =
            SkillReadScope::bind(&[root.to_path_buf()]).map_err(|error| error.to_string())?;
        let database = root.join("events.sqlite3");
        let resolved = scope
            .resolved_dir_path(root)
            .map_err(|error| error.to_string())?
            .join("events.sqlite3");
        let connection_path = connection.path().map(Path::new);
        if connection_path != Some(database.as_path())
            && connection_path != Some(resolved.as_path())
        {
            return Err("SQLite connection does not name the bound event database".into());
        }
        let metadata = fs::symlink_metadata(&database).map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err("Event database must be a single-link regular file".into());
        }
        let binding = Self {
            connection,
            scope,
            database,
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        binding.revalidate()?;
        Ok(binding)
    }

    /// Discrete drift checks, not an atomic binding of every SQLite sidecar IO.
    pub fn revalidate(&self) -> Result<(), String> {
        self.scope
            .revalidate_roots()
            .map_err(|error| error.to_string())?;
        let metadata = fs::symlink_metadata(&self.database).map_err(|error| error.to_string())?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.dev() != self.device
            || metadata.ino() != self.inode
        {
            return Err("Event database entry changed".into());
        }
        let mut moved: std::ffi::c_int = 1;
        // SQLite owns the borrowed live handle. HAS_MOVED writes one c_int and
        // does not transfer ownership or open/close an extra database handle.
        let result = unsafe {
            ffi::sqlite3_file_control(
                self.connection.handle(),
                c"main".as_ptr(),
                ffi::SQLITE_FCNTL_HAS_MOVED,
                (&mut moved as *mut std::ffi::c_int).cast(),
            )
        };
        if result != ffi::SQLITE_OK {
            return Err(format!(
                "SQLite connection movement check is unavailable ({result})"
            ));
        }
        if moved != 0 {
            return Err("SQLite connection no longer names the bound database".into());
        }
        self.scope
            .revalidate_roots()
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{process::Command, time::Duration};

    #[test]
    fn validates_live_connection_and_refuses_wrong_root_or_memory() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let connection = Connection::open(root.path().join("events.sqlite3")).unwrap();
        connection
            .execute_batch("CREATE TABLE sample (value INTEGER)")
            .unwrap();
        let binding = EventConnectionBinding::bind(&connection, root.path()).unwrap();
        binding.revalidate().unwrap();
        assert!(EventConnectionBinding::bind(&connection, other.path()).is_err());
        assert!(
            EventConnectionBinding::bind(&Connection::open_in_memory().unwrap(), root.path())
                .is_err()
        );
    }

    #[test]
    fn detects_replaced_database_and_connection_detached_before_binding() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("events.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE sample (value INTEGER)")
            .unwrap();
        let binding = EventConnectionBinding::bind(&connection, root.path()).unwrap();
        fs::rename(&path, root.path().join("old.sqlite3")).unwrap();
        fs::write(&path, []).unwrap();
        assert!(binding.revalidate().is_err());
        assert!(EventConnectionBinding::bind(&connection, root.path()).is_err());
    }

    #[test]
    fn rejects_database_links_and_replaced_parent() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        fs::create_dir(&state).unwrap();
        let path = state.join("events.sqlite3");
        let connection = Connection::open(&path).unwrap();
        let binding = EventConnectionBinding::bind(&connection, &state).unwrap();
        fs::hard_link(&path, state.join("alias")).unwrap();
        assert!(binding.revalidate().is_err());
        fs::remove_file(state.join("alias")).unwrap();
        binding.revalidate().unwrap();
        fs::rename(&state, root.path().join("old-state")).unwrap();
        fs::create_dir(&state).unwrap();
        fs::write(state.join("events.sqlite3"), []).unwrap();
        assert!(binding.revalidate().is_err());
    }

    #[test]
    fn validation_preserves_live_sqlite_transaction_lock() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("events.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE sample (value INTEGER); BEGIN IMMEDIATE; INSERT INTO sample VALUES (1)").unwrap();
        let binding = EventConnectionBinding::bind(&connection, root.path()).unwrap();
        for _ in 0..4 {
            binding.revalidate().unwrap();
        }
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "skill_event_binding::tests::sqlite_lock_child",
                "--ignored",
                "--nocapture",
            ])
            .env("SKILL_STUDIO_EVENT_BINDING_PROBE", &path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        connection.execute_batch("COMMIT").unwrap();
        binding.revalidate().unwrap();
    }

    #[test]
    #[ignore = "child process helper"]
    fn sqlite_lock_child() {
        let path = std::env::var_os("SKILL_STUDIO_EVENT_BINDING_PROBE").unwrap();
        let connection = Connection::open(PathBuf::from(path)).unwrap();
        connection.busy_timeout(Duration::ZERO).unwrap();
        let error = connection.execute_batch("BEGIN IMMEDIATE").unwrap_err();
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy)
        );
    }
}
