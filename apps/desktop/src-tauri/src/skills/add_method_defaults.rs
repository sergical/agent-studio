// ============================================================================
// Skills Module - add_method_defaults
// What the Add Skill sheet needs to know before it can default the Method
// picker and the Harnesses selector: whether the dotagents CLI can actually
// run, whether skills.sh has ever been used on this machine, and which
// first-class agents are themselves installed - see `AddMethodDefaults`.
// ============================================================================

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::agents::AgentId;
use skill_studio_core::lock_file;

/// The binary every dotagents command in `commands.rs`/`skill_add.rs`
/// actually shells out to - `npx -y @sentry/dotagents ...`, same as the
/// skills.sh commands' `npx skills ...`. Its presence on `PATH` is the best
/// available proxy for "can dotagents run at all" without invoking it.
const DOTAGENTS_BINARY: &str = "npx";

/// The home-relative config directories that mark a first-class agent
/// "installed" on this machine - not its skills directory (which Skill
/// Studio itself may have just created), but the directory the agent's own
/// CLI/app creates on first run. `OpenCode` checks both its current
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    /// Whether the install scope's `.claude/skills` is a symlink that resolves
    /// to the same scope's `.agents/skills` - true when Claude Code already
    /// reads the shared folder on its own, false when it's a real directory,
    /// a link to any other folder, or doesn't exist yet.
    pub claude_reads_shared_folder: bool,
}

/// `path` with `.` and `..` folded away, without touching the disk.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether `root/.claude/skills` is a link to `root/.agents/skills`. A link
/// whose target does not exist yet is compared by its written target, the
/// same way the core install plan decides it.
fn claude_reads_shared_folder(root: &Path) -> bool {
    let claude = root.join(".claude").join("skills");
    let shared = root.join(".agents").join("skills");
    let is_link =
        std::fs::symlink_metadata(&claude).is_ok_and(|meta| meta.file_type().is_symlink());
    if !is_link {
        return false;
    }
    match (
        std::fs::canonicalize(&claude),
        std::fs::canonicalize(&shared),
    ) {
        (Ok(resolved), Ok(shared)) => resolved == shared,
        _ => std::fs::read_link(&claude)
            .is_ok_and(|target| lexical(&root.join(".claude").join(target)) == lexical(&shared)),
    }
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
/// `project` picks the install scope for `claude_reads_shared_folder`: that
/// project's folders, or the home folders when it is `None`.
fn add_method_defaults(home: &Path, project: Option<&Path>, path_var: &str) -> AddMethodDefaults {
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
        claude_reads_shared_folder: claude_reads_shared_folder(project.unwrap_or(home)),
    }
}

/// Whether dotagents can run, whether skills.sh has been used before, and
/// which first-class agents are installed - the Add Skill sheet fetches this
/// when it opens, and again when the install scope or project changes, to
/// pick its Method and Harnesses defaults.
#[tauri::command]
pub async fn get_add_method_defaults(
    app: tauri::AppHandle,
    project_path: Option<String>,
) -> Result<AddMethodDefaults, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "get_add_method_defaults", move || {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let path_var = std::env::var("PATH").unwrap_or_default();
        Ok(add_method_defaults(
            &home,
            project_path.as_deref().map(Path::new),
            &path_var,
        ))
    })
    .await
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
        let defaults = add_method_defaults(tmp.path(), None, tmp.path().to_str().unwrap());
        assert!(!defaults.dotagents_installed);
    }

    #[test]
    fn dotagents_installed_is_true_when_npx_resolves_on_path() {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        make_executable(&bin_dir.join("npx"));

        let defaults = add_method_defaults(tmp.path(), None, bin_dir.to_str().unwrap());
        assert!(defaults.dotagents_installed);
    }

    #[test]
    fn has_skill_lock_is_false_when_the_lock_file_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let defaults = add_method_defaults(tmp.path(), None, "");
        assert!(!defaults.has_skill_lock);
    }

    #[test]
    fn has_skill_lock_is_true_when_the_lock_file_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().join(".agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join(".skill-lock.json"), "{}").unwrap();

        let defaults = add_method_defaults(tmp.path(), None, "");
        assert!(defaults.has_skill_lock);
    }

    #[test]
    fn installed_harnesses_is_empty_on_a_fresh_home() {
        let tmp = tempfile::tempdir().unwrap();
        let defaults = add_method_defaults(tmp.path(), None, "");
        assert!(defaults.installed_harnesses.is_empty());
    }

    #[test]
    fn installed_harnesses_finds_claude_code_and_opencodes_legacy_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        std::fs::create_dir_all(tmp.path().join(".opencode")).unwrap();

        let defaults = add_method_defaults(tmp.path(), None, "");
        assert_eq!(
            defaults.installed_harnesses,
            vec![AgentId::ClaudeCode, AgentId::OpenCode]
        );
    }

    #[test]
    fn claude_reads_shared_folder_is_false_when_claude_skills_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let defaults = add_method_defaults(tmp.path(), None, "");
        assert!(!defaults.claude_reads_shared_folder);
    }

    #[test]
    fn claude_reads_shared_folder_is_false_when_claude_skills_is_a_real_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude").join("skills")).unwrap();

        let defaults = add_method_defaults(tmp.path(), None, "");
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

        let defaults = add_method_defaults(tmp.path(), None, "");
        assert!(defaults.claude_reads_shared_folder);
    }

    #[test]
    #[cfg(unix)]
    fn claude_skills_linked_to_a_dotfiles_folder_is_not_read_as_the_shared_folder_or_the_ui_hides_its_own_folder(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".agents").join("skills")).unwrap();
        let dotfiles = tmp.path().join("dotfiles").join("claude-skills");
        std::fs::create_dir_all(&dotfiles).unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        std::os::unix::fs::symlink(&dotfiles, tmp.path().join(".claude").join("skills")).unwrap();

        let defaults = add_method_defaults(tmp.path(), None, "");
        assert!(
            !defaults.claude_reads_shared_folder,
            "a link to {} was reported as the shared folder",
            dotfiles.display()
        );
    }

    #[test]
    #[cfg(unix)]
    fn project_scope_reads_the_projects_claude_link_not_the_homes_or_the_ui_locks_the_wrong_scope()
    {
        let home = tempfile::tempdir().unwrap();
        let home_shared = home.path().join(".agents").join("skills");
        std::fs::create_dir_all(&home_shared).unwrap();
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::os::unix::fs::symlink(&home_shared, home.path().join(".claude").join("skills"))
            .unwrap();
        let plain_project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(plain_project.path().join(".claude").join("skills")).unwrap();
        let linked_project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(linked_project.path().join(".claude")).unwrap();
        std::os::unix::fs::symlink(
            "../.agents/skills",
            linked_project.path().join(".claude").join("skills"),
        )
        .unwrap();

        let plain = add_method_defaults(home.path(), Some(plain_project.path()), "");
        let linked = add_method_defaults(home.path(), Some(linked_project.path()), "");
        assert!(
            !plain.claude_reads_shared_folder,
            "a project with a real .claude/skills took the home link's answer"
        );
        assert!(
            linked.claude_reads_shared_folder,
            "a project .claude/skills linked to its own .agents/skills (not made yet) was missed"
        );
    }
}
