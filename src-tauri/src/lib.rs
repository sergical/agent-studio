// ============================================================================
// Agent Studio - Rust Backend
// Skills.sh integration for skill discovery, installation, and management
// ============================================================================

mod skills;

use tauri::Manager;

pub use skills::*;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let refresh_state = skills::skill_refresh::init(app.handle());
            app.manage(refresh_state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Skills.sh integration
            skills::commands::search_skills,
            skills::commands::get_popular_skills,
            skills::commands::get_skill_details,
            skills::commands::get_installed_skills,
            skills::commands::list_skill_projects,
            skills::commands::is_skill_installed,
            skills::commands::get_agent_targets,
            skills::commands::install_skill,
            skills::commands::remove_skill,
            skills::commands::update_skill,
            // Background refresh / invocation snapshot
            skills::skill_refresh::get_skill_snapshot,
            skills::skill_refresh::request_skill_rescan,
            skills::skill_refresh::register_skill_projects,
            skills::skill_refresh::unregister_skill_project,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
