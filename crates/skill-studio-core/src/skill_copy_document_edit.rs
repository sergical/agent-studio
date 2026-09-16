//! Persisted Copy editor changes. Consistency validation grants no path authority.
use crate::{
    skill_coordination::{FinalizedWriteLease, PreparedContentError},
    skill_discovery::PreparedCopyRepairContent,
    skill_scope::SkillReadScope,
    skill_service::{CancellationToken, ScopedSkillService, WritePreparationError},
};
use crate::{
    skill_copy_repair::CopyRepairTransition,
    skill_fork_registry::{CopyDeploymentRecord, RegistryOwnerRecord},
    skill_frontmatter_repair::content_fingerprint,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Component, PathBuf},
    time::Duration,
};

pub const MAX_COPY_DOCUMENT_EDIT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyDocumentEditRequest {
    pub deployment_id: String,
    pub expected_owner_revision: String,
    pub expected_content_fingerprint: String,
    pub proposed_content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyDocumentEditIntent {
    request: CopyDocumentEditRequest,
    proposed_content_fingerprint: String,
    transition: CopyRepairTransition,
    registry_path: PathBuf,
    registry_before_fingerprint: String,
    registry_after_fingerprint: String,
}

impl CopyDocumentEditIntent {
    /// Folder hashes and document bytes must come from the same complete scoped read.
    pub fn from_documents(
        request: CopyDocumentEditRequest,
        record: CopyDeploymentRecord,
        proposed_folder_hash: String,
        registry_path: PathBuf,
        registry_original: &[u8],
        original_content: &[u8],
    ) -> Result<Self, String> {
        let transition = CopyRepairTransition::new(record, proposed_folder_hash)?;
        let registry_after = transition.apply_document(registry_original)?;
        let intent = Self {
            proposed_content_fingerprint: content_fingerprint(request.proposed_content.as_bytes()),
            request,
            transition,
            registry_path,
            registry_before_fingerprint: content_fingerprint(registry_original),
            registry_after_fingerprint: content_fingerprint(&registry_after),
        };
        intent.validate_record()?;
        intent.validate_originals(original_content, registry_original)?;
        Ok(intent)
    }

    pub fn request(&self) -> &CopyDocumentEditRequest {
        &self.request
    }

    pub fn transition(&self) -> &CopyRepairTransition {
        &self.transition
    }

    pub fn registry_path(&self) -> &std::path::Path {
        &self.registry_path
    }

    pub fn validate_record(&self) -> Result<(), String> {
        self.transition.validate()?;
        let before = self.transition.before();
        let valid_fingerprint = |value: &str| {
            value.strip_prefix("sha256:").is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        };
        if self.request.deployment_id != before.deployment_id
            || RegistryOwnerRecord::Copy(before).revision().as_deref()
                != Some(self.request.expected_owner_revision.as_str())
            || !valid_fingerprint(&self.request.expected_content_fingerprint)
            || content_fingerprint(self.request.proposed_content.as_bytes())
                != self.proposed_content_fingerprint
            || self.proposed_content_fingerprint == self.request.expected_content_fingerprint
            || self.request.proposed_content.len() > MAX_COPY_DOCUMENT_EDIT_BYTES
            || !self.registry_path.is_absolute()
            || self.registry_path.file_name() != Some(std::ffi::OsStr::new("skill-studio.json"))
            || self
                .registry_path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
            || !valid_fingerprint(&self.registry_before_fingerprint)
            || !valid_fingerprint(&self.registry_after_fingerprint)
            || self.registry_before_fingerprint == self.registry_after_fingerprint
        {
            return Err("Copy document edit does not match its identity or fingerprints".into());
        }
        Ok(())
    }

    pub fn validate_originals(&self, document: &[u8], registry: &[u8]) -> Result<(), String> {
        self.validate_record()?;
        if document.len() > MAX_COPY_DOCUMENT_EDIT_BYTES
            || registry.len() > 8 * 1024 * 1024
            || content_fingerprint(document) != self.request.expected_content_fingerprint
            || content_fingerprint(registry) != self.registry_before_fingerprint
        {
            return Err("Copy document edit originals changed".into());
        }
        std::str::from_utf8(document).map_err(|_| "Copy edit original is not UTF-8")?;
        let proposed = self.transition.apply_document(registry)?;
        if content_fingerprint(&proposed) != self.registry_after_fingerprint {
            return Err("Copy document edit registry transition changed".into());
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum DocumentEditPreparationError {
    Inventory(WritePreparationError),
    Content(PreparedContentError),
    InvalidEdit(String),
}

impl std::fmt::Display for DocumentEditPreparationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inventory(error) => error.fmt(formatter),
            Self::Content(error) => error.fmt(formatter),
            Self::InvalidEdit(message) => formatter.write_str(message),
        }
    }
}
impl std::error::Error for DocumentEditPreparationError {}

pub enum CopyDocumentEditPreparation<'scope> {
    Unchanged { deployment_id: String },
    Ready(Box<PreparedCopyDocumentEdit<'scope>>),
}

pub struct PreparedCopyDocumentEdit<'scope> {
    intent: CopyDocumentEditIntent,
    original: Vec<u8>,
    registry_original: Vec<u8>,
    content: PreparedCopyRepairContent,
    content_scope: SkillReadScope,
    folder_hashes: (String, String),
    lease: FinalizedWriteLease<'scope>,
}

