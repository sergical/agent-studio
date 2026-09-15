//! Durable removal intent for one independently owned Copy deployment.
//!
//! The event deliberately retains the moved tree.  It is a recovery artifact,
//! not an undo record and no removal path recursively deletes it.
use crate::skill_copy_registry_removal::raw_trial_relates;
use crate::{
    skill_backup_copy::{inspect_entry, BackupCopyLimits},
    skill_backup_source::{BackupSource, BackupSourceRoot},
    skill_coordination::{CancellationToken, FinalizedWriteLease},
    skill_deployment::{parse_deployment_id, InstallScope, SkillDestination},
    skill_event::{EventDraft, EventRow, EventStatus},
    skill_event_operations::{EventWriteFailure, GuardedEventStore},
    skill_event_store::EventStore,
    skill_fork_registry::{CopyDeploymentRecord, RegistryOwnerRecord},
    skill_ownership::LifecycleOwnerKind,
    skill_scope::SkillReadScope,
    skill_service::ScopedSkillService,
};
use cap_std::fs::MetadataExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
    time::Duration,
};

pub const EVENT_KIND: &str = "remove_copy_deployment";

#[cfg(test)]
mod execution_tests;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyRemovalReader {
    pub deployment_id: String,
    pub path: PathBuf,
    pub raw_target: PathBuf,
    /// The exact unmodified registry value observed at admission. `None`
    /// records a checked absence in older registries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_value: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyRemovalIntent {
    version: u32,
    selected: CopyDeploymentRecord,
    readers: Vec<CopyRemovalReader>,
    expected_tree: String,
    registry_path: PathBuf,
    quarantine_path: PathBuf,
    configured_roots: Vec<PathBuf>,
    registry_values: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    trial_values: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyRemovalObservedState {
    /// Intent is durable but no selected live effect occurred.
    BeforeQuarantine,
    /// The selected tree moved; registry still owns it.
    Quarantined { readers_removed: bool },
    /// Ownership no longer names the selected deployment. The quarantined tree
    /// is retained and original paths are no longer writable by recovery.
    Published,
}

impl CopyRemovalIntent {
    pub fn selected(&self) -> &CopyDeploymentRecord {
        &self.selected
    }
    pub fn readers(&self) -> &[CopyRemovalReader] {
        &self.readers
    }
    pub fn expected_tree(&self) -> &str {
        &self.expected_tree
    }
    pub fn quarantine_path(&self) -> &Path {
        &self.quarantine_path
    }
    pub fn registry_path(&self) -> &Path {
        &self.registry_path
    }

    /// Persisted roots are audit data only. Each executor must call this with
    /// roots derived from its current approved `SkillScope` before any path IO.
    pub fn validate_current_roots(&self, current_roots: &[PathBuf]) -> Result<(), String> {
        self.validate()?;
        if current_roots.is_empty() || current_roots.iter().any(|root| !root.is_absolute()) {
            return Err("Copy removal has no approved configured roots".into());
        }
        let selected_root = current_roots.iter().any(|root| {
            self.selected.path.starts_with(root)
                || self
                    .selected
                    .path
                    .parent()
                    .and_then(Path::parent)
                    .is_some_and(|parent| parent == root)
        });
        if !selected_root
            || current_roots
                .iter()
                .any(|root| self.quarantine_path.starts_with(root))
        {
            return Err(
                "Copy removal paths are outside or overlap current configured roots".into(),
            );
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        let selected = &self.selected;
        let parsed = parse_deployment_id(&selected.deployment_id)
            .ok_or("Invalid Copy removal deployment ID")?;
        let scope = match selected.scope {
            InstallScope::Global => "global",
            InstallScope::Project => "project",
        };
        let tree_ok = self
            .expected_tree
            .strip_prefix("tree-v1:")
            .is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            });
        let clean = |path: &Path| {
            path.is_absolute()
                && !path
                    .components()
                    .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        };
        if self.version != 1
            || !tree_ok
            || !clean(&selected.path)
            || !clean(&self.registry_path)
            || !clean(&self.quarantine_path)
            || selected.destination != SkillDestination::Universal
                && selected.destination != SkillDestination::PerHarness
            || parsed.name != selected.name
            || parsed.scope != scope
            || parsed.destination != selected.destination
            || parsed.slot != selected.slot
            || parsed.project_path != selected.project_path
            || parsed.lexical_path != selected.path
            || self.registry_path.file_name().and_then(|n| n.to_str()) != Some("skill-studio.json")
            || self
                .registry_path
                .parent()
                .and_then(Path::file_name)
                .and_then(|n| n.to_str())
                != Some(".agents")
            || self.configured_roots.is_empty()
            || self.configured_roots.iter().any(|root| !clean(root))
            || self
                .readers
                .iter()
                .any(|reader| !clean(&reader.path) || reader.raw_target.as_os_str().is_empty())
        {
            return Err("Invalid or unsupported Copy removal intent".into());
        }
        if self.readers.iter().any(|a| {
            self.readers
                .iter()
                .filter(|b| b.deployment_id == a.deployment_id || b.path == a.path)
                .count()
                != 1
        }) {
            return Err("Copy removal readers are ambiguous".into());
        }
        if self.readers.iter().any(|reader| {
            parse_deployment_id(&reader.deployment_id).is_none_or(|reader_id| {
                selected.destination != SkillDestination::Universal
                    || reader_id.name != parsed.name
                    || reader_id.scope != parsed.scope
                    || reader_id.project_path != parsed.project_path
                    || reader_id.destination != SkillDestination::Universal
                    || reader_id.lexical_path != reader.path
            })
        }) {
            return Err(
                "Copy removal reader identity does not match the selected deployment".into(),
            );
        }
        let allowed: BTreeSet<_> = std::iter::once(selected.deployment_id.as_str())
            .chain(
                self.readers
                    .iter()
                    .map(|reader| reader.deployment_id.as_str()),
            )
            .collect();
        if !self.registry_values.contains_key(&selected.deployment_id)
            || self
                .registry_values
                .keys()
                .any(|key| !allowed.contains(key.as_str()))
            || self.readers.iter().any(|reader| {
                match (
                    &reader.registry_value,
                    self.registry_values.get(&reader.deployment_id),
                ) {
                    (Some(expected), Some(actual)) => expected != actual,
                    (None, None) => false,
                    _ => true,
                }
            })
            || self.registry_values.iter().any(|(key, value)| {
                serde_json::from_value::<CopyDeploymentRecord>(value.clone()).map_or(
                    true,
                    |record| {
                        record.deployment_id != *key
                            || (key == &selected.deployment_id && record != *selected)
                            || self
                                .readers
                                .iter()
                                .find(|reader| reader.deployment_id == *key)
                                .is_some_and(|reader| record.path != reader.path)
                    },
                )
            })
            || self
                .trial_values
                .values()
                .any(|value| !raw_trial_relates(value, selected, &self.readers))
        {
            return Err("Copy removal lacks the selected raw registry record".into());
        }
        Ok(())
    }

    /// Removes only the selected, typed Copy ownership records. Unknown JSON
    /// fields and unrelated records remain byte-for-byte represented by the
    /// same JSON values.
    pub fn apply_registry_document(&self, original: &[u8]) -> Result<Vec<u8>, String> {
        self.validate()?;
        crate::skill_copy_registry_removal::remove_copy_records(
            original,
            &self.selected,
            &self.readers,
            &self.registry_values,
            &self.trial_values,
        )
    }

