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
            app.manage(skills::skill_run_target::SkillRunTargetState::default());
            app.manage(skills::skill_fork::ForkMutationLock::default());
            skills::skill_update_check::spawn_update_check_loop(app.handle().clone());
            skills::skill_trial::spawn_trial_expiry_loop(app.handle().clone());
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
            skills::skill_update_check::check_skill_updates_now,
            // Fork / Pull upstream / Un-fork
            skills::skill_fork::fork_skill,
            skills::skill_fork::pull_fork_upstream,
            skills::skill_fork::unfork_skill,
            // Add skill / trials
            skills::skill_add::add_skill,
            skills::skill_trial::keep_skill_trial,
            skills::skill_trial::restore_trashed_skill,
            // Park (disable globally) / per-harness disable / invocation policy
            skills::skill_park::park_skill,
            skills::skill_park::unpark_skill,
            skills::skill_harness_disable::set_harness_enabled,
            skills::skill_invocation::set_skill_invocation,
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
            // Test run targets (scratch / worktree / in place)
            skills::skill_run_target::prepare_skill_run_target,
            skills::skill_run_target::skill_run_target_diff,
            skills::skill_run_target::apply_skill_run_target_diff,
            skills::skill_run_target::discard_skill_run_target,
            skills::skill_run_target::reveal_skill_run_target,
            // Run history
            skills::skill_run_history::record_skill_run,
            skills::skill_run_history::list_skill_runs,
            skills::skill_run_history::read_skill_run_events,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
