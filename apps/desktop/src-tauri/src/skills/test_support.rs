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

// Only `fixture_snapshot_owning` below uses these; gated the same way it is
// (see the module doc) so a non-test build of this always-`pub` module
// doesn't warn about them as unused.
#[cfg(test)]
use std::collections::BTreeMap;

#[cfg(test)]
use chrono::Utc;

#[cfg(test)]
use super::frontmatter::InvocationPolicy;
#[cfg(test)]
use super::skill_dto::{Deployment, InstalledSkill};
#[cfg(test)]
use super::{SkillSnapshot, SourceKind};

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

/// A minimal `SkillSnapshot` owning one deployment at `dep_dir`, for tests of
/// a guard that checks a path against the current snapshot
/// (`skill_refresh::snapshot_owns_path` and its callers) without a running
/// Tauri app. Mirrors `skill_refresh`'s own private `fixture_snapshot` test
/// helper; kept here (rather than shared with it) because that one is
/// `#[cfg(test)]`-private to its own module and this crate has no
/// `pub(crate)` re-export path into another module's `mod tests`.
#[cfg(test)]
pub(crate) fn fixture_snapshot_owning(dep_dir: &Path) -> SkillSnapshot {
    use skill_studio_core::skill_uses::InvocationHeatmap;

    SkillSnapshot {
        revision: 0,
        skills: vec![InstalledSkill {
            name: "foo".to_string(),
            source: "manual".to_string(),
            source_type: "manual".to_string(),
            source_url: None,
            skill_path: None,
            installed_at: Utc::now().to_rfc3339(),
            updated_at: None,
            has_update: false,
            update_owner_ids: Vec::new(),
            update_owners: Vec::new(),
            update_commit: None,
            update_commit_at: None,
            source_kind: SourceKind::Manual,
            deployments: vec![Deployment {
                agent: "Claude Code".to_string(),
                scope: "project".to_string(),
                path: dep_dir.to_string_lossy().to_string(),
                is_symlink: false,
                plugin: None,
                ..Default::default()
            }],
            has_spec: false,
            description: None,
            spec_violations: Vec::new(),
            skill_md_tokens: 0,
            description_tokens: 0,
            folder_bytes: 0,
            file_count: 0,
            content_hash: String::new(),
            content_hashes: Vec::new(),
            modified_at: None,
            frontmatter_fields: BTreeMap::new(),
            folder_truncated: false,
            fork: None,
            trial: None,
            trials: Vec::new(),
            parked: false,
            parked_at: None,
            invocation: InvocationPolicy::Both,
        }],
        projects: Vec::new(),
        invocations: Vec::new(),
        heatmap: InvocationHeatmap::default(),
        scanned_at: Utc::now().to_rfc3339(),
        last_test_by_skill: Default::default(),
        update_check: Default::default(),
        opencode_config_kind: None,
        scan_partial: false,
        scan_observations: Vec::new(),
        unread_roots: Vec::new(),
    }
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

/// The lock `OpencodeHomeGuard` holds, shared with any other test that
/// touches `SKILL_STUDIO_FIXTURE` directly (without going through the
/// guard) so the two never race: `SKILL_STUDIO_FIXTURE` picks
/// `core_scan_installed_skills`'s fixture-vs-live branch, and a test
/// asserting on the live branch's `CODEX_HOME` handling must not have
/// another parallel test flip it to fixture mid-scan.
pub fn opencode_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
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
        let lock = opencode_env_lock()
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

/// Prepends `dir` to `PATH` for the guarded test's whole body (RAII, so a
/// panic mid-test still restores it), holding the same shared
/// [`opencode_env_lock`] every other process-wide env mutation in this
/// crate's tests serializes on - `PATH` and the `OpenCode` env vars are
/// disjoint, but a shared lock is simpler than a second one and process
/// env mutation is inherently crate-wide regardless of which vars a test
/// touches.
pub struct PathGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev_path: Option<std::ffi::OsString>,
}

impl PathGuard {
    /// Prepends `dir` to the current `PATH`.
    pub fn new(dir: &Path) -> Self {
        let lock = opencode_env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prev_path = std::env::var_os("PATH");
        let new_path = match &prev_path {
            Some(p) => {
                let mut joined = std::ffi::OsString::from(dir);
                joined.push(":");
                joined.push(p);
                joined
            }
            None => dir.as_os_str().to_owned(),
        };
        // SAFETY: `lock` above serializes every test that touches `PATH`.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("PATH", new_path);
        }
        Self {
            _lock: lock,
            prev_path,
        }
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        // SAFETY: `self._lock` is still held for the whole body of `drop`.
        #[allow(unsafe_code)]
        unsafe {
            match self.prev_path.take() {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }
    }
}