    /// Classifies only complete, ordered effects. Callers must supply `None`
    /// only after a checked absence; an unreadable path is an error, never an
    /// absence. Reader booleans describe exact raw-target links.
    pub fn observe(
        &self,
        source_tree: Option<&str>,
        quarantine_tree: Option<&str>,
        readers_present: &[bool],
        registry: &[u8],
    ) -> Result<CopyRemovalObservedState, String> {
        self.validate()?;
        if readers_present.len() != self.readers.len() {
            return Err("Copy removal reader observation is incomplete".into());
        }
        let proposed = self.apply_registry_document(registry)?;
        let published = proposed == registry;
        let readers_removed = readers_present.iter().all(|present| !present);
        if published {
            // Registry publication transfers ownership. Never inspect or
            // write original paths after this point: new occupants are not us.
            return match quarantine_tree {
                Some(tree) if tree == self.expected_tree => Ok(CopyRemovalObservedState::Published),
                _ => Err("Copy removal quarantine changed after publication".into()),
            };
        }
        match (source_tree, quarantine_tree) {
            (Some(source), None) if source == self.expected_tree => {
                Ok(CopyRemovalObservedState::BeforeQuarantine)
            }
            (None, Some(tree)) if tree == self.expected_tree => {
                Ok(CopyRemovalObservedState::Quarantined { readers_removed })
            }
            _ => Err("Copy removal effects are missing, changed or out of order".into()),
        }
    }

    pub fn event_draft(&self) -> Result<EventDraft, String> {
        self.validate()?;
        Ok(EventDraft {
            kind: EVENT_KIND.into(),
            skill: self.selected.name.clone(),
            harness: Some(self.selected.slot.clone()),
            scope: Some(
                match self.selected.scope {
                    InstallScope::Global => "global",
                    InstallScope::Project => "project",
                }
                .into(),
            ),
            project_path: self.selected.project_path.clone(),
            payload: serde_json::to_value(self).map_err(|e| e.to_string())?,
            inverse: None,
            backup_dir: None,
            restorable: false,
        })
    }

    pub fn from_event(row: &EventRow) -> Result<Self, String> {
        if row.kind != EVENT_KIND
            || row.restorable
            || row.inverse.is_some()
            || row.backup_dir.is_some()
        {
            return Err("Not a Copy removal event".into());
        }
        let intent: Self =
            serde_json::from_value(row.payload.clone()).map_err(|e| e.to_string())?;
        intent.validate()?;
        let scope = match intent.selected.scope {
            InstallScope::Global => "global",
            InstallScope::Project => "project",
        };
        if row.skill != intent.selected.name
            || row.harness.as_deref() != Some(intent.selected.slot.as_str())
            || row.scope.as_deref() != Some(scope)
            || row.project_path != intent.selected.project_path
            || intent.quarantine_path.file_name().and_then(|n| n.to_str()) != Some(row.id.as_str())
            || intent
                .quarantine_path
                .parent()
                .and_then(Path::file_name)
                .and_then(|n| n.to_str())
                != Some(".skill-studio-removing")
        {
            return Err("Copy removal event metadata does not match its intent".into());
        }
        Ok(intent)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyRemovalRequest {
    pub deployment_id: String,
    pub expected_owner_revision: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CopyRemovalOutcome {
    pub event_id: String,
    pub removed_deployment_ids: Vec<String>,
}

#[derive(Debug)]
pub struct CopyRemovalError {
    pub event_id: Option<String>,
    pub recovery_required: bool,
    pub message: String,
}

struct PreparedCopyRemoval<'scope> {
    forward_ownership_matches: bool,
    intent: CopyRemovalIntent,
    registry: Vec<u8>,
    source: BackupSource,
    quarantine: Option<BackupSource>,
    holding: BackupSource,
    readers: Vec<BackupSource>,
    reader_stages: Vec<BackupSource>,
    lease: FinalizedWriteLease<'scope>,
    cancellation: CancellationToken,
}

fn configured_roots(scope: &crate::skill_service::SkillScope) -> Vec<PathBuf> {
    crate::skill_agents::skill_roots(&scope.home, &scope.projects)
        .into_iter()
        .filter(|root| root.label != "parked")
        .map(|root| root.path)
        .collect()
}

fn selected_root<'a>(record: &CopyDeploymentRecord, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {
    roots.iter().find(|root| {
        record.path == root.join(&record.name)
            || record.path == root.join(".skill-studio-disabled").join(&record.name)
    })
}

fn validate_quarantine_scope(intent: &CopyRemovalIntent, roots: &[PathBuf]) -> Result<(), String> {
    intent.validate_current_roots(roots)?;
    let root = selected_root(intent.selected(), roots)
        .ok_or("Copy removal source is outside current configured roots")?;
    let expected = root
        .parent()
        .ok_or("Configured skill root has no parent")?
        .join(".skill-studio-removing")
        .join(
            intent
                .quarantine_path()
                .file_name()
                .ok_or("Quarantine name is missing")?,
        );
    if expected != intent.quarantine_path() {
        return Err("Copy removal quarantine is not bound to the selected root".into());
    }
    for reader in &intent.readers {
        if !roots
            .iter()
            .any(|root| reader.path == root.join(&intent.selected.name))
        {
            return Err("Copy removal reader is outside the current selected scope".into());
        }
    }
    let parent = root.parent().ok_or("Configured skill root has no parent")?;
    let resolved_parent = std::fs::canonicalize(parent).map_err(|error| error.to_string())?;
    let resolved_quarantine = resolved_parent.join(".skill-studio-removing").join(
        intent
            .quarantine_path()
            .file_name()
            .ok_or("Quarantine name is missing")?,
    );
    for configured in roots {
        if intent.quarantine_path().starts_with(configured)
            || configured.starts_with(intent.quarantine_path())
        {
            return Err("Copy removal quarantine overlaps a configured skill root".into());
        }
        match std::fs::canonicalize(configured) {
            Ok(resolved)
                if resolved_quarantine.starts_with(&resolved)
                    || resolved.starts_with(&resolved_quarantine) =>
            {
                return Err(
                    "Copy removal quarantine resolves inside a configured skill root".into(),
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Cannot resolve configured skill root {}: {error}",
                    configured.display()
                ))
            }
        }
    }
    Ok(())
}

pub(crate) struct AdmittedCopy {
    pub(crate) selected: CopyDeploymentRecord,
    pub(crate) registry: Vec<u8>,
    pub(crate) registry_path: PathBuf,
    pub(crate) source: BackupSource,
    pub(crate) source_device: u64,
    pub(crate) tree: crate::skill_backup_copy::BackupCopyReport,
    pub(crate) reader_descriptions: Vec<CopyRemovalReader>,
    pub(crate) reader_sources: Vec<BackupSource>,
    pub(crate) registry_values: BTreeMap<String, serde_json::Value>,
    pub(crate) trial_values: BTreeMap<String, serde_json::Value>,
}

