// ============================================================================
// Skills Module - skill_ownership
// Resolves skills.sh and dotagents ownership against the matching
// scope/root/ledger entry. Same-named deployments elsewhere do not inherit
// ownership. Aggregate grouping by skill name stays presentation-only.
// ============================================================================

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::skill_candidate::{GitRepoEvidence, PluginEvidence, SkillCandidate};
use crate::skill_deployment::InstallScope;
use crate::skill_deployment::{
    agents_root_for_skills_dir, encode_id_path, path_is_under_universal_skills, SkillDestination,
};
use crate::skill_dotagents_ledger::{
    self as dotagents_ledger, AgentsLock, AgentsManifest, DotagentsSkill,
};
use crate::skill_fork_registry::CopyDeploymentRecord;
use crate::skill_inventory::OwnerUpdateSource;
use crate::skill_lock_file::{self as lock_file, SkillLockFile};
use crate::skill_project_lock::{parse_project_lock, ProjectSkillLock};
use crate::skill_provenance::SourceKind;

#[cfg(unix)]
use crate::skill_scope::{ScopedReadError, SkillReadScope};

/// Maximum bytes accepted from each ownership document by the scoped reader.
pub const SCOPED_OWNERSHIP_FILE_BYTE_LIMIT: usize = 16 * 1024 * 1024;

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
    Unknown,
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
            Self::Unknown => "unknown",
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
    pub project_lock: Option<ProjectSkillLock>,
    pub dotagents: Vec<DotagentsSkill>,
}

