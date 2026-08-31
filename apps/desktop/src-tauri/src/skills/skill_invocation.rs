// ============================================================================
// Skills Module - skill_invocation
// Sets a skill's invocation policy - "Both" (default), "User only"
// (`disable-model-invocation: true`), or "Model only" (`user-invocable:
// false`) - by rewriting just those two frontmatter keys, leaving every
// other line of `SKILL.md` byte-identical. See frontmatter.rs's
// `InvocationPolicy`/`invocation_policy` for how the reverse direction
// (parsing) works.
//
// Codex additionally reads its own `agents/openai.yaml` sidecar
// (`policy.allow_implicit_invocation: false`) as a note-only signal
// (`Deployment.codex_implicit_invocation`, set in skill_refresh.rs); setting
// a Codex-deployed skill to "User only" here also writes that key so Codex's
// own behavior matches what the frontmatter now says, and clears it (or
// removes the file if it becomes empty) for "Both"/"Model only".
// ============================================================================

use std::fs;
use std::path::{Path, PathBuf};

use super::commands::{
    atomic_write_skill_md, canonicalize_skill_md, check_skill_md_write_allowed,
    require_snapshot_owns_path,
};
use super::frontmatter::{invocation_policy, parse_frontmatter, InvocationPolicy};
use super::skill_refresh::{self, SkillRefreshState};

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

    let rewritten =
        parse_frontmatter(&out).ok_or("Rewritten frontmatter failed to parse back".to_string())?;
    let (rewritten_policy, _) = invocation_policy(Some(&rewritten));
    if rewritten_policy != policy {
        return Err(
            "Rewritten frontmatter does not round-trip to the requested invocation policy"
                .to_string(),
        );
    }

    Ok(out)
}

/// `~/.../<skill>/agents/openai.yaml` - Codex's own invocation-policy
/// sidecar, next to `SKILL.md`.
fn codex_openai_yaml_path(skill_dir: &Path) -> PathBuf {
    skill_dir.join("agents").join("openai.yaml")
}

/// Sets or clears `policy.allow_implicit_invocation: false` in a Codex
/// deployment's `agents/openai.yaml`, preserving any other top-level keys.
/// Creates the file (and its `agents/` directory) when setting the key on a
/// skill that didn't have one; deletes the file entirely when clearing the
/// key leaves it empty, rather than leaving a stray `{}`.
fn patch_codex_openai_yaml(skill_dir: &Path, user_only: bool) -> Result<(), String> {
    let path = codex_openai_yaml_path(skill_dir);
    let mut root: serde_yaml::Mapping = match fs::read_to_string(&path) {
        Ok(content) => match serde_yaml::from_str(&content) {
            Ok(serde_yaml::Value::Mapping(m)) => m,
            Ok(_) | Err(_) => {
                return Err(format!("{} is not a YAML mapping", path.display()));
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_yaml::Mapping::new(),
        Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
    };

    let policy_key = serde_yaml::Value::String("policy".to_string());
    let allow_key = serde_yaml::Value::String("allow_implicit_invocation".to_string());
    let mut policy = match root.get(&policy_key) {
        Some(serde_yaml::Value::Mapping(m)) => m.clone(),
        _ => serde_yaml::Mapping::new(),
    };

    if user_only {
        policy.insert(allow_key, serde_yaml::Value::Bool(false));
        root.insert(policy_key, serde_yaml::Value::Mapping(policy));
    } else {
        policy.remove(&allow_key);
        if policy.is_empty() {
            root.remove(&policy_key);
        } else {
            root.insert(policy_key, serde_yaml::Value::Mapping(policy));
        }
        if root.is_empty() {
            if path.is_file() {
                fs::remove_file(&path)
                    .map_err(|e| format!("Failed to remove {}: {e}", path.display()))?;
            }
            return Ok(());
        }
    }

    let parent = path.parent().ok_or("openai.yaml has no parent directory")?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(root))
        .map_err(|e| format!("Failed to serialize {}: {e}", path.display()))?;
    let tmp_path = path.with_extension("yaml.tmp");
    fs::write(&tmp_path, yaml)
        .map_err(|e| format!("Failed to write {}: {e}", tmp_path.display()))?;
    fs::rename(&tmp_path, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        format!("Failed to save {}: {e}", path.display())
    })
}

/// `set_skill_invocation`'s logic, taking the canonical `SKILL.md` path
/// directly so it's testable without a Tauri `AppHandle` or a snapshot.
/// `has_codex_deployment` gates the `agents/openai.yaml` sidecar patch -
/// only meaningful for a skill actually deployed to Codex.
pub fn set_skill_invocation_with(
    canonical_skill_md: &Path,
    policy: InvocationPolicy,
    has_codex_deployment: bool,
) -> Result<(), String> {
    let current = fs::read_to_string(canonical_skill_md)
        .map_err(|e| format!("Failed to open {}: {e}", canonical_skill_md.display()))?;
    let updated = rewrite_invocation_frontmatter(&current, policy)?;
    atomic_write_skill_md(canonical_skill_md, &updated)?;

    if has_codex_deployment {
        let skill_dir = canonical_skill_md
            .parent()
            .ok_or("SKILL.md has no parent directory")?;
        patch_codex_openai_yaml(skill_dir, policy == InvocationPolicy::UserOnly)?;
    }
    Ok(())
}

