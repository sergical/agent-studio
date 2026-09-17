use crate::{
    skill_backup_copy::inspect_entry,
    skill_backup_reservation::SkillsShReinstallRequest,
    skill_backup_reservation::{BackupCopyLimits, BackupStateRoot},
    skill_backup_source::{BackupSource, BackupSourceRoot},
    skill_coordination::FinalizedWriteLease,
    skill_dotagents_ledger::DotagentsReinstallRequest,
    skill_event_operations::GuardedEventStore,
    skill_event_store::EventStore,
    skill_fork_registry::RegistryOwnerRecord,
    skill_fork_transition::UnforkRegistryTransition,
    skill_frontmatter_repair::content_fingerprint,
    skill_ownership::LifecycleOwnerKind,
    skill_service::{
        exact_repair_deployment, CancellationToken, ScopedSkillService, WritePreparationError,
    },
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf, time::Duration};

#[path = "skill_unfork_snapshot.rs"]
mod snapshot;
pub use snapshot::{
    SkillsShUnforkSnapshotReference, UnforkSnapshotReceipt, UnforkSnapshotReference,
};
#[path = "skill_unfork_event.rs"]
mod event;
pub use event::{
    DotagentsRuntimeRecord, DotagentsUnforkIntent, PendingDotagentsUnforkEvent,
    PendingSkillsShUnforkEvent, PreparedUnforkPublication, SkillsShRuntimeRecord,
    SkillsShUnforkIntent, SkillsShUnforkProviderState, UnforkProviderState,
    UnforkPublicationDocuments, UnforkPublicationPlan, UnforkPublicationState,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DotagentsUnforkRequest {
    pub deployment_id: String,
    pub expected_owner_revision: String,
    pub expected_document_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsShUnforkRequest {
    pub deployment_id: String,
    pub expected_owner_revision: String,
    pub expected_document_fingerprint: String,
    /// The desktop adapter resolves the isolated mirror once before admission.
    pub resolved_commit: String,
}

pub struct PreparedSkillsShUnfork<'scope> {
    selection: UnforkRegistryTransition,
    reinstall: SkillsShReinstallRequest,
    live: BackupSource,
    live_identity: String,
    registry: Vec<u8>,
    provider_lock: Vec<u8>,
    state_path: PathBuf,
    lease: FinalizedWriteLease<'scope>,
}

impl PreparedSkillsShUnfork<'_> {
    pub fn selection(&self) -> &UnforkRegistryTransition {
        &self.selection
    }
    pub fn reinstall_request(&self) -> &SkillsShReinstallRequest {
        &self.reinstall
    }
    pub fn live_identity(&self) -> &str {
        &self.live_identity
    }
    pub fn revalidate(
        &self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), String> {
        if cancellation.is_cancelled() || store.app_data != self.state_path {
            return Err("skills.sh Unfork preparation changed or cancelled".into());
        }
        GuardedEventStore::bind(store, &self.lease)?;
        self.live.revalidate().map_err(|e| e.to_string())?;
        if inspect_entry(&self.live.directory, &self.live.name, limits, cancellation)
            .map_err(|e| e.to_string())?
            .tree_identity
            != self.live_identity
        {
            return Err("Prepared skills.sh Unfork live tree changed".into());
        }
        let agents = self
            .selection
            .record()
            .skill_dir
            .parent()
            .and_then(|path| path.parent())
            .ok_or("Unfork owner has no agents root")?;
        for (name, expected) in [
            ("skill-studio.json", &self.registry),
            (".skill-lock.json", &self.provider_lock),
        ] {
            if self
                .lease
                .read(&agents.join(name), 8 * 1024 * 1024)
                .map_err(|e| e.to_string())?
                != *expected
            {
                return Err(format!("Prepared skills.sh Unfork {name} changed"));
            }
        }
        self.lease.revalidate().map_err(|e| e.to_string())
    }
}

pub struct PreparedDotagentsUnfork<'scope> {
    selection: UnforkRegistryTransition,
    reinstall: DotagentsReinstallRequest,
    live: BackupSource,
    live_identity: String,
    registry: Vec<u8>,
    provider_lock: Vec<u8>,
    provider_manifest: Vec<u8>,
    state_path: PathBuf,
    lease: FinalizedWriteLease<'scope>,
}

