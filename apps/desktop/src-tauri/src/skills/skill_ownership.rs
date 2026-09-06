// ============================================================================
// Skills Module - skill_ownership
// Resolves skills.sh and dotagents ownership against the matching
// scope/root/ledger entry. Same-named deployments elsewhere do not inherit
// ownership. Aggregate grouping by skill name stays presentation-only.
// ============================================================================

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::dotagents_ledger::{self, DotagentsSkill};
use super::lock_file::{self, SkillLockFile};
use super::provenance::SourceKind;
use super::skill_candidate::SkillCandidate;
use super::skill_deployment::{
    agents_root_for_skills_dir, encode_id_path, is_universal_root_label,
    path_is_under_universal_skills, SkillDestination,
};
use super::skill_dto::InstallScope;
use super::skill_fork_registry::CopyDeploymentRecord;

/// The owner allowed to change a deployment. Read-only kinds use `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum LifecycleOwnerKind {
    SkillsSh,
    Dotagents,
    Copy,
    Fork,
    Plugin,
    InRepo,
    #[default]
    Manual,
    WildcardDotagents,
    Ambiguous,
}

impl LifecycleOwnerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SkillsSh => "skills-sh",
            Self::Dotagents => "dotagents",
            Self::Copy => "copy",
            Self::Fork => "fork",
            Self::Plugin => "plugin",
            Self::InRepo => "in-repo",
            Self::Manual => "manual",
            Self::WildcardDotagents => "wildcard-dotagents",
            Self::Ambiguous => "ambiguous",
        }
    }

    /// True when Skill Studio may run an owner adapter against this kind.
    pub fn is_mutable(self) -> bool {
        matches!(
            self,
            Self::SkillsSh | Self::Dotagents | Self::Copy | Self::Fork
        )
    }
}

/// One ledger that can own Universal deployments in a given `.agents` root.
#[derive(Debug, Clone)]
pub struct OwnershipLedgers {
    pub agents_dir: PathBuf,
    pub scope: InstallScope,
    pub project_path: Option<PathBuf>,
    pub lock: SkillLockFile,
    pub dotagents: Vec<DotagentsSkill>,
}

/// Load the home Universal ledger (`~/.agents`) plus one ledger per project
/// that has `.agents/agents.toml`, `agents.lock`, or `.skill-lock.json`.
pub fn load_ownership_ledgers(home: &Path, project_paths: &[PathBuf]) -> Vec<OwnershipLedgers> {
    let mut out = Vec::new();
    out.push(read_ledgers(
        home.join(".agents"),
        InstallScope::Global,
        None,
    ));
    for project in project_paths {
        let agents_dir = project.join(".agents");
        if agents_dir.join("agents.toml").exists()
            || agents_dir.join("agents.lock").exists()
            || agents_dir.join(".skill-lock.json").exists()
        {
            out.push(read_ledgers(
                agents_dir,
                InstallScope::Project,
                Some(project.clone()),
            ));
        }
    }
    out
}

fn read_ledgers(
    agents_dir: PathBuf,
    scope: InstallScope,
    project_path: Option<PathBuf>,
) -> OwnershipLedgers {
    let lock = lock_file::read_lock_file_at(&agents_dir.join(".skill-lock.json")).unwrap_or(
        SkillLockFile {
            version: 3,
            skills: HashMap::new(),
        },
    );
    let dotagents = dotagents_ledger::read_dotagents_ledger(&agents_dir).unwrap_or_default();
    OwnershipLedgers {
        agents_dir,
        scope,
        project_path,
        lock,
        dotagents,
    }
}

/// The ledger that matches this candidate's Universal root, if any.
pub fn ledger_for_candidate<'a>(
    candidate: &SkillCandidate,
    ledgers: &'a [OwnershipLedgers],
) -> Option<&'a OwnershipLedgers> {
    let skills_dir = candidate.path.parent()?;
    if !is_universal_root_label(&candidate.root_label)
        && !path_is_under_universal_skills(&candidate.path)
    {
        // A per-harness copy never inherits a Universal ledger.
        if candidate.root_label != "parked" {
            return None;
        }
    }
    let agents_dir = agents_root_for_skills_dir(skills_dir)?;
    ledgers
        .iter()
        .find(|ledger| ledger.agents_dir == agents_dir)
}