#[tauri::command]
pub fn set_skill_invocation(
    name: String,
    path: String,
    policy: InvocationPolicy,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
) -> Result<(), String> {
    let path_buf = PathBuf::from(&path);
    require_snapshot_owns_path(&refresh_state, &path_buf)?;
    let canonical = canonicalize_skill_md(&path_buf, &path)?;

    let snapshot = refresh_state.snapshot.read().ok().and_then(|g| g.clone());
    check_skill_md_write_allowed(snapshot.as_ref(), &path_buf)?;
    let has_codex_deployment = snapshot
        .as_ref()
        .and_then(|s| s.skills.iter().find(|s| s.name == name))
        .is_some_and(|s| s.deployments.iter().any(|d| d.agent == "Codex"));

    let result = set_skill_invocation_with(&canonical, policy, has_codex_deployment);
    if result.is_ok() {
        // Surgical: flip the field the frontend renders and emit right away;
        // the background loop's full rebuild (skills_dirty) reconciles the
        // derived state (frontmatter fields, hashes) moments later.
        if let Err(e) = skill_refresh::patch_snapshot_and_emit(&app, &refresh_state, |snapshot| {
            if let Some(skill) = snapshot.skills.iter_mut().find(|s| s.name == name) {
                skill.invocation = policy;
            }
        }) {
            eprintln!("[set_skill_invocation] snapshot patch failed: {e}");
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_removes_either_key() {
        let content =
            "---\nname: find-bugs\ndescription: test\ndisable-model-invocation: true\n---\nBody.";
        let updated = rewrite_invocation_frontmatter(content, InvocationPolicy::Both).unwrap();
        assert_eq!(
            updated,
            "---\nname: find-bugs\ndescription: test\n---\nBody."
        );
    }

    #[test]
    fn user_only_inserts_key_after_description() {
        let content = "---\nname: find-bugs\ndescription: test\nlicense: MIT\n---\nBody.";
        let updated = rewrite_invocation_frontmatter(content, InvocationPolicy::UserOnly).unwrap();
        assert_eq!(
            updated,
            "---\nname: find-bugs\ndescription: test\ndisable-model-invocation: true\nlicense: MIT\n---\nBody."
        );
    }

    #[test]
    fn model_only_replaces_an_existing_conflicting_key() {
        let content =
            "---\nname: find-bugs\ndescription: test\ndisable-model-invocation: true\n---\nBody.\n";
        let updated = rewrite_invocation_frontmatter(content, InvocationPolicy::ModelOnly).unwrap();
        assert_eq!(
            updated,
            "---\nname: find-bugs\ndescription: test\nuser-invocable: false\n---\nBody.\n"
        );
    }

    #[test]
    fn body_and_other_keys_are_byte_identical() {
        let content = "---\nname: find-bugs\ndescription: test\nmetadata:\n  foo: bar\n---\n# Heading\n\nSome body text.\n";
        let updated = rewrite_invocation_frontmatter(content, InvocationPolicy::UserOnly).unwrap();
        assert!(updated.contains("metadata:\n  foo: bar\n"));
        assert!(updated.ends_with("# Heading\n\nSome body text.\n"));
    }

    #[test]
    fn refuses_content_without_frontmatter() {
        let err = rewrite_invocation_frontmatter("no frontmatter here", InvocationPolicy::Both)
            .unwrap_err();
        assert!(err.contains("no frontmatter"));
    }

    #[test]
    fn set_skill_invocation_with_rewrites_the_file_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_md = tmp.path().join("SKILL.md");
        fs::write(
            &skill_md,
            "---\nname: find-bugs\ndescription: test\n---\nBody.",
        )
        .unwrap();

        set_skill_invocation_with(&skill_md, InvocationPolicy::UserOnly, false).unwrap();
        let content = fs::read_to_string(&skill_md).unwrap();
        assert!(content.contains("disable-model-invocation: true"));
    }

    #[test]
    fn codex_deployment_writes_openai_yaml_when_user_only() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_md = tmp.path().join("SKILL.md");
        fs::write(
            &skill_md,
            "---\nname: find-bugs\ndescription: test\n---\nBody.",
        )
        .unwrap();

        set_skill_invocation_with(&skill_md, InvocationPolicy::UserOnly, true).unwrap();
        let yaml = fs::read_to_string(codex_openai_yaml_path(tmp.path())).unwrap();
        assert!(yaml.contains("allow_implicit_invocation: false"));
    }

    #[test]
    fn codex_deployment_removes_openai_yaml_when_back_to_both() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_md = tmp.path().join("SKILL.md");
        fs::write(
            &skill_md,
            "---\nname: find-bugs\ndescription: test\n---\nBody.",
        )
        .unwrap();

        set_skill_invocation_with(&skill_md, InvocationPolicy::UserOnly, true).unwrap();
        set_skill_invocation_with(&skill_md, InvocationPolicy::Both, true).unwrap();
        assert!(!codex_openai_yaml_path(tmp.path()).is_file());
    }

    #[test]
    fn codex_deployment_preserves_other_openai_yaml_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_md = tmp.path().join("SKILL.md");
        fs::write(
            &skill_md,
            "---\nname: find-bugs\ndescription: test\n---\nBody.",
        )
        .unwrap();
        fs::create_dir_all(tmp.path().join("agents")).unwrap();
        fs::write(
            codex_openai_yaml_path(tmp.path()),
            "other_key: kept\npolicy:\n  something_else: true\n",
        )
        .unwrap();

        set_skill_invocation_with(&skill_md, InvocationPolicy::UserOnly, true).unwrap();
        let yaml = fs::read_to_string(codex_openai_yaml_path(tmp.path())).unwrap();
        assert!(yaml.contains("other_key: kept"));
        assert!(yaml.contains("something_else: true"));
        assert!(yaml.contains("allow_implicit_invocation: false"));
    }

    #[test]
    fn inserts_after_a_literal_block_scalar_description() {
        let content = "---\nname: find-bugs\ndescription: |\n  Line one.\n  Line two.\nlicense: MIT\n---\nBody.\n";
        let updated = rewrite_invocation_frontmatter(content, InvocationPolicy::UserOnly).unwrap();
        assert_eq!(
            updated,
            "---\nname: find-bugs\ndescription: |\n  Line one.\n  Line two.\ndisable-model-invocation: true\nlicense: MIT\n---\nBody.\n"
        );
    }

    #[test]
    fn inserts_after_a_folded_block_scalar_description() {
        let content = "---\nname: find-bugs\ndescription: >\n  Folded text\n  continues here.\nlicense: MIT\n---\nBody.\n";
        let updated = rewrite_invocation_frontmatter(content, InvocationPolicy::UserOnly).unwrap();
        assert_eq!(
            updated,
            "---\nname: find-bugs\ndescription: >\n  Folded text\n  continues here.\ndisable-model-invocation: true\nlicense: MIT\n---\nBody.\n"
        );
    }

    #[test]
    fn nested_key_sharing_a_name_stays_untouched() {
        let content = "---\nname: find-bugs\ndescription: test\nmetadata:\n  user-invocable: false\n---\nBody.\n";
        let updated = rewrite_invocation_frontmatter(content, InvocationPolicy::UserOnly).unwrap();
        assert!(updated.contains("metadata:\n  user-invocable: false\n"));
        assert!(updated.contains("disable-model-invocation: true"));
    }

    #[test]
    fn crlf_document_keeps_crlf_outside_the_edited_span() {
        let content =
            "---\r\nname: find-bugs\r\ndescription: test\r\nlicense: MIT\r\n---\r\nBody.\r\n";
        let updated = rewrite_invocation_frontmatter(content, InvocationPolicy::UserOnly).unwrap();
        assert_eq!(
            updated,
            "---\r\nname: find-bugs\r\ndescription: test\r\ndisable-model-invocation: true\r\nlicense: MIT\r\n---\r\nBody.\r\n"
        );
    }

    #[test]
    fn missing_final_newline_is_preserved_alongside_an_insertion() {
        let content = "---\nname: find-bugs\ndescription: test\n---\nBody.";
        let updated = rewrite_invocation_frontmatter(content, InvocationPolicy::UserOnly).unwrap();
        assert_eq!(
            updated,
            "---\nname: find-bugs\ndescription: test\ndisable-model-invocation: true\n---\nBody."
        );
        assert!(!updated.ends_with('\n'));
    }

    #[test]
    fn both_removes_conflicting_keys_leaving_neither() {
        let content = "---\nname: find-bugs\ndescription: test\ndisable-model-invocation: true\nuser-invocable: false\n---\nBody.\n";
        let updated = rewrite_invocation_frontmatter(content, InvocationPolicy::Both).unwrap();
        assert!(!updated.contains("disable-model-invocation"));
        assert!(!updated.contains("user-invocable"));
    }

    #[test]
    fn applying_the_same_policy_twice_is_idempotent() {
        let content = "---\nname: find-bugs\ndescription: test\nlicense: MIT\n---\nBody.\n";
        let once = rewrite_invocation_frontmatter(content, InvocationPolicy::UserOnly).unwrap();
        let twice = rewrite_invocation_frontmatter(&once, InvocationPolicy::UserOnly).unwrap();
        assert_eq!(once, twice);
    }
}
