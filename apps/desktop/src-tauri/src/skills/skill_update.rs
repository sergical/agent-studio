// ============================================================================
// Skills Module - skill_update
// In-app update through `tauri-plugin-updater` (unit 6.2): check `latest.json`
// on launch, every four hours, and from the Settings "Check for updates"
// button; download the new version in the background; install it only after
// the user confirms a restart. The state machine (`UpdateEngine`) and the
// schedule (`UpdateCheckScheduler`) are generic over `UpdaterPort` and
// `Clock` so both are unit-testable with a fake - no real network call and
// no wall-clock sleep in a test. `TauriUpdaterPort` is the only piece that
// touches the real plugin.
// ============================================================================

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

/// How often the background loop rechecks once it has already checked once
/// (the launch check is the first `due()` call, always true).
pub const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);

/// How often the background loop wakes to ask `UpdateCheckScheduler` whether
/// `UPDATE_CHECK_INTERVAL` has elapsed. Short relative to the interval so the
/// loop's own sleep never meaningfully delays the four-hour recheck; not a
/// wall-clock assertion any test depends on.
const LOOP_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Event the Settings "Version" card subscribes to, so an automatic launch
/// or four-hour check (no direct caller awaiting a command response) still
/// updates the UI, the same way `onSkillSnapshot` covers a background scan.
pub const UPDATE_STATUS_EVENT: &str = "skills://update-status";

/// The Settings "Version" card's update state - up to date, checking,
/// downloading, ready to install, or refused. Mirrors `serde(tag =
/// "status")` on the TypeScript side (`UpdateStatus` in `skill-types.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum UpdateStatus {
    UpToDate,
    Checking,
    Downloading { version: String },
    ReadyToInstall { version: String },
    Error { message: String },
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// An update `UpdaterPort::check` found, not yet downloaded. Boxed as a
/// trait object (rather than a concrete `tauri_plugin_updater::Update`) so
/// `UpdateEngine` compiles against a fake in tests without linking the real
/// plugin.
pub trait PendingUpdate: Send {
    fn version(&self) -> String;
    /// Downloads and verifies the update's signature against the manifest
    /// pubkey in `tauri.conf.json`. Runs in the background - the caller
    /// decides when (or whether) to call `ReadyUpdate::install` after this
    /// resolves.
    fn download(self: Box<Self>) -> BoxFuture<'static, Result<Box<dyn ReadyUpdate>, String>>;
}

/// A downloaded, signature-verified update, staged for install.
pub trait ReadyUpdate: Send {
    fn version(&self) -> String;
    /// Installs the staged update. The only caller is `UpdateEngine::confirm_install`,
    /// itself only reachable from the `install_update` command the Settings
    /// "Restart to update" button fires - never from `run_check` directly, so
    /// no update installs itself without the user's confirmation.
    fn install(self: Box<Self>) -> Result<(), String>;
}

/// The one seam between `UpdateEngine` and the real `tauri-plugin-updater`
/// network calls, so a test can substitute a fake that never touches the
/// network.
pub trait UpdaterPort: Send + Sync {
    fn check(&self) -> BoxFuture<'_, Result<Option<Box<dyn PendingUpdate>>, String>>;
}

/// The update state machine: check, (maybe) download, wait for confirmation,
/// install. One instance lives for the app's lifetime as Tauri-managed
/// state (`UpdateEngineState`), shared by the launch check, the four-hour
/// loop, and the manual "Check for updates" button.
pub struct UpdateEngine<U: UpdaterPort> {
    updater: U,
    status: Mutex<UpdateStatus>,
    /// Set only once `run_check` reaches `ReadyToInstall`; taken (not just
    /// read) by `confirm_install`, so an update can only ever be installed
    /// once per download.
    ready: Mutex<Option<Box<dyn ReadyUpdate>>>,
}

impl<U: UpdaterPort> UpdateEngine<U> {
    pub fn new(updater: U) -> Self {
        Self {
            updater,
            status: Mutex::new(UpdateStatus::UpToDate),
            ready: Mutex::new(None),
        }
    }

