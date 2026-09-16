//! Durable, non-restorable removal for one canonical Fork deployment.
//!
//! The moved tree is deliberately retained under an event-id quarantine.  It
//! is recovery material, never an undo record.
use crate::{
    skill_backup_copy::{inspect_entry, BackupCopyLimits},
    skill_backup_source::{BackupSource, BackupSourceRoot},
    skill_coordination::{CancellationToken, FinalizedWriteLease},
    skill_deployment::{parse_deployment_id, SkillDestination},
    skill_event::{EventDraft, EventRow, EventStatus},
    skill_event_operations::{EventWriteFailure, GuardedEventStore},
    skill_event_store::EventStore,
    skill_fork_registry::{ForkRecord, RegistryOwnerRecord},
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

pub const EVENT_KIND: &str = "remove_fork_deployment";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForkRemovalIntent {
    version: u32,
    selected: ForkRecord,
    name: String,
    path: PathBuf,
    expected_tree: String,
    registry_path: PathBuf,
    quarantine_path: PathBuf,
    configured_roots: Vec<PathBuf>,
    raw_registry_value: serde_json::Value,
    #[serde(default)]
    raw_trial_values: BTreeMap<String, serde_json::Value>,
}

impl ForkRemovalIntent {
    fn validate(&self) -> Result<(), String> {
        let clean = |p: &Path| {
            p.is_absolute()
                && !p
                    .components()
                    .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
        };
        let parsed = parse_deployment_id(&self.selected.deployment_id)
            .ok_or("Invalid Fork removal deployment ID")?;
        if self.version != 1
            || parsed.scope != "global"
            || parsed.slot != "universal"
            || parsed.destination != SkillDestination::Universal
            || parsed.name != self.name
            || parsed.lexical_path != self.path
            || self.selected.skill_dir != self.path
            || !clean(&self.path)
            || !clean(&self.registry_path)
            || !clean(&self.quarantine_path)
            || self
                .registry_path
                .parent()
                .and_then(Path::file_name)
                .and_then(|n| n.to_str())
                != Some(".agents")
            || self.configured_roots.iter().any(|root| !clean(root))
            || !self
                .configured_roots
                .iter()
                .any(|root| self.path == root.join(&self.name))
            || self.configured_roots.iter().any(|root| {
                self.quarantine_path.starts_with(root) || root.starts_with(&self.quarantine_path)
            })
            || self
                .quarantine_path
                .file_name()
                .and_then(|n| n.to_str())
                .is_none()
            || self
                .expected_tree
                .strip_prefix("tree-v1:")
                .is_none_or(|hash| {
                    hash.len() != 64
                        || !hash
                            .bytes()
                            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                })
        {
            return Err("Invalid or unsupported Fork removal intent".into());
        }
        Ok(())
    }
    fn event_draft(&self) -> Result<EventDraft, String> {
        self.validate()?;
        Ok(EventDraft {
            kind: EVENT_KIND.into(),
            skill: self.name.clone(),
            harness: Some("universal".into()),
            scope: Some("global".into()),
            project_path: None,
            payload: serde_json::to_value(self).map_err(|e| e.to_string())?,
            inverse: None,
            backup_dir: None,
            restorable: false,
        })
    }
    fn from_event(row: &EventRow) -> Result<Self, String> {
        if row.kind != EVENT_KIND
            || row.restorable
            || row.inverse.is_some()
            || row.backup_dir.is_some()
        {
            return Err("Not a Fork removal event".into());
        }
        let intent: Self =
            serde_json::from_value(row.payload.clone()).map_err(|e| e.to_string())?;
        intent.validate()?;
        if row.skill != intent.name
            || row.harness.as_deref() != Some("universal")
            || row.scope.as_deref() != Some("global")
            || row.project_path.is_some()
            || intent.quarantine_path.file_name().and_then(|n| n.to_str()) != Some(row.id.as_str())
        {
            return Err("Fork removal event metadata does not match its intent".into());
        }
        Ok(intent)
    }
    fn apply_registry(&self, original: &[u8]) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut document: serde_json::Value =
            serde_json::from_slice(original).map_err(|e| e.to_string())?;
        let current_trials =
            matching_trial_values(&document, &self.selected.deployment_id, &self.path)?;
        let forks = document
            .get("forks")
            .and_then(serde_json::Value::as_object)
            .ok_or("Fork registry records are missing or invalid")?;
        match forks.get(&self.name) {
            None => {
                if !current_trials.is_empty() {
                    return Err("Fork removal publication is incomplete".into());
                }
                return Ok(original.to_vec());
            }
            Some(value) if value != &self.raw_registry_value => {
                return Err("Fork ownership changed".into())
            }
            _ => {}
        }
        if current_trials != self.raw_trial_values {
            return Err("Fork trial ownership changed".into());
        }
        document
            .as_object_mut()
            .and_then(|root| root.get_mut("forks"))
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("Fork registry records are missing or invalid")?
            .remove(&self.name);
        if let Some(trials) = document
            .as_object_mut()
            .and_then(|root| root.get_mut("trials"))
            .and_then(serde_json::Value::as_object_mut)
        {
            for key in self.raw_trial_values.keys() {
                trials.remove(key);
            }
        }
        serde_json::to_vec(&document).map_err(|e| e.to_string())
    }
}

