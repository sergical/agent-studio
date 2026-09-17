// ============================================================================
// Skills Module - dotagents_ledger
// Reads getsentry/dotagents' own bookkeeping files - `agents.lock` (what's
// actually resolved on disk, including the pinned commit) and `agents.toml`
// (what the user declared, including an optional ref) - so
// `skill_update_check` can tell a dotagents-managed skill's installed commit
// from its declared ref without re-deriving either from the skill directory
// itself. Pure file reads: missing files are not an error, just an empty
// result.
// ============================================================================

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// One skill declared in `agents.lock` (joined with `agents.toml` for its ref).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DotagentsSkill {
    pub name: String,
    pub source: String,
    /// "owner/repo", when `source` points at a GitHub repo. `None` for other
    /// hosts (or a source shape we don't recognize).
    pub github_repo: Option<String>,
    pub path: String,
    pub installed_commit: Option<String>,
    /// The `ref` declared in `agents.toml` for this skill's `[[skills]]` row.
    /// `None` for an unpinned or wildcard (`--all`) entry.
    pub declared_ref: Option<String>,
    /// True when `agents.toml` has a `[[skills]]` row for this name at all -
    /// false for a wildcard (`--all`) entry, which `update_skill` re-installs
    /// with `dotagents install` instead of a per-skill `dotagents add`.
    pub has_manifest_row: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct AgentsLock {
    #[serde(default)]
    pub(crate) skills: HashMap<String, LockedSkill>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LockedSkill {
    pub(crate) source: String,
    #[serde(default)]
    pub(crate) resolved_path: Option<String>,
    #[serde(default)]
    pub(crate) resolved_commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct AgentsManifest {
    #[serde(default)]
    pub(crate) skills: Vec<ManifestSkill>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ManifestSkill {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) r#ref: Option<String>,
}

/// Complete selected TOML entries only; callers must independently retain the input scope.
#[derive(Debug, Clone)]
pub struct DotagentsDetachIntent {
    name: String,
    locked: toml::Value,
    declared: toml::Value,
}

/// Requested upstream identity from installed ledger data, not proof of fetched bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DotagentsForkSource {
    source: String,
    repo: String,
    path: String,
    commit: String,
    declared_ref: Option<String>,
}

impl DotagentsForkSource {
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn repo(&self) -> &str {
        &self.repo
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn commit(&self) -> &str {
        &self.commit
    }
    pub fn declared_ref(&self) -> Option<&str> {
        self.declared_ref.as_deref()
    }
}

const MAX_DETACH_BYTES: usize = 8 * 1024 * 1024;

/// Retained provider records only; this grants no authority over installed content or registry writes.
#[cfg(unix)]
pub struct DotagentsReattachmentObservation<'scope> {
    name: String,
    scope: &'scope crate::skill_scope::SkillReadScope,
    lock: crate::skill_scope::ScopedFileObservation,
    manifest: crate::skill_scope::ScopedFileObservation,
    lock_digest: [u8; 32],
    manifest_digest: [u8; 32],
    source: DotagentsForkSource,
}

#[cfg(unix)]
impl DotagentsReattachmentObservation<'_> {
    pub fn source(&self) -> &DotagentsForkSource {
        &self.source
    }

    pub fn selected_rows(
        &self,
        cancellation: &crate::skill_coordination::CancellationToken,
    ) -> Result<DotagentsDetachIntent, String> {
        self.scope
            .revalidate_roots()
            .map_err(|error| error.to_string())?;
        let selected = {
            let lock = self.read_verified_document(&self.lock, &self.lock_digest, cancellation)?;
            let manifest =
                self.read_verified_document(&self.manifest, &self.manifest_digest, cancellation)?;
            DotagentsDetachIntent::from_documents(
                &self.name,
                std::str::from_utf8(&lock).map_err(|error| error.to_string())?,
                std::str::from_utf8(&manifest).map_err(|error| error.to_string())?,
            )?
        };
        if selected.fork_source()? != self.source {
            return Err("Selected provider records differ from observed source".into());
        }
        self.revalidate(cancellation)?;
        Ok(selected)
    }

    fn read_verified_document(
        &self,
        file: &crate::skill_scope::ScopedFileObservation,
        expected: &[u8; 32],
        cancellation: &crate::skill_coordination::CancellationToken,
    ) -> Result<Vec<u8>, String> {
        use sha2::{Digest, Sha256};
        if cancellation.is_cancelled() {
            return Err("Provider observation cancelled".into());
        }
        let bytes = self
            .scope
            .read_observed(file, MAX_DETACH_BYTES)
            .map_err(|error| error.to_string())?;
        let actual: [u8; 32] = Sha256::digest(&bytes).into();
        if &actual != expected {
            return Err("Provider documents changed after observation".into());
        }
        if cancellation.is_cancelled() {
            return Err("Provider observation cancelled".into());
        }
        Ok(bytes)
    }

    pub fn revalidate(
        &self,
        cancellation: &crate::skill_coordination::CancellationToken,
    ) -> Result<(), String> {
        self.scope
            .revalidate_roots()
            .map_err(|error| error.to_string())?;
        for (file, expected) in [
            (&self.lock, &self.lock_digest),
            (&self.manifest, &self.manifest_digest),
        ] {
            self.read_verified_document(file, expected, cancellation)?;
        }
        self.scope
            .revalidate_roots()
            .map_err(|error| error.to_string())
    }
}

#[cfg(unix)]
impl DotagentsDetachIntent {
    pub fn observe_reinstalled<'scope>(
        &self,
        scope: &'scope crate::skill_scope::SkillReadScope,
        agents_dir: &Path,
        cancellation: &crate::skill_coordination::CancellationToken,
    ) -> Result<DotagentsReattachmentObservation<'scope>, String> {
        DotagentsReinstallRequest::from_detach(self)?.observe_reinstalled(
            scope,
            agents_dir,
            cancellation,
        )
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DotagentsDetachRecord {
    version: u32,
    name: String,
    locked: String,
    declared: String,
}

fn validate_detach_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains(['/', '\\']) || matches!(name, "." | ".." | "*") {
        return Err("Detach requires an exact skill name".into());
    }
    Ok(())
}

/// Expected dotagents reinstall origin. This is not write authority or proof
/// that a provider fetched or installed matching bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DotagentsReinstallRequest {
    name: String,
    source: String,
    repo: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    declared_ref: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DotagentsReinstallRequestWire {
    name: String,
    source: String,
    repo: String,
    path: String,
    #[serde(default)]
    declared_ref: Option<String>,
}

impl<'de> Deserialize<'de> for DotagentsReinstallRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = DotagentsReinstallRequestWire::deserialize(deserializer)?;
        Self::new(
            &wire.name,
            &wire.source,
            &wire.repo,
            &wire.path,
            wire.declared_ref.as_deref(),
        )
        .map_err(serde::de::Error::custom)
    }
}

impl DotagentsReinstallRequest {
    pub fn from_detach(intent: &DotagentsDetachIntent) -> Result<Self, String> {
        let source = intent.fork_source()?;
        Self::new(
            &intent.name,
            &source.source,
            &source.repo,
            &source.path,
            source.declared_ref.as_deref(),
        )
    }

    pub fn from_fork_record(
        record: &crate::skill_fork_registry::ForkRecord,
        name: &str,
    ) -> Result<Self, String> {
        if record.origin_tool != crate::skill_fork_registry::OriginTool::Dotagents {
            return Err("Unfork provider requires a dotagents origin".into());
        }
        Self::new(
            name,
            &record.origin_source,
            &record.repo,
            &record.path,
            record.declared_ref.as_deref(),
        )
    }

