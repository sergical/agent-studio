// ============================================================================
// Skills Module - skill_invocation
// Sets a skill's invocation policy - "Both" (default), "User only"
// (`disable-model-invocation: true`), or "Model only" (`user-invocable:
// false`) - by rewriting just those two frontmatter keys, leaving every
// other line of `SKILL.md` byte-identical. See skill_document.rs's
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
use tauri::Manager;

use super::commands::{canonicalize_skill_md, check_skill_md_deployment_write_allowed};
use super::frontmatter::InvocationPolicy;
use super::skill_deployment::parse_deployment_id;
use super::skill_dto::Deployment;
use super::skill_md_write::begin_skill_md_write_transaction;
use super::skill_ownership::LifecycleOwnerKind;
use super::skill_refresh::{self, SkillRefreshState, SkillSnapshot};

pub use skill_studio_core::skill_invocation_edit::rewrite_invocation_frontmatter;

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
pub async fn set_skill_invocation(
    name: String,
    path: String,
    policy: InvocationPolicy,
    app: tauri::AppHandle,
    refresh_state: tauri::State<'_, SkillRefreshState>,
    fork_lock: tauri::State<'_, super::skill_fork::ForkMutationLock>,
    event_store: tauri::State<'_, super::event_commands::EventStoreState>,
) -> Result<(), String> {
    let path_buf = PathBuf::from(&path);
    let snapshot = refresh_state
        .snapshot
        .read()
        .map_err(|error| format!("Snapshot lock poisoned: {error}"))?
        .clone()
        .ok_or_else(|| format!("Invocation target is stale: {path} is not an installed skill"))?;
    let deployment = exact_snapshot_invocation_deployment(&snapshot, &name, &path_buf)?;
    check_skill_md_deployment_write_allowed(deployment)?;
    if deployment.owner_kind == LifecycleOwnerKind::Copy {
        let _ = (fork_lock, event_store);
        let deployment_id = deployment.id.clone();
        let copy_name = name.clone();
        let copy_app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let fork_lock = copy_app.state::<super::skill_fork::ForkMutationLock>();
            let _fork = fork_lock.try_acquire()?;
            let home = dirs::home_dir().ok_or("Could not find home directory")?;
            let projects = super::skill_project_authority::scoped_projects(&home, [])?;
            let scope = super::skill_scope_config::desktop_skill_scope(&home, &projects)?;
            let event_store = copy_app.state::<super::event_commands::EventStoreState>();
            let store = event_store
                .0
                .lock()
                .map_err(|_| "Event store lock is unavailable")?;
            let store = store.as_ref().ok_or("Event store is unavailable")?;
            let mut service = skill_studio_core::skill_service::ScopedSkillService::bind(scope)
                .map_err(|error| error.to_string())?;
            let result = super::skill_copy_repair::apply_invocation(
                &mut service,
                store,
                &deployment_id,
                policy,
                &super::event_store::allocate_id(),
                skill_studio_core::skill_service::CancellationToken::default(),
            );
            let refresh = copy_app.state::<SkillRefreshState>();
            if let Err(error) =
                skill_refresh::reconcile_skill_names_and_emit(&copy_app, &refresh, [copy_name], &[])
            {
                eprintln!(
                    "[set_skill_invocation] targeted snapshot reconciliation failed: {error}"
                );
                refresh.mark_skills_dirty();
            }
            result
        })
        .await
        .map_err(|error| format!("Invocation worker failed: {error}"))??;
        return Ok(());
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
            ledger_only: Vec::new(),
            diagnosis: None,
            read_warnings: Vec::new(),
            revision: 1,
            full_refresh: None,
            skills: vec![InstalledSkill {
                update_sources: Vec::new(),
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
