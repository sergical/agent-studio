// ============================================================================
// Skill Studio - core_runtime
// The desktop's first wiring onto `skill-studio-core`'s `ops` functions.
// Mirrors `apps/cli/src/main.rs`'s `build_runtime_write` and the CLI's
// unflagged default scope (`apps/cli/src/scope.rs`), so the CLI, MCP, and
// desktop share one event store and lease root on the real machine and one
// `ops` function decides the result for all three.
// ============================================================================

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::ops::{Outcome, ResultEnvelope};
use skill_studio_core::ports::Runtime;
use skill_studio_core::{OpStatus, RuntimeScope};

/// `$XDG_DATA_HOME/skill-studio`, or `~/.local/share/skill-studio` when
/// `XDG_DATA_HOME` is unset, matching the CLI's default so the CLI, MCP, and
/// desktop read and write the same history database and lease file.
///
/// `pub(crate)` so `write_lease.rs` can root every desktop write's lease
/// under the same `leases` directory `build_runtime_write` uses for park
/// and unpark, instead of a second, unrelated location.
pub(crate) fn data_root() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("skill-studio");
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".local/share/skill-studio")
}

/// Builds a `Runtime` wired for a mutation: the real filesystem, a real
/// file lease, a writable `SQLite` history store, and a real process
/// spawner, rooted at the host's home directory. Every desktop command that
/// calls a core `ops` function that opens a `MutationSession` (park, unpark,
/// update, and every write to come) takes its `Runtime` from here. The
/// spawner (unused by park/unpark/`set_harness_enabled`) is what lets
/// `ops::update`'s `Dotagents`/`SkillsSh` methods shell out to `npx` - the
/// same `RealProcessSpawner` the CLI's own `build_runtime_write` wires in
/// `apps/cli/src/main.rs`.
pub fn build_runtime_write() -> Result<Runtime, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    build_runtime_write_at(&home, &data_root())
}

/// [`build_runtime_write`], but rooted at `home` and `data_root` given
/// directly rather than read from the host. `set_harness_enabled_with`
/// (Codex's `[[skills.config]]` write) takes this so it stays testable
/// against a tempdir `home`, the way it was before that write moved onto
/// `ops::set_codex_skill_disabled` - the real command still calls
/// `build_runtime_write` above, which resolves `home` and `data_root` from
/// the host exactly as it did before this function existed. `pub` (not
/// `pub(crate)`) so `tests/fix_parity.rs` can build the desktop side of its
/// parity check with the desktop adapter's own runtime constructor instead
/// of a hand-mirrored copy of it.
pub fn build_runtime_write_at(home: &Path, data_root: &Path) -> Result<Runtime, String> {
    let history_root = data_root.join("history");
    let codex_home = skill_studio_host::codex_home(home);
    let mut scope =
        RuntimeScope::live(home.to_path_buf(), history_root).with_codex_home(codex_home);
    scope.opencode_config_root = Some(skill_studio_host::opencode_config_dir(home));
    let catalog = Arc::new(HarnessCatalog::builtin());
    let lease_root = data_root.join("leases");
    let db_path = scope.history_root.join("events.sqlite3");
    let mut ports = skill_studio_host::default_ports_with_history(lease_root, catalog, db_path);
    ports.discovery = Some(Arc::new(skill_studio_host::HostProjectDiscovery::new()));
    ports.tools = Some(Arc::new(skill_studio_host::PathToolLookup::new()));
    // `ops::install`'s `Dotagents`/`SkillsSh` methods shell out through this
    // port (`ops_install_cli::install_via_cli`), and `ops::install_preferences`
    // checks it to detect `npx` on `PATH` - neither worked from the desktop
    // until this was added here, alongside `build_runtime_detect`'s own copy.
    ports.spawner = Some(Arc::new(skill_studio_host::RealProcessSpawner::new()));
    Runtime::new(&scope, ports).map_err(|err| err.message)
}

/// Builds a `Runtime` for `ops::harnesses`: the only op that resolves
/// executables and spawns `--version` probes, so it is also the only one
/// that needs the login-shell `PATH` (`LoginShellToolLookup`) instead of the
/// process's own, minimal `launchd` `PATH` (`PathToolLookup`, used by
/// `build_runtime_write` for every other command). Read-only: no lease root
/// or history store beyond what `Runtime::new` needs to normalize the scope.
pub fn build_runtime_detect() -> Result<Runtime, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let mut rt = build_runtime_write_at(&home, &data_root())?;
    rt.ports.tools = Some(Arc::new(skill_studio_host::LoginShellToolLookup::new()));
    rt.ports.spawner = Some(Arc::new(skill_studio_host::RealProcessSpawner::new()));
    Ok(rt)
}

/// Unwraps a `ResultEnvelope` into the plain `Result<T, String>` every
/// Tauri command returns. The envelope's `scope`/`timing`/`correlation_id`
/// fields are dropped here: `skill-api.ts`'s `parkSkill`/`unparkSkill` both
/// discard the command's return value already, so nothing downstream reads
/// them, and every other desktop command already returns `Result<T, String>`
/// bare.
pub fn to_command_result<T: Outcome>(envelope: ResultEnvelope<T>) -> Result<T, String> {
    match envelope.data {
        Some(data) if envelope.status != OpStatus::Error => Ok(data),
        _ => Err(envelope
            .errors
            .first()
            .map_or_else(|| "operation failed".to_string(), |e| e.message.clone())),
    }
}