    pub fn status(&self) -> UpdateStatus {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_status(&self, status: &UpdateStatus, on_change: &mut impl FnMut(&UpdateStatus)) {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = status.clone();
        on_change(status);
    }

    /// Runs one check-download pass: `Checking` -> (`UpToDate` or
    /// `Downloading` -> `ReadyToInstall`) -> or `Error` on either step's
    /// failure. Calls `on_change` after every transition, so a caller with
    /// no direct response to await (the launch check, the four-hour loop)
    /// can still emit each state to the UI. Never installs anything -
    /// `confirm_install` is the only path to `ReadyUpdate::install`.
    pub async fn run_check(&self, mut on_change: impl FnMut(&UpdateStatus)) {
        self.set_status(&UpdateStatus::Checking, &mut on_change);
        match self.updater.check().await {
            Ok(None) => self.set_status(&UpdateStatus::UpToDate, &mut on_change),
            Ok(Some(pending)) => {
                let version = pending.version();
                self.set_status(
                    &UpdateStatus::Downloading {
                        version: version.clone(),
                    },
                    &mut on_change,
                );
                match pending.download().await {
                    Ok(ready) => {
                        *self.ready.lock().unwrap_or_else(PoisonError::into_inner) = Some(ready);
                        self.set_status(&UpdateStatus::ReadyToInstall { version }, &mut on_change);
                    }
                    Err(message) => {
                        self.set_status(&UpdateStatus::Error { message }, &mut on_change);
                    }
                }
            }
            Err(message) => self.set_status(&UpdateStatus::Error { message }, &mut on_change),
        }
    }

    /// Installs the update `run_check` already downloaded. Refused - naming
    /// the missing state rather than silently doing nothing - unless a
    /// download already reached `ReadyToInstall`; the Settings button is the
    /// only caller, so this is also the only place an install ever happens.
    pub fn confirm_install(&self) -> Result<(), String> {
        let ready = self
            .ready
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match ready {
            Some(ready) => ready.install(),
            None => Err("No update is ready to install".to_string()),
        }
    }
}

/// Injectable time source so `UpdateCheckScheduler` never calls
/// `Instant::now()` directly - a test can advance a fake clock instead of
/// sleeping.
pub trait Clock: Send + Sync {
    fn now(&self) -> std::time::Instant;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }
}

/// Decides whether a background check is due: the first call is always due
/// (the launch check), and later calls are due once `interval` has elapsed
/// since the last due call. Records `now` as soon as a call is due, even if
/// the caller's check then fails - a failed check does not get an immediate
/// retry, matching `skill_update_check`'s six-hour loop precedent.
pub struct UpdateCheckScheduler<C: Clock = SystemClock> {
    clock: C,
    interval: Duration,
    last_check: Mutex<Option<std::time::Instant>>,
}

impl<C: Clock> UpdateCheckScheduler<C> {
    pub fn new(clock: C, interval: Duration) -> Self {
        Self {
            clock,
            interval,
            last_check: Mutex::new(None),
        }
    }

    pub fn due(&self) -> bool {
        let now = self.clock.now();
        let mut last = self
            .last_check
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let is_due = match *last {
            None => true,
            Some(previous) => now.duration_since(previous) >= self.interval,
        };
        if is_due {
            *last = Some(now);
        }
        is_due
    }
}

// ============================================================================
// Real plugin adapter
// ============================================================================

struct TauriReadyUpdate {
    update: tauri_plugin_updater::Update,
    bytes: Vec<u8>,
}

impl ReadyUpdate for TauriReadyUpdate {
    fn version(&self) -> String {
        self.update.version.clone()
    }

    fn install(self: Box<Self>) -> Result<(), String> {
        self.update.install(&self.bytes).map_err(|e| e.to_string())
    }
}

struct TauriPendingUpdate(tauri_plugin_updater::Update);

impl PendingUpdate for TauriPendingUpdate {
    fn version(&self) -> String {
        self.0.version.clone()
    }

    fn download(self: Box<Self>) -> BoxFuture<'static, Result<Box<dyn ReadyUpdate>, String>> {
        Box::pin(async move {
            let bytes = self
                .0
                .download(|_chunk_len, _total_len| {}, || {})
                .await
                .map_err(|e| e.to_string())?;
            Ok(Box::new(TauriReadyUpdate {
                update: self.0,
                bytes,
            }) as Box<dyn ReadyUpdate>)
        })
    }
}

