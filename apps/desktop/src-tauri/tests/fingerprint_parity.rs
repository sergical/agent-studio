//! Pins the host crate's content hash to the desktop's `fingerprint_path`.
//!
//! Backup manifests written by the shipped desktop app carry desktop
//! fingerprints. A restore performed through the shared core compares those
//! stored values against freshly computed ones, so any divergence between the
//! two hash implementations would report drift on untouched files and block
//! every pre-existing event from being restored.

use std::fs;
use std::os::unix::fs::symlink;

use skill_studio_lib::event_store::fingerprint_path;

#[test]
fn host_hash_matches_desktop_fingerprint() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();

    fs::write(root.join("plain.md"), b"# hello\nbody\n").expect("write file");
    fs::write(root.join("empty.md"), b"").expect("write empty file");
    fs::create_dir_all(root.join("tree/nested")).expect("create tree");
    fs::write(root.join("tree/a.md"), b"alpha").expect("write a");
    fs::write(root.join("tree/nested/b.md"), b"beta").expect("write b");
    symlink("../plain.md", root.join("tree/link.md")).expect("create symlink");

    let targets = [
        root.join("plain.md"),
        root.join("empty.md"),
        root.join("tree/a.md"),
        root.join("tree/link.md"),
        root.join("tree/nested"),
        root.join("tree"),
        root.to_path_buf(),
    ];

    for target in targets {
        let host = skill_studio_host::hash_entry(&target).expect("host hash");
        let desktop = fingerprint_path(&target);
        assert_eq!(host, desktop, "hash mismatch for {}", target.display());
    }
}

#[test]
fn desktop_reports_absent_for_a_missing_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let missing = dir.path().join("nope.md");
    assert_eq!(fingerprint_path(&missing), "absent");
    assert!(skill_studio_host::hash_entry(&missing).is_err());
}