/// Inspects the inventory selected by this lease. Retained source handles are
/// observations; later effects still require the caller's current write lease.
pub(crate) fn admit_copy_deployment(
    inventory: &crate::skill_service::InventoryRead,
    lease: &FinalizedWriteLease<'_>,
    request: &CopyRemovalRequest,
    limits: BackupCopyLimits,
    cancellation: CancellationToken,
) -> Result<AdmittedCopy, String> {
    let source_path = parse_deployment_id(&request.deployment_id)
        .ok_or("Invalid Copy removal deployment ID")?
        .lexical_path;
    let mut matches = inventory
        .skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .filter(|deployment| deployment.id == request.deployment_id);
    let deployment = matches.next().ok_or("Selected Copy deployment is absent")?;
    if matches.next().is_some()
        || deployment.owner_kind != LifecycleOwnerKind::Copy
        || deployment.owner_revision.as_deref() != Some(request.expected_owner_revision.as_str())
        || deployment.is_symlink
        || Path::new(&deployment.path) != source_path
    {
        return Err("Copy removal ownership is ambiguous, unsupported or changed".into());
    }
    let registry_path = inventory.scope.home.join(".agents/skill-studio.json");
    let registry = lease
        .read(&registry_path, 8 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    let document: serde_json::Value =
        serde_json::from_slice(&registry).map_err(|error| error.to_string())?;
    let copies = document
        .get("copies")
        .and_then(serde_json::Value::as_object)
        .ok_or("Copy registry records are missing or invalid")?;
    let selected_raw = copies
        .get(&request.deployment_id)
        .cloned()
        .ok_or("Selected Copy registry record is absent")?;
    let selected: CopyDeploymentRecord =
        serde_json::from_value(selected_raw.clone()).map_err(|error| error.to_string())?;
    if selected.path != source_path
        || RegistryOwnerRecord::Copy(&selected).revision().as_deref()
            != Some(request.expected_owner_revision.as_str())
        || selected.content_hash != deployment.content_hash
    {
        return Err("Copy removal registry ownership changed".into());
    }
    let content_scope = SkillReadScope::bind(std::slice::from_ref(&source_path))
        .map_err(|error| error.to_string())?;
    let selected_resolved =
        std::fs::canonicalize(&source_path).map_err(|error| error.to_string())?;
    let live_hash = crate::skill_discovery::live_skill_content_hash_with_check(
        &content_scope,
        &source_path,
        || {
            if cancellation.is_cancelled() {
                Err("Copy removal cancelled".into())
            } else {
                Ok(())
            }
        },
    )?;
    if live_hash != selected.content_hash || live_hash.is_empty() {
        return Err("Copy removal source content changed".into());
    }
    let source = BackupSourceRoot::bind(source_path.parent().ok_or("Copy source has no parent")?)
        .map_err(|error| error.to_string())?
        .select(source_path.file_name().ok_or("Copy source has no name")?)
        .map_err(|error| error.to_string())?;
    let source_device = source
        .directory
        .symlink_metadata(&source.name)
        .map_err(|error| error.to_string())?
        .dev();
    let tree = inspect_entry(&source.directory, &source.name, limits, &cancellation)
        .map_err(|error| error.to_string())?;
    let mut reader_descriptions = Vec::new();
    let mut reader_sources = Vec::new();
    for reader in inventory.skills.iter().flat_map(|skill| &skill.deployments).filter(|candidate| {
        candidate.scope == deployment.scope
            && candidate.project_path == deployment.project_path
            && matches!(&candidate.backing, crate::skill_deployment::BackingRelationship::LinkedTo { deployment_id } if deployment_id == &deployment.id)
    }) {
        let path = PathBuf::from(&reader.path);
        let retained = BackupSourceRoot::bind(path.parent().ok_or("Copy reader has no parent")?)
            .map_err(|error| error.to_string())?
            .select(path.file_name().ok_or("Copy reader has no name")?)
            .map_err(|error| error.to_string())?;
        let raw_target = retained.exact_symlink_target().map_err(|error| error.to_string())?
            .ok_or("Copy reader link is absent")?;
        if std::fs::canonicalize(&path).map_err(|error| error.to_string())? != selected_resolved {
            return Err("Copy reader no longer points to the selected deployment".into());
        }
        reader_descriptions.push(CopyRemovalReader {
            deployment_id: reader.id.clone(),
            path,
            raw_target,
            registry_value: copies.get(&reader.id).cloned(),
        });
        reader_sources.push(retained);
    }
    let mut registry_values = BTreeMap::from([(request.deployment_id.clone(), selected_raw)]);
    for reader in &reader_descriptions {
        if let Some(value) = &reader.registry_value {
            registry_values.insert(reader.deployment_id.clone(), value.clone());
        }
    }
    let trial_values = document
        .get("trials")
        .and_then(serde_json::Value::as_object)
        .map(|trials| {
            trials
                .iter()
                .filter(|(_, value)| raw_trial_relates(value, &selected, &reader_descriptions))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();
    Ok(AdmittedCopy {
        selected,
        registry,
        registry_path,
        source,
        source_device,
        tree,
        reader_descriptions,
        reader_sources,
        registry_values,
        trial_values,
    })
}

impl ScopedSkillService {
    fn prepare_copy_removal(
        &mut self,
        store: &EventStore,
        event_id: &str,
        request: &CopyRemovalRequest,
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedCopyRemoval<'_>, String> {
        let selected_id = parse_deployment_id(&request.deployment_id)
            .ok_or("Invalid Copy removal deployment ID")?;
        let service_scope = self.scope();
        let roots = configured_roots(&service_scope);
        let source_path = selected_id.lexical_path.clone();
        let root = roots
            .iter()
            .find(|root| {
                source_path == root.join(&selected_id.name)
                    || source_path == root.join(".skill-studio-disabled").join(&selected_id.name)
            })
            .ok_or("Selected Copy is outside configured skill roots")?;
        let root_parent = root
            .parent()
            .ok_or("Configured skill root has no parent")?
            .to_path_buf();
        let holding_path = root_parent.join(".skill-studio-removing");
        let quarantine_path = holding_path.join(event_id);
        let mut entries = vec![
            source_path.clone(),
            holding_path.clone(),
            quarantine_path.clone(),
        ];
        entries.extend(roots.iter().map(|root| root.join(&selected_id.name)));
        let (inventory, lease) = self
            .prepare_write_inventory_with_entries(
                Some(&BTreeSet::from([selected_id.name.clone()])),
                &[
                    store.app_data.clone(),
                    source_path.clone(),
                    root_parent.clone(),
                ],
                &entries,
                timeout,
                cancellation.clone(),
            )
            .map_err(|error| error.to_string())?;
        let AdmittedCopy {
            selected,
            registry,
            registry_path,
            source,
            source_device,
            tree,
            reader_descriptions,
            reader_sources,
            registry_values,
            trial_values,
        } = admit_copy_deployment(&inventory, &lease, request, limits, cancellation.clone())?;
        let reader_count = reader_descriptions.len();
        let intent = CopyRemovalIntent {
            version: 1,
            selected,
            readers: reader_descriptions,
            expected_tree: tree.tree_identity,
            registry_path,
            quarantine_path,
            configured_roots: roots,
            registry_values,
            trial_values,
        };
        validate_quarantine_scope(&intent, &configured_roots(&service_scope))?;
        let holding = BackupSourceRoot::bind(&root_parent)
            .map_err(|error| error.to_string())?
            .select(std::ffi::OsStr::new(".skill-studio-removing"))
            .map_err(|error| error.to_string())?;
        let root_parent_device = holding
            .directory
            .dir_metadata()
            .map_err(|error| error.to_string())?
            .dev();
        let holding_device = holding
            .private_directory_device()
            .map_err(|error| error.to_string())?;
        let destination_device = holding_device.unwrap_or(root_parent_device);
        if source_device != destination_device {
            return Err("Copy removal quarantine is on a different filesystem".into());
        }
        if reader_sources.iter().any(|reader| {
            reader
                .directory
                .dir_metadata()
                .map(|metadata| metadata.dev() != destination_device)
                .unwrap_or(true)
        }) {
            return Err("Copy removal reader staging requires the selected filesystem".into());
        }
        let quarantine = holding_device
            .is_some()
            .then(|| {
                BackupSourceRoot::bind(&holding_path)
                    .and_then(|root| root.select(event_id.as_ref()))
                    .map_err(|error| error.to_string())
            })
            .transpose()?;
        let reader_stages = if quarantine.is_some() {
            let stage_root =
                BackupSourceRoot::bind(&holding_path).map_err(|error| error.to_string())?;
            (0..reader_count)
                .map(|index| {
                    stage_root
                        .select(format!("{event_id}-reader-{index}").as_ref())
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        Ok(PreparedCopyRemoval {
            forward_ownership_matches: true,
            intent,
            registry,
            source,
            quarantine,
            holding,
            readers: reader_sources,
            reader_stages,
            lease,
            cancellation,
        })
    }
}

fn require_pending(
    store: &EventStore,
    lease: &FinalizedWriteLease<'_>,
    id: &str,
    intent: &CopyRemovalIntent,
) -> Result<EventRow, String> {
    let row = GuardedEventStore::bind(store, lease)?
        .next_recovery_event(lease)?
        .ok_or("Copy removal event is no longer pending")?;
    if row.id != id || CopyRemovalIntent::from_event(&row)? != *intent {
        return Err("Copy removal event changed or is out of order".into());
    }
    Ok(row)
}

fn inspect_tree(
    source: &BackupSource,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<Option<String>, String> {
    match source.directory.symlink_metadata(&source.name) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            inspect_entry(&source.directory, &source.name, limits, cancellation)
                .map(|report| Some(report.tree_identity))
                .map_err(|error| error.to_string())
        }
        Ok(_) => Err("Copy removal tree path is no longer an independent directory".into()),
    }
}

fn prepare_effects<'scope>(
    service: &'scope mut ScopedSkillService,
    store: &EventStore,
    intent: &CopyRemovalIntent,
    _limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<PreparedCopyRemoval<'scope>, String> {
    let roots = configured_roots(&service.scope());
    validate_quarantine_scope(intent, &roots)?;
    if intent.registry_path() != service.scope().home.join(".agents/skill-studio.json") {
        return Err("Copy removal registry is outside the current home".into());
    }
    let holding_path = intent
        .quarantine_path()
        .parent()
        .ok_or("Quarantine parent is missing")?
        .to_path_buf();
    let root_parent = holding_path
        .parent()
        .ok_or("Quarantine root parent is missing")?;
    let mut entries = vec![
        intent.selected.path.clone(),
        holding_path.clone(),
        intent.quarantine_path.clone(),
    ];
    entries.extend(intent.readers.iter().map(|reader| reader.path.clone()));
    entries.extend(intent.readers.iter().enumerate().map(|(index, _)| {
        holding_path.join(format!(
            "{}-reader-{index}",
            intent
                .quarantine_path
                .file_name()
                .unwrap()
                .to_string_lossy()
        ))
    }));
    let active_tree = match std::fs::symlink_metadata(&intent.selected.path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            intent.selected.path.clone()
        }
        Ok(_) => return Err("Copy removal source is not an independent directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::symlink_metadata(intent.quarantine_path()) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    intent.quarantine_path().to_path_buf()
                }
                Ok(_) => {
                    return Err("Copy removal quarantine is not an independent directory".into())
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    root_parent.to_path_buf()
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        Err(error) => return Err(error.to_string()),
    };
    let source_present = active_tree == intent.selected.path;
    let (inventory, lease) = service
        .prepare_write_inventory_with_entries(
            Some(&BTreeSet::from([intent.selected.name.clone()])),
            &[
                store.app_data.clone(),
                active_tree,
                root_parent.to_path_buf(),
            ],
            &entries,
            timeout,
            CancellationToken::default(),
        )
        .map_err(|error| error.to_string())?;
    let mut selected = inventory
        .skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .filter(|deployment| deployment.id == intent.selected.deployment_id);
    let forward_ownership_matches = match selected.next() {
        Some(deployment) => {
            selected.next().is_none()
                && deployment.owner_kind == LifecycleOwnerKind::Copy
                && deployment.owner_revision
                    == RegistryOwnerRecord::Copy(&intent.selected).revision()
                && !deployment.is_symlink
        }
        None => !source_present,
    };
    drop(inventory);
    let source = BackupSourceRoot::bind(
        intent
            .selected
            .path
            .parent()
            .ok_or("Copy source parent is missing")?,
    )
    .map_err(|error| error.to_string())?
    .select(
        intent
            .selected
            .path
            .file_name()
            .ok_or("Copy source name is missing")?,
    )
    .map_err(|error| error.to_string())?;
    let holding = BackupSourceRoot::bind(root_parent)
        .map_err(|error| error.to_string())?
        .select(std::ffi::OsStr::new(".skill-studio-removing"))
        .map_err(|error| error.to_string())?;
    let root_parent_device = holding
        .directory
        .dir_metadata()
        .map_err(|error| error.to_string())?
        .dev();
    let holding_device = holding
        .private_directory_device()
        .map_err(|error| error.to_string())?;
    if holding_device.is_some_and(|device| device != root_parent_device) {
        return Err("Copy removal quarantine is on a different filesystem".into());
    }
    let destination_device = holding_device.unwrap_or(root_parent_device);
    let quarantine = holding_device
        .is_some()
        .then(|| {
            BackupSourceRoot::bind(&holding_path)
                .and_then(|root| root.select(intent.quarantine_path.file_name().unwrap()))
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    let mut readers = Vec::new();
    let mut reader_stages = Vec::new();
    for (index, reader) in intent.readers.iter().enumerate() {
        readers.push(
            BackupSourceRoot::bind(
                reader
                    .path
                    .parent()
                    .ok_or("Copy reader parent is missing")?,
            )
            .map_err(|error| error.to_string())?
            .select(
                reader
                    .path
                    .file_name()
                    .ok_or("Copy reader name is missing")?,
            )
            .map_err(|error| error.to_string())?,
        );
        if quarantine.is_some() {
            reader_stages.push(
                BackupSourceRoot::bind(&holding_path)
                    .map_err(|error| error.to_string())?
                    .select(
                        format!(
                            "{}-reader-{index}",
                            intent
                                .quarantine_path
                                .file_name()
                                .unwrap()
                                .to_string_lossy()
                        )
                        .as_ref(),
                    )
                    .map_err(|error| error.to_string())?,
            );
        }
    }
    if readers.iter().any(|reader| {
        reader
            .directory
            .dir_metadata()
            .map(|metadata| metadata.dev() != destination_device)
            .unwrap_or(true)
    }) {
        return Err("Copy removal reader staging requires the selected filesystem".into());
    }
    let registry = lease
        .read(intent.registry_path(), 8 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    Ok(PreparedCopyRemoval {
        forward_ownership_matches,
        intent: intent.clone(),
        registry,
        source,
        quarantine,
        holding,
        readers,
        reader_stages,
        lease,
        cancellation: CancellationToken::default(),
    })
}

fn finish(
    store: &EventStore,
    prepared: &PreparedCopyRemoval<'_>,
    row: &EventRow,
    status: EventStatus,
) -> Result<(), String> {
    require_pending(store, &prepared.lease, &row.id, &prepared.intent)?;
    GuardedEventStore::bind(store, &prepared.lease)?
        .finish_recovery_snapshot(&prepared.lease, row, status, None)
        .map_err(|error| error.to_string())
}

fn finish_if_published(
    service: &ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    intent: &CopyRemovalIntent,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<bool, String> {
    use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
    let roots = configured_roots(&service.scope());
    validate_quarantine_scope(intent, &roots)?;
    if intent.registry_path() != service.scope().home.join(".agents/skill-studio.json") {
        return Err("Copy removal registry is outside the current home".into());
    }
    let registry_parent = intent
        .registry_path()
        .parent()
        .ok_or("Registry parent is missing")?;
    let registry_scope =
        SkillReadScope::bind(&[store.app_data.clone(), registry_parent.to_path_buf()])
            .map_err(|error| error.to_string())?;
    let registry_effects = vec![
        DirectoryEffect::tree(&store.app_data, CoordinationMode::Exclusive),
        DirectoryEffect::entry(intent.registry_path(), CoordinationMode::Exclusive),
    ];
    let registry_lease = CoordinationPlan::new(registry_effects, timeout)
        .map_err(|error| error.to_string())?
        .acquire()
        .map_err(|error| error.to_string())?
        .finalize_write(&registry_scope, &[intent.registry_path().to_path_buf()])
        .map_err(|error| error.to_string())?;
    let registry = registry_lease
        .read(intent.registry_path(), 8 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    if intent.apply_registry_document(&registry)? != registry {
        return Ok(false);
    }
    drop(registry_lease);

    let holding = intent
        .quarantine_path()
        .parent()
        .ok_or("Quarantine parent is missing")?;
    let root_parent = holding
        .parent()
        .ok_or("Quarantine root parent is missing")?;
    let holding_source = BackupSourceRoot::bind(root_parent)
        .map_err(|error| error.to_string())?
        .select(std::ffi::OsStr::new(".skill-studio-removing"))
        .map_err(|error| error.to_string())?;
    let root_parent_device = holding_source
        .directory
        .dir_metadata()
        .map_err(|error| error.to_string())?
        .dev();
    match holding_source
        .private_directory_device()
        .map_err(|error| error.to_string())?
    {
        Some(device) if device == root_parent_device => {}
        Some(_) => return Err("Copy removal quarantine is on a different filesystem".into()),
        None => return Ok(false),
    }
    let read_scope = SkillReadScope::bind(&[
        store.app_data.clone(),
        registry_parent.to_path_buf(),
        holding.to_path_buf(),
    ])
    .map_err(|error| error.to_string())?;
    let effects = vec![
        DirectoryEffect::tree(&store.app_data, CoordinationMode::Exclusive),
        DirectoryEffect::tree(holding, CoordinationMode::Exclusive),
        DirectoryEffect::tree(intent.quarantine_path(), CoordinationMode::Exclusive),
        DirectoryEffect::entry(intent.quarantine_path(), CoordinationMode::Exclusive),
        DirectoryEffect::entry(intent.registry_path(), CoordinationMode::Exclusive),
    ];
    let lease = CoordinationPlan::new(effects, timeout)
        .map_err(|error| error.to_string())?
        .acquire()
        .map_err(|error| error.to_string())?
        .finalize_write(&read_scope, &[intent.registry_path().to_path_buf()])
        .map_err(|error| error.to_string())?;
    match holding_source
        .private_directory_device()
        .map_err(|error| error.to_string())?
    {
        Some(device) if device == root_parent_device => {}
        Some(_) => return Err("Copy removal quarantine is on a different filesystem".into()),
        None => return Err("Published Copy removal holding directory is missing".into()),
    }
    let registry = lease
        .read(intent.registry_path(), 8 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    if intent.apply_registry_document(&registry)? != registry {
        return Err("Copy removal publication changed during settlement".into());
    }
    let quarantine = BackupSourceRoot::bind(holding)
        .map_err(|error| error.to_string())?
        .select(
            intent
                .quarantine_path()
                .file_name()
                .ok_or("Quarantine name is missing")?,
        )
        .map_err(|error| error.to_string())?;
    if inspect_tree(&quarantine, limits, &CancellationToken::default())?.as_deref()
        != Some(intent.expected_tree())
    {
        return Err("Copy removal quarantine changed after publication".into());
    }
    let pending = require_pending(store, &lease, &row.id, intent)?;
    GuardedEventStore::bind(store, &lease)?
        .finish_recovery_snapshot(&lease, &pending, EventStatus::Done, None)
        .map_err(|error| error.to_string())?;
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForwardCheckpoint {
    HoldingCreated,
    ReaderMoved,
    TreeMoved,
}

fn prepare_forward_effects<'scope>(
    service: &'scope mut ScopedSkillService,
    store: &EventStore,
    intent: &CopyRemovalIntent,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<PreparedCopyRemoval<'scope>, String> {
    let prepared = prepare_effects(service, store, intent, limits, timeout)?;
    if !prepared.forward_ownership_matches {
        return Err("Copy removal ownership changed after admission".into());
    }
    Ok(prepared)
}

fn execute_forward_with_checkpoint(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    mut checkpoint: impl FnMut(ForwardCheckpoint) -> Result<(), String>,
) -> Result<(), String> {
    let intent = CopyRemovalIntent::from_event(row)?;
    let mut prepared = prepare_forward_effects(service, store, &intent, limits, timeout)?;
    if intent.apply_registry_document(&prepared.registry)? == prepared.registry {
        let quarantine = prepared
            .quarantine
            .as_ref()
            .ok_or("Published Copy removal quarantine is missing")?;
        if inspect_tree(quarantine, limits, &prepared.cancellation)?.as_deref()
            != Some(intent.expected_tree())
        {
            return Err("Copy removal quarantine changed after publication".into());
        }
        return finish(store, &prepared, row, EventStatus::Done);
    }
    if prepared.quarantine.is_none() {
        prepared
            .holding
            .create_directory()
            .map_err(|error| error.to_string())?;
        drop(prepared);
        checkpoint(ForwardCheckpoint::HoldingCreated)?;
        prepared = prepare_forward_effects(service, store, &intent, limits, timeout)?;
    }
    if prepared.quarantine.is_none() {
        return Err("Copy removal quarantine is unavailable".into());
    }
    for index in 0..intent.readers.len() {
        let reader = &prepared.readers[index];
        let stage = &prepared.reader_stages[index];
        match (
            reader
                .exact_symlink_target()
                .map_err(|error| error.to_string())?,
            stage
                .exact_symlink_target()
                .map_err(|error| error.to_string())?,
        ) {
            (Some(actual), None) if actual == intent.readers[index].raw_target => {
                prepared
                    .lease
                    .validate_entry_move(&intent.readers[index].path, &stage.original_path)?;
                reader
                    .move_exact_symlink_to(stage, &intent.readers[index].raw_target)
                    .map_err(|error| error.to_string())?;
                drop(prepared);
                checkpoint(ForwardCheckpoint::ReaderMoved)?;
                prepared = prepare_forward_effects(service, store, &intent, limits, timeout)?;
            }
            (None, Some(actual)) if actual == intent.readers[index].raw_target => {}
            _ => return Err("Copy removal reader changed or is ambiguously staged".into()),
        }
    }
    let quarantine = prepared.quarantine.as_ref().unwrap();
    match (
        inspect_tree(&prepared.source, limits, &prepared.cancellation)?,
        inspect_tree(quarantine, limits, &prepared.cancellation)?,
    ) {
        (Some(tree), None) if tree == intent.expected_tree() => {
            require_pending(store, &prepared.lease, &row.id, &intent)?;
            prepared
                .source
                .move_verified_tree(
                    quarantine,
                    intent.expected_tree(),
                    prepared.lease,
                    limits,
                    &prepared.cancellation,
                )
                .map_err(|error| match error {
                    crate::skill_tree_move::TreeMoveFailure::BeforeMove(message)
                    | crate::skill_tree_move::TreeMoveFailure::MayHaveMoved(message) => message,
                })?;
            checkpoint(ForwardCheckpoint::TreeMoved)?;
            prepared = prepare_forward_effects(service, store, &intent, limits, timeout)?;
        }
        (None, Some(tree)) if tree == intent.expected_tree() => {}
        _ => return Err("Copy removal source or quarantine changed".into()),
    }
    require_pending(store, &prepared.lease, &row.id, &intent)?;
    let quarantine = prepared
        .quarantine
        .as_ref()
        .ok_or("Copy removal quarantine is missing")?;
    if inspect_tree(&prepared.source, limits, &prepared.cancellation)?.is_some()
        || inspect_tree(quarantine, limits, &prepared.cancellation)?.as_deref()
            != Some(intent.expected_tree())
    {
        return Err("Copy removal source or quarantine changed before publication".into());
    }
    for (index, reader) in prepared.readers.iter().enumerate() {
        if reader
            .exact_symlink_target()
            .map_err(|error| error.to_string())?
            .is_some()
            || prepared.reader_stages[index]
                .exact_symlink_target()
                .map_err(|error| error.to_string())?
                .as_ref()
                != Some(&intent.readers[index].raw_target)
        {
            return Err("Copy removal reader changed before publication".into());
        }
    }
    let target = crate::skill_document_target::SkillRegistryTarget::bind(
        intent
            .registry_path()
            .parent()
            .ok_or("Copy registry parent is missing")?,
    )?;
    let proposed = intent.apply_registry_document(&prepared.registry)?;
    target
        .replace(&mut prepared.lease, &prepared.registry, &proposed)
        .map_err(|error| error.to_string())?;
    finish(store, &prepared, row, EventStatus::Done)
}

pub fn recover_copy_removal(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<bool, String> {
    let intent = CopyRemovalIntent::from_event(row)?;
    if finish_if_published(service, store, row, &intent, limits, timeout)? {
        return Ok(true);
    }
    let prepared = prepare_effects(service, store, &intent, limits, timeout)?;
    let proposed = intent.apply_registry_document(&prepared.registry)?;
    if proposed == prepared.registry {
        drop(prepared);
        return finish_if_published(service, store, row, &intent, limits, timeout).and_then(
            |published| {
                published
                    .then_some(true)
                    .ok_or_else(|| "Copy removal publication changed during recovery".into())
            },
        );
    }
    let mut errors = Vec::new();
    let tree_state = prepared.quarantine.as_ref().map(|quarantine| {
        (
            inspect_tree(&prepared.source, limits, &prepared.cancellation),
            inspect_tree(quarantine, limits, &prepared.cancellation),
        )
    });
    match tree_state {
        Some((Ok(None), Ok(Some(tree)))) if tree == intent.expected_tree() => {
            let quarantine = prepared.quarantine.as_ref().unwrap();
            if let Err(error) = quarantine.move_verified_tree(
                &prepared.source,
                intent.expected_tree(),
                prepared.lease,
                limits,
                &prepared.cancellation,
            ) {
                errors.push(match error {
                    crate::skill_tree_move::TreeMoveFailure::BeforeMove(message)
                    | crate::skill_tree_move::TreeMoveFailure::MayHaveMoved(message) => message,
                });
            }
        }
        Some((Ok(Some(tree)), Ok(None))) if tree == intent.expected_tree() => drop(prepared),
        Some((Ok(source), Ok(quarantine))) => {
            drop(prepared);
            errors.push(format!(
                "Copy removal rollback paths changed: source={source:?}, quarantine={quarantine:?}"
            ));
        }
        Some((Err(error), _)) | Some((_, Err(error))) => {
            drop(prepared);
            errors.push(error);
        }
        None => {
            if inspect_tree(&prepared.source, limits, &prepared.cancellation)?.as_deref()
                != Some(intent.expected_tree())
            {
                errors.push("Copy removal source changed before quarantine creation".into());
            }
            drop(prepared);
        }
    }
    let mut prepared = prepare_effects(service, store, &intent, limits, timeout)?;
    for index in 0..intent.readers.len() {
        let restored = match prepared.reader_stages.get(index) {
            Some(stage) => {
                prepared
                    .lease
                    .validate_entry_move(&stage.original_path, &intent.readers[index].path)?;
                prepared.readers[index]
                    .restore_exact_symlink_from(stage, &intent.readers[index].raw_target)
            }
            None => {
                prepared.lease.validate_entry_move(
                    &intent.readers[index].path,
                    &intent.readers[index].path,
                )?;
                prepared.readers[index].restore_absent_symlink(&intent.readers[index].raw_target)
            }
        };
        if let Err(error) = restored {
            errors.push(error.to_string());
        }
        drop(prepared);
        prepared = prepare_effects(service, store, &intent, limits, timeout)?;
    }
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    finish(store, &prepared, row, EventStatus::Failed)?;
    Ok(false)
}

fn removal_outcome(id: String, intent: &CopyRemovalIntent) -> CopyRemovalOutcome {
    CopyRemovalOutcome {
        event_id: id,
        removed_deployment_ids: std::iter::once(intent.selected.deployment_id.clone())
            .chain(
                intent
                    .readers
                    .iter()
                    .map(|reader| reader.deployment_id.clone()),
            )
            .collect(),
    }
}

struct RemovalCheckpoints<Forward, After> {
    forward: Forward,
    after_forward_failure: After,
}

fn remove_copy_deployment_with_checkpoints(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &CopyRemovalRequest,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
    checkpoints: RemovalCheckpoints<
        impl FnMut(ForwardCheckpoint) -> Result<(), String>,
        impl FnMut(),
    >,
) -> Result<CopyRemovalOutcome, CopyRemovalError> {
    let RemovalCheckpoints {
        forward,
        mut after_forward_failure,
    } = checkpoints;
    let before = |message| CopyRemovalError {
        event_id: None,
        recovery_required: false,
        message,
    };
    let id = crate::skill_event_store::allocate_id();
    let prepared = service
        .prepare_copy_removal(store, &id, request, limits, timeout, cancellation)
        .map_err(before)?;
    let intent = prepared.intent.clone();
    GuardedEventStore::bind(store, &prepared.lease)
        .map_err(before)?
        .record_pending(&prepared.lease, &id, intent.event_draft().map_err(before)?)
        .map_err(|error| CopyRemovalError {
            event_id: matches!(error, EventWriteFailure::MayHaveWritten(_)).then(|| id.clone()),
            recovery_required: matches!(error, EventWriteFailure::MayHaveWritten(_)),
            message: error.to_string(),
        })?;
    drop(prepared);
    let row = store
        .get(&id)
        .map_err(|message| CopyRemovalError {
            event_id: Some(id.clone()),
            recovery_required: true,
            message,
        })?
        .ok_or_else(|| CopyRemovalError {
            event_id: Some(id.clone()),
            recovery_required: true,
            message: "Recorded Copy removal event is missing".into(),
        })?;
    match execute_forward_with_checkpoint(service, store, &row, limits, timeout, forward) {
        Ok(()) => Ok(removal_outcome(id, &intent)),
        Err(message) => {
            after_forward_failure();
            match recover_copy_removal(service, store, &row, limits, timeout) {
                Ok(true) => Ok(removal_outcome(id, &intent)),
                Ok(false) => Err(CopyRemovalError {
                    event_id: Some(id),
                    recovery_required: false,
                    message,
                }),
                Err(recovery) => Err(CopyRemovalError {
                    event_id: Some(id),
                    recovery_required: true,
                    message: format!("{message}; removal requires recovery: {recovery}"),
                }),
            }
        }
    }
}

pub fn remove_copy_deployment(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &CopyRemovalRequest,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<CopyRemovalOutcome, CopyRemovalError> {
    remove_copy_deployment_with_checkpoints(
        service,
        store,
        request,
        limits,
        timeout,
        cancellation,
        RemovalCheckpoints {
            forward: |_| Ok(()),
            after_forward_failure: || {},
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_deployment::deployment_id;
    use std::{fs, os::unix::fs::PermissionsExt as _};

    fn intent() -> CopyRemovalIntent {
        let path = PathBuf::from("/fixture/.cursor/skills/sample");
        let record = CopyDeploymentRecord {
            deployment_id: deployment_id(
                "sample",
                "global",
                SkillDestination::PerHarness,
                "cursor",
                None,
                &path,
            ),
            name: "sample".into(),
            path,
            scope: InstallScope::Global,
            destination: SkillDestination::PerHarness,
            slot: "cursor".into(),
            project_path: None,
            content_hash: "a".repeat(64),
            disabled: false,
        };
        let value = serde_json::to_value(&record).unwrap();
        CopyRemovalIntent {
            version: 1,
            selected: record.clone(),
            readers: vec![],
            expected_tree: format!("tree-v1:{}", "b".repeat(64)),
            registry_path: PathBuf::from("/fixture/.agents/skill-studio.json"),
            quarantine_path: PathBuf::from("/fixture/.cursor/.skill-studio-removing/event-1"),
            configured_roots: vec![PathBuf::from("/fixture/.cursor/skills")],
            registry_values: BTreeMap::from([(record.deployment_id.clone(), value)]),
            trial_values: BTreeMap::new(),
        }
    }

    #[test]
    fn raw_transition_refuses_drift_and_preserves_unknown_values() {
        let intent = intent();
        let selected = intent
            .registry_values
            .get(&intent.selected.deployment_id)
            .unwrap()
            .clone();
        let original = serde_json::json!({"future": {"keep": true}, "copies": {intent.selected.deployment_id.clone(): selected, "other": {"unknown": 1}}, "trials": {"other": {"keep": true}}});
        let bytes = serde_json::to_vec(&original).unwrap();
        let removed: serde_json::Value =
            serde_json::from_slice(&intent.apply_registry_document(&bytes).unwrap()).unwrap();
        assert_eq!(removed["future"], original["future"]);
        assert_eq!(removed["copies"]["other"], original["copies"]["other"]);
        let mut drifted = original;
        drifted["copies"][&intent.selected.deployment_id]["future_field"] =
            serde_json::json!("changed");
        assert!(intent
            .apply_registry_document(&serde_json::to_vec(&drifted).unwrap())
            .is_err());
    }

    #[test]
    fn event_binding_and_published_replacements_are_not_old_ownership() {
        let intent = intent();
        let row = EventRow {
            id: "event-1".into(),
            ts: "now".into(),
            kind: EVENT_KIND.into(),
            skill: "sample".into(),
            harness: Some("cursor".into()),
            scope: Some("global".into()),
            project_path: None,
            payload: serde_json::to_value(&intent).unwrap(),
            inverse: None,
            backup_dir: None,
            status: "pending".into(),
            reverted_by: None,
            restorable: false,
        };
        assert!(CopyRemovalIntent::from_event(&row).is_ok());
        let mut bad = row.clone();
        bad.scope = Some("project".into());
        assert!(CopyRemovalIntent::from_event(&bad).is_err());
        let published = intent.apply_registry_document(&serde_json::to_vec(&serde_json::json!({"copies": {intent.selected.deployment_id.clone(): intent.registry_values.get(&intent.selected.deployment_id).unwrap()}})).unwrap()).unwrap();
        assert_eq!(
            intent
                .observe(
                    Some("replacement"),
                    Some(intent.expected_tree()),
                    &[],
                    &published
                )
                .unwrap(),
            CopyRemovalObservedState::Published
        );
    }

    #[test]
    fn registered_reader_records_have_one_exact_repeatable_raw_transition() {
        let selected_path = PathBuf::from("/fixture/.agents/skills/sample");
        let selected = CopyDeploymentRecord {
            deployment_id: deployment_id(
                "sample",
                "global",
                SkillDestination::Universal,
                "universal",
                None,
                &selected_path,
            ),
            name: "sample".into(),
            path: selected_path,
            scope: InstallScope::Global,
            destination: SkillDestination::Universal,
            slot: "universal".into(),
            project_path: None,
            content_hash: "a".repeat(64),
            disabled: false,
        };
        let reader_path = PathBuf::from("/fixture/.claude/skills/sample");
        let reader_id = deployment_id(
            "sample",
            "global",
            SkillDestination::Universal,
            "claude-code",
            None,
            &reader_path,
        );
        let reader_record = CopyDeploymentRecord {
            deployment_id: reader_id.clone(),
            name: "sample".into(),
            path: reader_path.clone(),
            scope: InstallScope::Global,
            destination: SkillDestination::Universal,
            slot: "claude-code".into(),
            project_path: None,
            content_hash: "a".repeat(64),
            disabled: false,
        };
        let raw = serde_json::to_value(&reader_record).unwrap();
        let selected_raw = serde_json::to_value(&selected).unwrap();
        let intent = CopyRemovalIntent {
            version: 1,
            selected: selected.clone(),
            readers: vec![CopyRemovalReader {
                deployment_id: reader_id.clone(),
                path: reader_path,
                raw_target: PathBuf::from("../../.agents/skills/sample"),
                registry_value: Some(raw.clone()),
            }],
            expected_tree: format!("tree-v1:{}", "b".repeat(64)),
            registry_path: PathBuf::from("/fixture/.agents/skill-studio.json"),
            quarantine_path: PathBuf::from("/fixture/.agents/.skill-studio-removing/event-1"),
            configured_roots: vec![PathBuf::from("/fixture/.agents/skills")],
            registry_values: BTreeMap::from([
                (selected.deployment_id.clone(), selected_raw),
                (reader_id.clone(), raw.clone()),
            ]),
            trial_values: BTreeMap::new(),
        };
        let original = serde_json::to_vec(&serde_json::json!({"copies": {
            intent.selected.deployment_id.clone(): intent.registry_values[&intent.selected.deployment_id].clone(),
            reader_id: raw
        }, "trials": {}})).unwrap();
        let removed = intent.apply_registry_document(&original).unwrap();
        assert!(
            serde_json::from_slice::<serde_json::Value>(&removed).unwrap()["copies"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        assert_eq!(intent.apply_registry_document(&removed).unwrap(), removed);
        let mut forged = intent;
        forged.readers[0].deployment_id = deployment_id(
            "other",
            "global",
            SkillDestination::Universal,
            "claude-code",
            None,
            &forged.readers[0].path,
        );
        assert!(forged.validate().is_err());
    }

    #[test]
    fn integrated_removal_retains_tree_and_published_recovery_ignores_new_source() {
        let temp = tempfile::tempdir().unwrap();
        let fixture_root = temp.path().canonicalize().unwrap();
        let home = fixture_root.join("home");
        let source = home.join(".agents/skills/sample");
        let reader = home.join(".claude/skills/sample");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir(home.join(".git")).unwrap();
        fs::create_dir_all(reader.parent().unwrap()).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: sample\ndescription: Fixture\n---\nBody\n",
        )
        .unwrap();
        std::os::unix::fs::symlink("../../.agents/skills/sample", &reader).unwrap();
        let scope = crate::skill_service::SkillScope {
            home: home.clone(),
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        };
        let mut service = ScopedSkillService::bind(scope.clone()).unwrap();
        let inventory = service.scan(None, Some(Duration::from_secs(5))).unwrap();
        let deployment = inventory
            .skills
            .iter()
            .flat_map(|skill| &skill.deployments)
            .find(|deployment| Path::new(&deployment.path) == source)
            .unwrap();
        let record = CopyDeploymentRecord {
            deployment_id: deployment.id.clone(),
            name: "sample".into(),
            path: source.clone(),
            scope: InstallScope::Global,
            destination: SkillDestination::Universal,
            slot: "universal".into(),
            project_path: None,
            content_hash: deployment.content_hash.clone(),
            disabled: false,
        };
        let registry_path = home.join(".agents/skill-studio.json");
        fs::write(&registry_path, serde_json::to_vec(&serde_json::json!({
            "version": 4, "future": {"keep": true}, "copies": {record.deployment_id.clone(): serde_json::to_value(&record).unwrap()},
            "trials": {}
        })).unwrap()).unwrap();
        let mut service = ScopedSkillService::bind(scope.clone()).unwrap();
        let inventory = service.scan(None, Some(Duration::from_secs(5))).unwrap();
        let owned = inventory
            .skills
            .iter()
            .flat_map(|skill| &skill.deployments)
            .find(|deployment| deployment.id == record.deployment_id)
            .unwrap();
        assert_eq!(owned.owner_kind, LifecycleOwnerKind::Copy, "{owned:#?}");
        let request = CopyRemovalRequest {
            deployment_id: owned.id.clone(),
            expected_owner_revision: owned.owner_revision.clone().unwrap(),
        };
        let event_data = fixture_root.join("events");
        let store = EventStore::open(&event_data).unwrap();
        let pending_id = crate::skill_event_store::allocate_id();
        let pending = service
            .prepare_copy_removal(
                &store,
                &pending_id,
                &request,
                BackupCopyLimits {
                    max_bytes: 1024 * 1024,
                    max_entries: 100,
                    max_depth: 8,
                },
                Some(Duration::from_secs(5)),
                CancellationToken::default(),
            )
            .unwrap();
        let pending_intent = pending.intent.clone();
        GuardedEventStore::bind(&store, &pending.lease)
            .unwrap()
            .record_pending(
                &pending.lease,
                &pending_id,
                pending_intent.event_draft().unwrap(),
            )
            .unwrap();
        drop(pending);
        let holding = home.join(".agents/.skill-studio-removing");
        fs::create_dir(&holding).unwrap();
        fs::set_permissions(&holding, fs::Permissions::from_mode(0o700)).unwrap();
        let pending_row = store.get(&pending_id).unwrap().unwrap();
        assert!(!recover_copy_removal(
            &mut service,
            &store,
            &pending_row,
            BackupCopyLimits {
                max_bytes: 1024 * 1024,
                max_entries: 100,
                max_depth: 8
            },
            Some(Duration::from_secs(5))
        )
        .unwrap());
        assert!(source.join("SKILL.md").is_file());
        assert_eq!(
            fs::read_link(&reader).unwrap(),
            PathBuf::from("../../.agents/skills/sample")
        );
        assert_eq!(store.get(&pending_id).unwrap().unwrap().status, "failed");

        let outcome = remove_copy_deployment(
            &mut service,
            &store,
            &request,
            BackupCopyLimits {
                max_bytes: 1024 * 1024,
                max_entries: 100,
                max_depth: 8,
            },
            Some(Duration::from_secs(5)),
            CancellationToken::default(),
        )
        .unwrap();
        assert!(!source.exists());
        assert!(fs::symlink_metadata(&reader).is_err());
        let quarantine = home
            .join(".agents/.skill-studio-removing")
            .join(&outcome.event_id);
        assert_eq!(
            fs::read_to_string(quarantine.join("SKILL.md")).unwrap(),
            "---\nname: sample\ndescription: Fixture\n---\nBody\n"
        );
        let registry: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
        assert!(registry["copies"].as_object().unwrap().is_empty());
        assert_eq!(registry["future"]["keep"], true);
        assert!(registry["trials"].as_object().unwrap().is_empty());

        fs::write(&source, "replacement").unwrap();
        std::os::unix::fs::symlink("replacement-target", &reader).unwrap();
        store
            .conn
            .execute(
                "UPDATE events SET status = 'interrupted' WHERE id = ?1",
                [&outcome.event_id],
            )
            .unwrap();
        let row = store.get(&outcome.event_id).unwrap().unwrap();
        let mut recovered = ScopedSkillService::bind(scope).unwrap();
        assert!(recover_copy_removal(
            &mut recovered,
            &store,
            &row,
            BackupCopyLimits {
                max_bytes: 1024 * 1024,
                max_entries: 100,
                max_depth: 8
            },
            Some(Duration::from_secs(5))
        )
        .unwrap());
        assert_eq!(fs::read_to_string(&source).unwrap(), "replacement");
        assert_eq!(
            fs::read_link(&reader).unwrap(),
            PathBuf::from("replacement-target")
        );
        assert_eq!(
            store.get(&outcome.event_id).unwrap().unwrap().status,
            "done"
        );
    }
}
