//! Durable creation of one global Universal skills.sh Fork.
use crate::{
    skill_backup_copy::{inspect_entry, BackupCopyLimits},
    skill_backup_reservation::BackupStateRoot,
    skill_backup_source::BackupSourceRoot,
    skill_coordination::{CancellationToken, FinalizedWriteLease},
    skill_deployment::{parse_deployment_id, SkillDestination},
    skill_document_target::SkillRegistryTarget,
    skill_event::{EventDraft, EventRow, EventStatus},
    skill_event_operations::{EventWriteFailure, GuardedEventStore},
    skill_event_store::EventStore,
    skill_fork_registry::{ForkRecord, OriginTool},
    skill_fork_transition::ForkRegistryTransition,
    skill_ownership::LifecycleOwnerKind,
    skill_service::{ScopedSkillService, SkillScope},
};
use cap_std::fs::MetadataExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

pub const EVENT_KIND: &str = "fork_skills_sh";
const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
const EMPTY_REGISTRY: &[u8] = b"{}";
const PROVIDER_LIVENESS_FILE: &str = "provider-liveness";

#[derive(Debug)]
pub struct ProviderProcessGuard {
    file: cap_std::fs::File,
}

impl ProviderProcessGuard {
    /// Keep the event lock open in the provider process. The flag changes in
    /// the forked child only, before exec, so other threads cannot inherit it.
    pub fn inherit_by(&self, command: &mut Command) {
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;

        let descriptor = self.file.as_raw_fd();
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(descriptor, libc::F_GETFD);
                if flags == -1
                    || libc::fcntl(descriptor, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsShForkSource {
    pub origin_source: String,
    pub repo: String,
    pub path: String,
    pub base_commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsShForkRequest {
    pub deployment_id: String,
    pub expected_owner_revision: String,
    pub expected_source: SkillsShForkSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillsShForkOutcome {
    pub event_id: String,
    pub record: ForkRecord,
}

#[derive(Debug)]
pub struct SkillsShForkError {
    pub event_id: Option<String>,
    pub recovery_required: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProviderPhase {
    NotStarted,
    MayHaveStarted,
    ConfirmedDetached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillsMembership {
    root_device: u64,
    root_inode: u64,
    root_mode: u32,
    siblings: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderLivenessIdentity {
    device: u64,
    inode: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillsShForkIntent {
    version: u32,
    scope: SkillScope,
    deployment_id: String,
    owner_revision: String,
    source: SkillsShForkSource,
    name: String,
    skill_dir: PathBuf,
    skills_dir: PathBuf,
    agents_dir: PathBuf,
    provider_path: PathBuf,
    registry_path: PathBuf,
    live_tree: String,
    upstream_tree: String,
    baseline_before: Option<String>,
    provider_before: Vec<u8>,
    provider_after: Vec<u8>,
    membership_before: SkillsMembership,
    registry_before: Option<Vec<u8>>,
    registry_transition: ForkRegistryTransition,
    record: ForkRecord,
    provider_phase: ProviderPhase,
    provider_liveness: ProviderLivenessIdentity,
}

impl SkillsShForkIntent {
    fn draft(&self, event_id: &str) -> Result<EventDraft, String> {
        self.validate_recorded(event_id)?;
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
            || !matches!(row.status.as_str(), "pending" | "interrupted")
            || row.restorable
            || row.inverse.is_some()
        {
            return Err("Not a pending skills.sh Fork event".into());
        }
        let intent: Self =
            serde_json::from_value(row.payload.clone()).map_err(|error| error.to_string())?;
        intent.validate_recorded(&row.id)?;
        if row.skill != intent.name
            || row.harness.as_deref() != Some("universal")
            || row.scope.as_deref() != Some("global")
            || row.project_path.is_some()
            || row.backup_dir.as_deref() != Some(&format!("backups/{}", row.id))
            || row.reverted_by.is_some()
            || serde_json::to_value(&intent).map_err(|error| error.to_string())? != row.payload
        {
            return Err("Skills.sh Fork event metadata does not match its intent".into());
        }
        Ok(intent)
    }

    fn validate(&self, event_id: &str) -> Result<(), String> {
        let parsed = parse_deployment_id(&self.deployment_id)
            .ok_or("Invalid skills.sh Fork deployment ID")?;
        let expected_after = provider_after(&self.provider_before, &self.name)?;
        self.registry_transition.validate()?;
        self.registry_transition
            .apply_document(self.registry_before.as_deref().unwrap_or(EMPTY_REGISTRY))?;
        self.membership_before.validate()?;
        let valid_hash = matches!(self.source.base_commit.len(), 40 | 64)
            && self
                .source
                .base_commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if self.version != 1
            || !crate::skill_backup_reservation::valid_id(event_id)
            || !self.scope.home.is_absolute()
            || self.owner_revision.is_empty()
            || parsed.scope != "global"
            || parsed.slot != "universal"
            || parsed.destination != SkillDestination::Universal
            || parsed.name != self.name
            || parsed.lexical_path != self.skill_dir
            || self.agents_dir != self.scope.home.join(".agents")
            || self.skills_dir != self.agents_dir.join("skills")
            || self.skill_dir != self.skills_dir.join(&self.name)
            || self.provider_path != self.agents_dir.join(".skill-lock.json")
            || self.registry_path != self.agents_dir.join("skill-studio.json")
            || self.record.deployment_id != self.deployment_id
            || self.record.skill_dir != self.skill_dir
            || self.record.origin_tool != OriginTool::SkillsSh
            || self.record.origin_source != self.source.origin_source
            || self.record.repo != self.source.repo
            || self.record.path != self.source.path
            || self.record.base_commit != self.source.base_commit
            || self.record.declared_ref.is_some()
            || self.registry_transition.record() != &self.record
            || self.provider_after != expected_after
            || self.source.origin_source.is_empty()
            || self.source.repo.is_empty()
            || !valid_hash
            || tree_identity(&self.live_tree).is_err()
            || tree_identity(&self.upstream_tree).is_err()
            || self
                .baseline_before
                .as_deref()
                .is_some_and(|identity| tree_identity(identity).is_err())
        {
            return Err("Invalid skills.sh Fork intent".into());
        }
        Ok(())
    }

    fn validate_recorded(&self, event_id: &str) -> Result<(), String> {
        self.validate(event_id)?;
        if self.provider_liveness.device == 0 || self.provider_liveness.inode == 0 {
            return Err("Invalid skills.sh Fork provider liveness identity".into());
        }
        Ok(())
    }

    fn registry_after(&self) -> Result<Vec<u8>, String> {
        self.registry_transition
            .apply_document(self.registry_before.as_deref().unwrap_or(EMPTY_REGISTRY))
    }

    fn with_phase(&self, phase: ProviderPhase) -> Self {
        let mut next = self.clone();
        next.provider_phase = phase;
        next
    }

    fn with_provider_liveness(mut self, identity: ProviderLivenessIdentity) -> Self {
        self.provider_liveness = identity;
        self
    }
}

impl SkillsMembership {
    fn validate(&self) -> Result<(), String> {
        if self.root_device == 0
            || self.root_inode == 0
            || self.siblings.keys().any(|name| decode_name(name).is_err())
            || self.siblings.values().any(|identity| identity.is_empty())
        {
            return Err("Invalid skills.sh sibling membership evidence".into());
        }
        Ok(())
    }
}

fn encode_name(name: &OsStr) -> String {
    name.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode_name(name: &str) -> Result<OsString, String> {
    if name.is_empty() || !name.len().is_multiple_of(2) {
        return Err("Invalid encoded skill entry name".into());
    }
    let bytes = name
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .ok_or_else(|| "Invalid encoded skill entry name".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let name = OsString::from_vec(bytes);
    if crate::skill_backup_copy::valid_component(&name) {
        Ok(name)
    } else {
        Err("Invalid encoded skill entry name".into())
    }
}

fn tree_identity(value: &str) -> Result<&str, String> {
    value
        .strip_prefix("tree-v1:")
        .filter(|identity| !identity.is_empty())
        .ok_or_else(|| "Invalid Fork tree identity".into())
}

fn source(path: &Path) -> Result<crate::skill_backup_source::BackupSource, String> {
    BackupSourceRoot::bind(path.parent().ok_or("Skill path has no parent")?)
        .map_err(|error| error.to_string())?
        .select(path.file_name().ok_or("Skill path has no name")?)
        .map_err(|error| error.to_string())
}

fn tree(
    path: &Path,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<String, String> {
    let source = source(path)?;
    inspect_entry(&source.directory, &source.name, limits, cancellation)
        .map(|report| format!("tree-v1:{}", report.tree_identity))
        .map_err(|error| error.to_string())
}

fn optional_tree(
    path: &Path,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<Option<String>, String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            tree(path, limits, cancellation).map(Some)
        }
        Ok(_) => Err("Fork tree is not an independent directory".into()),
    }
}

fn baseline_path(store: &EventStore, name: &str) -> PathBuf {
    store
        .app_data
        .join("skill-studio/forks")
        .join(name)
        .join("base")
}

struct UniqueJson(Value);

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueJson;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("JSON with unique object keys")
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::Bool(value)))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<UniqueJson, E> {
                Ok(UniqueJson(value.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<UniqueJson, E> {
                Ok(UniqueJson(value.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<UniqueJson, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| UniqueJson(Value::Number(number)))
                    .ok_or_else(|| E::custom("Non-finite JSON number"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::String(value.into())))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut values = Vec::new();
                while let Some(UniqueJson(value)) = sequence.next_element()? {
                    values.push(value);
                }
                Ok(UniqueJson(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some((key, UniqueJson(value))) = map.next_entry::<String, UniqueJson>()? {
                    if values.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("Duplicate JSON object key"));
                    }
                }
                Ok(UniqueJson(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

fn json_document(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err("skills.sh lock exceeds its size limit".into());
    }
    serde_json::from_slice::<UniqueJson>(bytes)
        .map(|document| document.0)
        .map_err(|error| format!("Invalid skills.sh lock: {error}"))
}

fn provider_after(before: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let mut document = json_document(before)?;
    if document
        .get("version")
        .and_then(Value::as_u64)
        .is_none_or(|version| version < 3)
    {
        return Err("Fork requires a supported skills.sh lock version".into());
    }
    let skills = document
        .as_object_mut()
        .and_then(|root| root.get_mut("skills"))
        .and_then(Value::as_object_mut)
        .ok_or("skills.sh lock has no skills object")?;
    if skills.remove(name).is_none() {
        return Err("Selected skills.sh owner is absent from its lock".into());
    }
    serde_json::to_vec(&document).map_err(|error| error.to_string())
}

fn selected_source(before: &[u8], name: &str) -> Result<(String, String, String), String> {
    let document = json_document(before)?;
    let selected = document
        .get("skills")
        .and_then(Value::as_object)
        .and_then(|skills| skills.get(name))
        .and_then(Value::as_object)
        .ok_or("Selected skills.sh owner is absent from its lock")?;
    if selected.get("sourceType").and_then(Value::as_str) != Some("github") {
        return Err("Fork requires a GitHub skills.sh source".into());
    }
    let origin = selected
        .get("source")
        .and_then(Value::as_str)
        .ok_or("Skills.sh source is missing")?
        .to_string();
    let repo = crate::skill_dotagents_ledger::github_repo_from_source(&origin)
        .ok_or("Could not determine the skills.sh GitHub repository")?;
    let raw_path = selected
        .get("skillPath")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let path = raw_path
        .strip_suffix("/SKILL.md")
        .unwrap_or(raw_path)
        .to_string();
    Ok((origin, repo, path))
}

fn membership(
    skills_dir: &Path,
    selected: &OsStr,
    ignored: Option<&OsStr>,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<SkillsMembership, String> {
    let root = BackupSourceRoot::bind(skills_dir).map_err(|error| error.to_string())?;
    let directory = root.directory().map_err(|error| error.to_string())?;
    let metadata = directory
        .dir_metadata()
        .map_err(|error| error.to_string())?;
    let mut siblings = BTreeMap::new();
    let mut bytes = 0u64;
    let mut entries = 0u64;
    for entry in directory.entries().map_err(|error| error.to_string())? {
        if cancellation.is_cancelled() {
            return Err("Skills membership observation cancelled".into());
        }
        let name = entry.map_err(|error| error.to_string())?.file_name();
        if name == selected || ignored.is_some_and(|ignored| name == ignored) {
            continue;
        }
        let remaining = BackupCopyLimits {
            max_bytes: limits.max_bytes.saturating_sub(bytes),
            max_entries: limits.max_entries.saturating_sub(entries),
            max_depth: limits.max_depth,
        };
        if remaining.max_entries == 0 {
            return Err("Skills sibling membership exceeds its entry limit".into());
        }
        let report = inspect_entry(&directory, &name, remaining, cancellation)
            .map_err(|error| error.to_string())?;
        bytes = bytes.saturating_add(report.bytes);
        entries = entries.saturating_add(report.entries);
        if siblings
            .insert(encode_name(&name), report.tree_identity)
            .is_some()
        {
            return Err("Duplicate skills sibling membership entry".into());
        }
    }
    let after = root
        .directory()
        .and_then(|directory| directory.dir_metadata())
        .map_err(|error| error.to_string())?;
    if metadata.dev() != after.dev()
        || metadata.ino() != after.ino()
        || metadata.mode() != after.mode()
    {
        return Err("Skills parent changed during membership observation".into());
    }
    let result = SkillsMembership {
        root_device: metadata.dev(),
        root_inode: metadata.ino(),
        root_mode: metadata.mode(),
        siblings,
    };
    result.validate()?;
    Ok(result)
}

fn matching_source(
    inventory: &crate::skill_service::InventoryRead,
    name: &str,
    owner_id: &str,
) -> Result<crate::skill_inventory::OwnerUpdateSource, String> {
    let sources = inventory
        .skills
        .iter()
        .find(|skill| skill.name == name)
        .ok_or("Selected skills.sh Fork skill is absent")?
        .update_sources
        .iter()
        .filter(|source| source.owner_id == owner_id)
        .cloned()
        .collect::<Vec<_>>();
    if sources.len() != 1 {
        return Err("Skills.sh Fork source evidence is missing or ambiguous".into());
    }
    Ok(sources.into_iter().next().expect("one source"))
}

#[allow(clippy::too_many_arguments)]
fn prepare_admission<'a>(
    service: &'a mut ScopedSkillService,
    store: &EventStore,
    event_id: &str,
    request: &SkillsShForkRequest,
    upstream_path: &Path,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<(SkillsShForkIntent, FinalizedWriteLease<'a>), String> {
    let parsed = parse_deployment_id(&request.deployment_id)
        .ok_or("Invalid skills.sh Fork deployment ID")?;
    let scope = service.scope();
    let agents_dir = scope.home.join(".agents");
    let skills_dir = agents_dir.join("skills");
    let skill_dir = skills_dir.join(&parsed.name);
    let provider_path = agents_dir.join(".skill-lock.json");
    let registry_path = agents_dir.join("skill-studio.json");
    let (inventory, lease) = service
        .prepare_write_inventory_with_entries(
            Some(&BTreeSet::from([parsed.name.clone()])),
            &[
                store.app_data.clone(),
                upstream_path.to_path_buf(),
                agents_dir.clone(),
                skills_dir.clone(),
            ],
            &[
                skill_dir.clone(),
                upstream_path.to_path_buf(),
                provider_path.clone(),
                registry_path.clone(),
            ],
            timeout,
            cancellation.clone(),
        )
        .map_err(|error| error.to_string())?;
    let deployments = inventory
        .skills
        .iter()
        .filter(|skill| skill.name == parsed.name)
        .flat_map(|skill| &skill.deployments)
        .collect::<Vec<_>>();
    let selected = deployments
        .iter()
        .find(|deployment| deployment.id == request.deployment_id)
        .ok_or("Selected skills.sh Fork deployment is absent")?;
    if deployments.len() != 1
        || selected.owner_kind != LifecycleOwnerKind::SkillsSh
        || selected.is_symlink
        || selected.path != skill_dir.to_string_lossy()
        || selected.scope != "global"
        || selected.destination != SkillDestination::Universal
        || selected.agent != "shared"
        || selected.owner_revision.as_deref() != Some(request.expected_owner_revision.as_str())
    {
        return Err(
            "Fork requires one fresh canonical skills.sh deployment and refuses same-name harness paths"
                .into(),
        );
    }
    let owner_id = selected
        .owner_id
        .as_deref()
        .ok_or("Skills.sh Fork owner ID is missing")?;
    let admitted_source = matching_source(&inventory, &parsed.name, owner_id)?;
    if admitted_source.repo != request.expected_source.repo
        || admitted_source.path.as_deref().unwrap_or_default() != request.expected_source.path
        || admitted_source.source_ref.is_some()
    {
        return Err("Skills.sh Fork source changed before admission".into());
    }
    let provider_before = lease
        .read_retained(&provider_path, MAX_DOCUMENT_BYTES)
        .map_err(|error| format!("Could not read skills.sh lock: {error}"))?;
    let (origin_source, repo, path) = selected_source(&provider_before, &parsed.name)?;
    if origin_source != request.expected_source.origin_source
        || repo != request.expected_source.repo
        || path != request.expected_source.path
    {
        return Err("Skills.sh lock source changed before admission".into());
    }
    let provider_after = provider_after(&provider_before, &parsed.name)?;
    let registry_before = lease.read_ownership_registry(&registry_path, MAX_DOCUMENT_BYTES)?;
    let live_tree = tree(&skill_dir, limits, &cancellation)?;
    let upstream_tree = tree(upstream_path, limits, &cancellation)?;
    let baseline_before =
        optional_tree(&baseline_path(store, &parsed.name), limits, &cancellation)?;
    let membership_before = membership(
        &skills_dir,
        skill_dir.file_name().ok_or("Skill path has no name")?,
        None,
        limits,
        &cancellation,
    )?;
    lease.revalidate().map_err(|error| error.to_string())?;
    let record = ForkRecord {
        deployment_id: request.deployment_id.clone(),
        skill_dir: skill_dir.clone(),
        forked_at: chrono::Utc::now().to_rfc3339(),
        origin_tool: OriginTool::SkillsSh,
        origin_source: request.expected_source.origin_source.clone(),
        repo: request.expected_source.repo.clone(),
        path: request.expected_source.path.clone(),
        declared_ref: None,
        base_commit: request.expected_source.base_commit.clone(),
    };
    let registry_transition = ForkRegistryTransition::new(
        parsed.name.clone(),
        record.clone(),
        registry_before.as_deref().unwrap_or(EMPTY_REGISTRY),
    )?;
    let intent = SkillsShForkIntent {
        version: 1,
        scope,
        deployment_id: request.deployment_id.clone(),
        owner_revision: request.expected_owner_revision.clone(),
        source: request.expected_source.clone(),
        name: parsed.name,
        skill_dir,
        skills_dir,
        agents_dir,
        provider_path,
        registry_path,
        live_tree,
        upstream_tree,
        baseline_before,
        provider_before,
        provider_after,
        membership_before,
        registry_before,
        registry_transition,
        record,
        provider_phase: ProviderPhase::NotStarted,
        provider_liveness: ProviderLivenessIdentity {
            device: 0,
            inode: 0,
        },
    };
    intent.validate(event_id)?;
    Ok((intent, lease))
}

fn preserve_evidence(
    store: &EventStore,
    event_id: &str,
    intent: &SkillsShForkIntent,
    upstream_path: &Path,
    lease: &FinalizedWriteLease<'_>,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<ProviderLivenessIdentity, String> {
    lease.revalidate().map_err(|error| error.to_string())?;
    let root = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
    let backup = root.reserve(event_id).map_err(|error| error.to_string())?;
    let result = (|| {
        let live = source(&intent.skill_dir)?;
        let upstream = source(upstream_path)?;
        let saved_live = backup
            .copy_entry(
                &live.directory,
                &live.name,
                OsStr::new("live"),
                limits,
                cancellation,
            )
            .map_err(|error| error.to_string())?;
        let saved_upstream = backup
            .copy_entry(
                &upstream.directory,
                &upstream.name,
                OsStr::new("upstream"),
                limits,
                cancellation,
            )
            .map_err(|error| error.to_string())?;
        if let Some(expected) = &intent.baseline_before {
            let old = source(&baseline_path(store, &intent.name))?;
            let saved = backup
                .copy_entry(
                    &old.directory,
                    &old.name,
                    OsStr::new("previous-base"),
                    limits,
                    cancellation,
                )
                .map_err(|error| error.to_string())?;
            if format!("tree-v1:{}", saved.tree_identity) != *expected {
                return Err("Previous Fork base changed while it was preserved".into());
            }
        }
        backup
            .write_new_file("skill-lock.json", &intent.provider_before)
            .map_err(|error| error.to_string())?;
        backup
            .write_new_file(PROVIDER_LIVENESS_FILE, b"")
            .map_err(|error| error.to_string())?;
        let (device, inode) = backup
            .file_identity(PROVIDER_LIVENESS_FILE)
            .map_err(|error| error.to_string())?;
        match &intent.registry_before {
            Some(registry) => backup
                .write_new_file("registry.json", registry)
                .map_err(|error| error.to_string())?,
            None => backup
                .write_new_file("registry.absent", b"")
                .map_err(|error| error.to_string())?,
        }
        if format!("tree-v1:{}", saved_live.tree_identity) != intent.live_tree
            || format!("tree-v1:{}", saved_upstream.tree_identity) != intent.upstream_tree
        {
            return Err("Fork evidence changed while it was preserved".into());
        }
        backup.revalidate().map_err(|error| error.to_string())?;
        lease.revalidate().map_err(|error| error.to_string())?;
        Ok(ProviderLivenessIdentity { device, inode })
    })();
    match result {
        Ok(identity) => Ok(identity),
        Err(error) => match backup.discard() {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!(
                "{error}; could not remove unjournaled Fork evidence: {cleanup}"
            )),
        },
    }
}

fn unjournaled_error(
    store: &EventStore,
    event_id: &str,
    message: impl Into<String>,
) -> SkillsShForkError {
    let message = message.into();
    let cleanup = BackupStateRoot::bind(&store.app_data)
        .and_then(|root| root.open_existing(event_id)?.discard());
    SkillsShForkError {
        event_id: None,
        recovery_required: false,
        message: match cleanup {
            Ok(()) => message,
            Err(error) => {
                format!("{message}; could not remove unjournaled Fork evidence: {error}")
            }
        },
    }
}

fn validate_scope(service: &ScopedSkillService, intent: &SkillsShForkIntent) -> Result<(), String> {
    let current = service.scope();
    if current.home != intent.scope.home
        || current.backing_roots != intent.scope.backing_roots
        || current.plugin_ownership_roots != intent.scope.plugin_ownership_roots
    {
        return Err("Skills.sh Fork recovery scope changed".into());
    }
    Ok(())
}

fn acquire_provider_guard(
    store: &EventStore,
    event_id: &str,
    identity: ProviderLivenessIdentity,
    timeout: Option<Duration>,
) -> Result<ProviderProcessGuard, String> {
    let root = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
    let backup = root
        .open_existing(event_id)
        .map_err(|error| error.to_string())?;
    backup
        .verify_file(PROVIDER_LIVENESS_FILE, b"")
        .map_err(|error| error.to_string())?;
    let file = backup
        .open_file(PROVIDER_LIVENESS_FILE)
        .map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if metadata.dev() != identity.device || metadata.ino() != identity.inode {
        return Err("Skills.sh provider liveness file was replaced".into());
    }
    let deadline = timeout.map(|duration| Instant::now() + duration);
    loop {
        match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => break,
            Err(error) if error == rustix::io::Errno::WOULDBLOCK => {
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    return Err("Skills.sh provider is still running".into());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    backup
        .verify_file(PROVIDER_LIVENESS_FILE, b"")
        .map_err(|error| error.to_string())?;
    let rebound = backup
        .open_file(PROVIDER_LIVENESS_FILE)
        .map_err(|error| error.to_string())?;
    let metadata = rebound.metadata().map_err(|error| error.to_string())?;
    if metadata.dev() != identity.device || metadata.ino() != identity.inode {
        return Err("Skills.sh provider liveness file was replaced".into());
    }
    Ok(ProviderProcessGuard { file })
}

fn prepare_effects<'a>(
    service: &'a mut ScopedSkillService,
    store: &EventStore,
    intent: &SkillsShForkIntent,
    timeout: Option<Duration>,
) -> Result<FinalizedWriteLease<'a>, String> {
    validate_scope(service, intent)?;
    let (_inventory, lease) = service
        .prepare_write_inventory_with_entries(
            Some(&BTreeSet::from([intent.name.clone()])),
            &[
                store.app_data.clone(),
                intent.agents_dir.clone(),
                intent.skills_dir.clone(),
            ],
            &[
                intent.skill_dir.clone(),
                intent.provider_path.clone(),
                intent.registry_path.clone(),
            ],
            timeout,
            CancellationToken::default(),
        )
        .map_err(|error| error.to_string())?;
    lease.revalidate().map_err(|error| error.to_string())?;
    Ok(lease)
}

fn pending(
    store: &EventStore,
    lease: &FinalizedWriteLease<'_>,
    row: &EventRow,
    intent: &SkillsShForkIntent,
) -> Result<(), String> {
    let current = GuardedEventStore::bind(store, lease)?
        .next_recovery_event(lease)?
        .ok_or("Skills.sh Fork event is no longer pending")?;
    if current.id != row.id || SkillsShForkIntent::from_row(&current)? != *intent {
        return Err("Skills.sh Fork event changed or is out of order".into());
    }
    verify_document_evidence(store, row, intent)
}

fn verify_document_evidence(
    store: &EventStore,
    row: &EventRow,
    intent: &SkillsShForkIntent,
) -> Result<(), String> {
    let backup_root = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
    let backup = backup_root
        .open_existing(&row.id)
        .map_err(|error| error.to_string())?;
    backup
        .verify_file("skill-lock.json", &intent.provider_before)
        .map_err(|error| error.to_string())?;
    backup
        .verify_file(PROVIDER_LIVENESS_FILE, b"")
        .map_err(|error| error.to_string())?;
    match &intent.registry_before {
        Some(bytes) => backup
            .verify_file("registry.json", bytes)
            .map_err(|error| error.to_string()),
        None => backup
            .verify_file("registry.absent", b"")
            .map_err(|error| error.to_string()),
    }
}

fn verify_evidence(
    store: &EventStore,
    row: &EventRow,
    intent: &SkillsShForkIntent,
    limits: BackupCopyLimits,
) -> Result<(), String> {
    let backup_root = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
    let backup = backup_root
        .open_existing(&row.id)
        .map_err(|error| error.to_string())?;
    verify_document_evidence(store, row, intent)?;
    let cancellation = CancellationToken::default();
    backup
        .verify_entry(
            OsStr::new("live"),
            tree_identity(&intent.live_tree)?,
            limits,
            &cancellation,
        )
        .map_err(|error| error.to_string())?;
    backup
        .verify_entry(
            OsStr::new("upstream"),
            tree_identity(&intent.upstream_tree)?,
            limits,
            &cancellation,
        )
        .map_err(|error| error.to_string())?;
    if let Some(previous) = &intent.baseline_before {
        backup
            .verify_entry(
                OsStr::new("previous-base"),
                tree_identity(previous)?,
                limits,
                &cancellation,
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn read_optional_provider(
    lease: &FinalizedWriteLease<'_>,
    path: &Path,
) -> Result<Option<Vec<u8>>, String> {
    lease.revalidate().map_err(|error| error.to_string())?;
    let result = match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Some(
            lease
                .read_retained(path, MAX_DOCUMENT_BYTES)
                .map_err(|error| error.to_string())?,
        ),
        Ok(_) => return Err("Skills.sh provider lock is not a regular file".into()),
    };
    lease.revalidate().map_err(|error| error.to_string())?;
    Ok(result)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderState {
    Attached,
    Detached,
}

fn provider_state(
    current: Option<&[u8]>,
    intent: &SkillsShForkIntent,
) -> Result<ProviderState, String> {
    let current = current.ok_or("Skills.sh provider lock disappeared")?;
    let current = json_document(current)?;
    if current == json_document(&intent.provider_before)? {
        Ok(ProviderState::Attached)
    } else if current == json_document(&intent.provider_after)? {
        Ok(ProviderState::Detached)
    } else {
        Err("Skills.sh provider lock has partial or conflicting edits".into())
    }
}

fn baseline_is_published(
    store: &EventStore,
    row: &EventRow,
    intent: &SkillsShForkIntent,
    limits: BackupCopyLimits,
) -> Result<bool, String> {
    let cancellation = CancellationToken::default();
    let base = optional_tree(&baseline_path(store, &intent.name), limits, &cancellation)?;
    let journal = optional_tree(
        &store
            .app_data
            .join("skill-studio/forks")
            .join(&intent.name)
            .join(format!(".previous-{}", row.id)),
        limits,
        &cancellation,
    )?;
    if base.as_deref() == Some(&intent.upstream_tree) {
        if intent.baseline_before.as_deref().is_some_and(|before| {
            before != intent.upstream_tree && journal.as_deref() != Some(before)
        }) {
            return Err("Fork base was replaced without its admitted prior-base journal".into());
        }
        return Ok(true);
    }
    match (&intent.baseline_before, base, journal) {
        (None, None, None) => Ok(false),
        (Some(before), Some(base), None) if base == *before => Ok(false),
        (Some(before), None, Some(journal)) if journal == *before => Ok(false),
        _ => Err("Fork merge-base state changed after admission".into()),
    }
}

struct Observation {
    provider: ProviderState,
    live_present: bool,
    baseline_published: bool,
    registry_published: bool,
    restore_stage_present: bool,
}

fn observe(
    store: &EventStore,
    row: &EventRow,
    intent: &SkillsShForkIntent,
    lease: &FinalizedWriteLease<'_>,
    limits: BackupCopyLimits,
) -> Result<Observation, String> {
    let cancellation = CancellationToken::default();
    let provider = read_optional_provider(lease, &intent.provider_path)?;
    let provider = provider_state(provider.as_deref(), intent)?;
    let current_membership = membership(
        &intent.skills_dir,
        intent
            .skill_dir
            .file_name()
            .ok_or("Skill path has no name")?,
        Some(OsStr::new(&format!(".skill-studio-fork-live-{}", row.id))),
        limits,
        &cancellation,
    )?;
    if current_membership != intent.membership_before {
        return Err("Skills.sh provider changed sibling skill data".into());
    }
    let live = optional_tree(&intent.skill_dir, limits, &cancellation)?;
    let live_present = match live {
        Some(identity) if identity == intent.live_tree => true,
        None => false,
        Some(_) => return Err("Live Fork tree was replaced or changed".into()),
    };
    let baseline_published = baseline_is_published(store, row, intent, limits)?;
    let restore_stage_present = match std::fs::symlink_metadata(
        intent
            .skills_dir
            .join(format!(".skill-studio-fork-live-{}", row.id)),
    ) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.to_string()),
    };
    let registry =
        lease.read_current_ownership_registry(&intent.registry_path, MAX_DOCUMENT_BYTES)?;
    let after = intent.registry_after()?;
    let registry_published = if registry == intent.registry_before {
        false
    } else if registry.as_deref() == Some(after.as_slice()) {
        true
    } else {
        return Err("Skills.sh Fork registry has partial or conflicting edits".into());
    };
    if registry_published && !baseline_published {
        return Err("Skills.sh Fork registry was published before its merge base".into());
    }
    lease.revalidate().map_err(|error| error.to_string())?;
    pending(store, lease, row, intent)?;
    Ok(Observation {
        provider,
        live_present,
        baseline_published,
        registry_published,
        restore_stage_present,
    })
}

fn advance_phase(
    store: &EventStore,
    lease: &FinalizedWriteLease<'_>,
    row: &EventRow,
    intent: &SkillsShForkIntent,
    next: ProviderPhase,
) -> Result<(EventRow, SkillsShForkIntent), EventWriteFailure> {
    let allowed = matches!(
        (intent.provider_phase, next),
        (ProviderPhase::NotStarted, ProviderPhase::MayHaveStarted)
            | (
                ProviderPhase::MayHaveStarted,
                ProviderPhase::ConfirmedDetached
            )
    );
    if !allowed {
        return Err(EventWriteFailure::BeforeWrite(
            "Skills.sh provider phase cannot advance from this event".into(),
        ));
    }
    pending(store, lease, row, intent).map_err(EventWriteFailure::BeforeWrite)?;
    let proposed = intent.with_phase(next);
    proposed
        .validate(&row.id)
        .map_err(EventWriteFailure::BeforeWrite)?;
    let updated = GuardedEventStore::bind(store, lease)
        .map_err(EventWriteFailure::BeforeWrite)?
        .replace_pending_payload(
            lease,
            row,
            serde_json::to_value(&proposed)
                .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?,
        )?;
    let parsed =
        SkillsShForkIntent::from_row(&updated).map_err(EventWriteFailure::MayHaveWritten)?;
    if parsed != proposed {
        return Err(EventWriteFailure::MayHaveWritten(
            "Skills.sh provider phase receipt differs from its intent".into(),
        ));
    }
    Ok((updated, parsed))
}

fn finish_event(
    store: &EventStore,
    lease: &FinalizedWriteLease<'_>,
    row: &EventRow,
    intent: &SkillsShForkIntent,
    status: EventStatus,
) -> Result<(), String> {
    pending(store, lease, row, intent)?;
    GuardedEventStore::bind(store, lease)?
        .finish_recovery_snapshot(lease, row, status, None)
        .map_err(|error| error.to_string())
}

fn restore_live(
    store: &EventStore,
    row: &EventRow,
    intent: &SkillsShForkIntent,
    lease: &FinalizedWriteLease<'_>,
    limits: BackupCopyLimits,
) -> Result<(), String> {
    pending(store, lease, row, intent)?;
    let root = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
    let cancellation = CancellationToken::default();
    let published = root.restore_verified_fork_live(
        lease,
        &intent.skills_dir,
        &intent.name,
        &row.id,
        tree_identity(&intent.live_tree)?,
        limits,
        &cancellation,
        || pending(store, lease, row, intent),
        || Ok(()),
        || Ok(()),
        || Ok(()),
    )?;
    if published && tree(&intent.skill_dir, limits, &cancellation)? != intent.live_tree {
        return Err("Restored live Fork tree differs from immutable evidence".into());
    }
    Ok(())
}

fn publish_registry(
    intent: &SkillsShForkIntent,
    lease: &mut FinalizedWriteLease<'_>,
) -> Result<(), String> {
    let proposed = intent.registry_after()?;
    let target = SkillRegistryTarget::bind(&intent.agents_dir)?;
    match &intent.registry_before {
        Some(before) => target
            .replace(lease, before, &proposed)
            .map_err(|error| error.to_string()),
        None => target
            .create(lease, &proposed)
            .map_err(|error| error.to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryResult {
    Completed,
    ProviderDidNotDetach,
}

fn recover_internal(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<RecoveryResult, String> {
    let mut row = row.clone();
    let mut intent = SkillsShForkIntent::from_row(&row)?;
    validate_scope(service, &intent)?;
    let _provider_guard =
        acquire_provider_guard(store, &row.id, intent.provider_liveness, timeout)?;
    verify_evidence(store, &row, &intent, limits)?;
    loop {
        let mut lease = prepare_effects(service, store, &intent, timeout)?;
        let observation = observe(store, &row, &intent, &lease, limits)?;
        match intent.provider_phase {
            ProviderPhase::NotStarted => {
                if observation.provider != ProviderState::Attached
                    || !observation.live_present
                    || observation.baseline_published
                    || observation.registry_published
                {
                    return Err("Unstarted skills.sh Fork has unexpected filesystem effects".into());
                }
                finish_event(store, &lease, &row, &intent, EventStatus::Failed)?;
                return Ok(RecoveryResult::ProviderDidNotDetach);
            }
            ProviderPhase::MayHaveStarted => match observation.provider {
                ProviderState::Attached => {
                    if observation.baseline_published || observation.registry_published {
                        return Err("Skills.sh provider left a partial attached state".into());
                    }
                    if !observation.live_present || observation.restore_stage_present {
                        restore_live(store, &row, &intent, &lease, limits)?;
                        continue;
                    }
                    finish_event(store, &lease, &row, &intent, EventStatus::Failed)?;
                    return Ok(RecoveryResult::ProviderDidNotDetach);
                }
                ProviderState::Detached => {
                    if observation.baseline_published || observation.registry_published {
                        return Err(
                            "Skills.sh Fork app effects preceded provider confirmation".into()
                        );
                    }
                    let advanced = advance_phase(
                        store,
                        &lease,
                        &row,
                        &intent,
                        ProviderPhase::ConfirmedDetached,
                    )
                    .map_err(|error| error.to_string())?;
                    row = advanced.0;
                    intent = advanced.1;
                    if !observation.live_present {
                        restore_live(store, &row, &intent, &lease, limits)?;
                    }
                    continue;
                }
            },
            ProviderPhase::ConfirmedDetached => {
                if observation.provider != ProviderState::Detached {
                    return Err("Confirmed skills.sh detach no longer exists".into());
                }
                if !observation.live_present || observation.restore_stage_present {
                    restore_live(store, &row, &intent, &lease, limits)?;
                    continue;
                }
                if !observation.baseline_published {
                    let state = BackupStateRoot::bind(&store.app_data)
                        .map_err(|error| error.to_string())?;
                    state.publish_verified_fork_base(
                        &lease,
                        &intent.name,
                        &row.id,
                        tree_identity(&intent.upstream_tree)?,
                        intent
                            .baseline_before
                            .as_deref()
                            .map(tree_identity)
                            .transpose()?,
                        limits,
                        &CancellationToken::default(),
                        || pending(store, &lease, &row, &intent),
                        || Ok(()),
                    )?;
                    continue;
                }
                if !observation.registry_published {
                    publish_registry(&intent, &mut lease)?;
                    continue;
                }
                verify_evidence(store, &row, &intent, limits)?;
                finish_event(store, &lease, &row, &intent, EventStatus::Done)?;
                return Ok(RecoveryResult::Completed);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn create_skills_sh_fork(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &SkillsShForkRequest,
    upstream_path: &Path,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
    invoke_provider: impl FnOnce(&ProviderProcessGuard) -> Result<(), String>,
) -> Result<SkillsShForkOutcome, SkillsShForkError> {
    let id = crate::skill_event_store::allocate_id();
    let (intent, lease) = prepare_admission(
        service,
        store,
        &id,
        request,
        upstream_path,
        limits,
        timeout,
        cancellation.clone(),
    )
    .map_err(|message| SkillsShForkError {
        event_id: None,
        recovery_required: false,
        message,
    })?;
    let provider_liveness = preserve_evidence(
        store,
        &id,
        &intent,
        upstream_path,
        &lease,
        limits,
        &cancellation,
    )
    .map_err(|message| SkillsShForkError {
        event_id: None,
        recovery_required: false,
        message,
    })?;
    let intent = intent.with_provider_liveness(provider_liveness);
    let provider_guard = acquire_provider_guard(store, &id, intent.provider_liveness, timeout)
        .map_err(|message| unjournaled_error(store, &id, message))?;
    let draft = intent
        .draft(&id)
        .map_err(|message| unjournaled_error(store, &id, message))?;
    let guarded = GuardedEventStore::bind(store, &lease)
        .map_err(|message| unjournaled_error(store, &id, message))?;
    if let Err(error) = guarded.record_pending(&lease, &id, draft) {
        return match error {
            EventWriteFailure::MayHaveWritten(message) => Err(SkillsShForkError {
                event_id: Some(id),
                recovery_required: true,
                message,
            }),
            safe
            @ (EventWriteFailure::CancelledBeforeWrite | EventWriteFailure::BeforeWrite(_)) => {
                Err(unjournaled_error(store, &id, safe.to_string()))
            }
        };
    }
    let row = store
        .get(&id)
        .map_err(|message| SkillsShForkError {
            event_id: Some(id.clone()),
            recovery_required: true,
            message,
        })?
        .ok_or_else(|| SkillsShForkError {
            event_id: Some(id.clone()),
            recovery_required: true,
            message: "Recorded skills.sh Fork event is missing".into(),
        })?;
    let (row, intent) = advance_phase(store, &lease, &row, &intent, ProviderPhase::MayHaveStarted)
        .map_err(|error| SkillsShForkError {
            event_id: Some(id.clone()),
            recovery_required: true,
            message: error.to_string(),
        })?;
    drop(lease);
    let provider_error = invoke_provider(&provider_guard).err();
    drop(provider_guard);
    let recovered = recover_internal(service, store, &row, limits, timeout).map_err(|message| {
        SkillsShForkError {
            event_id: Some(id.clone()),
            recovery_required: true,
            message: match provider_error.as_deref() {
                Some(provider) => format!("{provider}; recovery could not complete: {message}"),
                None => message,
            },
        }
    })?;
    if recovered == RecoveryResult::ProviderDidNotDetach {
        return Err(SkillsShForkError {
            event_id: Some(id),
            recovery_required: false,
            message: provider_error
                .unwrap_or_else(|| "skills.sh remove returned without detaching the skill".into()),
        });
    }
    Ok(SkillsShForkOutcome {
        event_id: id,
        record: intent.record,
    })
}

pub fn recover_skills_sh_fork(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<bool, String> {
    recover_internal(service, store, row, limits, timeout).map(|result| {
        matches!(
            result,
            RecoveryResult::Completed | RecoveryResult::ProviderDidNotDetach
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    const LIMITS: BackupCopyLimits = BackupCopyLimits {
        max_bytes: 1024 * 1024,
        max_entries: 100,
        max_depth: 8,
    };
    const TIMEOUT: Option<Duration> = Some(Duration::from_secs(5));

    struct Fixture {
        _temp: tempfile::TempDir,
        scope: SkillScope,
        agents: PathBuf,
        skill: PathBuf,
        upstream: PathBuf,
        state: PathBuf,
        request: SkillsShForkRequest,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_temp(tempfile::tempdir().unwrap(), None)
        }

        fn with_temp(temp: tempfile::TempDir, state: Option<PathBuf>) -> Self {
            let root = temp.path().canonicalize().unwrap();
            let home = root.join("home");
            let agents = home.join(".agents");
            let skill = agents.join("skills/sample");
            let sibling = agents.join("skills/sibling");
            let upstream = root.join("upstream");
            for directory in [&skill, &sibling, &upstream] {
                std::fs::create_dir_all(directory).unwrap();
            }
            std::fs::write(
                skill.join("SKILL.md"),
                "---\nname: sample\ndescription: Test\n---\nLocal\n",
            )
            .unwrap();
            std::fs::write(skill.join("resource"), "local resource").unwrap();
            std::fs::write(
                sibling.join("SKILL.md"),
                "---\nname: sibling\ndescription: Test\n---\nSibling\n",
            )
            .unwrap();
            std::fs::write(
                upstream.join("SKILL.md"),
                "---\nname: sample\ndescription: Test\n---\nUpstream\n",
            )
            .unwrap();
            std::fs::write(
                agents.join(".skill-lock.json"),
                serde_json::to_vec(&serde_json::json!({
                    "version": 3,
                    "future": {"keep": true},
                    "skills": {
                        "sample": {
                            "source": "owner/repo",
                            "sourceType": "github",
                            "sourceUrl": "https://github.com/owner/repo",
                            "skillPath": "skills/sample/SKILL.md",
                            "skillFolderHash": "local-hash",
                            "installedAt": "2026-09-16T00:00:00Z",
                            "updatedAt": "2026-09-16T00:00:00Z",
                            "future": "keep"
                        },
                        "sibling": {
                            "source": "owner/sibling",
                            "sourceType": "github",
                            "sourceUrl": "https://github.com/owner/sibling",
                            "skillPath": "skills/sibling/SKILL.md",
                            "skillFolderHash": "sibling-hash",
                            "installedAt": "2026-09-16T00:00:00Z",
                            "updatedAt": "2026-09-16T00:00:00Z"
                        }
                    }
                }))
                .unwrap(),
            )
            .unwrap();
            std::fs::write(
                agents.join("skill-studio.json"),
                br#"{"version":4,"future":{"keep":true}}"#,
            )
            .unwrap();
            let scope = SkillScope {
                home: home.clone(),
                projects: vec![],
                backing_roots: vec![],
                plugin_ownership_roots: vec![],
            };
            let mut service = ScopedSkillService::bind(scope.clone()).unwrap();
            let snapshot = service.scan(None, TIMEOUT).unwrap();
            let deployment = snapshot
                .skills
                .iter()
                .flat_map(|skill| &skill.deployments)
                .find(|deployment| deployment.path == skill.to_string_lossy())
                .unwrap();
            assert_eq!(deployment.owner_kind, LifecycleOwnerKind::SkillsSh);
            let request = SkillsShForkRequest {
                deployment_id: deployment.id.clone(),
                expected_owner_revision: deployment.owner_revision.clone().unwrap(),
                expected_source: SkillsShForkSource {
                    origin_source: "owner/repo".into(),
                    repo: "owner/repo".into(),
                    path: "skills/sample".into(),
                    base_commit: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                },
            };
            Self {
                _temp: temp,
                scope,
                agents,
                skill,
                upstream,
                state: state.unwrap_or_else(|| root.join("state")),
                request,
            }
        }

        fn service(&self) -> ScopedSkillService {
            ScopedSkillService::bind(self.scope.clone()).unwrap()
        }

        fn store(&self) -> EventStore {
            EventStore::open(&self.state).unwrap()
        }

        fn detach(&self, replace_live: bool) {
            let lock_path = self.agents.join(".skill-lock.json");
            let before = std::fs::read(&lock_path).unwrap();
            std::fs::write(&lock_path, provider_after(&before, "sample").unwrap()).unwrap();
            std::fs::remove_dir_all(&self.skill).unwrap();
            if replace_live {
                std::fs::create_dir_all(&self.skill).unwrap();
                std::fs::write(self.skill.join("SKILL.md"), "replacement").unwrap();
            }
        }

        fn create(
            &self,
            provider: impl FnOnce() -> Result<(), String>,
        ) -> Result<SkillsShForkOutcome, SkillsShForkError> {
            create_skills_sh_fork(
                &mut self.service(),
                &self.store(),
                &self.request,
                &self.upstream,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
                |_| provider(),
            )
        }
    }

    #[test]
    fn provider_json_rejects_duplicate_keys_at_every_depth() {
        for input in [
            r#"{"version":3,"version":4}"#,
            r#"{"skills":{"sibling":{},"sibling":{}}}"#,
            r#"{"future":[{"key":true,"key":false}]}"#,
            r#"{"name":1,"\u006eame":2}"#,
        ] {
            assert!(json_document(input.as_bytes())
                .unwrap_err()
                .contains("Duplicate"));
        }
        let input = br#"{"version":3,"skills":{},"future":[null,true,-1,1.5,"keep"]}"#;
        assert_eq!(
            json_document(input).unwrap(),
            serde_json::from_slice::<Value>(input).unwrap()
        );
        assert!(json_document(b"{} trailing").is_err());
    }

    #[test]
    fn provider_detach_restores_live_and_publishes_exact_fork() {
        let fixture = Fixture::new();
        let live_before = tree(&fixture.skill, LIMITS, &CancellationToken::default()).unwrap();

        let outcome = fixture
            .create(|| {
                fixture.detach(false);
                Ok(())
            })
            .unwrap();

        assert_eq!(
            tree(&fixture.skill, LIMITS, &CancellationToken::default()).unwrap(),
            live_before
        );
        let lock: Value = serde_json::from_slice(
            &std::fs::read(fixture.agents.join(".skill-lock.json")).unwrap(),
        )
        .unwrap();
        assert!(lock["skills"].get("sample").is_none());
        assert_eq!(lock["skills"]["sibling"]["source"], "owner/sibling");
        assert_eq!(lock["future"]["keep"], true);
        let registry: Value = serde_json::from_slice(
            &std::fs::read(fixture.agents.join("skill-studio.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(registry["forks"]["sample"]["origin_tool"], "skills-sh");
        assert_eq!(registry["future"]["keep"], true);
        assert_eq!(
            fixture
                .store()
                .get(&outcome.event_id)
                .unwrap()
                .unwrap()
                .status,
            "done"
        );
    }

    #[test]
    fn provider_failure_before_effects_records_failure_without_app_effects() {
        let fixture = Fixture::new();
        let lock = std::fs::read(fixture.agents.join(".skill-lock.json")).unwrap();

        let error = fixture
            .create(|| Err("provider failed".into()))
            .unwrap_err();

        assert!(!error.recovery_required);
        assert_eq!(
            std::fs::read(fixture.agents.join(".skill-lock.json")).unwrap(),
            lock
        );
        assert!(!baseline_path(&fixture.store(), "sample").exists());
        assert_eq!(
            fixture
                .store()
                .get(error.event_id.as_ref().unwrap())
                .unwrap()
                .unwrap()
                .status,
            "failed"
        );
    }

    #[test]
    fn provider_failure_after_exact_effects_still_completes() {
        let fixture = Fixture::new();

        let outcome = fixture
            .create(|| {
                fixture.detach(false);
                Err("provider failed after effects".into())
            })
            .unwrap();

        assert_eq!(
            fixture
                .store()
                .get(&outcome.event_id)
                .unwrap()
                .unwrap()
                .status,
            "done"
        );
        assert!(fixture.skill.exists());
    }

    #[test]
    fn replacement_after_provider_detach_is_preserved_and_requires_recovery() {
        let fixture = Fixture::new();

        let error = fixture
            .create(|| {
                fixture.detach(true);
                Err("provider failed after replacement".into())
            })
            .unwrap_err();

        assert!(error.recovery_required);
        assert_eq!(
            std::fs::read_to_string(fixture.skill.join("SKILL.md")).unwrap(),
            "replacement"
        );
        assert_eq!(
            fixture
                .store()
                .get(error.event_id.as_ref().unwrap())
                .unwrap()
                .unwrap()
                .status,
            "pending"
        );
    }

    #[test]
    fn stale_phase_event_is_refused_without_payload_mutation() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let id = crate::skill_event_store::allocate_id();
        let mut service = fixture.service();
        let (intent, lease) = prepare_admission(
            &mut service,
            &store,
            &id,
            &fixture.request,
            &fixture.upstream,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        let provider_liveness = preserve_evidence(
            &store,
            &id,
            &intent,
            &fixture.upstream,
            &lease,
            LIMITS,
            &CancellationToken::default(),
        )
        .unwrap();
        let intent = intent.with_provider_liveness(provider_liveness);
        GuardedEventStore::bind(&store, &lease)
            .unwrap()
            .record_pending(&lease, &id, intent.draft(&id).unwrap())
            .unwrap();
        let stale = store.get(&id).unwrap().unwrap();
        let (current, current_intent) = advance_phase(
            &store,
            &lease,
            &stale,
            &intent,
            ProviderPhase::MayHaveStarted,
        )
        .unwrap();

        let error = advance_phase(
            &store,
            &lease,
            &stale,
            &intent,
            ProviderPhase::MayHaveStarted,
        )
        .unwrap_err();

        assert!(matches!(error, EventWriteFailure::BeforeWrite(_)));
        assert_eq!(store.get(&id).unwrap().unwrap().payload, current.payload);
        assert_eq!(current_intent.provider_phase, ProviderPhase::MayHaveStarted);
    }

    #[test]
    fn second_same_name_deployment_is_refused_before_provider() {
        let fixture = Fixture::new();
        let other = fixture.scope.home.join(".claude/skills/sample");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join("SKILL.md"),
            "---\nname: sample\ndescription: Other\n---\n",
        )
        .unwrap();
        let invoked = AtomicBool::new(false);

        let error = fixture
            .create(|| {
                invoked.store(true, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err();

        assert!(!error.recovery_required);
        assert!(!invoked.load(Ordering::SeqCst));
        assert!(other.exists());
    }
    fn admitted_detach(fixture: &Fixture) -> (EventRow, SkillsShForkIntent) {
        let store = fixture.store();
        let id = crate::skill_event_store::allocate_id();
        let mut service = fixture.service();
        let (intent, lease) = prepare_admission(
            &mut service,
            &store,
            &id,
            &fixture.request,
            &fixture.upstream,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        let provider_liveness = preserve_evidence(
            &store,
            &id,
            &intent,
            &fixture.upstream,
            &lease,
            LIMITS,
            &CancellationToken::default(),
        )
        .unwrap();
        let intent = intent.with_provider_liveness(provider_liveness);
        GuardedEventStore::bind(&store, &lease)
            .unwrap()
            .record_pending(&lease, &id, intent.draft(&id).unwrap())
            .unwrap();
        let row = store.get(&id).unwrap().unwrap();
        let result =
            advance_phase(&store, &lease, &row, &intent, ProviderPhase::MayHaveStarted).unwrap();
        drop(lease);
        fixture.detach(false);
        result
    }

    fn admitted_missing_with_attached_provider(
        fixture: &Fixture,
    ) -> (EventRow, SkillsShForkIntent) {
        let store = fixture.store();
        let id = crate::skill_event_store::allocate_id();
        let mut service = fixture.service();
        let (intent, lease) = prepare_admission(
            &mut service,
            &store,
            &id,
            &fixture.request,
            &fixture.upstream,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        let provider_liveness = preserve_evidence(
            &store,
            &id,
            &intent,
            &fixture.upstream,
            &lease,
            LIMITS,
            &CancellationToken::default(),
        )
        .unwrap();
        let intent = intent.with_provider_liveness(provider_liveness);
        GuardedEventStore::bind(&store, &lease)
            .unwrap()
            .record_pending(&lease, &id, intent.draft(&id).unwrap())
            .unwrap();
        let row = store.get(&id).unwrap().unwrap();
        let result =
            advance_phase(&store, &lease, &row, &intent, ProviderPhase::MayHaveStarted).unwrap();
        drop(lease);
        std::fs::remove_dir_all(&fixture.skill).unwrap();
        result
    }

    fn stage_base_then_stop(fixture: &Fixture) -> (EventRow, SkillsShForkIntent) {
        let (row, intent) = admitted_detach(fixture);
        let store = fixture.store();
        let mut service = fixture.service();
        {
            let lease = prepare_effects(&mut service, &store, &intent, TIMEOUT).unwrap();
            restore_live(&store, &row, &intent, &lease, LIMITS).unwrap();
        }
        {
            let lease = prepare_effects(&mut service, &store, &intent, TIMEOUT).unwrap();
            restore_live(&store, &row, &intent, &lease, LIMITS).unwrap();
        }
        let lease = prepare_effects(&mut service, &store, &intent, TIMEOUT).unwrap();
        let calls = AtomicUsize::new(0);
        let state = BackupStateRoot::bind(&store.app_data).unwrap();
        let result = state.publish_verified_fork_base(
            &lease,
            &intent.name,
            &row.id,
            tree_identity(&intent.upstream_tree).unwrap(),
            None,
            LIMITS,
            &CancellationToken::default(),
            || {
                if calls.fetch_add(1, Ordering::SeqCst) == 2 {
                    Err("stop after staging".into())
                } else {
                    Ok(())
                }
            },
            || Ok(()),
        );
        assert!(result.is_err());
        (row, intent)
    }

    #[test]
    fn attached_provider_with_missing_live_restores_original_and_fails_event() {
        let fixture = Fixture::new();
        let original = std::fs::read(fixture.skill.join("SKILL.md")).unwrap();
        let provider = std::fs::read(fixture.agents.join(".skill-lock.json")).unwrap();
        let (row, _) = admitted_missing_with_attached_provider(&fixture);
        let store = fixture.store();
        let mut service = fixture.service();

        assert!(recover_skills_sh_fork(&mut service, &store, &row, LIMITS, TIMEOUT).unwrap());
        assert_eq!(
            std::fs::read(fixture.skill.join("SKILL.md")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read(fixture.agents.join(".skill-lock.json")).unwrap(),
            provider
        );
        assert_eq!(store.get(&row.id).unwrap().unwrap().status, "failed");
        assert!(!baseline_path(&store, "sample").exists());
    }

    #[test]
    fn recovery_waits_while_provider_guard_is_held() {
        let fixture = Fixture::new();
        let (row, intent) = admitted_missing_with_attached_provider(&fixture);
        let store = fixture.store();
        let guard =
            acquire_provider_guard(&store, &row.id, intent.provider_liveness, TIMEOUT).unwrap();
        let mut service = fixture.service();

        let error = recover_skills_sh_fork(
            &mut service,
            &store,
            &row,
            LIMITS,
            Some(Duration::from_millis(40)),
        )
        .unwrap_err();
        assert!(error.contains("provider is still running"));
        assert!(!fixture.skill.exists());
        assert_eq!(store.get(&row.id).unwrap().unwrap().status, "pending");

        drop(guard);
        assert!(recover_skills_sh_fork(&mut service, &store, &row, LIMITS, TIMEOUT).unwrap());
        assert!(fixture.skill.exists());
    }

    #[test]
    fn provider_guard_remains_locked_across_exec() {
        let fixture = Fixture::new();
        let (row, intent) = admitted_missing_with_attached_provider(&fixture);
        let store = fixture.store();
        let guard =
            acquire_provider_guard(&store, &row.id, intent.provider_liveness, TIMEOUT).unwrap();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 0.2"]);
        guard.inherit_by(&mut command);
        let mut child = command.spawn().unwrap();
        drop(guard);

        assert!(acquire_provider_guard(
            &store,
            &row.id,
            intent.provider_liveness,
            Some(Duration::from_millis(40)),
        )
        .unwrap_err()
        .contains("provider is still running"));
        assert!(child.wait().unwrap().success());
        acquire_provider_guard(&store, &row.id, intent.provider_liveness, TIMEOUT).unwrap();
    }

    #[test]
    fn replacement_liveness_file_cannot_bypass_running_provider_guard() {
        let fixture = Fixture::new();
        let (row, intent) = admitted_missing_with_attached_provider(&fixture);
        let store = fixture.store();
        let guard =
            acquire_provider_guard(&store, &row.id, intent.provider_liveness, TIMEOUT).unwrap();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 0.2"]);
        guard.inherit_by(&mut command);
        let mut child = command.spawn().unwrap();
        drop(guard);
        let liveness = store
            .app_data
            .join("backups")
            .join(&row.id)
            .join(PROVIDER_LIVENESS_FILE);
        let original = fixture.upstream.join("original-provider-liveness");
        std::fs::rename(&liveness, &original).unwrap();
        std::fs::write(&liveness, b"").unwrap();

        let error = acquire_provider_guard(
            &store,
            &row.id,
            intent.provider_liveness,
            Some(Duration::from_millis(40)),
        )
        .unwrap_err();
        assert!(error.contains("liveness file was replaced"));
        assert!(original.is_file());
        assert!(liveness.is_file());
        assert!(child.wait().unwrap().success());
        assert!(
            acquire_provider_guard(&store, &row.id, intent.provider_liveness, TIMEOUT,)
                .unwrap_err()
                .contains("liveness file was replaced")
        );
    }

    #[test]
    fn unrelated_project_scope_drift_does_not_block_global_recovery() {
        let fixture = Fixture::new();
        let (row, _) = admitted_detach(&fixture);
        let project = fixture.scope.home.join("projects/new");
        std::fs::create_dir_all(&project).unwrap();
        let mut changed_scope = fixture.scope.clone();
        changed_scope.projects.push(project);
        let mut service = ScopedSkillService::bind(changed_scope).unwrap();
        let store = fixture.store();

        assert!(recover_skills_sh_fork(&mut service, &store, &row, LIMITS, TIMEOUT).unwrap());
        assert_eq!(store.get(&row.id).unwrap().unwrap().status, "done");
    }

    #[test]
    fn changed_base_stage_is_preserved() {
        let fixture = Fixture::new();
        let (row, intent) = stage_base_then_stop(&fixture);
        let stage = fixture
            .state
            .join("skill-studio/forks/sample")
            .join(format!(".base-{}", row.id));
        std::fs::write(stage.join("base/SKILL.md"), "external edit").unwrap();
        let store = fixture.store();
        let mut service = fixture.service();

        assert!(recover_skills_sh_fork(&mut service, &store, &row, LIMITS, TIMEOUT).is_err());
        assert_eq!(
            std::fs::read_to_string(stage.join("base/SKILL.md")).unwrap(),
            "external edit"
        );
        assert_eq!(intent.provider_phase, ProviderPhase::MayHaveStarted);
    }

    #[test]
    fn owned_partial_base_stage_recovers_from_matching_file_prefix() {
        let fixture = Fixture::new();
        let (row, _) = stage_base_then_stop(&fixture);
        let stage = fixture
            .state
            .join("skill-studio/forks/sample")
            .join(format!(".base-{}", row.id));
        let document = stage.join("base/SKILL.md");
        let original = std::fs::read(&document).unwrap();
        std::fs::write(&document, &original[..original.len() / 2]).unwrap();
        let store = fixture.store();
        let mut service = fixture.service();

        assert!(recover_skills_sh_fork(&mut service, &store, &row, LIMITS, TIMEOUT).unwrap());
        assert_eq!(
            std::fs::read(baseline_path(&store, "sample").join("SKILL.md")).unwrap(),
            original
        );
        assert!(!stage.exists());
    }

    #[test]
    fn empty_base_stage_without_owner_marker_recovers() {
        let fixture = Fixture::new();
        let (row, _) = admitted_detach(&fixture);
        let stage = fixture
            .state
            .join("skill-studio/forks/sample")
            .join(format!(".base-{}", row.id));
        std::fs::create_dir_all(&stage).unwrap();
        let store = fixture.store();
        let mut service = fixture.service();

        assert!(recover_skills_sh_fork(&mut service, &store, &row, LIMITS, TIMEOUT).unwrap());
        assert!(baseline_path(&store, "sample").join("SKILL.md").is_file());
        assert!(!stage.exists());
    }

    #[test]
    fn replaced_base_stage_is_preserved() {
        let fixture = Fixture::new();
        let (row, _) = stage_base_then_stop(&fixture);
        let stage = fixture
            .state
            .join("skill-studio/forks/sample")
            .join(format!(".base-{}", row.id));
        let moved = fixture.upstream.join("old-base-stage");
        std::fs::rename(&stage, &moved).unwrap();
        std::fs::create_dir(&stage).unwrap();
        std::fs::write(stage.join("external"), "keep").unwrap();
        let store = fixture.store();
        let mut service = fixture.service();

        assert!(recover_skills_sh_fork(&mut service, &store, &row, LIMITS, TIMEOUT).is_err());
        assert_eq!(
            std::fs::read_to_string(stage.join("external")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn replaced_empty_live_stage_is_preserved_after_publication() {
        let fixture = Fixture::new();
        let (row, intent) = admitted_detach(&fixture);
        let store = fixture.store();
        let mut service = fixture.service();
        let root = BackupStateRoot::bind(&store.app_data).unwrap();
        {
            let lease = prepare_effects(&mut service, &store, &intent, TIMEOUT).unwrap();
            root.restore_verified_fork_live(
                &lease,
                &intent.skills_dir,
                &intent.name,
                &row.id,
                tree_identity(&intent.live_tree).unwrap(),
                LIMITS,
                &CancellationToken::default(),
                || pending(&store, &lease, &row, &intent),
                || Ok(()),
                || Ok(()),
                || Ok(()),
            )
            .unwrap();
        }
        {
            let lease = prepare_effects(&mut service, &store, &intent, TIMEOUT).unwrap();
            assert!(root
                .restore_verified_fork_live(
                    &lease,
                    &intent.skills_dir,
                    &intent.name,
                    &row.id,
                    tree_identity(&intent.live_tree).unwrap(),
                    LIMITS,
                    &CancellationToken::default(),
                    || pending(&store, &lease, &row, &intent),
                    || Ok(()),
                    || Ok(()),
                    || Err("stop after publish".into()),
                )
                .is_err());
        }
        let stage = intent
            .skills_dir
            .join(format!(".skill-studio-fork-live-{}", row.id));
        let moved = fixture.upstream.join("old-empty-live-stage");
        std::fs::rename(&stage, &moved).unwrap();
        std::fs::create_dir(&stage).unwrap();

        let lease = prepare_effects(&mut service, &store, &intent, TIMEOUT).unwrap();
        assert!(root
            .restore_verified_fork_live(
                &lease,
                &intent.skills_dir,
                &intent.name,
                &row.id,
                tree_identity(&intent.live_tree).unwrap(),
                LIMITS,
                &CancellationToken::default(),
                || pending(&store, &lease, &row, &intent),
                || Ok(()),
                || Ok(()),
                || Ok(()),
            )
            .is_err());
        assert!(stage.is_dir());
    }

    #[test]
    fn live_stage_swap_after_partial_verification_preserves_replacement() {
        let fixture = Fixture::new();
        let (row, intent) = admitted_detach(&fixture);
        let store = fixture.store();
        let mut service = fixture.service();
        {
            let lease = prepare_effects(&mut service, &store, &intent, TIMEOUT).unwrap();
            restore_live(&store, &row, &intent, &lease, LIMITS).unwrap();
        }
        let backup = store.app_data.join("backups").join(&row.id);
        std::fs::remove_file(backup.join("live-stage-ready")).unwrap();
        let stage = intent
            .skills_dir
            .join(format!(".skill-studio-fork-live-{}", row.id));
        let live = stage.join("live");
        let document = live.join("SKILL.md");
        let original = std::fs::read(&document).unwrap();
        std::fs::write(&document, &original[..original.len() / 2]).unwrap();
        let moved = fixture.upstream.join("verified-partial-live");
        let root = BackupStateRoot::bind(&store.app_data).unwrap();
        let lease = prepare_effects(&mut service, &store, &intent, TIMEOUT).unwrap();

        assert!(root
            .restore_verified_fork_live(
                &lease,
                &intent.skills_dir,
                &intent.name,
                &row.id,
                tree_identity(&intent.live_tree).unwrap(),
                LIMITS,
                &CancellationToken::default(),
                || pending(&store, &lease, &row, &intent),
                || {
                    std::fs::rename(&live, &moved).unwrap();
                    std::fs::create_dir(&live).unwrap();
                    std::fs::write(live.join("external"), "keep").unwrap();
                    Ok(())
                },
                || Ok(()),
                || Ok(()),
            )
            .is_err());
        assert_eq!(
            std::fs::read_to_string(live.join("external")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn staged_live_is_published_after_reopening_with_a_fresh_lease() {
        let fixture = Fixture::new();
        let (row, intent) = admitted_detach(&fixture);
        {
            let store = fixture.store();
            let mut service = fixture.service();
            let lease = prepare_effects(&mut service, &store, &intent, TIMEOUT).unwrap();
            restore_live(&store, &row, &intent, &lease, LIMITS).unwrap();
            assert!(!fixture.skill.exists());
        }
        let store = fixture.store();
        let mut service = fixture.service();
        assert!(recover_skills_sh_fork(&mut service, &store, &row, LIMITS, TIMEOUT).unwrap());
        assert!(fixture.skill.join("SKILL.md").is_file());
        assert_eq!(store.get(&row.id).unwrap().unwrap().status, "done");
    }

    #[test]
    fn stage_mutation_before_publication_is_preserved_without_live_effects() {
        let fixture = Fixture::new();
        let (row, intent) = admitted_detach(&fixture);
        let store = fixture.store();
        let mut service = fixture.service();
        {
            let lease = prepare_effects(&mut service, &store, &intent, TIMEOUT).unwrap();
            restore_live(&store, &row, &intent, &lease, LIMITS).unwrap();
        }
        let lease = prepare_effects(&mut service, &store, &intent, TIMEOUT).unwrap();
        let stage = intent
            .skills_dir
            .join(format!(".skill-studio-fork-live-{}", row.id))
            .join("live/SKILL.md");
        let root = BackupStateRoot::bind(&store.app_data).unwrap();
        assert!(root
            .restore_verified_fork_live(
                &lease,
                &intent.skills_dir,
                &intent.name,
                &row.id,
                tree_identity(&intent.live_tree).unwrap(),
                LIMITS,
                &CancellationToken::default(),
                || pending(&store, &lease, &row, &intent),
                || Ok(()),
                || {
                    std::fs::write(&stage, "external edit").unwrap();
                    Ok(())
                },
                || Ok(()),
            )
            .is_err());
        assert!(!fixture.skill.exists());
        assert_eq!(std::fs::read_to_string(&stage).unwrap(), "external edit");
        drop(lease);
        assert!(recover_skills_sh_fork(&mut service, &store, &row, LIMITS, TIMEOUT).is_err());
        assert_eq!(std::fs::read_to_string(stage).unwrap(), "external edit");
    }
    #[test]
    fn owned_partial_stage_recovers_from_matching_file_prefix() {
        let fixture = Fixture::new();
        let (row, intent) = admitted_detach(&fixture);
        let store = fixture.store();
        let mut service = fixture.service();
        {
            let lease = prepare_effects(&mut service, &store, &intent, TIMEOUT).unwrap();
            restore_live(&store, &row, &intent, &lease, LIMITS).unwrap();
        }
        let stage = intent
            .skills_dir
            .join(format!(".skill-studio-fork-live-{}", row.id));
        std::fs::remove_file(
            store
                .app_data
                .join("backups")
                .join(&row.id)
                .join("live-stage-ready"),
        )
        .unwrap();
        let document = stage.join("live/SKILL.md");
        let original = std::fs::read(&document).unwrap();
        std::fs::write(&document, &original[..original.len() / 2]).unwrap();
        assert!(recover_skills_sh_fork(&mut service, &store, &row, LIMITS, TIMEOUT).unwrap());
        assert_eq!(
            std::fs::read(fixture.skill.join("SKILL.md")).unwrap(),
            original
        );
        assert!(!stage.exists());
    }

    #[test]
    fn replaced_stage_directory_is_preserved() {
        let fixture = Fixture::new();
        let (row, intent) = admitted_detach(&fixture);
        let store = fixture.store();
        let mut service = fixture.service();
        {
            let lease = prepare_effects(&mut service, &store, &intent, TIMEOUT).unwrap();
            restore_live(&store, &row, &intent, &lease, LIMITS).unwrap();
        }
        let stage = intent
            .skills_dir
            .join(format!(".skill-studio-fork-live-{}", row.id));
        let moved = fixture.upstream.join("old-stage");
        std::fs::rename(&stage, &moved).unwrap();
        std::fs::create_dir(&stage).unwrap();
        std::fs::write(stage.join("external"), "keep").unwrap();
        assert!(recover_skills_sh_fork(&mut service, &store, &row, LIMITS, TIMEOUT).is_err());
        assert_eq!(
            std::fs::read_to_string(stage.join("external")).unwrap(),
            "keep"
        );
        assert!(!fixture.skill.exists());
    }
    #[test]
    #[ignore = "creates retained native acceptance fixtures only when explicitly requested"]
    fn generate_native_restart_fixture() {
        let parent =
            std::env::var_os("SKILLS_SH_FORK_FIXTURE_PARENT").expect("set task fixture parent");
        let checkpoint = std::env::var("SKILLS_SH_FORK_CHECKPOINT").unwrap();
        assert!(matches!(
            checkpoint.as_str(),
            "detached" | "staged" | "partial" | "changed-evidence"
        ));
        let temp = tempfile::Builder::new()
            .prefix("native-skillsfork-")
            .tempdir_in(parent)
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let state = root.join("home/Library/Application Support/com.skillstudio.app");
        let fixture = Fixture::with_temp(temp, Some(state.clone()));
        let (row, intent) = admitted_detach(&fixture);
        if matches!(checkpoint.as_str(), "staged" | "partial") {
            let store = fixture.store();
            let mut service = fixture.service();
            let lease = prepare_effects(&mut service, &store, &intent, TIMEOUT).unwrap();
            restore_live(&store, &row, &intent, &lease, LIMITS).unwrap();
            drop(lease);
            if checkpoint == "partial" {
                std::fs::remove_file(state.join("backups").join(&row.id).join("live-stage-ready"))
                    .unwrap();
                let partial = intent
                    .skills_dir
                    .join(format!(".skill-studio-fork-live-{}", row.id))
                    .join("live/SKILL.md");
                let bytes = std::fs::read(&partial).unwrap();
                std::fs::write(partial, &bytes[..bytes.len() / 2]).unwrap();
            }
        }
        if checkpoint == "changed-evidence" {
            std::fs::write(
                state.join("backups").join(&row.id).join("skill-lock.json"),
                "external evidence",
            )
            .unwrap();
        }
        std::fs::write(
            fixture.agents.join("skill-studio-projects.json"),
            r#"{"tracked":[],"excluded":[]}"#,
        )
        .unwrap();
        std::fs::write(
            fixture.agents.join("skill-studio-scope.json"),
            r#"{"backing_roots":[],"plugin_ownership_roots":[]}"#,
        )
        .unwrap();
        std::fs::write(root.join("checkpoint.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "checkpoint":checkpoint,"event_id":row.id,"home":fixture.scope.home,"state":state,"skill":fixture.skill
        })).unwrap()).unwrap();
        println!("{}", fixture._temp.keep().display());
    }
}
