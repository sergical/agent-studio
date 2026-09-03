// ============================================================================
// Skills Module - add_method_defaults
// What the Add Skill sheet needs to know before it can default the Method
// picker and the Harnesses selector: whether the dotagents CLI can actually
// run, whether skills.sh has ever been used on this machine, and which
// first-class agents are themselves installed - see `AddMethodDefaults`.
// ============================================================================

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::agents::AgentId;
use super::lock_file;

/// The binary every dotagents command in `commands.rs`/`skill_add.rs`
/// actually shells out to - `npx -y @sentry/dotagents ...`, same as the
/// skills.sh commands' `npx skills ...`. Its presence on `PATH` is the best
/// available proxy for "can dotagents run at all" without invoking it.
const DOTAGENTS_BINARY: &str = "npx";

/// The home-relative config directories that mark a first-class agent
/// "installed" on this machine - not its skills directory (which Skill
/// Studio itself may have just created), but the directory the agent's own
/// CLI/app creates on first run. OpenCode checks both its current
/// (`.config/opencode`) and legacy (`.opencode`) locations.
fn harness_config_dirs(id: AgentId, home: &Path) -> Vec<PathBuf> {
    match id {
        AgentId::ClaudeCode => vec![home.join(".claude")],
        AgentId::Codex => vec![home.join(".codex")],
        AgentId::OpenCode => vec![
            home.join(".config").join("opencode"),
            home.join(".opencode"),
        ],
        AgentId::Pi => vec![home.join(".pi")],
        AgentId::Cursor => vec![home.join(".cursor")],
        AgentId::GrokBuild => vec![home.join(".grok")],
        _ => vec![],
    }
}

/// The first-class agents checked for "installed on this machine" - the same
/// six `skill_roots` scans for native provenance.
const CHECKED_HARNESSES: &[AgentId] = &[
    AgentId::ClaudeCode,
    AgentId::Codex,
    AgentId::OpenCode,
    AgentId::Pi,
    AgentId::Cursor,
    AgentId::GrokBuild,
];

/// What the Add Skill sheet needs before it can pick sensible defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddMethodDefaults {
    /// Whether `npx` (what every dotagents command shells out to) resolves
    /// on `PATH` - dotagents can't run at all without it.
    pub dotagents_installed: bool,
    /// Whether `~/.agents/.skill-lock.json` exists - skills.sh has been used
    /// to install at least one skill on this machine before.
    pub has_skill_lock: bool,
    /// Every first-class agent whose own config directory exists on this
    /// machine, in `AgentId`'s declaration order - see `harness_config_dirs`.
    pub installed_harnesses: Vec<AgentId>,
    /// Whether `~/.claude/skills` is a symlink into the shared folder -
    /// true when Claude Code already reads `.agents/skills` on its own,
    /// false when it's a real directory (or doesn't exist yet).
    pub claude_reads_shared_folder: bool,
}

/// Whether `path` is a symlink - used for `claude_reads_shared_folder`,
/// which cares whether Claude Code's skills dir was linked into the shared
/// folder rather than left as its own real directory.
fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// Whether `binary` resolves to an executable file on `path_var` (a `PATH`-
/// style, `:`-joined string) - walks each entry itself rather than shelling
/// out to `which`.
fn resolves_on_path(binary: &str, path_var: &str) -> bool {
    std::env::split_paths(path_var).any(|dir| {
        let candidate = dir.join(binary);
        std::fs::metadata(&candidate).is_ok_and(|meta| meta.is_file())
    })
}

/// `get_add_method_defaults`'s logic against an arbitrary home dir and `PATH`
/// value, so tests don't need to touch the real `~/.agents` or `PATH`.
fn add_method_defaults(home: &Path, path_var: &str) -> AddMethodDefaults {
    AddMethodDefaults {
        dotagents_installed: resolves_on_path(DOTAGENTS_BINARY, path_var),
        has_skill_lock: lock_file::lock_file_path(home).exists(),
        installed_harnesses: CHECKED_HARNESSES
            .iter()
            .copied()
            .filter(|id| {
                harness_config_dirs(*id, home)
                    .iter()
                    .any(|dir| dir.exists())
            })
            .collect(),
        claude_reads_shared_folder: is_symlink(&home.join(".claude").join("skills")),
    }
}

/// Whether dotagents can run, whether skills.sh has been used before, and
/// which first-class agents are installed - the Add Skill sheet fetches this
/// once when it opens to pick its Method and Harnesses defaults.
#[tauri::command]
pub fn get_add_method_defaults() -> Result<AddMethodDefaults, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let path_var = std::env::var("PATH").unwrap_or_default();
    Ok(add_method_defaults(&home, &path_var))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_executable(path: &Path) {
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms).unwrap();
        }
    }

    #[test]
    fn dotagents_installed_is_false_when_npx_is_not_on_path() {
        let tmp = tempfile::tempdir().unwrap();
        let defaults = add_method_defaults(tmp.path(), tmp.path().to_str().unwrap());
        assert!(!defaults.dotagents_installed);
    }

    #[test]
    fn dotagents_installed_is_true_when_npx_resolves_on_path() {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        make_executable(&bin_dir.join("npx"));

        let defaults = add_method_defaults(tmp.path(), bin_dir.to_str().unwrap());
        assert!(defaults.dotagents_installed);
    }

    #[test]
    fn has_skill_lock_is_false_when_the_lock_file_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let defaults = add_method_defaults(tmp.path(), "");
        assert!(!defaults.has_skill_lock);
    }

    #[test]
    fn has_skill_lock_is_true_when_the_lock_file_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().join(".agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join(".skill-lock.json"), "{}").unwrap();

        let defaults = add_method_defaults(tmp.path(), "");
        assert!(defaults.has_skill_lock);
    }

    #[test]
    fn installed_harnesses_is_empty_on_a_fresh_home() {
        let tmp = tempfile::tempdir().unwrap();
        let defaults = add_method_defaults(tmp.path(), "");
        assert!(defaults.installed_harnesses.is_empty());
    }

    #[test]
    fn installed_harnesses_finds_claude_code_and_opencodes_legacy_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        std::fs::create_dir_all(tmp.path().join(".opencode")).unwrap();

        let defaults = add_method_defaults(tmp.path(), "");
        assert_eq!(
            defaults.installed_harnesses,
            vec![AgentId::ClaudeCode, AgentId::OpenCode]
        );
    }

    #[test]
    fn claude_reads_shared_folder_is_false_when_claude_skills_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let defaults = add_method_defaults(tmp.path(), "");
        assert!(!defaults.claude_reads_shared_folder);
    }

    #[test]
    fn claude_reads_shared_folder_is_false_when_claude_skills_is_a_real_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude").join("skills")).unwrap();

        let defaults = add_method_defaults(tmp.path(), "");
        assert!(!defaults.claude_reads_shared_folder);
    }

    #[test]
    #[cfg(unix)]
    fn claude_reads_shared_folder_is_true_when_claude_skills_is_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join(".agents").join("skills");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        std::os::unix::fs::symlink(&shared, tmp.path().join(".claude").join("skills")).unwrap();

        let defaults = add_method_defaults(tmp.path(), "");
        assert!(defaults.claude_reads_shared_folder);
    }
}