    fn new(
        name: &str,
        source: &str,
        repo: &str,
        path: &str,
        declared_ref: Option<&str>,
    ) -> Result<Self, String> {
        validate_detach_name(name)?;
        if name.starts_with('-') || name.chars().any(char::is_control) {
            return Err("Unfork requires a safe exact skill name".into());
        }
        if source.starts_with('-') || github_repo_from_source(source).as_deref() != Some(repo) {
            return Err("Unfork provider record has an inconsistent origin".into());
        }
        let path = if path == "." { "" } else { path };
        if path.contains('\\')
            || path.chars().any(char::is_control)
            || (!path.is_empty() && path.split('/').any(|part| matches!(part, "" | "." | "..")))
        {
            return Err("Unfork source path must be repository-relative".into());
        }
        if declared_ref.is_some_and(|value| {
            value.is_empty() || value.starts_with('-') || value.chars().any(char::is_control)
        }) {
            return Err("Unfork provider record has an unsafe declared ref".into());
        }
        Ok(Self {
            name: name.into(),
            source: source.into(),
            repo: repo.into(),
            path: path.into(),
            declared_ref: declared_ref.map(str::to_owned),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn repo(&self) -> &str {
        &self.repo
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn declared_ref(&self) -> Option<&str> {
        self.declared_ref.as_deref()
    }

    pub(crate) fn cache_relative_source(&self) -> std::path::PathBuf {
        let mut path = if let Some(url) = self.source.strip_prefix("git:") {
            let (_, location) = url.split_once("://").expect("validated GitHub source");
            location
                .strip_suffix(".git")
                .unwrap_or(location)
                .split('/')
                .filter(|segment| !segment.is_empty())
                .collect::<std::path::PathBuf>()
        } else {
            std::path::PathBuf::from(&self.repo)
        };
        path.push(&self.path);
        path
    }

    pub fn reinstalled_source(
        &self,
        lock: &str,
        manifest: &str,
    ) -> Result<DotagentsForkSource, String> {
        self.validate_reinstalled_rows(&DotagentsDetachIntent::from_documents(
            &self.name, lock, manifest,
        )?)
    }

    pub(crate) fn validate_detached_documents(
        &self,
        lock: &str,
        manifest: &str,
    ) -> Result<(), String> {
        let (lock_document, manifest_document) = parse_detach_documents(lock, manifest)?;
        if lock_document
            .get("version")
            .and_then(toml::Value::as_integer)
            != Some(1)
            || manifest_document
                .get("version")
                .and_then(toml::Value::as_integer)
                != Some(1)
        {
            return Err("Unfork requires version 1 provider documents".into());
        }
        let (locked, declared) = detach_rows_in(&self.name, &lock_document, &manifest_document)?;
        if locked.is_some() || declared.is_some() {
            return Err("Unfork requires detached selected provider rows".into());
        }
        Ok(())
    }

    pub(crate) fn validate_reinstalled_rows(
        &self,
        current: &DotagentsDetachIntent,
    ) -> Result<DotagentsForkSource, String> {
        let actual = current.fork_source()?;
        if current.name != self.name
            || actual.source != self.source
            || actual.repo != self.repo
            || actual.path != self.path
            || actual.declared_ref != self.declared_ref
        {
            return Err("Reinstalled dotagents entry differs from the saved origin".into());
        }
        if let Some(source) = current.declared.get("source") {
            if source.as_str() != Some(actual.source.as_str()) {
                return Err("Reinstalled dotagents manifest source differs from its lock".into());
            }
        }
        Ok(actual)
    }

    #[cfg(unix)]
    pub fn observe_reinstalled<'scope>(
        &self,
        scope: &'scope crate::skill_scope::SkillReadScope,
        agents_dir: &Path,
        cancellation: &crate::skill_coordination::CancellationToken,
    ) -> Result<DotagentsReattachmentObservation<'scope>, String> {
        use sha2::{Digest, Sha256};
        if cancellation.is_cancelled() {
            return Err("Provider observation cancelled".into());
        }
        scope
            .revalidate_roots()
            .map_err(|error| error.to_string())?;
        let lock = scope
            .observe_regular(&agents_dir.join("agents.lock"))
            .map_err(|error| error.to_string())?;
        let manifest = scope
            .observe_regular(&agents_dir.join("agents.toml"))
            .map_err(|error| error.to_string())?;
        let lock_bytes = scope
            .read_observed(&lock, MAX_DETACH_BYTES)
            .map_err(|error| error.to_string())?;
        if cancellation.is_cancelled() {
            return Err("Provider observation cancelled".into());
        }
        let manifest_bytes = scope
            .read_observed(&manifest, MAX_DETACH_BYTES)
            .map_err(|error| error.to_string())?;
        let source = self.reinstalled_source(
            std::str::from_utf8(&lock_bytes).map_err(|error| error.to_string())?,
            std::str::from_utf8(&manifest_bytes).map_err(|error| error.to_string())?,
        )?;
        let observed = DotagentsReattachmentObservation {
            name: self.name.clone(),
            scope,
            lock,
            manifest,
            lock_digest: Sha256::digest(lock_bytes).into(),
            manifest_digest: Sha256::digest(manifest_bytes).into(),
            source,
        };
        observed.revalidate(cancellation)?;
        Ok(observed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DotagentsDetachState {
    Attached,
    Detached,
    Partial,
    Changed,
    Unavailable,
}

impl DotagentsDetachIntent {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn fork_source(&self) -> Result<DotagentsForkSource, String> {
        let source = self
            .locked
            .get("source")
            .and_then(toml::Value::as_str)
            .ok_or("Fork source is missing")?;
        let repo = github_repo_from_source(source)
            .ok_or("Fork source must identify a GitHub repository")?;
        let commit = self
            .locked
            .get("resolved_commit")
            .and_then(toml::Value::as_str)
            .ok_or("Fork requires an installed commit")?;
        if !matches!(commit.len(), 40 | 64)
            || !commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("Fork requires a full lowercase installed commit ID".into());
        }
        let path = match self.locked.get("resolved_path") {
            None => "",
            Some(value) => value.as_str().ok_or("Fork source path must be a string")?,
        };
        let path = if path == "." { "" } else { path };
        if !path.is_empty()
            && (path.contains(['\\', '\0'])
                || path.split('/').any(|part| matches!(part, "" | "." | "..")))
        {
            return Err("Fork source path must be repository-relative".into());
        }
        let declared_ref = self
            .declared
            .get("ref")
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or("Declared fork ref must be a string")
            })
            .transpose()?;
        Ok(DotagentsForkSource {
            source: source.into(),
            repo,
            path: path.into(),
            commit: commit.into(),
            declared_ref,
        })
    }

    /// Observe reattachment to the saved origin; callers must separately verify installed bytes.
    pub fn reinstalled_source(
        &self,
        lock: &str,
        manifest: &str,
    ) -> Result<DotagentsForkSource, String> {
        let current = Self::from_documents(&self.name, lock, manifest)?;
        self.validate_reinstalled_rows(&current)
    }

    pub(crate) fn validate_reinstalled_rows(
        &self,
        current: &Self,
    ) -> Result<DotagentsForkSource, String> {
        DotagentsReinstallRequest::from_detach(self)?.validate_reinstalled_rows(current)
    }

    pub fn from_documents(name: &str, lock: &str, manifest: &str) -> Result<Self, String> {
        validate_detach_name(name)?;
        let (locked, declared) = detach_rows(name, lock, manifest)?;
        Ok(Self {
            name: name.into(),
            locked: locked.ok_or("Detach requires a locked skill")?,
            declared: declared.ok_or("Detach requires a named manifest entry")?,
        })
    }

    /// Selected rows are encoded as TOML strings to retain TOML types inside JSON events.
    pub fn to_record_json(&self) -> Result<String, String> {
        let record = DotagentsDetachRecord {
            version: 1,
            name: self.name.clone(),
            locked: toml::to_string(&self.locked).map_err(|error| error.to_string())?,
            declared: toml::to_string(&self.declared).map_err(|error| error.to_string())?,
        };
        let json = serde_json::to_string(&record).map_err(|error| error.to_string())?;
        if json.len() > MAX_DETACH_BYTES {
            return Err("Detach record exceeds its limit".into());
        }
        Ok(json)
    }

    /// Decoding validates shape and identity; it grants no filesystem or execution authority.
    pub fn from_record_json(json: &str) -> Result<Self, String> {
        if json.len() > MAX_DETACH_BYTES {
            return Err("Detach record exceeds its limit".into());
        }
        let record: DotagentsDetachRecord =
            serde_json::from_str(json).map_err(|error| error.to_string())?;
        if record.version != 1 {
            return Err("Unsupported detach record version".into());
        }
        validate_detach_name(&record.name)?;
        let locked: toml::Value =
            toml::from_str(&record.locked).map_err(|error| error.to_string())?;
        let declared: toml::Value =
            toml::from_str(&record.declared).map_err(|error| error.to_string())?;
        if !locked.is_table()
            || !declared.is_table()
            || declared.get("name").and_then(toml::Value::as_str) != Some(record.name.as_str())
        {
            return Err("Detach record entries do not match its identity".into());
        }
        Ok(Self {
            name: record.name,
            locked,
            declared,
        })
    }

    pub fn observe(
        &self,
        lock: Option<&str>,
        manifest: Option<&str>,
    ) -> Result<DotagentsDetachState, String> {
        let (Some(lock), Some(manifest)) = (lock, manifest) else {
            return Ok(DotagentsDetachState::Unavailable);
        };
        let (locked, declared) = detach_rows(&self.name, lock, manifest)?;
        Ok(self.state_for_rows(locked, declared))
    }

    fn state_for_rows(
        &self,
        locked: Option<toml::Value>,
        declared: Option<toml::Value>,
    ) -> DotagentsDetachState {
        if locked.as_ref().is_some_and(|row| row != &self.locked)
            || declared.as_ref().is_some_and(|row| row != &self.declared)
        {
            return DotagentsDetachState::Changed;
        }
        match (locked.is_some(), declared.is_some()) {
            (true, true) => DotagentsDetachState::Attached,
            (false, false) => DotagentsDetachState::Detached,
            _ => DotagentsDetachState::Partial,
        }
    }
}

/// Document proposal only. Unrelated syntax is retained; comments attached to
/// removed entries are removed with them. Publication still requires scoped authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DotagentsDetachProposal {
    lock: String,
    manifest: String,
}

impl DotagentsDetachProposal {
    pub(crate) fn validate(&self, name: &str) -> Result<(), String> {
        let (lock, manifest) = parse_detach_documents(&self.lock, &self.manifest)?;
        if lock.get("version").and_then(toml::Value::as_integer) != Some(1)
            || manifest.get("version").and_then(toml::Value::as_integer) != Some(1)
            || detach_rows_in(name, &lock, &manifest)? != (None, None)
        {
            return Err("Invalid native detach document proposal".into());
        }
        Ok(())
    }

