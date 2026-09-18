// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Proves [`skill_studio_core::tree_hash::tree_hash`] against values git
//! itself produced, never against the module's own output.
//!
//! `fixtures/tree-hash/skills/*` and `fixtures/tree-hash/skill-lock.json`
//! were captured by copying each skill folder into its own scratch
//! directory and running `git init`, `git add -A`, `git write-tree` there;
//! the resulting SHAs went straight into the lock file fixture and into the
//! literal in the second test below. Regenerating them means re-running
//! that git sequence, never copying this module's output back into the
//! fixtures.

use std::fs;
use std::path::{Path, PathBuf};

use skill_studio_core::lock_file::SkillLockFile;
use skill_studio_core::tree_hash::tree_hash;

use skill_studio_host::RealFs;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/tree-hash")
}

#[test]
fn tree_hash_matches_the_skills_cli_lock_file_for_nine_fixture_skills_or_names_the_skill_with_a_mismatched_hash(
) {
    let base = fixtures_dir();
    let lock_bytes = fs::read(base.join("skill-lock.json")).expect("read fixture skill-lock.json");
    let lock: SkillLockFile =
        serde_json::from_slice(&lock_bytes).expect("parse fixture skill-lock.json");
    assert_eq!(
        lock.skills.len(),
        9,
        "fixture lock file must name nine skills, named the count instead"
    );

    let fs_adapter = RealFs::new();
    let mut mismatches = Vec::new();
    for (name, entry) in &lock.skills {
        let dir = base.join("skills").join(name);
        let actual = tree_hash(&fs_adapter, &dir).unwrap_or_else(|e| panic!("{name}: {e}"));
        if actual != entry.skill_folder_hash {
            mismatches.push(format!(
                "{name}: lock file has {}, computed {actual}",
                entry.skill_folder_hash
            ));
        }
    }
    assert!(mismatches.is_empty(), "mismatched skills: {mismatches:?}");
}

#[test]
fn tree_hash_matches_a_git_tree_sha_computed_by_git_for_the_same_folder_or_names_the_differing_byte(
) {
    // `nested`'s SHA below is `git write-tree`'s own output for a fresh
    // copy of `fixtures/tree-hash/skills/nested` - the same value recorded
    // in `skill-lock.json`, repeated here as a literal so this test does
    // not depend on that file parsing correctly.
    let dir = fixtures_dir().join("skills").join("nested");
    let fs_adapter = RealFs::new();
    let actual = tree_hash(&fs_adapter, &dir).expect("hash the nested fixture");
    assert_eq!(
        actual, "ad489fbfda3ca76c014a29ee02f1893069eccd35",
        "byte-for-byte mismatch against git's own write-tree output for `nested`"
    );
}

/// Writes `name` under `dir` with `bytes`, then backdates its mtime by
/// `days_old` days, so two builds of the same tree can carry deliberately
/// different mtimes.
fn write_aged(dir: &Path, name: &str, bytes: &[u8], days_old: u64) {
    let path = dir.join(name);
    fs::write(&path, bytes).expect("write fixture file");
    let stamp = std::time::SystemTime::now() - std::time::Duration::from_secs(days_old * 86_400);
    let file_time = filetime::FileTime::from_system_time(stamp);
    filetime::set_file_mtime(&path, file_time).expect("backdate mtime");
}

#[test]
fn tree_hash_ignores_file_order_and_mtimes_and_changes_only_when_content_or_names_change_or_names_the_false_positive_or_negative(
) {
    let fs_adapter = RealFs::new();

    // Same three files, written in reverse order and given different
    // mtimes: the hash must not move, since git trees are sorted by name
    // and carry no mtime.
    let forward_dir = tempfile::tempdir().expect("tempdir");
    write_aged(forward_dir.path(), "a.txt", b"alpha", 10);
    write_aged(forward_dir.path(), "b.txt", b"beta", 5);
    write_aged(forward_dir.path(), "c.txt", b"gamma", 1);
    let forward_hash = tree_hash(&fs_adapter, forward_dir.path()).unwrap();

    let reverse_dir = tempfile::tempdir().expect("tempdir");
    write_aged(reverse_dir.path(), "c.txt", b"gamma", 1);
    write_aged(reverse_dir.path(), "b.txt", b"beta", 5);
    write_aged(reverse_dir.path(), "a.txt", b"alpha", 10);
    let reverse_hash = tree_hash(&fs_adapter, reverse_dir.path()).unwrap();
    assert_eq!(
        forward_hash, reverse_hash,
        "false negative: write order and mtime alone changed the hash"
    );

    // Changing one file's content must change the hash.
    let edited_dir = tempfile::tempdir().expect("tempdir");
    write_aged(edited_dir.path(), "a.txt", b"ALPHA-EDITED", 10);
    write_aged(edited_dir.path(), "b.txt", b"beta", 5);
    write_aged(edited_dir.path(), "c.txt", b"gamma", 1);
    let edited_hash = tree_hash(&fs_adapter, edited_dir.path()).unwrap();
    assert_ne!(
        forward_hash, edited_hash,
        "false positive: editing a.txt's content left the hash unchanged"
    );

    // Renaming a file (same bytes, different name) must change the hash
    // too, since the name is part of the tree entry.
    let renamed_dir = tempfile::tempdir().expect("tempdir");
    write_aged(renamed_dir.path(), "a-renamed.txt", b"alpha", 10);
    write_aged(renamed_dir.path(), "b.txt", b"beta", 5);
    write_aged(renamed_dir.path(), "c.txt", b"gamma", 1);
    let renamed_hash = tree_hash(&fs_adapter, renamed_dir.path()).unwrap();
    assert_ne!(
        forward_hash, renamed_hash,
        "false positive: renaming a.txt left the hash unchanged"
    );
}