/// Classify ownership for one candidate against the matching ledger only.
pub fn classify_lifecycle_owner(
    candidate: &SkillCandidate,
    ledgers: &[OwnershipLedgers],
    destination: SkillDestination,
    deployment_id: &str,
    copy_records: &std::collections::BTreeMap<String, CopyDeploymentRecord>,
) -> (LifecycleOwnerKind, Option<String>, SourceKind) {
    if candidate.plugin.is_some() {
        return (LifecycleOwnerKind::Plugin, None, SourceKind::Plugin);
    }

    if copy_record_matches_candidate(copy_records, deployment_id, candidate, destination) {
        return (LifecycleOwnerKind::Copy, None, SourceKind::Manual);
    }

    if destination == SkillDestination::PerHarness {
        if candidate.in_git_repo {
            return (LifecycleOwnerKind::InRepo, None, SourceKind::InRepo);
        }
        return (LifecycleOwnerKind::Manual, None, SourceKind::Manual);
    }

    let Some(ledger) = ledger_for_candidate(candidate, ledgers) else {
        let kind = if candidate.in_git_repo {
            SourceKind::InRepo
        } else {
            SourceKind::Manual
        };
        let owner = if candidate.in_git_repo {
            LifecycleOwnerKind::InRepo
        } else {
            LifecycleOwnerKind::Manual
        };
        return (owner, None, kind);
    };

    let owner_id = owner_id_for(ledger, &candidate.name);
    let dotagents_entry = ledger.dotagents.iter().find(|s| s.name == candidate.name);
    let skills_sh_entry = ledger.lock.skills.contains_key(&candidate.name);
    if dotagents_entry.is_some() && skills_sh_entry {
        return (LifecycleOwnerKind::Ambiguous, None, SourceKind::Dotagents);
    }

    if let Some(entry) = dotagents_entry {
        if !entry.has_manifest_row {
            return (
                LifecycleOwnerKind::WildcardDotagents,
                Some(owner_id),
                SourceKind::Dotagents,
            );
        }
        return (
            LifecycleOwnerKind::Dotagents,
            Some(owner_id),
            SourceKind::Dotagents,
        );
    }

    if skills_sh_entry {
        return (
            LifecycleOwnerKind::SkillsSh,
            Some(owner_id),
            SourceKind::SkillsSh,
        );
    }

    // Compatibility: a Universal root next to agents.toml/lock without a
    // named row was classified as Dotagents. Its owner is ambiguous, so keep
    // the display kind but refuse owner-wide actions.
    if candidate.shared_root_has_lock_entry {
        return (LifecycleOwnerKind::Ambiguous, None, SourceKind::Dotagents);
    }

    if candidate.is_symlink {
        if let Some(target) = &candidate.symlink_target {
            if path_is_under_universal_skills(target) {
                return (LifecycleOwnerKind::Ambiguous, None, SourceKind::Dotagents);
            }
        }
    }

    let kind = if candidate.in_git_repo {
        SourceKind::InRepo
    } else {
        SourceKind::Manual
    };
    let owner = if candidate.in_git_repo {
        LifecycleOwnerKind::InRepo
    } else {
        LifecycleOwnerKind::Manual
    };
    (owner, None, kind)
}

fn copy_record_matches_candidate(
    records: &std::collections::BTreeMap<String, CopyDeploymentRecord>,
    deployment_id: &str,
    candidate: &SkillCandidate,
    destination: SkillDestination,
) -> bool {
    let Some(record) = records.get(deployment_id) else {
        return false;
    };
    let scope = match candidate.scope.as_str() {
        "global" => InstallScope::Global,
        "project" => InstallScope::Project,
        _ => return false,
    };
    record.deployment_id == deployment_id
        && !record.content_hash.is_empty()
        && record.content_hash == candidate.content_hash
        && record.name == candidate.name
        && record.path == candidate.path
        && record.scope == scope
        && record.destination == destination
        && record.disabled == candidate.studio_disabled
        && record.project_path.as_deref()
            == candidate
                .project_path
                .as_ref()
                .map(|path| path.to_string_lossy())
                .as_deref()
        && super::skill_deployment::parse_deployment_id(deployment_id)
            .is_some_and(|parsed| parsed.slot == record.slot && parsed.lexical_path == record.path)
}

pub fn owner_id_for(ledger: &OwnershipLedgers, name: &str) -> String {
    match (&ledger.scope, &ledger.project_path) {
        (InstallScope::Global, _) => format!("owner:v1/global/{name}"),
        (InstallScope::Project, Some(path)) => {
            format!(
                "owner:v1/project/{}/{}",
                encode_id_path(&path.to_string_lossy()),
                name
            )
        }
        (InstallScope::Project, None) => format!("owner:v1/project/-/{name}"),
    }
}