    pub fn lock(&self) -> &str {
        &self.lock
    }
    pub fn manifest(&self) -> &str {
        &self.manifest
    }
}

impl DotagentsDetachIntent {
    pub fn propose_document_detach(
        &self,
        lock: &str,
        manifest: &str,
    ) -> Result<DotagentsDetachProposal, String> {
        let (before_lock, before_manifest) = parse_detach_documents(lock, manifest)?;
        if before_lock.get("version").and_then(toml::Value::as_integer) != Some(1)
            || before_manifest
                .get("version")
                .and_then(toml::Value::as_integer)
                != Some(1)
        {
            return Err("Document detach requires explicit version 1 provider documents".into());
        }
        let (locked, declared) = detach_rows_in(&self.name, &before_lock, &before_manifest)?;
        if self.state_for_rows(locked, declared) != DotagentsDetachState::Attached {
            return Err("Detach proposal no longer matches the selected entries".into());
        }
        refuse_plugin_collision(&self.name, &before_lock, &before_manifest)?;
        let proposal = edit_detach_documents(&self.name, lock, manifest)?;
        if self.observe_document_effects(
            lock,
            manifest,
            Some(&proposal.lock),
            Some(&proposal.manifest),
        )? != DotagentsDetachState::Detached
        {
            return Err("Generated detach proposal changed unrelated provider values".into());
        }
        Ok(proposal)
    }
}

impl DotagentsDetachIntent {
    /// Baseline bytes must come from verified pre-detach snapshots. Formatting and
    /// empty selected-skill containers may change; all other TOML values must remain.
    pub fn observe_document_effects(
        &self,
        original_lock: &str,
        original_manifest: &str,
        lock: Option<&str>,
        manifest: Option<&str>,
    ) -> Result<DotagentsDetachState, String> {
        let (before_lock, before_manifest) =
            parse_detach_documents(original_lock, original_manifest)?;
        let (locked, declared) = detach_rows_in(&self.name, &before_lock, &before_manifest)?;
        if self.state_for_rows(locked, declared) != DotagentsDetachState::Attached {
            return Err("Detach baseline does not match the selected provider entries".into());
        }
        let (Some(lock), Some(manifest)) = (lock, manifest) else {
            return Ok(DotagentsDetachState::Unavailable);
        };
        let (after_lock, after_manifest) = parse_detach_documents(lock, manifest)?;
        let (locked, declared) = detach_rows_in(&self.name, &after_lock, &after_manifest)?;
        let state = self.state_for_rows(locked, declared);
        let before = detach_remainder(&self.name, before_lock, before_manifest)?;
        let after = detach_remainder(&self.name, after_lock, after_manifest)?;
        if before != after {
            return Ok(DotagentsDetachState::Changed);
        }
        Ok(state)
    }
}

/// Selected provider-row merge only; publication requires a prepared scoped operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DotagentsReattachProposal {
    lock: String,
    manifest: String,
}

impl DotagentsReattachProposal {
    pub fn lock(&self) -> &str {
        &self.lock
    }
    pub fn manifest(&self) -> &str {
        &self.manifest
    }
}

impl DotagentsDetachIntent {
    pub fn propose_document_reattach(
        &self,
        lock: &str,
        manifest: &str,
    ) -> Result<DotagentsReattachProposal, String> {
        let source = self.fork_source()?;
        if self
            .declared
            .get("source")
            .is_some_and(|value| value.as_str() != Some(source.source()))
        {
            return Err("Staged manifest source differs from its lock".into());
        }
        let (before_lock, before_manifest) = parse_detach_documents(lock, manifest)?;
        if before_lock.get("version").and_then(toml::Value::as_integer) != Some(1)
            || before_manifest
                .get("version")
                .and_then(toml::Value::as_integer)
                != Some(1)
        {
            return Err(
                "Document reattachment requires explicit version 1 provider documents".into(),
            );
        }
        refuse_plugin_collision(&self.name, &before_lock, &before_manifest)?;
        let (locked, declared) = detach_rows_in(&self.name, &before_lock, &before_manifest)?;
        let state = self.state_for_rows(locked.clone(), declared.clone());
        if state == DotagentsDetachState::Changed {
            return Err("Selected provider entries changed before reattachment".into());
        }
        let mut output_lock = editable_document(lock)?;
        let mut output_manifest = editable_document(manifest)?;
        if locked.is_none() {
            let selected = toml::to_string(&self.locked)
                .map_err(|error| error.to_string())?
                .parse::<toml_edit::DocumentMut>()
                .map_err(|error| error.to_string())?
                .into_table();
            if !output_lock.contains_key("skills") {
                output_lock.insert("skills", toml_edit::Item::Table(toml_edit::Table::new()));
            }
            let skills = output_lock.get_mut("skills").ok_or("Lock skills missing")?;
            let selected = if skills.is_inline_table() {
                toml_edit::Item::Value(toml_edit::Value::InlineTable(selected.into_inline_table()))
            } else {
                toml_edit::Item::Table(selected)
            };
            skills
                .as_table_like_mut()
                .ok_or("Lock skills cannot be edited as a table")?
                .insert(&self.name, selected);
        }
        if declared.is_none() {
            let selected = toml::to_string(&self.declared)
                .map_err(|error| error.to_string())?
                .parse::<toml_edit::DocumentMut>()
                .map_err(|error| error.to_string())?
                .into_table();
            if !output_manifest.contains_key("skills") {
                output_manifest.insert(
                    "skills",
                    toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()),
                );
            }
            let skills = output_manifest
                .get_mut("skills")
                .ok_or("Manifest skills missing")?;
            if let Some(rows) = skills.as_array_of_tables_mut() {
                rows.push(selected);
            } else if let Some(rows) = skills.as_array_mut() {
                rows.push(toml_edit::Value::InlineTable(selected.into_inline_table()));
            } else {
                return Err("Manifest skills cannot be edited as an array".into());
            }
        }
        let proposal = DotagentsReattachProposal {
            lock: output_lock.to_string(),
            manifest: output_manifest.to_string(),
        };
        if self.observe_document_effects(
            proposal.lock(),
            proposal.manifest(),
            Some(lock),
            Some(manifest),
        )? != state
        {
            return Err("Generated reattachment changed unrelated provider values".into());
        }
        Ok(proposal)
    }
}

fn refuse_plugin_collision(
    name: &str,
    lock: &toml::Value,
    manifest: &toml::Value,
) -> Result<(), String> {
    if let Some(plugins) = lock.get("plugins") {
        if plugins
            .as_table()
            .ok_or("Lock plugins must be a table")?
            .contains_key(name)
        {
            return Err("Selected name also identifies a plugin".into());
        }
    }
    if let Some(plugins) = manifest.get("plugins") {
        for plugin in plugins
            .as_array()
            .ok_or("Manifest plugins must be an array")?
        {
            if plugin
                .as_table()
                .ok_or("Manifest plugin must be a table")?
                .get("name")
                .and_then(toml::Value::as_str)
                == Some(name)
            {
                return Err("Selected name also identifies a plugin".into());
            }
        }
    }
    Ok(())
}

