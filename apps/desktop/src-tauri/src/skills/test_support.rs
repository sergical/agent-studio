// ============================================================================
// Skills Module - Test Support
// Fixture helpers shared across the skills module's inline test suites and
// (unlike the rest of this module) the separate `tests/core_scan_parity.rs`
// integration test crate: that crate links `skill_studio_lib` compiled
// without `--cfg test`, so anything it needs (this module, and
// `skill_refresh::build_snapshot`/`BuildPaths`) must be a plain `pub` item,
// not one gated behind `#[cfg(test)]`.
// ============================================================================

use std::path::Path;

/// Writes a minimal spec-valid `SKILL.md` at `dir/SKILL.md`, named `name`.
#[cfg(test)]
pub(crate) fn write_skill(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: test\n---\nBody."),
    )
    .unwrap();
}

/// Serializes and confines every test that reads or writes `OpenCode`
/// config through `skill_studio_host::opencode_config_dir`/
/// `skill_refresh::opencode_config_root`. Those resolvers check the
/// process-global `XDG_CONFIG_HOME`/`OPENCODE_CONFIG_DIR`/
/// `SKILL_STUDIO_FIXTURE` env vars - unset on a developer machine, but
/// GitHub's `ubuntu-latest` runners export a real `XDG_CONFIG_HOME`
/// (`/home/runner/.config`), so without this guard every OpenCode-touching
/// test read and wrote that one real shared directory instead of its own
/// fixture `home`, racing every other such test running in parallel (and,
/// for `SKILL_STUDIO_FIXTURE`, could leak a fixture run into a developer's
/// real `~/.config/opencode`). Held for the guarded test's whole body
/// (RAII, so a panic mid-test still restores the previous values, unlike
/// the hand-rolled save/restore this replaced) and serialized on a shared
/// lock, mirroring the host crate's `opencode_db::xdg_env_lock`.
pub struct OpencodeHomeGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev_xdg_config_home: Option<std::ffi::OsString>,
    prev_opencode_config_dir: Option<std::ffi::OsString>,
    prev_skill_studio_fixture: Option<std::ffi::OsString>,
}

/// Sets `XDG_CONFIG_HOME` to `<home>/.config` and clears
/// `OPENCODE_CONFIG_DIR`, without touching the lock or saving the previous
/// values. Split out of `OpencodeHomeGuard::new` so a test can re-pin the
/// env to `home` after deliberately overwriting it while still holding the
/// guard's lock, instead of racing another guarded test between the
/// overwrite and the guard's own pin.
pub fn pin_opencode_env(home: &Path) {
    // SAFETY: every caller holds `OpencodeHomeGuard`'s lock, which
    // serializes every test that touches these vars.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::remove_var("OPENCODE_CONFIG_DIR");
    }
}

impl OpencodeHomeGuard {
    /// Pins `XDG_CONFIG_HOME`/`OPENCODE_CONFIG_DIR` to `home` (see
    /// [`pin_opencode_env`]) and sets `SKILL_STUDIO_FIXTURE=1`, so every
    /// `OpenCode` config resolver in the desktop app - fixture-aware or
    /// not - agrees on `home/.config/opencode`.
    pub fn new(home: &Path) -> Self {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        let lock = LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prev_xdg_config_home = std::env::var_os("XDG_CONFIG_HOME");
        let prev_opencode_config_dir = std::env::var_os("OPENCODE_CONFIG_DIR");
        let prev_skill_studio_fixture = std::env::var_os("SKILL_STUDIO_FIXTURE");
        pin_opencode_env(home);
        // SAFETY: `lock` above serializes every test that touches this var.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("SKILL_STUDIO_FIXTURE", "1");
        }
        Self {
            _lock: lock,
            prev_xdg_config_home,
            prev_opencode_config_dir,
            prev_skill_studio_fixture,
        }
    }
}

impl Drop for OpencodeHomeGuard {
    fn drop(&mut self) {
        // SAFETY: `self._lock` is still held for the whole body of `drop`,
        // serializing every test that touches these vars.
        #[allow(unsafe_code)]
        unsafe {
            match self.prev_xdg_config_home.take() {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match self.prev_opencode_config_dir.take() {
                Some(v) => std::env::set_var("OPENCODE_CONFIG_DIR", v),
                None => std::env::remove_var("OPENCODE_CONFIG_DIR"),
            }
            match self.prev_skill_studio_fixture.take() {
                Some(v) => std::env::set_var("SKILL_STUDIO_FIXTURE", v),
                None => std::env::remove_var("SKILL_STUDIO_FIXTURE"),
            }
        }
    }
}