impl PreparedDotagentsUnfork<'_> {
    pub fn selection(&self) -> &UnforkRegistryTransition {
        &self.selection
    }
    pub fn reinstall_request(&self) -> &DotagentsReinstallRequest {
        &self.reinstall
    }
    pub fn live_identity(&self) -> &str {
        &self.live_identity
    }

    pub fn revalidate(
        &self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), String> {
        if cancellation.is_cancelled() {
            return Err("Unfork preparation cancelled".into());
        }
        if store.app_data != self.state_path {
            return Err("Unfork event store changed".into());
        }
        GuardedEventStore::bind(store, &self.lease)?;
        self.live.revalidate().map_err(|error| error.to_string())?;
        let report = inspect_entry(&self.live.directory, &self.live.name, limits, cancellation)
            .map_err(|error| error.to_string())?;
        if report.tree_identity != self.live_identity {
            return Err("Prepared Unfork live tree changed".into());
        }
        let agents = self
            .selection
            .record()
            .skill_dir
            .parent()
            .and_then(|path| path.parent())
            .ok_or("Unfork owner has no agents root")?;
        for (name, expected) in [
            ("skill-studio.json", &self.registry),
            ("agents.lock", &self.provider_lock),
            ("agents.toml", &self.provider_manifest),
        ] {
            let bytes = self
                .lease
                .read(&agents.join(name), 8 * 1024 * 1024)
                .map_err(|error| error.to_string())?;
            if &bytes != expected {
                return Err(format!("Prepared Unfork {name} changed"));
            }
        }
        self.lease.revalidate().map_err(|error| error.to_string())
    }
}

impl ScopedSkillService {
    pub fn prepare_current_skills_sh_unfork(
        &mut self,
        request: &SkillsShUnforkRequest,
        store: &EventStore,
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedSkillsShUnfork<'_>, WritePreparationError> {
        let invalid = WritePreparationError::InvalidRepairSelection;
        let parsed = crate::skill_deployment::parse_deployment_id(&request.deployment_id)
            .ok_or_else(|| invalid("Unfork deployment identity is invalid".into()))?;
        let agents = self.scope().home.join(".agents");
        let canonical_path = agents.join("skills").join(&parsed.name);
        let names = BTreeSet::from([parsed.name.clone()]);
        let (inventory, lease) = self.prepare_write_inventory(
            Some(&names),
            &[store.app_data.clone(), canonical_path.clone()],
            timeout,
            cancellation.clone(),
        )?;
        let deployment = exact_repair_deployment(&inventory, &request.deployment_id)?;
        if deployment.owner_kind != LifecycleOwnerKind::Fork
            || deployment.plugin.is_some()
            || deployment.is_symlink
            || deployment.disabled
            || deployment.scope != "global"
            || deployment.destination != crate::skill_deployment::SkillDestination::Universal
            || !matches!(
                deployment.backing,
                crate::skill_deployment::BackingRelationship::Canonical
            )
            || std::path::Path::new(&deployment.path) != canonical_path
            || deployment.owner_revision.as_deref()
                != Some(request.expected_owner_revision.as_str())
        {
            return Err(invalid(
                "Unfork ownership or canonical location changed".into(),
            ));
        }
        let registry = lease
            .read_ownership_registry(&agents.join("skill-studio.json"), 8 * 1024 * 1024)
            .map_err(invalid)?
            .ok_or_else(|| invalid("Unfork registry is missing".into()))?;
        let selection = UnforkRegistryTransition::bind_current_for_origin(
            parsed.name.clone(),
            request.deployment_id.clone(),
            canonical_path.clone(),
            &registry,
            crate::skill_fork_registry::OriginTool::SkillsSh,
        )
        .map_err(invalid)?;
        if RegistryOwnerRecord::Fork(selection.recorded_provenance())
            .revision()
            .as_deref()
            != Some(request.expected_owner_revision.as_str())
        {
            return Err(invalid("Unfork owner revision changed".into()));
        }
        let reinstall = SkillsShReinstallRequest::from_fork_record(
            selection.recorded_provenance(),
            selection.name(),
            &request.resolved_commit,
        )
        .map_err(invalid)?;
        let bytes = lease
            .read(
                &canonical_path.join("SKILL.md"),
                crate::skill_service::MAX_REPAIR_DOCUMENT_BYTES,
            )
            .map_err(|e| invalid(e.to_string()))?;
        if content_fingerprint(&bytes) != request.expected_document_fingerprint {
            return Err(invalid("Unfork document changed".into()));
        }
        GuardedEventStore::bind(store, &lease).map_err(invalid)?;
        let provider_lock = lease
            .read(&agents.join(".skill-lock.json"), 8 * 1024 * 1024)
            .map_err(|e| invalid(e.to_string()))?;
        let lock = crate::skill_skills_sh_lock_transition::SkillsShLockTransition::read(
            &provider_lock,
            selection.name(),
            reinstall.repo(),
            reinstall.path(),
            reinstall.declared_ref(),
        );
        if lock.is_ok() {
            return Err(invalid(
                "Selected skills.sh lock row remains attached".into(),
            ));
        }
        // Strictly distinguish an absent selected row from malformed or divergent input.
        let parsed_lock: serde_json::Value =
            crate::skill_skills_sh_fork_creation::json_document(&provider_lock).map_err(invalid)?;
        if parsed_lock
            .get("version")
            .and_then(serde_json::Value::as_u64)
            != Some(3)
            || parsed_lock
                .get("skills")
                .and_then(serde_json::Value::as_object)
                .is_none_or(|skills| skills.contains_key(selection.name()))
        {
            return Err(invalid(
                "skills.sh lock is not detached for the selected skill".into(),
            ));
        }
        let root =
            BackupSourceRoot::bind(&agents.join("skills")).map_err(|e| invalid(e.to_string()))?;
        let live = root
            .select(std::ffi::OsStr::new(selection.name()))
            .map_err(|e| invalid(e.to_string()))?;
        lease
            .validate_state_tree(&live.original_path)
            .map_err(invalid)?;
        if !live
            .directory
            .symlink_metadata(&live.name)
            .map_err(|e| invalid(e.to_string()))?
            .is_dir()
        {
            return Err(invalid("Unfork live entry is not a directory".into()));
        }
        let live_identity = inspect_entry(&live.directory, &live.name, limits, &cancellation)
            .map_err(|e| invalid(e.to_string()))?
            .tree_identity;
        let prepared = PreparedSkillsShUnfork {
            selection,
            reinstall,
            live,
            live_identity,
            registry,
            provider_lock,
            state_path: store.app_data.clone(),
            lease,
        };
        prepared
            .revalidate(store, limits, &cancellation)
            .map_err(invalid)?;
        Ok(prepared)
    }

