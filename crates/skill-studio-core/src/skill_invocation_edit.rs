//! Pure invocation-policy transforms. Outputs grant no filesystem authority.
use crate::skill_document::{invocation_policy, parse_frontmatter, InvocationPolicy};

/// Strips a line's trailing terminator (`\r\n` or `\n`), if it has one - used
/// to compare line *content* while the raw, terminator-included slice is kept
/// around separately for byte-identical reconstruction.
fn strip_terminator(raw: &str) -> &str {
    raw.strip_suffix("\r\n")
        .or_else(|| raw.strip_suffix('\n'))
        .unwrap_or(raw)
}

/// A line at column 0 (no leading whitespace) with some content - the start
/// of a new top-level YAML key. Blank lines and indented lines are
/// continuations of whatever top-level key preceded them (a nested mapping,
/// a block scalar body, or just blank padding).
fn is_top_level_line(text: &str) -> bool {
    !text.is_empty() && !text.starts_with(' ') && !text.starts_with('\t')
}

/// Whether `text` (a top-level line) is the given top-level `key`, i.e.
/// matches `^<key>\s*:`.
fn is_key(text: &str, key: &str) -> bool {
    match text.strip_prefix(key) {
        Some(rest) => rest.trim_start_matches([' ', '\t']).starts_with(':'),
        None => false,
    }
}

/// Groups `body` (the frontmatter's lines, one entry per line, sans
/// terminator) into `[start, end)` spans, one per top-level key: a span
/// starts at a column-0 line and extends through every blank or indented
/// line that follows, up to (but not including) the next column-0 line.
fn top_level_spans(body: &[&str]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut i = 0;
    while i < body.len() {
        if !is_top_level_line(body[i]) {
            // Malformed frontmatter (content before any top-level key) -
            // skip rather than looping forever; nothing to attach it to.
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < body.len()
            && (body[i].is_empty() || body[i].starts_with(' ') || body[i].starts_with('\t'))
        {
            i += 1;
        }
        spans.push((start, i));
    }
    spans
}

/// Removes (or replaces) the top-level `disable-model-invocation`/
/// `user-invocable` keys in `content`'s frontmatter block to match `policy`,
/// inserting the new key (if any) right after the `description` key's span -
/// after its block-scalar body, if it has one - or at the end of the
/// frontmatter when there's no `description`. Every other byte - other keys
/// (including a nested key that happens to share a name with one of these
/// two), the body, blank lines, the line separator style (`\r\n` vs `\n`),
/// and a missing final newline - is passed through unchanged. Errs when
/// `content` has no `---`-fenced frontmatter block to edit, or when the
/// result doesn't parse back to the requested `policy`.
pub fn rewrite_invocation_frontmatter(
    content: &str,
    policy: InvocationPolicy,
) -> Result<String, String> {
    let sep = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };

    // Raw segments keep each line's own terminator (or lack of one, for the
    // last line) attached, so untouched lines can be re-emitted byte for
    // byte instead of being rejoined with a terminator we chose ourselves.
    let raw_lines: Vec<&str> = content.split_inclusive('\n').collect();
    let lines: Vec<&str> = raw_lines.iter().copied().map(strip_terminator).collect();

    if lines.first().map(|l| l.trim()) != Some("---") {
        return Err("SKILL.md has no frontmatter to edit".to_string());
    }
    let close_idx = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| l.trim() == "---")
        .map(|(i, _)| i)
        .ok_or("SKILL.md frontmatter has no closing `---`")?;

    let body: &[&str] = &lines[1..close_idx];
    let body_raw: &[&str] = &raw_lines[1..close_idx];
    let spans = top_level_spans(body);

    let mut drop = vec![false; body.len()];
    for &(start, end) in &spans {
        if is_key(body[start], "disable-model-invocation") || is_key(body[start], "user-invocable")
        {
            for slot in drop.iter_mut().take(end).skip(start) {
                *slot = true;
            }
        }
    }
    let description_span = spans
        .iter()
        .find(|&&(start, _)| is_key(body[start], "description"))
        .copied();

    let new_key = match policy {
        InvocationPolicy::Both => None,
        InvocationPolicy::UserOnly => Some("disable-model-invocation: true"),
        InvocationPolicy::ModelOnly => Some("user-invocable: false"),
    };

    let mut out = String::new();
    out.push_str(raw_lines[0]);
    for idx in 0..body.len() {
        if drop[idx] {
            continue;
        }
        out.push_str(body_raw[idx]);
        let at_description_end = description_span.is_some_and(|(_, end)| idx == end - 1);
        if at_description_end {
            if let Some(key) = new_key {
                out.push_str(key);
                out.push_str(sep);
            }
        }
    }
    if description_span.is_none() {
        if let Some(key) = new_key {
            out.push_str(key);
            out.push_str(sep);
        }
    }
    out.push_str(raw_lines[close_idx]);
    for raw in &raw_lines[close_idx + 1..] {
        out.push_str(raw);
    }

    let parsed = parse_frontmatter(&out);
    let rewritten = parsed
        .as_frontmatter()
        .ok_or("Rewritten frontmatter failed to parse back".to_string())?;
    let (rewritten_policy, _) = invocation_policy(Some(rewritten));
    if rewritten_policy != policy {
        return Err(
            "Rewritten frontmatter does not round-trip to the requested invocation policy"
                .to_string(),
        );
    }

    Ok(out)
}

#[derive(Debug, Clone)]
pub struct CodexInvocationEdit {
    original: Option<String>,
    proposed: Option<String>,
}

