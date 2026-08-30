// ============================================================================
// Skills Module - skill_fs
// One shared directory-copy routine, reused everywhere a skill's files need
// to become a second, independent copy: `skill_fork`'s fork/pull snapshots,
// `skill_add`'s "Copy" method, `skill_trial`'s trash copy, and
// `skill_pack`'s bundling of manual/fork skills into a pack directory.
// ============================================================================

use std::fs;
use std::path::Path;

/// Recursively copies `src` into `dst`, creating `dst` if needed. Symlinks
/// are skipped - every caller here wants a plain, self-contained tree, not a
/// copy that can point back outside itself.
pub(crate) fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("Failed to create {}: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("Failed to read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("Failed to read a directory entry: {e}"))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("Failed to stat {}: {e}", entry.path().display()))?;
        let dest_path = dst.join(entry.file_name());
        if file_type.is_symlink() {
            continue;
        } else if file_type.is_dir() {
            copy_dir_all(&entry.path(), &dest_path)?;
        } else {
            fs::copy(entry.path(), &dest_path)
                .map_err(|e| format!("Failed to copy {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_files_and_skips_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("SKILL.md"), "hello").unwrap();
        fs::write(src.join("sub/file.txt"), "world").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("SKILL.md", src.join("link.md")).unwrap();

        let dst = tmp.path().join("dst");
        copy_dir_all(&src, &dst).unwrap();

        assert_eq!(fs::read_to_string(dst.join("SKILL.md")).unwrap(), "hello");
        assert_eq!(
            fs::read_to_string(dst.join("sub/file.txt")).unwrap(),
            "world"
        );
        assert!(!dst.join("link.md").exists());
    }
}