impl PreparedCopyDocumentEdit<'_> {
    pub fn intent(&self) -> &CopyDocumentEditIntent {
        &self.intent
    }

    pub fn revalidate(&self) -> Result<(), String> {
        self.revalidate_content().map_err(|error| error.to_string())
    }

    pub(crate) fn revalidate_content(&self) -> Result<(), PreparedContentError> {
        let hashes = self.content.hashes_with_document_limit(
            &self.content_scope,
            &self.lease,
            &self.original,
            self.intent.request().proposed_content.as_bytes(),
            MAX_COPY_DOCUMENT_EDIT_BYTES,
        )?;
        if hashes != self.folder_hashes {
            return Err("Prepared Copy edit folder changed".into());
        }
        let registry = self
            .lease
            .read(self.intent.registry_path(), 8 * 1024 * 1024)
            .map_err(PreparedContentError::from)?;
        if registry != self.registry_original {
            return Err("Prepared Copy edit registry changed".into());
        }
        self.intent.validate_originals(&self.original, &registry)?;
        self.lease.revalidate().map_err(PreparedContentError::from)
    }
}

impl ScopedSkillService {
    pub fn prepare_copy_document_edit(
        &mut self,
        request: &CopyDocumentEditRequest,
        additional_trees: &[PathBuf],
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<CopyDocumentEditPreparation<'_>, DocumentEditPreparationError> {
        let invalid = DocumentEditPreparationError::InvalidEdit;
        if request.proposed_content.len() > MAX_COPY_DOCUMENT_EDIT_BYTES {
            return Err(invalid("Copy edit document exceeds its limit".into()));
        }
        let selected = crate::skill_deployment::parse_deployment_id(&request.deployment_id)
            .ok_or_else(|| invalid("Invalid deployment ID".into()))?;
        let names = BTreeSet::from([selected.name]);
        let (inventory, lease) = self
            .prepare_write_inventory(
                Some(&names),
                additional_trees,
                timeout,
                cancellation.clone(),
            )
            .map_err(DocumentEditPreparationError::Inventory)?;
        prepare_copy_edit_with_inventory(request, inventory, lease, &cancellation)
    }
}

