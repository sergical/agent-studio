//! The built `skill-studio` CLI binary's path, shared by every parity test
//! in this directory that drives the CLI surface as a subprocess. A test
//! file includes it with `mod cli_binary;`.

use std::path::PathBuf;

/// Path to the `skill-studio` CLI binary. `env!("CARGO_BIN_EXE_skill-studio")`
/// isn't an option: Cargo only sets `CARGO_BIN_EXE_<name>` for binaries of
/// the crate under test, or a foreign crate's binary pulled in as an
/// unstable "artifact dependency" (`-Zbindeps`, nightly-only as of this
/// workspace's toolchain) - `apps/cli` is a separate workspace member with
/// no lib target, so it can't be an ordinary `dev-dependency` either.
/// This derives the build output dir the same way Cargo does:
/// `$CARGO_TARGET_DIR` when set (CI and any developer override), else the
/// workspace root's `target/`, found by walking up from
/// `CARGO_MANIFEST_DIR` to the directory that has the workspace
/// `Cargo.toml`, rather than a fixed `../../../` hop that would silently
/// point at the wrong tree if this file ever moves.
/// `cargo test --workspace` builds every member, `apps/cli` included,
/// before running any test, so the binary exists by the time this runs; a
/// standalone `cargo test -p skill-studio` needs `cargo build -p
/// skill-studio-cli` run first.
pub fn cli_binary_path() -> PathBuf {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let target_dir = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => workspace_root().join("target"),
    };
    target_dir.join(profile).join("skill-studio")
}

/// Walks up from `CARGO_MANIFEST_DIR` (`apps/desktop/src-tauri`) to the
/// nearest ancestor containing a `Cargo.toml` with `[workspace]`, so the
/// default target-dir fallback tracks the repo layout instead of a
/// hardcoded hop count.
fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join("Cargo.toml");
        if candidate.is_file() {
            let contents = std::fs::read_to_string(&candidate).unwrap_or_default();
            if contents.contains("[workspace]") {
                return dir;
            }
        }
        assert!(
            dir.pop(),
            "no workspace Cargo.toml found above {}",
            env!("CARGO_MANIFEST_DIR")
        );
    }
}