fn editable_document(input: &str) -> Result<toml_edit::DocumentMut, String> {
    let document = input
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| error.to_string())?;
    if document.to_string() != input {
        return Err("Provider syntax cannot be edited without unrelated formatting changes".into());
    }
    Ok(document)
}

fn edit_detach_documents(
    name: &str,
    lock: &str,
    manifest: &str,
) -> Result<DotagentsDetachProposal, String> {
    let mut lock = editable_document(lock)?;
    lock.get_mut("skills")
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or("Lock skills cannot be edited as a table")?
        .remove(name)
        .ok_or("Selected lock entry is missing")?;
    let mut manifest = editable_document(manifest)?;
    let skills = manifest
        .get_mut("skills")
        .ok_or("Manifest skills are missing")?;
    if let Some(rows) = skills.as_array_of_tables_mut() {
        rows.retain(|row| row.get("name").and_then(toml_edit::Item::as_str) != Some(name));
    } else if let Some(rows) = skills.as_array_mut() {
        rows.retain(|row| {
            row.as_inline_table()
                .and_then(|row| row.get("name"))
                .and_then(toml_edit::Value::as_str)
                != Some(name)
        });
    } else {
        return Err("Manifest skills cannot be edited as an array".into());
    }
    Ok(DotagentsDetachProposal {
        lock: lock.to_string(),
        manifest: manifest.to_string(),
    })
}

fn detach_remainder(
    name: &str,
    mut lock: toml::Value,
    mut manifest: toml::Value,
) -> Result<(toml::Value, toml::Value), String> {
    if let Some(skills) = lock.get_mut("skills") {
        let skills = skills.as_table_mut().ok_or("Lock skills must be a table")?;
        skills.remove(name);
        if skills.is_empty() {
            lock.as_table_mut()
                .ok_or("Lock must be a table")?
                .remove("skills");
        }
    }
    if let Some(skills) = manifest.get_mut("skills") {
        let skills = skills
            .as_array_mut()
            .ok_or("Manifest skills must be an array")?;
        skills.retain(|skill| skill.get("name").and_then(toml::Value::as_str) != Some(name));
        if skills.is_empty() {
            manifest
                .as_table_mut()
                .ok_or("Manifest must be a table")?
                .remove("skills");
        }
    }
    Ok((lock, manifest))
}

fn detach_rows(
    name: &str,
    lock: &str,
    manifest: &str,
) -> Result<(Option<toml::Value>, Option<toml::Value>), String> {
    let (lock, manifest) = parse_detach_documents(lock, manifest)?;
    detach_rows_in(name, &lock, &manifest)
}

fn parse_detach_documents(
    lock: &str,
    manifest: &str,
) -> Result<(toml::Value, toml::Value), String> {
    if lock.len() > MAX_DETACH_BYTES || manifest.len() > MAX_DETACH_BYTES {
        return Err("Detach input exceeds its limit".into());
    }
    let lock: toml::Value = toml::from_str(lock).map_err(|error| error.to_string())?;
    let manifest: toml::Value = toml::from_str(manifest).map_err(|error| error.to_string())?;
    Ok((lock, manifest))
}

fn detach_rows_in(
    name: &str,
    lock: &toml::Value,
    manifest: &toml::Value,
) -> Result<(Option<toml::Value>, Option<toml::Value>), String> {
    let locked = match lock.get("skills") {
        None => None,
        Some(value) => value
            .as_table()
            .ok_or("Lock skills must be a table")?
            .get(name)
            .cloned(),
    };
    if locked.as_ref().is_some_and(|row| !row.is_table()) {
        return Err("Locked skill must be a table".into());
    }
    let mut declared = None;
    if let Some(rows) = manifest.get("skills") {
        for row in rows.as_array().ok_or("Manifest skills must be an array")? {
            let row_name = row
                .get("name")
                .and_then(toml::Value::as_str)
                .ok_or("Manifest skill has no name")?;
            if row_name == name {
                if declared.is_some() {
                    return Err("Detach manifest has duplicate names".into());
                }
                declared = Some(row.clone());
            }
        }
    }
    Ok((locked, declared))
}