    pub fn prepare_current_dotagents_unfork(
        &mut self,
        request: &DotagentsUnforkRequest,
        store: &EventStore,
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedDotagentsUnfork<'_>, WritePreparationError> {
        let invalid = WritePreparationError::InvalidRepairSelection;
        let parsed = crate::skill_deployment::parse_deployment_id(&request.deployment_id)
            .ok_or_else(|| invalid("Unfork deployment identity is invalid".into()))?;
        let agents = self.scope().home.join(".agents");
        let canonical_path = agents.join("skills").join(&parsed.name);
        let names = BTreeSet::from([parsed.name.clone()]);
        let (inventory, lease) = self.prepare_write_inventory(
            Some(&names),
            &[store.app_data.clone(), canonical_path.clone()],
            timeout,
            cancellation.clone(),
        )?;
        let deployment = exact_repair_deployment(&inventory, &request.deployment_id)?;
        if deployment.owner_kind != LifecycleOwnerKind::Fork
            || deployment.plugin.is_some()
            || deployment.is_symlink
            || deployment.disabled
            || deployment.scope != "global"
            || deployment.destination != crate::skill_deployment::SkillDestination::Universal
            || !matches!(
                deployment.backing,
                crate::skill_deployment::BackingRelationship::Canonical
            )
            || std::path::Path::new(&deployment.path) != canonical_path
            || deployment.owner_revision.as_deref()
                != Some(request.expected_owner_revision.as_str())
        {
            return Err(invalid(
                "Unfork ownership or canonical location changed".into(),
            ));
        }
        let registry = lease
            .read_ownership_registry(&agents.join("skill-studio.json"), 8 * 1024 * 1024)
            .map_err(invalid)?
            .ok_or_else(|| invalid("Unfork registry is missing".into()))?;
        let selection = UnforkRegistryTransition::bind_current(
            parsed.name.clone(),
            request.deployment_id.clone(),
            canonical_path.clone(),
            &registry,
        )
        .map_err(invalid)?;
        if RegistryOwnerRecord::Fork(selection.recorded_provenance())
            .revision()
            .as_deref()
            != Some(request.expected_owner_revision.as_str())
        {
            return Err(invalid("Unfork owner revision changed".into()));
        }
        let reinstall = DotagentsReinstallRequest::from_fork_record(
            selection.recorded_provenance(),
            selection.name(),
        )
        .map_err(invalid)?;
        let bytes = lease
            .read(
                &canonical_path.join("SKILL.md"),
                crate::skill_service::MAX_REPAIR_DOCUMENT_BYTES,
            )
            .map_err(|error| invalid(error.to_string()))?;
        if content_fingerprint(&bytes) != request.expected_document_fingerprint {
            return Err(invalid("Unfork document changed".into()));
        }
        GuardedEventStore::bind(store, &lease).map_err(invalid)?;
        let provider_lock = lease
            .read(&agents.join("agents.lock"), 8 * 1024 * 1024)
            .map_err(|error| invalid(error.to_string()))?;
        let provider_manifest = lease
            .read(&agents.join("agents.toml"), 8 * 1024 * 1024)
            .map_err(|error| invalid(error.to_string()))?;
        reinstall
            .validate_detached_documents(
                std::str::from_utf8(&provider_lock).map_err(|error| invalid(error.to_string()))?,
                std::str::from_utf8(&provider_manifest)
                    .map_err(|error| invalid(error.to_string()))?,
            )
            .map_err(invalid)?;
        let root = BackupSourceRoot::bind(&agents.join("skills"))
            .map_err(|error| invalid(error.to_string()))?;
        let live = root
            .select(std::ffi::OsStr::new(selection.name()))
            .map_err(|error| invalid(error.to_string()))?;
        lease
            .validate_state_tree(&live.original_path)
            .map_err(invalid)?;
        if !live
            .directory
            .symlink_metadata(&live.name)
            .map_err(|error| invalid(error.to_string()))?
            .is_dir()
        {
            return Err(invalid("Unfork live entry is not a directory".into()));
        }
        let live_identity = inspect_entry(&live.directory, &live.name, limits, &cancellation)
            .map_err(|error| invalid(error.to_string()))?
            .tree_identity;
        let prepared = PreparedDotagentsUnfork {
            selection,
            reinstall,
            live,
            live_identity,
            registry,
            provider_lock,
            provider_manifest,
            state_path: store.app_data.clone(),
            lease,
        };
        prepared
            .revalidate(store, limits, &cancellation)
            .map_err(invalid)?;
        Ok(prepared)
    }
}

