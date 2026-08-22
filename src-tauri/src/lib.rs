// ============================================================================
// Agent Studio - Rust Backend
// Skills.sh integration for skill discovery, installation, and management
// ============================================================================

mod skills;

pub use skills::*;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            // Skills.sh integration
            skills::commands::search_skills,
            skills::commands::get_popular_skills,
            skills::commands::get_skill_details,
            skills::commands::get_installed_skills,
            skills::commands::is_skill_installed,
            skills::commands::get_agent_targets,
            skills::commands::install_skill,
            skills::commands::remove_skill,
            skills::commands::update_skill,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
