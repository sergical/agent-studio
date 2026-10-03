//! Where a parked copy lives, keyed by where it came from.
//!
//! Every parked copy sits under the global `~/.agents/skills-parked/`:
//!
//! - `universal/<name>`: the global `~/.agents/skills/<name>`;
//! - `<agent-id>/<name>`: a global agent folder (`~/.codex/skills/<name>`);
//! - `projects/<project-key>/<universal|agent-id>/<name>`: a project copy.
//!
//! The old flat `<name>` layout (global Universal only) is still scanned and
//! unparked; nothing migrates it.
//!
//! A project key hashes the project's canonical path, so it cannot be turned
//! back into the path. `projects/<project-key>/.origin` holds the path for the
//! scan, which has no history store to ask.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::identity::{RootKind, RootRef, RootScope};

/// Directory under the parked root that holds project copies.
pub(crate) const PARKED_PROJECTS_DIR: &str = "projects";
/// File, inside a project's parked directory, naming the project.
pub(crate) const PROJECT_ORIGIN_MARKER: &str = ".origin";
const UNIVERSAL_SLOT: &str = "universal";

/// The slot directory for an origin root kind, or `None` for a kind that
/// cannot be parked (legacy folders and plugin caches).
pub(crate) fn slot_for(kind: &RootKind) -> Option<String> {
    match kind {
        RootKind::Universal => Some(UNIVERSAL_SLOT.to_string()),
        RootKind::Harness(id) => Some(id.as_str().to_string()),
        RootKind::Parked | RootKind::Legacy(_) | RootKind::PluginCache(_) => None,
    }
}

/// `<sanitized basename>-<first 12 hex of sha256 of the canonical path>`.
pub(crate) fn project_key(canonical_project: &Path) -> String {
    let basename: String = canonical_project
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let basename = if basename.trim_matches('-').is_empty() {
        "project".to_string()
    } else {
        basename
    };
    let digest = Sha256::digest(canonical_project.to_string_lossy().as_bytes());
    let hex = digest.iter().take(6).fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    });
    format!("{basename}-{hex}")
}

/// The directory `origin`'s parked copies live in, under `parked_root`.
/// `project_key` is required for a project origin. `None` for an origin that
/// cannot be parked.
pub(crate) fn parked_slot_dir(
    parked_root: &Path,
    origin: &RootRef,
    project_key: Option<&str>,
) -> Option<PathBuf> {
    let slot = slot_for(&origin.kind)?;
    match (&origin.scope, project_key) {
        (RootScope::Global, _) => Some(parked_root.join(slot)),
        (RootScope::Project(_), Some(key)) => {
            Some(parked_root.join(PARKED_PROJECTS_DIR).join(key).join(slot))
        }
        (RootScope::Project(_), None) => None,
    }
}
