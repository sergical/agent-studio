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

use super::commands::canonicalize_skill_md;
use super::frontmatter::{invocation_policy, parse_frontmatter, InvocationPolicy};
use super::skill_deployment::parse_deployment_id;
use super::skill_dto::Deployment;
use super::skill_md_write::begin_skill_md_write_transaction;
use super::skill_refresh::{self, SkillRefreshState, SkillSnapshot};

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
/// `is_codex_deployment` gates the `agents/openai.yaml` sidecar patch.
pub fn set_skill_invocation_with(
    canonical_skill_md: &Path,
    policy: InvocationPolicy,
    is_codex_deployment: bool,
) -> Result<(), String> {
    set_skill_invocation_with_read_hook(canonical_skill_md, policy, is_codex_deployment, || {})
}

fn set_skill_invocation_with_read_hook(
    canonical_skill_md: &Path,
    policy: InvocationPolicy,
    is_codex_deployment: bool,
    after_read: impl FnOnce(),
) -> Result<(), String> {
    let transaction = begin_skill_md_write_transaction()?;
    let current = transaction.read_to_string(canonical_skill_md)?;
    after_read();
    let updated = rewrite_invocation_frontmatter(&current, policy)?;
    transaction.replace_text(canonical_skill_md, &updated)?;
    drop(transaction);

    if is_codex_deployment {
        let skill_dir = canonical_skill_md
            .parent()
            .ok_or("SKILL.md has no parent directory")?;
        patch_codex_openai_yaml(skill_dir, policy == InvocationPolicy::UserOnly)?;
    }
    Ok(())
}