pub(crate) fn manifest_refs(manifest: &AgentsManifest) -> BTreeMap<String, Option<String>> {
    manifest
        .skills
        .iter()
        .map(|skill| (skill.name.clone(), skill.r#ref.clone()))
        .collect()
}

/// "owner/repo" -> `Some("owner/repo")`; `"git:https://github.com/o/r.git"` ->
/// `Some("o/r")`; any other host (or a source shape that isn't a plain repo
/// slug) -> `None`.
pub fn github_repo_from_source(source: &str) -> Option<String> {
    let slug = if let Some(url) = source.strip_prefix("git:") {
        let path = [
            "https://github.com/",
            "http://github.com/",
            "ssh://git@github.com/",
            "ssh://github.com/",
        ]
        .iter()
        .find_map(|prefix| url.strip_prefix(prefix))?;
        path.trim_end_matches('/')
            .strip_suffix(".git")
            .unwrap_or(path.trim_end_matches('/'))
    } else {
        source
    };
    let (owner, repo) = slug.split_once('/')?;
    let valid_owner = !owner.is_empty()
        && owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    let valid_repo = !repo.is_empty()
        && !matches!(repo, "." | "..")
        && repo
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    (valid_owner && valid_repo).then(|| slug.to_owned())
}

/// Read `agents.lock` and `agents.toml` from `agents_dir` (normally
/// `~/.agents`) and join them by skill name. Either file missing yields an
/// empty `Vec`, not an error - most machines have no dotagents skills at all.
pub fn read_dotagents_ledger(agents_dir: &Path) -> Result<Vec<DotagentsSkill>, String> {
    let lock = read_agents_lock(agents_dir)?.unwrap_or_default();
    let manifest = read_agents_manifest(agents_dir)?.unwrap_or_default();
    Ok(join_dotagents_ledger(lock, manifest))
}

/// Read each dotagents input independently. `None` means the named input was
/// absent; callers that need ownership certainty must preserve errors rather
/// than replacing them with empty data.
pub fn read_agents_lock(agents_dir: &Path) -> Result<Option<AgentsLock>, String> {
    read_toml(&agents_dir.join("agents.lock"), parse_agents_lock)
}

pub fn read_agents_manifest(agents_dir: &Path) -> Result<Option<AgentsManifest>, String> {
    read_toml(&agents_dir.join("agents.toml"), parse_agents_manifest)
}

pub(crate) fn parse_agents_lock(content: &str, path: &Path) -> Result<AgentsLock, String> {
    parse_toml(content, path)
}

pub(crate) fn parse_agents_manifest(content: &str, path: &Path) -> Result<AgentsManifest, String> {
    let manifest: AgentsManifest = parse_toml(content, path)?;
    let mut names = std::collections::HashSet::new();
    for skill in &manifest.skills {
        if !names.insert(&skill.name) {
            return Err(format!(
                "Duplicate skill name {} in {}",
                skill.name,
                path.display()
            ));
        }
    }
    Ok(manifest)
}

pub fn join_dotagents_ledger(lock: AgentsLock, manifest: AgentsManifest) -> Vec<DotagentsSkill> {
    let declared_refs = manifest_refs(&manifest);

    let mut skills: Vec<DotagentsSkill> = lock
        .skills
        .into_iter()
        .map(|(name, locked)| {
            let github_repo = github_repo_from_source(&locked.source);
            let manifest_row = declared_refs.get(&name);
            let declared_ref = manifest_row.cloned().flatten();
            let has_manifest_row = manifest_row.is_some();
            DotagentsSkill {
                name,
                source: locked.source,
                github_repo,
                path: locked.resolved_path.unwrap_or_default(),
                installed_commit: locked.resolved_commit,
                declared_ref,
                has_manifest_row,
            }
        })
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Reads and parses `path` as TOML into `T`, or `Ok(None)` when `path` doesn't
/// exist.
fn read_toml<T>(
    path: &Path,
    parse: impl FnOnce(&str, &Path) -> Result<T, String>,
) -> Result<Option<T>, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Failed to read {}: {error}", path.display())),
    };
    parse(&content, path).map(Some)
}

fn parse_toml<T: for<'de> Deserialize<'de>>(content: &str, path: &Path) -> Result<T, String> {
    toml::from_str(content).map_err(|error| format!("Failed to parse {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn reattach_merges_only_selected_rows_and_resumes_partial_publication() {
        let staged_lock = format!("version = 1\n[skills.alpha]\nsource = 'owner/repo'\nresolved_commit = '{}'\nunknown_provider_field = 7\n", "b".repeat(40));
        let staged_manifest =
            "version = 1\n[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\nref = 'main'\n";
        let staged =
            DotagentsDetachIntent::from_documents("alpha", &staged_lock, staged_manifest).unwrap();
        for (lock, manifest) in [
            (
                "version = 1\n# keep lock\n[skills.beta]\nsource = 'other/repo'\n",
                "version = 1\n# keep manifest\n[[skills]]\nname = 'beta'\nsource = 'other/repo'\n",
            ),
            (
                "version = 1\nskills = { beta = { source = 'other/repo' } } # keep lock\n",
                "version = 1\nskills = [{name = 'beta', source = 'other/repo'}] # keep manifest\n",
            ),
            (
                "version = 1\n# keep lock\n[future]\nenabled = true\n",
                "version = 1\n# keep manifest\n[trust]\ngithub_orgs = ['other']\n",
            ),
        ] {
            let proposed = staged.propose_document_reattach(lock, manifest).unwrap();
            assert!(proposed.lock().contains("# keep lock"));
            assert!(proposed.manifest().contains("# keep manifest"));
            let reattached = DotagentsDetachIntent::from_documents(
                "alpha",
                proposed.lock(),
                proposed.manifest(),
            )
            .unwrap();
            assert_eq!(reattached.locked, staged.locked);
            assert_eq!(reattached.declared, staged.declared);
            assert_eq!(
                staged
                    .observe_document_effects(
                        proposed.lock(),
                        proposed.manifest(),
                        Some(lock),
                        Some(manifest)
                    )
                    .unwrap(),
                DotagentsDetachState::Detached
            );
            assert_eq!(
                staged
                    .propose_document_reattach(proposed.lock(), proposed.manifest())
                    .unwrap(),
                proposed
            );
            assert_eq!(
                staged
                    .propose_document_reattach(proposed.lock(), manifest)
                    .unwrap(),
                proposed
            );
            assert_eq!(
                staged
                    .propose_document_reattach(lock, proposed.manifest())
                    .unwrap(),
                proposed
            );
            let changed = proposed.lock().replace("owner/repo", "changed/repo");
            assert!(staged
                .propose_document_reattach(&changed, proposed.manifest())
                .is_err());
        }
        for (lock, manifest) in [
            ("version = 2\n", "version = 1\n"),
            (
                "version = 1\n[plugins.alpha]\nsource = 'plugin/repo'\n",
                "version = 1\n",
            ),
            (
                "version = 1\n",
                "version = 1\n[[plugins]]\nname = 'alpha'\n",
            ),
            (
                "version = 1\n",
                "version = 1\n[[skills]]\nname = 'alpha'\n[[skills]]\nname = 'alpha'\n",
            ),
        ] {
            assert!(staged.propose_document_reattach(lock, manifest).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn reattachment_accepts_captured_staged_dotagents_git_records() {
        use crate::{skill_coordination::CancellationToken, skill_scope::SkillReadScope};
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/dotagents-staged-add-3.0.1.json"
        ))
        .unwrap();
        let mut checked = 0;
        for case in fixture["cases"].as_array().unwrap() {
            if !case["mode"].as_str().unwrap().starts_with("github-") {
                continue;
            }
            assert_eq!(case["exit_code"], 0);
            assert_eq!(case["outside_write_denied"], true);
            let lock = case["lock"].as_str().unwrap();
            let manifest = case["manifest"].as_str().unwrap();
            let installed_commit = case["expected_commit"].as_str().unwrap();
            let before = lock.replace(installed_commit, &"a".repeat(40));
            assert_ne!(before, lock);
            let expected =
                DotagentsDetachIntent::from_documents("alpha", &before, manifest).unwrap();
            let temp = tempfile::tempdir().unwrap();
            std::fs::write(temp.path().join("agents.lock"), lock).unwrap();
            std::fs::write(temp.path().join("agents.toml"), manifest).unwrap();
            let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
            let cancellation = CancellationToken::default();
            let observed = expected
                .observe_reinstalled(&scope, temp.path(), &cancellation)
                .unwrap();
            assert_eq!(observed.source().commit(), installed_commit);
            assert_eq!(observed.source().source(), "fixture/skills");
            assert_eq!(observed.source().path(), "skills/alpha");
            assert_eq!(
                observed.source().declared_ref(),
                case["declared_ref"].as_str()
            );
            observed.revalidate(&cancellation).unwrap();
            let selected = observed.selected_rows(&cancellation).unwrap();
            assert_eq!(selected.name(), "alpha");
            assert_eq!(selected.fork_source().unwrap(), *observed.source());
            let record = selected.to_record_json().unwrap();
            assert_eq!(
                record,
                DotagentsDetachIntent::from_documents("alpha", lock, manifest)
                    .unwrap()
                    .to_record_json()
                    .unwrap()
            );
            let restored = DotagentsDetachIntent::from_record_json(&record).unwrap();
            let proposal = restored
                .propose_document_reattach("version = 1\n", "version = 1\n")
                .unwrap();
            assert_eq!(
                restored
                    .reinstalled_source(proposal.lock(), proposal.manifest())
                    .unwrap(),
                *observed.source()
            );
            checked += 1;
        }
        assert_eq!(checked, 2);
    }

    #[cfg(unix)]
    #[test]
    fn scoped_reattachment_refuses_replacement_escape_missing_and_cancelled_inputs() {
        use crate::{skill_coordination::CancellationToken, skill_scope::SkillReadScope};
        use std::{fs, os::unix::fs::symlink};
        let lock = format!(
            "[skills.alpha]\nsource = 'owner/repo'\nresolved_commit = '{}'\n",
            "a".repeat(40)
        );
        let manifest = "[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\n";
        let expected = DotagentsDetachIntent::from_documents("alpha", &lock, manifest).unwrap();
        for change in [
            "replace-lock",
            "change-bookkeeping",
            "change-manifest",
            "replace-root",
            "escape",
            "missing",
            "oversize",
            "cancel",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let agents = temp.path().join("agents");
            fs::create_dir(&agents).unwrap();
            fs::write(agents.join("agents.lock"), &lock).unwrap();
            fs::write(agents.join("agents.toml"), manifest).unwrap();
            let scope = SkillReadScope::bind(std::slice::from_ref(&agents)).unwrap();
            let cancellation = CancellationToken::default();
            let observed = expected
                .observe_reinstalled(&scope, &agents, &cancellation)
                .unwrap();
            assert_eq!(observed.source().commit(), "a".repeat(40));
            observed.revalidate(&cancellation).unwrap();
            match change {
                "replace-lock" => {
                    fs::write(agents.join("replacement"), &lock).unwrap();
                    fs::rename(agents.join("replacement"), agents.join("agents.lock")).unwrap();
                }
                "change-bookkeeping" => fs::write(
                    agents.join("agents.lock"),
                    format!("{lock}provider_note = 'changed'\n"),
                )
                .unwrap(),
                "change-manifest" => fs::write(
                    agents.join("agents.toml"),
                    format!("{manifest}ref = 'other'\n"),
                )
                .unwrap(),
                "replace-root" => {
                    fs::rename(&agents, temp.path().join("old-agents")).unwrap();
                    fs::create_dir(&agents).unwrap();
                }
                "escape" => {
                    fs::write(temp.path().join("outside"), &lock).unwrap();
                    fs::remove_file(agents.join("agents.lock")).unwrap();
                    symlink(temp.path().join("outside"), agents.join("agents.lock")).unwrap();
                }
                "missing" => fs::remove_file(agents.join("agents.toml")).unwrap(),
                "oversize" => {
                    fs::write(agents.join("agents.lock"), vec![b' '; MAX_DETACH_BYTES + 1]).unwrap()
                }
                "cancel" => cancellation.cancel(),
                _ => unreachable!(),
            }
            assert!(observed.revalidate(&cancellation).is_err(), "{change}");
            assert!(observed.selected_rows(&cancellation).is_err(), "{change}");
            if matches!(change, "replace-lock" | "change-bookkeeping") {
                assert!(expected
                    .observe_reinstalled(&scope, &agents, &cancellation)
                    .is_ok());
            } else {
                assert!(
                    expected
                        .observe_reinstalled(&scope, &agents, &cancellation)
                        .is_err(),
                    "{change}"
                );
            }
            fs::create_dir(temp.path().join("unrelated")).unwrap();
            let outside_scope = SkillReadScope::bind(&[temp.path().join("unrelated")]).unwrap();
            assert!(expected
                .observe_reinstalled(&outside_scope, &agents, &CancellationToken::default())
                .is_err());
        }
    }

    #[test]
    fn reinstall_request_accepts_legacy_origin_and_new_resolved_commit() {
        use crate::skill_fork_registry::{ForkRecord, OriginTool};
        let mut record = ForkRecord {
            deployment_id: String::new(),
            skill_dir: Default::default(),
            forked_at: String::new(),
            origin_tool: OriginTool::Dotagents,
            origin_source: "git:https://github.com/owner/repo.git".into(),
            repo: "owner/repo".into(),
            path: ".".into(),
            declared_ref: Some("release/stable".into()),
            base_commit: "a".repeat(40),
        };
        let request = DotagentsReinstallRequest::from_fork_record(&record, "alpha").unwrap();
        assert_eq!(request.path(), "");
        assert_eq!(request.source(), record.origin_source);
        request
            .validate_detached_documents("version = 1\n", "version = 1\n")
            .unwrap();
        assert!(request
            .validate_detached_documents(
                "version = 1\n",
                "version = 1\n[[skills]]\nsource = 'owner/repo'\n"
            )
            .is_err());
        assert!(request
            .validate_detached_documents(
                "version = 1\n[skills.alpha]\nsource = 'owner/repo'\n",
                "version = 1\n"
            )
            .is_err());
        let encoded = serde_json::to_vec(&request).unwrap();
        assert_eq!(
            serde_json::from_slice::<DotagentsReinstallRequest>(&encoded).unwrap(),
            request
        );
        for invalid in [
            String::from_utf8(encoded.clone())
                .unwrap()
                .replace("\"repo\":\"owner/repo\"", "\"repo\":\"other/repo\""),
            String::from_utf8(encoded.clone())
                .unwrap()
                .replace("{", "{\"unknown\":true,"),
        ] {
            assert!(serde_json::from_str::<DotagentsReinstallRequest>(&invalid).is_err());
        }
        let lock = format!(
            "[skills.alpha]\nsource = '{}'\nresolved_path = '.'\nresolved_commit = '{}'\n",
            record.origin_source,
            "b".repeat(40)
        );
        let manifest = format!(
            "[[skills]]\nname = 'alpha'\nsource = '{}'\nref = 'release/stable'\n",
            record.origin_source
        );
        assert_eq!(
            request
                .reinstalled_source(&lock, &manifest)
                .unwrap()
                .commit(),
            "b".repeat(40)
        );
        assert!(request
            .reinstalled_source(&lock, &manifest.replace("release/stable", "other"))
            .is_err());
        assert!(request
            .reinstalled_source(
                &lock.replace("resolved_path = '.'", "resolved_path = 'different'"),
                &manifest
            )
            .is_err());
        let original = DotagentsDetachIntent::from_documents("alpha", &lock, &manifest).unwrap();
        assert_eq!(
            DotagentsReinstallRequest::from_detach(&original).unwrap(),
            request
        );
        record.repo = "different/repo".into();
        assert!(DotagentsReinstallRequest::from_fork_record(&record, "alpha").is_err());
        record.repo = "owner/repo".into();
        for path in ["../escape", "/absolute", "a//b", "a/../b", "a\\b"] {
            record.path = path.into();
            assert!(DotagentsReinstallRequest::from_fork_record(&record, "alpha").is_err());
        }
        record.path = String::new();
        for reference in ["", "--help", "line\nref"] {
            record.declared_ref = Some(reference.into());
            assert!(DotagentsReinstallRequest::from_fork_record(&record, "alpha").is_err());
        }
        record.declared_ref = None;
        for name in ["../alpha", "--help", "alpha\0"] {
            assert!(DotagentsReinstallRequest::from_fork_record(&record, name).is_err());
        }
        record.origin_source = "--help/repo".into();
        record.repo = "--help/repo".into();
        assert!(DotagentsReinstallRequest::from_fork_record(&record, "alpha").is_err());
        record.origin_source = "owner/repo".into();
        record.repo = "owner/repo".into();
        record.origin_tool = OriginTool::SkillsSh;
        assert!(DotagentsReinstallRequest::from_fork_record(&record, "alpha").is_err());
    }

    #[test]
    fn reinstalled_source_requires_matching_named_provider_records() {
        let lock = format!("[skills.alpha]\nsource = 'owner/repo'\nresolved_path = 'skills/alpha'\nresolved_commit = '{}'\n", "a".repeat(40));
        let manifest = "[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\nref = 'main'\n";
        let original = DotagentsDetachIntent::from_documents("alpha", &lock, manifest).unwrap();
        let advanced = lock.replace(&"a".repeat(40), &"b".repeat(40));
        let observed = original.reinstalled_source(&advanced, manifest).unwrap();
        assert_eq!(observed.commit(), "b".repeat(40));
        assert_eq!(observed.source(), "owner/repo");
        assert_eq!(observed.path(), "skills/alpha");
        assert_eq!(observed.declared_ref(), Some("main"));
        let changed_source = advanced.replace("owner/repo", "other/repo");
        let changed_path = advanced.replace("skills/alpha", "skills/beta");
        let invalid_commit = advanced.replace(&"b".repeat(40), "short");
        let changed_ref = manifest.replace("'main'", "'other'");
        let changed_manifest_source = manifest.replace("owner/repo", "other/repo");
        let duplicate = format!("{manifest}{manifest}");
        for (case, current_lock, current_manifest) in [
            ("missing lock row", "", manifest),
            ("missing manifest row", advanced.as_str(), ""),
            ("malformed lock", "[", manifest),
            ("different source", changed_source.as_str(), manifest),
            ("different path", changed_path.as_str(), manifest),
            ("invalid resolved commit", invalid_commit.as_str(), manifest),
            (
                "different declared ref",
                advanced.as_str(),
                changed_ref.as_str(),
            ),
            (
                "contradictory manifest source",
                advanced.as_str(),
                changed_manifest_source.as_str(),
            ),
            (
                "duplicate manifest row",
                advanced.as_str(),
                duplicate.as_str(),
            ),
        ] {
            assert!(
                original
                    .reinstalled_source(current_lock, current_manifest)
                    .is_err(),
                "{case}"
            );
        }
        let unrelated_lock = format!("{advanced}\n[skills.beta]\nsource = 'other/repo'\n");
        let unrelated_manifest =
            format!("{manifest}\n[[skills]]\nname = 'beta'\nsource = 'other/repo'\n");
        assert_eq!(
            original
                .reinstalled_source(&unrelated_lock, &unrelated_manifest)
                .unwrap(),
            observed
        );
    }

    #[test]
    fn github_repo_from_source_handles_plain_slug() {
        assert_eq!(
            github_repo_from_source("getsentry/agent-browser"),
            Some("getsentry/agent-browser".to_string())
        );
    }

    #[test]
    fn github_repo_from_source_handles_git_url() {
        assert_eq!(
            github_repo_from_source("git:https://github.com/getsentry/agent-browser.git"),
            Some("getsentry/agent-browser".to_string())
        );
    }

    #[test]
    fn reinstall_cache_path_matches_provider_source_formats() {
        for (source, cache) in [
            ("owner/repo", "owner/repo"),
            (
                "git:https://github.com/owner/repo.git",
                "github.com/owner/repo",
            ),
            (
                "git:http://github.com/owner/repo.git",
                "github.com/owner/repo",
            ),
            (
                "git:ssh://github.com/owner/repo.git",
                "github.com/owner/repo",
            ),
            (
                "git:ssh://git@github.com/owner/repo.git",
                "git@github.com/owner/repo",
            ),
            (
                "git:https://github.com/owner/repo.git/",
                "github.com/owner/repo.git",
            ),
        ] {
            let request = DotagentsReinstallRequest::new(
                "skill",
                source,
                "owner/repo",
                "skills/skill",
                Some("main"),
            )
            .unwrap();
            assert_eq!(
                request.cache_relative_source(),
                std::path::Path::new(cache).join("skills/skill"),
                "{source}"
            );
        }
    }

    #[test]
    fn github_repo_from_source_rejects_non_github_host() {
        assert_eq!(
            github_repo_from_source("git:https://gitlab.com/getsentry/agent-browser.git"),
            None
        );
    }

    #[test]
    fn missing_files_yield_empty_vec() {
        let tmp = tempfile::tempdir().unwrap();
        let skills = read_dotagents_ledger(tmp.path()).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn joins_lock_and_manifest_by_name_pinned_and_unpinned() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("agents.lock"),
            r#"
[skills.agent-browser]
source = "getsentry/agent-browser"
resolved_path = "skills/agent-browser"
resolved_commit = "1111111111111111111111111111111111aaaa"

[skills.find-bugs]
source = "git:https://github.com/getsentry/find-bugs.git"
resolved_path = "skills/find-bugs"
resolved_commit = "2222222222222222222222222222222222bbbb"
"#,
        )
        .unwrap();
        fs::write(
            tmp.path().join("agents.toml"),
            r#"
[[skills]]
name = "agent-browser"
source = "getsentry/agent-browser"
path = "skills/agent-browser"
ref = "1111111111111111111111111111111111aaaa"

[[skills]]
name = "find-bugs"
source = "git:https://github.com/getsentry/find-bugs.git"
path = "skills/find-bugs"
"#,
        )
        .unwrap();

        let mut skills = read_dotagents_ledger(tmp.path()).unwrap();
        skills.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "agent-browser");
        assert_eq!(
            skills[0].github_repo,
            Some("getsentry/agent-browser".to_string())
        );
        assert_eq!(
            skills[0].declared_ref,
            Some("1111111111111111111111111111111111aaaa".to_string())
        );
        assert!(skills[0].has_manifest_row);
        assert_eq!(skills[1].name, "find-bugs");
        assert_eq!(
            skills[1].github_repo,
            Some("getsentry/find-bugs".to_string())
        );
        assert_eq!(skills[1].declared_ref, None);
        assert!(skills[1].has_manifest_row);
    }

    #[test]
    fn wildcard_entry_has_no_manifest_row() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("agents.lock"),
            r#"
[skills.some-wildcard-skill]
source = "getsentry/some-repo"
resolved_path = "skills/some-wildcard-skill"
resolved_commit = "3333333333333333333333333333333333cccc"
"#,
        )
        .unwrap();
        // No agents.toml at all - the wildcard case.
        let skills = read_dotagents_ledger(tmp.path()).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].declared_ref, None);
    }
}

