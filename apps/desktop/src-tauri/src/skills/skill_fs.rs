// ============================================================================
// Skills Module - skill_fs
// One shared directory-copy routine, reused everywhere a skill's files need
// to become a second, independent copy: `skill_fork`'s fork/pull snapshots,
// `skill_add`'s "Copy" method, and `skill_pack`'s bundling of manual/fork
// skills into a pack directory.
// ============================================================================

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

#[derive(Clone, Copy)]
enum SkillTreeCopyMode {
    SkipSymlinks,
    PreserveSymlinks,
}

/// Recursively copies `src` into `dst`, creating `dst` if needed. Symlinks
/// are skipped so the result is a plain, self-contained tree.
pub(crate) fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), String> {
    copy_skill_tree(src, dst, SkillTreeCopyMode::SkipSymlinks)
}

/// Recursively copies a plain skill tree and checks the Add operation before
/// each filesystem entry.
pub(crate) fn copy_dir_all_controlled(
    src: &Path,
    dst: &Path,
    control: &super::skill_process::AddOperationControl,
) -> Result<(), String> {
    copy_skill_tree_with_check(src, dst, SkillTreeCopyMode::SkipSymlinks, &mut || {
        control.check_message()
    })
}

/// Copies a skill tree without following symlinks. Each link keeps its literal
/// target, including dangling, absolute, and outside-tree targets.
pub(crate) fn copy_dir_preserving_symlinks(src: &Path, dst: &Path) -> Result<(), String> {
    copy_skill_tree(src, dst, SkillTreeCopyMode::PreserveSymlinks)
}

fn copy_skill_tree(src: &Path, dst: &Path, mode: SkillTreeCopyMode) -> Result<(), String> {
    copy_skill_tree_with_check(src, dst, mode, &mut || Ok(()))
}

/// Recursive-copy check seam used to make mid-copy cancellation deterministic
/// in tests.
#[cfg(test)]
pub(crate) fn copy_dir_all_with_check(
    src: &Path,
    dst: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    copy_skill_tree_with_check(src, dst, SkillTreeCopyMode::SkipSymlinks, check)
}

