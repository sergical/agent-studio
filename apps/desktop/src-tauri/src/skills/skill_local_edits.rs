// ============================================================================
// Skills Module - skill_local_edits
// On-demand check behind the "Update will replace your edits" warning: does
// the installed skills.sh folder an update would replace still match the
// `skillFolderHash` `npx skills` recorded? Run per Update click, never during
// the scan, because it hashes every file in each folder.
//
// Only a Global skills.sh deployment is comparable. A project install's
// `skills-lock.json` records a different `computedHash`, and the dotagents
// `agents.lock` records a commit (`resolved_commit`), not a folder hash, so
// those report `checked: false` and the UI updates without a warning.
// ============================================================================

use std::path::Path;

use skill_studio_core::lock_file::{self, LocalEdits};
use skill_studio_core::ports::ScopeFs;
use tauri::Manager;

use super::skill_dto::{Deployment, LifecycleTarget, LocalEditsDto};
use super::skill_lifecycle;
use super::skill_ownership::LifecycleOwnerKind;
use super::skill_refresh::SkillRefreshState;

const NOT_CHECKED: LocalEditsDto = LocalEditsDto {
    edited: false,
    checked: false,
};

/// The verdict for the deployment an update would replace.
fn deployment_local_edits(
    fs: &dyn ScopeFs,
    home: &Path,
    skill_name: &str,
    deployment: &Deployment,
) -> LocalEditsDto {
    if deployment.owner_kind != LifecycleOwnerKind::SkillsSh || deployment.scope != "global" {
        return NOT_CHECKED;
    }
    let lock = match lock_file::read_lock_file(fs, &lock_file::lock_file_path(home)) {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("[local-edits] {skill_name}: lock file unreadable: {e:?}");
            return NOT_CHECKED;
        }
    };
    match lock_file::local_edits(fs, &lock, skill_name, Path::new(&deployment.path)) {
        LocalEdits::Unedited => LocalEditsDto {
            edited: false,
            checked: true,
        },
        LocalEdits::Edited => LocalEditsDto {
            edited: true,
            checked: true,
        },
        LocalEdits::Unknown => {
            eprintln!("[local-edits] {skill_name}: no comparable lock hash");
            NOT_CHECKED
        }
    }
}

/// One verdict per target, in the order sent. A target that no longer
/// resolves reads as not checked: Update itself reports that refusal.
#[tauri::command]
pub async fn skill_local_edits(
    targets: Vec<LifecycleTarget>,
    app: tauri::AppHandle,
) -> Result<Vec<LocalEditsDto>, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "skill_local_edits", move || {
        let refresh_state = app.state::<SkillRefreshState>();
        let snapshot = skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let fs = skill_studio_host::RealFs::new();
        Ok(targets
            .iter()
            .map(|target| {
                match skill_lifecycle::resolve_lifecycle_target(&snapshot, target, "Update") {
                    Ok((skill, deployment)) => {
                        deployment_local_edits(&fs, &home, &skill.name, &deployment)
                    }
                    Err(e) => {
                        eprintln!("[local-edits] target not resolved: {e}");
                        NOT_CHECKED
                    }
                }
            })
            .collect())
    })
    .await
}
