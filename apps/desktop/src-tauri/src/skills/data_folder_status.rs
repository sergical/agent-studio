// ============================================================================
// Skills Module - data_folder_status
// Unit 6.3: the app data folder's `schema_version` marker, checked once at
// startup before anything else opens the folder. A folder older than this
// build is migrated forward in place; a folder newer than this build blocks
// the data layer and hands the frontend one message naming both versions.
// ============================================================================

use std::path::Path;
use std::sync::Mutex;

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
            eprintln!(
                "[data_folder_version] failed to read {}: {error}",
                app_data.display()
            );
            return None;
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
            eprintln!("[data_folder_version] migration failed: {error}");
        }
    }

    None
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
