use crate::skill_deployment::{BackingRelationship, DeploymentMutability, SkillDestination};
use crate::skill_document::{parse_frontmatter, FrontmatterParseResult};
use crate::skill_inventory::Deployment;
use crate::skill_ownership::LifecycleOwnerKind;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FrontmatterRepairApplyMode {
    ApplyFix,
    FixInstalledCopy,
    ForkAndFix,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontmatterRepairPreview {
    pub deployment_id: String,
    pub path: String,
    pub scope: String,
    pub reason: String,
    pub expected_content_fingerprint: String,
    pub proposal_id: String,
    pub original_content: String,
    pub proposed_content: String,
    pub allowed_apply_modes: Vec<FrontmatterRepairApplyMode>,
}

/// A caller's selection, not filesystem authority or permission to write.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundFrontmatterRepairRequest {
    pub deployment_id: String,
    pub proposal_id: String,
    pub expected_content_fingerprint: String,
    pub mode: FrontmatterRepairApplyMode,
}

impl BoundFrontmatterRepairRequest {
    /// Recompute from fresh, authorized deployment evidence and document bytes.
    /// The caller must retain its write lease through intent and replacement.
    pub fn validate(
        &self,
        deployment: &Deployment,
        bytes: &[u8],
    ) -> Result<FrontmatterRepairPreview, String> {
        if self.deployment_id.is_empty() || self.deployment_id != deployment.id {
            return Err("YAML repair needs the exact selected deployment".into());
        }
        let preview = preview_frontmatter_repair(deployment, bytes)?;
        if preview.expected_content_fingerprint != self.expected_content_fingerprint
            || preview.proposal_id != self.proposal_id
        {
            return Err(
                "YAML repair refused: the deployment, ownership, or content changed".into(),
            );
        }
        if !preview.allowed_apply_modes.contains(&self.mode) {
            return Err("This repair mode is not allowed for the selected deployment".into());
        }
        Ok(preview)
    }
}

pub fn content_fingerprint(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

pub(crate) fn proposal_id(deployment: &Deployment, fingerprint: &str, proposed: &str) -> String {
    let identity = format!(
        "{}\0{}\0{}\0{:?}\0{}\0{}\0{}",
        deployment.id,
        deployment.path,
        deployment.owner_id.as_deref().unwrap_or(""),
        deployment.owner_kind,
        deployment.owner_revision.as_deref().unwrap_or(""),
        fingerprint,
        proposed
    );
    content_fingerprint(identity.as_bytes())
}

fn frontmatter_end(lines: &[&str]) -> Option<usize> {
    if lines.first().map(|line| line.trim_end_matches('\r').trim()) != Some("---") {
        return None;
    }
    lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.trim_end_matches('\r').trim() == "---")
        .map(|(index, _)| index)
}

/// Produces an exact-byte proposal only when one top-level plain scalar is the
/// unique likely source of the YAML parser error.
pub fn propose_colon_scalar_repair(content: &str) -> Result<(String, String), String> {
    let parse_error = match parse_frontmatter(content) {
        FrontmatterParseResult::Invalid(error) => error,
        FrontmatterParseResult::Absent | FrontmatterParseResult::Valid(_) => {
            return Err("SKILL.md does not have a malformed YAML frontmatter block".to_string())
        }
    };
    let separator = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    if separator == "\r\n" && content.replace("\r\n", "").contains('\n') {
        return Err("Mixed line endings make the scalar boundary ambiguous".to_string());
    }
    let had_final_newline = content.ends_with(separator);
    let lines: Vec<&str> = content.split(separator).collect();
    let end = frontmatter_end(&lines).ok_or("Frontmatter is missing or unterminated")?;
    let mut candidates = Vec::new();
    for (index, line) in lines.iter().enumerate().take(end).skip(1) {
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some((key, value)) = line.split_once(": ") else {
            continue;
        };
        if !matches!(key, "name" | "description") || !value.contains(": ") {
            continue;
        }
        if value.starts_with(['\'', '"', '|', '>', '[', '{'])
            || value.ends_with(':')
            || value.contains(" #")
        {
            continue;
        }
        candidates.push((index, key, value));
    }
    let [(index, key, value)] = candidates.as_slice() else {
        return Err("No unique top-level name or description scalar can be repaired safely".into());
    };
    if parse_error.line != index + 1 || !parse_error.message.contains("mapping values") {
        return Err("The YAML error is not caused by the candidate scalar".to_string());
    }

    let replacement = if *key == "description" {
        format!("description: |-{}  {}", separator, value)
    } else {
        let quoted = serde_yaml::to_string(value)
            .map_err(|error| format!("Could not quote name: {error}"))?
            .trim_end()
            .to_string();
        if quoted.contains('\n') {
            return Err("Name repair would not remain single-line".to_string());
        }
        format!("name: {quoted}")
    };
    let mut proposed_lines: Vec<String> = lines.iter().map(|line| (*line).to_string()).collect();
    proposed_lines[*index] = replacement;
    let mut proposed = proposed_lines.join(separator);
    if had_final_newline && !proposed.ends_with(separator) {
        proposed.push_str(separator);
    }

    let parsed = match parse_frontmatter(&proposed) {
        FrontmatterParseResult::Valid(parsed) => parsed,
        _ => return Err("The proposed repair does not parse successfully".to_string()),
    };
    let repaired_value = if *key == "description" {
        parsed.description.as_deref()
    } else {
        parsed.name.as_deref()
    };
    if repaired_value != Some(*value) {
        return Err("The proposed repair changes the scalar value".to_string());
    }
    Ok((
        proposed,
        format!("Encode the top-level {key} value so its `: ` is text, not YAML syntax."),
    ))
}

