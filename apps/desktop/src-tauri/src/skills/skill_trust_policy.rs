// ============================================================================
// Skills Module - skill_trust_policy
// Explicit trust for a later dotagents Add Skill retry. Confirmation records
// a normalized repository identity through this seam; Add Skill never trusts
// arbitrary CLI stderr, and never records trust without the user clicking
// "Trust repository and retry".
// ============================================================================

use std::path::Path;
use std::{error::Error, fmt};

use super::skill_dto::{ParsedSkillSource, ParsedSkillSourceKind};
use super::skill_fork_registry::{read_fork_registry, write_fork_registry};

/// Unique prefix so an untrusted-source error greps back to this module.
pub const UNTRUSTED_DOTAGENTS_SOURCE_PREFIX: &str = "Untrusted dotagents source";

/// User-safe copy shown when a source is not yet trusted.
pub const UNTRUSTED_DOTAGENTS_SOURCE_MESSAGE: &str =
    "This repository is not trusted. Confirm Trust repository and retry to install it.";

/// A typed trust-policy refusal, so operation callers can enter `NeedsTrust`
/// without parsing an error string while legacy callers still receive text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DotagentsSourceTrustError {
    Untrusted { identity: String },
    Registry(String),
}

impl fmt::Display for DotagentsSourceTrustError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Untrusted { identity } => {
                formatter.write_str(&untrusted_dotagents_source_error(identity))
            }
            Self::Registry(error) => formatter.write_str(error),
        }
    }
}

impl Error for DotagentsSourceTrustError {}

/// Normalize a GitHub `owner/repo` or `git:<url>` identity for trust lookup.
/// Local sources are never gated. Returns `None` when the source has no
/// repository identity to trust.
pub fn normalize_dotagents_source_identity(source: &ParsedSkillSource) -> Option<String> {
    match source.kind {
        ParsedSkillSourceKind::Github => source
            .repo
            .as_deref()
            .map(str::trim)
            .filter(|repo| !repo.is_empty())
            .map(|repo| repo.trim_end_matches(".git").to_ascii_lowercase()),
        ParsedSkillSourceKind::Git => source.url.as_deref().map(normalize_git_url_identity),
        ParsedSkillSourceKind::Local => None,
    }
}

fn normalize_git_url_identity(url: &str) -> String {
    url.trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_ascii_lowercase()
}

/// True when `source` is already in `home`'s trusted set, or has no
/// repository identity (local Copy).
pub fn is_dotagents_source_trusted(
    home: &Path,
    source: &ParsedSkillSource,
) -> Result<bool, String> {
    match require_trusted_dotagents_source(home, source) {
        Ok(()) => Ok(true),
        Err(DotagentsSourceTrustError::Untrusted { .. }) => Ok(false),
        Err(DotagentsSourceTrustError::Registry(error)) => Err(error),
    }
}

/// Require explicit trust for a parsed remote source. Local sources continue
/// without a registry entry.
pub fn require_trusted_dotagents_source(
    home: &Path,
    source: &ParsedSkillSource,
) -> Result<(), DotagentsSourceTrustError> {
    let Some(identity) = normalize_dotagents_source_identity(source) else {
        return Ok(());
    };
    require_trusted_dotagents_identity(home, &identity)
}

/// Require explicit trust for an already validated `owner/repo` pack source.
pub fn require_trusted_dotagents_identity(
    home: &Path,
    identity: &str,
) -> Result<(), DotagentsSourceTrustError> {
    let normalized =
        normalize_confirmation_identity(identity).map_err(DotagentsSourceTrustError::Registry)?;
    let registry = read_fork_registry(home).map_err(DotagentsSourceTrustError::Registry)?;
    if registry.trusted_dotagents_sources.contains(&normalized) {
        Ok(())
    } else {
        Err(DotagentsSourceTrustError::Untrusted {
            identity: normalized,
        })
    }
}

