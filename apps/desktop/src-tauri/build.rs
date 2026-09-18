fn main() {
    // The release workflow sets `SKILL_STUDIO_COMMIT` to the release SHA; a
    // plain `cargo build` (dev, CI) leaves it unset, so the app falls back to
    // "dev" instead of shelling out to git.
    let commit = std::env::var("SKILL_STUDIO_COMMIT").unwrap_or_else(|_| "dev".to_string());
    println!("cargo:rustc-env=SKILL_STUDIO_COMMIT={commit}");
    println!("cargo:rerun-if-env-changed=SKILL_STUDIO_COMMIT");

    tauri_build::build()
}
