// ============================================================================
// Agent Studio - Rust Backend
// Comprehensive Claude Code entity discovery and management
// ============================================================================

mod commands;

pub use commands::*;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            // Discovery
            commands::discover_all,
            commands::scan_projects,
            
            // Entity-specific discovery
            commands::discover_settings,
            commands::discover_memory,
            commands::discover_agents,
            commands::discover_skills,
            commands::discover_commands,
            commands::discover_plugins,
            commands::discover_mcp_servers,
            commands::extract_hooks,
            
            // Analysis
            commands::find_duplicates,
            commands::check_symlink,
            
            // File operations
            commands::read_file,
            commands::write_file,
            commands::file_exists,
            commands::delete_file,
            commands::delete_directory,
            
            // Entity creation
            commands::create_entity,
            
            // Utility
            commands::get_home_directory,
            commands::get_config_directory,
            commands::get_global_claude_path,
            
            // Legacy (for backward compatibility)
            commands::discover_configs,
            commands::create_agent,
            commands::create_skill,
            commands::delete_skill,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
