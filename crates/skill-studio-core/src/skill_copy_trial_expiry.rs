use crate::{
    skill_copy_registry_removal::{raw_trial_relates, remove_copy_records},
    skill_copy_removal::CopyRemovalReader,
    skill_deployment::{parse_deployment_id, InstallScope, SkillDestination},
    skill_fork_registry::{AddMethod, CopyDeploymentRecord, TrialRecord, TrialScope, TrialStatus},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

mod backup_execution;
pub use backup_execution::{
    begin_copy_trial_expiry, expire_copy_trial, recover_copy_trial_expiry, CopyTrialExpiryError,
    CopyTrialExpiryReceipt, CopyTrialExpiryRequest, PendingCopyTrialExpiry,
};

/// Registry evidence only. The executor must independently admit current scope,
/// ownership, content and physical reader links under its write lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CopyTrialRegistrySelection {
    selected: CopyDeploymentRecord,
    readers: Vec<CopyRemovalReader>,
    copies: BTreeMap<String, Value>,
    trials: BTreeMap<String, Value>,
    trial: TrialRecord,
}

pub const EVENT_KIND: &str = "expire_copy_trial";

/// The immutable, home-scoped portion of a completed expiry that a later
/// restore may use. It deliberately does not require the original project to
/// remain registered: the retained backup belongs to the current home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedCopyTrialBackup {
    pub source_event_id: String,
    pub name: String,
    pub backup: PathBuf,
    pub expected_tree: String,
}

/// Durable evidence, not write authority. A resumed executor must validate the
/// current scope and re-admit each effect under a fresh lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyTrialExpiryIntent {
    version: u32,
    event_id: String,
    admitted_at: DateTime<Utc>,
    home: PathBuf,
    #[serde(deserialize_with = "deserialize_selection")]
    selection: CopyTrialRegistrySelection,
    expected_tree: String,
}

