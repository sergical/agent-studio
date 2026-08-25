// ============================================================================
// Skill Studio - Rust Backend
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
            app.manage(skills::skill_agent_runner::SkillAgentRunnerState::default());
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
            skills::commands::read_installed_skill_md,
            skills::commands::write_installed_skill_md,
            skills::commands::write_installed_skill_md_if_unchanged,
            skills::commands::open_skill_path,
            // Background refresh / invocation snapshot
            skills::skill_refresh::get_skill_snapshot,
            skills::skill_refresh::request_skill_rescan,
            skills::skill_refresh::register_skill_projects,
            skills::skill_refresh::unregister_skill_project,
            // Local harness runner
            skills::skill_agent_runner::start_skill_agent_run,
            skills::skill_agent_runner::cancel_skill_agent_run,
            skills::skill_agent_runner::create_skill_scratch_dir,
            skills::skill_agent_runner::remove_skill_scratch_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
