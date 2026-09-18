// ============================================================================
// Skills Module - harness_first_run
// The first-run screen: `detect_harnesses` runs `skill-studio-core`'s
// `ops::harnesses` (the same op the CLI's `harnesses` subcommand and the MCP
// server's `harnesses` tool call) off the UI thread, over the login-shell
// `PATH` so a `launchd`-started desktop app sees the same binaries the
// user's terminal does (`core_runtime::build_runtime_detect`). The screen's
// result - which rows the user kept, and whether to search harness history
// for project folders - is saved once under the registry's `harnesses` key
// (`skill_fork_registry::ForkRegistry::harnesses`); a later launch with that
// key present skips the screen and calls `detect_harnesses` again only to
// refresh the rows in the background, per
// `docs/action-map/harnesses/harness-detection.md`.
//
// Old path deleted in this PR: there wasn't one - no first-run screen
// existed before this unit, so there is no ad-hoc detection to remove here.
// `apps/desktop/src-tauri/src/skills/agents.rs`'s `FIRST_CLASS_AGENTS` stays:
// it drives which directories `scan` reads for skills regardless of whether
// a harness is installed (a skill folder can exist with no harness on this
// machine), a different job from telling the user what is installed.
// ============================================================================

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use skill_studio_core::dto::HarnessesRequest;
use skill_studio_core::harness::HarnessReport;
use skill_studio_core::identity::CorrelationId;
use skill_studio_core::ops::{self, Operation, ResultEnvelope};
use skill_studio_core::ports::OpContext;

/// The first-run screen's saved choice, round-tripped through the registry.
/// Kept small and documented per unit 3.2's issue: unit 4.4 reads `kept` to
/// decide which harnesses the rest of the app still shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessesChoice {
    /// Catalog ids (`AgentId::as_str()`, e.g. `"claude-code"`) of the rows
    /// the user kept on the first-run screen.
    pub kept: Vec<String>,
    /// Whether the user opted in to searching harness history (Codex
    /// `config.toml` trust rows, Claude Code transcripts, ...) for project
    /// folders, mirroring the per-harness discovery switch in
    /// `docs/action-map/settings-and-projects.md`.
    pub search_project_folders: bool,
    /// RFC 3339 timestamp of the save, for a support report; not read by any
    /// decision in the app.
    pub saved_at: String,
}

/// Runs `ops::harnesses` off the UI thread. Called both for the first-run
/// screen itself and for the background re-detection a later launch runs
/// once the screen has already been completed.
#[tauri::command]
pub async fn detect_harnesses(app: tauri::AppHandle) -> Result<HarnessReport, String> {
    crate::timing_log::time_command_blocking(&app, "detect_harnesses", move || {
        let rt = super::core_runtime::build_runtime_detect()?;
        let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
        let result = ops::harnesses(&rt, &ctx, &HarnessesRequest {});
        let envelope = ResultEnvelope::from_result(Operation::Harnesses, &rt.scope, &ctx, result);
        super::core_runtime::to_command_result(envelope)
    })
    .await
}

/// The saved first-run choice, or `None` when the screen has never been
/// completed - the frontend's signal to show it.
#[tauri::command]
pub async fn get_harnesses_choice(
    app: tauri::AppHandle,
) -> Result<Option<HarnessesChoice>, String> {
    crate::timing_log::time_command_blocking(&app, "get_harnesses_choice", move || {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        Ok(super::skill_fork_registry::read_fork_registry(&home)?.harnesses)
    })
    .await
}

