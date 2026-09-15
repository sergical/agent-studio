use std::path::Path;

use serde::{Deserialize, Serialize};

/// The operation that produced a coverage observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoveryExtent {
    Full,
    Named,
}

/// The result of reading one requested inventory source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceReadOutcome {
    Read,
    Absent,
    Incomplete,
    Failed,
}

/// A root whose membership was checked by the scanner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MembershipSource {
    AgentRoot,
    DisabledRoot,
    PluginCache,
}

/// Membership and fact coverage are deliberately separate. A source can
/// establish its candidate set while content or provenance remains unknown.
#[derive(Debug, Clone, Serialize)]
pub struct SourceCoverage {
    pub path: std::path::PathBuf,
    pub source: MembershipSource,
    pub extent: DiscoveryExtent,
    pub membership: SourceReadOutcome,
    pub facts: SourceReadOutcome,
}

/// A read problem observed while discovery scans configured skill roots and
/// plugin caches. An empty list does not prove the complete inventory:
/// ownership data has separate coverage and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoveryReadIssueKind {
    Root,
    Entry,
    Metadata,
    GitScopeBoundary,
    SkillDocument,
    Resource,
    Cap,
    Tokenizer,
    PluginManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryReadIssue {
    pub kind: DiscoveryReadIssueKind,
    pub path: String,
    pub message: String,
}

impl DiscoveryReadIssue {
    pub fn new(kind: DiscoveryReadIssueKind, path: &Path, message: impl Into<String>) -> Self {
        Self {
            kind,
            path: path.to_string_lossy().into_owned(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_uses_the_kebab_case_transport_shape() {
        let issue = DiscoveryReadIssue::new(
            DiscoveryReadIssueKind::SkillDocument,
            Path::new("/tmp/skill/SKILL.md"),
            "could not read",
        );
        let value = serde_json::to_value(issue).unwrap();
        assert_eq!(value["kind"], "skill-document");
        assert_eq!(value["path"], "/tmp/skill/SKILL.md");
        assert_eq!(value["message"], "could not read");
    }
}
