// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! `skill-studio --version` prints the crate version and the build commit
//! hash on one line, so a bug report can quote a single line.

use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_skill-studio")
}

#[test]
fn cli_version_prints_version_and_commit_hash_in_one_line() {
    let output = Command::new(bin())
        .arg("--version")
        .output()
        .expect("skill-studio --version should run");
    assert!(output.status.success(), "--version should exit 0");

    let stdout = String::from_utf8(output.stdout).expect("--version output should be UTF-8");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "expected one line, got: {stdout:?}");

    let line = lines[0];
    assert!(
        line.contains(env!("CARGO_PKG_VERSION")),
        "line should contain the crate version, got: {line:?}"
    );

    // "skill-studio <version> (<commit>)" - the commit is "dev" outside a
    // release build (no SKILL_STUDIO_COMMIT set), or a hex git SHA.
    let commit = line
        .rsplit_once('(')
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .unwrap_or_else(|| panic!("line should end with a (<commit>), got: {line:?}"));
    let is_dev = commit == "dev";
    let is_hex_hash = !commit.is_empty() && commit.chars().all(|c| c.is_ascii_hexdigit());
    assert!(
        is_dev || is_hex_hash,
        "commit should be \"dev\" or a hex hash, got: {commit:?}"
    );
}
