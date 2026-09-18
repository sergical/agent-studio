// ============================================================================
// Skills Module - data_folder_status
// Unit 6.3: the app data folder's `schema_version` marker, checked once at
// startup before anything else opens the folder. A folder older than this
// build is migrated forward in place; a folder newer than this build blocks
// the data layer and hands the frontend one message naming both versions.
// ============================================================================

use std::path::Path;
use std::sync::Mutex;

use tauri::Manager;

/// Startup verdict on the app data folder's version. `None` until
/// [`check_and_migrate`] runs in `lib.rs`'s `setup`; `Some` names the app
/// to update, and stays set for the life of the process - there is no way
/// to un-block a running process short of restarting it against an
/// updated app.
#[derive(Default)]
pub struct DataFolderStatusState(pub Mutex<Option<String>>);

/// Reads `app_data`'s version marker, migrates it forward in place when it
/// is older than [`skill_studio_host::CURRENT_DATA_VERSION`], and returns
/// the blocking message when it is newer. Called once, before
/// `lib.rs::open_event_store`, so a migration always lands before anything
/// else touches the folder, and a newer folder is never opened at all.
pub fn check_and_migrate(app_data: &Path) -> Option<String> {
    let fs = skill_studio_host::RealDataFolderFs::new();
    let folder_version = match skill_studio_host::read_version(&fs, app_data) {
        Ok(version) => version,
        Err(error) => {
            let message = format!(
                "Could not read the data folder's version marker at {}: {error}. \
                 The app cannot tell whether this folder is safe to open.",
                app_data.display()
            );
            eprintln!("[data_folder_version] {message}");
            return Some(message);
        }
    };

    if let Err(mismatch) =
        skill_studio_host::check_compatible(folder_version, skill_studio_host::CURRENT_DATA_VERSION)
    {
        return Some(skill_studio_host::newer_data_folder_message(mismatch));
    }

    if folder_version < skill_studio_host::CURRENT_DATA_VERSION {
        if let Err(error) = skill_studio_host::migrate(
            &fs,
            app_data,
            folder_version,
            skill_studio_host::CURRENT_DATA_VERSION,
        ) {
            let message = format!(
                "Migrating the data folder at {} failed: {error}. \
                 The folder is unchanged; restart the app to retry.",
                app_data.display()
            );
            eprintln!("[data_folder_version] {message}");
            return Some(message);
        }
    }

    None
}

/// True when no [`DataFolderStatusState`] message is set for `app` - the
/// timing log ([`crate::timing_log::record_command`]) and the update-check
/// writer (`skill_update_check::check_now`) both consult this before
/// touching `app_data_dir`, so neither background job writes into a folder
/// [`check_and_migrate`] refused to open. Absent state (nothing has called
/// [`check_and_migrate`] yet) reads as writable, since setup runs that check
/// before managing any state a command could reach.
pub fn data_folder_writable(app: &tauri::AppHandle) -> bool {
    match app.try_state::<DataFolderStatusState>() {
        None => true,
        Some(state) => state.0.lock().is_ok_and(|guard| guard.is_none()),
    }
}

/// The blocking message [`check_and_migrate`] set at startup, if any - the
/// frontend calls this before rendering its normal chrome (`App.tsx`).
#[tauri::command]
// Tauri commands deserialize their arguments fresh per invocation, so
// `state` can't be borrowed from the caller - it must be owned.
#[allow(clippy::needless_pass_by_value)]
pub fn data_folder_status(state: tauri::State<'_, DataFolderStatusState>) -> Option<String> {
    state.0.lock().ok().and_then(|guard| guard.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A marker file that doesn't hold a decimal version blocks startup with
    /// a message naming the path and the read error, the same as a folder
    /// this build refuses as too new - `lib.rs::run` skips `open_event_store`
    /// whenever `check_and_migrate` returns `Some`, so this also stands in
    /// for "never opens the event store" (N5, review round 1).
    #[test]
    fn corrupt_version_marker_blocks_startup_or_names_the_read_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("schema_version"), b"not-a-number").expect("write marker");

        let message = check_and_migrate(dir.path()).expect("a corrupt marker must block startup");
        assert!(
            message.contains(&dir.path().display().to_string()),
            "message should name the folder path, got: {message}"
        );
        assert!(
            message.contains("version"),
            "message should describe the version-read failure, got: {message}"
        );
    }

    /// A migration that fails partway through blocks startup with a message
    /// naming the path and the write error, rather than silently leaving
    /// `check_and_migrate` reporting the folder as fine (N5, review round
    /// 1). Pre-creating the marker path as a directory makes the marker
    /// write's rename fail, standing in for a real disk-full or permission
    /// error without needing to inject one into `RealFs`.
    #[test]
    fn migration_failure_blocks_startup_or_names_the_write_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("schema_version")).expect("mkdir marker path");

        let message = check_and_migrate(dir.path()).expect("a failed migration must block startup");
        assert!(
            message.contains(&dir.path().display().to_string()),
            "message should name the folder path, got: {message}"
        );
    }
}
