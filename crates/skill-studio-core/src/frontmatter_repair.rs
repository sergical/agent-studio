//! Deterministic malformed frontmatter repair.
//!
//! Ported from the desktop app's `skills/skill_frontmatter_repair.rs`
//! `propose_colon_scalar_repair` (and its `frontmatter_end` helper). Pure
//! function over `&str`; the core never touches a filesystem here. Previews
//! and proposes safe repairs: an unquoted `: ` in a top-level `name` or
//! `description` scalar, a `name` that disagrees with its folder, and the
//! contradictory invocation keys. Every rewrite after the colon repair is a
//! one-line edit that leaves all other bytes untouched.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::frontmatter::{
    is_valid_skill_name, parse_frontmatter, validate_skill, FrontmatterParseResult,
};

/// Which deterministic fix to propose.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FrontmatterRepairKind {
    /// Quote or block-encode a plain scalar whose `: ` breaks the YAML.
    #[default]
    ColonScalar,
    /// Set `name` to the folder name.
    NameMismatch,
    /// Set an invalidly formatted `name` to the folder name.
    NameFormat,
    /// Remove one of the two contradicting invocation keys.
    InvocationConflict,
}

/// Which side of the invocation conflict survives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum InvocationConflictChoice {
    /// Only the user can run it: drops `user-invocable: false`.
    UserOnly,
    /// Only the agent runs it: drops `disable-model-invocation: true`.
    ModelOnly,
}

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
        format!("description: |-{separator}  {value}")
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

    let FrontmatterParseResult::Valid(parsed) = parse_frontmatter(&proposed) else {
        return Err("The proposed repair does not parse successfully".to_string());
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

const CONFLICT_REASON: &str = "Both `disable-model-invocation: true` and `user-invocable: false` are set, so nothing can run this skill.";

fn violation_is_name_mismatch(violation: &str) -> bool {
    violation.starts_with("name \"") && violation.contains("does not match its directory name")
}

fn violation_is_name_format(violation: &str) -> bool {
    violation.starts_with("name \"") && violation.contains("must be 1-64 lowercase")
}

fn violation_is_invocation_conflict(violation: &str) -> bool {
    violation == "conflicting invocation keys"
}

fn violations_of(dir_name: &str, content: &str) -> Vec<String> {
    validate_skill(
        dir_name,
        &parse_frontmatter(content),
        content.lines().count(),
    )
}

/// Splits keeping each line's own ending, so a rewrite of one line cannot
/// disturb CRLF or LF endings elsewhere in the file.
fn lines_with_endings(content: &str) -> Vec<&str> {
    content.split_inclusive('\n').collect()
}

fn body_of(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

fn ending_of(line: &str) -> &str {
    &line[body_of(line).len()..]
}

/// Index of the top-level `key:` line inside the frontmatter fence.
fn find_key_line(lines: &[&str], key: &str) -> Option<usize> {
    let end = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| body_of(line).trim() == "---")
        .map(|(index, _)| index)?;
    lines
        .iter()
        .enumerate()
        .take(end)
        .skip(1)
        .find(|(_, line)| {
            body_of(line)
                .strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
        })
        .map(|(index, _)| index)
}

/// Replaces a single-line plain or quoted scalar with `value`, keeping the
/// key's spacing, the value's quote style, and any trailing comment.
fn replace_scalar_line(line: &str, key: &str, value: &str) -> Option<String> {
    let body = body_of(line);
    let after_key = &body[key.len() + 1..];
    let value_start = after_key.len() - after_key.trim_start().len();
    let (gap, rest) = after_key.split_at(value_start);
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''));
    let (open, tail) = if let Some(q) = quote {
        let closing = rest[1..].find(q)? + 2;
        (q.to_string(), &rest[closing..])
    } else {
        if rest.is_empty() || rest.starts_with(['|', '>', '&', '*', '!', '#']) {
            return None;
        }
        let end = rest.find(" #").unwrap_or(rest.trim_end().len());
        (String::new(), &rest[end..])
    };
    Some(format!(
        "{key}:{gap}{open}{value}{open}{tail}{}",
        ending_of(line)
    ))
}