/// One ownership read. `Absent` is only returned for a direct NotFound read;
/// malformed files and every other I/O error remain visible to classification.
#[derive(Debug, Clone)]
pub enum OwnershipInput<T> {
    Absent,
    Loaded(T),
    Failed(OwnershipReadIssue),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OwnershipReadIssueKind {
    SkillsRoot,
    SkillsShLock,
    DotagentsLock,
    DotagentsManifest,
    LifecycleRegistry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipReadIssue {
    pub kind: OwnershipReadIssueKind,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ScopedOwnershipInputs {
    pub agents_dir: PathBuf,
    /// The physical Universal skills root when it exists. This is resolved
    /// once at the read boundary so linked roots, including a `.agents/skills`
    /// symlink to an external directory, can match a failed ownership input
    /// without turning classification into filesystem work.
    canonical_skills_dir: Option<PathBuf>,
    skills_root: OwnershipInput<PathBuf>,
    pub scope: InstallScope,
    pub project_path: Option<PathBuf>,
    pub skills_sh: OwnershipInput<SkillLockFile>,
    pub project_skills_sh: Option<OwnershipInput<ProjectSkillLock>>,
    skills_sh_conflicts: Vec<OwnershipReadIssue>,
    pub(crate) dotagents: DotagentsInputs,
}

#[derive(Debug, Clone)]
pub(crate) struct DotagentsInputPair {
    root: PathBuf,
    lock: OwnershipInput<AgentsLock>,
    manifest: OwnershipInput<AgentsManifest>,
}

#[derive(Debug, Clone)]
pub(crate) struct DotagentsInputs {
    current: DotagentsInputPair,
    legacy: Option<DotagentsInputPair>,
    conflict: Option<OwnershipReadIssue>,
}

#[derive(Debug, Clone)]
pub struct OwnershipReadReport {
    pub scopes: Vec<ScopedOwnershipInputs>,
    pub registry: OwnershipInput<crate::skill_fork_registry::ForkRegistry>,
}

impl OwnershipReadReport {
    pub fn empty() -> Self {
        Self {
            scopes: Vec::new(),
            registry: OwnershipInput::Absent,
        }
    }

    pub fn global_lock(&self) -> SkillLockFile {
        self.scopes
            .first()
            .and_then(|scope| match &scope.skills_sh {
                OwnershipInput::Loaded(lock) => Some(lock.clone()),
                OwnershipInput::Absent | OwnershipInput::Failed(_) => None,
            })
            .unwrap_or_else(lock_file::empty_lock_file)
    }

    pub fn copy_records(&self) -> std::collections::BTreeMap<String, CopyDeploymentRecord> {
        match &self.registry {
            OwnershipInput::Loaded(registry) => registry.copies.clone(),
            OwnershipInput::Absent | OwnershipInput::Failed(_) => Default::default(),
        }
    }

    pub fn failures(&self) -> Vec<OwnershipReadIssue> {
        let mut failures = Vec::new();
        for scope in &self.scopes {
            append_failure(&mut failures, &scope.skills_root);
            append_failure(&mut failures, &scope.skills_sh);
            if let Some(project_lock) = &scope.project_skills_sh {
                append_failure(&mut failures, project_lock);
            }
            failures.extend(scope.skills_sh_conflicts.iter().cloned());
            scope.dotagents.append_failures(&mut failures);
        }
        if let OwnershipInput::Failed(issue) = &self.registry {
            failures.push(issue.clone());
        }
        failures
    }
}

fn append_failure<T>(failures: &mut Vec<OwnershipReadIssue>, input: &OwnershipInput<T>) {
    if let OwnershipInput::Failed(issue) = input {
        failures.push(issue.clone());
    }
}

fn read_project_ledger(path: &Path) -> Result<Option<ProjectSkillLock>, String> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            for ancestor in path.ancestors() {
                match std::fs::symlink_metadata(ancestor) {
                    Ok(metadata) => {
                        if metadata.is_symlink() {
                            std::fs::metadata(ancestor).map_err(|e| e.to_string())?;
                        }
                        return Ok(None);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            return Err(error.to_string());
        }
        Err(error) => return Err(error.to_string()),
    };
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Project skill ledger is not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(SCOPED_OWNERSHIP_FILE_BYTE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > SCOPED_OWNERSHIP_FILE_BYTE_LIMIT {
        return Err("Project skill ledger exceeds the ownership byte limit".into());
    }
    parse_project_lock(&bytes)
        .map(Some)
        .map_err(|e| e.to_string())
}

fn project_ledger_conflicts(
    agents_dir: &Path,
    legacy: &OwnershipInput<SkillLockFile>,
    project: &Option<OwnershipInput<ProjectSkillLock>>,
) -> Vec<OwnershipReadIssue> {
    let (OwnershipInput::Loaded(legacy), Some(OwnershipInput::Loaded(project))) = (legacy, project)
    else {
        return Vec::new();
    };
    project
        .skills
        .iter()
        .filter_map(|(name, entry)| {
            let previous = legacy.skills.get(name)?;
            let different_url = entry
                .source_url
                .as_ref()
                .is_some_and(|url| !previous.source_url.is_empty() && url != &previous.source_url);
            let different_path = matches!(
                (&entry.skill_path, &previous.skill_path),
                (Some(current), Some(previous)) if current != previous
            );
            (entry.source != previous.source
                || entry.source_type != previous.source_type
                || different_url
                || different_path)
                .then(|| OwnershipReadIssue {
                    kind: OwnershipReadIssueKind::SkillsShLock,
                    path: agents_dir
                        .with_file_name("skills-lock.json")
                        .to_string_lossy()
                        .into_owned(),
                    message: format!(
                        "Conflicting source facts for {name} in project ledger and {}",
                        agents_dir.join(".skill-lock.json").display()
                    ),
                })
        })
        .collect()
}

/// Read the global scope and every explicitly supplied project scope.
pub fn load_ownership_inputs(home: &Path, project_paths: &[PathBuf]) -> OwnershipReadReport {
    let mut scopes = Vec::new();
    scopes.push(read_inputs(
        home.join(".agents"),
        InstallScope::Global,
        None,
    ));
    for project in project_paths {
        let agents_dir = project.join(".agents");
        scopes.push(read_inputs(
            agents_dir,
            InstallScope::Project,
            Some(project.clone()),
        ));
    }
    OwnershipReadReport {
        scopes,
        registry: read_input(
            OwnershipReadIssueKind::LifecycleRegistry,
            crate::skill_fork_registry::fork_registry_path(home),
            || crate::skill_fork_registry::read_fork_registry_optional(home),
        ),
    }
}

fn read_inputs(
    agents_dir: PathBuf,
    scope: InstallScope,
    project_path: Option<PathBuf>,
) -> ScopedOwnershipInputs {
    let canonical_skills_dir = std::fs::canonicalize(agents_dir.join("skills")).ok();
    let skills_root = canonical_skills_dir
        .clone()
        .map(OwnershipInput::Loaded)
        .unwrap_or(OwnershipInput::Absent);
    let skills_sh = read_input(
        OwnershipReadIssueKind::SkillsShLock,
        agents_dir.join(".skill-lock.json"),
        || lock_file::read_lock_file_optional_at(&agents_dir.join(".skill-lock.json")),
    );
    let dotagents = read_dotagents_inputs(&agents_dir, project_path.as_deref());
    let project_skills_sh = project_path.as_ref().map(|project| {
        let path = project.join("skills-lock.json");
        read_input(OwnershipReadIssueKind::SkillsShLock, path.clone(), || {
            read_project_ledger(&path)
        })
    });
    let skills_sh_conflicts = project_ledger_conflicts(&agents_dir, &skills_sh, &project_skills_sh);
    ScopedOwnershipInputs {
        agents_dir,
        canonical_skills_dir,
        skills_root,
        scope,
        project_path,
        skills_sh,
        project_skills_sh,
        skills_sh_conflicts,
        dotagents,
    }
}

/// Read ownership data only through the caller's declared directory handles.
/// Callers must bind every home, project, and allowed backing root before use.
#[cfg(unix)]
pub(crate) struct PreparedOwnershipRead {
    scopes: Vec<PreparedOwnershipScope>,
    registry: PreparedOwnershipInput,
}

#[cfg(unix)]
struct PreparedOwnershipScope {
    agents_dir: PathBuf,
    scope: InstallScope,
    project_path: Option<PathBuf>,
    skills_root: OwnershipInput<(PathBuf, cap_std::fs::Metadata)>,
    skills_sh: PreparedOwnershipInput,
    project_skills_sh: Option<PreparedOwnershipInput>,
    dotagents: PreparedDotagentsPair,
    legacy_dotagents: Option<PreparedDotagentsPair>,
}

#[cfg(unix)]
struct PreparedDotagentsPair {
    root: PathBuf,
    lock: PreparedOwnershipInput,
    manifest: PreparedOwnershipInput,
}

#[cfg(unix)]
impl PreparedOwnershipRead {
    pub(crate) fn enumerate(
        read_scope: &SkillReadScope,
        home: &Path,
        projects: &[PathBuf],
    ) -> Self {
        let mut scopes = vec![PreparedOwnershipScope::enumerate(
            read_scope,
            home.join(".agents"),
            InstallScope::Global,
            None,
        )];
        scopes.extend(projects.iter().map(|project| {
            PreparedOwnershipScope::enumerate(
                read_scope,
                project.join(".agents"),
                InstallScope::Project,
                Some(project.clone()),
            )
        }));
        Self {
            scopes,
            registry: PreparedOwnershipInput::enumerate(
                read_scope,
                OwnershipReadIssueKind::LifecycleRegistry,
                crate::skill_fork_registry::fork_registry_path(home),
            ),
        }
    }

    pub(crate) fn materialize(
        &self,
        read_scope: &SkillReadScope,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
    ) -> OwnershipReadReport {
        OwnershipReadReport {
            scopes: self
                .scopes
                .iter()
                .map(|scope| scope.materialize(read_scope, guard))
                .collect(),
            registry: self.registry.materialize(read_scope, guard, |content| {
                crate::skill_fork_registry::parse_fork_registry(content, &self.registry.path)
            }),
        }
    }

    fn inputs(&self) -> impl Iterator<Item = &PreparedOwnershipInput> {
        std::iter::once(&self.registry).chain(self.scopes.iter().flat_map(|scope| {
            [
                Some(&scope.skills_sh),
                scope.project_skills_sh.as_ref(),
                Some(&scope.dotagents.lock),
                Some(&scope.dotagents.manifest),
                scope.legacy_dotagents.as_ref().map(|pair| &pair.lock),
                scope.legacy_dotagents.as_ref().map(|pair| &pair.manifest),
            ]
            .into_iter()
            .flatten()
        }))
    }

    pub(crate) fn registry_was_absent(&self, path: &Path) -> Result<bool, String> {
        if self.registry.path != path {
            return Err("Registry path is not the retained ownership source".into());
        }
        match &self.registry.observation {
            OwnershipInput::Absent => Ok(true),
            OwnershipInput::Loaded(_) => Ok(false),
            OwnershipInput::Failed(_) => Err("Registry ownership observation failed".into()),
        }
    }

    pub(crate) fn regular_files(&self) -> Vec<PathBuf> {
        self.inputs()
            .filter_map(|input| match &input.observation {
                OwnershipInput::Loaded(observation) => Some(observation.requested.clone()),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(crate) fn revalidate(&self, read_scope: &SkillReadScope) -> Result<(), OwnershipReadIssue> {
        self.revalidate_remaining(read_scope, |_| false)
    }

    pub(crate) fn revalidate_remaining(
        &self,
        read_scope: &SkillReadScope,
        published: impl Fn(&Path) -> bool,
    ) -> Result<(), OwnershipReadIssue> {
        use cap_std::fs::MetadataExt;
        for input in self.inputs() {
            if (input.path == self.registry.path
                || matches!(
                    input.kind,
                    OwnershipReadIssueKind::DotagentsLock
                        | OwnershipReadIssueKind::DotagentsManifest
                ))
                && (matches!(input.observation, OwnershipInput::Loaded(_))
                    || (input.path == self.registry.path
                        && matches!(input.observation, OwnershipInput::Absent)))
                && published(&input.path)
            {
                continue;
            }
            let unchanged = match &input.observation {
                OwnershipInput::Loaded(before) => {
                    matches!(read_scope.observe_content_regular(&input.path), Ok(after) if before == &after)
                }
                OwnershipInput::Absent => matches!(
                    read_scope.observe_content_regular(&input.path),
                    Err(ScopedReadError::Missing { .. })
                ),
                OwnershipInput::Failed(_) => true,
            };
            if !unchanged {
                return Err(OwnershipReadIssue {
                    kind: input.kind,
                    path: input.path.to_string_lossy().into_owned(),
                    message: "Ownership input changed after preparation".into(),
                });
            }
        }
        for scope in &self.scopes {
            let path = scope.agents_dir.join("skills");
            let unchanged = match (&scope.skills_root, observe_skills_root(read_scope, &path)) {
                (
                    OwnershipInput::Loaded((before_path, before)),
                    OwnershipInput::Loaded((after_path, after)),
                ) => {
                    before_path == &after_path
                        && before.dev() == after.dev()
                        && before.ino() == after.ino()
                        && before.ctime() == after.ctime()
                        && before.ctime_nsec() == after.ctime_nsec()
                }
                (OwnershipInput::Absent, OwnershipInput::Absent)
                | (OwnershipInput::Failed(_), _) => true,
                _ => false,
            };
            if !unchanged {
                return Err(OwnershipReadIssue {
                    kind: OwnershipReadIssueKind::SkillsRoot,
                    path: path.to_string_lossy().into_owned(),
                    message: "Skills root changed after ownership preparation".into(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
impl PreparedOwnershipScope {
    fn enumerate(
        read_scope: &SkillReadScope,
        agents_dir: PathBuf,
        scope: InstallScope,
        project_path: Option<PathBuf>,
    ) -> Self {
        let (current, legacy) = dotagents_roots(&agents_dir, project_path.as_deref());
        Self {
            skills_root: observe_skills_root(read_scope, &agents_dir.join("skills")),
            skills_sh: PreparedOwnershipInput::enumerate(
                read_scope,
                OwnershipReadIssueKind::SkillsShLock,
                agents_dir.join(".skill-lock.json"),
            ),
            project_skills_sh: project_path.as_ref().map(|project| {
                PreparedOwnershipInput::enumerate(
                    read_scope,
                    OwnershipReadIssueKind::SkillsShLock,
                    project.join("skills-lock.json"),
                )
            }),
            dotagents: PreparedDotagentsPair::enumerate(read_scope, current),
            legacy_dotagents: legacy.map(|root| PreparedDotagentsPair::enumerate(read_scope, root)),
            agents_dir,
            scope,
            project_path,
        }
    }

    fn materialize(
        &self,
        read_scope: &SkillReadScope,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
    ) -> ScopedOwnershipInputs {
        use cap_std::fs::MetadataExt;
        let root_path = self.agents_dir.join("skills");
        let skills_root = match (
            &self.skills_root,
            observe_skills_root(read_scope, &root_path),
        ) {
            (OwnershipInput::Failed(issue), _) => OwnershipInput::Failed(issue.clone()),
            (OwnershipInput::Absent, OwnershipInput::Absent) => OwnershipInput::Absent,
            (OwnershipInput::Loaded((path, before)), OwnershipInput::Loaded((current, after)))
                if path == &current
                    && before.dev() == after.dev()
                    && before.ino() == after.ino()
                    && before.ctime() == after.ctime()
                    && before.ctime_nsec() == after.ctime_nsec() =>
            {
                OwnershipInput::Loaded(path.clone())
            }
            (_, OwnershipInput::Failed(issue)) => OwnershipInput::Failed(issue),
            _ => OwnershipInput::Failed(OwnershipReadIssue {
                kind: OwnershipReadIssueKind::SkillsRoot,
                path: root_path.to_string_lossy().into_owned(),
                message: "Skills root changed after ownership enumeration".into(),
            }),
        };
        let canonical_skills_dir = match &skills_root {
            OwnershipInput::Loaded(path) => Some(path.clone()),
            _ => None,
        };
        let skills_sh = self
            .skills_sh
            .materialize(read_scope, guard, lock_file::parse_lock_file);
        let project_skills_sh = self.project_skills_sh.as_ref().map(|input| {
            input.materialize(read_scope, guard, |content| {
                parse_project_lock(content.as_bytes()).map_err(|e| e.to_string())
            })
        });
        let skills_sh_conflicts =
            project_ledger_conflicts(&self.agents_dir, &skills_sh, &project_skills_sh);
        ScopedOwnershipInputs {
            agents_dir: self.agents_dir.clone(),
            scope: self.scope.clone(),
            project_path: self.project_path.clone(),
            canonical_skills_dir,
            skills_root,
            skills_sh,
            project_skills_sh,
            skills_sh_conflicts,
            dotagents: DotagentsInputs::new(
                self.dotagents.materialize(read_scope, guard),
                self.legacy_dotagents
                    .as_ref()
                    .map(|pair| pair.materialize(read_scope, guard)),
            ),
        }
    }
}

#[cfg(unix)]
impl PreparedDotagentsPair {
    fn enumerate(read_scope: &SkillReadScope, root: PathBuf) -> Self {
        Self {
            lock: PreparedOwnershipInput::enumerate(
                read_scope,
                OwnershipReadIssueKind::DotagentsLock,
                root.join("agents.lock"),
            ),
            manifest: PreparedOwnershipInput::enumerate(
                read_scope,
                OwnershipReadIssueKind::DotagentsManifest,
                root.join("agents.toml"),
            ),
            root,
        }
    }

    fn materialize(
        &self,
        read_scope: &SkillReadScope,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
    ) -> DotagentsInputPair {
        DotagentsInputPair {
            root: self.root.clone(),
            lock: self.lock.materialize(read_scope, guard, |content| {
                dotagents_ledger::parse_agents_lock(content, &self.lock.path)
            }),
            manifest: self.manifest.materialize(read_scope, guard, |content| {
                dotagents_ledger::parse_agents_manifest(content, &self.manifest.path)
            }),
        }
    }
}

#[cfg(unix)]
pub fn load_ownership_inputs_scoped(
    read_scope: &SkillReadScope,
    home: &Path,
    project_paths: &[PathBuf],
) -> OwnershipReadReport {
    PreparedOwnershipRead::enumerate(read_scope, home, project_paths).materialize(read_scope, None)
}

fn dotagents_roots(agents_dir: &Path, project_path: Option<&Path>) -> (PathBuf, Option<PathBuf>) {
    match project_path {
        Some(project_path) => (project_path.to_path_buf(), Some(agents_dir.to_path_buf())),
        None => (agents_dir.to_path_buf(), None),
    }
}

fn read_dotagents_inputs(agents_dir: &Path, project_path: Option<&Path>) -> DotagentsInputs {
    let (current_root, legacy_root) = dotagents_roots(agents_dir, project_path);
    let current = read_dotagents_pair(current_root);
    let legacy = legacy_root.map(read_dotagents_pair);
    DotagentsInputs::new(current, legacy)
}

fn read_dotagents_pair(root: PathBuf) -> DotagentsInputPair {
    let lock_path = root.join("agents.lock");
    let manifest_path = root.join("agents.toml");
    DotagentsInputPair {
        root: root.clone(),
        lock: read_input(OwnershipReadIssueKind::DotagentsLock, lock_path, || {
            dotagents_ledger::read_agents_lock(&root)
        }),
        manifest: read_input(
            OwnershipReadIssueKind::DotagentsManifest,
            manifest_path,
            || dotagents_ledger::read_agents_manifest(&root),
        ),
    }
}

#[cfg(unix)]
fn observe_skills_root(
    read_scope: &SkillReadScope,
    path: &Path,
) -> OwnershipInput<(PathBuf, cap_std::fs::Metadata)> {
    match read_scope.resolved_path_metadata(path) {
        Ok((resolved, metadata)) if metadata.is_dir() => {
            OwnershipInput::Loaded((resolved, metadata))
        }
        Ok(_) => OwnershipInput::Failed(OwnershipReadIssue {
            kind: OwnershipReadIssueKind::SkillsRoot,
            path: path.to_string_lossy().into_owned(),
            message: "Skills root is not a directory".into(),
        }),
        Err(ScopedReadError::Missing { .. }) => OwnershipInput::Absent,
        Err(error) => OwnershipInput::Failed(scoped_issue(
            OwnershipReadIssueKind::SkillsRoot,
            path.to_path_buf(),
            error,
        )),
    }
}

#[cfg(unix)]
struct PreparedOwnershipInput {
    kind: OwnershipReadIssueKind,
    path: PathBuf,
    observation: OwnershipInput<crate::skill_scope::ScopedContentObservation>,
}

#[cfg(unix)]
impl PreparedOwnershipInput {
    fn enumerate(scope: &SkillReadScope, kind: OwnershipReadIssueKind, path: PathBuf) -> Self {
        let observation = match scope.observe_content_regular(&path) {
            Ok(observation) => OwnershipInput::Loaded(observation),
            Err(ScopedReadError::Missing { .. }) => OwnershipInput::Absent,
            Err(error) => OwnershipInput::Failed(scoped_issue(kind, path.clone(), error)),
        };
        Self {
            kind,
            path,
            observation,
        }
    }

    fn failure<T>(&self, message: impl Into<String>) -> OwnershipInput<T> {
        OwnershipInput::Failed(OwnershipReadIssue {
            kind: self.kind,
            path: self.path.to_string_lossy().into_owned(),
            message: message.into(),
        })
    }

    fn materialize<T>(
        &self,
        scope: &SkillReadScope,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
        parse: impl FnOnce(&str) -> Result<T, String>,
    ) -> OwnershipInput<T> {
        use crate::skill_scope::ScopedContentFoldError;
        let observation = match &self.observation {
            OwnershipInput::Failed(issue) => return OwnershipInput::Failed(issue.clone()),
            OwnershipInput::Absent => {
                return match scope.observe_content_regular(&self.path) {
                    Err(ScopedReadError::Missing { .. }) => OwnershipInput::Absent,
                    Ok(_) => self.failure("Ownership input appeared after enumeration"),
                    Err(error) => {
                        OwnershipInput::Failed(scoped_issue(self.kind, self.path.clone(), error))
                    }
                }
            }
            OwnershipInput::Loaded(observation) => observation,
        };
        let bytes = match guard {
            Some(guard) => guard.read_content(scope, observation, SCOPED_OWNERSHIP_FILE_BYTE_LIMIT),
            None => scope
                .read_content_observed(observation, SCOPED_OWNERSHIP_FILE_BYTE_LIMIT)
                .map_err(ScopedContentFoldError::Read),
        };
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(ScopedContentFoldError::Read(error)) => {
                return OwnershipInput::Failed(scoped_issue(self.kind, self.path.clone(), error))
            }
            Err(ScopedContentFoldError::Changed) => {
                return self.failure("Ownership input changed after enumeration")
            }
            Err(ScopedContentFoldError::Cancelled(message)) => return self.failure(message),
        };
        let content = match std::str::from_utf8(&bytes) {
            Ok(content) => content,
            Err(error) => return self.failure(format!("Failed to parse ownership input: {error}")),
        };
        match parse(content) {
            Ok(value) => OwnershipInput::Loaded(value),
            Err(message) => self.failure(message),
        }
    }
}

#[cfg(unix)]
fn scoped_issue(
    kind: OwnershipReadIssueKind,
    path: PathBuf,
    error: ScopedReadError,
) -> OwnershipReadIssue {
    OwnershipReadIssue {
        kind,
        path: path.to_string_lossy().into_owned(),
        message: format!("Failed to read ownership input: {error}"),
    }
}

fn read_input<T>(
    kind: OwnershipReadIssueKind,
    path: PathBuf,
    read: impl FnOnce() -> Result<Option<T>, String>,
) -> OwnershipInput<T> {
    match read() {
        Ok(Some(value)) => OwnershipInput::Loaded(value),
        Ok(None) => OwnershipInput::Absent,
        Err(message) => OwnershipInput::Failed(OwnershipReadIssue {
            kind,
            path: path.to_string_lossy().into_owned(),
            message,
        }),
    }
}

impl DotagentsInputs {
    fn new(current: DotagentsInputPair, legacy: Option<DotagentsInputPair>) -> Self {
        let conflict = legacy
            .as_ref()
            .filter(|legacy| current.failure().is_none() && legacy.failure().is_none())
            .filter(|legacy| current.is_present() && legacy.is_present())
            .filter(|legacy| !current.agrees_with(legacy))
            .map(|legacy| current.conflict_with(legacy));
        Self {
            current,
            legacy,
            conflict,
        }
    }

    fn append_failures(&self, failures: &mut Vec<OwnershipReadIssue>) {
        self.current.append_failures(failures);
        if let Some(legacy) = &self.legacy {
            legacy.append_failures(failures);
        }
        if let Some(conflict) = &self.conflict {
            failures.push(conflict.clone());
        }
    }

    fn effective_pair(&self) -> Result<&DotagentsInputPair, OwnershipReadIssue> {
        if let Some(issue) = self.current.failure() {
            return Err(issue.clone());
        }
        if let Some(legacy) = &self.legacy {
            if let Some(issue) = legacy.failure() {
                return Err(issue.clone());
            }
        }
        if let Some(issue) = &self.conflict {
            return Err(issue.clone());
        }
        match &self.legacy {
            Some(legacy) if self.current.is_absent() => Ok(legacy),
            _ => Ok(&self.current),
        }
    }
}

impl DotagentsInputPair {
    fn is_absent(&self) -> bool {
        matches!(self.lock, OwnershipInput::Absent)
            && matches!(self.manifest, OwnershipInput::Absent)
    }

    fn is_present(&self) -> bool {
        !self.is_absent()
    }

    fn failure(&self) -> Option<&OwnershipReadIssue> {
        match (&self.lock, &self.manifest) {
            (OwnershipInput::Failed(issue), _) | (_, OwnershipInput::Failed(issue)) => Some(issue),
            _ => None,
        }
    }

    fn append_failures(&self, failures: &mut Vec<OwnershipReadIssue>) {
        append_failure(failures, &self.lock);
        append_failure(failures, &self.manifest);
    }

    fn agrees_with(&self, other: &Self) -> bool {
        let locks_match = match (&self.lock, &other.lock) {
            (OwnershipInput::Loaded(current), OwnershipInput::Loaded(legacy)) => {
                current.skills == legacy.skills
            }
            (OwnershipInput::Loaded(current), OwnershipInput::Absent) => current.skills.is_empty(),
            (OwnershipInput::Absent, OwnershipInput::Loaded(legacy)) => legacy.skills.is_empty(),
            (OwnershipInput::Absent, OwnershipInput::Absent) => true,
            (OwnershipInput::Failed(_), _) | (_, OwnershipInput::Failed(_)) => false,
        };
        let manifests_match = match (&self.manifest, &other.manifest) {
            (OwnershipInput::Loaded(current), OwnershipInput::Loaded(legacy)) => {
                dotagents_ledger::manifest_refs(current) == dotagents_ledger::manifest_refs(legacy)
            }
            (OwnershipInput::Loaded(current), OwnershipInput::Absent) => {
                dotagents_ledger::manifest_refs(current).is_empty()
            }
            (OwnershipInput::Absent, OwnershipInput::Loaded(legacy)) => {
                dotagents_ledger::manifest_refs(legacy).is_empty()
            }
            (OwnershipInput::Absent, OwnershipInput::Absent) => true,
            (OwnershipInput::Failed(_), _) | (_, OwnershipInput::Failed(_)) => false,
        };
        locks_match && manifests_match
    }

    fn conflict_with(&self, legacy: &Self) -> OwnershipReadIssue {
        OwnershipReadIssue {
            kind: OwnershipReadIssueKind::DotagentsLock,
            path: self.root.join("agents.lock").to_string_lossy().into_owned(),
            message: format!(
                "Conflicting dotagents inputs at {} and {}",
                self.root.join("agents.lock").display(),
                legacy.root.join("agents.lock").display()
            ),
        }
    }
}

impl ScopedOwnershipInputs {
    pub(crate) fn is_failed(&self) -> bool {
        matches!(self.skills_root, OwnershipInput::Failed(_))
            || matches!(self.skills_sh, OwnershipInput::Failed(_))
            || matches!(self.project_skills_sh, Some(OwnershipInput::Failed(_)))
            || !self.skills_sh_conflicts.is_empty()
            || self.dotagents.effective_pair().is_err()
    }

    pub fn as_ledger(&self) -> Result<OwnershipLedgers, OwnershipReadIssue> {
        if let OwnershipInput::Failed(issue) = &self.skills_root {
            return Err(issue.clone());
        }
        if let OwnershipInput::Failed(issue) = &self.skills_sh {
            return Err(issue.clone());
        }
        if let Some(OwnershipInput::Failed(issue)) = &self.project_skills_sh {
            return Err(issue.clone());
        }
        if let Some(issue) = self.skills_sh_conflicts.first() {
            return Err(issue.clone());
        }
        let dotagents = self.dotagents.effective_pair()?;
        let lock = match &self.skills_sh {
            OwnershipInput::Loaded(lock) => lock.clone(),
            OwnershipInput::Absent | OwnershipInput::Failed(_) => lock_file::empty_lock_file(),
        };
        let dotagents_lock = match &dotagents.lock {
            OwnershipInput::Loaded(lock) => lock.clone(),
            OwnershipInput::Absent | OwnershipInput::Failed(_) => AgentsLock::default(),
        };
        let dotagents_manifest = match &dotagents.manifest {
            OwnershipInput::Loaded(manifest) => manifest.clone(),
            OwnershipInput::Absent | OwnershipInput::Failed(_) => AgentsManifest::default(),
        };
        Ok(OwnershipLedgers {
            agents_dir: self.agents_dir.clone(),
            scope: self.scope.clone(),
            project_path: self.project_path.clone(),
            lock,
            project_lock: match &self.project_skills_sh {
                Some(OwnershipInput::Loaded(lock)) => Some(lock.clone()),
                _ => None,
            },
            dotagents: dotagents_ledger::join_dotagents_ledger(dotagents_lock, dotagents_manifest),
        })
    }
}

/// Classify ownership for one candidate against the matching ledger only.
pub fn classify_lifecycle_owner(
    candidate: &SkillCandidate,
    report: &OwnershipReadReport,
    destination: SkillDestination,
    deployment_id: &str,
    copy_records: &std::collections::BTreeMap<String, CopyDeploymentRecord>,
) -> (LifecycleOwnerKind, Option<String>, SourceKind) {
    let plugin_unknown = matches!(candidate.plugin, PluginEvidence::Unknown);
    match candidate.plugin {
        PluginEvidence::Confirmed(_) => {
            return (LifecycleOwnerKind::Plugin, None, SourceKind::Plugin);
        }
        PluginEvidence::Unknown | PluginEvidence::Absent => {}
    }

    let selection = ownership_inputs_for_candidate(candidate, &report.scopes);
    let matching_input = match &selection {
        OwnershipScopeMatch::Unique(input) => Some(*input),
        OwnershipScopeMatch::Absent | OwnershipScopeMatch::Ambiguous => None,
    };
    if matches!(report.registry, OwnershipInput::Failed(_))
        || matching_input.is_some_and(ScopedOwnershipInputs::is_failed)
        || report.scopes.iter().any(|scope| {
            scope.project_path == candidate.project_path
                && matches!(
                    (&scope.scope, candidate.scope.as_str()),
                    (InstallScope::Global, "global") | (InstallScope::Project, "project")
                )
                && scope.is_failed()
        })
        || has_unread_ownership_input_for_actual_target(candidate, &report.scopes)
    {
        return (
            LifecycleOwnerKind::Unknown,
            None,
            non_owned_lifecycle_owner(candidate).2,
        );
    }

    if matches!(selection, OwnershipScopeMatch::Ambiguous) {
        return (LifecycleOwnerKind::Ambiguous, None, SourceKind::Unknown);
    }

    if copy_record_matches_candidate(copy_records, deployment_id, candidate, destination) {
        return (LifecycleOwnerKind::Copy, None, SourceKind::Manual);
    }

    if plugin_unknown {
        return (LifecycleOwnerKind::Unknown, None, SourceKind::Unknown);
    }

    if copy_records.contains_key(deployment_id) {
        return (LifecycleOwnerKind::Unknown, None, SourceKind::Unknown);
    }

    if destination == SkillDestination::PerHarness {
        return non_owned_lifecycle_owner(candidate);
    }

    let Some(input) = matching_input else {
        return non_owned_lifecycle_owner(candidate);
    };

    let owner_id = owner_id_for_scope(input, &candidate.name);
    let dotagents = input
        .dotagents
        .effective_pair()
        .expect("failed inputs return above");
    let dotagents_entry = matches!(
        &dotagents.lock,
        OwnershipInput::Loaded(lock) if lock.skills.contains_key(&candidate.name)
    );
    let manifest_entry = matches!(
        &dotagents.manifest,
        OwnershipInput::Loaded(manifest) if manifest.skills.iter().any(|skill| skill.name == candidate.name)
    );
    let skills_sh_entry = matches!(
        &input.skills_sh,
        OwnershipInput::Loaded(lock) if lock.skills.contains_key(&candidate.name)
    ) || matches!(
        &input.project_skills_sh,
        Some(OwnershipInput::Loaded(lock)) if lock.skills.contains_key(&candidate.name)
    );
    if dotagents_entry && skills_sh_entry {
        return (LifecycleOwnerKind::Ambiguous, None, SourceKind::Dotagents);
    }

    if dotagents_entry {
        if !manifest_entry {
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
    if dotagents.is_present() {
        return (LifecycleOwnerKind::Ambiguous, None, SourceKind::Dotagents);
    }

    if candidate.is_symlink {
        if let Some(target) = &candidate.symlink_target {
            if path_is_under_universal_skills(target) {
                return (LifecycleOwnerKind::Ambiguous, None, SourceKind::Dotagents);
            }
        }
    }

    non_owned_lifecycle_owner(candidate)
}

enum OwnershipScopeMatch<'a> {
    Absent,
    Unique(&'a ScopedOwnershipInputs),
    Ambiguous,
}

fn enclosing_agents_skills_dir(path: &Path) -> Option<&Path> {
    path.ancestors().find(|ancestor| {
        ancestor.file_name().is_some_and(|name| name == "skills")
            && agents_root_for_skills_dir(ancestor).is_some()
    })
}

fn ownership_inputs_for_candidate<'a>(
    candidate: &SkillCandidate,
    scopes: &'a [ScopedOwnershipInputs],
) -> OwnershipScopeMatch<'a> {
    if let Some(input) = candidate
        .path
        .parent()
        .and_then(enclosing_agents_skills_dir)
        .and_then(agents_root_for_skills_dir)
        .and_then(|agents_dir| scopes.iter().find(|scope| scope.agents_dir == agents_dir))
    {
        return OwnershipScopeMatch::Unique(input);
    }
    let mut matches = scopes.iter().filter(|scope| {
        [
            candidate.resolved_path.as_deref(),
            candidate.symlink_target.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|target| {
            target
                .file_name()
                .is_some_and(|name| name == candidate.name.as_str())
        })
        .filter_map(Path::parent)
        .any(|target_skills| {
            scope.canonical_skills_dir.as_deref() == Some(target_skills)
                || agents_root_for_skills_dir(target_skills)
                    .is_some_and(|agents| agents == scope.agents_dir)
        })
    });
    match (matches.next(), matches.next()) {
        (None, _) => OwnershipScopeMatch::Absent,
        (Some(input), None) => OwnershipScopeMatch::Unique(input),
        (Some(_), Some(_)) => OwnershipScopeMatch::Ambiguous,
    }
}

/// Finds the ownership input that backs the actual Universal directory when
/// it is known. The lexical path still decides normal ledger ownership. This
/// lookup exists only to preserve the conservative `Unknown` state when a
/// symlink or whole-root alias resolves into an unread or failed manager scope.
fn has_unread_ownership_input_for_actual_target(
    candidate: &SkillCandidate,
    scopes: &[ScopedOwnershipInputs],
) -> bool {
    [
        candidate.resolved_path.as_deref(),
        candidate.symlink_target.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter_map(|target| target.parent())
    .any(|target_skills_dir| {
        let lexical_target = lexical_failure_target(target_skills_dir);
        let manager_skills = enclosing_agents_skills_dir(&lexical_target);
        let mut matching = scopes
            .iter()
            .filter(|scope| {
                if let Some(manager_skills) = manager_skills {
                    scope.canonical_skills_dir.as_deref() == Some(manager_skills)
                        || manager_skills.parent() == Some(scope.agents_dir.as_path())
                } else {
                    scope
                        .canonical_skills_dir
                        .as_deref()
                        .is_some_and(|root| target_skills_dir.starts_with(root))
                }
            })
            .peekable();
        if matching.peek().is_none() {
            return manager_skills.is_some();
        }
        matching.any(ScopedOwnershipInputs::is_failed)
    })
}

// Broken relative links retain `..` components. This comparison can deny
// ownership but cannot establish physical identity or authorize a mutation.
fn lexical_failure_target(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn non_owned_lifecycle_owner(
    candidate: &SkillCandidate,
) -> (LifecycleOwnerKind, Option<String>, SourceKind) {
    if candidate.git_repo == GitRepoEvidence::Present {
        (LifecycleOwnerKind::InRepo, None, SourceKind::InRepo)
    } else if candidate.git_repo == GitRepoEvidence::Unknown {
        (LifecycleOwnerKind::Unknown, None, SourceKind::Unknown)
    } else if candidate.git_repo == GitRepoEvidence::Truncated {
        // Plugin and manager ownership were resolved before this fallback.
        // Unobserved Git ancestors affect provenance, not local document edits.
        (LifecycleOwnerKind::Manual, None, SourceKind::Unknown)
    } else {
        (LifecycleOwnerKind::Manual, None, SourceKind::Manual)
    }
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
        && crate::skill_deployment::parse_deployment_id(deployment_id)
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

fn normalized_skill_path(path: Option<&str>) -> Option<String> {
    path.filter(|value| !value.is_empty())
        .map(|value| value.strip_suffix("/SKILL.md").unwrap_or(value).to_string())
}

/// Return the exact source evidence used to check one ledger owner.
pub fn update_source_for_owner(
    ledger: &OwnershipLedgers,
    name: &str,
    owner_kind: LifecycleOwnerKind,
) -> Option<OwnerUpdateSource> {
    let owner_id = owner_id_for(ledger, name);
    match owner_kind {
        LifecycleOwnerKind::Dotagents => {
            let skill = ledger.dotagents.iter().find(|skill| skill.name == name)?;
            Some(OwnerUpdateSource {
                owner_id,
                repo: skill.github_repo.clone()?,
                path: (!skill.path.is_empty()).then(|| skill.path.clone()),
                source_ref: skill.declared_ref.clone(),
                baseline_identity: Some(format!(
                    "dotagents-installed-commit:{}",
                    skill.installed_commit.as_deref().unwrap_or("unknown")
                )),
            })
        }
        LifecycleOwnerKind::SkillsSh => {
            if let Some(entry) = ledger
                .project_lock
                .as_ref()
                .and_then(|lock| lock.skills.get(name))
            {
                if entry.source_type != "github" {
                    return None;
                }
                return Some(OwnerUpdateSource {
                    owner_id,
                    repo: dotagents_ledger::github_repo_from_source(&entry.source)?,
                    path: normalized_skill_path(entry.skill_path.as_deref()),
                    source_ref: entry.source_ref.clone(),
                    baseline_identity: Some(format!(
                        "project-computed-hash:{}",
                        entry.computed_hash
                    )),
                });
            }
            let entry = ledger.lock.skills.get(name)?;
            if entry.source_type != "github" {
                return None;
            }
            let updated_at = if entry.updated_at.is_empty() {
                &entry.installed_at
            } else {
                &entry.updated_at
            };
            Some(OwnerUpdateSource {
                owner_id,
                repo: dotagents_ledger::github_repo_from_source(&entry.source)?,
                path: normalized_skill_path(entry.skill_path.as_deref()),
                source_ref: None,
                baseline_identity: Some(format!(
                    "global-updated-at:{updated_at}:folder-hash:{}",
                    entry.skill_folder_hash
                )),
            })
        }
        LifecycleOwnerKind::Copy
        | LifecycleOwnerKind::Fork
        | LifecycleOwnerKind::Plugin
        | LifecycleOwnerKind::InRepo
        | LifecycleOwnerKind::Manual
        | LifecycleOwnerKind::WildcardDotagents
        | LifecycleOwnerKind::Ambiguous
        | LifecycleOwnerKind::Unknown => None,
    }
}

#[derive(Serialize)]
enum ManagedSourceRevision<'a> {
    GlobalSkillsSh(&'a crate::skill_lock_file::InstalledSkillEntry),
    ProjectSkillsSh(&'a crate::skill_project_lock::ProjectSkillEntry),
    Dotagents(&'a DotagentsSkill),
}

fn source_revision_for_owner(
    ledger: &OwnershipLedgers,
    name: &str,
    kind: LifecycleOwnerKind,
) -> Option<String> {
    use sha2::{Digest, Sha256};
    let source = match kind {
        LifecycleOwnerKind::SkillsSh => {
            if let Some(entry) = ledger
                .project_lock
                .as_ref()
                .and_then(|lock| lock.skills.get(name))
            {
                ManagedSourceRevision::ProjectSkillsSh(entry)
            } else {
                ManagedSourceRevision::GlobalSkillsSh(ledger.lock.skills.get(name)?)
            }
        }
        LifecycleOwnerKind::Dotagents => ManagedSourceRevision::Dotagents(
            ledger.dotagents.iter().find(|skill| skill.name == name)?,
        ),
        _ => return None,
    };
    let bytes =
        serde_json::to_vec(&("managed-source-v1", owner_id_for(ledger, name), source)).ok()?;
    let hex: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Some(format!("sha256:{hex}"))
}

pub(crate) fn source_evidence_for_candidate(
    candidate: &SkillCandidate,
    report: &OwnershipReadReport,
    owner_kind: LifecycleOwnerKind,
) -> (Option<OwnerUpdateSource>, Option<String>) {
    if !matches!(
        owner_kind,
        LifecycleOwnerKind::SkillsSh | LifecycleOwnerKind::Dotagents
    ) {
        return (None, None);
    }
    let OwnershipScopeMatch::Unique(input) =
        ownership_inputs_for_candidate(candidate, &report.scopes)
    else {
        return (None, None);
    };
    let Ok(ledger) = input.as_ledger() else {
        return (None, None);
    };
    (
        update_source_for_owner(&ledger, &candidate.name, owner_kind),
        source_revision_for_owner(&ledger, &candidate.name, owner_kind),
    )
}

/// Resolve source evidence from the scope that classified a deployment.
pub fn update_source_for_candidate(
    candidate: &SkillCandidate,
    report: &OwnershipReadReport,
    owner_kind: LifecycleOwnerKind,
) -> Option<OwnerUpdateSource> {
    source_evidence_for_candidate(candidate, report, owner_kind).0
}

fn owner_id_for_scope(scope: &ScopedOwnershipInputs, name: &str) -> String {
    match (&scope.scope, &scope.project_path) {
        (InstallScope::Global, _) => format!("owner:v1/global/{name}"),
        (InstallScope::Project, Some(path)) => format!(
            "owner:v1/project/{}/{}",
            encode_id_path(&path.to_string_lossy()),
            name
        ),
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
    #[test]
    fn managed_source_revisions_cover_non_github_entries_and_selected_record_drift() {
        let entry: crate::skill_lock_file::InstalledSkillEntry =
            serde_json::from_value(serde_json::json!({
                "source": "/local/skill", "sourceType": "local", "sourceUrl": "file:///local/skill",
                "skillFolderHash": "hash", "installedAt": "before", "updatedAt": "before"
            }))
            .unwrap();
        let mut ledger = OwnershipLedgers {
            agents_dir: PathBuf::from("/fixture/.agents"),
            scope: InstallScope::Global,
            project_path: None,
            lock: crate::skill_lock_file::empty_lock_file(),
            project_lock: None,
            dotagents: vec![],
        };
        ledger.lock.skills.insert("sample".into(), entry.clone());
        assert!(update_source_for_owner(&ledger, "sample", LifecycleOwnerKind::SkillsSh).is_none());
        let original =
            source_revision_for_owner(&ledger, "sample", LifecycleOwnerKind::SkillsSh).unwrap();
        ledger.lock.skills.insert("unrelated".into(), entry);
        assert_eq!(
            source_revision_for_owner(&ledger, "sample", LifecycleOwnerKind::SkillsSh).unwrap(),
            original
        );
        ledger.lock.skills.get_mut("sample").unwrap().source_url =
            "file:///replacement/skill".into();
        assert_ne!(
            source_revision_for_owner(&ledger, "sample", LifecycleOwnerKind::SkillsSh).unwrap(),
            original
        );
        ledger.project_lock = Some(crate::skill_project_lock::ProjectSkillLock { skills: std::collections::BTreeMap::from([("sample".into(), serde_json::from_value(serde_json::json!({
            "source": "https://gitlab.test/team/skills", "sourceType": "git", "computedHash": "old"
        })).unwrap())]) });
        let original =
            source_revision_for_owner(&ledger, "sample", LifecycleOwnerKind::SkillsSh).unwrap();
        ledger
            .project_lock
            .as_mut()
            .unwrap()
            .skills
            .get_mut("sample")
            .unwrap()
            .computed_hash = "new".into();
        assert_ne!(
            source_revision_for_owner(&ledger, "sample", LifecycleOwnerKind::SkillsSh).unwrap(),
            original
        );
        ledger.dotagents.push(DotagentsSkill {
            name: "sample".into(),
            source: "git:https://gitlab.test/team/skills".into(),
            github_repo: None,
            path: "skill".into(),
            installed_commit: Some("old".into()),
            declared_ref: None,
            has_manifest_row: true,
        });
        assert!(
            update_source_for_owner(&ledger, "sample", LifecycleOwnerKind::Dotagents).is_none()
        );
        let original =
            source_revision_for_owner(&ledger, "sample", LifecycleOwnerKind::Dotagents).unwrap();
        ledger.dotagents[0].installed_commit = Some("new".into());
        assert_ne!(
            source_revision_for_owner(&ledger, "sample", LifecycleOwnerKind::Dotagents).unwrap(),
            original
        );
    }

    #[cfg(unix)]
    #[test]
    fn final_ownership_validation_retains_loaded_and_absent_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        for root in [&home, &project] {
            std::fs::create_dir_all(root.join(".agents/skills")).unwrap();
        }
        let scope = SkillReadScope::bind(&[home.clone(), project.clone()]).unwrap();
        let plan = PreparedOwnershipRead::enumerate(&scope, &home, std::slice::from_ref(&project));
        let paths = plan
            .inputs()
            .map(|input| input.path.clone())
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 10);
        for present in [false, true] {
            for path in &paths {
                if path.exists() {
                    std::fs::remove_file(path).unwrap();
                }
                if present {
                    std::fs::write(path, "before").unwrap();
                }
                let plan =
                    PreparedOwnershipRead::enumerate(&scope, &home, std::slice::from_ref(&project));
                let _report = plan.materialize(&scope, None);
                assert!(plan.revalidate(&scope).is_ok());
                std::fs::write(path, "changed after materialization").unwrap();
                let error = plan.revalidate(&scope).unwrap_err();
                assert_eq!(std::path::Path::new(&error.path), path);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn final_ownership_validation_rejects_skills_root_membership_changes() {
        for present in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            std::fs::create_dir_all(home.join(".agents")).unwrap();
            let skills = home.join(".agents/skills");
            if present {
                std::fs::create_dir(&skills).unwrap();
            }
            let scope = SkillReadScope::bind(std::slice::from_ref(&home)).unwrap();
            let plan = PreparedOwnershipRead::enumerate(&scope, &home, &[]);
            let _report = plan.materialize(&scope, None);
            assert!(plan.revalidate(&scope).is_ok());
            std::fs::create_dir_all(skills.join("new-skill")).unwrap();
            assert_eq!(
                plan.revalidate(&scope).unwrap_err().kind,
                OwnershipReadIssueKind::SkillsRoot
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn ownership_plan_collects_global_project_legacy_and_registry_files() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        for root in [
            home.join(".agents"),
            project.clone(),
            project.join(".agents"),
        ] {
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("agents.lock"), "").unwrap();
            std::fs::write(root.join("agents.toml"), "").unwrap();
        }
        for root in [home.join(".agents"), project.join(".agents")] {
            std::fs::create_dir(root.join("skills")).unwrap();
            std::fs::write(
                root.join(".skill-lock.json"),
                r#"{"version":3,"skills":{}}"#,
            )
            .unwrap();
        }
        std::fs::write(
            project.join("skills-lock.json"),
            r#"{"version":1,"skills":{}}"#,
        )
        .unwrap();
        let registry = crate::skill_fork_registry::fork_registry_path(&home);
        std::fs::write(
            &registry,
            serde_json::to_vec(&crate::skill_fork_registry::ForkRegistry::default()).unwrap(),
        )
        .unwrap();
        let scope = SkillReadScope::bind(&[home.clone(), project.clone()]).unwrap();
        let plan = PreparedOwnershipRead::enumerate(&scope, &home, std::slice::from_ref(&project));
        let files = plan.regular_files();
        assert_eq!(files.len(), 10);
        assert!(files.contains(&registry));
        assert!(files.contains(&project.join("skills-lock.json")));
        assert!(files.contains(&project.join(".agents/agents.lock")));
        let expected = plan.materialize(&scope, None);
        let guard = ownership_guard(&scope, temp.path(), &files);
        let actual = plan.materialize(&scope, Some(&guard));
        assert!(actual.failures().is_empty(), "{:?}", actual.failures());
        assert_eq!(actual.scopes.len(), 2);
        assert_eq!(actual.failures(), expected.failures());
        assert!(matches!(
            actual.scopes[1].project_skills_sh,
            Some(OwnershipInput::Loaded(_))
        ));
        assert!(matches!(actual.registry, OwnershipInput::Loaded(_)));
        guard.revalidate(&scope).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn ownership_plan_rejects_a_replaced_skills_root() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let skills = home.join(".agents/skills");
        std::fs::create_dir_all(&skills).unwrap();
        let scope = SkillReadScope::bind(std::slice::from_ref(&home)).unwrap();
        let plan = PreparedOwnershipRead::enumerate(&scope, &home, &[]);
        std::fs::rename(&skills, home.join("old-skills")).unwrap();
        std::fs::create_dir(&skills).unwrap();
        let result = plan.materialize(&scope, None);
        assert!(
            matches!(&result.scopes[0].skills_root, OwnershipInput::Failed(issue)
            if issue.kind == OwnershipReadIssueKind::SkillsRoot)
        );
        assert!(result.scopes[0].canonical_skills_dir.is_none());
    }

    #[cfg(unix)]
    fn ownership_guard(
        scope: &SkillReadScope,
        root: &Path,
        paths: &[PathBuf],
    ) -> crate::skill_coordination::CoordinatedReadGuard {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        CoordinationPlan::new_fixture(
            vec![DirectoryEffect::tree(root, CoordinationMode::Shared)],
            root,
            Some(std::time::Duration::from_secs(10)),
        )
        .unwrap()
        .acquire()
        .unwrap()
        .continue_with_files(scope, paths, CoordinationMode::Shared)
        .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn prepared_ownership_parses_guarded_regular_and_hard_linked_inputs() {
        for kind in [
            OwnershipReadIssueKind::SkillsShLock,
            OwnershipReadIssueKind::DotagentsLock,
            OwnershipReadIssueKind::DotagentsManifest,
            OwnershipReadIssueKind::LifecycleRegistry,
        ] {
            for hard_link in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let path = temp.path().join("input");
                std::fs::write(&path, "fixture input").unwrap();
                if hard_link {
                    std::fs::hard_link(&path, temp.path().join("alias")).unwrap();
                }
                let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
                let prepared = PreparedOwnershipInput::enumerate(&scope, kind, path.clone());
                let guard = ownership_guard(&scope, temp.path(), &[path]);
                let result =
                    prepared.materialize(&scope, Some(&guard), |content| Ok(content.to_string()));
                assert!(
                    matches!(result, OwnershipInput::Loaded(value) if value == "fixture input")
                );
                guard.revalidate(&scope).unwrap();
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn prepared_ownership_preserves_absence_and_rejects_later_appearance() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("input");
        let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
        let prepared = PreparedOwnershipInput::enumerate(
            &scope,
            OwnershipReadIssueKind::SkillsShLock,
            path.clone(),
        );
        let absent: OwnershipInput<()> =
            prepared.materialize(&scope, None, |_| panic!("absent input parsed"));
        assert!(matches!(absent, OwnershipInput::Absent));
        std::fs::write(&path, "new input").unwrap();
        let guard = ownership_guard(&scope, temp.path(), std::slice::from_ref(&path));
        let changed: OwnershipInput<()> =
            prepared.materialize(&scope, Some(&guard), |_| panic!("new input parsed"));
        assert!(matches!(changed, OwnershipInput::Failed(issue)
            if issue.kind == OwnershipReadIssueKind::SkillsShLock && issue.path == path.to_string_lossy()));
    }

    #[cfg(unix)]
    #[test]
    fn prepared_ownership_rejects_stale_unplanned_and_failed_inputs_before_parsing() {
        for change in ["replace", "remove", "unplanned", "failed"] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("input");
            if change == "failed" {
                std::fs::create_dir(&path).unwrap();
            } else {
                std::fs::write(&path, "original input").unwrap();
            }
            let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
            let prepared = PreparedOwnershipInput::enumerate(
                &scope,
                OwnershipReadIssueKind::LifecycleRegistry,
                path.clone(),
            );
            match change {
                "replace" => std::fs::write(&path, "replacement").unwrap(),
                "remove" => std::fs::remove_file(&path).unwrap(),
                "failed" => {
                    std::fs::remove_dir(&path).unwrap();
                    std::fs::write(&path, "now readable").unwrap();
                }
                _ => {}
            }
            let paths = if change == "unplanned" || change == "remove" {
                Vec::new()
            } else {
                vec![path.clone()]
            };
            let guard = ownership_guard(&scope, temp.path(), &paths);
            let result: OwnershipInput<()> =
                prepared.materialize(&scope, Some(&guard), |_| panic!("invalid input parsed"));
            assert!(matches!(result, OwnershipInput::Failed(issue)
                if issue.kind == OwnershipReadIssueKind::LifecycleRegistry && issue.path == path.to_string_lossy()));
        }
    }

    use super::*;

    fn discover(home: &Path) -> crate::skill_discovery::DiscoveryReport {
        let context = crate::skill_discovery::SkillDiscoveryReadContext::bind(
            home.to_path_buf(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        crate::skill_discovery::discover_skill_candidates(&context)
    }
    use std::collections::BTreeMap;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[cfg(unix)]
    fn project_reports(root: &Path, projects: &[PathBuf]) -> Vec<OwnershipReadReport> {
        let home = root.join("home");
        let scope = SkillReadScope::bind(&[root.to_path_buf()]).unwrap();
        vec![
            load_ownership_inputs(&home, projects),
            load_ownership_inputs_scoped(&scope, &home, projects),
        ]
    }

    fn write_project_ledger(project: &Path, source: &str) {
        fs::create_dir_all(project.join(".agents/skills/alpha")).unwrap();
        fs::write(project.join("skills-lock.json"), serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "skills": {"alpha": {"source":source,"sourceType":"github","computedHash":"content-hash"}}
        })).unwrap()).unwrap();
    }

    fn write_legacy_project_ledger(project: &Path, source: &str) {
        fs::write(
            project.join(".agents/.skill-lock.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": 3,
                "skills": {"alpha": {"source":source,"sourceType":"github","sourceUrl":"",
                    "skillFolderHash":"tree-hash","installedAt":"installed","updatedAt":"updated"}}
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn write_project_ledgers_with_entry_fields(
        project: &Path,
        current_source_type: &str,
        legacy_source_type: &str,
        current_source_url: Option<&str>,
        legacy_source_url: &str,
        current_skill_path: Option<&str>,
        legacy_skill_path: Option<&str>,
    ) {
        fs::create_dir_all(project.join(".agents/skills/alpha")).unwrap();
        fs::write(
            project.join("skills-lock.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "skills": {"alpha": {
                    "source": "owner/repo",
                    "sourceType": current_source_type,
                    "computedHash": "content-hash",
                    "sourceUrl": current_source_url,
                    "skillPath": current_skill_path,
                }}
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            project.join(".agents/.skill-lock.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": 3,
                "skills": {"alpha": {
                    "source": "owner/repo",
                    "sourceType": legacy_source_type,
                    "sourceUrl": legacy_source_url,
                    "skillPath": legacy_skill_path,
                    "skillFolderHash": "tree-hash",
                    "installedAt": "installed",
                    "updatedAt": "updated",
                }}
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn project_owner(report: &OwnershipReadReport, project: &Path) -> LifecycleOwnerKind {
        let skill = candidate(
            "alpha",
            "Shared",
            &project.join(".agents/skills/alpha").to_string_lossy(),
        );
        classify_lifecycle_owner(
            &skill,
            report,
            SkillDestination::Universal,
            "deployment",
            &BTreeMap::new(),
        )
        .0
    }

    #[cfg(unix)]
    #[test]
    fn project_current_ledger_classifies_each_project_without_global_fields() {
        let root = tempfile::tempdir().unwrap();
        let projects = vec![root.path().join("first"), root.path().join("second")];
        write_project_ledger(&projects[0], "owner/first");
        write_project_ledger(&projects[1], "owner/second");
        for report in project_reports(root.path(), &projects) {
            assert!(report.failures().is_empty());
            assert!(report.scopes[0].project_skills_sh.is_none());
            for (index, project) in projects.iter().enumerate() {
                assert_eq!(
                    project_owner(&report, project),
                    LifecycleOwnerKind::SkillsSh
                );
                let ledger = report.scopes[index + 1].as_ledger().unwrap();
                assert!(ledger.lock.skills.is_empty());
                let project_lock = ledger.project_lock.unwrap();
                assert_eq!(
                    project_lock.skills["alpha"].source,
                    format!("owner/{}", project.file_name().unwrap().to_str().unwrap())
                );
                assert_eq!(project_lock.skills["alpha"].computed_hash, "content-hash");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn project_current_and_legacy_conflicts_preserve_both_paths() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        write_project_ledger(&project, "owner/current");
        write_legacy_project_ledger(&project, "owner/legacy");
        for report in project_reports(root.path(), std::slice::from_ref(&project)) {
            assert_eq!(
                project_owner(&report, &project),
                LifecycleOwnerKind::Unknown
            );
            let failures = report.failures();
            assert_eq!(failures.len(), 1);
            assert_eq!(
                failures[0].path,
                project.join("skills-lock.json").to_string_lossy()
            );
            assert!(failures[0]
                .message
                .contains(project.join(".agents/.skill-lock.json").to_str().unwrap()));
            assert!(report.scopes[1].as_ledger().is_err());
        }
        write_legacy_project_ledger(&project, "owner/current");
        for report in project_reports(root.path(), std::slice::from_ref(&project)) {
            assert!(report.failures().is_empty());
            assert_eq!(
                project_owner(&report, &project),
                LifecycleOwnerKind::SkillsSh
            );
            let ledger = report.scopes[1].as_ledger().unwrap();
            assert_eq!(ledger.lock.skills["alpha"].skill_folder_hash, "tree-hash");
            assert_eq!(
                ledger.project_lock.unwrap().skills["alpha"].computed_hash,
                "content-hash"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn project_ledger_entry_field_conflicts_deny_ownership_in_both_read_modes() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        for (
            current_source_type,
            legacy_source_type,
            current_url,
            legacy_url,
            current_path,
            legacy_path,
        ) in [
            (
                "local",
                "github",
                Some("https://example.test/owner/repo"),
                "https://example.test/owner/repo",
                Some("skills/alpha"),
                Some("skills/alpha"),
            ),
            (
                "github",
                "github",
                Some("https://example.test/current"),
                "https://example.test/legacy",
                Some("skills/alpha"),
                Some("skills/alpha"),
            ),
            (
                "github",
                "github",
                Some("https://example.test/owner/repo"),
                "https://example.test/owner/repo",
                Some("skills/current"),
                Some("skills/legacy"),
            ),
        ] {
            write_project_ledgers_with_entry_fields(
                &project,
                current_source_type,
                legacy_source_type,
                current_url,
                legacy_url,
                current_path,
                legacy_path,
            );
            for report in project_reports(root.path(), std::slice::from_ref(&project)) {
                assert_eq!(
                    project_owner(&report, &project),
                    LifecycleOwnerKind::Unknown
                );
                assert_eq!(report.failures().len(), 1);
                assert!(report.scopes[1].as_ledger().is_err());
            }
        }

        write_project_ledgers_with_entry_fields(
            &project,
            "github",
            "github",
            None,
            "https://example.test/owner/repo",
            None,
            Some("skills/alpha"),
        );
        for report in project_reports(root.path(), std::slice::from_ref(&project)) {
            assert!(report.failures().is_empty());
            assert_eq!(
                project_owner(&report, &project),
                LifecycleOwnerKind::SkillsSh
            );
        }

        write_project_ledgers_with_entry_fields(
            &project,
            "github",
            "github",
            Some("https://example.test/owner/repo"),
            "",
            Some("skills/alpha"),
            Some("skills/alpha"),
        );
        for report in project_reports(root.path(), std::slice::from_ref(&project)) {
            assert!(report.failures().is_empty());
            assert_eq!(
                project_owner(&report, &project),
                LifecycleOwnerKind::SkillsSh
            );
        }

        write_project_ledgers_with_entry_fields(
            &project,
            "github",
            "github",
            None,
            "https://example.test/owner/repo",
            Some("skills/alpha"),
            None,
        );
        for report in project_reports(root.path(), std::slice::from_ref(&project)) {
            assert!(report.failures().is_empty());
            assert_eq!(
                project_owner(&report, &project),
                LifecycleOwnerKind::SkillsSh
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn ambient_project_ledger_fifo_does_not_block_ownership_reads() {
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let project = root.path().join("project");
        fs::create_dir_all(project.join(".agents/skills/alpha")).unwrap();
        let fifo = project.join("skills-lock.json");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());

        let (sender, receiver) = mpsc::channel();
        let reader_project = project.clone();
        let reader = thread::spawn(move || {
            sender
                .send(load_ownership_inputs(&home, &[reader_project]))
                .map_err(|_| ())
        });
        match receiver.recv_timeout(Duration::from_secs(1)) {
            Ok(report) => {
                reader.join().unwrap().unwrap();
                assert!(matches!(
                    report.scopes[1].project_skills_sh,
                    Some(OwnershipInput::Failed(_))
                ));
            }
            Err(error) => {
                let writer = fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&fifo)
                    .unwrap();
                drop(writer);
                let _ = receiver.recv_timeout(Duration::from_secs(1));
                let _ = reader.join();
                panic!("ambient ownership read blocked on FIFO: {error}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn failed_project_current_ledger_never_falls_back_to_legacy() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        write_project_ledger(&project, "owner/repo");
        write_legacy_project_ledger(&project, "owner/repo");
        for bytes in [b"bad json".as_slice(), br#"{"version":2,"skills":{}}"#] {
            fs::write(project.join("skills-lock.json"), bytes).unwrap();
            for report in project_reports(root.path(), std::slice::from_ref(&project)) {
                assert_eq!(
                    project_owner(&report, &project),
                    LifecycleOwnerKind::Unknown
                );
                assert!(matches!(
                    report.scopes[1].project_skills_sh,
                    Some(OwnershipInput::Failed(_))
                ));
                assert!(matches!(
                    report.scopes[1].skills_sh,
                    OwnershipInput::Loaded(_)
                ));
            }
        }
        fs::remove_file(project.join("skills-lock.json")).unwrap();
        for report in project_reports(root.path(), std::slice::from_ref(&project)) {
            assert_eq!(
                project_owner(&report, &project),
                LifecycleOwnerKind::SkillsSh
            );
            assert!(matches!(
                report.scopes[1].project_skills_sh,
                Some(OwnershipInput::Absent)
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn project_ledger_links_caps_and_wrong_types_remain_failures() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        write_project_ledger(&project, "owner/repo");
        let path = project.join("skills-lock.json");
        fs::File::create(&path)
            .unwrap()
            .set_len(SCOPED_OWNERSHIP_FILE_BYTE_LIMIT as u64 + 1)
            .unwrap();
        for report in project_reports(root.path(), std::slice::from_ref(&project)) {
            assert_eq!(
                project_owner(&report, &project),
                LifecycleOwnerKind::Unknown
            );
        }
        fs::remove_file(&path).unwrap();
        symlink(project.join("missing-ledger"), &path).unwrap();
        for report in project_reports(root.path(), std::slice::from_ref(&project)) {
            assert_eq!(
                project_owner(&report, &project),
                LifecycleOwnerKind::Unknown
            );
        }
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        for report in project_reports(root.path(), std::slice::from_ref(&project)) {
            assert_eq!(
                project_owner(&report, &project),
                LifecycleOwnerKind::Unknown
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn project_current_ledger_preserves_dotagents_ambiguity() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        write_project_ledger(&project, "owner/repo");
        fs::write(
            project.join(".agents/agents.lock"),
            "[skills.alpha]\nsource = \"owner/repo\"\n",
        )
        .unwrap();
        for report in project_reports(root.path(), std::slice::from_ref(&project)) {
            assert_eq!(
                project_owner(&report, &project),
                LifecycleOwnerKind::Ambiguous
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn scoped_project_ledger_rejects_readable_bytes_outside_declared_roots() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        write_project_ledger(&project, "owner/inside");
        write_project_ledger(outside.path(), "owner/outside");
        let path = project.join("skills-lock.json");
        fs::remove_file(&path).unwrap();
        symlink(outside.path().join("skills-lock.json"), &path).unwrap();
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
        let report = load_ownership_inputs_scoped(
            &scope,
            &root.path().join("home"),
            std::slice::from_ref(&project),
        );
        assert_eq!(
            project_owner(&report, &project),
            LifecycleOwnerKind::Unknown
        );
        assert!(matches!(
            report.scopes[1].project_skills_sh,
            Some(OwnershipInput::Failed(_))
        ));
        assert_eq!(report.failures()[0].path, path.to_string_lossy());
    }

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
            plugin: PluginEvidence::Absent,
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
            git_repo: GitRepoEvidence::Absent,
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

        let ledgers = load_ownership_inputs(&home, std::slice::from_ref(&project));
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

    #[test]
    fn ownership_reads_keep_supplied_homes_independent_without_writes() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["alpha", "beta"] {
            let agents = tmp.path().join(name).join(".agents");
            fs::create_dir_all(&agents).unwrap();
            fs::write(
                agents.join(".skill-lock.json"),
                serde_json::json!({
                    "version": 3,
                    "skills": {name: {
                        "source": format!("{name}/repo"), "sourceType": "github",
                        "sourceUrl": "https://example.test/repo", "skillFolderHash": name,
                        "installedAt": "t", "updatedAt": "t"
                    }}
                })
                .to_string(),
            )
            .unwrap();
            fs::write(
                agents.join("skill-studio.json"),
                serde_json::json!({"preferred_editor": name}).to_string(),
            )
            .unwrap();
        }
        for name in ["alpha", "beta", "alpha"] {
            let home = tmp.path().join(name);
            let agents = home.join(".agents");
            let lock_path = agents.join(".skill-lock.json");
            let registry_path = agents.join("skill-studio.json");
            let before = (
                fs::read(&lock_path).unwrap(),
                fs::read(&registry_path).unwrap(),
            );
            let report = load_ownership_inputs(&home, &[]);
            assert!(report.failures().is_empty());
            assert_eq!(report.scopes.len(), 1);
            assert_eq!(report.scopes[0].agents_dir, agents);
            let lock = report.global_lock();
            assert_eq!(lock.skills.len(), 1);
            assert_eq!(lock.skills[name].source, format!("{name}/repo"));
            let OwnershipInput::Loaded(registry) = report.registry else {
                panic!("fixture registry must be loaded");
            };
            assert_eq!(registry.preferred_editor.as_deref(), Some(name));
            assert_eq!(
                (
                    fs::read(lock_path).unwrap(),
                    fs::read(registry_path).unwrap()
                ),
                before
            );
            assert_eq!(fs::read_dir(&agents).unwrap().count(), 2);
        }
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

    fn write_dotagents_pair(root: &Path, source: &str, path: &str, commit: &str, manifest: &str) {
        fs::create_dir_all(root).unwrap();
        fs::write(
            root.join("agents.lock"),
            format!(
                "[skills.alpha]\nsource = \"{source}\"\nresolved_path = \"{path}\"\nresolved_commit = \"{commit}\"\n"
            ),
        )
        .unwrap();
        fs::write(root.join("agents.toml"), manifest).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn project_dotagents_selects_atomic_current_or_legacy_pairs_for_both_readers() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        fs::create_dir_all(project.join(".agents/skills/alpha")).unwrap();
        let manifest = "[[skills]]\nname = \"alpha\"\nref = \"main\"\n";

        write_dotagents_pair(&project, "owner/repo", "skills/alpha", "commit", manifest);
        for report in project_reports(root.path(), std::slice::from_ref(&project)) {
            let input = &report.scopes[1];
            assert_eq!(input.dotagents.effective_pair().unwrap().root, project);
            assert_eq!(
                project_owner(&report, &project),
                LifecycleOwnerKind::Dotagents
            );
        }

        fs::remove_file(project.join("agents.lock")).unwrap();
        fs::remove_file(project.join("agents.toml")).unwrap();
        write_dotagents_pair(
            &project.join(".agents"),
            "owner/repo",
            "skills/alpha",
            "commit",
            manifest,
        );
        for report in project_reports(root.path(), std::slice::from_ref(&project)) {
            let input = &report.scopes[1];
            assert_eq!(
                input.dotagents.effective_pair().unwrap().root,
                project.join(".agents")
            );
            assert_eq!(
                project_owner(&report, &project),
                LifecycleOwnerKind::Dotagents
            );
        }

        write_dotagents_pair(&project, "owner/repo", "skills/alpha", "commit", manifest);
        for report in project_reports(root.path(), std::slice::from_ref(&project)) {
            let input = &report.scopes[1];
            assert_eq!(input.dotagents.effective_pair().unwrap().root, project);
            assert_eq!(
                project_owner(&report, &project),
                LifecycleOwnerKind::Dotagents
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn project_dotagents_conflicts_and_split_pairs_stay_unknown_for_both_readers() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        fs::create_dir_all(project.join(".agents/skills/alpha")).unwrap();
        let manifest = "[[skills]]\nname = \"alpha\"\nref = \"main\"\n";
        let legacy = project.join(".agents");

        for (
            current_source,
            current_path,
            current_commit,
            current_manifest,
            legacy_source,
            legacy_path,
            legacy_commit,
            legacy_manifest,
        ) in [
            (
                "other/repo",
                "skills/alpha",
                "commit",
                manifest,
                "owner/repo",
                "skills/alpha",
                "commit",
                manifest,
            ),
            (
                "owner/repo",
                "other/alpha",
                "commit",
                manifest,
                "owner/repo",
                "skills/alpha",
                "commit",
                manifest,
            ),
            (
                "owner/repo",
                "skills/alpha",
                "other",
                manifest,
                "owner/repo",
                "skills/alpha",
                "commit",
                manifest,
            ),
            (
                "owner/repo",
                "skills/alpha",
                "commit",
                "[[skills]]\nname = \"beta\"\nref = \"main\"\n",
                "owner/repo",
                "skills/alpha",
                "commit",
                manifest,
            ),
            (
                "owner/repo",
                "skills/alpha",
                "commit",
                "[[skills]]\nname = \"alpha\"\nref = \"next\"\n",
                "owner/repo",
                "skills/alpha",
                "commit",
                manifest,
            ),
        ] {
            write_dotagents_pair(
                &project,
                current_source,
                current_path,
                current_commit,
                current_manifest,
            );
            write_dotagents_pair(
                &legacy,
                legacy_source,
                legacy_path,
                legacy_commit,
                legacy_manifest,
            );
            for report in project_reports(root.path(), std::slice::from_ref(&project)) {
                let input = &report.scopes[1];
                assert!(input.dotagents.effective_pair().is_err());
                assert!(input.as_ledger().is_err());
                assert_eq!(
                    project_owner(&report, &project),
                    LifecycleOwnerKind::Unknown
                );
                let failures = report.failures();
                let issue = failures.last().unwrap();
                assert!(issue
                    .message
                    .contains(project.join("agents.lock").to_str().unwrap()));
                assert!(issue
                    .message
                    .contains(legacy.join("agents.lock").to_str().unwrap()));
            }
        }

        for current_lock in [true, false] {
            let _ = fs::remove_file(project.join("agents.lock"));
            let _ = fs::remove_file(project.join("agents.toml"));
            let _ = fs::remove_file(legacy.join("agents.lock"));
            let _ = fs::remove_file(legacy.join("agents.toml"));
            if current_lock {
                write_dotagents_pair(&project, "owner/repo", "skills/alpha", "commit", "");
                fs::remove_file(project.join("agents.toml")).unwrap();
                fs::write(legacy.join("agents.toml"), manifest).unwrap();
            } else {
                fs::write(project.join("agents.toml"), manifest).unwrap();
                write_dotagents_pair(&legacy, "owner/repo", "skills/alpha", "commit", "");
                fs::remove_file(legacy.join("agents.toml")).unwrap();
            }
            for report in project_reports(root.path(), std::slice::from_ref(&project)) {
                assert!(report.scopes[1].dotagents.effective_pair().is_err());
                assert_eq!(
                    project_owner(&report, &project),
                    LifecycleOwnerKind::Unknown
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn project_dotagents_preserves_failures_and_empty_pairs_for_both_readers() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let legacy = project.join(".agents");
        fs::create_dir_all(legacy.join("skills/alpha")).unwrap();
        let manifest = "[[skills]]\nname = \"alpha\"\nref = \"main\"\n";
        write_dotagents_pair(&project, "owner/repo", "skills/alpha", "commit", manifest);
        write_dotagents_pair(&legacy, "owner/repo", "skills/alpha", "commit", manifest);

        for malformed_current in [true, false] {
            let path = if malformed_current {
                project.join("agents.lock")
            } else {
                legacy.join("agents.toml")
            };
            fs::write(&path, "{ malformed").unwrap();
            for report in project_reports(root.path(), std::slice::from_ref(&project)) {
                assert!(report.scopes[1].is_failed());
                assert!(report.scopes[1].as_ledger().is_err());
                assert_eq!(
                    project_owner(&report, &project),
                    LifecycleOwnerKind::Unknown
                );
                let failures = report.failures();
                assert_eq!(failures.len(), 1);
                assert_eq!(failures[0].path, path.to_string_lossy());
                assert_eq!(
                    failures[0].kind,
                    if malformed_current {
                        OwnershipReadIssueKind::DotagentsLock
                    } else {
                        OwnershipReadIssueKind::DotagentsManifest
                    }
                );
            }
            if malformed_current {
                write_dotagents_pair(&project, "owner/repo", "skills/alpha", "commit", manifest);
            } else {
                write_dotagents_pair(&legacy, "owner/repo", "skills/alpha", "commit", manifest);
            }
        }

        for path in [
            project.join("agents.lock"),
            project.join("agents.toml"),
            legacy.join("agents.lock"),
            legacy.join("agents.toml"),
        ] {
            fs::remove_file(path).unwrap();
        }
        for report in project_reports(root.path(), std::slice::from_ref(&project)) {
            assert!(report.scopes[1].dotagents.current.is_absent());
            assert!(report.scopes[1]
                .dotagents
                .legacy
                .as_ref()
                .unwrap()
                .is_absent());
            assert!(report.scopes[1].as_ledger().is_ok());
            assert_eq!(project_owner(&report, &project), LifecycleOwnerKind::Manual);
        }
    }

    #[cfg(unix)]
    #[test]
    fn duplicate_dotagents_manifest_names_fail_in_both_locations_and_orders() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let legacy = project.join(".agents");
        fs::create_dir_all(legacy.join("skills/alpha")).unwrap();
        let valid = "[[skills]]\nname = \"alpha\"\nref = \"main\"\n";
        for duplicate_root in [&project, &legacy] {
            for refs in [["old", "main"], ["main", "old"], ["main", "main"]] {
                write_dotagents_pair(&project, "owner/repo", "skills/alpha", "commit", valid);
                write_dotagents_pair(&legacy, "owner/repo", "skills/alpha", "commit", valid);
                let duplicate = refs
                    .iter()
                    .map(|value| format!("[[skills]]\nname = \"alpha\"\nref = {value:?}\n"))
                    .collect::<String>();
                let path = duplicate_root.join("agents.toml");
                fs::write(&path, duplicate).unwrap();
                for report in project_reports(root.path(), std::slice::from_ref(&project)) {
                    assert_eq!(
                        project_owner(&report, &project),
                        LifecycleOwnerKind::Unknown
                    );
                    assert!(report.scopes[1].as_ledger().is_err());
                    let failures = report.failures();
                    assert_eq!(failures.len(), 1);
                    assert_eq!(failures[0].kind, OwnershipReadIssueKind::DotagentsManifest);
                    assert_eq!(failures[0].path, path.to_string_lossy());
                    assert!(failures[0].message.contains("Duplicate skill name alpha"));
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn scoped_project_dotagents_rejects_outside_dangling_and_oversized_current_inputs() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        fs::create_dir_all(project.join(".agents/skills/alpha")).unwrap();
        fs::write(
            outside.path().join("agents.lock"),
            "[skills.alpha]\nsource = \"outside\"\n",
        )
        .unwrap();
        let lock_path = project.join("agents.lock");
        symlink(outside.path().join("agents.lock"), &lock_path).unwrap();
        let scope = SkillReadScope::bind(&[root.path().to_path_buf()]).unwrap();
        let report = load_ownership_inputs_scoped(
            &scope,
            &root.path().join("home"),
            std::slice::from_ref(&project),
        );
        assert!(report.scopes[1].is_failed());
        assert_eq!(
            project_owner(&report, &project),
            LifecycleOwnerKind::Unknown
        );

        fs::remove_file(&lock_path).unwrap();
        symlink("missing.lock", &lock_path).unwrap();
        let report = load_ownership_inputs_scoped(
            &scope,
            &root.path().join("home"),
            std::slice::from_ref(&project),
        );
        assert!(report.scopes[1].is_failed());
        assert_eq!(
            project_owner(&report, &project),
            LifecycleOwnerKind::Unknown
        );

        fs::remove_file(&lock_path).unwrap();
        fs::write(&lock_path, vec![b'x'; SCOPED_OWNERSHIP_FILE_BYTE_LIMIT + 1]).unwrap();
        let report = load_ownership_inputs_scoped(
            &scope,
            &root.path().join("home"),
            std::slice::from_ref(&project),
        );
        assert!(report.scopes[1].is_failed());
        assert_eq!(
            project_owner(&report, &project),
            LifecycleOwnerKind::Unknown
        );
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
        let ledgers = load_ownership_inputs(home, &[]);

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
        let ledgers = load_ownership_inputs(&home, std::slice::from_ref(&project));

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

        let candidate = discover(home)
            .candidates
            .into_iter()
            .find(|candidate| candidate.path == skill_dir)
            .unwrap();
        let ledgers = load_ownership_inputs(home, &[]);
        let (owner, owner_id, _) = classify_lifecycle_owner(
            &candidate,
            &ledgers,
            SkillDestination::Universal,
            "missing",
            &Default::default(),
        );
        assert_eq!(candidate.name, "foo");
        assert_eq!(owner, LifecycleOwnerKind::Manual);
        assert!(!owner.is_mutable());
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

        let candidate = discover(home)
            .candidates
            .into_iter()
            .find(|candidate| candidate.path == skill_dir)
            .unwrap();
        let ledgers = load_ownership_inputs(home, &[]);
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
        let ledgers = load_ownership_inputs(&home, &[]);
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
            let (id, destination, _) = crate::skill_deployment::id_for_candidate(
                crate::skill_deployment::DeploymentCandidate {
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
        let id = crate::skill_deployment::deployment_id(
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
        let (owner, owner_id, _) = classify_lifecycle_owner(
            &candidate,
            &OwnershipReadReport::empty(),
            SkillDestination::PerHarness,
            &id,
            &records,
        );
        assert_eq!(owner, LifecycleOwnerKind::Copy);
        assert!(owner_id.is_none());

        let wrong_id = id.replace("/codex/", "/claude-code/");
        let (owner, _, _) = classify_lifecycle_owner(
            &candidate,
            &OwnershipReadReport::empty(),
            SkillDestination::PerHarness,
            &wrong_id,
            &records,
        );
        assert_eq!(owner, LifecycleOwnerKind::Manual);
    }

    #[test]
    fn outside_home_project_copy_can_override_unknown_plugin_ancestry() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("outside-project");
        let skill_dir = project.join(".agents/skills/sample");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: sample\ndescription: Fixture\n---\nBody\n",
        )
        .unwrap();
        let context = crate::skill_plugins::SkillDiscoveryReadContext::bind(
            home.clone(),
            vec![project.clone()],
            vec![],
            vec![],
        );
        let candidate = crate::skill_discovery::discover_skill_candidates(&context)
            .candidates
            .into_iter()
            .find(|candidate| candidate.path == skill_dir)
            .unwrap();
        assert_eq!(candidate.project_path.as_deref(), Some(project.as_path()));
        assert!(matches!(candidate.plugin, PluginEvidence::Unknown));

        let (deployment_id, destination, _) = crate::skill_deployment::id_for_candidate(
            crate::skill_deployment::DeploymentCandidate {
                name: &candidate.name,
                root_label: &candidate.root_label,
                scope: &candidate.scope,
                path: &candidate.path,
                project_path: candidate.project_path.as_deref().and_then(Path::to_str),
                is_symlink: candidate.is_symlink,
                symlink_target: candidate.symlink_target.as_deref(),
                resolved_path: candidate.resolved_path.as_deref(),
                shared_via_whole_dir_link: candidate.shared_via_whole_dir_link,
            },
        );
        let record = CopyDeploymentRecord {
            deployment_id: deployment_id.clone(),
            name: candidate.name.clone(),
            path: candidate.path.clone(),
            scope: InstallScope::Project,
            destination,
            slot: "universal".to_string(),
            project_path: Some(project.to_string_lossy().into_owned()),
            content_hash: candidate.content_hash.clone(),
            disabled: false,
        };
        let registry = crate::skill_fork_registry::ForkRegistry {
            copies: BTreeMap::from([(deployment_id.clone(), record)]),
            ..Default::default()
        };
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(
            home.join(".agents/skill-studio.json"),
            serde_json::to_vec(&registry).unwrap(),
        )
        .unwrap();
        let report = load_ownership_inputs(&home, std::slice::from_ref(&project));
        let records = report.copy_records();
        assert_eq!(records.len(), 1);

        assert_eq!(
            classify_lifecycle_owner(&candidate, &report, destination, &deployment_id, &records,).0,
            LifecycleOwnerKind::Copy
        );

        let mut changed = candidate;
        changed.content_hash = "different-content".to_string();
        assert_eq!(
            classify_lifecycle_owner(&changed, &report, destination, &deployment_id, &records,).0,
            LifecycleOwnerKind::Unknown
        );
    }

    fn discover_copy_candidate(home: &Path, skill_dir: &Path) -> SkillCandidate {
        discover(home)
            .candidates
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
        let deployment_id = crate::skill_deployment::deployment_id(
            "find-bugs",
            "global",
            SkillDestination::PerHarness,
            "codex",
            None,
            &skill_dir,
        );
        (tmp, skill_dir, candidate, deployment_id)
    }

    #[cfg(unix)]
    #[test]
    fn unregistered_link_target_cannot_bypass_manager_reads() {
        for relative in [
            "foo",
            ".skill-studio-disabled/foo",
            "nested/foo",
            "nested/.agents/skills/foo",
        ] {
            let home = tempfile::tempdir().unwrap();
            let project = home.path().join("unregistered-project");
            let target = project.join(".agents/skills").join(relative);
            let link = home.path().join(".claude/skills/foo");
            fs::create_dir_all(&target).unwrap();
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            fs::write(
                target.join("SKILL.md"),
                "---\nname: foo\ndescription: fixture\n---\nbody",
            )
            .unwrap();
            std::os::unix::fs::symlink(&target, &link).unwrap();
            for contents in ["{", r#"{"version":3,"skills":{}}"#] {
                fs::write(project.join(".agents/.skill-lock.json"), contents).unwrap();
                let candidate = discover(home.path())
                    .candidates
                    .into_iter()
                    .find(|candidate| candidate.path == link)
                    .unwrap();
                let report = load_ownership_inputs(home.path(), &[]);
                let (owner, _, _) = classify_lifecycle_owner(
                    &candidate,
                    &report,
                    SkillDestination::PerHarness,
                    "linked",
                    &BTreeMap::new(),
                );
                assert_eq!(
                    owner,
                    LifecycleOwnerKind::Unknown,
                    "unread target records must not grant edits"
                );
                let known = load_ownership_inputs(home.path(), std::slice::from_ref(&project));
                let (owner, _, _) = classify_lifecycle_owner(
                    &candidate,
                    &known,
                    SkillDestination::PerHarness,
                    "linked",
                    &BTreeMap::new(),
                );
                assert_eq!(
                    owner,
                    if contents == "{" || relative == "nested/.agents/skills/foo" {
                        LifecycleOwnerKind::Unknown
                    } else {
                        LifecycleOwnerKind::Manual
                    }
                );
                if relative == "foo" || relative == ".skill-studio-disabled/foo" {
                    let context = crate::skill_plugins::SkillDiscoveryReadContext::bind(
                        home.path().to_path_buf(),
                        vec![project.clone()],
                        vec![],
                        vec![],
                    );
                    let direct = crate::skill_discovery::discover_skill_candidates(&context)
                        .candidates
                        .into_iter()
                        .find(|candidate| candidate.path == target)
                        .unwrap();
                    let (owner, _, _) = classify_lifecycle_owner(
                        &direct,
                        &known,
                        SkillDestination::Universal,
                        "direct",
                        &BTreeMap::new(),
                    );
                    assert_eq!(
                        owner,
                        if contents == "{" {
                            LifecycleOwnerKind::Unknown
                        } else {
                            LifecycleOwnerKind::Manual
                        }
                    );
                }
            }
        }
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
            &OwnershipReadReport::empty(),
            SkillDestination::PerHarness,
            &deployment_id,
            &BTreeMap::from([(deployment_id.clone(), record)]),
        );

        assert_eq!(owner, LifecycleOwnerKind::Unknown);
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
            &OwnershipReadReport::empty(),
            SkillDestination::PerHarness,
            &deployment_id,
            &BTreeMap::from([(deployment_id.clone(), record)]),
        );

        assert_eq!(owner, LifecycleOwnerKind::Unknown);
        assert!(!owner.is_mutable());
    }

    #[test]
    fn copy_ownership_rejects_an_empty_legacy_content_hash() {
        let (_tmp, _skill_dir, candidate, deployment_id) = discovered_copy_fixture();
        let mut record = copy_record(&candidate, &deployment_id);
        record.content_hash.clear();

        let (owner, _, _) = classify_lifecycle_owner(
            &candidate,
            &OwnershipReadReport::empty(),
            SkillDestination::PerHarness,
            &deployment_id,
            &BTreeMap::from([(deployment_id.clone(), record)]),
        );

        assert_eq!(owner, LifecycleOwnerKind::Unknown);
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
        let ledgers = load_ownership_inputs(&home, &[]);
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
        let ledgers = load_ownership_inputs(&home, &[]);
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

    #[test]
    fn ownership_inputs_distinguish_absence_from_each_failed_source() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("project");
        let report = load_ownership_inputs(home, std::slice::from_ref(&project));
        assert_eq!(report.scopes.len(), 2);
        for scope in &report.scopes {
            assert!(matches!(scope.skills_sh, OwnershipInput::Absent));
            assert!(matches!(
                scope.dotagents.current.lock,
                OwnershipInput::Absent
            ));
            assert!(matches!(
                scope.dotagents.current.manifest,
                OwnershipInput::Absent
            ));
            assert!(scope.as_ledger().is_ok());
        }
        assert!(matches!(report.registry, OwnershipInput::Absent));
        assert!(report.failures().is_empty());
        assert!(!home.join(".agents").exists());
        assert!(!project.exists());

        let agents = home.join(".agents");
        fs::create_dir(&agents).unwrap();
        for (filename, kind) in [
            (".skill-lock.json", OwnershipReadIssueKind::SkillsShLock),
            ("agents.lock", OwnershipReadIssueKind::DotagentsLock),
            ("agents.toml", OwnershipReadIssueKind::DotagentsManifest),
            (
                "skill-studio.json",
                OwnershipReadIssueKind::LifecycleRegistry,
            ),
        ] {
            let path = agents.join(filename);
            fs::write(&path, "{ malformed").unwrap();
            let report = load_ownership_inputs(home, &[]);
            let failures = report.failures();
            assert_eq!(failures.len(), 1, "{filename}");
            assert_eq!(failures[0].kind, kind);
            assert_eq!(failures[0].path, path.to_string_lossy());
            assert!(!failures[0].message.is_empty());
            let c = candidate(
                "alpha",
                "shared",
                &agents.join("skills/alpha").to_string_lossy(),
            );
            let (owner, id, _) = classify_lifecycle_owner(
                &c,
                &report,
                SkillDestination::Universal,
                "alpha",
                &Default::default(),
            );
            assert_eq!(owner, LifecycleOwnerKind::Unknown, "{filename}");
            assert!(id.is_none());
            if kind != OwnershipReadIssueKind::LifecycleRegistry {
                assert!(report.scopes[0].as_ledger().is_err());
            }
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn failed_project_ledger_does_not_change_other_scopes() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project_a = tmp.path().join("a");
        let project_b = tmp.path().join("b");
        fs::create_dir_all(project_a.join(".agents")).unwrap();
        fs::write(project_a.join(".agents/agents.toml"), "[broken").unwrap();
        let report = load_ownership_inputs(&home, &[project_a.clone(), project_b.clone()]);
        assert_eq!(report.failures().len(), 1);
        for (root, expected) in [
            (&home, LifecycleOwnerKind::Manual),
            (&project_a, LifecycleOwnerKind::Unknown),
            (&project_b, LifecycleOwnerKind::Manual),
        ] {
            let mut c = candidate(
                "alpha",
                "shared",
                &root.join(".agents/skills/alpha").to_string_lossy(),
            );
            if root != &home {
                c.scope = "project".to_string();
                c.project_path = Some(root.clone());
            }
            let (owner, _, _) = classify_lifecycle_owner(
                &c,
                &report,
                SkillDestination::Universal,
                "alpha",
                &Default::default(),
            );
            assert_eq!(owner, expected);
        }
    }

    #[test]
    fn failed_registry_blocks_recorded_per_harness_copy_but_not_plugin_evidence() {
        let (tmp, _, mut c, deployment_id) = discovered_copy_fixture();
        let records = BTreeMap::from([(deployment_id.clone(), copy_record(&c, &deployment_id))]);
        let registry = crate::skill_fork_registry::ForkRegistry {
            copies: records.clone(),
            ..Default::default()
        };
        crate::skill_fork_registry::write_fork_registry(tmp.path(), &registry).unwrap();
        let report = load_ownership_inputs(tmp.path(), &[]);
        assert_eq!(
            classify_lifecycle_owner(
                &c,
                &report,
                SkillDestination::PerHarness,
                &deployment_id,
                &report.copy_records()
            )
            .0,
            LifecycleOwnerKind::Copy
        );
        let path = crate::skill_fork_registry::fork_registry_path(tmp.path());
        fs::write(path, "{ broken").unwrap();
        let report = load_ownership_inputs(tmp.path(), &[]);
        assert_eq!(
            classify_lifecycle_owner(
                &c,
                &report,
                SkillDestination::PerHarness,
                &deployment_id,
                &records
            )
            .0,
            LifecycleOwnerKind::Unknown
        );
        c.plugin = PluginEvidence::Confirmed(crate::skill_plugins::PluginInfo {
            name: "fixture".to_string(),
            version: None,
            harness: "Codex".to_string(),
        });
        assert_eq!(
            classify_lifecycle_owner(
                &c,
                &report,
                SkillDestination::PerHarness,
                &deployment_id,
                &records
            )
            .0,
            LifecycleOwnerKind::Plugin
        );
    }

    #[test]
    fn unknown_plugin_evidence_precedes_valid_manual_and_ledger_ownership() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let agents = home.join(".agents");
        fs::create_dir_all(agents.join("skills/alpha")).unwrap();
        fs::write(
            agents.join(".skill-lock.json"),
            r#"{"version":3,"skills":{"alpha":{"source":"o/r","sourceType":"github","sourceUrl":"https://github.com/o/r","skillFolderHash":"hash","installedAt":"t","updatedAt":"t"}}}"#,
        )
        .unwrap();
        let mut candidate = candidate(
            "alpha",
            "shared",
            &agents.join("skills/alpha").to_string_lossy(),
        );
        candidate.plugin = PluginEvidence::Unknown;

        let manual = OwnershipReadReport::empty();
        assert_eq!(
            classify_lifecycle_owner(
                &candidate,
                &manual,
                SkillDestination::PerHarness,
                "alpha",
                &Default::default(),
            )
            .0,
            LifecycleOwnerKind::Unknown
        );
        let ledger = load_ownership_inputs(home, &[]);
        assert_eq!(
            classify_lifecycle_owner(
                &candidate,
                &ledger,
                SkillDestination::Universal,
                "alpha",
                &Default::default(),
            )
            .0,
            LifecycleOwnerKind::Unknown
        );
    }

    #[test]
    fn failed_scoped_lock_is_unknown_but_plugin_evidence_stays_authoritative() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let agents = home.join(".agents");
        fs::create_dir_all(agents.join("skills/find-bugs")).unwrap();
        fs::write(agents.join(".skill-lock.json"), "not json").unwrap();
        let report = load_ownership_inputs(&home, &[]);
        assert!(matches!(
            report.scopes[0].skills_sh,
            OwnershipInput::Failed(_)
        ));

        let candidate = candidate(
            "find-bugs",
            "shared",
            &agents.join("skills/find-bugs").to_string_lossy(),
        );
        let (owner, id, _) = classify_lifecycle_owner(
            &candidate,
            &report,
            SkillDestination::Universal,
            "deployment",
            &Default::default(),
        );
        assert_eq!(owner, LifecycleOwnerKind::Unknown);
        assert_eq!(id, None);

        let mut harness = candidate.clone();
        harness.path = home.join(".claude/skills/find-bugs");
        assert_eq!(
            classify_lifecycle_owner(
                &harness,
                &report,
                SkillDestination::PerHarness,
                "harness",
                &Default::default(),
            )
            .0,
            LifecycleOwnerKind::Unknown,
        );

        let mut plugin = candidate;
        plugin.plugin = PluginEvidence::Confirmed(crate::skill_plugins::PluginInfo {
            name: "plugin".to_string(),
            version: None,
            harness: "Codex".to_string(),
        });
        let (owner, _, kind) = classify_lifecycle_owner(
            &plugin,
            &report,
            SkillDestination::Universal,
            "deployment",
            &Default::default(),
        );
        assert_eq!(owner, LifecycleOwnerKind::Plugin);
        assert_eq!(kind, SourceKind::Plugin);
    }

    #[cfg(unix)]
    fn scoped_reader(home: &Path, roots: &[PathBuf]) -> OwnershipReadReport {
        let scope = SkillReadScope::bind(roots).unwrap();
        load_ownership_inputs_scoped(&scope, home, &[])
    }

    #[cfg(unix)]
    #[test]
    fn scoped_ownership_uses_the_same_decoders_as_ambient_reads() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let agents = home.join(".agents");
        fs::create_dir_all(agents.join("skills")).unwrap();
        fs::write(
            agents.join(".skill-lock.json"),
            r#"{"version":3,"skills":{"alpha":{"source":"o/r","sourceType":"github","sourceUrl":"https://example.test/o/r","skillFolderHash":"hash","installedAt":"t","updatedAt":"t"}}}"#,
        )
        .unwrap();
        fs::write(
            agents.join("agents.lock"),
            "[skills.alpha]\nsource = \"o/r\"\nresolved_commit = \"commit\"\n",
        )
        .unwrap();
        fs::write(
            agents.join("agents.toml"),
            "[[skills]]\nname = \"alpha\"\nref = \"main\"\n",
        )
        .unwrap();
        fs::write(
            agents.join("skill-studio.json"),
            r#"{"preferred_editor":"Code"}"#,
        )
        .unwrap();

        let ambient = load_ownership_inputs(&home, &[]);
        let scoped = scoped_reader(&home, std::slice::from_ref(&home));
        assert_eq!(
            ambient.global_lock().skills["alpha"].source,
            scoped.global_lock().skills["alpha"].source
        );
        let ambient_ledger = ambient.scopes[0].as_ledger().unwrap();
        let scoped_ledger = scoped.scopes[0].as_ledger().unwrap();
        assert_eq!(
            ambient_ledger.dotagents[0].installed_commit,
            scoped_ledger.dotagents[0].installed_commit
        );
        assert_eq!(
            ambient_ledger.dotagents[0].declared_ref,
            scoped_ledger.dotagents[0].declared_ref
        );
        let OwnershipInput::Loaded(registry) = scoped.registry else {
            panic!("scoped registry must load");
        };
        assert_eq!(registry.preferred_editor.as_deref(), Some("Code"));
    }

    #[cfg(unix)]
    #[test]
    fn scoped_ownership_reports_each_malformed_source_with_parse_parity() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let agents = home.join(".agents");
        fs::create_dir_all(agents.join("skills")).unwrap();
        for (filename, kind) in [
            (".skill-lock.json", OwnershipReadIssueKind::SkillsShLock),
            ("agents.lock", OwnershipReadIssueKind::DotagentsLock),
            ("agents.toml", OwnershipReadIssueKind::DotagentsManifest),
            (
                "skill-studio.json",
                OwnershipReadIssueKind::LifecycleRegistry,
            ),
        ] {
            let path = agents.join(filename);
            fs::write(&path, "{ malformed").unwrap();
            let ambient = load_ownership_inputs(&home, &[]);
            let scoped = scoped_reader(&home, std::slice::from_ref(&home));
            assert_eq!(scoped.failures().len(), 1, "{filename}");
            assert_eq!(scoped.failures()[0].kind, kind);
            assert_eq!(scoped.failures()[0].path, path.to_string_lossy());
            assert_eq!(scoped.failures()[0].message, ambient.failures()[0].message);
            fs::remove_file(path).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn scoped_ownership_distinguishes_missing_from_dangling_links() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".agents/skills")).unwrap();
        let absent = scoped_reader(&home, std::slice::from_ref(&home));
        assert!(matches!(absent.scopes[0].skills_sh, OwnershipInput::Absent));
        assert!(matches!(absent.registry, OwnershipInput::Absent));

        symlink("missing-lock", home.join(".agents/.skill-lock.json")).unwrap();
        let dangling_final = scoped_reader(&home, std::slice::from_ref(&home));
        assert!(matches!(
            dangling_final.scopes[0].skills_sh,
            OwnershipInput::Failed(_)
        ));
        assert_eq!(
            dangling_final.failures()[0].kind,
            OwnershipReadIssueKind::SkillsShLock
        );

        fs::remove_file(home.join(".agents/.skill-lock.json")).unwrap();
        fs::remove_dir_all(home.join(".agents")).unwrap();
        symlink("missing-agents", home.join(".agents")).unwrap();
        let dangling_ancestor = scoped_reader(&home, std::slice::from_ref(&home));
        assert!(matches!(
            dangling_ancestor.scopes[0].skills_root,
            OwnershipInput::Failed(_)
        ));
        assert!(matches!(
            dangling_ancestor.scopes[0].skills_sh,
            OwnershipInput::Failed(_)
        ));
        assert!(matches!(
            dangling_ancestor.registry,
            OwnershipInput::Failed(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn scoped_ownership_resolves_declared_relative_and_absolute_skills_backing() {
        let tmp = tempfile::tempdir().unwrap();
        let backing = tmp.path().join("backing");
        fs::create_dir_all(&backing).unwrap();
        for (name, target) in [
            ("absolute", backing.clone()),
            ("relative", PathBuf::from("../../backing")),
        ] {
            let home = tmp.path().join(name);
            fs::create_dir_all(home.join(".agents")).unwrap();
            symlink(target, home.join(".agents/skills")).unwrap();
            let report = scoped_reader(&home, &[home.clone(), backing.clone()]);
            assert_eq!(
                report.scopes[0].canonical_skills_dir.as_deref(),
                Some(std::fs::canonicalize(&backing).unwrap().as_path())
            );
            assert!(matches!(
                report.scopes[0].skills_root,
                OwnershipInput::Loaded(_)
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn scoped_ownership_denies_outside_bytes_and_preserves_unknown_ownership() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(home.join(".agents/skills/alpha")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(
            outside.join("lock"),
            r#"{"version":3,"skills":{"outside":{"source":"outside bytes must not be loaded","sourceType":"github","sourceUrl":"https://example.test/outside","skillFolderHash":"hash","installedAt":"t","updatedAt":"t"}}}"#,
        )
        .unwrap();
        symlink(outside.join("lock"), home.join(".agents/.skill-lock.json")).unwrap();
        let report = scoped_reader(&home, std::slice::from_ref(&home));
        assert!(matches!(
            report.scopes[0].skills_sh,
            OwnershipInput::Failed(_)
        ));
        assert!(!report.failures()[0]
            .message
            .contains("outside bytes must not be loaded"));
        let deployment = candidate(
            "alpha",
            "shared",
            &home.join(".agents/skills/alpha").to_string_lossy(),
        );
        assert_eq!(
            classify_lifecycle_owner(
                &deployment,
                &report,
                SkillDestination::Universal,
                "alpha",
                &Default::default()
            )
            .0,
            LifecycleOwnerKind::Unknown
        );
    }

    #[cfg(unix)]
    #[test]
    fn shared_backing_alias_does_not_guess_between_healthy_scopes() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let backing = tmp.path().join("backing");
        fs::create_dir_all(backing.join("alpha")).unwrap();
        for (root, source) in [(&home, "global/repo"), (&project, "project/repo")] {
            let agents = root.join(".agents");
            fs::create_dir_all(&agents).unwrap();
            symlink(&backing, agents.join("skills")).unwrap();
            fs::write(
                agents.join(".skill-lock.json"),
                serde_json::json!({"version": 3, "skills": {"alpha": {
                    "source": source, "sourceType": "github", "sourceUrl": "https://example.com/repo",
                    "skillFolderHash": "hash", "installedAt": "t", "updatedAt": "t"
                }}}).to_string(),
            ).unwrap();
        }
        let scope =
            SkillReadScope::bind(&[home.clone(), project.clone(), backing.clone()]).unwrap();
        let report = load_ownership_inputs_scoped(&scope, &home, std::slice::from_ref(&project));
        assert!(report.failures().is_empty());
        let target = fs::canonicalize(backing.join("alpha")).unwrap();
        for root in [&home, &project] {
            let mut alias = candidate(
                "alpha",
                "Claude Code",
                &root.join(".claude/skills/alpha").to_string_lossy(),
            );
            alias.is_symlink = true;
            alias.resolved_path = Some(target.clone());
            alias.symlink_target = Some(target.clone());
            if root == &project {
                alias.scope = "project".into();
                alias.project_path = Some(project.clone());
            }
            for git in [
                GitRepoEvidence::Absent,
                GitRepoEvidence::Present,
                GitRepoEvidence::Unknown,
            ] {
                alias.git_repo = git;
                let mut reordered = report.clone();
                for _ in 0..2 {
                    let owner = classify_lifecycle_owner(
                        &alias,
                        &reordered,
                        SkillDestination::Universal,
                        "alias",
                        &Default::default(),
                    );
                    assert_eq!(
                        owner,
                        (LifecycleOwnerKind::Ambiguous, None, SourceKind::Unknown)
                    );
                    assert!(!owner.0.is_mutable());
                    assert!(update_source_for_candidate(
                        &alias,
                        &reordered,
                        LifecycleOwnerKind::SkillsSh
                    )
                    .is_none());
                    reordered.scopes.reverse();
                }
                let mut failed_registry = report.clone();
                failed_registry.registry = OwnershipInput::Failed(OwnershipReadIssue {
                    kind: OwnershipReadIssueKind::LifecycleRegistry,
                    path: "fixture-registry".into(),
                    message: "unavailable".into(),
                });
                let owner = classify_lifecycle_owner(
                    &alias,
                    &failed_registry,
                    SkillDestination::Universal,
                    "alias",
                    &Default::default(),
                );
                assert_eq!(owner.0, LifecycleOwnerKind::Unknown);
                assert_eq!(owner.1, None);
            }
            let mut exact = alias.clone();
            exact.path = root.join(".agents/skills/alpha");
            let input = report
                .scopes
                .iter()
                .find(|input| input.agents_dir == root.join(".agents"))
                .unwrap();
            let owner = classify_lifecycle_owner(
                &exact,
                &report,
                SkillDestination::Universal,
                "exact",
                &Default::default(),
            );
            assert_eq!(owner.0, LifecycleOwnerKind::SkillsSh);
            assert_eq!(owner.1, Some(owner_id_for_scope(input, "alpha")));

            for input in &report.scopes {
                let unique = OwnershipReadReport {
                    scopes: vec![input.clone()],
                    registry: OwnershipInput::Absent,
                };
                let owner = classify_lifecycle_owner(
                    &alias,
                    &unique,
                    SkillDestination::Universal,
                    "unique",
                    &Default::default(),
                );
                assert_eq!(owner.0, LifecycleOwnerKind::SkillsSh);
                assert_eq!(owner.1, Some(owner_id_for_scope(input, "alpha")));
                assert!(update_source_for_candidate(&alias, &unique, owner.0).is_some());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn failed_scoped_ownership_stays_unknown_through_a_shared_backing_alias() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let backing = tmp.path().join("backing");
        fs::create_dir_all(backing.join("alpha")).unwrap();
        for agents in [home.join(".agents"), project.join(".agents")] {
            fs::create_dir_all(&agents).unwrap();
            symlink(&backing, agents.join("skills")).unwrap();
        }
        fs::write(home.join(".agents/.skill-lock.json"), "not json").unwrap();
        let scope =
            SkillReadScope::bind(&[home.clone(), project.clone(), backing.clone()]).unwrap();
        let report = load_ownership_inputs_scoped(&scope, &home, std::slice::from_ref(&project));
        let mut deployment = candidate("alpha", "shared", &backing.join("alpha").to_string_lossy());
        deployment.resolved_path = Some(std::fs::canonicalize(backing.join("alpha")).unwrap());
        deployment.scope = "project".to_string();
        deployment.project_path = Some(project);
        assert_eq!(
            classify_lifecycle_owner(
                &deployment,
                &report,
                SkillDestination::Universal,
                "alpha",
                &Default::default()
            )
            .0,
            LifecycleOwnerKind::Unknown
        );
    }

    #[cfg(unix)]
    #[test]
    fn scoped_ownership_loads_a_ten_thousand_skill_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let agents = home.join(".agents");
        fs::create_dir_all(agents.join("skills")).unwrap();
        let mut lock = lock_file::empty_lock_file();
        for index in 0..10_000 {
            lock.skills.insert(
                format!("skill-{index:05}"),
                lock_file::InstalledSkillEntry {
                    source: "example/skills".into(),
                    source_type: "github".into(),
                    source_url: "https://github.com/example/skills".into(),
                    skill_path: Some(format!("skills/skill-{index:05}")),
                    skill_folder_hash: "a".repeat(64),
                    installed_at: "2026-09-09T00:00:00Z".into(),
                    updated_at: "2026-09-09T00:00:00Z".into(),
                },
            );
        }
        let bytes = serde_json::to_vec_pretty(&lock).unwrap();
        assert!(bytes.len() > 1024 * 1024);
        assert!(bytes.len() < SCOPED_OWNERSHIP_FILE_BYTE_LIMIT);
        fs::write(agents.join(".skill-lock.json"), bytes).unwrap();
        let report = scoped_reader(&home, std::slice::from_ref(&home));
        assert!(report.failures().is_empty());
        let OwnershipInput::Loaded(loaded) = &report.scopes[0].skills_sh else {
            panic!("the complete reference ledger must load");
        };
        assert_eq!(loaded.skills.len(), 10_000);
        assert_eq!(loaded.skills["skill-09999"].source, "example/skills");
    }

    #[cfg(unix)]
    #[test]
    fn scoped_ownership_enforces_the_document_byte_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".agents/skills")).unwrap();
        fs::write(
            home.join(".agents/.skill-lock.json"),
            vec![b'x'; SCOPED_OWNERSHIP_FILE_BYTE_LIMIT + 1],
        )
        .unwrap();
        let report = scoped_reader(&home, std::slice::from_ref(&home));
        assert!(matches!(
            report.scopes[0].skills_sh,
            OwnershipInput::Failed(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn scoped_ownership_retains_ledger_only_data_when_the_skills_root_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let agents = home.join(".agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("agents.lock"),
            "[skills.alpha]\nsource = \"o/r\"\nresolved_commit = \"commit\"\n",
        )
        .unwrap();
        fs::write(agents.join("agents.toml"), "[[skills]]\nname = \"alpha\"\n").unwrap();
        let report = scoped_reader(&home, std::slice::from_ref(&home));
        assert!(matches!(
            report.scopes[0].skills_root,
            OwnershipInput::Absent
        ));
        let ledger = report.scopes[0].as_ledger().unwrap();
        assert_eq!(ledger.dotagents[0].name, "alpha");
    }

    #[cfg(unix)]
    #[test]
    fn scoped_ownership_rejects_a_regular_file_as_the_skills_root() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(home.join(".agents/skills"), "not a directory").unwrap();
        let report = scoped_reader(&home, std::slice::from_ref(&home));
        assert!(matches!(
            report.scopes[0].skills_root,
            OwnershipInput::Failed(_)
        ));
        assert_eq!(
            report.failures()[0].kind,
            OwnershipReadIssueKind::SkillsRoot
        );
    }

    #[cfg(unix)]
    #[test]
    fn scoped_ownership_uses_only_each_explicit_home_without_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let homes: Vec<_> = ["alpha", "beta"].map(|name| tmp.path().join(name)).to_vec();
        for home in &homes {
            let agents = home.join(".agents");
            fs::create_dir_all(agents.join("skills")).unwrap();
            fs::write(
                agents.join(".skill-lock.json"),
                format!(r#"{{"version":3,"skills":{{"{0}":{{"source":"{0}/repo","sourceType":"github","sourceUrl":"https://example.test","skillFolderHash":"h","installedAt":"t","updatedAt":"t"}}}}}}"#, home.file_name().unwrap().to_string_lossy()),
            )
            .unwrap();
        }
        for home in &homes {
            let lock_path = home.join(".agents/.skill-lock.json");
            let before = fs::read(&lock_path).unwrap();
            let mut entries_before: Vec<_> = fs::read_dir(home.join(".agents"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            entries_before.sort();
            let report = scoped_reader(home, std::slice::from_ref(home));
            assert_eq!(report.global_lock().skills.len(), 1);
            assert_eq!(fs::read(lock_path).unwrap(), before);
            let mut entries_after: Vec<_> = fs::read_dir(home.join(".agents"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            entries_after.sort();
            assert_eq!(entries_after, entries_before);
        }
    }
}