/// Record explicit trust for `identity`. Rejects empty or mismatched values
/// so confirmation cannot trust arbitrary error text.
pub fn record_trusted_dotagents_source(home: &Path, identity: &str) -> Result<String, String> {
    let normalized = normalize_confirmation_identity(identity)?;
    let mut registry = read_fork_registry(home)?;
    registry
        .trusted_dotagents_sources
        .insert(normalized.clone());
    write_fork_registry(home, &registry)?;
    Ok(normalized)
}

/// Record one explicit pack confirmation as one registry read and write.
/// Callers must hold `ForkMutationLock` so unrelated concurrent registry
/// changes cannot be overwritten.
pub fn record_trusted_dotagents_sources(
    home: &Path,
    identities: &[String],
) -> Result<Vec<String>, String> {
    let normalized = identities
        .iter()
        .map(|identity| normalize_confirmation_identity(identity))
        .collect::<Result<Vec<_>, _>>()?;
    let mut registry = read_fork_registry(home)?;
    registry
        .trusted_dotagents_sources
        .extend(normalized.iter().cloned());
    write_fork_registry(home, &registry)?;
    Ok(normalized)
}

/// Reject empty, whitespace, or multi-line confirmation identities.
pub fn normalize_confirmation_identity(identity: &str) -> Result<String, String> {
    let trimmed = identity.trim();
    if trimmed.is_empty() || trimmed.contains('\n') || trimmed.contains('\r') {
        return Err("Trust confirmation needs a repository identity".to_string());
    }
    if trimmed.contains("://") {
        return Ok(normalize_git_url_identity(trimmed));
    }
    Ok(trimmed.trim_end_matches(".git").to_ascii_lowercase())
}

/// Typed untrusted-source error. Callers must not parse CLI stderr for a
/// repo name; they pass the already-normalized request identity.
pub fn untrusted_dotagents_source_error(identity: &str) -> String {
    format!("{UNTRUSTED_DOTAGENTS_SOURCE_PREFIX}: {identity}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn github(repo: &str) -> ParsedSkillSource {
        ParsedSkillSource {
            kind: ParsedSkillSourceKind::Github,
            repo: Some(repo.to_string()),
            path: None,
            git_ref: None,
            skill_name: None,
            url: None,
            local_path: None,
        }
    }

    #[test]
    fn github_identity_is_lowercase_owner_repo() {
        assert_eq!(
            normalize_dotagents_source_identity(&github("KentCDodds/KCD-Skills.git")).as_deref(),
            Some("kentcdodds/kcd-skills")
        );
    }

    #[test]
    fn kcd_skills_is_untrusted_until_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let source = github("kentcdodds/kcd-skills");
        assert!(!is_dotagents_source_trusted(tmp.path(), &source).unwrap());
        record_trusted_dotagents_source(tmp.path(), "kentcdodds/kcd-skills").unwrap();
        assert!(is_dotagents_source_trusted(tmp.path(), &source).unwrap());
    }

    #[test]
    fn trust_requirement_returns_the_normalized_untrusted_identity() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            require_trusted_dotagents_source(tmp.path(), &github("KentCDodds/KCD-Skills.git")),
            Err(DotagentsSourceTrustError::Untrusted {
                identity: "kentcdodds/kcd-skills".to_string(),
            })
        );
    }

    #[test]
    fn confirmation_rejects_empty_and_multiline_text() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(record_trusted_dotagents_source(tmp.path(), "  ").is_err());
        assert!(record_trusted_dotagents_source(tmp.path(), "a\nb").is_err());
    }

    #[test]
    fn local_sources_are_not_gated() {
        let tmp = tempfile::tempdir().unwrap();
        let source = ParsedSkillSource {
            kind: ParsedSkillSourceKind::Local,
            repo: None,
            path: None,
            git_ref: None,
            skill_name: Some("find-bugs".to_string()),
            url: None,
            local_path: Some("/tmp/skill".to_string()),
        };
        assert!(is_dotagents_source_trusted(tmp.path(), &source).unwrap());
    }
}
