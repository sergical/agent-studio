use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSkillEntry {
    pub source: String,
    pub source_type: String,
    pub computed_hash: String,
    pub source_url: Option<String>,
    pub skill_path: Option<String>,
    #[serde(rename = "ref")]
    pub source_ref: Option<String>,
    pub subagents: Option<Vec<String>>,
    pub well_known_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSkillLock {
    pub skills: BTreeMap<String, ProjectSkillEntry>,
}

#[derive(Debug)]
pub enum ProjectLockError {
    InvalidSchema(serde_json::Error),
    UnsupportedVersion(u64),
}

impl std::fmt::Display for ProjectLockError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSchema(error) => {
                write!(formatter, "invalid project skill ledger: {error}")
            }
            Self::UnsupportedVersion(version) => {
                write!(
                    formatter,
                    "unsupported project skill ledger version: {version}"
                )
            }
        }
    }
}

impl std::error::Error for ProjectLockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidSchema(error) => Some(error),
            Self::UnsupportedVersion(_) => None,
        }
    }
}

/// Decodes project schema v1 without resolving local sources or inventing global metadata.
/// The caller supplies bounded bytes and retains the source path and read failures.
pub fn parse_project_lock(bytes: &[u8]) -> Result<ProjectSkillLock, ProjectLockError> {
    #[derive(Deserialize)]
    struct Envelope {
        version: u64,
        skills: BTreeMap<String, ProjectSkillEntry>,
    }

    let envelope: Envelope =
        serde_json::from_slice(bytes).map_err(ProjectLockError::InvalidSchema)?;
    if envelope.version != 1 {
        return Err(ProjectLockError::UnsupportedVersion(envelope.version));
    }
    Ok(ProjectSkillLock {
        skills: envelope.skills,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_free_project_entry_preserves_local_source_and_hash() {
        let lock = parse_project_lock(
            br#"{"version":1,"skills":{"example":{"source":"../local-skill","sourceType":"local","computedHash":"project-content-hash"}}}"#,
        )
        .unwrap();
        let entry = &lock.skills["example"];
        assert_eq!(entry.source, "../local-skill");
        assert_eq!(entry.source_type, "local");
        assert_eq!(entry.computed_hash, "project-content-hash");
        assert_eq!(entry.source_url, None);
        assert_eq!(entry.skill_path, None);
        assert_eq!(entry.source_ref, None);
    }

    #[test]
    fn optional_reinstall_fields_are_preserved() {
        let lock = parse_project_lock(
            br#"{"version":1,"skills":{"example":{"source":"owner/repo","sourceType":"github","computedHash":"content","sourceUrl":"https://github.com/owner/repo","skillPath":"skills/example/SKILL.md","ref":"release","subagents":["","review"],"wellKnownDigest":"digest"}}}"#,
        )
        .unwrap();
        let entry = &lock.skills["example"];
        assert_eq!(
            entry.source_url.as_deref(),
            Some("https://github.com/owner/repo")
        );
        assert_eq!(entry.skill_path.as_deref(), Some("skills/example/SKILL.md"));
        assert_eq!(entry.source_ref.as_deref(), Some("release"));
        assert_eq!(entry.subagents, Some(vec![String::new(), "review".into()]));
        assert_eq!(entry.well_known_digest.as_deref(), Some("digest"));
    }

    #[test]
    fn unsupported_versions_are_explicit() {
        for version in [0, 2, 3, u64::MAX] {
            let bytes = format!(r#"{{"version":{version},"skills":{{}}}}"#);
            assert!(matches!(
                parse_project_lock(bytes.as_bytes()),
                Err(ProjectLockError::UnsupportedVersion(found)) if found == version
            ));
        }
    }

    #[test]
    fn malformed_input_never_becomes_an_empty_ledger() {
        for bytes in [
            b"not json".as_slice(),
            br#"{"version":1}"#,
            br#"{"version":"1","skills":{}}"#,
            br#"{"version":1,"skills":[]}"#,
            br#"{"version":1,"skills":{"x":{"source":"o/r","sourceType":"github"}}}"#,
            br#"{"version":1,"skills":{"x":{"source":false,"sourceType":"github","computedHash":"hash"}}}"#,
            br#"{"version":1,"skills":{"x":{"source":"o/r","sourceType":"github","computedHash":"hash","subagents":"review"}}}"#,
        ] {
            assert!(matches!(
                parse_project_lock(bytes),
                Err(ProjectLockError::InvalidSchema(_))
            ));
        }
        assert!(parse_project_lock(br#"{"version":1,"skills":{}}"#)
            .unwrap()
            .skills
            .is_empty());
    }
}
