// ============================================================================
// Skill Studio - core_runtime
// The desktop's first wiring onto `skill-studio-core`'s `ops` functions.
// Mirrors `apps/cli/src/main.rs`'s `build_runtime_write` and the CLI's
// unflagged default scope (`apps/cli/src/scope.rs`), so the CLI, MCP, and
// desktop share one event store and lease root on the real machine and one
// `ops` function decides the result for all three.
// ============================================================================

use std::path::PathBuf;
use std::sync::Arc;

use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::ops::{Outcome, ResultEnvelope};
use skill_studio_core::ports::Runtime;
use skill_studio_core::{OpStatus, RuntimeScope};

/// `$XDG_DATA_HOME/skill-studio`, or `~/.local/share/skill-studio` when
/// `XDG_DATA_HOME` is unset, matching the CLI's default so the CLI, MCP, and
/// desktop read and write the same history database and lease file.
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
/// file lease, and a writable SQLite history store, rooted at the host's
/// home directory. Every desktop command that calls a core `ops` function
/// that opens a `MutationSession` (park, unpark, and every write to come)
/// takes its `Runtime` from here.
pub fn build_runtime_write() -> Result<Runtime, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    build_runtime_write_at(home, data_root())
}

/// [`build_runtime_write`], but rooted at `home` and `data_root` given
/// directly rather than read from the host. `set_harness_enabled_with`
/// (Codex's `[[skills.config]]` write) takes this so it stays testable
/// against a tempdir `home`, the way it was before that write moved onto
/// `ops::set_codex_skill_disabled` - the real command still calls
/// `build_runtime_write` above, which resolves `home` and `data_root` from
/// the host exactly as it did before this function existed.
pub(crate) fn build_runtime_write_at(home: PathBuf, data_root: PathBuf) -> Result<Runtime, String> {
    let history_root = data_root.join("history");
    let codex_home = skill_studio_host::codex_home(&home);
    let scope = RuntimeScope::live(home, history_root).with_codex_home(codex_home);
    let catalog = Arc::new(HarnessCatalog::builtin());
    let lease_root = data_root.join("leases");
    let db_path = scope.history_root.join("events.sqlite3");
    let mut ports = skill_studio_host::default_ports_with_history(lease_root, catalog, db_path);
    ports.discovery = Some(Arc::new(skill_studio_host::HostProjectDiscovery::new()));
    ports.tools = Some(Arc::new(skill_studio_host::PathToolLookup::new()));
    Runtime::new(&scope, ports).map_err(|err| err.message)
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
            .map(|e| e.message.clone())
            .unwrap_or_else(|| "operation failed".to_string())),
    }
}