fn matching_trial_values(
    document: &serde_json::Value,
    deployment_id: &str,
    path: &Path,
) -> Result<BTreeMap<String, serde_json::Value>, String> {
    let Some(value) = document.get("trials") else {
        return Ok(BTreeMap::new());
    };
    let trials = value.as_object().ok_or("Fork trial records are invalid")?;
    Ok(trials
        .iter()
        .filter(|(_, value)| {
            value
                .get("deployment_id")
                .and_then(serde_json::Value::as_str)
                == Some(deployment_id)
                || value.get("skill_dir").and_then(serde_json::Value::as_str)
                    == Some(path.to_string_lossy().as_ref())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkRemovalRequest {
    pub deployment_id: String,
    pub expected_owner_revision: String,
}
#[derive(Debug, PartialEq, Eq)]
pub struct ForkRemovalOutcome {
    pub event_id: String,
    pub removed_deployment_ids: Vec<String>,
}
#[derive(Debug)]
pub struct ForkRemovalError {
    pub event_id: Option<String>,
    pub recovery_required: bool,
    pub message: String,
}

fn roots(service: &ScopedSkillService) -> Vec<PathBuf> {
    let scope = service.scope();
    let mut roots = crate::skill_agents::skill_roots(&scope.home, &scope.projects)
        .into_iter()
        .filter(|root| root.label != "parked")
        .map(|root| root.path)
        .chain(scope.backing_roots)
        .chain(scope.plugin_ownership_roots)
        .collect::<Vec<_>>();
    roots.sort();
    roots.dedup();
    roots
}

fn validate_current_scope(
    service: &ScopedSkillService,
    store: &EventStore,
    event_id: &str,
    intent: &ForkRemovalIntent,
) -> Result<(PathBuf, PathBuf), String> {
    if !crate::skill_backup_reservation::valid_id(event_id) {
        return Err("Invalid Fork removal event ID".into());
    }
    intent.validate()?;
    let scope = service.scope();
    let current_roots = roots(service);
    if intent.configured_roots != current_roots {
        return Err("Fork removal configured roots changed".into());
    }
    if intent.registry_path != scope.home.join(".agents/skill-studio.json") {
        return Err("Fork removal registry is outside the current home".into());
    }
    let selected_root = current_roots
        .iter()
        .find(|root| intent.path == root.join(&intent.name))
        .ok_or("Fork removal is outside the current configured roots")?;
    let parent = selected_root
        .parent()
        .ok_or("Configured skill root has no parent")?
        .to_path_buf();
    let holding_path = parent.join(".skill-studio-removing");
    if intent.quarantine_path != holding_path.join(event_id) {
        return Err("Fork removal quarantine is not bound to the selected root".into());
    }
    if !store.app_data.is_absolute() {
        return Err("Fork removal event store is outside the current scope".into());
    }
    let resolved_parent = std::fs::canonicalize(&parent).map_err(|e| e.to_string())?;
    let resolved_quarantine = resolved_parent
        .join(".skill-studio-removing")
        .join(event_id);
    for root in &current_roots {
        if intent.quarantine_path.starts_with(root) || root.starts_with(&intent.quarantine_path) {
            return Err("Fork removal quarantine overlaps a configured skill root".into());
        }
        match std::fs::canonicalize(root) {
            Ok(resolved)
                if resolved_quarantine.starts_with(&resolved)
                    || resolved.starts_with(&resolved_quarantine) =>
            {
                return Err(
                    "Fork removal quarantine resolves inside a configured skill root".into(),
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Cannot resolve configured skill root {}: {error}",
                    root.display()
                ));
            }
        }
    }
    Ok((parent, holding_path))
}

struct Prepared<'a> {
    intent: ForkRemovalIntent,
    registry: Vec<u8>,
    source: BackupSource,
    quarantine: Option<BackupSource>,
    holding: BackupSource,
    lease: FinalizedWriteLease<'a>,
    cancellation: CancellationToken,
}

fn tree(
    source: &BackupSource,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<Option<String>, String> {
    match source.directory.symlink_metadata(&source.name) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
            inspect_entry(&source.directory, &source.name, limits, cancellation)
                .map(|report| Some(report.tree_identity))
                .map_err(|e| e.to_string())
        }
        Ok(_) => Err("Fork removal tree path is no longer an independent directory".into()),
    }
}

fn move_error(error: crate::skill_tree_move::TreeMoveFailure) -> String {
    match error {
        crate::skill_tree_move::TreeMoveFailure::BeforeMove(message)
        | crate::skill_tree_move::TreeMoveFailure::MayHaveMoved(message) => message,
    }
}

fn move_to_quarantine(prepared: Prepared<'_>, limits: BackupCopyLimits) -> Result<(), String> {
    let Prepared {
        intent,
        source,
        quarantine,
        lease,
        cancellation,
        ..
    } = prepared;
    let expected = intent.expected_tree;
    source
        .move_verified_tree(
            quarantine
                .as_ref()
                .ok_or("Fork removal quarantine is missing")?,
            &expected,
            lease,
            limits,
            &cancellation,
        )
        .map_err(move_error)
}

fn restore_from_quarantine(prepared: Prepared<'_>, limits: BackupCopyLimits) -> Result<(), String> {
    let Prepared {
        intent,
        source,
        quarantine,
        lease,
        cancellation,
        ..
    } = prepared;
    let expected = intent.expected_tree;
    quarantine
        .ok_or("Fork removal quarantine is missing")?
        .move_verified_tree(&source, &expected, lease, limits, &cancellation)
        .map_err(move_error)
}

#[allow(clippy::too_many_arguments)]
fn prepare<'a>(
    service: &'a mut ScopedSkillService,
    store: &EventStore,
    event_id: &str,
    request: Option<&ForkRemovalRequest>,
    intent: Option<&ForkRemovalIntent>,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<Prepared<'a>, String> {
    if !crate::skill_backup_reservation::valid_id(event_id) {
        return Err("Invalid Fork removal event ID".into());
    }
    let scope = service.scope();
    let current_roots = roots(service);
    let (name, path) = match (request, intent) {
        (Some(request), _) => {
            let parsed = parse_deployment_id(&request.deployment_id)
                .ok_or("Invalid Fork removal deployment ID")?;
            (parsed.name, parsed.lexical_path)
        }
        (_, Some(intent)) => (intent.name.clone(), intent.path.clone()),
        _ => return Err("Fork removal request is missing".into()),
    };
    let root = current_roots
        .iter()
        .find(|root| path == root.join(&name))
        .ok_or("Fork removal is outside the current Universal scope")?;
    let parent = root
        .parent()
        .ok_or("Universal skill root has no parent")?
        .to_path_buf();
    let holding_path = parent.join(".skill-studio-removing");
    let quarantine_path = holding_path.join(event_id);
    if let Some(intent) = intent {
        intent.validate()?;
        if intent.registry_path != scope.home.join(".agents/skill-studio.json")
            || intent.quarantine_path != quarantine_path
        {
            return Err("Fork removal intent is outside the current scope".into());
        }
    }
    let resolved_parent = std::fs::canonicalize(&parent).map_err(|e| e.to_string())?;
    let resolved_quarantine = resolved_parent
        .join(".skill-studio-removing")
        .join(event_id);
    for root in &current_roots {
        if quarantine_path.starts_with(root) || root.starts_with(&quarantine_path) {
            return Err("Fork removal quarantine overlaps a configured skill root".into());
        }
        match std::fs::canonicalize(root) {
            Ok(resolved)
                if resolved_quarantine.starts_with(&resolved)
                    || resolved.starts_with(&resolved_quarantine) =>
            {
                return Err(
                    "Fork removal quarantine resolves inside a configured skill root".into(),
                )
            }
            Ok(_) | Err(_) => {}
        }
    }
    let entries = vec![path.clone(), holding_path.clone(), quarantine_path.clone()];
    let (inventory, lease) = service
        .prepare_write_inventory_with_entries(
            Some(&BTreeSet::from([name.clone()])),
            &[store.app_data.clone(), path.clone(), parent.clone()],
            &entries,
            timeout,
            cancellation.clone(),
        )
        .map_err(|e| e.to_string())?;
    let registry_path = scope.home.join(".agents/skill-studio.json");
    let registry = lease
        .read(&registry_path, 8 * 1024 * 1024)
        .map_err(|e| e.to_string())?;
    let doc: serde_json::Value = serde_json::from_slice(&registry).map_err(|e| e.to_string())?;
    let raw_now = doc
        .get("forks")
        .and_then(serde_json::Value::as_object)
        .and_then(|forks| forks.get(&name))
        .cloned();
    let (record, raw, live_hash) = if let Some(durable) = intent {
        if let Some(actual) = raw_now {
            if actual != durable.raw_registry_value {
                return Err("Fork ownership changed".into());
            }
        }
        (
            durable.selected.clone(),
            durable.raw_registry_value.clone(),
            durable
                .expected_tree
                .strip_prefix("tree-v1:")
                .unwrap()
                .to_owned(),
        )
    } else {
        let raw = raw_now.ok_or("Selected Fork registry record is absent")?;
        let raw_record: ForkRecord =
            serde_json::from_value(raw.clone()).map_err(|e| e.to_string())?;
        let mut found = inventory
            .skills
            .iter()
            .flat_map(|skill| &skill.deployments)
            .filter(|deployment| deployment.id == request.unwrap().deployment_id);
        let deployment = found.next().ok_or("Selected Fork deployment is absent")?;
        if found.next().is_some()
            || deployment.owner_kind != LifecycleOwnerKind::Fork
            || deployment.is_symlink
            || deployment.path != path.to_string_lossy()
            || deployment.scope != "global"
            || deployment.destination != SkillDestination::Universal
            || deployment.agent != "shared"
        {
            return Err("Fork removal ownership is ambiguous or unsupported".into());
        }
        if deployment.owner_revision.as_deref()
            != Some(request.unwrap().expected_owner_revision.as_str())
            || RegistryOwnerRecord::Fork(&raw_record).revision().as_deref()
                != deployment.owner_revision.as_deref()
        {
            return Err("Fork removal registry ownership changed".into());
        }
        let content_scope =
            SkillReadScope::bind(std::slice::from_ref(&path)).map_err(|e| e.to_string())?;
        let live_hash = crate::skill_discovery::live_skill_content_hash_with_check(
            &content_scope,
            &path,
            || {
                if cancellation.is_cancelled() {
                    Err("Fork removal cancelled".into())
                } else {
                    Ok(())
                }
            },
        )?;
        if live_hash != deployment.content_hash || live_hash.is_empty() {
            return Err("Fork removal source content changed".into());
        }
        let mut record = raw_record;
        if record.deployment_id.is_empty() {
            record.deployment_id = deployment.id.clone();
        }
        if record.skill_dir.as_os_str().is_empty() {
            record.skill_dir = path.clone();
        }
        (record, raw, live_hash)
    };
    let source = BackupSourceRoot::bind(path.parent().ok_or("Fork source has no parent")?)
        .map_err(|e| e.to_string())?
        .select(path.file_name().ok_or("Fork source has no name")?)
        .map_err(|e| e.to_string())?;
    let expected_tree = if intent.is_some() {
        format!("tree-v1:{live_hash}")
    } else {
        tree(&source, limits, &cancellation)?.ok_or("Fork source disappeared during admission")?
    };
    let holding = BackupSourceRoot::bind(&parent)
        .map_err(|e| e.to_string())?
        .select(std::ffi::OsStr::new(".skill-studio-removing"))
        .map_err(|e| e.to_string())?;
    let destination_device = holding
        .private_directory_device()
        .map_err(|e| e.to_string())?
        .unwrap_or(
            holding
                .directory
                .dir_metadata()
                .map_err(|e| e.to_string())?
                .dev(),
        );
    if source
        .directory
        .dir_metadata()
        .map_err(|e| e.to_string())?
        .dev()
        != destination_device
    {
        return Err("Fork removal quarantine is on a different filesystem".into());
    }
    let quarantine = holding
        .private_directory_device()
        .map_err(|e| e.to_string())?
        .is_some()
        .then(|| {
            BackupSourceRoot::bind(&holding_path)
                .and_then(|root| root.select(event_id.as_ref()))
                .map_err(|e| e.to_string())
        })
        .transpose()?;
    let trial_values = matching_trial_values(&doc, &record.deployment_id, &path)?;
    let intent = intent.cloned().unwrap_or(ForkRemovalIntent {
        version: 1,
        selected: record,
        name,
        path,
        expected_tree,
        registry_path,
        quarantine_path,
        configured_roots: current_roots,
        raw_registry_value: raw,
        raw_trial_values: trial_values,
    });
    intent.validate()?;
    Ok(Prepared {
        intent,
        registry,
        source,
        quarantine,
        holding,
        lease,
        cancellation,
    })
}