/// Resolves one invocation edit to its exact lexical deployment before the
/// requested `SKILL.md` is canonicalized. This prevents separate deployment
/// paths that resolve to one directory from losing their harness identity.
fn exact_snapshot_invocation_deployment<'a>(
    snapshot: &'a SkillSnapshot,
    name: &str,
    requested_skill_md: &Path,
) -> Result<&'a Deployment, String> {
    if requested_skill_md
        .file_name()
        .and_then(|file| file.to_str())
        != Some("SKILL.md")
    {
        return Err(format!(
            "Invocation target is stale: {} is not a SKILL.md path",
            requested_skill_md.display()
        ));
    }
    let requested_dir = requested_skill_md.parent().ok_or_else(|| {
        format!(
            "Invocation target is stale: {} has no deployment directory",
            requested_skill_md.display()
        )
    })?;
    let mut matching = snapshot.skills.iter().flat_map(|skill| {
        skill
            .deployments
            .iter()
            .filter(move |deployment| Path::new(&deployment.path) == requested_dir)
            .map(move |deployment| (skill, deployment))
    });
    let (skill, deployment) = matching.next().ok_or_else(|| {
        format!(
            "Invocation target is stale: {} is not an exact deployment in the current snapshot",
            requested_skill_md.display()
        )
    })?;
    if matching.next().is_some() {
        return Err(format!(
            "Invocation target is ambiguous: {} matches more than one deployment",
            requested_skill_md.display()
        ));
    }
    if skill.name != name {
        return Err(format!(
            "Invocation target is stale: {} belongs to {}, not {name}",
            requested_skill_md.display(),
            skill.name
        ));
    }

    let parsed = parse_deployment_id(&deployment.id).ok_or_else(|| {
        format!(
            "Invocation target is stale: deployment {} has an invalid identity",
            deployment.id
        )
    })?;
    let codex_identity_mismatch = (parsed.slot == "codex") != (deployment.agent == "Codex");
    if parsed.name != skill.name
        || parsed.scope != deployment.scope
        || parsed.destination != deployment.destination
        || parsed.project_path != deployment.project_path
        || parsed.lexical_path != Path::new(&deployment.path)
        || codex_identity_mismatch
    {
        return Err(format!(
            "Invocation target is stale: deployment {} no longer matches its snapshot identity",
            deployment.id
        ));
    }

    Ok(deployment)
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
    let snapshot = refresh_state
        .snapshot
        .read()
        .map_err(|error| format!("Snapshot lock poisoned: {error}"))?
        .clone()
        .ok_or_else(|| format!("Invocation target is stale: {path} is not an installed skill"))?;
    let deployment = exact_snapshot_invocation_deployment(&snapshot, &name, &path_buf)?;
    if deployment.plugin.is_some() {
        return Err("Skill is managed by a plugin and cannot be edited here".to_string());
    }
    let is_codex_deployment = deployment.agent == "Codex";
    let canonical = canonicalize_skill_md(&path_buf, &path)?;

    let result = set_skill_invocation_with(&canonical, policy, is_codex_deployment);
    if result.is_ok() {
        if let Err(error) =
            skill_refresh::reconcile_skill_names_and_emit(&app, &refresh_state, [name], &[])
        {
            eprintln!("[set_skill_invocation] targeted snapshot reconciliation failed: {error}");
            refresh_state.mark_skills_dirty();
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation_snapshot(deployments: Vec<Deployment>) -> SkillSnapshot {
        use super::super::provenance::SourceKind;
        use super::super::skill_dto::InstalledSkill;
        use super::super::skill_invocations::InvocationHeatmap;

        SkillSnapshot {
            revision: 1,
            skills: vec![InstalledSkill {
                name: "find-bugs".to_string(),
                source: "manual".to_string(),
                source_type: "manual".to_string(),
                source_url: None,
                skill_path: None,
                installed_at: "2024-01-01T00:00:00Z".to_string(),
                updated_at: None,
                has_update: false,
                update_owner_ids: Vec::new(),
                update_owners: Vec::new(),
                update_commit: None,
                update_commit_at: None,
                source_kind: SourceKind::Manual,
                deployments,
                has_spec: false,
                description: None,
                spec_violations: Vec::new(),
                skill_md_tokens: 0,
                description_tokens: 0,
                folder_bytes: 0,
                file_count: 0,
                content_hash: String::new(),
                content_hashes: Vec::new(),
                modified_at: None,
                frontmatter_fields: Default::default(),
                folder_truncated: false,
                fork: None,
                trial: None,
                trials: Vec::new(),
                parked: false,
                parked_at: None,
                invocation: InvocationPolicy::Both,
            }],
            projects: Vec::new(),
            invocations: Vec::new(),
            heatmap: InvocationHeatmap::default(),
            scanned_at: "2024-01-01T00:00:00Z".to_string(),
            last_test_by_skill: Default::default(),
            update_check: Default::default(),
            opencode_config_kind: None,
        }
    }

    fn invocation_deployment(
        path: &Path,
        agent: &str,
        slot: &str,
        destination: super::super::skill_deployment::SkillDestination,
    ) -> Deployment {
        use super::super::skill_deployment::{deployment_id, DeploymentMutability};

        Deployment {
            id: deployment_id("find-bugs", "global", destination, slot, None, path),
            destination,
            mutability: DeploymentMutability::Mutable,
            agent: agent.to_string(),
            scope: "global".to_string(),
            path: path.to_string_lossy().into_owned(),
            ..Default::default()
        }
    }

    fn write_invocation_skill(skill_dir: &Path) -> PathBuf {
        fs::create_dir_all(skill_dir).unwrap();
        let skill_md = skill_dir.join("SKILL.md");
        fs::write(
            &skill_md,
            "---\nname: find-bugs\ndescription: test\n---\nBody.",
        )
        .unwrap();
        skill_md
    }

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
    fn invocation_read_and_replace_share_the_skill_md_write_transaction() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_md = tmp.path().join("SKILL.md");
        fs::write(
            &skill_md,
            "---\nname: find-bugs\ndescription: test\n---\nBody.",
        )
        .unwrap();

        set_skill_invocation_with_read_hook(&skill_md, InvocationPolicy::UserOnly, false, || {
            assert!(
                super::super::skill_md_write::skill_md_write_transaction_is_held(),
                "invocation read completed without the SKILL.md transaction"
            );
        })
        .unwrap();

        assert!(fs::read_to_string(skill_md)
            .unwrap()
            .contains("disable-model-invocation: true"));
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
    fn same_named_codex_sibling_does_not_create_a_claude_sidecar() {
        use super::super::skill_deployment::SkillDestination;

        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude/skills/find-bugs");
        let codex_dir = tmp.path().join(".codex/skills/find-bugs");
        let claude_skill_md = write_invocation_skill(&claude_dir);
        write_invocation_skill(&codex_dir);
        let snapshot = invocation_snapshot(vec![
            invocation_deployment(
                &claude_dir,
                "Claude Code",
                "claude-code",
                SkillDestination::PerHarness,
            ),
            invocation_deployment(&codex_dir, "Codex", "codex", SkillDestination::PerHarness),
        ]);

        let deployment =
            exact_snapshot_invocation_deployment(&snapshot, "find-bugs", &claude_skill_md).unwrap();
        set_skill_invocation_with(
            &claude_skill_md,
            InvocationPolicy::UserOnly,
            deployment.agent == "Codex",
        )
        .unwrap();

        assert!(!codex_openai_yaml_path(&claude_dir).exists());
        assert!(!codex_openai_yaml_path(&codex_dir).exists());
    }

    #[test]
    fn exact_codex_deployment_keeps_sidecar_behavior_with_same_named_sibling() {
        use super::super::skill_deployment::SkillDestination;

        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude/skills/find-bugs");
        let codex_dir = tmp.path().join(".codex/skills/find-bugs");
        write_invocation_skill(&claude_dir);
        let codex_skill_md = write_invocation_skill(&codex_dir);
        let snapshot = invocation_snapshot(vec![
            invocation_deployment(
                &claude_dir,
                "Claude Code",
                "claude-code",
                SkillDestination::PerHarness,
            ),
            invocation_deployment(&codex_dir, "Codex", "codex", SkillDestination::PerHarness),
        ]);

        let deployment =
            exact_snapshot_invocation_deployment(&snapshot, "find-bugs", &codex_skill_md).unwrap();
        set_skill_invocation_with(
            &codex_skill_md,
            InvocationPolicy::UserOnly,
            deployment.agent == "Codex",
        )
        .unwrap();

        let yaml = fs::read_to_string(codex_openai_yaml_path(&codex_dir)).unwrap();
        assert!(yaml.contains("allow_implicit_invocation: false"));
        assert!(!codex_openai_yaml_path(&claude_dir).exists());
    }

    #[test]
    fn universal_deployment_with_a_codex_link_does_not_receive_a_sidecar() {
        use super::super::skill_deployment::{BackingRelationship, SkillDestination};

        let tmp = tempfile::tempdir().unwrap();
        let universal_dir = tmp.path().join(".agents/skills/find-bugs");
        let codex_dir = tmp.path().join(".codex/skills/find-bugs");
        let universal_skill_md = write_invocation_skill(&universal_dir);
        let universal = invocation_deployment(
            &universal_dir,
            "shared",
            "universal",
            SkillDestination::Universal,
        );
        let mut codex_link =
            invocation_deployment(&codex_dir, "Codex", "codex", SkillDestination::Universal);
        codex_link.is_symlink = true;
        codex_link.backing = BackingRelationship::LinkedTo {
            deployment_id: universal.id.clone(),
        };
        let snapshot = invocation_snapshot(vec![universal, codex_link]);

        let deployment =
            exact_snapshot_invocation_deployment(&snapshot, "find-bugs", &universal_skill_md)
                .unwrap();
        set_skill_invocation_with(
            &universal_skill_md,
            InvocationPolicy::UserOnly,
            deployment.agent == "Codex",
        )
        .unwrap();

        assert!(!codex_openai_yaml_path(&universal_dir).exists());
    }

    #[test]
    fn exact_invocation_deployment_rejects_stale_name_and_ambiguous_path() {
        use super::super::skill_deployment::SkillDestination;

        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude/skills/find-bugs");
        let skill_md = write_invocation_skill(&claude_dir);
        let deployment = invocation_deployment(
            &claude_dir,
            "Claude Code",
            "claude-code",
            SkillDestination::PerHarness,
        );
        let snapshot = invocation_snapshot(vec![deployment.clone()]);

        let stale =
            exact_snapshot_invocation_deployment(&snapshot, "other-name", &skill_md).unwrap_err();
        assert!(stale.contains("stale"));

        let ambiguous = invocation_snapshot(vec![deployment.clone(), deployment]);
        let error =
            exact_snapshot_invocation_deployment(&ambiguous, "find-bugs", &skill_md).unwrap_err();
        assert!(error.contains("ambiguous"));
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