fn prepare_copy_edit_with_inventory<'scope>(
    request: &CopyDocumentEditRequest,
    inventory: crate::skill_service::InventoryRead,
    lease: FinalizedWriteLease<'scope>,
    cancellation: &CancellationToken,
) -> Result<CopyDocumentEditPreparation<'scope>, DocumentEditPreparationError> {
    let content_error = DocumentEditPreparationError::Content;
    let invalid = DocumentEditPreparationError::InvalidEdit;
    let mut matches = inventory
        .skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .filter(|deployment| deployment.id == request.deployment_id);
    let deployment = matches
        .next()
        .ok_or_else(|| invalid("Selected deployment is absent".into()))?;
    if matches.next().is_some()
        || deployment.owner_kind != crate::skill_ownership::LifecycleOwnerKind::Copy
        || deployment.owner_revision.as_deref() != Some(request.expected_owner_revision.as_str())
    {
        return Err(invalid(
            "Copy edit ownership is ambiguous, unsupported or changed".into(),
        ));
    }
    let skill_dir = PathBuf::from(&deployment.path);
    let original = lease
        .read(&skill_dir.join("SKILL.md"), MAX_COPY_DOCUMENT_EDIT_BYTES)
        .map_err(|error| content_error(error.into()))?;
    if content_fingerprint(&original) != request.expected_content_fingerprint {
        return Err(invalid("Copy edit document changed".into()));
    }
    let content_scope = SkillReadScope::bind(std::slice::from_ref(&skill_dir))
        .map_err(|error| invalid(error.to_string()))?;
    let content =
        PreparedCopyRepairContent::enumerate_controlled(&content_scope, &skill_dir, cancellation)
            .map_err(content_error)?;
    let folder_hashes = content
        .hashes_with_document_limit(
            &content_scope,
            &lease,
            &original,
            request.proposed_content.as_bytes(),
            MAX_COPY_DOCUMENT_EDIT_BYTES,
        )
        .map_err(content_error)?;
    let registry_path = inventory.scope.home.join(".agents/skill-studio.json");
    let registry_original = lease
        .read(&registry_path, 8 * 1024 * 1024)
        .map_err(|error| content_error(error.into()))?;
    let registry: crate::skill_fork_registry::ForkRegistry =
        serde_json::from_slice(&registry_original).map_err(|error| invalid(error.to_string()))?;
    let record = registry
        .copies
        .get(&request.deployment_id)
        .ok_or_else(|| invalid("Copy registry record is missing".into()))?;
    if record.content_hash != folder_hashes.0
        || RegistryOwnerRecord::Copy(record).revision().as_deref()
            != Some(request.expected_owner_revision.as_str())
    {
        return Err(invalid("Copy folder or ownership record changed".into()));
    }
    lease
        .revalidate()
        .map_err(|error| content_error(error.into()))?;
    if original == request.proposed_content.as_bytes() {
        return Ok(CopyDocumentEditPreparation::Unchanged {
            deployment_id: request.deployment_id.clone(),
        });
    }
    let intent = CopyDocumentEditIntent::from_documents(
        request.clone(),
        record.clone(),
        folder_hashes.1.clone(),
        registry_path,
        &registry_original,
        &original,
    )
    .map_err(invalid)?;
    let prepared = PreparedCopyDocumentEdit {
        intent,
        original,
        registry_original,
        content,
        content_scope,
        folder_hashes,
        lease,
    };
    prepared.revalidate_content().map_err(content_error)?;
    Ok(CopyDocumentEditPreparation::Ready(Box::new(prepared)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_deployment::{deployment_id, InstallScope, SkillDestination};

    fn fixture(proposed: &str) -> (CopyDocumentEditIntent, Vec<u8>, Vec<u8>) {
        let path = PathBuf::from("/fixture/.agents/skills/sample");
        let record = CopyDeploymentRecord {
            deployment_id: deployment_id(
                "sample",
                "global",
                SkillDestination::Universal,
                "universal",
                None,
                &path,
            ),
            name: "sample".into(),
            path,
            scope: InstallScope::Global,
            destination: SkillDestination::Universal,
            slot: "universal".into(),
            project_path: None,
            content_hash: "a".repeat(64),
            disabled: false,
        };
        let original = b"---\nname: sample\ndescription: Original\n---\nBody\n".to_vec();
        let request = CopyDocumentEditRequest {
            deployment_id: record.deployment_id.clone(),
            expected_owner_revision: RegistryOwnerRecord::Copy(&record).revision().unwrap(),
            expected_content_fingerprint: content_fingerprint(&original),
            proposed_content: proposed.into(),
        };
        let mut value = serde_json::to_value(&record).unwrap();
        value["future_field"] = serde_json::json!({"preserve": true});
        let registry = serde_json::to_vec(&serde_json::json!({
            "version": 4, "copies": {record.deployment_id.clone(): value},
            "other_state": {"unrelated": [1,2,3]}
        }))
        .unwrap();
        let intent = CopyDocumentEditIntent::from_documents(
            request,
            record,
            "b".repeat(64),
            PathBuf::from("/fixture/.agents/skill-studio.json"),
            &registry,
            &original,
        )
        .unwrap();
        (intent, original, registry)
    }

    #[test]
    fn editor_intent_refuses_an_original_that_text_undo_cannot_restore() {
        let (intent, _, registry) = fixture("Edited text");
        let original = b"invalid text: \xff";
        let mut request = intent.request().clone();
        request.expected_content_fingerprint = content_fingerprint(original);
        let result = CopyDocumentEditIntent::from_documents(
            request,
            intent.transition().before().clone(),
            "b".repeat(64),
            intent.registry_path().to_path_buf(),
            &registry,
            original,
        );
        assert!(result.unwrap_err().contains("not UTF-8"));
    }

    #[test]
    fn editor_intent_preserves_entry_identity_and_unrelated_registry_data() {
        for proposed in ["Edited body", "---\nname: different\n---\nEdited body"] {
            let (intent, original, registry) = fixture(proposed);
            let decoded: CopyDocumentEditIntent =
                serde_json::from_slice(&serde_json::to_vec(&intent).unwrap()).unwrap();
            decoded.validate_originals(&original, &registry).unwrap();
            assert_eq!(decoded.request().proposed_content, proposed);
            assert_eq!(decoded.transition.before().name, "sample");
            let applied = decoded.transition.apply_document(&registry).unwrap();
            assert_eq!(
                decoded.transition.apply_document(&applied).unwrap(),
                applied
            );
            let undone = decoded.transition.roll_back_document(&applied).unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&undone).unwrap(),
                serde_json::from_slice::<serde_json::Value>(&registry).unwrap()
            );
        }
    }

    #[test]
    fn editor_intent_rejects_stale_originals_and_owner_revision() {
        let (mut intent, original, registry) = fixture("Edited body");
        assert!(intent
            .validate_originals(b"external edit", &registry)
            .is_err());
        assert!(intent.validate_originals(&original, b"{}").is_err());
        intent.request.expected_owner_revision = "stale".into();
        assert!(intent.validate_record().is_err());
    }

    #[test]
    fn persisted_editor_identity_and_transition_cannot_diverge() {
        let (intent, original, registry) = fixture("Edited body");
        for pointer in [
            "/request/deployment_id",
            "/request/expected_content_fingerprint",
            "/request/proposed_content",
            "/registry_path",
            "/registry_after_fingerprint",
            "/transition/after/path",
        ] {
            let mut value = serde_json::to_value(&intent).unwrap();
            *value.pointer_mut(pointer).unwrap() = serde_json::json!("changed");
            let decoded: CopyDocumentEditIntent = serde_json::from_value(value).unwrap();
            assert!(
                decoded.validate_originals(&original, &registry).is_err(),
                "{pointer}"
            );
        }
    }
}

