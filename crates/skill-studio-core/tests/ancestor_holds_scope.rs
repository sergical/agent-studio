//! `ScopeFs::ancestor_holds` must stop climbing at the scope root, not the
//! filesystem root - the deliberate divergence from the desktop's own
//! `.git` walk (see `docs/spec-core-primitives.md` section 13.2), which has
//! no scope to stop at and climbs to the real filesystem root instead.

use skill_studio_core::ports::{ScopeFs, ScopedReads};
use skill_studio_core::scope::{NormalizedScope, RuntimeScope};

use skill_studio_host::RealFs;

#[test]
fn ancestor_holds_stops_at_the_scope_root() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().canonicalize().expect("canonicalize temp dir");

    // A `.git` directory above the scope's home - never visible through a
    // scope-bounded walk.
    std::fs::create_dir_all(root.join(".git")).expect("create outer .git");

    let home = root.join("home");
    std::fs::create_dir_all(&home).expect("create home");

    let fs = RealFs::new();
    let raw_scope = RuntimeScope::fixture(&home);
    let scope = NormalizedScope::normalize(&raw_scope, &fs).expect("normalize scope");
    let scoped = ScopedReads::new(&fs, &scope);

    assert!(
        !scoped
            .ancestor_holds(&home, ".git")
            .expect("ancestor_holds must not error"),
        "a `.git` above the scope root must not be visible"
    );

    // Once `.git` is inside the scope's home, the same walk finds it.
    std::fs::create_dir_all(home.join(".git")).expect("create inner .git");
    assert!(
        scoped
            .ancestor_holds(&home, ".git")
            .expect("ancestor_holds must not error"),
        "a `.git` inside the scope root must be visible"
    );
}

#[test]
fn ancestor_holds_finds_an_entry_on_an_in_scope_ancestor() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().canonicalize().expect("canonicalize temp dir");

    std::fs::create_dir_all(home.join(".git")).expect("create .git");
    let skill_dir = home.join("nested/skill");
    std::fs::create_dir_all(&skill_dir).expect("create nested skill dir");

    let fs = RealFs::new();
    let raw_scope = RuntimeScope::fixture(&home);
    let scope = NormalizedScope::normalize(&raw_scope, &fs).expect("normalize scope");
    let scoped = ScopedReads::new(&fs, &scope);

    assert!(scoped
        .ancestor_holds(&skill_dir, ".git")
        .expect("ancestor_holds must not error"));

    // Sanity: unrelated names are absent.
    assert!(!scoped
        .ancestor_holds(&skill_dir, "does-not-exist")
        .expect("ancestor_holds must not error"));
}