fn prepare_effects<'a>(
    service: &'a mut ScopedSkillService,
    store: &EventStore,
    event_id: &str,
    intent: &ForkRemovalIntent,
    timeout: Option<Duration>,
) -> Result<Prepared<'a>, String> {
    let (parent, holding_path) = validate_current_scope(service, store, event_id, intent)?;
    let active_tree = match std::fs::symlink_metadata(&intent.path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            intent.path.clone()
        }
        Ok(_) => return Err("Fork removal source is not an independent directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::symlink_metadata(&intent.quarantine_path) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    intent.quarantine_path.clone()
                }
                Ok(_) => {
                    return Err("Fork removal quarantine is not an independent directory".into());
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => parent.clone(),
                Err(error) => return Err(error.to_string()),
            }
        }
        Err(error) => return Err(error.to_string()),
    };
    let entries = vec![
        intent.path.clone(),
        holding_path.clone(),
        intent.quarantine_path.clone(),
    ];
    let (_inventory, lease) = service
        .prepare_write_inventory_with_entries(
            Some(&BTreeSet::from([intent.name.clone()])),
            &[store.app_data.clone(), active_tree, parent.clone()],
            &entries,
            timeout,
            CancellationToken::default(),
        )
        .map_err(|error| error.to_string())?;
    let registry = lease
        .read(&intent.registry_path, 8 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    let source = BackupSourceRoot::bind(
        intent
            .path
            .parent()
            .ok_or("Fork source parent is missing")?,
    )
    .map_err(|error| error.to_string())?
    .select(
        intent
            .path
            .file_name()
            .ok_or("Fork source name is missing")?,
    )
    .map_err(|error| error.to_string())?;
    let holding = BackupSourceRoot::bind(&parent)
        .map_err(|error| error.to_string())?
        .select(std::ffi::OsStr::new(".skill-studio-removing"))
        .map_err(|error| error.to_string())?;
    let parent_device = holding
        .directory
        .dir_metadata()
        .map_err(|error| error.to_string())?
        .dev();
    let holding_device = holding
        .private_directory_device()
        .map_err(|error| error.to_string())?;
    if holding_device.is_some_and(|device| device != parent_device) {
        return Err("Fork removal quarantine is on a different filesystem".into());
    }
    let quarantine = holding_device
        .is_some()
        .then(|| {
            BackupSourceRoot::bind(&holding_path)
                .and_then(|root| {
                    root.select(intent.quarantine_path.file_name().ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "Fork quarantine name is missing",
                        )
                    })?)
                })
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    Ok(Prepared {
        intent: intent.clone(),
        registry,
        source,
        quarantine,
        holding,
        lease,
        cancellation: CancellationToken::default(),
    })
}

