// ============================================================================
// Skills Module - Test Support
// Fixture helpers shared across the skills module's inline test suites
// ============================================================================

use std::fs;
use std::path::Path;

/// Writes a minimal spec-valid `SKILL.md` at `dir/SKILL.md`, named `name`.
#[cfg(test)]
pub(crate) fn write_skill(dir: &Path, name: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: test\n---\nBody."),
    )
    .unwrap();
}