#[cfg(test)]
mod detach_tests {
    use super::*;
    #[test]
    fn fork_source_uses_installed_commit_and_refuses_unsafe_source_data() {
        let lock = format!("[skills.alpha]\nsource = 'owner/repo'\nresolved_path = 'skills/alpha'\nresolved_commit = '{}'\n", "a".repeat(40));
        let manifest = "[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\nref = 'main'\n";
        let source = DotagentsDetachIntent::from_documents("alpha", &lock, manifest)
            .unwrap()
            .fork_source()
            .unwrap();
        assert_eq!(source.repo(), "owner/repo");
        assert_eq!(source.commit(), "a".repeat(40));
        assert_eq!(source.path(), "skills/alpha");
        assert_eq!(source.declared_ref(), Some("main"));
        let root_source = DotagentsDetachIntent::from_documents(
            "alpha",
            &lock.replace("skills/alpha", "."),
            manifest,
        )
        .unwrap()
        .fork_source()
        .unwrap();
        assert_eq!(root_source.path(), "");
        for changed in [
            lock.replace(&"a".repeat(40), "main"),
            lock.replace(&"a".repeat(40), &"A".repeat(40)),
            lock.replace("skills/alpha", "../alpha"),
            lock.replace("skills/alpha", "/alpha"),
            lock.replace(
                "owner/repo",
                "git:https://evil.example/github.com/owner/repo",
            ),
        ] {
            assert!(
                DotagentsDetachIntent::from_documents("alpha", &changed, manifest)
                    .unwrap()
                    .fork_source()
                    .is_err()
            );
        }
        for source in [
            "git:https://github.com.evil/owner/repo",
            "git:https://evil/github.com/owner/repo",
            "git:https://github.com/owner/repo?ref=main",
            "owner/../repo",
            "owner/repo/extra",
            "../repo",
        ] {
            assert_eq!(github_repo_from_source(source), None, "{source}");
        }
        assert_eq!(
            github_repo_from_source("git:ssh://git@github.com/owner/repo.git"),
            Some("owner/repo".into())
        );
    }