fn pending(
    store: &EventStore,
    lease: &FinalizedWriteLease<'_>,
    id: &str,
    intent: &ForkRemovalIntent,
) -> Result<EventRow, String> {
    let row = GuardedEventStore::bind(store, lease)?
        .next_recovery_event(lease)?
        .ok_or("Fork removal event is no longer pending")?;
    if row.id != id || ForkRemovalIntent::from_event(&row)? != *intent {
        return Err("Fork removal event changed or is out of order".into());
    }
    Ok(row)
}

fn finish_if_published(
    service: &ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    intent: &ForkRemovalIntent,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<bool, String> {
    use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};

    let (parent, holding_path) = validate_current_scope(service, store, &row.id, intent)?;
    let registry_parent = intent
        .registry_path
        .parent()
        .ok_or("Fork registry parent is missing")?;
    let registry_scope =
        SkillReadScope::bind(&[store.app_data.clone(), registry_parent.to_path_buf()])
            .map_err(|error| error.to_string())?;
    let registry_effects = vec![
        DirectoryEffect::tree(&store.app_data, CoordinationMode::Exclusive),
        DirectoryEffect::entry(&intent.registry_path, CoordinationMode::Exclusive),
    ];
    let registry_lease = CoordinationPlan::new(registry_effects, timeout)
        .map_err(|error| error.to_string())?
        .acquire()
        .map_err(|error| error.to_string())?
        .finalize_write(&registry_scope, std::slice::from_ref(&intent.registry_path))
        .map_err(|error| error.to_string())?;
    let registry = registry_lease
        .read(&intent.registry_path, 8 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    if intent.apply_registry(&registry)? != registry {
        return Ok(false);
    }
    drop(registry_lease);

    let holding = BackupSourceRoot::bind(&parent)
        .map_err(|error| error.to_string())?
        .select(std::ffi::OsStr::new(".skill-studio-removing"))
        .map_err(|error| error.to_string())?;
    let parent_device = holding
        .directory
        .dir_metadata()
        .map_err(|error| error.to_string())?
        .dev();
    match holding
        .private_directory_device()
        .map_err(|error| error.to_string())?
    {
        Some(device) if device == parent_device => {}
        Some(_) => return Err("Fork removal quarantine is on a different filesystem".into()),
        None => return Err("Published Fork removal holding directory is missing".into()),
    }
    let read_scope = SkillReadScope::bind(&[
        store.app_data.clone(),
        registry_parent.to_path_buf(),
        holding_path.clone(),
    ])
    .map_err(|error| error.to_string())?;
    let effects = vec![
        DirectoryEffect::tree(&store.app_data, CoordinationMode::Exclusive),
        DirectoryEffect::tree(&holding_path, CoordinationMode::Exclusive),
        DirectoryEffect::tree(&intent.quarantine_path, CoordinationMode::Exclusive),
        DirectoryEffect::entry(&intent.quarantine_path, CoordinationMode::Exclusive),
        DirectoryEffect::entry(&intent.registry_path, CoordinationMode::Exclusive),
    ];
    let lease = CoordinationPlan::new(effects, timeout)
        .map_err(|error| error.to_string())?
        .acquire()
        .map_err(|error| error.to_string())?
        .finalize_write(&read_scope, std::slice::from_ref(&intent.registry_path))
        .map_err(|error| error.to_string())?;
    let registry = lease
        .read(&intent.registry_path, 8 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    if intent.apply_registry(&registry)? != registry {
        return Err("Fork removal publication changed during settlement".into());
    }
    let quarantine = BackupSourceRoot::bind(&holding_path)
        .map_err(|error| error.to_string())?
        .select(
            intent
                .quarantine_path
                .file_name()
                .ok_or("Fork quarantine name is missing")?,
        )
        .map_err(|error| error.to_string())?;
    if tree(&quarantine, limits, &CancellationToken::default())?.as_deref()
        != Some(&intent.expected_tree)
    {
        return Err("Fork removal quarantine changed after publication".into());
    }
    let pending = pending(store, &lease, &row.id, intent)?;
    GuardedEventStore::bind(store, &lease)?
        .finish_recovery_snapshot(&lease, &pending, EventStatus::Done, None)
        .map_err(|error| error.to_string())?;
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForwardCheckpoint {
    HoldingCreated,
    TreeMoved,
    RegistryPublished,
}

fn execute_forward_with_checkpoint(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    mut checkpoint: impl FnMut(ForwardCheckpoint) -> Result<(), String>,
) -> Result<(), String> {
    let intent = ForkRemovalIntent::from_event(row)?;
    if finish_if_published(service, store, row, &intent, limits, timeout)? {
        return Ok(());
    }
    let mut prepared = prepare_effects(service, store, &row.id, &intent, timeout)?;
    if prepared.quarantine.is_none() {
        unpublished_registry(&intent, &prepared.registry)?;
        pending(store, &prepared.lease, &row.id, &intent)?;
        prepared
            .holding
            .create_directory()
            .map_err(|e| e.to_string())?;
        drop(prepared);
        checkpoint(ForwardCheckpoint::HoldingCreated)?;
        prepared = prepare_effects(service, store, &row.id, &intent, timeout)?;
    }
    unpublished_registry(&intent, &prepared.registry)?;
    let quarantine = prepared
        .quarantine
        .as_ref()
        .ok_or("Fork removal quarantine is unavailable")?;
    match (
        tree(&prepared.source, limits, &prepared.cancellation)?,
        tree(quarantine, limits, &prepared.cancellation)?,
    ) {
        (Some(source), None) if source == intent.expected_tree => {
            pending(store, &prepared.lease, &row.id, &intent)?;
            move_to_quarantine(prepared, limits)?;
            checkpoint(ForwardCheckpoint::TreeMoved)?;
        }
        (None, Some(q)) if q == intent.expected_tree => {}
        _ => return Err("Fork removal source or quarantine changed".into()),
    }
    let mut prepared = prepare_effects(service, store, &row.id, &intent, timeout)?;
    let quarantine = prepared
        .quarantine
        .as_ref()
        .ok_or("Fork removal quarantine is missing")?;
    if tree(&prepared.source, limits, &prepared.cancellation)?.is_some()
        || tree(quarantine, limits, &prepared.cancellation)?.as_deref()
            != Some(&intent.expected_tree)
    {
        return Err("Fork removal source changed before publication".into());
    }
    pending(store, &prepared.lease, &row.id, &intent)?;
    let target = crate::skill_document_target::SkillRegistryTarget::bind(
        intent
            .registry_path
            .parent()
            .ok_or("Fork registry parent is missing")?,
    )?;
    let proposed = unpublished_registry(&intent, &prepared.registry)?;
    target
        .replace(&mut prepared.lease, &prepared.registry, &proposed)
        .map_err(|e| e.to_string())?;
    checkpoint(ForwardCheckpoint::RegistryPublished)?;
    finish(store, &prepared, row, EventStatus::Done)
}

fn unpublished_registry(intent: &ForkRemovalIntent, registry: &[u8]) -> Result<Vec<u8>, String> {
    let proposed = intent.apply_registry(registry)?;
    if proposed == registry {
        return Err("Fork ownership disappeared before removal publication".into());
    }
    Ok(proposed)
}

fn forward(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<(), String> {
    execute_forward_with_checkpoint(service, store, row, limits, timeout, |_| Ok(()))
}

fn finish(
    store: &EventStore,
    prepared: &Prepared<'_>,
    row: &EventRow,
    status: EventStatus,
) -> Result<(), String> {
    pending(store, &prepared.lease, &row.id, &prepared.intent)?;
    GuardedEventStore::bind(store, &prepared.lease)?
        .finish_recovery_snapshot(&prepared.lease, row, status, None)
        .map_err(|e| e.to_string())
}

pub fn recover_fork_removal(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<bool, String> {
    recover_with_checkpoint(service, store, row, limits, timeout, || Ok(()))
}

fn recover_with_checkpoint(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    checkpoint: impl FnOnce() -> Result<(), String>,
) -> Result<bool, String> {
    let intent = ForkRemovalIntent::from_event(row)?;
    if finish_if_published(service, store, row, &intent, limits, timeout)? {
        return Ok(true);
    }
    checkpoint()?;
    let prepared = prepare_effects(service, store, &row.id, &intent, timeout)?;
    unpublished_registry(&intent, &prepared.registry)?;
    let needs_restore = match (
        &prepared.quarantine,
        tree(&prepared.source, limits, &prepared.cancellation)?,
    ) {
        (Some(quarantine), None) => {
            if tree(quarantine, limits, &prepared.cancellation)?.as_deref()
                != Some(&intent.expected_tree)
            {
                return Err("Fork removal quarantine changed before recovery".into());
            }
            true
        }
        (_, Some(tree)) if tree == intent.expected_tree => false,
        _ => return Err("Fork removal rollback paths changed".into()),
    };
    if needs_restore {
        pending(store, &prepared.lease, &row.id, &intent)?;
        restore_from_quarantine(prepared, limits)?;
    } else {
        drop(prepared);
    }
    let prepared = prepare_effects(service, store, &row.id, &intent, timeout)?;
    unpublished_registry(&intent, &prepared.registry)?;
    finish(store, &prepared, row, EventStatus::Failed)?;
    Ok(false)
}

pub fn remove_fork_deployment(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &ForkRemovalRequest,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<ForkRemovalOutcome, ForkRemovalError> {
    let id = crate::skill_event_store::allocate_id();
    let prepared = prepare(
        service,
        store,
        &id,
        Some(request),
        None,
        limits,
        timeout,
        cancellation,
    )
    .map_err(|message| ForkRemovalError {
        event_id: None,
        recovery_required: false,
        message,
    })?;
    let intent = prepared.intent.clone();
    GuardedEventStore::bind(store, &prepared.lease)
        .map_err(|message| ForkRemovalError {
            event_id: None,
            recovery_required: false,
            message,
        })?
        .record_pending(
            &prepared.lease,
            &id,
            intent.event_draft().map_err(|message| ForkRemovalError {
                event_id: None,
                recovery_required: false,
                message,
            })?,
        )
        .map_err(|error| ForkRemovalError {
            event_id: matches!(error, EventWriteFailure::MayHaveWritten(_)).then(|| id.clone()),
            recovery_required: matches!(error, EventWriteFailure::MayHaveWritten(_)),
            message: error.to_string(),
        })?;
    drop(prepared);
    let row = store
        .get(&id)
        .map_err(|message| ForkRemovalError {
            event_id: Some(id.clone()),
            recovery_required: true,
            message,
        })?
        .ok_or_else(|| ForkRemovalError {
            event_id: Some(id.clone()),
            recovery_required: true,
            message: "Recorded Fork removal event is missing".into(),
        })?;
    match forward(service, store, &row, limits, timeout) {
        Ok(()) => Ok(ForkRemovalOutcome {
            event_id: id,
            removed_deployment_ids: vec![intent.selected.deployment_id],
        }),
        Err(message) => match recover_fork_removal(service, store, &row, limits, timeout) {
            Ok(true) => Ok(ForkRemovalOutcome {
                event_id: id,
                removed_deployment_ids: vec![intent.selected.deployment_id],
            }),
            Ok(false) => Err(ForkRemovalError {
                event_id: Some(id),
                recovery_required: false,
                message,
            }),
            Err(recovery) => Err(ForkRemovalError {
                event_id: Some(id),
                recovery_required: true,
                message: format!("{message}; removal requires recovery: {recovery}"),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{skill_deployment::deployment_id, skill_fork_registry::OriginTool};

    fn intent() -> ForkRemovalIntent {
        let path = PathBuf::from("/tmp/home/.agents/skills/sample");
        let id = deployment_id(
            "sample",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            &path,
        );
        let selected = ForkRecord {
            deployment_id: id,
            skill_dir: path.clone(),
            forked_at: "now".into(),
            origin_tool: OriginTool::SkillsSh,
            origin_source: "o/r".into(),
            repo: "o/r".into(),
            path: "skills/sample".into(),
            declared_ref: None,
            base_commit: "base".into(),
        };
        let raw_registry_value = serde_json::to_value(&selected).unwrap();
        let raw_trial_value = serde_json::json!({"deployment_id":selected.deployment_id});
        ForkRemovalIntent {
            version: 1,
            selected,
            name: "sample".into(),
            path,
            expected_tree: format!("tree-v1:{}", "a".repeat(64)),
            registry_path: PathBuf::from("/tmp/home/.agents/skill-studio.json"),
            quarantine_path: PathBuf::from("/tmp/home/.agents/.skill-studio-removing/event"),
            configured_roots: vec![PathBuf::from("/tmp/home/.agents/skills")],
            raw_registry_value,
            raw_trial_values: BTreeMap::from([("deployment/x".into(), raw_trial_value)]),
        }
    }

    #[test]
    fn removal_preserves_unknown_and_sibling_registry_values() {
        let intent = intent();
        let document = serde_json::json!({"unknown":{"kept":true}, "forks":{"sample":intent.raw_registry_value, "other":{"opaque":1}}, "trials":{"deployment/x":intent.raw_trial_values["deployment/x"], "other":{"opaque":2}}});
        let output: serde_json::Value = serde_json::from_slice(
            &intent
                .apply_registry(&serde_json::to_vec(&document).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(output["unknown"], serde_json::json!({"kept":true}));
        assert_eq!(output["forks"]["other"], serde_json::json!({"opaque":1}));
        assert!(output["forks"].get("sample").is_none());
        assert!(output["trials"].get("deployment/x").is_none());
        assert_eq!(output["trials"]["other"], serde_json::json!({"opaque":2}));
    }

    #[test]
    fn changed_trial_refuses_publication() {
        let intent = intent();
        let document = serde_json::json!({"forks":{"sample":intent.raw_registry_value}, "trials":{"deployment/x":{"deployment_id":"changed"}}});
        assert!(intent
            .apply_registry(&serde_json::to_vec(&document).unwrap())
            .is_err());
    }
    #[test]
    fn removes_real_fork_and_retains_tree_with_completed_event() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        let path = home.join(".agents/skills/sample");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::create_dir_all(home.join(".git")).unwrap();
        std::fs::write(
            path.join("SKILL.md"),
            "---\nname: sample\ndescription: Fixture\n---\nOriginal fork.\n",
        )
        .unwrap();
        let id = deployment_id(
            "sample",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            &path,
        );
        let selected = ForkRecord {
            deployment_id: id.clone(),
            skill_dir: path.clone(),
            forked_at: "2026-09-16T00:00:00Z".into(),
            origin_tool: OriginTool::SkillsSh,
            origin_source: "o/r".into(),
            repo: "o/r".into(),
            path: "skills/sample".into(),
            declared_ref: None,
            base_commit: "a".repeat(40),
        };
        std::fs::write(
            home.join(".agents/skill-studio.json"),
            serde_json::to_vec(&serde_json::json!({"version":4,"forks":{"sample":selected}}))
                .unwrap(),
        )
        .unwrap();
        let scope = crate::skill_service::SkillScope {
            home: home.clone(),
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        };
        let mut service = ScopedSkillService::bind(scope).unwrap();
        let store = EventStore::open(&root.join("app-data")).unwrap();
        let request = ForkRemovalRequest {
            deployment_id: id,
            expected_owner_revision: RegistryOwnerRecord::Fork(&selected).revision().unwrap(),
        };
        let limits = BackupCopyLimits {
            max_bytes: 1024 * 1024,
            max_entries: 100,
            max_depth: 8,
        };
        let result = remove_fork_deployment(
            &mut service,
            &store,
            &request,
            limits,
            Some(Duration::from_secs(2)),
            CancellationToken::default(),
        )
        .unwrap();
        assert!(!path.exists());
        let row = store.get(&result.event_id).unwrap().unwrap();
        assert_eq!(row.status, "done");
        assert!(!row.restorable);
        let intent = ForkRemovalIntent::from_event(&row).unwrap();
        assert!(intent.quarantine_path.join("SKILL.md").exists());
        let registry: serde_json::Value =
            serde_json::from_slice(&std::fs::read(home.join(".agents/skill-studio.json")).unwrap())
                .unwrap();
        assert!(registry["forks"].get("sample").is_none());
    }
}

#[cfg(test)]
#[path = "skill_fork_removal/execution_tests.rs"]
mod execution_tests;
