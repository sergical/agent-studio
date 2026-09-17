//! Deterministic malformed frontmatter repair.
//!
//! Ported from the desktop app's `skills/skill_frontmatter_repair.rs`
//! `propose_colon_scalar_repair` (and its `frontmatter_end` helper). Pure
//! function over `&str`; the core never touches a filesystem here. Previews
//! and proposes the one safe first-version repair: an unquoted `: ` in a
//! top-level `name` or `description` scalar.

use crate::frontmatter::{parse_frontmatter, FrontmatterParseResult};

/// Finds the line index of the closing `---` fence, given the file already
/// starts with an opening one. `None` when the file has no fence, or the
/// fence never closes.
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

/// Produces an exact-byte proposal only when one top-level plain scalar is
/// the unique likely source of the YAML parser error.
///
/// Returns `Ok((proposed_content, reason))` on success; `Err(message)` when
/// no safe, unique, deterministic repair exists.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repairs_a_colon_in_description() {
        let content = "---\nname: zeta-bad\ndescription: Use this: when needed\n---\nBody.\n";
        let (proposed, reason) = propose_colon_scalar_repair(content).unwrap();
        assert!(proposed.contains("description: |-"));
        assert!(reason.contains("description"));
        assert!(matches!(
            parse_frontmatter(&proposed),
            FrontmatterParseResult::Valid(_)
        ));
    }

    #[test]
    fn refuses_content_that_already_parses() {
        let content = "---\nname: ok\ndescription: fine\n---\nBody.\n";
        assert!(propose_colon_scalar_repair(content).is_err());
    }

    #[test]
    fn refuses_content_with_no_fence() {
        assert!(propose_colon_scalar_repair("no frontmatter here").is_err());
    }
}