    #[test]
    fn persisted_detach_retains_toml_types_and_selected_unknown_fields() {
        let lock = "[skills.alpha]\nsource = 'owner/repo'\nstamp = 2026-09-11T12:00:00Z\nfuture = { enabled = true, count = 9 }\n[skills.other]\nsource = 'unrelated/repo'\n";
        let manifest = "[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\nvalues = [1, 2, 3]\n";
        let original = DotagentsDetachIntent::from_documents("alpha", lock, manifest).unwrap();
        let json = original.to_record_json().unwrap();
        assert!(!json.contains("unrelated/repo"));
        let restored = DotagentsDetachIntent::from_record_json(&json).unwrap();
        assert_eq!(
            restored.observe(Some(lock), Some(manifest)).unwrap(),
            DotagentsDetachState::Attached
        );
        assert_eq!(
            restored.observe(Some(lock), Some("")).unwrap(),
            DotagentsDetachState::Partial
        );
        assert_eq!(
            restored.observe(Some(""), Some("")).unwrap(),
            DotagentsDetachState::Detached
        );
        assert_eq!(
            restored
                .observe(
                    Some(&lock.replace("count = 9", "count = 10")),
                    Some(manifest)
                )
                .unwrap(),
            DotagentsDetachState::Changed
        );
        let record: serde_json::Value = serde_json::from_str(&json).unwrap();
        for (key, value) in [
            ("version", serde_json::json!(2)),
            ("name", serde_json::json!("../alpha")),
            ("name", serde_json::json!("other")),
            ("locked", serde_json::json!("invalid TOML")),
            ("declared", serde_json::json!("name = 'other'")),
            ("extra", serde_json::json!(true)),
        ] {
            let mut changed = record.clone();
            changed[key] = value;
            assert!(DotagentsDetachIntent::from_record_json(&changed.to_string()).is_err());
        }
        assert!(
            DotagentsDetachIntent::from_record_json(&" ".repeat(MAX_DETACH_BYTES + 1)).is_err()
        );
    }

    #[test]
    fn detach_distinguishes_complete_partial_changed_and_missing_evidence() {
        let lock = "[skills.alpha]\nsource = 'owner/repo'\nresolved_commit = 'base'\n";
        let manifest = "[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\nref = 'main'\n";
        let intent = DotagentsDetachIntent::from_documents("alpha", lock, manifest).unwrap();
        for (lock, manifest, state) in [
            (Some(lock), Some(manifest), DotagentsDetachState::Attached),
            (Some(""), Some(""), DotagentsDetachState::Detached),
            (Some(lock), Some(""), DotagentsDetachState::Partial),
            (Some(""), Some(manifest), DotagentsDetachState::Partial),
            (None, Some(""), DotagentsDetachState::Unavailable),
            (Some(""), None, DotagentsDetachState::Unavailable),
        ] {
            assert_eq!(intent.observe(lock, manifest).unwrap(), state);
        }
        let changed = manifest.replace("owner/repo", "other/repo");
        assert_eq!(
            intent.observe(Some(lock), Some(&changed)).unwrap(),
            DotagentsDetachState::Changed
        );
        let changed = format!("{lock}future_field = true\n");
        assert_eq!(
            intent.observe(Some(&changed), Some(manifest)).unwrap(),
            DotagentsDetachState::Changed
        );
        let unrelated = format!("{lock}[skills.other]\nsource = 'another/repo'\n");
        assert_eq!(
            intent.observe(Some(&unrelated), Some(manifest)).unwrap(),
            DotagentsDetachState::Attached
        );
        assert!(intent
            .observe(Some("skills = false"), Some(manifest))
            .is_err());
        assert!(intent
            .observe(Some(lock), Some(&format!("{manifest}{manifest}")))
            .is_err());
        assert!(DotagentsDetachIntent::from_documents("alpha", lock, "").is_err());
        assert!(DotagentsDetachIntent::from_documents("alpha", "", manifest).is_err());
        assert!(intent
            .observe(Some(&" ".repeat(8 * 1024 * 1024 + 1)), Some(manifest))
            .is_err());
    }
    #[test]
    fn document_detach_proposal_preserves_unrelated_values_and_rejects_ambiguous_names() {
        let lock = "version = 1\nfuture = 'keep'\n[skills.alpha]\nsource = 'owner/repo'\n[skills.beta]\nsource = 'other/repo'\n[plugins.example]\nsource = 'plugin/repo'\n";
        let manifest = "version = 1\nagents = ['codex']\n[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\n[[skills]]\nname = 'beta'\nsource = 'other/repo'\n[[plugins]]\nname = 'example'\nsource = 'plugin/repo'\n";
        let intent = DotagentsDetachIntent::from_documents("alpha", lock, manifest).unwrap();
        let proposal = intent.propose_document_detach(lock, manifest).unwrap();
        let parsed: toml::Value = toml::from_str(proposal.lock()).unwrap();
        assert_eq!(parsed["future"].as_str(), Some("keep"));
        assert_eq!(
            parsed["skills"]["beta"]["source"].as_str(),
            Some("other/repo")
        );
        assert!(parsed["skills"].get("alpha").is_none());
        assert_eq!(
            intent
                .observe_document_effects(
                    lock,
                    manifest,
                    Some(proposal.lock()),
                    Some(proposal.manifest())
                )
                .unwrap(),
            DotagentsDetachState::Detached
        );
        assert!(intent
            .propose_document_detach(proposal.lock(), proposal.manifest())
            .is_err());
        for changed in [
            lock.replace("version = 1", "version = 2"),
            lock.replace("version = 1\n", ""),
            lock.replace("plugins.example", "plugins.alpha"),
        ] {
            assert!(intent.propose_document_detach(&changed, manifest).is_err());
        }
        assert!(intent
            .propose_document_detach(
                lock,
                &manifest.replace("name = 'example'", "name = 'alpha'")
            )
            .is_err());
        assert!(intent
            .propose_document_detach(
                &lock.replace("source = 'owner/repo'", "source = 'changed/repo'"),
                manifest
            )
            .is_err());
    }