fn rebuild(lines: &[&str], index: usize, replacement: Option<&str>) -> String {
    let mut out = String::with_capacity(lines.iter().map(|line| line.len()).sum());
    for (at, line) in lines.iter().enumerate() {
        if at != index {
            out.push_str(line);
        } else if let Some(text) = replacement {
            out.push_str(text);
        }
    }
    out
}

fn propose_name_repair(
    content: &str,
    dir_name: &str,
    targets: fn(&str) -> bool,
) -> Result<String, String> {
    if !is_valid_skill_name(dir_name) {
        return Err("The folder name is not a valid skill name".to_string());
    }
    if !violations_of(dir_name, content).iter().any(|v| targets(v)) {
        return Err("SKILL.md does not have this name violation".to_string());
    }
    let lines = lines_with_endings(content);
    let index = find_key_line(&lines, "name").ok_or("No top-level name line to rewrite")?;
    let replacement = replace_scalar_line(lines[index], "name", dir_name)
        .ok_or("The name value is not a single-line scalar")?;
    Ok(rebuild(&lines, index, Some(&replacement)))
}

fn propose_conflict_repair(
    content: &str,
    dir_name: &str,
    choice: InvocationConflictChoice,
) -> Result<String, String> {
    if !violations_of(dir_name, content)
        .iter()
        .any(|v| violation_is_invocation_conflict(v))
    {
        return Err("SKILL.md does not have conflicting invocation keys".to_string());
    }
    let key = match choice {
        InvocationConflictChoice::UserOnly => "user-invocable",
        InvocationConflictChoice::ModelOnly => "disable-model-invocation",
    };
    let lines = lines_with_endings(content);
    let index = find_key_line(&lines, key).ok_or_else(|| format!("No top-level {key} line"))?;
    Ok(rebuild(&lines, index, None))
}

