// ============================================================================
// Skills Module - skill_fs
// One shared directory-copy routine, reused everywhere a skill's files need
// to become a second, independent copy: `skill_fork`'s fork/pull snapshots,
// `skill_add`'s "Copy" method, `skill_trial`'s trash copy, and
// `skill_pack`'s bundling of manual/fork skills into a pack directory.
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

pub(crate) fn copy_dir_preserving_symlinks_controlled_bounded(
    src: &Path,
    dst: &Path,
    control: &super::skill_process::AddOperationControl,
    max_bytes: usize,
    max_entries: usize,
    max_depth: usize,
) -> Result<(), String> {
    struct Budget {
        bytes: usize,
        entries: usize,
        max_bytes: usize,
        max_entries: usize,
        max_depth: usize,
    }
    fn copy(
        src: &Path,
        dst: &Path,
        depth: usize,
        budget: &mut Budget,
        control: &super::skill_process::AddOperationControl,
    ) -> Result<(), String> {
        let (max_bytes, max_entries, max_depth) =
            (budget.max_bytes, budget.max_entries, budget.max_depth);
        control.check_message()?;
        if depth > max_depth {
            return Err(format!(
                "Skill staging exceeded {max_depth} directory depth limit"
            ));
        }
        fs::create_dir_all(dst).map_err(|e| format!("Failed to create {}: {e}", dst.display()))?;
        for entry in
            fs::read_dir(src).map_err(|e| format!("Failed to read {}: {e}", src.display()))?
        {
            control.check_message()?;
            budget.entries += 1;
            if budget.entries > max_entries {
                return Err(format!("Skill staging exceeded {max_entries} entry limit"));
            }
            let entry = entry.map_err(|e| format!("Failed to read a directory entry: {e}"))?;
            let source = entry.path();
            let destination = dst.join(entry.file_name());
            let kind = entry
                .file_type()
                .map_err(|e| format!("Failed to stat {}: {e}", source.display()))?;
            if kind.is_symlink() {
                create_symlink(
                    &fs::read_link(&source)
                        .map_err(|e| format!("Failed to read symlink {}: {e}", source.display()))?,
                    &destination,
                )?;
            } else if kind.is_dir() {
                copy(&source, &destination, depth + 1, budget, control)?;
            } else if kind.is_file() {
                let mut options = fs::OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
                }
                let mut input = options
                    .open(&source)
                    .map_err(|e| format!("Failed to open {}: {e}", source.display()))?;
                if !input
                    .metadata()
                    .map_err(|e| format!("Failed to stat {}: {e}", source.display()))?
                    .is_file()
                {
                    return Err(format!(
                        "Refused to stage non-regular file: {}",
                        source.display()
                    ));
                }
                let mut output = fs::File::create(&destination)
                    .map_err(|e| format!("Failed to create {}: {e}", destination.display()))?;
                let mut buffer = [0_u8; 64 * 1024];
                loop {
                    control.check_message()?;
                    let count = input
                        .read(&mut buffer)
                        .map_err(|e| format!("Failed to read {}: {e}", source.display()))?;
                    if count == 0 {
                        break;
                    }
                    if count > max_bytes.saturating_sub(budget.bytes) {
                        return Err(format!("Skill staging exceeded {max_bytes} byte limit"));
                    }
                    output
                        .write_all(&buffer[..count])
                        .map_err(|e| format!("Failed to write {}: {e}", destination.display()))?;
                    budget.bytes += count;
                }
                output
                    .set_permissions(input.metadata().map_err(|e| e.to_string())?.permissions())
                    .map_err(|e| format!("Failed to retain staging permissions: {e}"))?;
            } else {
                return Err(format!(
                    "Refused to stage unsupported special file: {}",
                    source.display()
                ));
            }
        }
        fs::set_permissions(
            dst,
            fs::metadata(src).map_err(|e| e.to_string())?.permissions(),
        )
        .map_err(|e| format!("Failed to retain staging directory permissions: {e}"))?;
        Ok(())
    }
    copy(
        src,
        dst,
        0,
        &mut Budget {
            bytes: 0,
            entries: 0,
            max_bytes,
            max_entries,
            max_depth,
        },
        control,
    )
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
    let mut buffer = [0_u8; 64 * 1024];
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[test]
    fn bounded_staging_refuses_each_limit_and_retains_link_and_mode() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("source");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::write(src.join("script"), b"body").unwrap();
        let control = super::super::skill_process::AddOperationControl::bounded_default();
        for (i, limits) in [(0, (0, 10, 4)), (1, (100, 0, 4)), (2, (100, 10, 0))] {
            let destination = temp.path().join(format!("refused-{i}"));
            assert!(copy_dir_preserving_symlinks_controlled_bounded(
                &src,
                &destination,
                &control,
                limits.0,
                limits.1,
                limits.2
            )
            .is_err());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(src.join("script"), fs::Permissions::from_mode(0o755)).unwrap();
            symlink("missing", src.join("dangling")).unwrap();
        }
        let destination = temp.path().join("accepted");
        copy_dir_preserving_symlinks_controlled_bounded(&src, &destination, &control, 100, 10, 4)
            .unwrap();
        assert_eq!(fs::read(destination.join("script")).unwrap(), b"body");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(destination.join("script"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
            assert_eq!(
                fs::read_link(destination.join("dangling")).unwrap(),
                Path::new("missing")
            );
        }
    }

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
}