fn deserialize_selection<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<CopyTrialRegistrySelection, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct SavedSelection {
        selected: CopyDeploymentRecord,
        readers: Vec<CopyRemovalReader>,
        copies: BTreeMap<String, Value>,
        trials: BTreeMap<String, Value>,
        trial: TrialRecord,
    }
    let saved = SavedSelection::deserialize(deserializer)?;
    Ok(CopyTrialRegistrySelection {
        selected: saved.selected,
        readers: saved.readers,
        copies: saved.copies,
        trials: saved.trials,
        trial: saved.trial,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct CopyTrialExpiryPaths {
    pub registry: PathBuf,
    pub source: PathBuf,
    pub source_quarantine: PathBuf,
    pub reader_stages: Vec<PathBuf>,
    pub hidden_trash: PathBuf,
    pub visible_trash: PathBuf,
}

impl CopyTrialExpiryIntent {
    fn trash_invocation(&self) -> String {
        let selected = &self.selection.selected;
        let digest = Sha256::digest(selected.deployment_id.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        format!(
            "{}-{}--v2-{}-{}",
            selected.name,
            self.admitted_at.format("%Y%m%d-%H%M%S"),
            &digest[..16],
            self.event_id
        )
    }

    pub fn new(
        scope: &crate::skill_service::SkillScope,
        event_id: String,
        selection: CopyTrialRegistrySelection,
        expected_tree: String,
        admitted_at: DateTime<Utc>,
    ) -> Result<Self, String> {
        let intent = Self {
            version: 1,
            event_id,
            admitted_at,
            home: scope.home.clone(),
            selection,
            expected_tree,
        };
        intent.paths_for_scope(scope)?;
        Ok(intent)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || ulid::Ulid::from_string(&self.event_id).is_err()
            || !crate::skill_trial_restore::valid_skill_name(&self.selection.selected.name)
            || self.event_id.len() != 26
            || !self
                .expected_tree
                .strip_prefix("tree-v1:")
                .is_some_and(|hash| {
                    hash.len() == 64
                        && hash
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
            || !clean_absolute(&self.home)
        {
            return Err("Invalid Copy trial expiry intent identity".into());
        }
        let registry = serde_json::to_vec(&serde_json::json!({
            "copies": self.selection.copies, "trials": self.selection.trials,
        }))
        .map_err(|error| error.to_string())?;
        let reconstructed = select_due_copy_trial(
            &registry,
            &self.selection.selected,
            &self.selection.readers,
            &self.selection.trial.deployment_fingerprint,
            self.admitted_at,
        )?;
        if reconstructed != self.selection {
            return Err("Copy trial expiry intent records do not agree".into());
        }
        Ok(())
    }

    /// Lexical scope checks only. The executor must also bind and validate actual
    /// filesystem entries, including symlinks and device boundaries.
    pub fn paths_for_scope(
        &self,
        scope: &crate::skill_service::SkillScope,
    ) -> Result<CopyTrialExpiryPaths, String> {
        self.validate()?;
        if self.home != scope.home {
            return Err("Copy trial expiry home is outside the current scope".into());
        }
        let selected = &self.selection.selected;
        let project = selected.project_path.as_deref().map(PathBuf::from);
        let base = match &project {
            Some(project) if scope.projects.contains(project) => project,
            Some(_) => return Err("Copy trial expiry project is outside the current scope".into()),
            None => &scope.home,
        };
        let source_root = base.join(".agents/skills");
        let source = if selected.disabled {
            source_root
                .join(".skill-studio-disabled")
                .join(&selected.name)
        } else {
            source_root.join(&selected.name)
        };
        if source != selected.path {
            return Err("Copy trial expiry source is not the scoped Universal Copy".into());
        }
        let roots = crate::skill_agents::skill_roots(&scope.home, &scope.projects);
        let mut reader_stages = Vec::new();
        for (index, reader) in self.selection.readers.iter().enumerate() {
            let root = roots
                .iter()
                .find(|root| {
                    root.project_path == project
                        && root.label != "parked"
                        && root.path.join(&selected.name) == reader.path
                })
                .ok_or("Copy trial reader is outside the selected scope")?;
            reader_stages.push(
                root.path
                    .parent()
                    .ok_or("Copy trial reader root has no parent")?
                    .join(".skill-studio-expiring")
                    .join(format!("{}-reader-{index}", self.event_id)),
            );
        }
        let invocation = self.trash_invocation();
        let trash = scope.home.join(".agents/skills-trash");
        Ok(CopyTrialExpiryPaths {
            registry: scope.home.join(".agents/skill-studio.json"),
            source,
            source_quarantine: base
                .join(".agents/.skill-studio-expiring")
                .join(&self.event_id),
            reader_stages,
            hidden_trash: trash
                .join(format!(".trial-expiry-{}", self.event_id))
                .join("backup"),
            visible_trash: trash.join(invocation).join("backup"),
        })
    }

    pub fn event_draft(&self) -> Result<crate::skill_event::EventDraft, String> {
        self.validate()?;
        let selected = &self.selection.selected;
        Ok(crate::skill_event::EventDraft {
            kind: EVENT_KIND.into(),
            skill: selected.name.clone(),
            harness: Some(selected.slot.clone()),
            scope: Some(
                match selected.scope {
                    InstallScope::Global => "global",
                    InstallScope::Project => "project",
                }
                .into(),
            ),
            project_path: selected.project_path.clone(),
            payload: serde_json::to_value(self).map_err(|error| error.to_string())?,
            inverse: None,
            backup_dir: None,
            restorable: false,
        })
    }

    pub fn from_event(row: &crate::skill_event::EventRow) -> Result<Self, String> {
        let intent: Self =
            serde_json::from_value(row.payload.clone()).map_err(|error| error.to_string())?;
        let expected = intent.event_draft()?;
        if row.id != intent.event_id
            || row.kind != expected.kind
            || row.skill != expected.skill
            || row.harness != expected.harness
            || row.scope != expected.scope
            || row.project_path != expected.project_path
            || row.restorable
            || row.inverse.is_some()
            || row.backup_dir.is_some()
        {
            return Err("Copy trial expiry event metadata does not match its intent".into());
        }
        Ok(intent)
    }

    /// Validates a completed expiry for restoration using only paths rooted at
    /// `home`. Unlike expiry execution, restore does not need authority over
    /// the original project or its former readers.
    pub fn completed_backup_for_home(
        row: &crate::skill_event::EventRow,
        home: &Path,
    ) -> Result<CompletedCopyTrialBackup, String> {
        if row.status != "done" || row.reverted_by.is_some() {
            return Err("Copy trial expiry is not available for backup restoration".into());
        }
        let intent = Self::from_event(row)?;
        if intent.home != home {
            return Err("Copy trial expiry home is outside the current scope".into());
        }
        let selected = &intent.selection.selected;
        let invocation = intent.trash_invocation();
        Ok(CompletedCopyTrialBackup {
            source_event_id: row.id.clone(),
            name: selected.name.clone(),
            backup: home
                .join(".agents/skills-trash")
                .join(invocation)
                .join("backup"),
            expected_tree: intent.expected_tree.clone(),
        })
    }
}

fn clean_absolute(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
}

impl CopyTrialRegistrySelection {
    pub fn trial(&self) -> &TrialRecord {
        &self.trial
    }

    /// Produces bytes for CAS against this same current document. This does not
    /// publish the document or authorize a filesystem effect.
    pub fn without_selected_records(&self, current: &[u8]) -> Result<Vec<u8>, String> {
        remove_copy_records(
            current,
            &self.selected,
            &self.readers,
            &self.copies,
            &self.trials,
        )
    }
}

pub fn select_due_copy_trial(
    registry: &[u8],
    selected: &CopyDeploymentRecord,
    readers: &[CopyRemovalReader],
    live_fingerprint: &str,
    now: DateTime<Utc>,
) -> Result<CopyTrialRegistrySelection, String> {
    if registry.len() > 8 * 1024 * 1024 {
        return Err("Copy registry exceeds its limit".into());
    }
    let parsed = parse_deployment_id(&selected.deployment_id)
        .ok_or("Copy trial has an invalid deployment ID")?;
    let (scope, expected_scope) = match selected.scope {
        InstallScope::Global => ("global", TrialScope::Global),
        InstallScope::Project => ("project", TrialScope::Project),
    };
    if selected.destination != SkillDestination::Universal
        || selected.slot != "universal"
        || parsed.name != selected.name
        || parsed.scope != scope
        || parsed.destination != selected.destination
        || parsed.slot != selected.slot
        || parsed.project_path != selected.project_path
        || parsed.lexical_path != selected.path
        || !selected.path.is_absolute()
        || selected
            .path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        || (selected.scope == InstallScope::Global && selected.project_path.is_some())
        || (selected.scope == InstallScope::Project && selected.project_path.is_none())
        || selected.project_path.as_ref().is_some_and(|project| {
            !Path::new(project).is_absolute()
                || Path::new(project)
                    .components()
                    .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        })
    {
        return Err("Copy trial deployment identity or Universal destination changed".into());
    }
    let document: Value = serde_json::from_slice(registry).map_err(|error| error.to_string())?;
    let copies = document
        .get("copies")
        .and_then(Value::as_object)
        .ok_or("Copy registry records are missing or invalid")?;
    let selected_raw = copies
        .get(&selected.deployment_id)
        .ok_or("Selected Copy registry record is absent")?;
    let actual: CopyDeploymentRecord =
        serde_json::from_value(selected_raw.clone()).map_err(|error| error.to_string())?;
    if &actual != selected {
        return Err("Copy trial registry ownership changed".into());
    }
    let trials = document
        .get("trials")
        .and_then(Value::as_object)
        .ok_or("Copy trial records are missing or invalid")?;
    let key = format!("deployment/{}", selected.deployment_id);
    let raw = trials
        .get(&key)
        .ok_or("Exact Copy trial record is absent")?;
    let trial: TrialRecord =
        serde_json::from_value(raw.clone()).map_err(|error| error.to_string())?;
    if trial.method != AddMethod::Copy
        || trial.deployment_id != selected.deployment_id
        || trial.skill_dir != selected.path
        || trial.scope != expected_scope
        || trial.project_path != selected.project_path
        || trial.deployment_fingerprint != live_fingerprint
        || live_fingerprint.len() != 64
        || !live_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("Copy trial identity, ownership or content changed".into());
    }
    match trial.status {
        TrialStatus::Expiring => {}
        TrialStatus::Active
            if DateTime::parse_from_rfc3339(&trial.expires_at)
                .is_ok_and(|expires_at| expires_at.with_timezone(&Utc) <= now) => {}
        _ => return Err("Copy trial is not due or needs manual recovery".into()),
    }
    match (&trial.claude_link, &trial.claude_link_target) {
        (None, None) => {}
        (Some(path), Some(target))
            if readers
                .iter()
                .any(|reader| &reader.path == path && &reader.raw_target == target) => {}
        _ => return Err("Copy trial Claude reader identity changed".into()),
    }
    if trials
        .iter()
        .any(|(other_key, value)| other_key != &key && raw_trial_relates(value, selected, readers))
    {
        return Err("Multiple trial records refer to the selected Copy or its readers".into());
    }
    let mut expected_copies =
        BTreeMap::from([(selected.deployment_id.clone(), selected_raw.clone())]);
    let mut reader_ids = BTreeSet::new();
    let mut reader_paths = BTreeSet::new();
    for reader in readers {
        if reader.deployment_id == selected.deployment_id
            || reader.path == selected.path
            || !reader_ids.insert(&reader.deployment_id)
            || !reader_paths.insert(&reader.path)
        {
            return Err("Copy trial reader identity is duplicated".into());
        }
        let identity = parse_deployment_id(&reader.deployment_id)
            .ok_or("Copy trial reader deployment ID is invalid")?;
        if identity.name != selected.name
            || identity.scope != scope
            || identity.project_path != selected.project_path
            || identity.destination != SkillDestination::Universal
            || identity.lexical_path != reader.path
            || !reader.path.is_absolute()
            || reader
                .path
                .components()
                .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        {
            return Err("Copy trial reader scope or identity changed".into());
        }
        if copies.get(&reader.deployment_id) != reader.registry_value.as_ref() {
            return Err("Copy trial reader ownership changed".into());
        }
        if let Some(value) = &reader.registry_value {
            let record: CopyDeploymentRecord =
                serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
            if record.deployment_id != reader.deployment_id
                || record.path != reader.path
                || record.scope != selected.scope
                || record.project_path != selected.project_path
                || record.name != selected.name
                || record.destination != SkillDestination::Universal
                || record.slot != identity.slot
            {
                return Err("Copy trial reader registry identity changed".into());
            }
            expected_copies.insert(reader.deployment_id.clone(), value.clone());
        }
    }
    Ok(CopyTrialRegistrySelection {
        selected: selected.clone(),
        readers: readers.to_vec(),
        copies: expected_copies,
        trials: BTreeMap::from([(key, raw.clone())]),
        trial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_deployment::deployment_id;
    use std::path::PathBuf;

    fn fixture(project: bool) -> (CopyDeploymentRecord, Value, DateTime<Utc>) {
        fixture_at(Path::new("/fixture"), project)
    }

    fn fixture_at(home: &Path, project: bool) -> (CopyDeploymentRecord, Value, DateTime<Utc>) {
        let now = DateTime::parse_from_rfc3339("2026-09-14T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let project_path = project.then(|| home.join("project").to_string_lossy().into_owned());
        let path = project_path
            .as_deref()
            .map(Path::new)
            .unwrap_or(home)
            .join(".agents/skills/sample");
        let scope = if project { "project" } else { "global" };
        let selected = CopyDeploymentRecord {
            deployment_id: deployment_id(
                "sample",
                scope,
                SkillDestination::Universal,
                "universal",
                project_path.as_deref(),
                &path,
            ),
            name: "sample".into(),
            path: path.clone(),
            scope: if project {
                InstallScope::Project
            } else {
                InstallScope::Global
            },
            destination: SkillDestination::Universal,
            slot: "universal".into(),
            project_path: project_path.clone(),
            content_hash: "a".repeat(64),
            disabled: false,
        };
        let trial = TrialRecord {
            deployment_id: selected.deployment_id.clone(),
            started_at: (now - chrono::Duration::hours(24)).to_rfc3339(),
            expires_at: now.to_rfc3339(),
            status: TrialStatus::Active,
            method: AddMethod::Copy,
            scope: if project {
                TrialScope::Project
            } else {
                TrialScope::Global
            },
            project_path,
            skill_dir: path,
            deployment_fingerprint: "b".repeat(64),
            claude_link: None,
            claude_link_target: None,
        };
        let mut raw = serde_json::to_value(trial).unwrap();
        raw["future_field"] = serde_json::json!({"preserve": true});
        let document = serde_json::json!({"version": 2,"future_root": [1,2,3],
            "copies": {selected.deployment_id.clone(): selected, "unrelated": {"future": 17}},
            "trials": {format!("deployment/{}", selected.deployment_id): raw, "unrelated": {"future": 29}}});
        (selected, document, now)
    }

    fn intent_fixture(project: bool) -> (CopyTrialExpiryIntent, crate::skill_service::SkillScope) {
        let (selected, document, now) = fixture(project);
        let scope = crate::skill_service::SkillScope {
            home: PathBuf::from("/fixture"),
            projects: selected.project_path.iter().map(PathBuf::from).collect(),
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        };
        let intent = CopyTrialExpiryIntent::new(
            &scope,
            "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
            select(&selected, &document, &[], now).unwrap(),
            format!("tree-v1:{}", "c".repeat(64)),
            now,
        )
        .unwrap();
        (intent, scope)
    }

    fn event_row(intent: &CopyTrialExpiryIntent) -> crate::skill_event::EventRow {
        let draft = intent.event_draft().unwrap();
        crate::skill_event::EventRow {
            id: intent.event_id.clone(),
            ts: intent.admitted_at.to_rfc3339(),
            kind: draft.kind,
            skill: draft.skill,
            harness: draft.harness,
            scope: draft.scope,
            project_path: draft.project_path,
            payload: draft.payload,
            inverse: draft.inverse,
            backup_dir: draft.backup_dir,
            status: "pending".into(),
            reverted_by: None,
            restorable: draft.restorable,
        }
    }

    #[test]
    fn expiry_event_round_trips_and_refuses_forged_metadata_or_payload() {
        for project in [false, true] {
            let (intent, scope) = intent_fixture(project);
            let row = event_row(&intent);
            assert_eq!(
                CopyTrialExpiryIntent::from_event(&row)
                    .unwrap()
                    .paths_for_scope(&scope)
                    .unwrap(),
                intent.paths_for_scope(&scope).unwrap()
            );
            for field in [
                "id", "kind", "skill", "scope", "harness", "project", "restore", "inverse",
                "backup",
            ] {
                let mut changed = row.clone();
                match field {
                    "id" => changed.id = "other".into(),
                    "kind" => changed.kind = "remove_copy_deployment".into(),
                    "skill" => changed.skill = "other".into(),
                    "scope" => changed.scope = Some("other".into()),
                    "harness" => changed.harness = Some("other".into()),
                    "project" => changed.project_path = Some("/other".into()),
                    "restore" => changed.restorable = true,
                    "inverse" => changed.inverse = Some(serde_json::json!({})),
                    "backup" => changed.backup_dir = Some("backup".into()),
                    _ => unreachable!(),
                }
                assert!(
                    CopyTrialExpiryIntent::from_event(&changed).is_err(),
                    "{field}"
                );
            }
            for (field, value) in [
                ("version", serde_json::json!(2)),
                ("expected_tree", serde_json::json!("bad")),
                ("unexpected", serde_json::json!(true)),
            ] {
                let mut changed = row.clone();
                changed.payload[field] = value;
                assert!(
                    CopyTrialExpiryIntent::from_event(&changed).is_err(),
                    "{field}"
                );
            }
            let mut changed = row;
            changed.payload["selection"]["trial"]["expires_at"] = serde_json::json!("tampered");
            assert!(CopyTrialExpiryIntent::from_event(&changed).is_err());
        }
    }

    #[test]
    fn expiry_paths_require_current_home_project_and_reader_scope() {
        let (global, global_scope) = intent_fixture(false);
        let (mut project, project_scope) = intent_fixture(true);
        assert!(project.paths_for_scope(&global_scope).is_err());
        let mut other_home = global_scope.clone();
        other_home.home = PathBuf::from("/other");
        assert!(global.paths_for_scope(&other_home).is_err());
        let paths = project.paths_for_scope(&project_scope).unwrap();
        assert!(paths
            .source_quarantine
            .starts_with("/fixture/project/.agents/.skill-studio-expiring"));
        assert!(paths
            .visible_trash
            .starts_with("/fixture/.agents/skills-trash"));
        assert_ne!(
            paths.visible_trash,
            global.paths_for_scope(&global_scope).unwrap().visible_trash
        );

        let path = PathBuf::from("/fixture/project/.claude/skills/sample");
        let reader = CopyRemovalReader {
            deployment_id: deployment_id(
                "sample",
                "project",
                SkillDestination::Universal,
                "claude-code",
                Some("/fixture/project"),
                &path,
            ),
            path,
            raw_target: PathBuf::from("../../.agents/skills/sample"),
            registry_value: None,
        };
        project.selection.readers.push(reader);
        let paths = project.paths_for_scope(&project_scope).unwrap();
        assert!(
            paths.reader_stages[0].starts_with("/fixture/project/.claude/.skill-studio-expiring")
        );
        project.selection.readers[0].path = PathBuf::from("/fixture/.claude/skills/sample");
        project.selection.readers[0].deployment_id = deployment_id(
            "sample",
            "project",
            SkillDestination::Universal,
            "claude-code",
            Some("/fixture/project"),
            &project.selection.readers[0].path,
        );
        assert!(project.paths_for_scope(&project_scope).is_err());
    }

    #[test]
    fn expiry_trash_layout_round_trips_through_existing_unmanaged_restore() {
        for project in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let (selected, document, now) = fixture_at(home.path(), project);
            let scope = crate::skill_service::SkillScope {
                home: home.path().to_path_buf(),
                projects: selected.project_path.iter().map(PathBuf::from).collect(),
                backing_roots: vec![],
                plugin_ownership_roots: vec![],
            };
            let selection = select(&selected, &document, &[], now).unwrap();
            let registry = selection
                .without_selected_records(&serde_json::to_vec(&document).unwrap())
                .unwrap();
            let intent = CopyTrialExpiryIntent::new(
                &scope,
                "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
                selection,
                format!("tree-v1:{}", "c".repeat(64)),
                now,
            )
            .unwrap();
            let paths = intent.paths_for_scope(&scope).unwrap();
            std::fs::create_dir_all(&paths.visible_trash).unwrap();
            std::fs::write(paths.visible_trash.join("SKILL.md"), "saved trial content").unwrap();
            std::fs::write(&paths.registry, &registry).unwrap();
            let receipt = crate::skill_trial_restore::restore_trial_backup(
                home.path(),
                paths.visible_trash.to_str().unwrap(),
                crate::skill_backup_copy::BackupCopyLimits {
                    max_bytes: 1024 * 1024,
                    max_entries: 100,
                    max_depth: 10,
                },
            )
            .unwrap();
            assert_eq!(receipt.name, "sample");
            assert_eq!(
                std::fs::read_to_string(home.path().join(".agents/skills/sample/SKILL.md"))
                    .unwrap(),
                "saved trial content"
            );
            assert_eq!(std::fs::read(&paths.registry).unwrap(), registry);
            assert!(paths.visible_trash.join("SKILL.md").exists());
            if project {
                assert!(!paths.source.exists());
            }
            assert!(!intent.event_draft().unwrap().restorable);
        }
    }

    fn select(
        selected: &CopyDeploymentRecord,
        document: &Value,
        readers: &[CopyRemovalReader],
        now: DateTime<Utc>,
    ) -> Result<CopyTrialRegistrySelection, String> {
        select_due_copy_trial(
            &serde_json::to_vec(document).unwrap(),
            selected,
            readers,
            &"b".repeat(64),
            now,
        )
    }

    #[test]
    fn exact_due_global_and_project_trials_preserve_other_scope_and_unknown_json() {
        for project in [false, true] {
            let (selected, mut document, now) = fixture(project);
            let (other, other_document, _) = fixture(!project);
            document["copies"][&other.deployment_id] =
                other_document["copies"][&other.deployment_id].clone();
            let other_key = format!("deployment/{}", other.deployment_id);
            document["trials"][&other_key] = other_document["trials"][&other_key].clone();
            let admission = select(&selected, &document, &[], now).unwrap();
            assert_eq!(admission.trial().deployment_id, selected.deployment_id);
            let before = serde_json::to_vec(&document).unwrap();
            let removed = admission.without_selected_records(&before).unwrap();
            let actual: Value = serde_json::from_slice(&removed).unwrap();
            document["copies"]
                .as_object_mut()
                .unwrap()
                .remove(&selected.deployment_id);
            document["trials"]
                .as_object_mut()
                .unwrap()
                .remove(&format!("deployment/{}", selected.deployment_id));
            assert_eq!(actual, document);
            assert_eq!(
                admission.without_selected_records(&removed).unwrap(),
                removed
            );
        }
    }

    #[test]
    fn refuses_changed_trial_identity_deadline_content_and_owner() {
        let (selected, document, now) = fixture(false);
        let key = format!("deployment/{}", selected.deployment_id);
        for (field, value) in [
            ("method", serde_json::json!("skills-sh")),
            ("scope", serde_json::json!("project")),
            ("project_path", serde_json::json!("/other")),
            ("skill_dir", serde_json::json!("/other/skill")),
            ("deployment_id", serde_json::json!("wrong")),
            ("deployment_fingerprint", serde_json::json!("c".repeat(64))),
            (
                "expires_at",
                serde_json::json!((now + chrono::Duration::seconds(1)).to_rfc3339()),
            ),
            ("expires_at", serde_json::json!("invalid")),
            ("status", serde_json::json!("recovery-required")),
        ] {
            let mut changed = document.clone();
            changed["trials"][&key][field] = value;
            assert!(select(&selected, &changed, &[], now).is_err(), "{field}");
        }
        let mut changed = document.clone();
        changed["copies"][&selected.deployment_id]["content_hash"] =
            serde_json::json!("c".repeat(64));
        assert!(select(&selected, &changed, &[], now).is_err());
        changed = document.clone();
        changed["trials"][&key]["status"] = serde_json::json!("expiring");
        changed["trials"][&key]["expires_at"] = serde_json::json!("invalid");
        assert!(select(&selected, &changed, &[], now).is_ok());
        changed = document;
        let trial = changed["trials"]
            .as_object_mut()
            .unwrap()
            .remove(&key)
            .unwrap();
        changed["trials"]["legacy-alias"] = trial;
        assert!(select(&selected, &changed, &[], now).is_err());
    }

    #[test]
    fn refuses_duplicate_trial_aliases_and_late_selected_raw_edits() {
        let (selected, document, now) = fixture(false);
        let selection = select(&selected, &document, &[], now).unwrap();
        let key = format!("deployment/{}", selected.deployment_id);
        for alias in [
            serde_json::json!({"deployment_id":selected.deployment_id}),
            serde_json::json!({"skill_dir":selected.path}),
        ] {
            let mut changed = document.clone();
            changed["trials"]["duplicate"] = alias;
            assert!(select(&selected, &changed, &[], now).is_err());
            assert!(selection
                .without_selected_records(&serde_json::to_vec(&changed).unwrap())
                .is_err());
        }
        for bucket in ["copies", "trials"] {
            let mut changed = document.clone();
            let selected_key = if bucket == "copies" {
                &selected.deployment_id
            } else {
                &key
            };
            changed[bucket][selected_key]["future_field"] = serde_json::json!("changed");
            assert!(selection
                .without_selected_records(&serde_json::to_vec(&changed).unwrap())
                .is_err());
        }
        let mut unrelated = document;
        unrelated["future_root"] = serde_json::json!("changed concurrently");
        let removed: Value = serde_json::from_slice(
            &selection
                .without_selected_records(&serde_json::to_vec(&unrelated).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(removed["future_root"], unrelated["future_root"]);
    }

    #[test]
    fn reader_binding_refuses_duplicates_wrong_scope_and_retargeting() {
        let (selected, mut document, now) = fixture(false);
        let path = PathBuf::from("/fixture/.claude/skills/sample");
        let reader = CopyRemovalReader {
            deployment_id: deployment_id(
                "sample",
                "global",
                SkillDestination::Universal,
                "claude-code",
                None,
                &path,
            ),
            path,
            raw_target: PathBuf::from("../../.agents/skills/sample"),
            registry_value: None,
        };
        let key = format!("deployment/{}", selected.deployment_id);
        document["trials"][&key]["claude_link"] = serde_json::json!(reader.path);
        document["trials"][&key]["claude_link_target"] = serde_json::json!(reader.raw_target);
        assert!(select(&selected, &document, std::slice::from_ref(&reader), now).is_ok());
        assert!(select(&selected, &document, &[reader.clone(), reader.clone()], now).is_err());
        let mut changed = reader.clone();
        changed.raw_target = PathBuf::from("elsewhere");
        assert!(select(&selected, &document, &[changed], now).is_err());
        let mut changed = reader.clone();
        changed.deployment_id = deployment_id(
            "sample",
            "project",
            SkillDestination::Universal,
            "claude-code",
            Some("/project"),
            &changed.path,
        );
        assert!(select(&selected, &document, &[changed], now).is_err());
        document["trials"]["alias"] = serde_json::json!({"claude_link": reader.path});
        assert!(select(&selected, &document, &[reader], now).is_err());
    }
}