fn copy_skill_tree_with_check(
    src: &Path,
    dst: &Path,
    mode: SkillTreeCopyMode,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    check()?;
    fs::create_dir_all(dst).map_err(|e| format!("Failed to create {}: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("Failed to read {}: {e}", src.display()))? {
        check()?;
        let entry = entry.map_err(|e| format!("Failed to read a directory entry: {e}"))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("Failed to stat {}: {e}", entry.path().display()))?;
        let dest_path = dst.join(entry.file_name());
        if file_type.is_symlink() {
            if matches!(mode, SkillTreeCopyMode::PreserveSymlinks) {
                let target = fs::read_link(entry.path()).map_err(|e| {
                    format!("Failed to read symlink {}: {e}", entry.path().display())
                })?;
                create_symlink(&target, &dest_path)?;
            }
        } else if file_type.is_dir() {
            copy_skill_tree_with_check(&entry.path(), &dest_path, mode, check)?;
        } else if file_type.is_file() {
            copy_file_with_check(&entry.path(), &dest_path, check)?;
        } else {
            return Err(format!(
                "Refused to copy unsupported special file: {}",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

fn copy_file_with_check(
    source: &Path,
    destination: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let mut input = fs::File::open(source)
        .map_err(|error| format!("Failed to open {}: {error}", source.display()))?;
    let mut output = fs::File::create(destination)
        .map_err(|error| format!("Failed to create {}: {error}", destination.display()))?;
    // Heap-allocated: a 64 KiB stack array is flagged as an oversized local.
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        check()?;
        let count = input
            .read(&mut buffer)
            .map_err(|error| format!("Failed to read {}: {error}", source.display()))?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| format!("Failed to write {}: {error}", destination.display()))?;
    }
    let permissions = fs::metadata(source)
        .map_err(|error| format!("Failed to stat {}: {error}", source.display()))?
        .permissions();
    fs::set_permissions(destination, permissions).map_err(|error| {
        format!(
            "Failed to set permissions on {}: {error}",
            destination.display()
        )
    })
}

fn create_symlink(target: &Path, link: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
            .map_err(|e| format!("Failed to create symlink {}: {e}", link.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        Err("Symlink-preserving copies are only supported on Unix".to_string())
    }
}

// ============================================================================
// Claude Code symlink rule - moved from `skill_add.rs` (unit 3.5c).
// `ops::install` now creates this link itself for every method it owns, but
// `skill_pack`'s import still calls this directly for its own, unrelated
// writes.
// ============================================================================

/// `~/.claude/skills/<name>` -> `../../.agents/skills/<name>`, relative -
/// only when Claude Code is one of `agents`, `shared_skills_dir`'s sibling
/// `claude_skills_dir` exists as a *real* directory (not the whole-dir
/// symlink some setups use instead), and no entry named `name` is already
/// there. Returns the created symlink's path, or `None` when nothing was
/// created (Claude Code not selected, or the whole-dir symlink already
/// covers it).
pub(crate) fn maybe_claude_code_symlink(
    claude_skills_dir: &Path,
    shared_skills_dir: &Path,
    name: &str,
    agents: &[super::agents::AgentId],
) -> Result<Option<String>, String> {
    if !agents.contains(&super::agents::AgentId::ClaudeCode) {
        return Ok(None);
    }
    if let Ok(meta) = fs::symlink_metadata(claude_skills_dir) {
        if meta.file_type().is_symlink() {
            // The whole-dir symlink to the shared root already covers it.
            return Ok(None);
        }
    } else {
        fs::create_dir_all(claude_skills_dir)
            .map_err(|e| format!("Failed to create {}: {e}", claude_skills_dir.display()))?;
    }

    let link_path = claude_skills_dir.join(name);
    if fs::symlink_metadata(&link_path).is_ok() {
        return Ok(None);
    }
    let target = relative_path_between(claude_skills_dir, shared_skills_dir).join(name);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link_path)
        .map_err(|e| format!("Failed to symlink {}: {e}", link_path.display()))?;
    #[cfg(not(unix))]
    return Err("Symlinking is only supported on Unix".to_string());
    Ok(Some(link_path.to_string_lossy().to_string()))
}

/// A relative path that leads from inside `from` to `to`: one `..` per
/// component of `from` below the common prefix, then the rest of `to`.
/// For `~/.claude/skills` and `~/.agents/skills` that is
/// `../../.agents/skills`.
fn relative_path_between(from: &Path, to: &Path) -> std::path::PathBuf {
    let from_parts: Vec<_> = from.components().collect();
    let to_parts: Vec<_> = to.components().collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut rel = std::path::PathBuf::new();
    for _ in common..from_parts.len() {
        rel.push("..");
    }
    for part in &to_parts[common..] {
        rel.push(part);
    }
    rel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[test]
    fn copies_files_and_skips_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("SKILL.md"), "hello").unwrap();
        fs::write(src.join("sub/file.txt"), "world").unwrap();
        #[cfg(unix)]
        symlink("SKILL.md", src.join("link.md")).unwrap();

        let dst = tmp.path().join("dst");
        copy_dir_all(&src, &dst).unwrap();

        assert_eq!(fs::read_to_string(dst.join("SKILL.md")).unwrap(), "hello");
        assert_eq!(
            fs::read_to_string(dst.join("sub/file.txt")).unwrap(),
            "world"
        );
        assert!(!dst.join("link.md").exists());
    }

    #[test]
    fn recursive_copy_stops_at_injected_cancellation() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::write(src.join("first"), "one").unwrap();
        fs::write(src.join("nested/second"), "two").unwrap();
        let mut checks = 0;
        let error = copy_dir_all_with_check(&src, &tmp.path().join("dst"), &mut || {
            checks += 1;
            if checks >= 3 {
                Err(super::super::skill_process::PROCESS_CANCELLED_MESSAGE.to_string())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(
            error,
            super::super::skill_process::PROCESS_CANCELLED_MESSAGE
        );
        assert!(checks >= 3);
    }

    #[test]
    fn recursive_copy_checks_cancellation_between_file_chunks() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("large"), vec![7_u8; 192 * 1024]).unwrap();
        let dst = tmp.path().join("dst");
        let mut checks = 0;
        let error = copy_dir_all_with_check(&src, &dst, &mut || {
            checks += 1;
            if checks >= 4 {
                Err(super::super::skill_process::PROCESS_CANCELLED_MESSAGE.to_string())
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert_eq!(
            error,
            super::super::skill_process::PROCESS_CANCELLED_MESSAGE
        );
        assert_eq!(fs::metadata(dst.join("large")).unwrap().len(), 64 * 1024);
    }

    #[cfg(unix)]
    #[test]
    fn preserving_copy_keeps_relative_file_symlink_target() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("file.txt"), "inside").unwrap();
        symlink("file.txt", src.join("file-link")).unwrap();

        let dst = tmp.path().join("dst");
        copy_dir_preserving_symlinks(&src, &dst).unwrap();

        assert_eq!(
            fs::read_link(dst.join("file-link")).unwrap(),
            Path::new("file.txt")
        );
    }

    #[cfg(unix)]
    #[test]
    fn preserving_copy_keeps_relative_directory_symlink_without_traversing_it() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("directory")).unwrap();
        fs::write(src.join("directory/file.txt"), "inside").unwrap();
        symlink("directory", src.join("directory-link")).unwrap();

        let dst = tmp.path().join("dst");
        copy_dir_preserving_symlinks(&src, &dst).unwrap();

        assert!(fs::symlink_metadata(dst.join("directory-link"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_link(dst.join("directory-link")).unwrap(),
            Path::new("directory")
        );
    }

    #[cfg(unix)]
    #[test]
    fn preserving_copy_keeps_absolute_outside_symlink_without_copying_target() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside.txt");
        fs::write(&outside, "outside").unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        symlink(&outside, src.join("outside-link")).unwrap();

        let dst = tmp.path().join("dst");
        copy_dir_preserving_symlinks(&src, &dst).unwrap();

        assert_eq!(fs::read_link(dst.join("outside-link")).unwrap(), outside);
        fs::write(tmp.path().join("outside.txt"), "changed outside").unwrap();
        assert_eq!(
            fs::read_to_string(dst.join("outside-link")).unwrap(),
            "changed outside"
        );
    }

    #[cfg(unix)]
    #[test]
    fn preserving_copy_keeps_dangling_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        symlink("missing", src.join("dangling")).unwrap();

        let dst = tmp.path().join("dst");
        copy_dir_preserving_symlinks(&src, &dst).unwrap();

        assert_eq!(
            fs::read_link(dst.join("dangling")).unwrap(),
            Path::new("missing")
        );
    }

    #[cfg(unix)]
    #[test]
    fn preserving_copy_refuses_unix_socket() {
        use std::os::unix::net::UnixListener;

        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        let socket_path = src.join("special.socket");
        let _listener = UnixListener::bind(&socket_path).unwrap();

        let error = copy_dir_preserving_symlinks(&src, &tmp.path().join("dst")).unwrap_err();

        assert!(error.contains("Refused to copy unsupported special file"));
        assert!(error.contains("special.socket"));
    }

    // Review item 5: ported from the deleted `skill_add.rs` (unit 3.5c),
    // unchanged apart from calling `maybe_claude_code_symlink`/
    // `relative_path_between` directly instead of through `add_skill_with`.

    #[test]
    fn relative_path_between_walks_up_to_the_common_parent_or_names_the_wrong_relative_path() {
        assert_eq!(
            relative_path_between(
                Path::new("/h/.claude/skills"),
                Path::new("/h/.agents/skills")
            ),
            std::path::PathBuf::from("../../.agents/skills")
        );
        assert_eq!(
            relative_path_between(
                Path::new("/p/.claude/skills"),
                Path::new("/p/.agents/skills")
            ),
            std::path::PathBuf::from("../../.agents/skills")
        );
    }

    #[cfg(unix)]
    #[test]
    fn claude_code_symlink_created_only_for_a_real_directory_or_names_the_missing_link() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let shared = home.join(".agents/skills");
        let claude = home.join(".claude/skills");
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir_all(&claude).unwrap();

        let created = maybe_claude_code_symlink(
            &claude,
            &shared,
            "find-bugs",
            &[super::super::agents::AgentId::ClaudeCode],
        )
        .unwrap();

        let link = claude.join("find-bugs");
        assert_eq!(created, Some(link.to_string_lossy().to_string()));
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_link(&link).unwrap(),
            Path::new("../../.agents/skills/find-bugs")
        );
    }

    #[cfg(unix)]
    #[test]
    fn claude_code_symlink_skipped_when_claude_skills_is_the_whole_dir_symlink_or_names_the_created_link(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let shared = home.join(".agents/skills");
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir_all(home.join(".claude")).unwrap();
        let claude = home.join(".claude/skills");
        symlink(&shared, &claude).unwrap();

        let created = maybe_claude_code_symlink(
            &claude,
            &shared,
            "find-bugs",
            &[super::super::agents::AgentId::ClaudeCode],
        )
        .unwrap();

        // No per-skill symlink created - the whole-dir symlink already covers it.
        assert_eq!(created, None);
        assert!(!claude.join("find-bugs").exists());
    }
}