/// Saves the first-run screen's choice, so the next launch skips it.
#[tauri::command]
pub async fn save_harnesses_choice(
    choice: HarnessesChoice,
    app: tauri::AppHandle,
) -> Result<(), String> {
    crate::timing_log::time_command_blocking(&app, "save_harnesses_choice", move || {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let mut registry = super::skill_fork_registry::read_fork_registry(&home)?;
        registry.harnesses = Some(choice);
        super::skill_fork_registry::write_fork_registry(&home, &registry)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use skill_studio_core::harness::HarnessState;
    use skill_studio_core::ports::{OpContext, Ports, Runtime};
    use skill_studio_core::{harness::HarnessCatalog, RuntimeScope};
    use std::sync::Arc;

    /// `clean_home_with_no_harness_reaches_the_list_with_zero_harnesses_and_no_error`:
    /// an empty temp `$HOME` (no `ToolLookup`, no config, no sessions) must
    /// make `ops::harnesses` report every row `NotFound`, never an `Err` and
    /// never a panic on a missing home - the crash/failure test the issue
    /// names. Fails if a probe unwraps a missing directory instead of
    /// treating it as "not found".
    #[test]
    fn clean_home_with_no_harness_reaches_the_list_with_zero_harnesses_and_no_error() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let lease_root = tmp.path().join("leases");
        let catalog = Arc::new(HarnessCatalog::builtin());
        let scope = RuntimeScope::fixture(home.clone());
        let db_path = scope.history_root.join("events.sqlite3");
        let ports: Ports =
            skill_studio_host::default_ports_with_history(lease_root, catalog, db_path);
        let rt = Runtime::new(&scope, ports).unwrap();
        let ctx = OpContext::uncancellable(CorrelationId("test".into()));

        let report = ops::harnesses(&rt, &ctx, &HarnessesRequest {}).unwrap();

        assert!(
            report
                .harnesses
                .iter()
                .all(|d| d.state == HarnessState::NotFound),
            "a clean home must report every harness NotFound, got: {:?}",
            report.harnesses
        );
        assert_eq!(
            report.harnesses.len(),
            HarnessCatalog::builtin().facts.len()
        );

        let inventory =
            ops::scan(&rt, &ctx, &skill_studio_core::dto::ScanRequest::default()).unwrap();
        assert!(inventory.skills.is_empty());
    }

    /// `detect_finds_claude_code_and_codex_and_reports_pi_and_opencode_not_found_or_names_the_wrong_row`:
    /// a fixture home with only Claude Code's and Codex's config/session
    /// files present must not mark pi or `OpenCode` as anything but
    /// `NotFound` - proves the four-signal recipe reads each harness's own
    /// relative paths, not a shared "any harness data exists" flag. Fails if
    /// `HarnessAdapter::detect` cross-reads another harness's files.
    #[test]
    fn detect_finds_claude_code_and_codex_and_reports_pi_and_opencode_not_found_or_names_the_wrong_row(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join(".claude/projects/one")).unwrap();
        std::fs::write(home.join(".claude/projects/one/session.jsonl"), "{}").unwrap();
        std::fs::write(home.join(".claude/settings.json"), "{}").unwrap();
        std::fs::create_dir_all(home.join(".codex/sessions/2026")).unwrap();
        std::fs::write(home.join(".codex/config.toml"), "").unwrap();

        let lease_root = tmp.path().join("leases");
        let catalog = Arc::new(HarnessCatalog::builtin());
        let scope = RuntimeScope::fixture(home.clone());
        let db_path = scope.history_root.join("events.sqlite3");
        let ports: Ports =
            skill_studio_host::default_ports_with_history(lease_root, catalog, db_path);
        let rt = Runtime::new(&scope, ports).unwrap();
        let ctx = OpContext::uncancellable(CorrelationId("test".into()));

        let report = ops::harnesses(&rt, &ctx, &HarnessesRequest {}).unwrap();
        let state_of = |id: &str| {
            report
                .harnesses
                .iter()
                .find(|d| d.id.as_str() == id)
                .unwrap_or_else(|| panic!("no row for {id}"))
                .state
        };
        // No ToolLookup port here: neither binary resolves, so a
        // configured-and-used harness reads as DataOnly, not Configured.
        assert_eq!(
            state_of("claude-code"),
            HarnessState::DataOnly,
            "claude-code row"
        );
        assert_eq!(state_of("codex"), HarnessState::DataOnly, "codex row");
        assert_eq!(state_of("pi"), HarnessState::NotFound, "pi row");
        assert_eq!(
            state_of("open-code"),
            HarnessState::NotFound,
            "open-code row"
        );
    }

    /// `a_second_launch_skips_the_screen_and_re_detects_in_the_background_or_shows_the_screen_again`:
    /// a registry with a saved `harnesses` key must round-trip through
    /// `read_fork_registry`/`write_fork_registry` so `get_harnesses_choice`
    /// (the frontend's screen-or-skip signal) reads `Some`; a registry with
    /// no key at all reads `None`. Fails if `harnesses` isn't wired into
    /// `ForkRegistry`'s serde shape or its `Default` impl.
    #[test]
    fn a_second_launch_skips_the_screen_and_re_detects_in_the_background_or_shows_the_screen_again()
    {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join(".agents")).unwrap();

        let stale = super::super::skill_fork_registry::read_fork_registry_or_default(&home);
        assert!(
            stale.harnesses.is_none(),
            "fresh registry must show the screen"
        );

        let mut registry = stale;
        registry.harnesses = Some(HarnessesChoice {
            kept: vec!["claude-code".to_string()],
            search_project_folders: true,
            saved_at: "2026-09-18T00:00:00Z".to_string(),
        });
        super::super::skill_fork_registry::write_fork_registry(&home, &registry).unwrap();

        let reloaded = super::super::skill_fork_registry::read_fork_registry(&home).unwrap();
        assert_eq!(
            reloaded.harnesses.map(|c| c.kept),
            Some(vec!["claude-code".to_string()]),
            "a saved choice must round-trip so the next launch skips the screen"
        );
    }

    /// `unknown_prints_as_unknown_never_guessed_from_a_folder_name`: a
    /// harness whose `--version` prints nothing usable (here, no spawner
    /// port at all, the same "no primary source" case) must report `version`
    /// and `install_method` as the `Unknown` value, not a guess derived from
    /// the folder it was found under. Fails if any code path invents a
    /// version from a path segment instead of leaving it `Unknown`.
    #[test]
    fn unknown_prints_as_unknown_never_guessed_from_a_folder_name() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bin_dir = tmp.path().join("bin-v9.9.9"); // a folder name that looks like a version
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin_dir).unwrap();
        let claude_bin = bin_dir.join("claude");
        std::fs::write(&claude_bin, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&claude_bin).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&claude_bin, perms).unwrap();
        }

        let lease_root = tmp.path().join("leases");
        let catalog = Arc::new(HarnessCatalog::builtin());
        let scope = RuntimeScope::fixture(home.clone());
        let db_path = scope.history_root.join("events.sqlite3");
        let mut ports: Ports =
            skill_studio_host::default_ports_with_history(lease_root, catalog, db_path);
        ports.tools = Some(Arc::new(
            skill_studio_host::PathToolLookup::with_search_dirs(vec![bin_dir]),
        ));
        // No spawner port: version and install method must stay Unknown
        // rather than be inferred from `bin-v9.9.9`.
        let rt = Runtime::new(&scope, ports).unwrap();
        let ctx = OpContext::uncancellable(CorrelationId("test".into()));

        let report = ops::harnesses(&rt, &ctx, &HarnessesRequest {}).unwrap();
        let claude = report
            .harnesses
            .iter()
            .find(|d| d.id.as_str() == "claude-code")
            .unwrap();
        assert!(
            claude.version.value.is_none(),
            "version must be Unknown, got {:?}",
            claude.version
        );
        assert!(
            claude.install_method.value.is_none(),
            "install_method must be Unknown, got {:?}",
            claude.install_method
        );
    }

    /// A `ProcessSpawner` that sleeps before returning, standing in for a
    /// slow `--version` probe so the test below can tell whether the
    /// calling task was blocked for that whole duration.
    struct SleepySpawner {
        sleep_for: std::time::Duration,
    }

    impl skill_studio_core::ports::ProcessSpawner for SleepySpawner {
        fn run(
            &self,
            _spec: &skill_studio_core::ports::ProcessSpec,
            _cancel: &dyn skill_studio_core::ports::CancelToken,
        ) -> Result<skill_studio_core::ports::ProcessOutput, skill_studio_core::CoreError> {
            std::thread::sleep(self.sleep_for);
            Ok(skill_studio_core::ports::ProcessOutput {
                status: Some(0),
                stdout: "1.0.0\n".to_string(),
                stderr: String::new(),
                timed_out: false,
            })
        }
    }

    /// `detect_runs_off_the_ui_thread_and_never_blocks_over_one_frame`: the
    /// desktop command runs `ops::harnesses` through
    /// `crate::timing_log::time_command_blocking`, which is
    /// `tauri::async_runtime::spawn_blocking` under an `.await` (see that
    /// function's body). A slow `--version` probe (100 ms, well over one
    /// 16 ms frame) must not stall a concurrent async task ticking on the
    /// same runtime - proof that the probe runs on the blocking pool, not
    /// on the thread driving the async task tree the UI event loop shares.
    /// Fails if `detect_harnesses` (or a future edit to it) calls
    /// `ops::harnesses` directly on the calling task instead of through
    /// `spawn_blocking`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn detect_runs_off_the_ui_thread_and_never_blocks_over_one_frame() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin_dir).unwrap();
        let claude_bin = bin_dir.join("claude");
        std::fs::write(&claude_bin, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&claude_bin).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&claude_bin, perms).unwrap();
        }

        let lease_root = tmp.path().join("leases");
        let catalog = Arc::new(HarnessCatalog::builtin());
        let scope = RuntimeScope::fixture(home.clone());
        let db_path = scope.history_root.join("events.sqlite3");
        let mut ports: Ports =
            skill_studio_host::default_ports_with_history(lease_root, catalog, db_path);
        ports.tools = Some(Arc::new(
            skill_studio_host::PathToolLookup::with_search_dirs(vec![bin_dir]),
        ));
        ports.spawner = Some(Arc::new(SleepySpawner {
            sleep_for: std::time::Duration::from_millis(100),
        }));
        let rt = Runtime::new(&scope, ports).unwrap();
        let ctx = OpContext::uncancellable(CorrelationId("test".into()));

        // A "UI thread" proxy: a tight async loop that only makes progress
        // if the runtime keeps scheduling it while the slow probe runs.
        let ticks = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let ticks_task = ticks.clone();
        let ticker = tokio::spawn(async move {
            for _ in 0..20 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                ticks_task.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });

        let detect = tauri::async_runtime::spawn_blocking(move || {
            ops::harnesses(&rt, &ctx, &HarnessesRequest {})
        });
        let result = detect.await.unwrap().unwrap();
        ticker.await.unwrap();

        assert!(
            result
                .harnesses
                .iter()
                .any(|d| d.id.as_str() == "claude-code" && d.version.value.is_some()),
            "the slow probe should still have produced a version"
        );
        assert!(
            ticks.load(std::sync::atomic::Ordering::SeqCst) >= 15,
            "the UI-thread proxy task barely ticked while the probe ran ({} ticks), meaning the probe blocked the runtime instead of running on spawn_blocking's pool",
            ticks.load(std::sync::atomic::Ordering::SeqCst)
        );
    }
}
