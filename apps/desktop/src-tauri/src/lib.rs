// ============================================================================
// Skill Studio - Rust Backend
// Skills.sh integration for skill discovery, installation, and management
// ============================================================================

mod skills;

use tauri::Manager;

pub use skills::*;

/// Opens the event store at `app`'s data dir (docs/spec-event-store.md) - not
/// `~/.agents`, which stays reserved for `skill-studio.json` - and reconciles
/// any row left `pending` by a crash. A failure at either step returns
/// `None` rather than aborting startup; every event command surfaces that as
/// an ordinary `Err`.
fn open_event_store(app: &tauri::App) -> Option<skills::event_store::EventStore> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| eprintln!("[event_store] could not resolve app data dir: {e}"))
        .ok()?;
    let store = skills::event_store::EventStore::open(&app_data)
        .map_err(|e| eprintln!("[event_store] failed to open: {e}"))
        .ok()?;
    match store.reconcile_at_startup() {
        Ok(flipped) => {
            for row in &flipped {
                eprintln!(
                    "[event_store] event {} ({}) was pending at startup - the app was quit mid-operation; flipped to interrupted",
                    row.id, row.kind
                );
            }
        }
        Err(e) => eprintln!("[event_store] startup reconcile failed: {e}"),
    }
    Some(store)
}

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

            let event_store = open_event_store(app);
            app.manage(skills::event_commands::EventStoreState(
                std::sync::Mutex::new(event_store),
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            skills::add_method_defaults::get_add_method_defaults,
            // Skills.sh integration
            skills::commands::get_skills_sh_access,
            skills::commands::set_skills_sh_api_key,
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
            skills::commands::list_installed_editors,
            skills::commands::get_preferred_editor,
            skills::commands::set_preferred_editor,
            skills::skill_update_check::check_skill_updates_now,
            // Fork / Pull upstream / Un-fork
            skills::skill_fork::fork_skill,
            skills::skill_fork::pull_fork_upstream,
            skills::skill_fork::unfork_skill,
            // Add skill / trials
            skills::skill_add::add_skill,
            skills::skill_add::add_skills,
            skills::github_skill_listing::list_github_skills,
            skills::skill_trial::keep_skill_trial,
            skills::skill_trial::restore_trashed_skill,
            // Park (disable globally) / per-harness disable / invocation policy
            skills::skill_park::park_skill,
            skills::skill_park::unpark_skill,
            skills::skill_harness_disable::set_harness_enabled,
            skills::skill_harness_disable::set_deployment_enabled,
            skills::skill_invocation::set_skill_invocation,
            // Event store: History and per-harness materialize disable
            skills::event_commands::list_skill_events,
            skills::event_commands::restore_skill_event,
            skills::event_commands::set_shared_harness_skill_enabled,
            skills::event_commands::materialize_harness_root,
            skills::event_commands::distribute_skill_from_shared,
            skills::event_commands::repair_skill_link,
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
            // Packs
            skills::skill_pack::list_skill_packs,
            skills::skill_pack::create_skill_pack,
            skills::skill_pack::update_skill_pack,
            skills::skill_pack::publish_skill_pack,
            skills::skill_pack::delete_skill_pack,
            skills::skill_pack::import_skill_pack,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