#[cfg(test)]
pub(crate) fn verify_preparation_fixture(
    service: &mut ScopedSkillService,
    store: &EventStore,
    completed: &crate::skill_event::EventRow,
    limits: BackupCopyLimits,
) {
    let completed =
        crate::skill_fork_repair_intent::CompletedDotagentsForkEvent::from_row(completed).unwrap();
    let intent = completed.intent();
    let record = intent.registry().record();
    let document_path = record.skill_dir.join("SKILL.md");
    let document = std::fs::read(&document_path).unwrap();
    assert_eq!(document, intent.repair().proposed_content.as_bytes());
    let request = DotagentsUnforkRequest {
        deployment_id: record.deployment_id.clone(),
        expected_owner_revision: RegistryOwnerRecord::Fork(record).revision().unwrap(),
        expected_document_fingerprint: content_fingerprint(&document),
    };
    let prepared = service
        .prepare_current_dotagents_unfork(
            &request,
            store,
            limits,
            Some(Duration::from_secs(10)),
            CancellationToken::default(),
        )
        .unwrap();
    assert!(prepared.selection().is_bound_current());
    assert_eq!(prepared.selection().record(), record);
    assert_eq!(prepared.reinstall_request().name(), intent.repair().name);
    assert_eq!(prepared.reinstall_request().source(), record.origin_source);
    assert_eq!(prepared.reinstall_request().repo(), record.repo);
    assert_eq!(prepared.reinstall_request().path(), record.path);
    assert_eq!(
        prepared.reinstall_request().declared_ref(),
        record.declared_ref.as_deref()
    );
    prepared
        .revalidate(store, limits, &CancellationToken::default())
        .unwrap();
}

#[cfg(test)]
mod current_record_lifecycle_tests {
    use super::*;
    use crate::{
        skill_deployment::{deployment_id, SkillDestination},
        skill_fork_registry::{ForkRecord, OriginTool},
        skill_service::SkillScope,
    };
    use std::{fs, path::Path, time::Duration};

    fn runtime() -> DotagentsRuntimeRecord {
        DotagentsRuntimeRecord {
            provider_version: "3.0.1".into(),
            provider_tree_identity: format!("tree-v1:{}", "a".repeat(64)),
            node_version: "v26.8.2".into(),
            node_content_digest: format!("sha256:{}", "b".repeat(64)),
            copy_contract: "dotagents-3.0.1-default-node-copy".into(),
        }
    }

    #[test]
    fn current_records_publish_without_history_and_refuse_changed_evidence() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let agents = home.join(".agents");
        fs::create_dir_all(agents.join("skills")).unwrap();
        let state_path = root.path().join("state");
        let store = EventStore::open(&state_path).unwrap();
        let limits = BackupCopyLimits {
            max_bytes: 2 * 1024 * 1024,
            max_entries: 128,
            max_depth: 16,
        };
        let token = CancellationToken::default();

