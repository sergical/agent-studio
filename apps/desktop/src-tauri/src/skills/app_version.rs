// ============================================================================
// Skills Module - app_version
// The running app's own version, build commit, and release notes, so the
// Settings "Version" row and "What's new" panel need no network round trip.
// ============================================================================

use std::fmt;

use serde::{Deserialize, Serialize};

/// The changelog bundled at compile time via `include_str!`, one `## v<x>`
/// section per release - see `../../../../../CHANGELOG.md` (repository root).
const CHANGELOG: &str = include_str!("../../../../../CHANGELOG.md");

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppVersion {
    pub version: String,
    pub commit: String,
    pub notes: Option<String>,
}

/// `changelog_section` found no `## v<version>` heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingChangelogSection {
    version: String,
}

impl fmt::Display for MissingChangelogSection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CHANGELOG.md has no section for v{}", self.version)
    }
}

impl std::error::Error for MissingChangelogSection {}

/// The Markdown body of a changelog's `## v<version>` section, up to (but
/// not including) the next `## ` heading or the end of the file.
pub fn changelog_section(
    changelog: &str,
    version: &str,
) -> Result<String, MissingChangelogSection> {
    let heading = format!("## v{version}");
    let start = changelog
        .find(&heading)
        .ok_or_else(|| MissingChangelogSection {
            version: version.to_string(),
        })?;
    let body_start = start + heading.len();
    let rest = &changelog[body_start..];
    let body_end = rest.find("\n## ").unwrap_or(rest.len());
    Ok(rest[..body_end].trim().to_string())
}

#[tauri::command]
pub fn app_version() -> AppVersion {
    let version = env!("CARGO_PKG_VERSION").to_string();
    let notes = changelog_section(CHANGELOG, &version).ok();
    AppVersion {
        version,
        commit: env!("SKILL_STUDIO_COMMIT").to_string(),
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changelog_section_for_the_running_version_is_found_or_the_missing_version_is_named() {
        let version = env!("CARGO_PKG_VERSION");
        let found = changelog_section(CHANGELOG, version);
        assert!(
            found.is_ok(),
            "CHANGELOG.md has no `## v{version}` section for the crate's own version"
        );

        let fixture = "## Unreleased\n\nnothing yet\n";
        let err = changelog_section(fixture, version).expect_err("fixture has no section");
        assert!(
            err.to_string().contains(version),
            "error message should name the missing version, got: {err}"
        );
    }
}