#[cfg(all(test, unix))]
mod preparation_tests {
    use super::*;
    use crate::{
        skill_deployment::{InstallScope, SkillDestination},
        skill_fork_registry::ForkRegistry,
        skill_service::SkillScope,
    };
    use std::fs;

    pub(super) struct Fixture {
        _temp: tempfile::TempDir,
        pub(super) scope: SkillScope,
        pub(super) skill: PathBuf,
        pub(super) registry: PathBuf,
        pub(super) sibling: PathBuf,
        pub(super) request: CopyDocumentEditRequest,
    }

    impl Fixture {
        pub(super) fn new(project: bool, per_harness: bool, large: bool) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().canonicalize().unwrap().join("home");
            let project_path = home.join("projects/project");
            let relative = if per_harness {
                ".codex/skills/sample"
            } else {
                ".agents/skills/sample"
            };
            let skill = if project { &project_path } else { &home }.join(relative);
            let sibling = if project { &home } else { &project_path }
                .join(relative)
                .join("SKILL.md");
            fs::create_dir_all(&skill).unwrap();
            fs::create_dir_all(sibling.parent().unwrap()).unwrap();
            fs::create_dir(home.join(".git")).unwrap();
            fs::create_dir(project_path.join(".git")).unwrap();
            fs::create_dir_all(home.join(".agents")).unwrap();
            let mut original = "---\nname: sample\ndescription: Fixture\n---\nBody\n".to_string();
            if large {
                original.push_str(&"a".repeat(1024 * 1024));
            }
            fs::write(skill.join("SKILL.md"), &original).unwrap();
            fs::write(skill.join("resource.txt"), "resource").unwrap();
            fs::write(
                &sibling,
                "---\nname: sample\ndescription: Sibling\n---\nUntouched\n",
            )
            .unwrap();
            let scope = SkillScope {
                home: home.clone(),
                projects: vec![project_path.clone()],
                backing_roots: vec![],
                plugin_ownership_roots: vec![],
            };
            let mut service = ScopedSkillService::bind(scope.clone()).unwrap();
            let inventory = service.scan(None, Some(Duration::from_secs(10))).unwrap();
            let deployment = inventory
                .skills
                .iter()
                .flat_map(|s| &s.deployments)
                .find(|d| std::path::Path::new(&d.path) == skill)
                .unwrap();
            let record = CopyDeploymentRecord {
                deployment_id: deployment.id.clone(),
                name: "sample".into(),
                path: skill.clone(),
                scope: if project {
                    InstallScope::Project
                } else {
                    InstallScope::Global
                },
                destination: if per_harness {
                    SkillDestination::PerHarness
                } else {
                    SkillDestination::Universal
                },
                slot: if per_harness { "codex" } else { "universal" }.into(),
                project_path: project.then(|| project_path.to_string_lossy().into_owned()),
                content_hash: deployment.content_hash.clone(),
                disabled: false,
            };
            let request = CopyDocumentEditRequest {
                deployment_id: record.deployment_id.clone(),
                expected_owner_revision: RegistryOwnerRecord::Copy(&record).revision().unwrap(),
                expected_content_fingerprint: content_fingerprint(original.as_bytes()),
                proposed_content: original + "Edited\n",
            };
            let mut registry = ForkRegistry::default();
            registry.copies.insert(record.deployment_id.clone(), record);
            fs::write(
                home.join(".agents/skill-studio.json"),
                serde_json::to_vec_pretty(&registry).unwrap(),
            )
            .unwrap();
            Self {
                _temp: temp,
                scope,
                skill,
                registry: home.join(".agents/skill-studio.json"),
                sibling,
                request,
            }
        }