        for legacy in [false, true] {
            let name = if legacy { "legacy" } else { "modern" };
            let source = if legacy {
                "git:https://github.com/owner/repo.git"
            } else {
                "owner/repo"
            };
            let cache_root = if legacy {
                "github.com/owner/repo"
            } else {
                "owner/repo"
            };
            let live = agents.join("skills").join(name);
            fs::create_dir_all(&live).unwrap();
            fs::write(live.join("SKILL.md"), format!("edited {name}")).unwrap();
            fs::write(agents.join("agents.lock"), "version = 1\n").unwrap();
            fs::write(agents.join("agents.toml"), "version = 1\n").unwrap();
            let id = deployment_id(
                name,
                "global",
                SkillDestination::Universal,
                "universal",
                None,
                &live,
            );
            let record = ForkRecord {
                deployment_id: id.clone(),
                skill_dir: live.clone(),
                forked_at: "2026-09-14T00:00:00Z".into(),
                origin_tool: OriginTool::Dotagents,
                origin_source: source.into(),
                repo: "owner/repo".into(),
                path: format!("skills/{name}"),
                declared_ref: Some("main".into()),
                base_commit: "a".repeat(40),
            };
            let mut raw = serde_json::to_value(&record).unwrap();
            raw["future"] = serde_json::json!({"preserve": true});
            if legacy {
                raw.as_object_mut().unwrap().remove("deployment_id");
                raw.as_object_mut().unwrap().remove("skill_dir");
            }
            let registry = serde_json::json!({"forks": { name: raw }, "trials": {}, "unrelated": {"keep": true}});
            fs::write(
                agents.join("skill-studio.json"),
                serde_json::to_vec_pretty(&registry).unwrap(),
            )
            .unwrap();
            let scope = SkillScope {
                home: home.clone(),
                projects: vec![],
                backing_roots: vec![],
                plugin_ownership_roots: vec![],
            };
            let mut service = ScopedSkillService::bind(scope.clone()).unwrap();
            let inventory = service.scan(None, None).unwrap();
            let owner_revision = inventory
                .skills
                .iter()
                .flat_map(|skill| &skill.deployments)
                .find(|deployment| deployment.id == id)
                .and_then(|deployment| deployment.owner_revision.clone())
                .unwrap();
            let request = DotagentsUnforkRequest {
                deployment_id: id,
                expected_owner_revision: owner_revision,
                expected_document_fingerprint: content_fingerprint(
                    &fs::read(live.join("SKILL.md")).unwrap(),
                ),
            };
            let prepared = service
                .prepare_current_dotagents_unfork(
                    &request,
                    &store,
                    limits,
                    Some(Duration::from_secs(5)),
                    token.clone(),
                )
                .unwrap();
            let registry_path = agents.join("skill-studio.json");
            let original_registry = fs::read(&registry_path).unwrap();
            let mut changed_registry: serde_json::Value =
                serde_json::from_slice(&original_registry).unwrap();
            changed_registry["forks"][name]["origin_source"] = serde_json::json!("other/repo");
            fs::write(
                &registry_path,
                serde_json::to_vec(&changed_registry).unwrap(),
            )
            .unwrap();
            assert!(prepared.revalidate(&store, limits, &token).is_err());
            fs::write(&registry_path, original_registry).unwrap();
            drop(prepared);
            let prepared = service
                .prepare_current_dotagents_unfork(
                    &request,
                    &store,
                    limits,
                    Some(Duration::from_secs(5)),
                    token.clone(),
                )
                .unwrap();
            let op = format!("{name}-unfork");
            let pending = prepared
                .record_native_pending(&store, &op, runtime(), limits, &token)
                .unwrap();
            let started = prepared
                .mark_provider_may_have_started(&store, &pending, limits, &token)
                .unwrap();
            let state = BackupStateRoot::bind(&store.app_data).unwrap();
            let reservation = state.open_managed_source_reservation(&op).unwrap();
            let stage = reservation.stage_path().unwrap().join("home/.agents");
            fs::create_dir_all(
                reservation
                    .cache_path()
                    .unwrap()
                    .join(cache_root)
                    .join(format!("skills/{name}")),
            )
            .unwrap();
            fs::write(
                reservation
                    .cache_path()
                    .unwrap()
                    .join(cache_root)
                    .join(format!("skills/{name}"))
                    .join("SKILL.md"),
                format!("published {name}"),
            )
            .unwrap();
            fs::create_dir_all(stage.join("skills").join(name)).unwrap();
            fs::write(
                stage.join("skills").join(name).join("SKILL.md"),
                format!("published {name}"),
            )
            .unwrap();
            fs::write(stage.join("agents.lock"), format!("version = 1\n[skills.{name}]\nsource = '{source}'\nresolved_path = 'skills/{name}'\nresolved_commit = '{}'\n", "b".repeat(40))).unwrap();
            fs::write(
                stage.join("agents.toml"),
                format!(
                    "version = 1\n[[skills]]\nname = '{name}'\nsource = '{source}'\nref = 'main'\n"
                ),
            )
            .unwrap();
            let reference = reservation.seal_cache(limits, &token).unwrap();
            let sealed = state
                .open_managed_source(&reference, limits, &token)
                .unwrap();
            let staged = sealed
                .record_dotagents_stage_v2(
                    prepared.reinstall_request(),
                    Path::new("home/.agents"),
                    limits,
                    &token,
                )
                .unwrap();
            let verified = prepared
                .record_verified_source(&store, &started, &staged, limits, &token)
                .unwrap();
            prepared
                .begin_publication(&store, &verified, limits, &token)
                .unwrap();
            drop(prepared);
            let cache = sealed.cache_path(limits, &token).unwrap();
            let mut publication_scope = scope.clone();
            publication_scope.backing_roots.push(cache);
            store
                .conn
                .execute(
                    "UPDATE events SET status = 'interrupted' WHERE id = ?1",
                    [&op],
                )
                .unwrap();
            let interrupted =
                PendingDotagentsUnforkEvent::from_row(&store.get(&op).unwrap().unwrap()).unwrap();
            let registry_path = agents.join("skill-studio.json");
            let registry_before_recovery = fs::read(&registry_path).unwrap();
            let mut changed_registry: serde_json::Value =
                serde_json::from_slice(&registry_before_recovery).unwrap();
            changed_registry["forks"][name]["repo"] = serde_json::json!("other/repo");
            fs::write(
                &registry_path,
                serde_json::to_vec(&changed_registry).unwrap(),
            )
            .unwrap();
            assert!(ScopedSkillService::bind(publication_scope.clone())
                .unwrap()
                .resume_unfork_publication(
                    &interrupted,
                    &store,
                    limits,
                    Some(Duration::from_secs(5)),
                    token.clone(),
                )
                .is_err());
            fs::write(&registry_path, registry_before_recovery).unwrap();
            let publication = ScopedSkillService::bind(publication_scope).unwrap();
            publication
                .resume_unfork_publication(
                    &interrupted,
                    &store,
                    limits,
                    Some(Duration::from_secs(5)),
                    token.clone(),
                )
                .unwrap();
            assert_eq!(
                fs::read_to_string(live.join("SKILL.md")).unwrap(),
                format!("published {name}")
            );
            let after: serde_json::Value =
                serde_json::from_slice(&fs::read(agents.join("skill-studio.json")).unwrap())
                    .unwrap();
            assert!(after["forks"].get(name).is_none());
            assert_eq!(after["unrelated"]["keep"], true);
            assert_eq!(store.get(&op).unwrap().unwrap().status, "done");
        }
    }
    #[test]
    fn skills_sh_current_records_recover_every_publication_prefix() {
        for legacy in [false, true] {
            for prefix in -2_i32..=3 {
                let root = tempfile::tempdir().unwrap();
                let home = root.path().join("home");
                let agents = home.join(".agents");
                let live = agents.join("skills/alpha");
                fs::create_dir_all(&live).unwrap();
                fs::write(live.join("SKILL.md"), "local edits").unwrap();
                fs::write(agents.join(".skill-lock.json"), br#"{"version":3,"future":true,"skills":{"sibling":{"source":"other/repo","sourceType":"github","sourceUrl":"https://github.com/other/repo.git","skillPath":"SKILL.md","skillFolderHash":"sibling-hash","installedAt":"now","updatedAt":"now","future":"keep"}}}"#).unwrap();
                let id = deployment_id(
                    "alpha",
                    "global",
                    SkillDestination::Universal,
                    "universal",
                    None,
                    &live,
                );
                let record = ForkRecord {
                    deployment_id: id.clone(),
                    skill_dir: live.clone(),
                    forked_at: "2026-09-16T00:00:00Z".into(),
                    origin_tool: OriginTool::SkillsSh,
                    origin_source: "owner/repo".into(),
                    repo: "owner/repo".into(),
                    path: "skills/alpha/SKILL.md".into(),
                    declared_ref: None,
                    base_commit: "a".repeat(40),
                };
                let mut raw = serde_json::to_value(&record).unwrap();
                if legacy {
                    raw.as_object_mut().unwrap().remove("deployment_id");
                    raw.as_object_mut().unwrap().remove("skill_dir");
                }
                let registry =
                    serde_json::json!({"forks":{"alpha":raw},"trials":{},"future":"keep"});
                fs::write(
                    agents.join("skill-studio.json"),
                    serde_json::to_vec(&registry).unwrap(),
                )
                .unwrap();
                let store = EventStore::open(&root.path().join("state")).unwrap();
                let scope = SkillScope {
                    home: home.clone(),
                    projects: vec![],
                    backing_roots: vec![],
                    plugin_ownership_roots: vec![],
                };
                let mut service = ScopedSkillService::bind(scope.clone()).unwrap();
                let limits = BackupCopyLimits {
                    max_bytes: 2 * 1024 * 1024,
                    max_entries: 128,
                    max_depth: 16,
                };
                let token = CancellationToken::default();
                let inventory = service.scan(None, None).unwrap();
                let owner_revision = inventory
                    .skills
                    .iter()
                    .flat_map(|skill| &skill.deployments)
                    .find(|deployment| deployment.id == id)
                    .and_then(|deployment| deployment.owner_revision.clone())
                    .unwrap();
                let request = SkillsShUnforkRequest {
                    deployment_id: id,
                    expected_owner_revision: owner_revision,
                    expected_document_fingerprint: content_fingerprint(b"local edits"),
                    resolved_commit: "b".repeat(40),
                };
                let prepared = service
                    .prepare_current_skills_sh_unfork(
                        &request,
                        &store,
                        limits,
                        Some(Duration::from_secs(5)),
                        token.clone(),
                    )
                    .unwrap();
                let runtime = SkillsShRuntimeRecord {
                    provider_version: "1.5.25".into(),
                    provider_tree_identity: format!("tree-v1:{}", "a".repeat(64)),
                    node_version: "v24.19.0".into(),
                    node_content_digest: format!("sha256:{}", "b".repeat(64)),
                    copy_contract: "skills-1.5.25-global-universal-copy".into(),
                };
                let pending = prepared
                    .record_skills_sh_pending(&store, "operation", runtime, limits, &token)
                    .unwrap();
                if prefix == -2 {
                    drop(prepared);
                    let mut restarted = ScopedSkillService::bind(scope.clone()).unwrap();
                    fs::write(live.join("SKILL.md"), "outside edit").unwrap();
                    assert!(restarted
                        .prepare_skills_sh_unfork_resume(
                            &pending,
                            &store,
                            limits,
                            Some(Duration::from_secs(5)),
                            token.clone()
                        )
                        .is_err());
                    fs::write(live.join("SKILL.md"), "local edits").unwrap();
                    restarted
                        .prepare_skills_sh_unfork_resume(
                            &pending,
                            &store,
                            limits,
                            Some(Duration::from_secs(5)),
                            token.clone(),
                        )
                        .unwrap()
                        .resolve_skills_sh_unapplied(&store, &pending, limits, &token)
                        .unwrap();
                    assert_eq!(store.get("operation").unwrap().unwrap().status, "failed");
                    assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), b"local edits");
                    continue;
                }
                let started = prepared
                    .mark_skills_sh_provider_may_have_started(&store, &pending, limits, &token)
                    .unwrap();
                if prefix == -1 {
                    drop(prepared);
                    let mut restarted = ScopedSkillService::bind(scope.clone()).unwrap();
                    restarted
                        .prepare_skills_sh_unfork_resume(
                            &started,
                            &store,
                            limits,
                            Some(Duration::from_secs(5)),
                            token.clone(),
                        )
                        .unwrap()
                        .resolve_skills_sh_unapplied(&store, &started, limits, &token)
                        .unwrap();
                    assert_eq!(store.get("operation").unwrap().unwrap().status, "failed");
                    assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), b"local edits");
                    continue;
                }
                let state = BackupStateRoot::bind(&store.app_data).unwrap();
                let reserved = state.open_managed_source_reservation("operation").unwrap();
                let source = reserved
                    .cache_path()
                    .unwrap()
                    .join("owner/repo/skills/alpha");
                fs::create_dir_all(&source).unwrap();
                fs::write(source.join("SKILL.md"), "upstream").unwrap();
                reserved
                    .admit_skills_sh_source(prepared.reinstall_request(), limits, &token)
                    .unwrap();
                let stage = reserved.stage_path().unwrap().join("home/.agents");
                fs::create_dir_all(stage.join("skills/alpha")).unwrap();
                fs::write(stage.join("skills/alpha/SKILL.md"), "upstream").unwrap();
                let row = serde_json::json!({"source":"owner/repo","sourceType":"github","sourceUrl":"https://github.com/owner/repo.git","skillPath":"skills/alpha/SKILL.md","skillFolderHash":"c".repeat(64),"installedAt":"now","updatedAt":"now","future":"preserved"});
                fs::write(
                    stage.join(".skill-lock.json"),
                    serde_json::to_vec(&serde_json::json!({"version":3,"skills":{"alpha":row}}))
                        .unwrap(),
                )
                .unwrap();
                let reference = reserved.seal_cache(limits, &token).unwrap();
                let sealed = state
                    .open_managed_source(&reference, limits, &token)
                    .unwrap();
                let staged = sealed
                    .record_skills_sh_stage(
                        prepared.reinstall_request(),
                        Path::new("home/.agents"),
                        limits,
                        &token,
                    )
                    .unwrap();
                let verified = prepared
                    .record_skills_sh_verified_source(&store, &started, &staged, limits, &token)
                    .unwrap();
                drop(prepared);
                let prepared = service
                    .prepare_skills_sh_unfork_resume(
                        &verified,
                        &store,
                        limits,
                        Some(Duration::from_secs(5)),
                        token.clone(),
                    )
                    .unwrap();
                let publishing = prepared
                    .begin_skills_sh_publication(&store, &verified, limits, &token)
                    .unwrap();
                drop(prepared);
                let mut publication_scope = scope;
                publication_scope
                    .backing_roots
                    .push(sealed.cache_path(limits, &token).unwrap());
                let mut publication = ScopedSkillService::bind(publication_scope.clone()).unwrap();
                if prefix > 0 {
                    publication
                        .prepare_skills_sh_unfork_publication(
                            &publishing,
                            &store,
                            limits,
                            Some(Duration::from_secs(5)),
                            token.clone(),
                        )
                        .unwrap()
                        .exchange_tree(&store, limits, &token)
                        .unwrap();
                }
                if prefix > 1 {
                    let result = publication
                        .prepare_skills_sh_unfork_publication(
                            &publishing,
                            &store,
                            limits,
                            Some(Duration::from_secs(5)),
                            token.clone(),
                        )
                        .unwrap()
                        .publish_documents_with_checkpoint(&store, limits, &token, |index| {
                            if index as i32 == prefix - 2 {
                                Err("injected interruption".into())
                            } else {
                                Ok(())
                            }
                        });
                    assert!(result.is_err());
                }
                store
                    .conn
                    .execute(
                        "UPDATE events SET status='interrupted' WHERE id='operation'",
                        [],
                    )
                    .unwrap();
                let interrupted =
                    PendingSkillsShUnforkEvent::from_row(&store.get("operation").unwrap().unwrap())
                        .unwrap();
                let lock_path = agents.join(".skill-lock.json");
                let before_lock = fs::read(&lock_path).unwrap();
                let mut drift: serde_json::Value = serde_json::from_slice(&before_lock).unwrap();
                drift["skills"]["alpha"] = serde_json::json!({"source":"other/repo"});
                fs::write(&lock_path, serde_json::to_vec(&drift).unwrap()).unwrap();
                assert!(publication
                    .resume_skills_sh_unfork_publication(
                        &interrupted,
                        &store,
                        limits,
                        Some(Duration::from_secs(5)),
                        token.clone()
                    )
                    .is_err());
                fs::write(&lock_path, &before_lock).unwrap();
                drop(publication);
                ScopedSkillService::bind(publication_scope)
                    .unwrap()
                    .resume_skills_sh_unfork_publication(
                        &interrupted,
                        &store,
                        limits,
                        Some(Duration::from_secs(5)),
                        token.clone(),
                    )
                    .unwrap_or_else(|error| panic!("legacy={legacy} prefix={prefix}: {error}"));
                assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), b"upstream");
                let after: serde_json::Value =
                    serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
                assert_eq!(after["skills"]["alpha"], row);
                assert_eq!(after["skills"]["sibling"]["future"], "keep");
                assert_eq!(after["future"], true);
                let after: serde_json::Value =
                    serde_json::from_slice(&fs::read(agents.join("skill-studio.json")).unwrap())
                        .unwrap();
                assert!(after["forks"].get("alpha").is_none());
                assert_eq!(after["future"], "keep");
                assert_eq!(store.get("operation").unwrap().unwrap().status, "done");
            }
        }
    }
}