/// Proposes the fix for `kind` on `content`, the `SKILL.md` of a skill in a
/// folder called `dir_name`.
///
/// Returns `Ok((proposed_content, reason))`, or `Err(message)` when no safe
/// repair exists. A proposal is returned only if re-validating it clears the
/// targeted violation and adds none. For [`FrontmatterRepairKind::
/// InvocationConflict`] without a `choice`, the content comes back unchanged
/// (after proving both options are possible) so a caller can offer them.
pub fn propose_repair(
    kind: FrontmatterRepairKind,
    content: &str,
    dir_name: &str,
    choice: Option<InvocationConflictChoice>,
) -> Result<(String, String), String> {
    let (proposed, reason, targets): (String, String, fn(&str) -> bool) = match kind {
        FrontmatterRepairKind::ColonScalar => return propose_colon_scalar_repair(content),
        FrontmatterRepairKind::NameMismatch => (
            propose_name_repair(content, dir_name, violation_is_name_mismatch)?,
            format!("Set name to the folder name \"{dir_name}\"."),
            violation_is_name_mismatch,
        ),
        FrontmatterRepairKind::NameFormat => (
            propose_name_repair(content, dir_name, violation_is_name_format)?,
            format!("Set name to the folder name \"{dir_name}\"."),
            violation_is_name_format,
        ),
        FrontmatterRepairKind::InvocationConflict => {
            for option in [
                InvocationConflictChoice::UserOnly,
                InvocationConflictChoice::ModelOnly,
            ] {
                propose_conflict_repair(content, dir_name, option)?;
            }
            let Some(choice) = choice else {
                return Ok((content.to_string(), CONFLICT_REASON.to_string()));
            };
            let reason = match choice {
                InvocationConflictChoice::UserOnly => {
                    "Remove `user-invocable: false` so only you can run it."
                }
                InvocationConflictChoice::ModelOnly => {
                    "Remove `disable-model-invocation: true` so only the agent runs it."
                }
            };
            (
                propose_conflict_repair(content, dir_name, choice)?,
                reason.to_string(),
                violation_is_invocation_conflict,
            )
        }
    };
    let before = violations_of(dir_name, content);
    let after = violations_of(dir_name, &proposed);
    if after.iter().any(|v| targets(v)) || after.iter().any(|v| !before.contains(v)) {
        return Err("The proposed repair would still leave a violation".to_string());
    }
    Ok((proposed, reason))
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

    const CONFLICT: &str = "---\nname: sample\ndescription: d\n# keep me\ndisable-model-invocation: true\nuser-invocable: false\nlicense: MIT\n---\nBody.\n";

    /// Flow: name disagrees with a valid folder name. Expect: only the name
    /// line changes to the folder name and the violation clears. Failure: the
    /// fix edits other lines or leaves the mismatch.
    #[test]
    fn name_mismatch_is_set_to_the_folder_name_and_clears_the_violation() {
        let content = "---\nname: other\ndescription: d\nlicense: MIT\n---\nBody.\n";
        let (proposed, _) =
            propose_repair(FrontmatterRepairKind::NameMismatch, content, "sample", None).unwrap();
        assert_eq!(
            proposed,
            "---\nname: sample\ndescription: d\nlicense: MIT\n---\nBody.\n"
        );
        assert!(violations_of("sample", &proposed).is_empty());
    }

    /// Flow: the folder name itself breaks the naming rule. Expect: no
    /// proposal. Failure: the fix writes an invalid name or renames a folder.
    #[test]
    fn name_repair_is_refused_when_the_folder_name_is_invalid() {
        let content = "---\nname: sample\ndescription: d\n---\n";
        for kind in [
            FrontmatterRepairKind::NameMismatch,
            FrontmatterRepairKind::NameFormat,
        ] {
            assert!(propose_repair(kind, content, "Bad_Folder", None).is_err());
        }
    }

    /// Flow: name has uppercase letters and the folder is valid. Expect: the
    /// name becomes the folder name and both name violations clear. Failure:
    /// the bad format survives the fix.
    #[test]
    fn bad_format_name_is_set_to_the_folder_name() {
        let content = "---\nname: Sample Skill\ndescription: d\n---\n";
        let (proposed, _) =
            propose_repair(FrontmatterRepairKind::NameFormat, content, "sample", None).unwrap();
        assert_eq!(proposed, "---\nname: sample\ndescription: d\n---\n");
        assert!(violations_of("sample", &proposed).is_empty());
    }

    /// Flow: a CRLF file with a mismatched name. Expect: every line keeps
    /// CRLF. Failure: the rewrite normalises line endings.
    #[test]
    fn name_repair_preserves_crlf_endings() {
        let content = "---\r\nname: other\r\ndescription: d\r\n---\r\nBody.\r\n";
        let (proposed, _) =
            propose_repair(FrontmatterRepairKind::NameMismatch, content, "sample", None).unwrap();
        assert_eq!(
            proposed,
            "---\r\nname: sample\r\ndescription: d\r\n---\r\nBody.\r\n"
        );
    }

    /// Flow: comments, other keys, and a trailing comment surround the name.
    /// Expect: only the name value changes. Failure: comments or key order
    /// are lost.
    #[test]
    fn name_repair_keeps_comments_key_order_and_other_values() {
        let content = "---\n# header\ndescription: 'quoted: kept'\nname: other # why\nlicense: \"MIT\"\n---\nname: other\n";
        let (proposed, _) =
            propose_repair(FrontmatterRepairKind::NameMismatch, content, "sample", None).unwrap();
        assert_eq!(
            proposed,
            "---\n# header\ndescription: 'quoted: kept'\nname: sample # why\nlicense: \"MIT\"\n---\nname: other\n"
        );
    }

    /// Flow: the name value is double- or single-quoted. Expect: the quote
    /// style survives. Failure: the fix strips or breaks the quotes.
    #[test]
    fn quoted_name_values_keep_their_quote_style() {
        for (original, expected) in [("\"Foo\"", "\"sample\""), ("'Foo'", "'sample'")] {
            let content = format!("---\nname: {original}\ndescription: d\n---\n");
            let (proposed, _) =
                propose_repair(FrontmatterRepairKind::NameFormat, &content, "sample", None)
                    .unwrap();
            assert_eq!(
                proposed,
                format!("---\nname: {expected}\ndescription: d\n---\n")
            );
        }
    }

    /// Flow: the name is a block scalar. Expect: no proposal. Failure: the
    /// first line is replaced and the indented continuation is orphaned.
    #[test]
    fn multi_line_name_is_not_rewritten() {
        let content = "---\nname: >\n  other\ndescription: d\n---\n";
        assert!(
            propose_repair(FrontmatterRepairKind::NameMismatch, content, "sample", None).is_err()
        );
    }

    /// Flow: the name already matches. Expect: no proposal. Failure: a no-op
    /// rewrite is offered as a fix.
    #[test]
    fn name_repair_is_refused_when_the_violation_is_absent() {
        let content = "---\nname: sample\ndescription: d\n---\n";
        assert!(
            propose_repair(FrontmatterRepairKind::NameMismatch, content, "sample", None).is_err()
        );
    }

    /// Flow: the user keeps only "you can run it". Expect: exactly the
    /// `user-invocable: false` line is gone and the conflict clears. Failure:
    /// another line goes, or the conflict stays.
    #[test]
    fn conflict_user_only_removes_exactly_the_user_invocable_line() {
        let (proposed, _) = propose_repair(
            FrontmatterRepairKind::InvocationConflict,
            CONFLICT,
            "sample",
            Some(InvocationConflictChoice::UserOnly),
        )
        .unwrap();
        assert_eq!(proposed, CONFLICT.replace("user-invocable: false\n", ""));
        assert!(violations_of("sample", &proposed).is_empty());
    }

    /// Flow: the user keeps only "the agent runs it". Expect: exactly the
    /// `disable-model-invocation: true` line is gone and the conflict clears.
    /// Failure: another line goes, or the conflict stays.
    #[test]
    fn conflict_model_only_removes_exactly_the_disable_model_line() {
        let (proposed, _) = propose_repair(
            FrontmatterRepairKind::InvocationConflict,
            CONFLICT,
            "sample",
            Some(InvocationConflictChoice::ModelOnly),
        )
        .unwrap();
        assert_eq!(
            proposed,
            CONFLICT.replace("disable-model-invocation: true\n", "")
        );
        assert!(violations_of("sample", &proposed).is_empty());
    }

    /// Flow: the dialog opens before the user picks. Expect: unchanged
    /// content comes back so the options can show. Failure: a choice is
    /// silently made for the user.
    #[test]
    fn conflict_without_a_choice_returns_the_content_unchanged() {
        let (proposed, _) = propose_repair(
            FrontmatterRepairKind::InvocationConflict,
            CONFLICT,
            "sample",
            None,
        )
        .unwrap();
        assert_eq!(proposed, CONFLICT);
    }

    /// Flow: CRLF file with the conflict. Expect: remaining lines keep CRLF.
    /// Failure: line endings change.
    #[test]
    fn conflict_repair_preserves_crlf_endings() {
        let content = CONFLICT.replace('\n', "\r\n");
        let (proposed, _) = propose_repair(
            FrontmatterRepairKind::InvocationConflict,
            &content,
            "sample",
            Some(InvocationConflictChoice::UserOnly),
        )
        .unwrap();
        assert_eq!(proposed, content.replace("user-invocable: false\r\n", ""));
    }

    /// Flow: only one invocation key is set. Expect: no conflict proposal.
    /// Failure: a fix is offered for a skill with nothing to fix.
    #[test]
    fn conflict_repair_is_refused_without_a_conflict() {
        let content = "---\nname: sample\ndescription: d\nuser-invocable: false\n---\n";
        assert!(propose_repair(
            FrontmatterRepairKind::InvocationConflict,
            content,
            "sample",
            Some(InvocationConflictChoice::UserOnly)
        )
        .is_err());
    }

    /// Flow: an escaped quote inside the name makes the line rewrite produce
    /// broken YAML. Expect: no proposal. Failure: a fix that adds a YAML error
    /// is offered.
    #[test]
    fn proposal_is_refused_when_the_result_would_still_violate() {
        let content = "---\nname: \"a\\\"b\"\ndescription: d\n---\n";
        assert!(
            propose_repair(FrontmatterRepairKind::NameFormat, content, "sample", None).is_err()
        );
    }
}