pub fn allowed_repair_modes(deployment: &Deployment) -> Vec<FrontmatterRepairApplyMode> {
    if deployment.plugin.is_some()
        || deployment.is_symlink
        || deployment.shared_via_whole_dir_link
        || deployment.mutability == DeploymentMutability::ReadOnly
            && deployment.owner_kind != LifecycleOwnerKind::Manual
    {
        return vec![];
    }
    match deployment.owner_kind {
        LifecycleOwnerKind::SkillsSh | LifecycleOwnerKind::Dotagents => {
            if deployment
                .owner_revision
                .as_ref()
                .is_none_or(|revision| revision.is_empty())
            {
                return vec![];
            }
            if deployment.scope == "global"
                && deployment.destination == SkillDestination::Universal
                && matches!(deployment.backing, BackingRelationship::Canonical)
            {
                vec![
                    FrontmatterRepairApplyMode::ForkAndFix,
                    FrontmatterRepairApplyMode::FixInstalledCopy,
                ]
            } else {
                vec![]
            }
        }
        LifecycleOwnerKind::Copy | LifecycleOwnerKind::Fork | LifecycleOwnerKind::Manual => {
            vec![FrontmatterRepairApplyMode::ApplyFix]
        }
        _ => vec![],
    }
}

pub fn preview_frontmatter_repair(
    deployment: &Deployment,
    bytes: &[u8],
) -> Result<FrontmatterRepairPreview, String> {
    let original =
        String::from_utf8(bytes.to_vec()).map_err(|_| "SKILL.md is not UTF-8".to_string())?;
    let (proposed, reason) = propose_colon_scalar_repair(&original)?;
    let fingerprint = content_fingerprint(bytes);
    Ok(FrontmatterRepairPreview {
        deployment_id: deployment.id.clone(),
        path: deployment.path.clone(),
        scope: deployment.scope.clone(),
        reason,
        expected_content_fingerprint: fingerprint.clone(),
        proposal_id: proposal_id(deployment, &fingerprint, &proposed),
        original_content: original,
        proposed_content: proposed,
        allowed_apply_modes: allowed_repair_modes(deployment),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bound_request_refuses_target_drift_and_forbidden_modes() {
        let deployment = Deployment {
            id: "selected-deployment".into(),
            path: "/fixture/sample".into(),
            owner_kind: LifecycleOwnerKind::Manual,
            mutability: DeploymentMutability::Mutable,
            ..Deployment::default()
        };
        let bytes = b"---\nname: sample\ndescription: Use when: testing\n---\nbody\n";
        let preview = preview_frontmatter_repair(&deployment, bytes).unwrap();
        let request = BoundFrontmatterRepairRequest {
            deployment_id: deployment.id.clone(),
            proposal_id: preview.proposal_id,
            expected_content_fingerprint: preview.expected_content_fingerprint,
            mode: FrontmatterRepairApplyMode::ApplyFix,
        };
        assert!(request.validate(&deployment, bytes).is_ok());
        let mut changed = deployment.clone();
        changed.id = "other-deployment".into();
        assert!(request.validate(&changed, bytes).is_err());
        changed = deployment.clone();
        changed.path = "/other/sample".into();
        assert!(request.validate(&changed, bytes).is_err());
        changed = deployment.clone();
        changed.owner_kind = LifecycleOwnerKind::SkillsSh;
        assert!(request.validate(&changed, bytes).is_err());
        assert!(request
            .validate(&deployment, &[bytes.as_slice(), b"drift"].concat())
            .is_err());
        let forbidden = BoundFrontmatterRepairRequest {
            mode: FrontmatterRepairApplyMode::ForkAndFix,
            ..request.clone()
        };
        assert!(forbidden.validate(&deployment, bytes).is_err());
        let mut wire = serde_json::to_value(&request).unwrap();
        wire["path"] = serde_json::json!("/other/sample");
        assert!(serde_json::from_value::<BoundFrontmatterRepairRequest>(wire).is_err());
    }

    #[test]
    fn repairs_description_without_touching_body_or_crlf() {
        let input = "---\r\nname: sample\r\ndescription: Use when: testing \"quotes\"\r\nlicense: MIT\r\n---\r\n# Body\r\nbytes: stay\r\n";
        let (actual, _) = propose_colon_scalar_repair(input).unwrap();
        assert_eq!(actual, "---\r\nname: sample\r\ndescription: |-\r\n  Use when: testing \"quotes\"\r\nlicense: MIT\r\n---\r\n# Body\r\nbytes: stay\r\n");
        let FrontmatterParseResult::Valid(parsed) = parse_frontmatter(&actual) else {
            panic!("proposal did not parse")
        };
        assert_eq!(
            parsed.description.as_deref(),
            Some("Use when: testing \"quotes\"")
        );
    }

    #[test]
    fn refuses_ambiguous_and_other_yaml_failures() {
        for input in [
            "---\nname: one: two\ndescription: three: four\n---\n",
            "---\nname: [broken\ndescription: ok\n---\n",
            "---\nname: 'broken\ndescription: ok: here\n---\n",
            "---\nname: ok\ndescription: nested:\n  child: value\n---\n",
        ] {
            assert!(propose_colon_scalar_repair(input).is_err(), "{input}");
        }
    }

    #[test]
    fn repairs_name_as_one_line_with_exact_value() {
        let input = "---\nname: alpha: beta\ndescription: safe\n---\nbody\n";
        let (actual, _) = propose_colon_scalar_repair(input).unwrap();
        assert!(!actual.contains("name: |"));
        let FrontmatterParseResult::Valid(parsed) = parse_frontmatter(&actual) else {
            panic!("proposal did not parse")
        };
        assert_eq!(parsed.name.as_deref(), Some("alpha: beta"));
        assert!(actual.ends_with("---\nbody\n"));
    }

    #[test]
    fn managed_repair_requires_source_revision_evidence() {
        for owner_kind in [LifecycleOwnerKind::SkillsSh, LifecycleOwnerKind::Dotagents] {
            let deployment = Deployment {
                owner_kind,
                scope: "global".into(),
                destination: SkillDestination::Universal,
                backing: BackingRelationship::Canonical,
                mutability: DeploymentMutability::Mutable,
                ..Deployment::default()
            };
            assert!(allowed_repair_modes(&deployment).is_empty());
        }
    }

    #[test]
    fn action_policy_is_exact_for_each_owner_class() {
        let managed = Deployment {
            owner_kind: LifecycleOwnerKind::SkillsSh,
            owner_revision: Some("fixture-source-revision".into()),
            mutability: DeploymentMutability::Mutable,
            destination: SkillDestination::Universal,
            backing: BackingRelationship::Canonical,
            scope: "global".to_string(),
            ..Deployment::default()
        };
        assert_eq!(
            allowed_repair_modes(&managed),
            vec![
                FrontmatterRepairApplyMode::ForkAndFix,
                FrontmatterRepairApplyMode::FixInstalledCopy
            ]
        );
        for owner_kind in [
            LifecycleOwnerKind::Copy,
            LifecycleOwnerKind::Fork,
            LifecycleOwnerKind::Manual,
        ] {
            assert_eq!(
                allowed_repair_modes(&Deployment {
                    owner_kind,
                    mutability: DeploymentMutability::Mutable,
                    ..Deployment::default()
                }),
                vec![FrontmatterRepairApplyMode::ApplyFix]
            );
        }
        for owner_kind in [
            LifecycleOwnerKind::Plugin,
            LifecycleOwnerKind::WildcardDotagents,
            LifecycleOwnerKind::Ambiguous,
            LifecycleOwnerKind::Unknown,
        ] {
            assert!(allowed_repair_modes(&Deployment {
                owner_kind,
                ..Deployment::default()
            })
            .is_empty());
        }
    }
}
