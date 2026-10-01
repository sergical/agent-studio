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

/// Backdates `path` to 2000, as `cp -p`, `rsync -a`, unzip and `mv` leave it.
fn backdate(path: &std::path::Path) {
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(946_684_800);
    std::fs::File::open(path)
        .unwrap()
        .set_modified(old)
        .unwrap();
}

/// An install an hour ago: files and folder all older than the lock entry.
fn installed_dir(files: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for name in files {
        std::fs::write(dir.path().join(name), "body\n").unwrap();
        backdate(&dir.path().join(name));
    }
    backdate(dir.path());
    dir
}

fn an_hour_ago() -> String {
    (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339()
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

#[test]
fn an_untouched_install_with_a_hash_mismatch_reads_as_unknown_or_the_control_proves_nothing() {
    let dir = installed_dir(&["SKILL.md"]);
    assert_eq!(
        local_edits(&RealFs::new(), &lock(&an_hour_ago()), "s", dir.path()),
        LocalEdits::Unknown
    );
}

#[test]
fn an_added_file_with_an_old_mtime_reads_as_edited_or_update_deletes_it_silently() {
    let dir = installed_dir(&["SKILL.md"]);
    std::fs::write(dir.path().join("notes.md"), "mine\n").unwrap();
    backdate(&dir.path().join("notes.md"));
    assert_eq!(
        local_edits(&RealFs::new(), &lock(&an_hour_ago()), "s", dir.path()),
        LocalEdits::Edited
    );
}

#[test]
fn a_deleted_file_reads_as_edited_or_update_restores_it_without_asking() {
    let dir = installed_dir(&["SKILL.md", "extra.md"]);
    std::fs::remove_file(dir.path().join("extra.md")).unwrap();
    assert_eq!(
        local_edits(&RealFs::new(), &lock(&an_hour_ago()), "s", dir.path()),
        LocalEdits::Edited
    );
}

#[test]
fn a_file_added_in_a_subfolder_reads_as_edited_or_only_the_top_folder_is_watched() {
    let dir = installed_dir(&["SKILL.md"]);
    let sub = dir.path().join("refs");
    std::fs::create_dir(&sub).unwrap();
    backdate(&sub);
    backdate(dir.path());
    std::fs::write(sub.join("a.md"), "mine\n").unwrap();
    backdate(&sub.join("a.md"));
    assert_eq!(
        local_edits(&RealFs::new(), &lock(&an_hour_ago()), "s", dir.path()),
        LocalEdits::Edited
    );
}

#[test]
fn an_untouched_install_seen_through_a_new_agent_link_reads_as_unknown_or_relinking_warns() {
    let dir = installed_dir(&["SKILL.md"]);
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("s");
    std::os::unix::fs::symlink(dir.path(), &link).unwrap();
    assert_eq!(
        local_edits(&RealFs::new(), &lock(&an_hour_ago()), "s", &link),
        LocalEdits::Unknown
    );
}