/// Parse `owner:v1/global/<name>` or `owner:v1/project/<encoded>/<name>`.
pub fn parse_owner_id(id: &str) -> Option<ParsedOwnerId> {
    let rest = id.strip_prefix("owner:v1/")?;
    if let Some(name) = rest.strip_prefix("global/") {
        return Some(ParsedOwnerId {
            scope: InstallScope::Global,
            project_path: None,
            name: name.to_string(),
        });
    }
    let rest = rest.strip_prefix("project/")?;
    let (encoded, name) = rest.rsplit_once('/')?;
    let project_path = if encoded == "-" {
        None
    } else {
        Some(encoded.replace("%2F", "/").replace("%25", "%"))
    };
    Some(ParsedOwnerId {
        scope: InstallScope::Project,
        project_path,
        name: name.to_string(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedOwnerId {
    pub scope: InstallScope,
    pub project_path: Option<String>,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;

    fn candidate(name: &str, root_label: &str, path: &str) -> SkillCandidate {
        SkillCandidate {
            name: name.to_string(),
            path: PathBuf::from(path),
            root_label: root_label.to_string(),
            scope: "global".to_string(),
            project_path: None,
            is_symlink: false,
            symlink_target: None,
            resolved_path: None,
            symlink_is_broken: false,
            symlink_error: None,
            plugin: None,
            shared_root_has_lock_entry: false,
            frontmatter: None,
            frontmatter_fields: BTreeMap::new(),
            spec_violations: Vec::new(),
            has_spec: false,
            folder_bytes: 0,
            file_count: 0,
            skill_md_tokens: 0,
            description_tokens: 0,
            content_hash: String::new(),
            modified_at: None,
            folder_truncated: false,
            in_git_repo: false,
            studio_disabled: false,
            shared_via_whole_dir_link: false,
        }
    }

    #[test]
    fn skills_sh_lock_only_matches_same_agents_root() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("proj");
        fs::create_dir_all(home.join(".agents/skills/find-bugs")).unwrap();
        fs::create_dir_all(project.join(".agents/skills/find-bugs")).unwrap();
        fs::write(
            home.join(".agents/.skill-lock.json"),
            r#"{"version":3,"skills":{"find-bugs":{"source":"o/r","sourceType":"github","sourceUrl":"https://github.com/o/r","skillFolderHash":"a","installedAt":"t","updatedAt":"t"}}}"#,
        )
        .unwrap();

        let ledgers = load_ownership_ledgers(&home, std::slice::from_ref(&project));
        let global = candidate(
            "find-bugs",
            "shared",
            &home.join(".agents/skills/find-bugs").to_string_lossy(),
        );
        let project_c = {
            let mut c = candidate(
                "find-bugs",
                "shared",
                &project.join(".agents/skills/find-bugs").to_string_lossy(),
            );
            c.scope = "project".to_string();
            c.project_path = Some(project.clone());
            c
        };

        let (g_owner, _, g_kind) = classify_lifecycle_owner(
            &global,
            &ledgers,
            SkillDestination::Universal,
            "missing",
            &Default::default(),
        );
        let (p_owner, _, p_kind) = classify_lifecycle_owner(
            &project_c,
            &ledgers,
            SkillDestination::Universal,
            "missing",
            &Default::default(),
        );
        assert_eq!(g_owner, LifecycleOwnerKind::SkillsSh);
        assert_eq!(g_kind, SourceKind::SkillsSh);
        assert_eq!(p_owner, LifecycleOwnerKind::Manual);
        assert_eq!(p_kind, SourceKind::Manual);
    }

    fn write_dual_ledger(agents_dir: &Path, name: &str) {
        fs::create_dir_all(agents_dir.join("skills").join(name)).unwrap();
        fs::write(
            agents_dir.join("agents.toml"),
            format!("[[skills]]\nname = \"{name}\"\nsource = \"o/r\"\n"),
        )
        .unwrap();
        fs::write(
            agents_dir.join("agents.lock"),
            format!(
                "[skills.{name}]\nsource = \"o/r\"\nresolved_path = \"skills/{name}\"\nresolved_commit = \"abc\"\n"
            ),
        )
        .unwrap();
        fs::write(
            agents_dir.join(".skill-lock.json"),
            format!(
                r#"{{"version":3,"skills":{{"{name}":{{"source":"x/y","sourceType":"github","sourceUrl":"https://github.com/x/y","skillFolderHash":"a","installedAt":"t","updatedAt":"t"}}}}}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn exact_global_dual_ledger_owner_is_ambiguous_and_read_only() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_dual_ledger(&home.join(".agents"), "find-bugs");
        let candidate = candidate(
            "find-bugs",
            "shared",
            &home.join(".agents/skills/find-bugs").to_string_lossy(),
        );
        let ledgers = load_ownership_ledgers(home, &[]);

        let (owner, owner_id, _) = classify_lifecycle_owner(
            &candidate,
            &ledgers,
            SkillDestination::Universal,
            "missing",
            &Default::default(),
        );

        assert_eq!(owner, LifecycleOwnerKind::Ambiguous);
        assert!(owner_id.is_none());
        assert!(!owner.is_mutable());
    }

    #[test]
    fn exact_project_dual_ledger_owner_is_ambiguous_and_read_only() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        write_dual_ledger(&project.join(".agents"), "find-bugs");
        let mut candidate = candidate(
            "find-bugs",
            "shared",
            &project.join(".agents/skills/find-bugs").to_string_lossy(),
        );
        candidate.scope = "project".to_string();
        candidate.project_path = Some(project.clone());
        let ledgers = load_ownership_ledgers(&home, std::slice::from_ref(&project));

        let (owner, owner_id, _) = classify_lifecycle_owner(
            &candidate,
            &ledgers,
            SkillDestination::Universal,
            "missing",
            &Default::default(),
        );

        assert_eq!(owner, LifecycleOwnerKind::Ambiguous);
        assert!(owner_id.is_none());
        assert!(!owner.is_mutable());
    }

    #[test]
    fn frontmatter_name_cannot_claim_a_skills_sh_owner() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let skill_dir = home.join(".agents/skills/foo");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: bar\ndescription: mismatch\n---\nbody",
        )
        .unwrap();
        fs::write(
            home.join(".agents/.skill-lock.json"),
            r#"{"version":3,"skills":{"bar":{"source":"o/r","sourceType":"github","sourceUrl":"https://github.com/o/r","skillFolderHash":"a","installedAt":"t","updatedAt":"t"}}}"#,
        )
        .unwrap();

        let candidate = super::super::skill_discovery::discover_skill_candidates(home, &[])
            .into_iter()
            .find(|candidate| candidate.path == skill_dir)
            .unwrap();
        let ledgers = load_ownership_ledgers(home, &[]);
        let (owner, owner_id, _) = classify_lifecycle_owner(
            &candidate,
            &ledgers,
            SkillDestination::Universal,
            "missing",
            &Default::default(),
        );
        assert_eq!(candidate.name, "foo");
        assert_eq!(owner, LifecycleOwnerKind::Manual);
        assert!(owner_id.is_none());
        assert!(candidate
            .spec_violations
            .iter()
            .any(|violation| violation.contains("does not match its directory name \"foo\"")));
    }

    #[test]
    fn frontmatter_name_cannot_claim_a_dotagents_owner() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let agents = home.join(".agents");
        let skill_dir = agents.join("skills/foo");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: bar\ndescription: mismatch\n---\nbody",
        )
        .unwrap();
        fs::write(
            agents.join("agents.toml"),
            "[[skills]]\nname = \"bar\"\nsource = \"o/r\"\n",
        )
        .unwrap();
        fs::write(
            agents.join("agents.lock"),
            "[skills.bar]\nsource = \"o/r\"\nresolved_path = \"skills/bar\"\nresolved_commit = \"abc\"\n",
        )
        .unwrap();

        let candidate = super::super::skill_discovery::discover_skill_candidates(home, &[])
            .into_iter()
            .find(|candidate| candidate.path == skill_dir)
            .unwrap();
        let ledgers = load_ownership_ledgers(home, &[]);
        let (owner, owner_id, _) = classify_lifecycle_owner(
            &candidate,
            &ledgers,
            SkillDestination::Universal,
            "missing",
            &Default::default(),
        );
        assert_eq!(candidate.name, "foo");
        assert_eq!(owner, LifecycleOwnerKind::Ambiguous);
        assert!(owner_id.is_none());
    }

    #[test]
    fn unrecorded_first_class_per_harness_folders_remain_manual_despite_universal_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(
            home.join(".agents/.skill-lock.json"),
            r#"{"version":3,"skills":{"find-bugs":{"source":"o/r","sourceType":"github","sourceUrl":"https://github.com/o/r","skillFolderHash":"a","installedAt":"t","updatedAt":"t"}}}"#,
        )
        .unwrap();
        let ledgers = load_ownership_ledgers(&home, &[]);
        let harnesses = [
            ("Claude Code", ".claude/skills"),
            ("Codex", ".codex/skills"),
            ("OpenCode", ".config/opencode/skills"),
            ("pi", ".pi/agent/skills"),
            ("Cursor", ".cursor/skills"),
            ("Grok Build", ".grok/skills"),
        ];
        for (label, root) in harnesses {
            let manual = candidate(
                "find-bugs",
                label,
                &home.join(root).join("find-bugs").to_string_lossy(),
            );
            let (id, destination, _) = super::super::skill_deployment::id_for_candidate(
                super::super::skill_deployment::DeploymentCandidate {
                    name: &manual.name,
                    root_label: &manual.root_label,
                    scope: &manual.scope,
                    path: &manual.path,
                    project_path: None,
                    is_symlink: false,
                    symlink_target: None,
                    resolved_path: None,
                    shared_via_whole_dir_link: false,
                },
            );
            let (owner, owner_id, kind) =
                classify_lifecycle_owner(&manual, &ledgers, destination, &id, &Default::default());
            assert_eq!(owner, LifecycleOwnerKind::Manual, "{label}");
            assert!(owner_id.is_none(), "{label}");
            assert_eq!(kind, SourceKind::Manual, "{label}");
            assert!(!owner.is_mutable(), "{label}");
        }
    }

    #[test]
    fn copy_ownership_requires_an_exact_recorded_deployment_identity() {
        let mut candidate = candidate("find-bugs", "Codex", "/h/.codex/skills/find-bugs");
        candidate.content_hash = "installed-hash".to_string();
        let id = super::super::skill_deployment::deployment_id(
            "find-bugs",
            "global",
            SkillDestination::PerHarness,
            "codex",
            None,
            &candidate.path,
        );
        let record = CopyDeploymentRecord {
            deployment_id: id.clone(),
            name: "find-bugs".to_string(),
            path: candidate.path.clone(),
            scope: InstallScope::Global,
            destination: SkillDestination::PerHarness,
            slot: "codex".to_string(),
            project_path: None,
            content_hash: candidate.content_hash.clone(),
            disabled: false,
        };
        let records = std::collections::BTreeMap::from([(id.clone(), record)]);
        let (owner, owner_id, _) =
            classify_lifecycle_owner(&candidate, &[], SkillDestination::PerHarness, &id, &records);
        assert_eq!(owner, LifecycleOwnerKind::Copy);
        assert!(owner_id.is_none());

        let wrong_id = id.replace("/codex/", "/claude-code/");
        let (owner, _, _) = classify_lifecycle_owner(
            &candidate,
            &[],
            SkillDestination::PerHarness,
            &wrong_id,
            &records,
        );
        assert_eq!(owner, LifecycleOwnerKind::Manual);
    }

    fn discover_copy_candidate(home: &Path, skill_dir: &Path) -> SkillCandidate {
        super::super::skill_discovery::discover_skill_candidates(home, &[])
            .into_iter()
            .find(|candidate| candidate.path == skill_dir)
            .unwrap()
    }

    fn copy_record(candidate: &SkillCandidate, deployment_id: &str) -> CopyDeploymentRecord {
        CopyDeploymentRecord {
            deployment_id: deployment_id.to_string(),
            name: candidate.name.clone(),
            path: candidate.path.clone(),
            scope: InstallScope::Global,
            destination: SkillDestination::PerHarness,
            slot: "codex".to_string(),
            project_path: None,
            content_hash: candidate.content_hash.clone(),
            disabled: false,
        }
    }

    fn discovered_copy_fixture() -> (tempfile::TempDir, PathBuf, SkillCandidate, String) {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join(".codex/skills/find-bugs");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "original content").unwrap();
        let candidate = discover_copy_candidate(tmp.path(), &skill_dir);
        let deployment_id = super::super::skill_deployment::deployment_id(
            "find-bugs",
            "global",
            SkillDestination::PerHarness,
            "codex",
            None,
            &skill_dir,
        );
        (tmp, skill_dir, candidate, deployment_id)
    }

    #[test]
    fn copy_ownership_rejects_a_replacement_at_the_same_path() {
        let (tmp, skill_dir, original, deployment_id) = discovered_copy_fixture();
        let record = copy_record(&original, &deployment_id);
        fs::remove_dir_all(&skill_dir).unwrap();
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "replacement content").unwrap();
        let replacement = discover_copy_candidate(tmp.path(), &skill_dir);

        let (owner, _, _) = classify_lifecycle_owner(
            &replacement,
            &[],
            SkillDestination::PerHarness,
            &deployment_id,
            &BTreeMap::from([(deployment_id.clone(), record)]),
        );

        assert_eq!(owner, LifecycleOwnerKind::Manual);
        assert!(!owner.is_mutable());
    }

    #[test]
    fn copy_ownership_rejects_edited_content() {
        let (tmp, skill_dir, original, deployment_id) = discovered_copy_fixture();
        let record = copy_record(&original, &deployment_id);
        fs::write(skill_dir.join("SKILL.md"), "edited content").unwrap();
        let edited = discover_copy_candidate(tmp.path(), &skill_dir);

        let (owner, _, _) = classify_lifecycle_owner(
            &edited,
            &[],
            SkillDestination::PerHarness,
            &deployment_id,
            &BTreeMap::from([(deployment_id.clone(), record)]),
        );

        assert_eq!(owner, LifecycleOwnerKind::Manual);
        assert!(!owner.is_mutable());
    }

    #[test]
    fn copy_ownership_rejects_an_empty_legacy_content_hash() {
        let (_tmp, _skill_dir, candidate, deployment_id) = discovered_copy_fixture();
        let mut record = copy_record(&candidate, &deployment_id);
        record.content_hash.clear();

        let (owner, _, _) = classify_lifecycle_owner(
            &candidate,
            &[],
            SkillDestination::PerHarness,
            &deployment_id,
            &BTreeMap::from([(deployment_id.clone(), record)]),
        );

        assert_eq!(owner, LifecycleOwnerKind::Manual);
        assert!(!owner.is_mutable());
    }

    #[test]
    fn named_dotagents_row_is_mutable_dotagents() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let agents = home.join(".agents");
        fs::create_dir_all(agents.join("skills/find-bugs")).unwrap();
        fs::write(
            agents.join("agents.lock"),
            "[skills.find-bugs]\nsource = \"o/r\"\nresolved_path = \"skills/find-bugs\"\nresolved_commit = \"abc\"\n",
        )
        .unwrap();
        fs::write(
            agents.join("agents.toml"),
            "[[skills]]\nname = \"find-bugs\"\nsource = \"o/r\"\n",
        )
        .unwrap();
        let ledgers = load_ownership_ledgers(&home, &[]);
        let c = candidate(
            "find-bugs",
            "shared",
            &agents.join("skills/find-bugs").to_string_lossy(),
        );
        let (owner, id, kind) = classify_lifecycle_owner(
            &c,
            &ledgers,
            SkillDestination::Universal,
            "missing",
            &Default::default(),
        );
        assert_eq!(owner, LifecycleOwnerKind::Dotagents);
        assert_eq!(kind, SourceKind::Dotagents);
        assert_eq!(id.as_deref(), Some("owner:v1/global/find-bugs"));
    }

    #[test]
    fn wildcard_dotagents_is_read_only() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let agents = home.join(".agents");
        fs::create_dir_all(agents.join("skills/find-bugs")).unwrap();
        fs::write(
            agents.join("agents.lock"),
            "[skills.find-bugs]\nsource = \"o/r\"\nresolved_path = \"skills/find-bugs\"\nresolved_commit = \"abc\"\n",
        )
        .unwrap();
        fs::write(agents.join("agents.toml"), "").unwrap();
        let ledgers = load_ownership_ledgers(&home, &[]);
        let c = candidate(
            "find-bugs",
            "shared",
            &agents.join("skills/find-bugs").to_string_lossy(),
        );
        let (owner, _, _) = classify_lifecycle_owner(
            &c,
            &ledgers,
            SkillDestination::Universal,
            "missing",
            &Default::default(),
        );
        assert_eq!(owner, LifecycleOwnerKind::WildcardDotagents);
        assert!(!owner.is_mutable());
    }

    #[test]
    fn owner_id_round_trips_project() {
        let parsed = parse_owner_id("owner:v1/project/%2Fwork%2Fapp/find-bugs").unwrap();
        assert_eq!(parsed.scope, InstallScope::Project);
        assert_eq!(parsed.project_path.as_deref(), Some("/work/app"));
        assert_eq!(parsed.name, "find-bugs");
    }
}
