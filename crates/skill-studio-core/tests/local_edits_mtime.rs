#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! `local_edits` against real files, because only a real filesystem reports
//! mtimes. The lock records the hash of a folder that also held an upstream
//! `metadata.json`, which the CLI never copies - the one case where a hash
//! mismatch is not an edit.

use skill_studio_core::lock_file::{local_edits, LocalEdits, SkillLockFile};
use skill_studio_host::RealFs;

fn lock(updated_at: &str) -> SkillLockFile {
    let json = format!(
        r#"{{"version":3,"skills":{{"s":{{"source":"o/r","sourceType":"github","sourceUrl":"u",
        "skillFolderHash":"{}","installedAt":"{updated_at}","updatedAt":"{updated_at}"}}}}}}"#,
        "1".repeat(40)
    );
    serde_json::from_str(&json).unwrap()
}

fn skill_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("SKILL.md"), "body\n").unwrap();
    dir
}

#[test]
fn a_mismatch_with_no_file_newer_than_the_install_reads_as_unknown_or_upstream_only_files_warn() {
    let dir = skill_dir();
    assert_eq!(
        local_edits(
            &RealFs::new(),
            &lock("2099-01-01T00:00:00.000Z"),
            "s",
            dir.path()
        ),
        LocalEdits::Unknown
    );
}

#[test]
fn a_mismatch_with_a_file_newer_than_the_install_reads_as_edited_or_a_real_edit_is_missed() {
    let dir = skill_dir();
    assert_eq!(
        local_edits(
            &RealFs::new(),
            &lock("2000-01-01T00:00:00.000Z"),
            "s",
            dir.path()
        ),
        LocalEdits::Edited
    );
}