        pub(super) fn bytes(&self) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
            (
                fs::read(self.skill.join("SKILL.md")).unwrap(),
                fs::read(&self.registry).unwrap(),
                fs::read(&self.sibling).unwrap(),
            )
        }
    }

    #[test]
    fn copy_edit_preparation_preserves_cancellation_after_inventory() {
        let fixture = Fixture::new(false, false, false);
        let before = fixture.bytes();
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        let token = CancellationToken::default();
        let (inventory, lease) = service
            .prepare_write_inventory(None, &[], Some(Duration::from_secs(10)), token.clone())
            .unwrap();
        token.cancel();
        let result = prepare_copy_edit_with_inventory(&fixture.request, inventory, lease, &token);
        assert!(
            matches!(result, Err(DocumentEditPreparationError::Content(error)) if error.is_cancelled())
        );
        assert_eq!(fixture.bytes(), before);
    }

    #[test]
    fn copy_edit_revalidation_distinguishes_cancellation_from_changed_content() {
        for cancel_before in [true, false] {
            let fixture = Fixture::new(false, false, false);
            let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
            let token = CancellationToken::default();
            let CopyDocumentEditPreparation::Ready(prepared) = service
                .prepare_copy_document_edit(
                    &fixture.request,
                    &[],
                    Some(Duration::from_secs(10)),
                    token.clone(),
                )
                .unwrap()
            else {
                panic!("expected prepared edit")
            };
            if cancel_before {
                token.cancel();
            } else {
                fs::write(fixture.skill.join("SKILL.md"), "external edit").unwrap();
            }
            let before = fixture.bytes();
            let error = prepared.revalidate_content().unwrap_err();
            token.cancel();
            assert_eq!(error.is_cancelled(), cancel_before, "{error}");
            assert_eq!(fixture.bytes(), before);
        }
        let error = PreparedContentError::from(crate::skill_scope::ScopedReadError::Io(
            std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "directory coordination was cancelled",
            ),
        ));
        assert!(!error.is_cancelled());
    }

    #[test]
    fn copy_edit_preparation_preserves_files_across_scopes_and_destinations() {
        for project in [false, true] {
            for per_harness in [false, true] {
                let fixture = Fixture::new(project, per_harness, false);
                let before = fixture.bytes();
                let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
                for proposed in [
                    fixture.request.proposed_content.clone(),
                    "---\nname: renamed\ndescription: Edited\n---\nBody\n".into(),
                ] {
                    let mut request = fixture.request.clone();
                    request.proposed_content = proposed;
                    let CopyDocumentEditPreparation::Ready(prepared) = service
                        .prepare_copy_document_edit(
                            &request,
                            &[],
                            Some(Duration::from_secs(10)),
                            CancellationToken::default(),
                        )
                        .unwrap()
                    else {
                        panic!("edit was not prepared")
                    };
                    prepared.revalidate().unwrap();
                    prepared
                        .intent()
                        .validate_originals(&before.0, &before.1)
                        .unwrap();
                    assert_eq!(
                        prepared.intent().transition().before().deployment_id,
                        request.deployment_id
                    );
                    assert_eq!(prepared.intent().transition().before().name, "sample");
                    assert_eq!(fixture.bytes(), before);
                }
                let mut request = fixture.request.clone();
                request.proposed_content = String::from_utf8(before.0.clone()).unwrap();
                assert!(
                    matches!(service.prepare_copy_document_edit(&request, &[], Some(Duration::from_secs(10)), CancellationToken::default()).unwrap(), CopyDocumentEditPreparation::Unchanged { deployment_id } if deployment_id == request.deployment_id)
                );
                assert_eq!(fixture.bytes(), before);
            }
        }
    }

    #[test]
    fn copy_edit_preparation_preserves_editor_size_limit() {
        let fixture = Fixture::new(false, false, true);
        let before = fixture.bytes();
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        let prepared = service
            .prepare_copy_document_edit(
                &fixture.request,
                &[],
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        assert!(matches!(prepared, CopyDocumentEditPreparation::Ready(_)));
        drop(prepared);
        let mut oversized = fixture.request.clone();
        oversized.proposed_content = "a".repeat(MAX_COPY_DOCUMENT_EDIT_BYTES + 1);
        assert!(service
            .prepare_copy_document_edit(
                &oversized,
                &[],
                Some(Duration::from_secs(10)),
                CancellationToken::default()
            )
            .is_err());
        assert_eq!(fixture.bytes(), before);
    }

    #[test]
    fn copy_edit_preparation_refuses_stale_noop_wrong_owner_and_cancellation() {
        let fixture = Fixture::new(false, false, false);
        let before = fixture.bytes();
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        for field in ["owner", "content", "identity"] {
            let mut request = fixture.request.clone();
            request.proposed_content = String::from_utf8(before.0.clone()).unwrap();
            match field {
                "owner" => request.expected_owner_revision.push('x'),
                "content" => request.expected_content_fingerprint.push('x'),
                _ => request.deployment_id.push('x'),
            }
            assert!(
                service
                    .prepare_copy_document_edit(
                        &request,
                        &[],
                        Some(Duration::from_secs(10)),
                        CancellationToken::default()
                    )
                    .is_err(),
                "{field}"
            );
        }
        let cancellation = CancellationToken::default();
        cancellation.cancel();
        assert!(service
            .prepare_copy_document_edit(
                &fixture.request,
                &[],
                Some(Duration::from_secs(10)),
                cancellation
            )
            .is_err());
        assert_eq!(fixture.bytes(), before);
        fs::write(
            &fixture.registry,
            serde_json::to_vec_pretty(&ForkRegistry::default()).unwrap(),
        )
        .unwrap();
        let without_owner = fixture.bytes();
        assert!(service
            .prepare_copy_document_edit(
                &fixture.request,
                &[],
                Some(Duration::from_secs(10)),
                CancellationToken::default()
            )
            .is_err());
        assert_eq!(fixture.bytes(), without_owner);
    }

    #[test]
    fn copy_edit_preparation_refuses_resource_and_registry_drift() {
        for target in ["SKILL.md", "resource.txt", "new-resource.txt", "registry"] {
            let fixture = Fixture::new(false, false, false);
            let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
            let CopyDocumentEditPreparation::Ready(prepared) = service
                .prepare_copy_document_edit(
                    &fixture.request,
                    &[],
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap()
            else {
                panic!("edit was not prepared")
            };
            let path = if target == "registry" {
                fixture.registry.clone()
            } else {
                fixture.skill.join(target)
            };
            fs::write(path, "external change").unwrap();
            let after_external_change = fixture.bytes();
            assert!(prepared.revalidate().is_err(), "{target}");
            assert_eq!(fixture.bytes(), after_external_change);
        }
    }

    #[test]
    fn copy_edit_preparation_refuses_incomplete_folder() {
        let fixture = Fixture::new(false, false, false);
        std::os::unix::fs::symlink("loop", fixture.skill.join("loop")).unwrap();
        let before = fixture.bytes();
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        assert!(service
            .prepare_copy_document_edit(
                &fixture.request,
                &[],
                Some(Duration::from_secs(10)),
                CancellationToken::default()
            )
            .is_err());
        assert_eq!(fixture.bytes(), before);
    }
}

#[cfg(all(unix, feature = "event-store"))]
#[path = "skill_copy_document_execution.rs"]
mod execution;
#[cfg(all(unix, feature = "event-store"))]
pub use execution::*;
