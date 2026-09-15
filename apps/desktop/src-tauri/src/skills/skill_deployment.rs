pub use skill_studio_core::skill_deployment::*;

use std::path::{Path, PathBuf};

/// The Universal skills directory for `scope` under `home` or `project`.
pub fn universal_skills_dir(
    home: &Path,
    scope: InstallScope,
    project_path: Option<&Path>,
) -> PathBuf {
    match scope {
        InstallScope::Global => home.join(".agents").join("skills"),
        InstallScope::Project => project_path
            .unwrap_or_else(|| Path::new(""))
            .join(".agents")
            .join("skills"),
    }
}
