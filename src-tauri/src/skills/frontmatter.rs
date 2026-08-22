// ============================================================================
// Skills Module - SKILL.md Frontmatter
// Parsing and validation of SKILL.md frontmatter against the agentskills.io
// spec.
// ============================================================================

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

/// SKILL.md frontmatter, per the agentskills.io spec. Unknown keys are
/// ignored by default serde behavior, so agent-specific extensions in a
/// skill's frontmatter don't break parsing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    /// Required by spec: 1-64 chars, lowercase a-z0-9 and hyphens, must
    /// match the parent directory name.
    pub name: Option<String>,
    /// Required by spec: 1-1024 chars.
    pub description: Option<String>,
    pub license: Option<String>,
    /// Optional, spec caps this at 500 chars.
    pub compatibility: Option<String>,
    /// Tolerate non-string metadata values (numbers, lists, nested maps).
    pub metadata: Option<HashMap<String, serde_yaml::Value>>,
    #[serde(rename = "allowed-tools")]
    pub allowed_tools: Option<String>,
}

/// Parse SKILL.md frontmatter per the agentskills.io spec. Malformed YAML
/// (or a missing `---` fence) is treated the same as no frontmatter at all -
/// `validate_skill` turns that into "missing name/description" violations
/// rather than this function returning an error.
pub fn parse_frontmatter(content: &str) -> Option<SkillFrontmatter> {
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let mut yaml_block = String::new();
    for line in lines {
        if line.trim() == "---" {
            return serde_yaml::from_str(&yaml_block).ok();
        }
        yaml_block.push_str(line);
        yaml_block.push('\n');
    }
    None
}

/// Every top-level frontmatter key, stringified, for the dashboard to show
/// agent-specific extensions the typed `SkillFrontmatter` doesn't model.
/// Malformed or missing frontmatter yields an empty map.
pub fn frontmatter_fields(content: &str) -> BTreeMap<String, String> {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return BTreeMap::new();
    }
    let mut yaml_block = String::new();
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        yaml_block.push_str(line);
        yaml_block.push('\n');
    }
    let Ok(serde_yaml::Value::Mapping(map)) = serde_yaml::from_str(&yaml_block) else {
        return BTreeMap::new();
    };
    map.into_iter()
        .filter_map(|(k, v)| {
            let key = k.as_str()?.to_string();
            let value = match v {
                serde_yaml::Value::String(s) => s,
                other => serde_yaml::to_string(&other)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            };
            Some((key, value))
        })
        .collect()
}

/// Skill name constraints from the agentskills.io spec: 1-64 chars,
/// lowercase a-z0-9 and hyphens, no leading/trailing/consecutive hyphens.
fn is_valid_skill_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return false;
    }
    if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return false;
    }
    true
}

/// Validate SKILL.md frontmatter (plus overall file length) against the
/// agentskills.io spec. Returns human-readable violations; an empty vec
/// means the skill is spec-compliant. The spec's optional `scripts/`,
/// `references/`, and `assets/` directories aren't validated - they're
/// free-form by design.
pub fn validate_skill(
    dir_name: &str,
    frontmatter: Option<&SkillFrontmatter>,
    skill_md_line_count: usize,
) -> Vec<String> {
    let mut violations = Vec::new();

    match frontmatter
        .and_then(|f| f.name.as_deref())
        .filter(|n| !n.is_empty())
    {
        None => violations.push("missing required frontmatter field: name".to_string()),
        Some(name) => {
            if !is_valid_skill_name(name) {
                violations.push(format!(
                    "name \"{name}\" must be 1-64 lowercase a-z0-9 characters and hyphens, with no leading, trailing, or consecutive hyphens"
                ));
            }
            if name != dir_name {
                violations.push(format!(
                    "name \"{name}\" does not match its directory name \"{dir_name}\""
                ));
            }
        }
    }

    match frontmatter
        .and_then(|f| f.description.as_deref())
        .filter(|d| !d.is_empty())
    {
        None => violations.push("missing required frontmatter field: description".to_string()),
        Some(d) if d.chars().count() > 1024 => {
            violations.push("description exceeds 1024 characters".to_string())
        }
        Some(_) => {}
    }

    if let Some(compat) = frontmatter.and_then(|f| f.compatibility.as_deref()) {
        if compat.chars().count() > 500 {
            violations.push("compatibility exceeds 500 characters".to_string());
        }
    }

    if skill_md_line_count > 500 {
        violations.push("SKILL.md exceeds recommended 500 lines".to_string());
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_frontmatter() -> SkillFrontmatter {
        SkillFrontmatter {
            name: Some("write-tests".to_string()),
            description: Some("Writes tests for the current change.".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn fully_valid_skill_has_no_violations() {
        let violations = validate_skill("write-tests", Some(&valid_frontmatter()), 50);
        assert!(
            violations.is_empty(),
            "expected no violations, got {violations:?}"
        );
    }

    #[test]
    fn bad_name_characters_are_flagged() {
        let mut fm = valid_frontmatter();
        fm.name = Some("Write_Tests!".to_string());
        let violations = validate_skill("Write_Tests!", Some(&fm), 10);
        assert!(violations
            .iter()
            .any(|v| v.contains("must be 1-64 lowercase")));
    }

    #[test]
    fn name_directory_mismatch_is_flagged() {
        let violations = validate_skill("other-dir", Some(&valid_frontmatter()), 10);
        assert!(violations
            .iter()
            .any(|v| v.contains("does not match its directory name")));
    }

    #[test]
    fn missing_description_is_flagged() {
        let mut fm = valid_frontmatter();
        fm.description = None;
        let violations = validate_skill("write-tests", Some(&fm), 10);
        assert!(violations
            .iter()
            .any(|v| v.contains("missing required frontmatter field: description")));
    }

    #[test]
    fn overlong_description_is_flagged() {
        let mut fm = valid_frontmatter();
        fm.description = Some("x".repeat(1025));
        let violations = validate_skill("write-tests", Some(&fm), 10);
        assert!(violations
            .iter()
            .any(|v| v.contains("exceeds 1024 characters")));
    }

    #[test]
    fn missing_frontmatter_flags_both_required_fields() {
        let violations = validate_skill("write-tests", None, 10);
        assert!(violations.iter().any(|v| v.contains("name")));
        assert!(violations.iter().any(|v| v.contains("description")));
    }

    #[test]
    fn frontmatter_fields_captures_every_top_level_key() {
        let content = "---\nname: write-tests\ndescription: Writes tests.\ncustom-field: some-value\n---\nBody.";
        let fields = frontmatter_fields(content);
        assert_eq!(fields.get("name").map(String::as_str), Some("write-tests"));
        assert_eq!(
            fields.get("custom-field").map(String::as_str),
            Some("some-value")
        );
    }

    #[test]
    fn frontmatter_fields_empty_without_frontmatter() {
        assert!(frontmatter_fields("no frontmatter here").is_empty());
    }
}