impl CodexInvocationEdit {
    pub fn new(original: Option<String>, policy: InvocationPolicy) -> Result<Self, String> {
        let proposed = rewrite_codex_invocation_policy(original.as_deref(), policy)?;
        Ok(Self { original, proposed })
    }
    pub fn original(&self) -> Option<&str> {
        self.original.as_deref()
    }
    pub fn proposed(&self) -> Option<&str> {
        self.proposed.as_deref()
    }
    /// Builds an edit from already verified fixed-file endpoints. Callers may
    /// use this only for the `agents/openai.yaml` participant in a persisted
    /// Copy document transaction.
    pub(crate) fn from_endpoints(original: Option<String>, proposed: Option<String>) -> Self {
        Self { original, proposed }
    }
    pub fn validate_current(&self, current: Option<&str>) -> Result<(), String> {
        if current != self.original() {
            return Err(
                "openai.yaml changed since invocation planning; reload before retrying".into(),
            );
        }
        Ok(())
    }
}

/// None is a proven absent input; a None output means remove an empty sidecar.
pub fn rewrite_codex_invocation_policy(
    content: Option<&str>,
    policy: InvocationPolicy,
) -> Result<Option<String>, String> {
    let mut root: serde_yaml::Mapping = match content {
        Some(content) => match serde_yaml::from_str(content) {
            Ok(serde_yaml::Value::Mapping(mapping)) => mapping,
            Ok(_) | Err(_) => return Err("openai.yaml is not a YAML mapping".into()),
        },
        None => serde_yaml::Mapping::new(),
    };
    let policy_key = serde_yaml::Value::String("policy".into());
    let allow_key = serde_yaml::Value::String("allow_implicit_invocation".into());
    let mut policy_mapping = match root.get(&policy_key) {
        Some(serde_yaml::Value::Mapping(mapping)) => mapping.clone(),
        None => serde_yaml::Mapping::new(),
        Some(_) => return Err("openai.yaml policy is not a YAML mapping".into()),
    };
    if policy == InvocationPolicy::UserOnly {
        policy_mapping.insert(allow_key, serde_yaml::Value::Bool(false));
        root.insert(policy_key, serde_yaml::Value::Mapping(policy_mapping));
    } else {
        policy_mapping.remove(&allow_key);
        if policy_mapping.is_empty() {
            root.remove(&policy_key);
        } else {
            root.insert(policy_key, serde_yaml::Value::Mapping(policy_mapping));
        }
        if root.is_empty() {
            return Ok(None);
        }
    }
    serde_yaml::to_string(&serde_yaml::Value::Mapping(root))
        .map(Some)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_edit_requires_exact_original_presence_and_bytes() {
        let absent = CodexInvocationEdit::new(None, InvocationPolicy::UserOnly).unwrap();
        absent.validate_current(None).unwrap();
        assert!(absent.validate_current(Some("")).is_err());
        let original = "interface:\n  display_name: Sample\n";
        let present =
            CodexInvocationEdit::new(Some(original.into()), InvocationPolicy::Both).unwrap();
        present.validate_current(Some(original)).unwrap();
        assert!(present.validate_current(None).is_err());
        assert!(present
            .validate_current(Some("interface: {display_name: Sample}\n"))
            .is_err());
        assert_eq!(present.original(), Some(original));
    }

    #[test]
    fn codex_policy_refuses_non_mapping_policy() {
        for original in ["policy: custom\n", "policy: [custom]\n", "policy: null\n"] {
            for policy in [
                InvocationPolicy::Both,
                InvocationPolicy::UserOnly,
                InvocationPolicy::ModelOnly,
            ] {
                assert_eq!(
                    rewrite_codex_invocation_policy(Some(original), policy).unwrap_err(),
                    "openai.yaml policy is not a YAML mapping"
                );
            }
        }
    }

    #[test]
    fn codex_policy_round_trip_preserves_other_settings() {
        let original = "interface:\n  display_name: Sample\npolicy:\n  future: keep\n";
        let enabled = rewrite_codex_invocation_policy(Some(original), InvocationPolicy::UserOnly)
            .unwrap()
            .unwrap();
        let value: serde_yaml::Value = serde_yaml::from_str(&enabled).unwrap();
        assert_eq!(value["interface"]["display_name"].as_str(), Some("Sample"));
        assert_eq!(value["policy"]["future"].as_str(), Some("keep"));
        assert_eq!(
            value["policy"]["allow_implicit_invocation"].as_bool(),
            Some(false)
        );
        assert_eq!(
            rewrite_codex_invocation_policy(Some(&enabled), InvocationPolicy::UserOnly)
                .unwrap()
                .as_deref(),
            Some(enabled.as_str())
        );
        for policy in [InvocationPolicy::Both, InvocationPolicy::ModelOnly] {
            let cleared = rewrite_codex_invocation_policy(Some(&enabled), policy)
                .unwrap()
                .unwrap();
            assert_eq!(
                serde_yaml::from_str::<serde_yaml::Value>(&cleared).unwrap(),
                serde_yaml::from_str::<serde_yaml::Value>(original).unwrap()
            );
            assert!(rewrite_codex_invocation_policy(None, policy)
                .unwrap()
                .is_none());
            assert!(rewrite_codex_invocation_policy(
                Some("policy:\n  allow_implicit_invocation: false\n"),
                policy
            )
            .unwrap()
            .is_none());
        }
    }
}