    #[test]
    fn document_detach_keeps_unrelated_comments_quotes_and_order() {
        let lock = "# lock header\nversion  =  1 # version note\nfuture = 'keep'\n\n[skills.'alpha'] # removed\nsource = 'owner/repo'\n[skills.'alpha'.options]\nfuture = true\n\n# beta notes\n[skills.beta]\nsource= \"other/repo\" # keep this\n\n# trailing notes\n";
        let manifest = "# config header\nversion=1\n\n# selected notes\n[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\n\n# beta notes\n[[skills]]\nname=\"beta\"\nsource = 'other/repo' # keep source\n\n# plugin notes\n[[plugins]]\nname = 'example'\nsource = 'plugin/repo'\n";
        let normalized_key = lock.replace("[skills.'alpha'.options]", "[skills.alpha.options]");
        assert!(
            DotagentsDetachIntent::from_documents("alpha", &normalized_key, manifest)
                .unwrap()
                .propose_document_detach(&normalized_key, manifest)
                .is_err()
        );
        let proposal = DotagentsDetachIntent::from_documents("alpha", lock, manifest)
            .unwrap()
            .propose_document_detach(lock, manifest)
            .unwrap();
        assert_eq!(proposal.lock(), "# lock header\nversion  =  1 # version note\nfuture = 'keep'\n\n# beta notes\n[skills.beta]\nsource= \"other/repo\" # keep this\n\n# trailing notes\n");
        assert_eq!(proposal.manifest(), "# config header\nversion=1\n\n# beta notes\n[[skills]]\nname=\"beta\"\nsource = 'other/repo' # keep source\n\n# plugin notes\n[[plugins]]\nname = 'example'\nsource = 'plugin/repo'\n");
        let inline_lock = "version = 1\nskills = { alpha = { source = 'owner/repo' }, beta = { source = 'other/repo' } } # table note\n";
        let inline_manifest = "version = 1\nskills = [{ name = 'alpha', source = 'owner/repo' }, { name = 'beta', source = 'other/repo' }] # array note\n";
        let proposal = DotagentsDetachIntent::from_documents("alpha", inline_lock, inline_manifest)
            .unwrap()
            .propose_document_detach(inline_lock, inline_manifest)
            .unwrap();
        assert!(proposal.lock().contains("beta = { source = 'other/repo' }"));
        assert!(proposal.lock().ends_with("# table note\n"));
        assert!(proposal
            .manifest()
            .contains("{ name = 'beta', source = 'other/repo' }"));
        assert!(proposal.manifest().ends_with("# array note\n"));
    }

    #[test]
    fn observes_captured_dotagents_process_results() {
        let evidence: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/dotagents-detach-3.0.1.json"
        ))
        .unwrap();
        let cases = evidence["cases"].as_array().unwrap();
        let failed = cases
            .iter()
            .find(|case| case["case"] == "lock-write-denied")
            .unwrap();
        let original_manifest =
            "version = 1\n[[skills]]\nname = \"alpha\"\nsource = \"owner/repo\"\n";
        let intent = DotagentsDetachIntent::from_documents(
            "alpha",
            failed["lock"].as_str().unwrap(),
            original_manifest,
        )
        .unwrap();
        let intent =
            DotagentsDetachIntent::from_record_json(&intent.to_record_json().unwrap()).unwrap();
        assert_eq!(cases.len(), 2);
        for case in cases {
            let observed = intent
                .observe_document_effects(
                    failed["lock"].as_str().unwrap(),
                    original_manifest,
                    Some(case["lock"].as_str().unwrap()),
                    Some(case["config"].as_str().unwrap()),
                )
                .unwrap();
            assert_eq!(
                observed,
                if case["case"] == "success" {
                    DotagentsDetachState::Detached
                } else {
                    DotagentsDetachState::Partial
                }
            );
            assert_eq!(case["skill_exists"], false);
            assert_eq!(case["outside_canary_unchanged"], true);
        }
    }
    #[test]
    fn detach_effects_preserve_unrelated_provider_values() {
        let lock = "version = 1\nfuture = 'keep'\n[skills.alpha]\nsource = 'owner/repo'\n[skills.beta]\nsource = 'other/repo'\n[plugins.tool]\nsource = 'plugin/repo'\n";
        let manifest = "version = 1\nagents = ['codex']\n[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\n[[skills]]\nname = 'beta'\nsource = 'other/repo'\n";
        let remaining_lock = "# rewritten\nversion = 1\nfuture = 'keep'\n[skills.beta]\nsource = 'other/repo'\n[plugins.tool]\nsource = 'plugin/repo'\n";
        let remaining_manifest =
            "version = 1\nagents = ['codex']\n[[skills]]\nname = 'beta'\nsource = 'other/repo'\n";
        let intent = DotagentsDetachIntent::from_documents("alpha", lock, manifest).unwrap();
        assert_eq!(
            intent
                .observe_document_effects(lock, manifest, Some(lock), Some(manifest))
                .unwrap(),
            DotagentsDetachState::Attached
        );
        assert_eq!(
            intent
                .observe_document_effects(
                    lock,
                    manifest,
                    Some(remaining_lock),
                    Some(remaining_manifest)
                )
                .unwrap(),
            DotagentsDetachState::Detached
        );
        assert_eq!(
            intent
                .observe_document_effects(lock, manifest, Some(lock), Some(remaining_manifest))
                .unwrap(),
            DotagentsDetachState::Partial
        );
        for (changed_lock, changed_manifest) in [
            (
                remaining_lock.replace("future = 'keep'\n", ""),
                remaining_manifest.to_string(),
            ),
            (
                remaining_lock.replace("other/repo", "changed/repo"),
                remaining_manifest.to_string(),
            ),
            (
                remaining_lock.replace("plugin/repo", "changed/plugin"),
                remaining_manifest.to_string(),
            ),
            (
                remaining_lock.replace("version = 1", "version = 2"),
                remaining_manifest.to_string(),
            ),
            (
                remaining_lock.to_string(),
                remaining_manifest.replace("codex", "claude"),
            ),
            (
                remaining_lock.to_string(),
                remaining_manifest.replace("other/repo", "changed/repo"),
            ),
        ] {
            // Selected rows alone still look detached in each unrelated-change case.
            assert_eq!(
                intent
                    .observe(Some(&changed_lock), Some(&changed_manifest))
                    .unwrap(),
                DotagentsDetachState::Detached
            );
            assert_eq!(
                intent
                    .observe_document_effects(
                        lock,
                        manifest,
                        Some(&changed_lock),
                        Some(&changed_manifest)
                    )
                    .unwrap(),
                DotagentsDetachState::Changed
            );
        }
        assert_eq!(
            intent
                .observe_document_effects(lock, manifest, None, Some(remaining_manifest))
                .unwrap(),
            DotagentsDetachState::Unavailable
        );
        assert!(intent
            .observe_document_effects(
                remaining_lock,
                manifest,
                Some(remaining_lock),
                Some(remaining_manifest)
            )
            .is_err());
    }
}
