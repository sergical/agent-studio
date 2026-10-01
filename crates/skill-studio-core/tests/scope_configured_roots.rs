#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A configured Codex or `OpenCode` root that resolves to a filesystem root
//! or an ancestor of the home must be discarded, or `contains` stops
//! confining writes. The fixture fs does not resolve `..` or real symlinks,
//! so these run against the real filesystem.

use std::path::{Path, PathBuf};

use skill_studio_core::scope::{NormalizedScope, RuntimeScope};
use skill_studio_host::RealFs;

fn normalize_with(home: &Path, codex_home: &Path, opencode_root: &Path) -> NormalizedScope {
    let mut raw = RuntimeScope::fixture(home);
    raw.codex_home = Some(codex_home.to_path_buf());
    raw.opencode_config_root = Some(opencode_root.to_path_buf());
    NormalizedScope::normalize(&raw, &RealFs::new()).expect("normalize scope")
}

fn assert_defaults_and_confined(scope: &NormalizedScope, home: &Path, what: &str) {
    assert!(
        !scope.contains(Path::new("/etc/passwd")),
        "{what} must not make /etc/passwd part of the scope"
    );
    assert_eq!(
        scope.global_root_path(Path::new(".codex/skills")),
        home.join(".codex/skills"),
        "{what}: codex_home must fall back to the default"
    );
    assert_eq!(
        scope.global_root_path(Path::new(".config/opencode/skills")),
        home.join(".config/opencode/skills"),
        "{what}: opencode root must fall back to the default"
    );
}

#[test]
fn a_configured_root_spelled_with_dotdot_that_resolves_to_slash_is_discarded() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().canonicalize().unwrap().join("home");
    std::fs::create_dir_all(&home).unwrap();

    for bad in ["/usr/..", "/.."] {
        let bad = PathBuf::from(bad);
        let scope = normalize_with(&home, &bad, &bad);
        assert_defaults_and_confined(&scope, &home, &format!("root {bad:?}"));
    }
}

#[cfg(unix)]
#[test]
fn a_configured_root_that_is_a_symlink_to_slash_or_a_home_ancestor_is_discarded() {
    let dir = tempfile::tempdir().expect("temp dir");
    let base = dir.path().canonicalize().unwrap();
    let home = base.join("outer/home");
    std::fs::create_dir_all(&home).unwrap();

    for (name, target) in [
        ("to-slash", PathBuf::from("/")),
        ("to-ancestor", base.join("outer")),
    ] {
        let link = base.join(name);
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let scope = normalize_with(&home, &link, &link);
        assert_defaults_and_confined(&scope, &home, &format!("symlink {name} -> {target:?}"));
    }
}
