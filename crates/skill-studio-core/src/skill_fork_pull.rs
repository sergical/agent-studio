//! Durable three-way Pull for one global Universal Fork.
use crate::{
    skill_backup_copy::{inspect_entry, BackupCopyLimits},
    skill_backup_reservation::BackupStateRoot,
    skill_backup_source::{BackupSource, BackupSourceRoot},
    skill_coordination::{CancellationToken, FinalizedWriteLease},
    skill_deployment::{parse_deployment_id, SkillDestination},
    skill_document_target::SkillRegistryTarget,
    skill_event::{EventDraft, EventRow, EventStatus},
    skill_event_operations::{EventWriteFailure, GuardedEventStore},
    skill_event_store::EventStore,
    skill_fork_registry::{ForkRecord, ForkRegistry, RegistryOwnerRecord},
    skill_ownership::LifecycleOwnerKind,
    skill_service::{ScopedSkillService, SkillScope},
    skill_tree_exchange::TreeExchangeFailure,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Component, Path, PathBuf},
    time::Duration,
};

pub const EVENT_KIND: &str = "pull_fork_upstream";
const MAX_REGISTRY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkPullRequest {
    pub deployment_id: String,
    pub expected_owner_revision: String,
    pub to_commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ForkPullResult {
    pub from_commit: String,
    pub to_commit: String,
    pub merged: Vec<String>,
    pub conflicts: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: usize,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum ForkPullPreparationOutcome {
    UpToDate(ForkPullResult),
    Prepared(ForkPullPreparation),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkPullPreparation {
    version: u32,
    id: String,
    scope: SkillScope,
    app_data: PathBuf,
    request: ForkPullRequest,
    name: String,
    live_path: PathBuf,
    base_path: PathBuf,
    registry_path: PathBuf,
    record_before: ForkRecord,
    record_after: ForkRecord,
    live_identity: String,
    base_identity: String,
    registry_before: Vec<u8>,
    preparation_root: PathBuf,
    preparation_device: u64,
    preparation_inode: u64,
}

impl ForkPullPreparation {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn repo(&self) -> &str {
        &self.record_before.repo
    }
    pub fn source_path(&self) -> &str {
        &self.record_before.path
    }
    pub fn declared_ref(&self) -> Option<&str> {
        self.record_before.declared_ref.as_deref()
    }
    pub fn from_commit(&self) -> &str {
        &self.record_before.base_commit
    }
    pub fn to_commit(&self) -> &str {
        &self.request.to_commit
    }
}

#[derive(Debug)]
pub struct ForkPullError {
    pub event_id: Option<String>,
    pub recovery_required: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkPullTextMergeResult {
    Clean(Vec<u8>),
    Conflicts(Vec<u8>),
}

pub trait ForkPullTextMerge: Send + Sync {
    fn merge(
        &self,
        mine: &[u8],
        base: &[u8],
        theirs: &[u8],
        relative_path: &Path,
        max_output: u64,
    ) -> Result<ForkPullTextMergeResult, String>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForkPullIntent {
    version: u32,
    scope: SkillScope,
    app_data: PathBuf,
    request: ForkPullRequest,
    name: String,
    live_path: PathBuf,
    base_path: PathBuf,
    registry_path: PathBuf,
    record_before: ForkRecord,
    record_after: ForkRecord,
    live_before: String,
    live_after: String,
    base_before: String,
    base_after: String,
    registry_before: Vec<u8>,
    registry_after: Vec<u8>,
    result: ForkPullResult,
}

impl ForkPullIntent {
    fn validate(&self, event_id: &str) -> Result<(), String> {
        let parsed =
            parse_deployment_id(&self.request.deployment_id).ok_or("Invalid Pull deployment ID")?;
        if self.version != 1
            || !crate::skill_backup_reservation::valid_id(event_id)
            || parsed.name != self.name
            || parsed.scope != "global"
            || parsed.slot != "universal"
            || parsed.destination != SkillDestination::Universal
            || parsed.lexical_path != self.live_path
            || self.request.expected_owner_revision.is_empty()
            || !valid_commit(&self.request.to_commit)
            || self.record_before.base_commit == self.request.to_commit
            || self.record_after.base_commit != self.request.to_commit
            || self.record_after.deployment_id != self.request.deployment_id
            || self.record_after.skill_dir != self.live_path
            || self.result.from_commit != self.record_before.base_commit
            || self.result.to_commit != self.request.to_commit
            || !self.live_path.is_absolute()
            || !self.base_path.is_absolute()
            || !self.registry_path.is_absolute()
            || !self.app_data.is_absolute()
            || self.live_path != self.scope.home.join(".agents/skills").join(&self.name)
            || self.registry_path != self.scope.home.join(".agents/skill-studio.json")
            || self.base_path
                != self
                    .app_data
                    .join("skill-studio/forks")
                    .join(&self.name)
                    .join("base")
            || !valid_tree_identity(&self.live_before)
            || !valid_tree_identity(&self.live_after)
            || !valid_tree_identity(&self.base_before)
            || !valid_tree_identity(&self.base_after)
        {
            return Err("Invalid Fork Pull intent".into());
        }
        let transition = ForkPullRegistryTransition::new(
            self.name.clone(),
            self.record_before.clone(),
            self.record_after.clone(),
            &self.registry_before,
        )?;
        if transition.apply_document(&self.registry_before)? != self.registry_after {
            return Err("Invalid Fork Pull registry transition".into());
        }
        Ok(())
    }

    fn draft(&self, event_id: &str) -> Result<EventDraft, String> {
        self.validate(event_id)?;
        Ok(EventDraft {
            kind: EVENT_KIND.into(),
            skill: self.name.clone(),
            harness: Some("universal".into()),
            scope: Some("global".into()),
            project_path: None,
            payload: serde_json::to_value(self).map_err(|error| error.to_string())?,
            inverse: None,
            backup_dir: Some(format!("backups/{event_id}")),
            restorable: false,
        })
    }

    fn from_row(row: &EventRow) -> Result<Self, String> {
        if row.kind != EVENT_KIND
            || row.restorable
            || row.inverse.is_some()
            || row.reverted_by.is_some()
        {
            return Err("Not a Fork Pull event".into());
        }
        let intent: Self =
            serde_json::from_value(row.payload.clone()).map_err(|error| error.to_string())?;
        intent.validate(&row.id)?;
        if row.skill != intent.name
            || row.harness.as_deref() != Some("universal")
            || row.scope.as_deref() != Some("global")
            || row.project_path.is_some()
            || row.backup_dir.as_deref() != Some(&format!("backups/{}", row.id))
        {
            return Err("Fork Pull event metadata does not match its intent".into());
        }
        Ok(intent)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForkPullRegistryTransition {
    name: String,
    before: ForkRecord,
    after: ForkRecord,
    selected_before: Value,
    selected_after: Value,
}

impl ForkPullRegistryTransition {
    fn new(
        name: String,
        before: ForkRecord,
        after: ForkRecord,
        document: &[u8],
    ) -> Result<Self, String> {
        let selected_before = Self::parse(document)?
            .get("forks")
            .and_then(Value::as_object)
            .and_then(|forks| forks.get(&name))
            .cloned()
            .ok_or("Selected Fork registry row is missing")?;
        let parsed_before: ForkRecord =
            serde_json::from_value(selected_before.clone()).map_err(|error| error.to_string())?;
        if parsed_before != before {
            return Err("Selected Fork registry row changed".into());
        }
        let mut selected_after = selected_before.clone();
        let after_object = selected_after
            .as_object_mut()
            .ok_or("Selected Fork registry row must be an object")?;
        after_object.insert(
            "deployment_id".into(),
            Value::String(after.deployment_id.clone()),
        );
        after_object.insert(
            "skill_dir".into(),
            Value::String(after.skill_dir.to_string_lossy().into_owned()),
        );
        after_object.insert(
            "base_commit".into(),
            Value::String(after.base_commit.clone()),
        );
        let parsed_after: ForkRecord =
            serde_json::from_value(selected_after.clone()).map_err(|error| error.to_string())?;
        if parsed_after != after {
            return Err("Proposed Fork registry row does not match Pull intent".into());
        }
        let transition = Self {
            name,
            before,
            after,
            selected_before,
            selected_after,
        };
        transition.validate()?;
        Ok(transition)
    }

    fn validate(&self) -> Result<(), String> {
        if self.name.is_empty()
            || self.name.contains(['/', '\\'])
            || self.before.repo.is_empty()
            || self.before.path.is_empty()
            || self.after.repo != self.before.repo
            || self.after.path != self.before.path
            || self.after.origin_tool != self.before.origin_tool
            || self.after.origin_source != self.before.origin_source
            || self.after.forked_at != self.before.forked_at
            || self.after.declared_ref != self.before.declared_ref
            || !valid_commit(&self.before.base_commit)
            || !valid_commit(&self.after.base_commit)
            || serde_json::from_value::<ForkRecord>(self.selected_before.clone()).ok()
                != Some(self.before.clone())
            || serde_json::from_value::<ForkRecord>(self.selected_after.clone()).ok()
                != Some(self.after.clone())
        {
            return Err("Invalid Fork Pull registry transition".into());
        }
        Ok(())
    }

    fn parse(document: &[u8]) -> Result<Value, String> {
        if document.len() > MAX_REGISTRY_BYTES {
            return Err("Fork registry exceeds its limit".into());
        }
        let value: Value = serde_json::from_slice(document).map_err(|error| error.to_string())?;
        if !value.is_object() || value.get("forks").is_some_and(|value| !value.is_object()) {
            return Err("Fork registry must contain an object-valued forks section".into());
        }
        Ok(value)
    }

    fn apply_document(&self, current: &[u8]) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut document = Self::parse(current)?;
        let selected = document
            .get("forks")
            .and_then(Value::as_object)
            .and_then(|forks| forks.get(&self.name))
            .cloned();
        if selected == Some(self.selected_after.clone()) {
            return Ok(current.into());
        }
        if selected != Some(self.selected_before.clone()) {
            return Err("Selected Fork registry row changed".into());
        }
        document
            .as_object_mut()
            .and_then(|root| root.get_mut("forks"))
            .and_then(Value::as_object_mut)
            .ok_or("Fork registry must contain an object-valued forks section")?
            .insert(self.name.clone(), self.selected_after.clone());
        let bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_REGISTRY_BYTES {
            return Err("Fork registry exceeds its limit".into());
        }
        Ok(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TreeNode {
    Directory(u32),
    RegularFile(Vec<u8>, u32),
    Symlink(PathBuf),
}

pub(crate) fn valid_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_tree_identity(value: &str) -> bool {
    value.strip_prefix("tree-v1:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn source(path: &Path) -> Result<BackupSource, String> {
    let parent = path.parent().ok_or("Tree path has no parent")?;
    let name = path.file_name().ok_or("Tree path has no name")?;
    BackupSourceRoot::bind(parent)
        .and_then(|root| root.select(name))
        .map_err(|error| error.to_string())
}

fn tree_identity(
    path: &Path,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<String, String> {
    let selected = source(path)?;
    inspect_entry(&selected.directory, &selected.name, limits, cancellation)
        .map(|report| report.tree_identity)
        .map_err(|error| error.to_string())
}

fn preparation_path(store: &EventStore, id: &str) -> PathBuf {
    store
        .app_data
        .join("skill-studio")
        .join("pull-preparations")
        .join(id)
}

fn base_path(store: &EventStore, name: &str) -> PathBuf {
    store
        .app_data
        .join("skill-studio")
        .join("forks")
        .join(name)
        .join("base")
}

fn registry_path(scope: &SkillScope) -> PathBuf {
    scope.home.join(".agents/skill-studio.json")
}

fn ensure_private_preparation(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("Pull preparation has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
        Ok(_) => return Err("Pull preparation already exists; preserving it".into()),
    }
    fs::create_dir(path).map_err(|error| error.to_string())?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| error.to_string())
}

fn discard_preparation(preparation: &ForkPullPreparation) -> Result<(), String> {
    if preparation.preparation_root
        != preparation
            .app_data
            .join("skill-studio/pull-preparations")
            .join(&preparation.id)
    {
        return Err("Pull preparation path is not owned by this operation".into());
    }
    BackupStateRoot::bind(&preparation.preparation_root)
        .map_err(|error| error.to_string())?
        .discard_owned_root(
            preparation.preparation_device,
            preparation.preparation_inode,
        )
        .map_err(|error| error.to_string())
}

fn validate_deployment(
    inventory: &crate::skill_service::InventoryRead,
    request: &ForkPullRequest,
    name: &str,
    live_path: &Path,
    allowed_owner_revisions: &[String],
) -> Result<(), String> {
    let mut selected = inventory
        .skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .filter(|deployment| deployment.id == request.deployment_id)
        .peekable();
    let deployment = selected
        .next()
        .ok_or("Selected Fork deployment is absent")?;
    if selected.peek().is_some()
        || deployment.owner_kind != LifecycleOwnerKind::Fork
        || !deployment
            .owner_revision
            .as_ref()
            .is_some_and(|revision| allowed_owner_revisions.contains(revision))
        || deployment.destination != SkillDestination::Universal
        || deployment.scope != "global"
        || deployment.agent != "shared"
        || deployment.is_symlink
        || deployment.path != live_path.to_string_lossy()
        || live_path.file_name() != Some(OsStr::new(name))
    {
        return Err("Pull requires one fresh canonical Global Universal Fork".into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn prepare_lease<'a>(
    service: &'a mut ScopedSkillService,
    store: &EventStore,
    request: &ForkPullRequest,
    name: &str,
    live_path: &Path,
    base_path: &Path,
    registry_path: &Path,
    candidate_path: Option<&Path>,
    allowed_owner_revisions: &[String],
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<FinalizedWriteLease<'a>, String> {
    let mut trees = vec![store.app_data.clone(), live_path.to_path_buf()];
    let mut entries = vec![
        live_path.to_path_buf(),
        base_path.to_path_buf(),
        registry_path.to_path_buf(),
    ];
    if let Some(candidate) = candidate_path {
        trees.push(candidate.to_path_buf());
        entries.push(candidate.to_path_buf());
    }
    let (inventory, lease) = service
        .prepare_write_inventory_with_entries(
            Some(&BTreeSet::from([name.to_string()])),
            &trees,
            &entries,
            timeout,
            cancellation,
        )
        .map_err(|error| error.to_string())?;
    validate_deployment(
        &inventory,
        request,
        name,
        live_path,
        allowed_owner_revisions,
    )?;
    lease.revalidate().map_err(|error| error.to_string())?;
    Ok(lease)
}

pub fn prepare_fork_pull_inputs(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &ForkPullRequest,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<ForkPullPreparationOutcome, String> {
    let parsed = parse_deployment_id(&request.deployment_id).ok_or("Invalid Pull deployment ID")?;
    let scope = service.scope();
    let live_path = scope.home.join(".agents/skills").join(&parsed.name);
    if parsed.scope != "global"
        || parsed.slot != "universal"
        || parsed.destination != SkillDestination::Universal
        || parsed.lexical_path != live_path
        || request.expected_owner_revision.is_empty()
        || !valid_commit(&request.to_commit)
    {
        return Err("Pull needs one exact Global Universal Fork".into());
    }
    let base_path = base_path(store, &parsed.name);
    let registry_path = registry_path(&scope);
    let legacy_scratch = store.app_data.join("skill-studio/forks").join(&parsed.name);
    for name in [
        "staging-live",
        "staging-base",
        "live-backup",
        "old-base-backup",
    ] {
        let path = legacy_scratch.join(name);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Could not inspect legacy Pull recovery path {}: {error}",
                    path.display()
                ))
            }
            Ok(_) => {
                return Err(format!(
                    "Unresolved legacy Pull recovery material exists at {}; preserving it",
                    path.display()
                ))
            }
        }
    }
    let lease = prepare_lease(
        service,
        store,
        request,
        &parsed.name,
        &live_path,
        &base_path,
        &registry_path,
        None,
        std::slice::from_ref(&request.expected_owner_revision),
        timeout,
        cancellation.clone(),
    )?;
    let registry_before = lease
        .read_current_ownership_registry(&registry_path, MAX_REGISTRY_BYTES)?
        .ok_or("Fork registry is missing")?;
    let registry: ForkRegistry =
        serde_json::from_slice(&registry_before).map_err(|_| "Fork registry is malformed")?;
    let record = registry
        .forks
        .get(&parsed.name)
        .cloned()
        .ok_or("Selected Fork record is missing")?;
    if (!record.deployment_id.is_empty() && record.deployment_id != request.deployment_id)
        || (!record.skill_dir.as_os_str().is_empty() && record.skill_dir != live_path)
        || record.repo.is_empty()
        || record.path.is_empty()
        || !valid_commit(&record.base_commit)
    {
        return Err("Selected Fork record does not bind the canonical deployment".into());
    }
    if record.base_commit == request.to_commit {
        return Ok(ForkPullPreparationOutcome::UpToDate(ForkPullResult {
            from_commit: record.base_commit,
            to_commit: request.to_commit.clone(),
            message: Some("Already up to date".into()),
            ..Default::default()
        }));
    }
    let live_identity = tree_identity(&live_path, limits, &cancellation)?;
    let base_identity = tree_identity(&base_path, limits, &cancellation)?;
    lease.revalidate().map_err(|error| error.to_string())?;
    let id = crate::skill_event_store::allocate_id();
    let preparation_root = preparation_path(store, &id);
    ensure_private_preparation(&preparation_root)?;
    let preparation_metadata =
        fs::symlink_metadata(&preparation_root).map_err(|error| error.to_string())?;
    let preparation_state =
        BackupStateRoot::bind(&preparation_root).map_err(|error| error.to_string())?;
    let prepared = preparation_state
        .reserve("inputs")
        .map_err(|error| error.to_string())?;
    let result = (|| {
        for (path, target, expected) in [
            (&live_path, "live", &live_identity),
            (&base_path, "base", &base_identity),
        ] {
            let selected = source(path)?;
            let report = prepared
                .copy_entry(
                    &selected.directory,
                    &selected.name,
                    OsStr::new(target),
                    limits,
                    &cancellation,
                )
                .map_err(|error| error.to_string())?;
            if &report.tree_identity != expected {
                return Err("Pull input changed while it was copied".into());
            }
        }
        prepared
            .write_new_file("registry.json", &registry_before)
            .map_err(|error| error.to_string())?;
        lease.revalidate().map_err(|error| error.to_string())
    })();
    if let Err(error) = result {
        let _ = BackupStateRoot::bind(&preparation_root).and_then(|root| {
            root.discard_owned_root(
                std::os::unix::fs::MetadataExt::dev(&preparation_metadata),
                std::os::unix::fs::MetadataExt::ino(&preparation_metadata),
            )
        });
        return Err(error);
    }
    let mut record_after = record.clone();
    record_after.deployment_id = request.deployment_id.clone();
    record_after.skill_dir = live_path.clone();
    record_after.base_commit = request.to_commit.clone();
    Ok(ForkPullPreparationOutcome::Prepared(ForkPullPreparation {
        version: 1,
        id,
        scope,
        app_data: store.app_data.clone(),
        request: request.clone(),
        name: parsed.name,
        live_path,
        base_path,
        registry_path,
        record_before: record,
        record_after,
        live_identity,
        base_identity,
        registry_before,
        preparation_root,
        preparation_device: std::os::unix::fs::MetadataExt::dev(&preparation_metadata),
        preparation_inode: std::os::unix::fs::MetadataExt::ino(&preparation_metadata),
    }))
}

pub fn cancel_fork_pull(preparation: &ForkPullPreparation) -> Result<(), String> {
    discard_preparation(preparation)
}

fn safe_component(name: &OsStr) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes != b"."
        && bytes != b".."
        && bytes.len() <= 255
        && !bytes.contains(&b'/')
        && !bytes.contains(&0)
}

fn walk_tree(
    root: &Path,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<BTreeMap<PathBuf, TreeNode>, String> {
    #[allow(clippy::too_many_arguments)]
    fn visit(
        root: &Path,
        relative: &Path,
        depth: usize,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
        bytes: &mut u64,
        entries: &mut u64,
        nodes: &mut BTreeMap<PathBuf, TreeNode>,
    ) -> Result<(), String> {
        if cancellation.is_cancelled() {
            return Err("Pull merge cancelled".into());
        }
        if depth > limits.max_depth || *entries >= limits.max_entries {
            return Err("Pull merge entry or depth limit exceeded".into());
        }
        let path = root.join(relative);
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        *entries += 1;
        let mode = metadata.permissions().mode() & 0o7777;
        if metadata.file_type().is_symlink() {
            nodes.insert(
                relative.to_path_buf(),
                TreeNode::Symlink(fs::read_link(&path).map_err(|error| error.to_string())?),
            );
        } else if metadata.is_file() {
            if metadata.len() > limits.max_bytes.saturating_sub(*bytes) {
                return Err("Pull merge byte limit exceeded".into());
            }
            let content = fs::read(&path).map_err(|error| error.to_string())?;
            *bytes += content.len() as u64;
            nodes.insert(relative.to_path_buf(), TreeNode::RegularFile(content, mode));
        } else if metadata.is_dir() {
            nodes.insert(relative.to_path_buf(), TreeNode::Directory(mode));
            let mut names = fs::read_dir(&path)
                .map_err(|error| error.to_string())?
                .map(|entry| {
                    entry
                        .map(|entry| entry.file_name())
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            names.sort();
            for name in names {
                if !safe_component(&name) {
                    return Err("Pull merge found an unsupported entry name".into());
                }
                visit(
                    root,
                    &relative.join(name),
                    depth + 1,
                    limits,
                    cancellation,
                    bytes,
                    entries,
                    nodes,
                )?;
            }
        } else {
            return Err("Pull merge found an unsupported special node".into());
        }
        Ok(())
    }
    let metadata = fs::symlink_metadata(root).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Pull merge input is not an independent directory".into());
    }
    let mut nodes = BTreeMap::new();
    let mut bytes = 0;
    let mut entries = 0;
    visit(
        root,
        Path::new(""),
        0,
        limits,
        cancellation,
        &mut bytes,
        &mut entries,
        &mut nodes,
    )?;
    Ok(nodes)
}

fn same_kind(left: &TreeNode, right: &TreeNode) -> bool {
    matches!(
        (left, right),
        (TreeNode::Directory(_), TreeNode::Directory(_))
            | (TreeNode::RegularFile(_, _), TreeNode::RegularFile(_, _))
            | (TreeNode::Symlink(_), TreeNode::Symlink(_))
    )
}

fn merge_mode(base: Option<u32>, mine: u32, theirs: u32) -> (u32, bool) {
    if mine == theirs {
        (mine, false)
    } else if base == Some(mine) {
        (theirs, false)
    } else if base == Some(theirs) {
        (mine, false)
    } else {
        (mine, true)
    }
}

fn merge_nodes(
    base: &BTreeMap<PathBuf, TreeNode>,
    mine: &BTreeMap<PathBuf, TreeNode>,
    theirs: &BTreeMap<PathBuf, TreeNode>,
    from_commit: &str,
    to_commit: &str,
    limits: BackupCopyLimits,
    text_merge: &dyn ForkPullTextMerge,
) -> Result<(BTreeMap<PathBuf, TreeNode>, ForkPullResult), String> {
    let paths = base
        .keys()
        .chain(mine.keys())
        .chain(theirs.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut result = ForkPullResult {
        from_commit: from_commit.into(),
        to_commit: to_commit.into(),
        ..Default::default()
    };
    let mut merged = BTreeMap::new();
    let mut output_bytes = 0_u64;
    for path in paths {
        let base_node = base.get(&path);
        let mine_node = mine.get(&path);
        let theirs_node = theirs.get(&path);
        let mut conflict = false;
        let selected = if mine_node == theirs_node {
            if mine_node.is_some() && !path.as_os_str().is_empty() {
                result.unchanged += 1;
            }
            mine_node.cloned()
        } else if theirs_node == base_node {
            mine_node.cloned()
        } else if mine_node == base_node {
            if mine_node.is_none() {
                result.added.push(path.to_string_lossy().into_owned());
            } else if theirs_node.is_none() {
                result.removed.push(path.to_string_lossy().into_owned());
            } else {
                result.merged.push(path.to_string_lossy().into_owned());
            }
            theirs_node.cloned()
        } else {
            let present = [base_node, mine_node, theirs_node]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            if present
                .iter()
                .skip(1)
                .any(|node| !same_kind(present[0], node))
            {
                return Err(format!(
                    "Pull found an unsupported type collision at {}",
                    path.display()
                ));
            }
            match (base_node, mine_node, theirs_node) {
                (Some(_), None, Some(theirs)) => {
                    conflict = true;
                    Some(theirs.clone())
                }
                (Some(_), Some(mine), None) => {
                    conflict = true;
                    Some(mine.clone())
                }
                (None, Some(mine), Some(theirs)) | (Some(_), Some(mine), Some(theirs)) => {
                    match (mine, theirs) {
                        (
                            TreeNode::RegularFile(mine_bytes, mine_mode),
                            TreeNode::RegularFile(theirs_bytes, theirs_mode),
                        ) if base_node
                            .is_none_or(|node| matches!(node, TreeNode::RegularFile(_, _))) =>
                        {
                            let base_bytes = match base_node {
                                Some(TreeNode::RegularFile(bytes, _)) => bytes.as_slice(),
                                _ => &[],
                            };
                            let base_mode = match base_node {
                                Some(TreeNode::RegularFile(_, mode)) => Some(*mode),
                                _ => None,
                            };
                            let (mode, mode_conflict) =
                                merge_mode(base_mode, *mine_mode, *theirs_mode);
                            let text = !mine_bytes.contains(&0)
                                && !theirs_bytes.contains(&0)
                                && !base_bytes.contains(&0);
                            let (bytes, has_conflict) =
                                if mine_bytes == theirs_bytes || theirs_bytes == base_bytes {
                                    (mine_bytes.clone(), false)
                                } else if mine_bytes == base_bytes {
                                    (theirs_bytes.clone(), false)
                                } else if text {
                                    match text_merge.merge(
                                        mine_bytes,
                                        base_bytes,
                                        theirs_bytes,
                                        &path,
                                        limits.max_bytes,
                                    )? {
                                        ForkPullTextMergeResult::Clean(bytes) => (bytes, false),
                                        ForkPullTextMergeResult::Conflicts(bytes) => (bytes, true),
                                    }
                                } else {
                                    (mine_bytes.clone(), true)
                                };
                            if bytes.len() as u64 > limits.max_bytes
                                || (bytes.is_empty()
                                    && (!mine_bytes.is_empty()
                                        || !base_bytes.is_empty()
                                        || !theirs_bytes.is_empty()))
                            {
                                return Err("Pull text merge returned invalid output".into());
                            }
                            conflict = has_conflict || mode_conflict;
                            if !has_conflict {
                                result.merged.push(path.to_string_lossy().into_owned());
                            }
                            Some(TreeNode::RegularFile(bytes, mode))
                        }
                        _ => {
                            conflict = true;
                            Some(mine.clone())
                        }
                    }
                }
                (None, Some(mine), None) => Some(mine.clone()),
                (None, None, Some(theirs)) => Some(theirs.clone()),
                (_, None, None) => None,
            }
        };
        if conflict {
            result.conflicts.push(if path.as_os_str().is_empty() {
                ".".into()
            } else {
                path.to_string_lossy().into_owned()
            });
        }
        if let Some(node) = selected {
            if let TreeNode::RegularFile(bytes, _) = &node {
                output_bytes = output_bytes.saturating_add(bytes.len() as u64);
                if output_bytes > limits.max_bytes {
                    return Err("Pull merged output exceeds its byte limit".into());
                }
            }
            merged.insert(path, node);
        }
    }
    let retained = merged.keys().cloned().collect::<Vec<_>>();
    for path in retained {
        for ancestor in path.ancestors().skip(1) {
            if ancestor.as_os_str().is_empty() || merged.contains_key(ancestor) {
                continue;
            }
            let directory = mine
                .get(ancestor)
                .or_else(|| base.get(ancestor))
                .or_else(|| theirs.get(ancestor));
            match directory {
                Some(TreeNode::Directory(mode)) => {
                    merged.insert(ancestor.to_path_buf(), TreeNode::Directory(*mode));
                    let conflict_path = ancestor.to_string_lossy().into_owned();
                    if !result.conflicts.contains(&conflict_path) {
                        result.conflicts.push(conflict_path);
                    }
                }
                _ => {
                    return Err(format!(
                        "Pull cannot preserve the parent of {}",
                        path.display()
                    ))
                }
            }
        }
    }
    Ok((merged, result))
}

fn write_tree(root: &Path, nodes: &BTreeMap<PathBuf, TreeNode>) -> Result<(), String> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
        Ok(_) => return Err("Pull merge output path is occupied; preserving it".into()),
    }
    fs::create_dir(root).map_err(|error| error.to_string())?;
    let root_mode = match nodes.get(Path::new("")) {
        Some(TreeNode::Directory(mode)) => *mode,
        _ => return Err("Pull merge output has no root directory".into()),
    };
    let mut directory_modes = Vec::new();
    for (path, node) in nodes {
        if path.as_os_str().is_empty() {
            continue;
        }
        if path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err("Pull merge output path is invalid".into());
        }
        let destination = root.join(path);
        let parent = destination.parent().ok_or("Pull output has no parent")?;
        let parent_meta = fs::symlink_metadata(parent).map_err(|error| error.to_string())?;
        if !parent_meta.is_dir() || parent_meta.file_type().is_symlink() {
            return Err("Pull output parent changed or is not a directory".into());
        }
        match node {
            TreeNode::Directory(mode) => {
                fs::create_dir(&destination).map_err(|error| error.to_string())?;
                directory_modes.push((destination, *mode));
            }
            TreeNode::RegularFile(bytes, mode) => {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&destination)
                    .map_err(|error| error.to_string())?;
                file.write_all(bytes).map_err(|error| error.to_string())?;
                file.set_permissions(fs::Permissions::from_mode(*mode))
                    .map_err(|error| error.to_string())?;
                file.sync_all().map_err(|error| error.to_string())?;
            }
            TreeNode::Symlink(target) => {
                std::os::unix::fs::symlink(target, &destination)
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    directory_modes.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (directory, mode) in directory_modes {
        fs::set_permissions(directory, fs::Permissions::from_mode(mode))
            .map_err(|error| error.to_string())?;
    }
    fs::set_permissions(root, fs::Permissions::from_mode(root_mode))
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn verify_preparation(
    preparation: &ForkPullPreparation,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    if preparation.version != 1
        || preparation.preparation_root
            != preparation
                .app_data
                .join("skill-studio/pull-preparations")
                .join(&preparation.id)
    {
        return Err("Invalid Pull preparation".into());
    }
    let root =
        BackupStateRoot::bind(&preparation.preparation_root).map_err(|error| error.to_string())?;
    let inputs = root
        .open_existing("inputs")
        .map_err(|error| error.to_string())?;
    for (name, identity) in [
        ("live", &preparation.live_identity),
        ("base", &preparation.base_identity),
    ] {
        inputs
            .verify_entry(OsStr::new(name), identity, limits, cancellation)
            .map_err(|error| error.to_string())?;
    }
    inputs
        .verify_file("registry.json", &preparation.registry_before)
        .map_err(|error| error.to_string())
}

fn read_preparation_tree(preparation: &ForkPullPreparation, name: &str) -> PathBuf {
    preparation
        .preparation_root
        .join("backups/inputs")
        .join(name)
}

fn current_registry(lease: &FinalizedWriteLease<'_>, path: &Path) -> Result<Vec<u8>, String> {
    lease
        .read_current_ownership_registry(path, MAX_REGISTRY_BYTES)?
        .ok_or_else(|| "Fork registry is missing".into())
}

fn revalidate_originals(
    lease: &FinalizedWriteLease<'_>,
    preparation: &ForkPullPreparation,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    if tree_identity(&preparation.live_path, limits, cancellation)? != preparation.live_identity
        || tree_identity(&preparation.base_path, limits, cancellation)? != preparation.base_identity
        || current_registry(lease, &preparation.registry_path)? != preparation.registry_before
    {
        return Err("Fork Pull inputs changed after preparation".into());
    }
    lease.revalidate().map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
fn copy_evidence(
    store: &EventStore,
    preparation: &ForkPullPreparation,
    upstream_path: &Path,
    candidate_path: &Path,
    upstream_identity: &str,
    candidate_identity: &str,
    lease: &FinalizedWriteLease<'_>,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    let state = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
    let evidence = state
        .reserve(&preparation.id)
        .map_err(|error| error.to_string())?;
    let result = (|| {
        for (path, target, expected) in [
            (
                preparation.live_path.as_path(),
                "live-before",
                preparation.live_identity.as_str(),
            ),
            (
                preparation.base_path.as_path(),
                "base-before",
                preparation.base_identity.as_str(),
            ),
            (upstream_path, "upstream", upstream_identity),
            (candidate_path, "candidate", candidate_identity),
            (candidate_path, "candidate-proof", candidate_identity),
        ] {
            let selected = source(path)?;
            let report = evidence
                .copy_entry(
                    &selected.directory,
                    &selected.name,
                    OsStr::new(target),
                    limits,
                    cancellation,
                )
                .map_err(|error| error.to_string())?;
            if report.tree_identity != expected {
                return Err("Fork Pull candidate evidence changed while copied".into());
            }
        }
        evidence
            .write_new_file("registry.json", &preparation.registry_before)
            .map_err(|error| error.to_string())?;
        revalidate_originals(lease, preparation, limits, cancellation)?;
        evidence.revalidate().map_err(|error| error.to_string())
    })();
    if let Err(error) = result {
        return match evidence.discard() {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!(
                "{error}; could not remove unjournaled Pull evidence: {cleanup}"
            )),
        };
    }
    Ok(())
}

fn unjournaled_error(store: &EventStore, id: &str, message: impl Into<String>) -> ForkPullError {
    let message = message.into();
    let cleanup =
        BackupStateRoot::bind(&store.app_data).and_then(|root| root.open_existing(id)?.discard());
    ForkPullError {
        event_id: None,
        recovery_required: false,
        message: match cleanup {
            Ok(()) => message,
            Err(error) => format!("{message}; could not remove unjournaled Pull evidence: {error}"),
        },
    }
}

#[allow(clippy::too_many_arguments)]
pub fn commit_fork_pull(
    service: &mut ScopedSkillService,
    store: &EventStore,
    preparation: ForkPullPreparation,
    upstream_path: &Path,
    text_merge: &dyn ForkPullTextMerge,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<ForkPullResult, ForkPullError> {
    commit_fork_pull_with_recovery_hook(
        service,
        store,
        preparation,
        upstream_path,
        text_merge,
        limits,
        timeout,
        cancellation,
        &mut |_| Ok(()),
    )
}

#[allow(clippy::too_many_arguments)]
fn commit_fork_pull_with_recovery_hook(
    service: &mut ScopedSkillService,
    store: &EventStore,
    preparation: ForkPullPreparation,
    upstream_path: &Path,
    text_merge: &dyn ForkPullTextMerge,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
    recovery_hook: &mut dyn FnMut(PullCheckpoint) -> Result<(), String>,
) -> Result<ForkPullResult, ForkPullError> {
    struct PreparationCleanup<'a> {
        preparation: &'a ForkPullPreparation,
        armed: bool,
    }
    impl Drop for PreparationCleanup<'_> {
        fn drop(&mut self) {
            if self.armed {
                let _ = discard_preparation(self.preparation);
            }
        }
    }
    let before_error = |message| ForkPullError {
        event_id: None,
        recovery_required: false,
        message,
    };
    if preparation.app_data != store.app_data {
        return Err(before_error(
            "Pull preparation belongs to another event store".into(),
        ));
    }
    let mut preparation_cleanup = PreparationCleanup {
        preparation: &preparation,
        armed: true,
    };
    verify_preparation(&preparation, limits, &cancellation).map_err(before_error)?;
    let base = walk_tree(
        &read_preparation_tree(&preparation, "base"),
        limits,
        &cancellation,
    )
    .map_err(before_error)?;
    let mine = walk_tree(
        &read_preparation_tree(&preparation, "live"),
        limits,
        &cancellation,
    )
    .map_err(before_error)?;
    let upstream_identity =
        tree_identity(upstream_path, limits, &cancellation).map_err(before_error)?;
    let theirs = walk_tree(upstream_path, limits, &cancellation).map_err(before_error)?;
    if tree_identity(upstream_path, limits, &cancellation).map_err(before_error)?
        != upstream_identity
    {
        return Err(before_error(
            "Upstream Pull input changed while it was read".into(),
        ));
    }
    let (merged, result) = merge_nodes(
        &base,
        &mine,
        &theirs,
        &preparation.record_before.base_commit,
        &preparation.request.to_commit,
        limits,
        text_merge,
    )
    .map_err(before_error)?;
    let candidate_path = preparation.preparation_root.join("merged");
    write_tree(&candidate_path, &merged).map_err(before_error)?;
    let candidate_identity =
        tree_identity(&candidate_path, limits, &cancellation).map_err(before_error)?;
    let lease = prepare_lease(
        service,
        store,
        &preparation.request,
        &preparation.name,
        &preparation.live_path,
        &preparation.base_path,
        &preparation.registry_path,
        Some(&candidate_path),
        std::slice::from_ref(&preparation.request.expected_owner_revision),
        timeout,
        cancellation.clone(),
    )
    .map_err(before_error)?;
    revalidate_originals(&lease, &preparation, limits, &cancellation).map_err(before_error)?;
    copy_evidence(
        store,
        &preparation,
        upstream_path,
        &candidate_path,
        &upstream_identity,
        &candidate_identity,
        &lease,
        limits,
        &cancellation,
    )
    .map_err(|message| unjournaled_error(store, &preparation.id, message))?;
    revalidate_originals(&lease, &preparation, limits, &cancellation)
        .map_err(|message| unjournaled_error(store, &preparation.id, message))?;
    let transition = ForkPullRegistryTransition::new(
        preparation.name.clone(),
        preparation.record_before.clone(),
        preparation.record_after.clone(),
        &preparation.registry_before,
    )
    .map_err(|message| unjournaled_error(store, &preparation.id, message))?;
    let registry_after = transition
        .apply_document(&preparation.registry_before)
        .map_err(|message| unjournaled_error(store, &preparation.id, message))?;
    let intent = ForkPullIntent {
        version: 1,
        scope: preparation.scope.clone(),
        app_data: preparation.app_data.clone(),
        request: preparation.request.clone(),
        name: preparation.name.clone(),
        live_path: preparation.live_path.clone(),
        base_path: preparation.base_path.clone(),
        registry_path: preparation.registry_path.clone(),
        record_before: preparation.record_before.clone(),
        record_after: preparation.record_after.clone(),
        live_before: preparation.live_identity.clone(),
        live_after: candidate_identity,
        base_before: preparation.base_identity.clone(),
        base_after: upstream_identity,
        registry_before: preparation.registry_before.clone(),
        registry_after,
        result: result.clone(),
    };
    let draft = intent
        .draft(&preparation.id)
        .map_err(|message| unjournaled_error(store, &preparation.id, message))?;
    let guarded = GuardedEventStore::bind(store, &lease)
        .map_err(|message| unjournaled_error(store, &preparation.id, message))?;
    if let Err(error) = guarded.record_pending(&lease, &preparation.id, draft) {
        return match error {
            EventWriteFailure::MayHaveWritten(message) => Err(ForkPullError {
                event_id: Some(preparation.id.clone()),
                recovery_required: true,
                message,
            }),
            EventWriteFailure::CancelledBeforeWrite | EventWriteFailure::BeforeWrite(_) => {
                Err(unjournaled_error(store, &preparation.id, error.to_string()))
            }
        };
    }
    preparation_cleanup.armed = false;
    drop(preparation_cleanup);
    drop(lease);
    let row = store
        .get(&preparation.id)
        .map_err(|message| ForkPullError {
            event_id: Some(preparation.id.clone()),
            recovery_required: true,
            message,
        })?
        .ok_or_else(|| ForkPullError {
            event_id: Some(preparation.id.clone()),
            recovery_required: true,
            message: "Recorded Fork Pull event is missing".into(),
        })?;
    let forward = recover_fork_pull_with_hook(service, store, &row, limits, timeout, recovery_hook);
    if forward.is_ok() {
        let _ = discard_preparation(&preparation);
    }
    forward.map(|_| result).map_err(|message| ForkPullError {
        event_id: Some(preparation.id),
        recovery_required: true,
        message,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PullCheckpoint {
    IntentRecorded,
    LivePublished,
    BasePublished,
    RegistryPublished,
}

fn pending(
    store: &EventStore,
    lease: &FinalizedWriteLease<'_>,
    row: &EventRow,
    intent: &ForkPullIntent,
) -> Result<(), String> {
    let current = GuardedEventStore::bind(store, lease)?
        .next_recovery_event(lease)?
        .ok_or("Fork Pull event is no longer pending")?;
    if current.id != row.id || ForkPullIntent::from_row(&current)? != *intent {
        return Err("Fork Pull event changed or is out of order".into());
    }
    Ok(())
}

fn evidence_path(store: &EventStore, event_id: &str, name: &str) -> PathBuf {
    store.app_data.join("backups").join(event_id).join(name)
}

fn verify_evidence(
    store: &EventStore,
    row: &EventRow,
    intent: &ForkPullIntent,
    lease: &FinalizedWriteLease<'_>,
    limits: BackupCopyLimits,
) -> Result<(), String> {
    pending(store, lease, row, intent)?;
    let state = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
    let evidence = state
        .open_existing(&row.id)
        .map_err(|error| error.to_string())?;
    for (name, identity) in [
        ("live-before", &intent.live_before),
        ("base-before", &intent.base_before),
        ("upstream", &intent.base_after),
        ("candidate-proof", &intent.live_after),
    ] {
        evidence
            .verify_entry(
                OsStr::new(name),
                identity,
                limits,
                &CancellationToken::default(),
            )
            .map_err(|error| error.to_string())?;
    }
    evidence
        .verify_file("registry.json", &intent.registry_before)
        .map_err(|error| error.to_string())?;
    lease.revalidate().map_err(|error| error.to_string())
}

fn observe_checkpoint(
    store: &EventStore,
    row: &EventRow,
    intent: &ForkPullIntent,
    lease: &FinalizedWriteLease<'_>,
    limits: BackupCopyLimits,
) -> Result<PullCheckpoint, String> {
    verify_evidence(store, row, intent, lease, limits)?;
    let cancellation = CancellationToken::default();
    let live = tree_identity(&intent.live_path, limits, &cancellation)?;
    let base_published = base_is_published(row, intent, limits)?;
    let candidate = tree_identity(
        &evidence_path(store, &row.id, "candidate"),
        limits,
        &cancellation,
    )?;
    let registry = current_registry(lease, &intent.registry_path)?;
    let live_before = live == intent.live_before && candidate == intent.live_after;
    let live_after = live == intent.live_after
        && (candidate == intent.live_before || intent.live_before == intent.live_after);
    let checkpoint = if live_after && base_published && registry == intent.registry_after {
        PullCheckpoint::RegistryPublished
    } else if live_after && base_published && registry == intent.registry_before {
        PullCheckpoint::BasePublished
    } else if live_after && !base_published && registry == intent.registry_before {
        PullCheckpoint::LivePublished
    } else if live_before && !base_published && registry == intent.registry_before {
        PullCheckpoint::IntentRecorded
    } else {
        return Err("Fork Pull recovery found an invalid or changed effect prefix".into());
    };
    lease.revalidate().map_err(|error| error.to_string())?;
    Ok(checkpoint)
}

fn optional_tree_identity(
    path: &Path,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<Option<String>, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            tree_identity(path, limits, cancellation).map(Some)
        }
        Ok(_) => Err("Fork base entry is not an independent directory".into()),
    }
}

fn base_is_published(
    row: &EventRow,
    intent: &ForkPullIntent,
    limits: BackupCopyLimits,
) -> Result<bool, String> {
    let cancellation = CancellationToken::default();
    let base = optional_tree_identity(&intent.base_path, limits, &cancellation)?;
    let journal = optional_tree_identity(
        &intent
            .base_path
            .parent()
            .ok_or("Fork base has no parent")?
            .join(format!(".previous-{}", row.id)),
        limits,
        &cancellation,
    )?;
    if base.as_deref() == Some(&intent.base_after) {
        if intent.base_before != intent.base_after
            && journal.as_deref() != Some(&intent.base_before)
        {
            return Err("Published Fork base has no matching prior-base journal".into());
        }
        return Ok(true);
    }
    match (base.as_deref(), journal.as_deref()) {
        (Some(base), None) if base == intent.base_before => Ok(false),
        (None, Some(journal)) if journal == intent.base_before => Ok(false),
        _ => Err("Fork Pull base or prior-base journal changed".into()),
    }
}

fn recovery_lease<'a>(
    service: &'a mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    intent: &ForkPullIntent,
    timeout: Option<Duration>,
) -> Result<FinalizedWriteLease<'a>, String> {
    let scope = service.scope();
    if scope.home != intent.scope.home
        || scope.backing_roots != intent.scope.backing_roots
        || scope.plugin_ownership_roots != intent.scope.plugin_ownership_roots
        || store.app_data != intent.app_data
    {
        return Err("Fork Pull recovery scope changed".into());
    }
    let after_revision = RegistryOwnerRecord::Fork(&intent.record_after)
        .revision()
        .ok_or("Fork Pull after-owner revision is unavailable")?;
    let allowed_revisions = vec![
        intent.request.expected_owner_revision.clone(),
        after_revision,
    ];
    prepare_lease(
        service,
        store,
        &intent.request,
        &intent.name,
        &intent.live_path,
        &intent.base_path,
        &intent.registry_path,
        Some(&evidence_path(store, &row.id, "candidate")),
        &allowed_revisions,
        timeout,
        CancellationToken::default(),
    )
}

fn publish_live(
    store: &EventStore,
    row: &EventRow,
    intent: &ForkPullIntent,
    lease: &FinalizedWriteLease<'_>,
    limits: BackupCopyLimits,
) -> Result<(), String> {
    if intent.live_before == intent.live_after {
        return Ok(());
    }
    let candidate = source(&evidence_path(store, &row.id, "candidate"))?;
    let live = source(&intent.live_path)?;
    candidate
        .exchange_verified_tree(
            &live,
            &intent.live_after,
            &intent.live_before,
            lease,
            limits,
            &CancellationToken::default(),
        )
        .map_err(|error| match error {
            TreeExchangeFailure::BeforeExchange(message)
            | TreeExchangeFailure::MayHaveExchanged(message) => message,
        })
}

fn publish_base(
    store: &EventStore,
    row: &EventRow,
    intent: &ForkPullIntent,
    lease: &FinalizedWriteLease<'_>,
    limits: BackupCopyLimits,
) -> Result<(), String> {
    BackupStateRoot::bind(&store.app_data)
        .map_err(|error| error.to_string())?
        .publish_verified_fork_base(
            lease,
            &intent.name,
            &row.id,
            &intent.base_after,
            Some(&intent.base_before),
            limits,
            &CancellationToken::default(),
            || pending(store, lease, row, intent),
            || Ok(()),
        )
        .map(|_| ())
}

fn publish_registry(
    intent: &ForkPullIntent,
    lease: &mut FinalizedWriteLease<'_>,
) -> Result<(), String> {
    SkillRegistryTarget::bind(
        intent
            .registry_path
            .parent()
            .ok_or("Fork registry has no parent")?,
    )?
    .replace(lease, &intent.registry_before, &intent.registry_after)
    .map_err(|error| error.to_string())
}

pub fn recover_fork_pull(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<(), String> {
    recover_fork_pull_with_hook(service, store, row, limits, timeout, &mut |_| Ok(()))
}

fn recover_fork_pull_with_hook(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    hook: &mut dyn FnMut(PullCheckpoint) -> Result<(), String>,
) -> Result<(), String> {
    let intent = ForkPullIntent::from_row(row)?;
    loop {
        let mut lease = recovery_lease(service, store, row, &intent, timeout)?;
        let checkpoint = observe_checkpoint(store, row, &intent, &lease, limits)?;
        hook(checkpoint)?;
        match checkpoint {
            PullCheckpoint::IntentRecorded => {
                publish_live(store, row, &intent, &lease, limits)?;
                drop(lease);
            }
            PullCheckpoint::LivePublished => {
                publish_base(store, row, &intent, &lease, limits)?;
                drop(lease);
            }
            PullCheckpoint::BasePublished => {
                publish_registry(&intent, &mut lease)?;
                drop(lease);
            }
            PullCheckpoint::RegistryPublished => {
                pending(store, &lease, row, &intent)?;
                GuardedEventStore::bind(store, &lease)?
                    .finish_recovery_snapshot(&lease, row, EventStatus::Done, None)
                    .map_err(|error| error.to_string())?;
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        skill_deployment::deployment_id,
        skill_fork_registry::{OriginTool, RegistryOwnerRecord},
    };
    use std::os::unix::fs::{symlink, PermissionsExt};

    const LIMITS: BackupCopyLimits = BackupCopyLimits {
        max_bytes: 1024 * 1024,
        max_entries: 256,
        max_depth: 16,
    };
    const TIMEOUT: Option<Duration> = Some(Duration::from_secs(5));

    struct Merge;
    impl ForkPullTextMerge for Merge {
        fn merge(
            &self,
            mine: &[u8],
            _base: &[u8],
            theirs: &[u8],
            _relative_path: &Path,
            _max_output: u64,
        ) -> Result<ForkPullTextMergeResult, String> {
            let mut output = b"<<<<<<< mine\n".to_vec();
            output.extend_from_slice(mine);
            output.extend_from_slice(b"\n=======\n");
            output.extend_from_slice(theirs);
            output.extend_from_slice(b"\n>>>>>>> theirs\n");
            Ok(ForkPullTextMergeResult::Conflicts(output))
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        scope: SkillScope,
        source: PathBuf,
        base: PathBuf,
        request: ForkPullRequest,
    }

    impl Fixture {
        fn new(origin_tool: OriginTool, legacy: bool) -> Self {
            Self::with_temp(origin_tool, legacy, tempfile::tempdir().unwrap())
        }

        fn with_temp(origin_tool: OriginTool, legacy: bool, temp: tempfile::TempDir) -> Self {
            let root = temp.path().canonicalize().unwrap();
            let home = root.join("home");
            let source = home.join(".agents/skills/sample");
            fs::create_dir_all(&source).unwrap();
            fs::write(source.join("SKILL.md"), "base\n").unwrap();
            let app_data = home.join("Library/Application Support/com.skillstudio.app");
            let base = app_data.join("skill-studio/forks/sample/base");
            fs::create_dir_all(&base).unwrap();
            fs::write(base.join("SKILL.md"), "base\n").unwrap();
            let id = deployment_id(
                "sample",
                "global",
                SkillDestination::Universal,
                "universal",
                None,
                &source,
            );
            let record = ForkRecord {
                deployment_id: if legacy { String::new() } else { id.clone() },
                skill_dir: if legacy {
                    PathBuf::new()
                } else {
                    source.clone()
                },
                forked_at: "2026-09-16T00:00:00Z".into(),
                origin_tool,
                origin_source: "owner/repo".into(),
                repo: "owner/repo".into(),
                path: "skills/sample".into(),
                declared_ref: (origin_tool == OriginTool::Dotagents).then(|| "main".into()),
                base_commit: "a".repeat(40),
            };
            let mut selected = serde_json::to_value(&record).unwrap();
            selected["future_row"] = serde_json::json!("preserve");
            let sibling_dir = home.join(".agents/skills/sibling");
            let sibling = ForkRecord {
                deployment_id: deployment_id(
                    "sibling",
                    "global",
                    SkillDestination::Universal,
                    "universal",
                    None,
                    &sibling_dir,
                ),
                skill_dir: sibling_dir,
                forked_at: "2026-09-16T00:00:00Z".into(),
                origin_tool: OriginTool::Dotagents,
                origin_source: "owner/sibling".into(),
                repo: "owner/sibling".into(),
                path: "skills/sibling".into(),
                declared_ref: None,
                base_commit: "c".repeat(40),
            };
            let mut sibling = serde_json::to_value(sibling).unwrap();
            sibling["future_only"] = serde_json::json!(true);
            let registry = serde_json::json!({
                "version": 4,
                "future_root": {"preserve": true},
                "forks": {
                    "sample": selected,
                    "sibling": sibling
                }
            });
            fs::write(
                home.join(".agents/skill-studio.json"),
                serde_json::to_vec(&registry).unwrap(),
            )
            .unwrap();
            let scope = SkillScope {
                home,
                projects: vec![],
                backing_roots: vec![],
                plugin_ownership_roots: vec![],
            };
            let request = ForkPullRequest {
                deployment_id: id,
                expected_owner_revision: RegistryOwnerRecord::Fork(&record).revision().unwrap(),
                to_commit: "b".repeat(40),
            };
            Self {
                _temp: temp,
                root,
                scope,
                source,
                base,
                request,
            }
        }

        fn store(&self) -> EventStore {
            EventStore::open(
                &self
                    .scope
                    .home
                    .join("Library/Application Support/com.skillstudio.app"),
            )
            .unwrap()
        }

        fn service(&self) -> ScopedSkillService {
            ScopedSkillService::bind(self.scope.clone()).unwrap()
        }

        fn prepare(&self, store: &EventStore) -> ForkPullPreparation {
            match prepare_fork_pull_inputs(
                &mut self.service(),
                store,
                &self.request,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
            )
            .unwrap()
            {
                ForkPullPreparationOutcome::Prepared(preparation) => preparation,
                ForkPullPreparationOutcome::UpToDate(_) => panic!("fixture must need Pull"),
            }
        }
    }

    #[test]
    fn merge_policy_preserves_links_modes_empty_directories_and_delete_conflicts() {
        let root = PathBuf::new();
        let file = PathBuf::from("file");
        let link = PathBuf::from("link");
        let empty = PathBuf::from("empty");
        let deleted = PathBuf::from("deleted");
        let removed = PathBuf::from("removed");
        let local = PathBuf::from("gone/local");
        let mut base = BTreeMap::from([
            (root.clone(), TreeNode::Directory(0o755)),
            (file.clone(), TreeNode::RegularFile(b"base".to_vec(), 0o644)),
            (link.clone(), TreeNode::Symlink(PathBuf::from("old"))),
            (
                deleted.clone(),
                TreeNode::RegularFile(b"base".to_vec(), 0o644),
            ),
            (
                removed.clone(),
                TreeNode::RegularFile(b"base".to_vec(), 0o644),
            ),
            (PathBuf::from("gone"), TreeNode::Directory(0o755)),
        ]);
        base.insert(empty.clone(), TreeNode::Directory(0o755));
        let mut mine = base.clone();
        mine.insert(file.clone(), TreeNode::RegularFile(b"mine".to_vec(), 0o644));
        mine.remove(&deleted);
        mine.insert(
            removed.clone(),
            TreeNode::RegularFile(b"mine".to_vec(), 0o644),
        );
        mine.insert(
            local.clone(),
            TreeNode::RegularFile(b"local".to_vec(), 0o600),
        );
        let mut theirs = base.clone();
        theirs.insert(file.clone(), TreeNode::RegularFile(b"base".to_vec(), 0o755));
        theirs.insert(link.clone(), TreeNode::Symlink(PathBuf::from("new")));
        theirs.insert(
            deleted.clone(),
            TreeNode::RegularFile(b"theirs".to_vec(), 0o644),
        );
        theirs.remove(&removed);
        theirs.remove(Path::new("gone"));
        let (merged, result) = merge_nodes(
            &base,
            &mine,
            &theirs,
            &"a".repeat(40),
            &"b".repeat(40),
            LIMITS,
            &Merge,
        )
        .unwrap();
        assert_eq!(
            merged[&file],
            TreeNode::RegularFile(b"mine".to_vec(), 0o755)
        );
        assert_eq!(merged[&link], TreeNode::Symlink(PathBuf::from("new")));
        assert_eq!(merged[&empty], TreeNode::Directory(0o755));
        assert_eq!(
            merged[&deleted],
            TreeNode::RegularFile(b"theirs".to_vec(), 0o644)
        );
        assert_eq!(
            merged[&removed],
            TreeNode::RegularFile(b"mine".to_vec(), 0o644)
        );
        assert_eq!(
            merged[&local],
            TreeNode::RegularFile(b"local".to_vec(), 0o600)
        );
        assert!(merged.contains_key(Path::new("gone")));
        for conflict in ["deleted", "removed", "gone"] {
            assert!(
                result.conflicts.contains(&conflict.to_string()),
                "{conflict}"
            );
        }
    }

    #[test]
    fn merge_accepts_unilateral_type_transitions_and_labels_root_mode_conflicts() {
        let root = PathBuf::new();
        let path = PathBuf::from("entry");
        let base = BTreeMap::from([
            (root.clone(), TreeNode::Directory(0o755)),
            (path.clone(), TreeNode::RegularFile(b"base".to_vec(), 0o644)),
        ]);

        let mine = base.clone();
        let mut theirs = base.clone();
        theirs.insert(path.clone(), TreeNode::Symlink(PathBuf::from("target")));
        let (merged, result) = merge_nodes(
            &base,
            &mine,
            &theirs,
            &"a".repeat(40),
            &"b".repeat(40),
            LIMITS,
            &Merge,
        )
        .unwrap();
        assert_eq!(merged[&path], TreeNode::Symlink(PathBuf::from("target")));
        assert!(result.conflicts.is_empty());

        let mut mine = base.clone();
        mine.insert(path.clone(), TreeNode::Directory(0o750));
        let (merged, result) = merge_nodes(
            &base,
            &mine,
            &base,
            &"a".repeat(40),
            &"b".repeat(40),
            LIMITS,
            &Merge,
        )
        .unwrap();
        assert_eq!(merged[&path], TreeNode::Directory(0o750));
        assert!(result.conflicts.is_empty());

        let mut theirs = base.clone();
        theirs.insert(path.clone(), TreeNode::Symlink(PathBuf::from("target")));
        assert!(merge_nodes(
            &base,
            &mine,
            &theirs,
            &"a".repeat(40),
            &"b".repeat(40),
            LIMITS,
            &Merge,
        )
        .unwrap_err()
        .contains("type collision"));

        let base = BTreeMap::from([(root.clone(), TreeNode::Directory(0o755))]);
        let mine = BTreeMap::from([(root.clone(), TreeNode::Directory(0o700))]);
        let theirs = BTreeMap::from([(root.clone(), TreeNode::Directory(0o750))]);
        let (merged, result) = merge_nodes(
            &base,
            &mine,
            &theirs,
            &"a".repeat(40),
            &"b".repeat(40),
            LIMITS,
            &Merge,
        )
        .unwrap();
        assert_eq!(merged[&root], TreeNode::Directory(0o700));
        assert_eq!(result.conflicts, vec!["."]);
    }

    #[test]
    fn write_tree_populates_read_only_directories_before_applying_their_modes() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output");
        let nodes = BTreeMap::from([
            (PathBuf::new(), TreeNode::Directory(0o500)),
            (PathBuf::from("locked"), TreeNode::Directory(0o500)),
            (
                PathBuf::from("locked/file"),
                TreeNode::RegularFile(b"content".to_vec(), 0o400),
            ),
        ]);
        write_tree(&output, &nodes).unwrap();
        assert_eq!(fs::read(output.join("locked/file")).unwrap(), b"content");
        assert_eq!(
            fs::metadata(output.join("locked"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o500
        );
    }

    #[test]
    fn pull_completes_for_both_origins_and_legacy_binding_while_preserving_unknown_registry_data() {
        for origin in [OriginTool::Dotagents, OriginTool::SkillsSh] {
            for legacy in [false, true] {
                let fixture = Fixture::new(origin, legacy);
                let store = fixture.store();
                let preparation = fixture.prepare(&store);
                let upstream = fixture.root.join("upstream");
                fs::create_dir(&upstream).unwrap();
                fs::write(upstream.join("SKILL.md"), "upstream\n").unwrap();
                fs::create_dir(upstream.join("empty")).unwrap();
                symlink("missing-target", upstream.join("dangling")).unwrap();
                let result = commit_fork_pull(
                    &mut fixture.service(),
                    &store,
                    preparation,
                    &upstream,
                    &Merge,
                    LIMITS,
                    TIMEOUT,
                    CancellationToken::default(),
                )
                .unwrap();
                assert!(result.conflicts.is_empty());
                assert_eq!(
                    fs::read(fixture.source.join("SKILL.md")).unwrap(),
                    b"upstream\n"
                );
                assert_eq!(
                    fs::read(fixture.base.join("SKILL.md")).unwrap(),
                    b"upstream\n"
                );
                assert_eq!(
                    fs::read_link(fixture.source.join("dangling")).unwrap(),
                    PathBuf::from("missing-target")
                );
                let registry: Value = serde_json::from_slice(
                    &fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
                )
                .unwrap();
                assert_eq!(registry["future_root"]["preserve"], true);
                assert_eq!(registry["forks"]["sample"]["future_row"], "preserve");
                assert_eq!(registry["forks"]["sample"]["base_commit"], "b".repeat(40));
                assert_eq!(registry["forks"]["sibling"]["future_only"], true);
                let event = store
                    .get(&store.list(1, None).unwrap()[0].id)
                    .unwrap()
                    .unwrap();
                assert_eq!(event.status, "done");
                assert!(!event.restorable);
                assert!(event.inverse.is_none());
            }
        }
    }

    #[test]
    fn stale_live_input_refuses_before_event_and_preserves_all_originals() {
        let fixture = Fixture::new(OriginTool::Dotagents, false);
        let store = fixture.store();
        let preparation = fixture.prepare(&store);
        fs::write(fixture.source.join("SKILL.md"), "replacement\n").unwrap();
        let upstream = fixture.root.join("upstream");
        fs::create_dir(&upstream).unwrap();
        fs::write(upstream.join("SKILL.md"), "upstream\n").unwrap();
        let error = commit_fork_pull(
            &mut fixture.service(),
            &store,
            preparation,
            &upstream,
            &Merge,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap_err();
        assert!(error.event_id.is_none());
        assert!(!error.recovery_required);
        assert_eq!(
            fs::read(fixture.source.join("SKILL.md")).unwrap(),
            b"replacement\n"
        );
        assert_eq!(fs::read(fixture.base.join("SKILL.md")).unwrap(), b"base\n");
        assert!(store.list(10, None).unwrap().is_empty());
    }

    #[test]
    fn legacy_pull_scratch_is_preserved_and_refused_before_preparation() {
        let fixture = Fixture::new(OriginTool::SkillsSh, false);
        let store = fixture.store();
        let legacy = store
            .app_data
            .join("skill-studio/forks/sample/staging-live");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("evidence"), "preserve").unwrap();
        let error = prepare_fork_pull_inputs(
            &mut fixture.service(),
            &store,
            &fixture.request,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap_err();
        assert!(error.contains("Unresolved legacy Pull recovery material"));
        assert_eq!(fs::read(legacy.join("evidence")).unwrap(), b"preserve");
        assert_eq!(
            fs::read(fixture.source.join("SKILL.md")).unwrap(),
            b"base\n"
        );
    }

    #[test]
    fn every_durable_prefix_recovers_without_replaying_fetch_or_merge() {
        for stopped_at in [
            PullCheckpoint::IntentRecorded,
            PullCheckpoint::LivePublished,
            PullCheckpoint::BasePublished,
            PullCheckpoint::RegistryPublished,
        ] {
            let fixture = Fixture::new(OriginTool::Dotagents, false);
            let store = fixture.store();
            let preparation = fixture.prepare(&store);
            let upstream = fixture.root.join("upstream");
            fs::create_dir(&upstream).unwrap();
            fs::write(upstream.join("SKILL.md"), "upstream\n").unwrap();
            let mut interrupt = |checkpoint| {
                if checkpoint == stopped_at {
                    Err(format!("interrupt at {checkpoint:?}"))
                } else {
                    Ok(())
                }
            };
            let error = commit_fork_pull_with_recovery_hook(
                &mut fixture.service(),
                &store,
                preparation,
                &upstream,
                &Merge,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
                &mut interrupt,
            )
            .unwrap_err();
            assert!(error.recovery_required);
            let event_id = error.event_id.unwrap();
            let row = store.get(&event_id).unwrap().unwrap();
            assert_eq!(row.status, "pending");

            if stopped_at == PullCheckpoint::LivePublished {
                fs::rename(
                    &fixture.base,
                    fixture
                        .base
                        .parent()
                        .unwrap()
                        .join(format!(".previous-{event_id}")),
                )
                .unwrap();
            }

            let mut recovery_scope = fixture.scope.clone();
            if stopped_at == PullCheckpoint::RegistryPublished {
                let unrelated = fixture.root.join("unrelated-project");
                fs::create_dir(&unrelated).unwrap();
                recovery_scope.projects.push(unrelated);
            }
            recover_fork_pull(
                &mut ScopedSkillService::bind(recovery_scope).unwrap(),
                &store,
                &row,
                LIMITS,
                TIMEOUT,
            )
            .unwrap();
            assert_eq!(
                fs::read(fixture.source.join("SKILL.md")).unwrap(),
                b"upstream\n"
            );
            assert_eq!(
                fs::read(fixture.base.join("SKILL.md")).unwrap(),
                b"upstream\n"
            );
            let registry: Value = serde_json::from_slice(
                &fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(registry["forks"]["sample"]["base_commit"], "b".repeat(40));
            assert_eq!(store.get(&event_id).unwrap().unwrap().status, "done");
        }
    }

    #[test]
    fn changed_live_replacement_and_changed_candidate_evidence_remain_unresolved() {
        for tamper_candidate in [false, true] {
            let fixture = Fixture::new(OriginTool::SkillsSh, false);
            let store = fixture.store();
            let preparation = fixture.prepare(&store);
            let upstream = fixture.root.join("upstream");
            fs::create_dir(&upstream).unwrap();
            fs::write(upstream.join("SKILL.md"), "upstream\n").unwrap();
            let stopped_at = if tamper_candidate {
                PullCheckpoint::IntentRecorded
            } else {
                PullCheckpoint::LivePublished
            };
            let mut interrupt = |checkpoint| {
                (checkpoint != stopped_at)
                    .then_some(())
                    .ok_or_else(|| "interrupt".to_string())
            };
            let error = commit_fork_pull_with_recovery_hook(
                &mut fixture.service(),
                &store,
                preparation,
                &upstream,
                &Merge,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
                &mut interrupt,
            )
            .unwrap_err();
            let event_id = error.event_id.unwrap();
            let row = store.get(&event_id).unwrap().unwrap();
            if tamper_candidate {
                fs::write(
                    evidence_path(&store, &event_id, "candidate-proof").join("SKILL.md"),
                    "changed evidence\n",
                )
                .unwrap();
            } else {
                let published = fixture.root.join("published-live");
                fs::rename(&fixture.source, &published).unwrap();
                fs::create_dir(&fixture.source).unwrap();
                fs::write(fixture.source.join("SKILL.md"), "external replacement\n").unwrap();
            }
            let recovery = recover_fork_pull(&mut fixture.service(), &store, &row, LIMITS, TIMEOUT);
            assert!(recovery.is_err());
            assert_eq!(store.get(&event_id).unwrap().unwrap().status, "pending");
            if tamper_candidate {
                assert_eq!(
                    fs::read(fixture.source.join("SKILL.md")).unwrap(),
                    b"base\n"
                );
            } else {
                assert_eq!(
                    fs::read(fixture.source.join("SKILL.md")).unwrap(),
                    b"external replacement\n"
                );
            }
        }
    }

    #[test]
    fn equal_before_and_after_trees_advance_only_the_registry() {
        let fixture = Fixture::new(OriginTool::Dotagents, false);
        let store = fixture.store();
        let preparation = fixture.prepare(&store);
        let upstream = fixture.root.join("upstream");
        fs::create_dir(&upstream).unwrap();
        fs::write(upstream.join("SKILL.md"), "base\n").unwrap();
        commit_fork_pull(
            &mut fixture.service(),
            &store,
            preparation,
            &upstream,
            &Merge,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        assert_eq!(fs::read(fixture.base.join("SKILL.md")).unwrap(), b"base\n");
        let registry: Value = serde_json::from_slice(
            &fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(registry["forks"]["sample"]["base_commit"], "b".repeat(40));
        assert_eq!(store.list(10, None).unwrap()[0].status, "done");
    }

    #[test]
    fn up_to_date_pull_records_no_event() {
        let mut fixture = Fixture::new(OriginTool::Dotagents, false);
        let store = fixture.store();
        fixture.request.to_commit = "a".repeat(40);
        let outcome = prepare_fork_pull_inputs(
            &mut fixture.service(),
            &store,
            &fixture.request,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        assert!(matches!(outcome, ForkPullPreparationOutcome::UpToDate(_)));
        assert!(store.list(10, None).unwrap().is_empty());
    }

    #[test]
    fn binary_conflicts_keep_local_bytes_and_invalid_empty_text_output_is_refused() {
        struct EmptyMerge;
        impl ForkPullTextMerge for EmptyMerge {
            fn merge(
                &self,
                _mine: &[u8],
                _base: &[u8],
                _theirs: &[u8],
                _relative_path: &Path,
                _max_output: u64,
            ) -> Result<ForkPullTextMergeResult, String> {
                Ok(ForkPullTextMergeResult::Clean(Vec::new()))
            }
        }

        let root = PathBuf::new();
        let file = PathBuf::from("file");
        let trees = |bytes: &[u8]| {
            BTreeMap::from([
                (root.clone(), TreeNode::Directory(0o755)),
                (file.clone(), TreeNode::RegularFile(bytes.to_vec(), 0o644)),
            ])
        };
        let base = trees(b"base\0");
        let mine = trees(b"mine\0");
        let theirs = trees(b"theirs\0");
        let (merged, result) = merge_nodes(
            &base,
            &mine,
            &theirs,
            &"a".repeat(40),
            &"b".repeat(40),
            LIMITS,
            &Merge,
        )
        .unwrap();
        assert_eq!(
            merged[&file],
            TreeNode::RegularFile(b"mine\0".to_vec(), 0o644)
        );
        assert_eq!(result.conflicts, vec!["file"]);

        let error = merge_nodes(
            &trees(b"base"),
            &trees(b"mine"),
            &trees(b"theirs"),
            &"a".repeat(40),
            &"b".repeat(40),
            LIMITS,
            &EmptyMerge,
        )
        .unwrap_err();
        assert_eq!(error, "Pull text merge returned invalid output");
    }

    #[test]
    fn cancellation_preserves_a_replaced_preparation_directory() {
        let fixture = Fixture::new(OriginTool::Dotagents, false);
        let store = fixture.store();
        let preparation = fixture.prepare(&store);
        let moved = fixture.root.join("original-preparation");
        fs::rename(&preparation.preparation_root, &moved).unwrap();
        fs::create_dir(&preparation.preparation_root).unwrap();
        fs::set_permissions(
            &preparation.preparation_root,
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::write(preparation.preparation_root.join("external"), "preserve").unwrap();
        assert!(cancel_fork_pull(&preparation).is_err());
        assert_eq!(
            fs::read(preparation.preparation_root.join("external")).unwrap(),
            b"preserve"
        );
    }

    #[test]
    #[ignore = "creates a retained native Pull restart fixture under an explicit task-owned parent"]
    fn generate_native_restart_fixture() {
        let parent = std::env::var_os("FORK_PULL_FIXTURE_PARENT")
            .expect("set a task-owned fixture parent directory");
        let boundary = std::env::var("FORK_PULL_CHECKPOINT").unwrap();
        let stop = match boundary.as_str() {
            "intent" => PullCheckpoint::IntentRecorded,
            "live" | "base-moved" => PullCheckpoint::LivePublished,
            "base" => PullCheckpoint::BasePublished,
            "registry" => PullCheckpoint::RegistryPublished,
            _ => panic!("unsupported checkpoint"),
        };
        let temp = tempfile::Builder::new()
            .prefix("native-pull-restart-")
            .tempdir_in(parent)
            .unwrap();
        let fixture = Fixture::with_temp(OriginTool::SkillsSh, false, temp);
        let store = fixture.store();
        let preparation = fixture.prepare(&store);
        let upstream = fixture.root.join("upstream");
        fs::create_dir(&upstream).unwrap();
        fs::write(upstream.join("SKILL.md"), "upstream\n").unwrap();
        let mut interrupt = |checkpoint| {
            (checkpoint != stop)
                .then_some(())
                .ok_or_else(|| "native checkpoint".to_string())
        };
        let error = commit_fork_pull_with_recovery_hook(
            &mut fixture.service(),
            &store,
            preparation,
            &upstream,
            &Merge,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
            &mut interrupt,
        )
        .unwrap_err();
        assert_eq!(error.message, "native checkpoint");
        let event_id = error.event_id.unwrap();
        if boundary == "base-moved" {
            fs::rename(
                &fixture.base,
                fixture
                    .base
                    .parent()
                    .unwrap()
                    .join(format!(".previous-{event_id}")),
            )
            .unwrap();
        }
        fs::write(
            fixture
                .scope
                .home
                .join(".agents/skill-studio-projects.json"),
            r#"{"tracked":[],"excluded":[]}"#,
        )
        .unwrap();
        fs::write(
            fixture.scope.home.join(".agents/skill-studio-scope.json"),
            r#"{"backing_roots":[],"plugin_ownership_roots":[]}"#,
        )
        .unwrap();
        fs::write(
            fixture.root.join("checkpoint.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "checkpoint": boundary,
                "event_id": event_id,
                "home": fixture.scope.home,
                "app_data": store.app_data,
                "skill": fixture.source,
                "base": fixture.base,
            }))
            .unwrap(),
        )
        .unwrap();
        println!("{}", fixture._temp.keep().display());
    }
}
