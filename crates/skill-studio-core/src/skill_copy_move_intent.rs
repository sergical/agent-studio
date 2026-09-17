//! Durable Copy move data. A validated intent is not filesystem write authority.
use crate::{
    skill_copy_move::CopyMoveTransition,
    skill_deployment::InstallScope,
    skill_event::{EventDraft, EventRow},
};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

const EVENT_KIND: &str = "move_copy_deployment";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyMoveIntent {
    version: u32,
    transition: CopyMoveTransition,
    expected_tree: String,
    registry_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<CopyMoveSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CopyMoveSource {
    pub event_id: String,
    fingerprint: String,
}

fn source_fingerprint(row: &EventRow) -> Result<String, String> {
    let mut row = row.clone();
    row.reverted_by = None;
    Ok(crate::skill_frontmatter_repair::content_fingerprint(
        &serde_json::to_vec(&row).map_err(|e| e.to_string())?,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyMoveObservedState {
    BeforeMove,
    TreeMoved,
    RegistryPublished,
}

impl CopyMoveIntent {
    pub fn new(
        transition: CopyMoveTransition,
        expected_tree: String,
        registry_path: PathBuf,
    ) -> Result<Self, String> {
        let intent = Self {
            version: 1,
            transition,
            expected_tree,
            registry_path,
            source: None,
        };
        intent.validate()?;
        Ok(intent)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.transition.validate()?;
        if let Some(source) = &self.source {
            if !crate::skill_backup_reservation::valid_id(&source.event_id)
                || source
                    .fingerprint
                    .strip_prefix("sha256:")
                    .is_none_or(|hash| {
                        hash.len() != 64
                            || !hash
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    })
            {
                return Err("Invalid Copy move history reference".into());
            }
        }
        let valid_tree = self
            .expected_tree
            .strip_prefix("tree-v1:")
            .is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        if self.version != 1
            || !valid_tree
            || !self.registry_path.is_absolute()
            || self
                .registry_path
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
            || self
                .registry_path
                .file_name()
                .and_then(|name| name.to_str())
                != Some("skill-studio.json")
            || self
                .registry_path
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                != Some(".agents")
        {
            return Err("Invalid or unsupported Copy move intent".into());
        }
        Ok(())
    }

    pub fn transition(&self) -> &CopyMoveTransition {
        &self.transition
    }
    pub fn expected_tree(&self) -> &str {
        &self.expected_tree
    }
    pub fn registry_path(&self) -> &Path {
        &self.registry_path
    }

    pub(crate) fn source(&self) -> Option<&CopyMoveSource> {
        self.source.as_ref()
    }

    pub(crate) fn reversing(mut self, source: &EventRow) -> Result<Self, String> {
        if source.status != "done" || source.reverted_by.is_some() {
            return Err("Copy visibility event is no longer available to reverse".into());
        }
        self.validate_inverse_of(&Self::from_event(source)?)?;
        self.source = Some(CopyMoveSource {
            event_id: source.id.clone(),
            fingerprint: source_fingerprint(source)?,
        });
        self.validate()?;
        Ok(self)
    }

    fn validate_inverse_of(&self, source: &Self) -> Result<(), String> {
        if self.registry_path != source.registry_path
            || self.expected_tree != source.expected_tree
            || self.transition.before() != source.transition.after()
            || self.transition.after() != source.transition.before()
        {
            return Err("Copy visibility reversal does not invert its source".into());
        }
        Ok(())
    }

    pub(crate) fn validate_source_claim(
        &self,
        source: &EventRow,
        event_id: &str,
    ) -> Result<(), String> {
        let reference = self
            .source
            .as_ref()
            .ok_or("Copy move has no source reference")?;
        if source.id != reference.event_id
            || source.status != "done"
            || source.reverted_by.as_deref() != Some(event_id)
            || source_fingerprint(source)? != reference.fingerprint
        {
            return Err("Copy visibility history claim changed".into());
        }
        self.validate_inverse_of(&Self::from_event(source)?)
    }

    fn scope(&self) -> &'static str {
        match self.transition.before().scope {
            InstallScope::Global => "global",
            InstallScope::Project => "project",
        }
    }

    /// The caller must provide complete scoped observations: `None` means
    /// verified absence, never an unreadable path or a failed scan.
    pub fn observe(
        &self,
        source_tree: Option<&str>,
        destination_tree: Option<&str>,
        registry: &[u8],
    ) -> Result<CopyMoveObservedState, String> {
        self.validate()?;
        let registry_published = self.transition.apply_document(registry)? == registry;
        match (source_tree, destination_tree, registry_published) {
            (Some(source), None, false) if source == self.expected_tree => {
                Ok(CopyMoveObservedState::BeforeMove)
            }
            (None, Some(destination), false) if destination == self.expected_tree => {
                Ok(CopyMoveObservedState::TreeMoved)
            }
            (None, Some(destination), true) if destination == self.expected_tree => {
                Ok(CopyMoveObservedState::RegistryPublished)
            }
            _ => Err("Copy move effects are missing, changed or out of order".into()),
        }
    }

    pub fn event_draft(&self) -> Result<EventDraft, String> {
        self.validate()?;
        let before = self.transition.before();
        Ok(EventDraft {
            kind: EVENT_KIND.into(),
            skill: before.name.clone(),
            harness: Some(before.slot.clone()),
            scope: Some(self.scope().into()),
            project_path: before.project_path.clone(),
            payload: serde_json::to_value(self).map_err(|error| error.to_string())?,
            inverse: None,
            backup_dir: None,
            restorable: false,
        })
    }

    pub fn from_event(event: &EventRow) -> Result<Self, String> {
        if event.kind != EVENT_KIND
            || event.restorable
            || event.inverse.is_some()
            || event.backup_dir.is_some()
        {
            return Err("Not a Copy move event".into());
        }
        let intent: Self =
            serde_json::from_value(event.payload.clone()).map_err(|error| error.to_string())?;
        intent.validate()?;
        if intent
            .source
            .as_ref()
            .is_some_and(|source| source.event_id == event.id)
        {
            return Err("Copy move cannot reverse itself".into());
        }
        let before = intent.transition.before();
        if event.skill != before.name
            || event.harness.as_deref() != Some(before.slot.as_str())
            || event.scope.as_deref() != Some(intent.scope())
            || event.project_path != before.project_path
        {
            return Err("Copy move event metadata does not match its intent".into());
        }
        Ok(intent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect},
        skill_deployment::{deployment_id, SkillDestination},
        skill_event_operations::GuardedEventStore,
        skill_event_store::EventStore,
        skill_fork_registry::CopyDeploymentRecord,
        skill_scope::SkillReadScope,
    };
    use std::{fs, time::Duration};

    fn intent(home: &Path, project: bool) -> CopyMoveIntent {
        let project_path = project.then(|| home.join("project").to_string_lossy().into_owned());
        let root = project_path.as_deref().map(Path::new).unwrap_or(home);
        let path = root.join(".cursor/skills/sample");
        let record = CopyDeploymentRecord {
            deployment_id: deployment_id(
                "sample",
                if project { "project" } else { "global" },
                SkillDestination::PerHarness,
                "cursor",
                project_path.as_deref(),
                &path,
            ),
            name: "sample".into(),
            path,
            scope: if project {
                InstallScope::Project
            } else {
                InstallScope::Global
            },
            destination: SkillDestination::PerHarness,
            slot: "cursor".into(),
            project_path,
            content_hash: "a".repeat(64),
            disabled: false,
        };
        CopyMoveIntent::new(
            CopyMoveTransition::new(record, false).unwrap(),
            format!("tree-v1:{}", "b".repeat(64)),
            home.join(".agents/skill-studio.json"),
        )
        .unwrap()
    }

    #[test]
    fn guarded_pending_intent_survives_store_reopen_without_moving_files() {
        for project in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let home = fs::canonicalize(temp.path()).unwrap();
            let intent = intent(&home, project);
            let path = &intent.transition.before().path;
            fs::create_dir_all(path).unwrap();
            fs::write(path.join("SKILL.md"), "original").unwrap();
            let state = home.join("state");
            {
                let store = EventStore::open(&state).unwrap();
                let scope = SkillReadScope::bind(std::slice::from_ref(&home)).unwrap();
                let lease = CoordinationPlan::new(
                    vec![DirectoryEffect::tree(&state, CoordinationMode::Exclusive)],
                    Some(Duration::from_secs(2)),
                )
                .unwrap()
                .acquire()
                .unwrap()
                .finalize_write(&scope, &[])
                .unwrap();
                let guarded = GuardedEventStore::bind(&store, &lease).unwrap();
                guarded
                    .record_pending(&lease, "copy-move-fixture", intent.event_draft().unwrap())
                    .unwrap();
                assert!(guarded
                    .record_pending(&lease, "second-copy-move", intent.event_draft().unwrap())
                    .is_err());
            }
            let store = EventStore::open(&state).unwrap();
            let event = store.get("copy-move-fixture").unwrap().unwrap();
            assert_eq!(event.status, "pending");
            assert!(!event.restorable);
            assert!(event.inverse.is_none());
            let decoded = CopyMoveIntent::from_event(&event).unwrap();
            assert_eq!(
                serde_json::to_value(decoded).unwrap(),
                serde_json::to_value(intent.clone()).unwrap()
            );
            assert_eq!(fs::read(path.join("SKILL.md")).unwrap(), b"original");
            assert!(!intent.transition.after().path.exists());
            assert_eq!(store.list(10, None).unwrap().len(), 1);
        }
    }

    #[test]
    fn unknown_versions_digests_and_registry_paths_are_refused() {
        let intent = intent(Path::new("/fixture"), false);
        for (field, value) in [
            ("version", serde_json::json!(2)),
            ("expected_tree", serde_json::json!("tree-v1:bad")),
            (
                "registry_path",
                serde_json::json!("/fixture/.agents/other.json"),
            ),
            (
                "registry_path",
                serde_json::json!("/fixture/../.agents/skill-studio.json"),
            ),
        ] {
            let mut encoded = serde_json::to_value(&intent).unwrap();
            encoded[field] = value;
            let decoded: CopyMoveIntent = serde_json::from_value(encoded).unwrap();
            assert!(decoded.validate().is_err());
            assert!(decoded.event_draft().is_err());
        }
    }

    #[test]
    fn observed_states_accept_only_the_expected_publication_order() {
        let intent = intent(Path::new("/fixture"), false);
        let before = intent.transition.before();
        let registry = serde_json::to_vec(
            &serde_json::json!({"copies": {before.deployment_id.clone(): before}}),
        )
        .unwrap();
        let published = intent.transition.apply_document(&registry).unwrap();
        let expected = intent.expected_tree.as_str();
        assert_eq!(
            intent.observe(Some(expected), None, &registry).unwrap(),
            CopyMoveObservedState::BeforeMove
        );
        assert_eq!(
            intent.observe(None, Some(expected), &registry).unwrap(),
            CopyMoveObservedState::TreeMoved
        );
        assert_eq!(
            intent.observe(None, Some(expected), &published).unwrap(),
            CopyMoveObservedState::RegistryPublished
        );
        for (source, destination, document) in [
            (Some(expected), None, &published),
            (Some(expected), Some(expected), &registry),
            (None, None, &registry),
            (Some("different"), None, &registry),
            (None, Some("different"), &published),
        ] {
            assert!(intent.observe(source, destination, document).is_err());
        }
        assert!(intent
            .observe(None, Some(expected), b"{\"copies\":{}}")
            .is_err());
    }

    #[test]
    fn mismatched_event_scope_is_refused() {
        let intent = intent(Path::new("/fixture"), true);
        let row = EventRow {
            id: "fixture".into(),
            ts: String::new(),
            kind: EVENT_KIND.into(),
            skill: "sample".into(),
            harness: Some("cursor".into()),
            scope: Some("global".into()),
            project_path: None,
            payload: serde_json::to_value(intent).unwrap(),
            inverse: None,
            backup_dir: None,
            status: "pending".into(),
            reverted_by: None,
            restorable: false,
        };
        assert!(CopyMoveIntent::from_event(&row).is_err());
    }
}