/// The real `UpdaterPort`: reads the endpoint and pubkey from
/// `tauri.conf.json`'s `plugins.updater` via `UpdaterExt::updater`, so this
/// adapter carries no config of its own.
pub struct TauriUpdaterPort {
    app: AppHandle,
}

impl TauriUpdaterPort {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl UpdaterPort for TauriUpdaterPort {
    fn check(&self) -> BoxFuture<'_, Result<Option<Box<dyn PendingUpdate>>, String>> {
        let app = self.app.clone();
        Box::pin(async move {
            let updater =
                tauri_plugin_updater::UpdaterExt::updater(&app).map_err(|e| e.to_string())?;
            let found = updater.check().await.map_err(|e| e.to_string())?;
            Ok(found.map(|update| Box::new(TauriPendingUpdate(update)) as Box<dyn PendingUpdate>))
        })
    }
}

/// Tauri-managed state wrapping the one `UpdateEngine` the app's lifetime
/// shares between the background loop and both commands below.
pub struct UpdateEngineState(pub Arc<UpdateEngine<TauriUpdaterPort>>);

async fn run_check_and_emit(
    app: &AppHandle,
    engine: &UpdateEngine<TauriUpdaterPort>,
) -> UpdateStatus {
    let app_for_emit = app.clone();
    engine
        .run_check(move |status| {
            // Best-effort, matching every other `emit` in this crate - a
            // frontend that hasn't subscribed yet (or has none open) is not
            // a failure the check itself should report.
            let _ = app_for_emit.emit(UPDATE_STATUS_EVENT, status.clone());
        })
        .await;
    engine.status()
}

/// Starts the background loop on the async runtime (never the main thread,
/// per the 0.3 rule): checks once immediately (the launch check - the
/// scheduler's first `due()` call is always true), then rechecks every
/// `UPDATE_CHECK_INTERVAL`. A failed check is logged and does not stop the
/// loop or block the rest of startup, matching the issue's "no network -
/// starts and behaves as before" requirement.
pub fn spawn_update_check_loop(app: AppHandle) {
    let engine = app.state::<UpdateEngineState>().0.clone();
    let scheduler = UpdateCheckScheduler::new(SystemClock, UPDATE_CHECK_INTERVAL);
    tauri::async_runtime::spawn(async move {
        loop {
            if scheduler.due() {
                if let UpdateStatus::Error { message } = run_check_and_emit(&app, &engine).await {
                    eprintln!("[skill_update] check failed: {message}");
                }
            }
            tokio::time::sleep(LOOP_POLL_INTERVAL).await;
        }
    });
}

/// Manual "Check for updates" button (Settings) and the launch check both
/// call this directly; only the four-hour loop goes through the scheduler,
/// so a manual click always runs immediately regardless of when the loop
/// last checked.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<UpdateStatus, String> {
    crate::timing_log::time_command_async(&app, "check_for_update", async {
        let engine = app.state::<UpdateEngineState>().0.clone();
        Ok(run_check_and_emit(&app, &engine).await)
    })
    .await
}

/// Catch-up read for the Settings card on mount, matching
/// `get_add_skill_operation`'s pattern - the background loop's own checks
/// have no other way to reach a component that (re)mounted after they ran.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri's command extractor requires an owned `State<T>`.
pub fn get_update_status(state: tauri::State<UpdateEngineState>) -> UpdateStatus {
    state.0.status()
}

