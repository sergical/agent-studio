// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    #[cfg(all(target_os = "macos", feature = "worker-repair"))]
    if std::env::args_os()
        .nth(1)
        .is_some_and(|argument| argument == "__event-worker")
    {
        if std::env::args_os().count() != 2 {
            return std::process::ExitCode::from(64);
        }
        return unsafe { skill_studio_core::skill_event_worker_entry::run_event_worker_stdio() };
    }
    skill_studio_lib::run();
    std::process::ExitCode::SUCCESS
}