/// The Settings "Restart to update" button's only backend call: installs
/// the already-downloaded update and restarts. Refused (naming the state)
/// if no download has reached `ReadyToInstall` - the button is disabled
/// until then, but the backend does not trust that alone.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let restart_app = app.clone();
    crate::timing_log::time_command_async(&app, "install_update", async move {
        let engine = restart_app.state::<UpdateEngineState>().0.clone();
        engine.confirm_install()?;
        restart_app.request_restart();
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    struct FakeClock {
        now: Mutex<std::time::Instant>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                now: Mutex::new(std::time::Instant::now()),
            }
        }

        fn advance(&self, by: Duration) {
            let mut now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
            *now += by;
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> std::time::Instant {
            *self.now.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    /// Flow: the background loop's very first `due()` call, before any
    /// check has ever run.
    /// Expectation: `due()` is true - this is what makes the launch check
    /// happen without a separate "first run" branch in the loop.
    /// A failure here means a fresh install would wait a full interval
    /// before its first update check instead of checking on launch.
    #[test]
    fn due_is_true_on_the_first_call_covering_the_launch_check() {
        let scheduler = UpdateCheckScheduler::new(FakeClock::new(), Duration::from_secs(60));
        assert!(scheduler.due());
    }

    /// Flow: `due()` called again immediately after a due call, then again
    /// after the clock advances past the interval.
    /// Expectation: false until the interval elapses, then true - the
    /// four-hour recheck, not a tighter or looser cadence.
    /// A failure here means the loop would either hammer the endpoint on
    /// every poll or never recheck after the first launch.
    #[test]
    fn due_is_false_until_the_interval_elapses_then_true_again() {
        let clock = FakeClock::new();
        let scheduler = UpdateCheckScheduler::new(clock, Duration::from_secs(60));
        assert!(scheduler.due());
        assert!(!scheduler.due(), "should not be due again immediately");
        scheduler.clock.advance(Duration::from_secs(59));
        assert!(!scheduler.due(), "should not be due one second early");
        scheduler.clock.advance(Duration::from_secs(1));
        assert!(scheduler.due(), "should be due once the interval elapses");
    }

    struct FakeReadyUpdate {
        version: String,
        install_calls: Arc<AtomicU32>,
    }

    impl ReadyUpdate for FakeReadyUpdate {
        fn version(&self) -> String {
            self.version.clone()
        }

        fn install(self: Box<Self>) -> Result<(), String> {
            self.install_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FakePendingUpdate {
        version: String,
        install_calls: Arc<AtomicU32>,
        download_called: Arc<AtomicBool>,
        download_fails: bool,
    }

    impl PendingUpdate for FakePendingUpdate {
        fn version(&self) -> String {
            self.version.clone()
        }

        fn download(self: Box<Self>) -> BoxFuture<'static, Result<Box<dyn ReadyUpdate>, String>> {
            self.download_called.store(true, Ordering::SeqCst);
            let fails = self.download_fails;
            Box::pin(async move {
                if fails {
                    return Err("signature mismatch".to_string());
                }
                Ok(Box::new(FakeReadyUpdate {
                    version: self.version,
                    install_calls: self.install_calls,
                }) as Box<dyn ReadyUpdate>)
            })
        }
    }

    /// `None` when no update is queued, `Some` (built from the other
    /// fields) once, so a test can hand `run_check` exactly one pending
    /// update and then observe `check` is not called a second time by
    /// anything in this module.
    struct FakeUpdaterPort {
        version: String,
        has_update: bool,
        download_fails: bool,
        install_calls: Arc<AtomicU32>,
        download_called: Arc<AtomicBool>,
    }

    impl UpdaterPort for FakeUpdaterPort {
        fn check(&self) -> BoxFuture<'_, Result<Option<Box<dyn PendingUpdate>>, String>> {
            let pending = self.has_update.then(|| {
                Box::new(FakePendingUpdate {
                    version: self.version.clone(),
                    install_calls: self.install_calls.clone(),
                    download_called: self.download_called.clone(),
                    download_fails: self.download_fails,
                }) as Box<dyn PendingUpdate>
            });
            Box::pin(async move { Ok(pending) })
        }
    }

    /// Flow: `run_check` against a port with no update available.
    /// Expectation: the final status is `UpToDate`, `on_change` still saw
    /// `Checking` first - proves the transition happens even with nothing
    /// to download.
    /// A failure here means a no-update response would strand the UI on
    /// "Checking" forever or skip straight to `UpToDate` with no
    /// `Checking` state.
    #[tokio::test]
    async fn run_check_with_no_update_available_reports_up_to_date() {
        let port = FakeUpdaterPort {
            version: "9.9.9".to_string(),
            has_update: false,
            download_fails: false,
            install_calls: Arc::new(AtomicU32::new(0)),
            download_called: Arc::new(AtomicBool::new(false)),
        };
        let engine = UpdateEngine::new(port);
        let mut seen = Vec::new();
        engine.run_check(|status| seen.push(status.clone())).await;
        assert_eq!(seen, vec![UpdateStatus::Checking, UpdateStatus::UpToDate]);
        assert_eq!(engine.status(), UpdateStatus::UpToDate);
    }

    /// Flow: `run_check` against a port with an update, download succeeds.
    /// Expectation: ends at `ReadyToInstall`, download ran, but
    /// `install_calls` is still zero - `run_check` downloads, it never
    /// installs.
    /// A failure here (a non-zero `install_calls`) means an update could
    /// install itself the moment it finishes downloading, without the user
    /// ever pressing "Restart to update".
    #[tokio::test]
    async fn run_check_downloads_an_available_update_without_installing_it() {
        let install_calls = Arc::new(AtomicU32::new(0));
        let download_called = Arc::new(AtomicBool::new(false));
        let port = FakeUpdaterPort {
            version: "2.0.0".to_string(),
            has_update: true,
            download_fails: false,
            install_calls: install_calls.clone(),
            download_called: download_called.clone(),
        };
        let engine = UpdateEngine::new(port);
        engine.run_check(|_| {}).await;
        assert_eq!(
            engine.status(),
            UpdateStatus::ReadyToInstall {
                version: "2.0.0".to_string()
            }
        );
        assert!(download_called.load(Ordering::SeqCst));
        assert_eq!(install_calls.load(Ordering::SeqCst), 0);
    }

    /// Flow: `confirm_install` called after `run_check` reached
    /// `ReadyToInstall`.
    /// Expectation: `install` runs exactly once, and a second
    /// `confirm_install` call is refused rather than installing again -
    /// `ready` is taken, not just read.
    /// A failure here means clicking "Restart to update" twice (a
    /// double-click, a retried IPC call) would install the same update
    /// twice.
    #[tokio::test]
    async fn confirm_install_runs_the_downloaded_update_exactly_once() {
        let install_calls = Arc::new(AtomicU32::new(0));
        let port = FakeUpdaterPort {
            version: "2.0.0".to_string(),
            has_update: true,
            download_fails: false,
            install_calls: install_calls.clone(),
            download_called: Arc::new(AtomicBool::new(false)),
        };
        let engine = UpdateEngine::new(port);
        engine.run_check(|_| {}).await;

        engine
            .confirm_install()
            .expect("first confirm should install");
        assert_eq!(install_calls.load(Ordering::SeqCst), 1);

        let second = engine.confirm_install();
        assert!(second.is_err(), "a second confirm must not re-install");
        assert_eq!(install_calls.load(Ordering::SeqCst), 1);
    }

    /// Flow: `confirm_install` called before any check has ever run.
    /// Expectation: refused, naming that nothing is ready - never a panic
    /// or a silent no-op that would look like a successful install to the
    /// caller.
    /// A failure here means the "Restart to update" button, if ever enabled
    /// too early, could report success while installing nothing.
    #[tokio::test]
    async fn confirm_install_without_a_completed_download_is_refused() {
        let port = FakeUpdaterPort {
            version: "2.0.0".to_string(),
            has_update: false,
            download_fails: false,
            install_calls: Arc::new(AtomicU32::new(0)),
            download_called: Arc::new(AtomicBool::new(false)),
        };
        let engine = UpdateEngine::new(port);
        let error = engine
            .confirm_install()
            .expect_err("nothing downloaded yet");
        assert!(error.contains("No update"));
    }

    /// Flow: `run_check` against a port whose download fails (e.g. a
    /// signature mismatch).
    /// Expectation: ends at `Error` naming the failure, and
    /// `confirm_install` afterward is still refused - a failed download
    /// never leaves a stale `ready` update behind.
    /// A failure here means a corrupted or tampered download could still
    /// be installed after being reported as failed.
    #[tokio::test]
    async fn run_check_reports_a_failed_download_as_an_error_and_leaves_nothing_ready() {
        let port = FakeUpdaterPort {
            version: "2.0.0".to_string(),
            has_update: true,
            download_fails: true,
            install_calls: Arc::new(AtomicU32::new(0)),
            download_called: Arc::new(AtomicBool::new(false)),
        };
        let engine = UpdateEngine::new(port);
        engine.run_check(|_| {}).await;
        match engine.status() {
            UpdateStatus::Error { message } => assert!(message.contains("signature")),
            other => panic!("expected Error status, got {other:?}"),
        }
        assert!(engine.confirm_install().is_err());
    }
}
