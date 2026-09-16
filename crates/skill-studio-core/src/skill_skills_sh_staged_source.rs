use super::*;
use crate::{
    skill_backup_source::{BackupSource, BackupSourceRoot},
    skill_fork_registry::{ForkRecord, OriginTool},
    skill_skills_sh_lock_transition::SkillsShLockTransition,
};
use sha2::{Digest, Sha256};

const RECORD: &str = "skills-sh-staged-source.json";
const SOURCE_RECORD: &str = "skills-sh-source.json";
const MAX_RECORD: usize = 64 * 1024;
const COPY_CONTRACT: &str = "skills-1.5.25-global-universal-copy";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsShReinstallRequest {
    name: String,
    repo: String,
    path: String,
    declared_ref: Option<String>,
    resolved_commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifiedSource {
    repo: String,
    resolved_commit: String,
    source_relative: PathBuf,
    source_identity: String,
    projection_identity: String,
}

impl SkillsShReinstallRequest {
    pub fn from_fork_record(
        record: &ForkRecord,
        name: &str,
        resolved_commit: &str,
    ) -> Result<Self, String> {
        if record.origin_tool != OriginTool::SkillsSh {
            return Err("Unfork provider requires a skills.sh origin".into());
        }
        Self::new(
            name,
            &record.origin_source,
            &record.repo,
            &record.path,
            record.declared_ref.as_deref(),
            resolved_commit,
        )
    }

    pub fn new(
        name: &str,
        source: &str,
        repo: &str,
        path: &str,
        declared_ref: Option<&str>,
        resolved_commit: &str,
    ) -> Result<Self, String> {
        if name.is_empty()
            || name.starts_with('-')
            || name.contains(['/', '\\'])
            || matches!(name, "." | "..")
            || name.chars().any(char::is_control)
        {
            return Err("Unfork requires a safe exact skill name".into());
        }
        if crate::skill_dotagents_ledger::github_repo_from_source(source).as_deref() != Some(repo)
            || crate::skill_dotagents_ledger::github_repo_from_source(repo).as_deref() != Some(repo)
            || source != repo
        {
            return Err("Unfork provider record has an inconsistent origin".into());
        }
        let path = path.strip_suffix("/SKILL.md").unwrap_or(path);
        let path = if matches!(path, "." | "SKILL.md") {
            ""
        } else {
            path
        };
        if path.contains('\\')
            || path.chars().any(char::is_control)
            || (!path.is_empty() && path.split('/').any(|part| matches!(part, "" | "." | "..")))
        {
            return Err("Unfork source path must be repository-relative".into());
        }
        if declared_ref.is_some_and(|value| {
            value.is_empty()
                || value.starts_with('-')
                || value.contains('#')
                || value
                    .chars()
                    .any(|character| character.is_control() || character.is_whitespace())
        }) {
            return Err("Unfork provider record has an unsafe declared ref".into());
        }
        if !matches!(resolved_commit.len(), 40 | 64)
            || !resolved_commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("Unfork requires an exact fetched commit".into());
        }
        Ok(Self {
            name: name.into(),
            repo: repo.into(),
            path: path.into(),
            declared_ref: declared_ref.map(str::to_owned),
            resolved_commit: resolved_commit.into(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
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
    pub fn resolved_commit(&self) -> &str {
        &self.resolved_commit
    }
    pub fn source_argument(&self) -> String {
        let mut source = self.repo.clone();
        if !self.path.is_empty() {
            source.push('/');
            source.push_str(&self.path);
        }
        if let Some(reference) = &self.declared_ref {
            source.push('#');
            source.push_str(reference);
        }
        source
    }
    fn cache_relative_source(&self) -> PathBuf {
        let mut path = PathBuf::from(&self.repo);
        if !self.path.is_empty() {
            path.push(&self.path);
        }
        path
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsShStagedSourceReference {
    version: u32,
    cache: ManagedSourceReference,
    digest: String,
}

impl SkillsShStagedSourceReference {
    pub fn cache(&self) -> &ManagedSourceReference {
        &self.cache
    }
    pub fn validate(&self) -> io::Result<()> {
        self.cache.validate()?;
        if self.version != 1 || !self.digest.strip_prefix("sha256:").is_some_and(valid_hash) {
            return Err(io::Error::other(
                "Invalid skills.sh staged source reference",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsShStagedSourceReceipt {
    version: u32,
    cache: ManagedSourceReference,
    copy_contract: String,
    request: SkillsShReinstallRequest,
    selected_row: SkillsShLockTransition,
    agents_relative: PathBuf,
    installed_identity: String,
}

impl SkillsShStagedSourceReceipt {
    pub fn request(&self) -> &SkillsShReinstallRequest {
        &self.request
    }
    pub fn selected_row(&self) -> &SkillsShLockTransition {
        &self.selected_row
    }
    pub fn installed_identity(&self) -> &str {
        &self.installed_identity
    }
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn digest(bytes: &[u8]) -> String {
    format!(
        "sha256:{}",
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

impl SealedManagedSource<'_> {
    /// Opens the fixed publication candidate. The directory is prepared from
    /// the sealed stage and is never supplied by a deserialized event.
    pub fn open_skills_sh_publication_candidate(
        &self,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<BackupSource> {
        self.revalidate(limits, cancellation)?;
        let directory = self.location.directory.open_dir_nofollow("publication")?;
        let root = BackupSourceRoot::bind(&self.location.path().join("publication"))?;
        if !same_directory(
            &directory.dir_metadata()?,
            &root.directory()?.dir_metadata()?,
        ) {
            return Err(io::Error::other("Publication directory changed"));
        }
        let candidate = root.select(OsStr::new("tree"))?;
        if !candidate
            .directory
            .symlink_metadata(&candidate.name)?
            .is_dir()
        {
            return Err(io::Error::other("Publication candidate is not a directory"));
        }
        candidate.revalidate()?;
        Ok(candidate)
    }

    /// Materializes a durable tree-exchange candidate after reopening and
    /// verifying the exact staged receipt.
    pub fn prepare_skills_sh_publication_candidate(
        &self,
        reference: &SkillsShStagedSourceReference,
        request: &SkillsShReinstallRequest,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<BackupSource> {
        let receipt = self.read_skills_sh_stage(reference, request, limits, cancellation)?;
        match self.location.directory.create_dir("publication") {
            Ok(()) => sync_directory(&self.location.directory)?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let publication = self.location.directory.open_dir_nofollow("publication")?;
        let verify = || -> io::Result<BackupSource> {
            let candidate = self.open_skills_sh_publication_candidate(limits, cancellation)?;
            let report = sync_entry(&candidate.directory, &candidate.name, limits, cancellation)?;
            if report.tree_identity != receipt.installed_identity {
                return Err(io::Error::other(
                    "Publication candidate differs from staged skills.sh source",
                ));
            }
            self.read_skills_sh_stage(reference, request, limits, cancellation)?;
            candidate.revalidate()?;
            sync_directory(&publication)?;
            Ok(candidate)
        };
        match publication.symlink_metadata("tree") {
            Ok(_) => return verify(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let temporary = format!(
            ".candidate-{}-{}",
            std::process::id(),
            SEAL_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        publication.create_dir(&temporary)?;
        let destination = publication.open_dir_nofollow(&temporary)?;
        let stage = BackupSourceRoot::bind(&self.location.path().join("stage"))?;
        let source =
            stage.select_relative(&receipt.agents_relative.join("skills").join(request.name()))?;
        let report = crate::skill_backup_copy::copy_entry(
            &source.directory,
            &source.name,
            &destination,
            OsStr::new("tree"),
            limits,
            cancellation,
        )?;
        if report.tree_identity != receipt.installed_identity {
            return Err(io::Error::other(
                "Publication copy differs from staged skills.sh source",
            ));
        }
        sync_entry(&destination, OsStr::new("tree"), limits, cancellation)?;
        sync_directory(&destination)?;
        self.read_skills_sh_stage(reference, request, limits, cancellation)?;
        rustix::fs::renameat_with(
            &destination,
            "tree",
            &publication,
            "tree",
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::other)?;
        sync_directory(&destination)?;
        sync_directory(&publication)?;
        let candidate = verify()?;
        publication.remove_dir(&temporary)?;
        sync_directory(&publication)?;
        Ok(candidate)
    }

    fn prepare_skills_sh_stage(
        &self,
        request: &SkillsShReinstallRequest,
        agents_relative: &Path,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<SkillsShStagedSourceReceipt> {
        self.revalidate(limits, cancellation)?;
        if agents_relative.is_absolute() || agents_relative.components().count() > limits.max_depth
        {
            return Err(io::Error::other("Invalid staged skills.sh agents path"));
        }
        let stage = self.location.path().join("stage");
        let root = BackupSourceRoot::bind(&stage)?;
        let agents = root.select_relative(agents_relative)?;
        if !agents.directory.symlink_metadata(&agents.name)?.is_dir() {
            return Err(io::Error::other(
                "Staged skills.sh agents root is not a directory",
            ));
        }
        let agents_directory = agents.directory.open_dir_nofollow(&agents.name)?;
        let lock_bytes = read_record_file(&agents_directory, ".skill-lock.json", MAX_RECORD)?;
        SkillsShLockTransition::require_only_selected(&lock_bytes, request.name())
            .map_err(io::Error::other)?;
        let selected = SkillsShLockTransition::read(
            &lock_bytes,
            request.name(),
            request.repo(),
            request.path(),
            request.declared_ref(),
        )
        .map_err(io::Error::other)?;
        let installed =
            root.select_relative(&agents_relative.join("skills").join(request.name()))?;
        if !installed
            .directory
            .symlink_metadata(&installed.name)?
            .is_dir()
        {
            return Err(io::Error::other(
                "Staged skills.sh installation is not a directory",
            ));
        }
        let cache = BackupSourceRoot::bind(&self.location.path().join("cache"))?;
        let source_record: VerifiedSource = serde_json::from_slice(&read_record_file(
            &self.location.directory,
            SOURCE_RECORD,
            4096,
        )?)
        .map_err(io::Error::other)?;
        if source_record.repo != request.repo
            || source_record.resolved_commit != request.resolved_commit
            || source_record.source_relative != request.cache_relative_source()
        {
            return Err(io::Error::other(
                "Verified skills.sh source differs from the request",
            ));
        }
        let source = cache.select_relative(&request.cache_relative_source())?;
        let expected = inspect_entry(&source.directory, &source.name, limits, cancellation)?;
        if expected.tree_identity != source_record.source_identity {
            return Err(io::Error::other(
                "Verified skills.sh source changed after admission",
            ));
        }
        if crate::skill_skills_sh_copy::source_identity(
            &cache.directory()?,
            &request.cache_relative_source(),
            limits,
            cancellation,
        )? != source_record.projection_identity
        {
            return Err(io::Error::other(
                "Skills.sh copied source changed after admission",
            ));
        }
        let actual = inspect_entry(&installed.directory, &installed.name, limits, cancellation)?;
        crate::skill_skills_sh_copy::verify_copy(
            &cache.directory()?,
            &request.cache_relative_source(),
            &installed.directory.open_dir_nofollow(&installed.name)?,
            limits,
            cancellation,
        )?;
        sync_entry(&installed.directory, &installed.name, limits, cancellation)?;
        sync_entry(
            &agents_directory,
            OsStr::new(".skill-lock.json"),
            limits,
            cancellation,
        )?;
        self.revalidate(limits, cancellation)?;
        Ok(SkillsShStagedSourceReceipt {
            version: 1,
            cache: self.reference.clone(),
            copy_contract: COPY_CONTRACT.into(),
            request: request.clone(),
            selected_row: selected,
            agents_relative: agents_relative.into(),
            installed_identity: actual.tree_identity,
        })
    }

    pub fn record_skills_sh_stage(
        &self,
        request: &SkillsShReinstallRequest,
        agents_relative: &Path,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<SkillsShStagedSourceReference> {
        let receipt =
            self.prepare_skills_sh_stage(request, agents_relative, limits, cancellation)?;
        let bytes = serde_json::to_vec(&receipt).map_err(io::Error::other)?;
        if bytes.len() > MAX_RECORD {
            return Err(io::Error::other(
                "Skills.sh staged source receipt exceeds limit",
            ));
        }
        let reference = SkillsShStagedSourceReference {
            version: 1,
            cache: self.reference.clone(),
            digest: digest(&bytes),
        };
        match self.location.directory.symlink_metadata(RECORD) {
            Ok(_) => {
                self.read_skills_sh_stage(&reference, request, limits, cancellation)?;
                return Ok(reference);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let temporary = format!(
            ".skills-sh-staged-source-{}-{}",
            std::process::id(),
            SEAL_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let mut file = self
            .location
            .directory
            .open_with(&temporary, OpenOptions::new().write(true).create_new(true))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        rustix::fs::renameat_with(
            &self.location.directory,
            temporary.as_str(),
            &self.location.directory,
            RECORD,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::other)?;
        sync_directory(&self.location.directory)?;
        self.read_skills_sh_stage(&reference, request, limits, cancellation)?;
        Ok(reference)
    }

    pub fn read_skills_sh_stage(
        &self,
        reference: &SkillsShStagedSourceReference,
        request: &SkillsShReinstallRequest,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<SkillsShStagedSourceReceipt> {
        reference.validate()?;
        if reference.cache != self.reference {
            return Err(io::Error::other(
                "Skills.sh staged source names another cache",
            ));
        }
        self.revalidate(limits, cancellation)?;
        let bytes = read_record_file(&self.location.directory, RECORD, MAX_RECORD)?;
        if digest(&bytes) != reference.digest {
            return Err(io::Error::other(
                "Skills.sh staged source receipt differs from reference",
            ));
        }
        let saved: SkillsShStagedSourceReceipt =
            serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if saved.version != 1
            || saved.cache != self.reference
            || saved.copy_contract != COPY_CONTRACT
            || saved.request != *request
        {
            return Err(io::Error::other(
                "Skills.sh staged source receipt differs from request",
            ));
        }
        let current =
            self.prepare_skills_sh_stage(request, &saved.agents_relative, limits, cancellation)?;
        if current != saved {
            return Err(io::Error::other(
                "Skills.sh staged source changed after recording",
            ));
        }
        if read_record_file(&self.location.directory, RECORD, MAX_RECORD)? != bytes {
            return Err(io::Error::other(
                "Skills.sh staged source receipt changed during verification",
            ));
        }
        Ok(saved)
    }
}

impl ReservedManagedSource<'_> {
    /// Persists the already verified fetched source before the provider may write stage or cache.
    pub fn admit_skills_sh_source(
        &self,
        request: &SkillsShReinstallRequest,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<()> {
        self.revalidate()?;
        let cache = BackupSourceRoot::bind(&self.location.path().join("cache"))?;
        let source_relative = request.cache_relative_source();
        let source = cache.select_relative(&source_relative)?;
        let report = inspect_entry(&source.directory, &source.name, limits, cancellation)?;
        let receipt = VerifiedSource {
            repo: request.repo.clone(),
            resolved_commit: request.resolved_commit.clone(),
            source_relative: source_relative.clone(),
            source_identity: report.tree_identity,
            projection_identity: crate::skill_skills_sh_copy::source_identity(
                &cache.directory()?,
                &source_relative,
                limits,
                cancellation,
            )?,
        };
        let bytes = serde_json::to_vec(&receipt).map_err(io::Error::other)?;
        match self.location.directory.symlink_metadata(SOURCE_RECORD) {
            Ok(_) => {
                let saved: VerifiedSource = serde_json::from_slice(&read_record_file(
                    &self.location.directory,
                    SOURCE_RECORD,
                    4096,
                )?)
                .map_err(io::Error::other)?;
                if saved != receipt {
                    return Err(io::Error::other(
                        "Verified skills.sh source receipt differs from admission",
                    ));
                }
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let temporary = format!(
            ".skills-sh-source-{}-{}",
            std::process::id(),
            SEAL_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let mut file = self
            .location
            .directory
            .open_with(&temporary, OpenOptions::new().write(true).create_new(true))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        rustix::fs::renameat_with(
            &self.location.directory,
            temporary.as_str(),
            &self.location.directory,
            SOURCE_RECORD,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::other)?;
        sync_directory(&self.location.directory)?;
        self.revalidate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn limits() -> BackupCopyLimits {
        BackupCopyLimits {
            max_bytes: 1024 * 1024,
            max_entries: 50,
            max_depth: 12,
        }
    }
    fn request(path: &str, reference: Option<&str>) -> SkillsShReinstallRequest {
        SkillsShReinstallRequest::new(
            "alpha",
            "owner/repo",
            "owner/repo",
            path,
            reference,
            &"a".repeat(40),
        )
        .unwrap()
    }
    fn lock(reference: Option<&str>) -> String {
        let mut row = serde_json::json!({"source":"owner/repo","sourceType":"github","sourceUrl":"https://github.com/owner/repo.git","skillPath":"skills/alpha/SKILL.md","skillFolderHash":"b".repeat(64),"installedAt":"x","updatedAt":"y","future":"kept"});
        if let Some(value) = reference {
            row["ref"] = value.into();
        }
        serde_json::json!({"version":3,"unknown":true,"skills":{"alpha":row}}).to_string()
    }
    fn staged(
        reference: Option<&str>,
        change: Option<&str>,
    ) -> (
        tempfile::TempDir,
        BackupStateRoot,
        ManagedSourceReference,
        SkillsShReinstallRequest,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = BackupStateRoot::bind(temp.path()).unwrap();
        let reserved = root.reserve_managed_source("operation").unwrap();
        let cache = reserved.cache_path().unwrap();
        fs::create_dir_all(cache.join("owner/repo/skills/alpha")).unwrap();
        fs::write(cache.join("owner/repo/skills/alpha/SKILL.md"), b"upstream").unwrap();
        let request = request("skills/alpha", reference);
        let token = CancellationToken::default();
        reserved
            .admit_skills_sh_source(&request, limits(), &token)
            .unwrap();
        let stage = reserved.stage_path().unwrap();
        let agents = stage.join("home/.agents");
        fs::create_dir_all(agents.join("skills/alpha")).unwrap();
        let bytes: &[u8] = if change == Some("tree") {
            b"changed"
        } else {
            b"upstream"
        };
        fs::write(agents.join("skills/alpha/SKILL.md"), bytes).unwrap();
        fs::write(agents.join(".skill-lock.json"), lock(reference)).unwrap();
        let sealed = reserved.seal_cache(limits(), &token).unwrap();
        (temp, root, sealed, request)
    }
    #[test]
    fn source_admission_binds_link_targets_outside_selected_directory() {
        let temp = tempfile::tempdir().unwrap();
        let root = BackupStateRoot::bind(temp.path()).unwrap();
        let reserved = root.reserve_managed_source("linked-source").unwrap();
        let cache = reserved.cache_path().unwrap();
        fs::create_dir_all(cache.join("owner/repo/skills/alpha")).unwrap();
        fs::write(cache.join("owner/repo/shared"), b"original").unwrap();
        std::os::unix::fs::symlink(
            "../../shared",
            cache.join("owner/repo/skills/alpha/SKILL.md"),
        )
        .unwrap();
        let request = request("skills/alpha", None);
        let token = CancellationToken::default();
        reserved
            .admit_skills_sh_source(&request, limits(), &token)
            .unwrap();
        let stage = reserved.stage_path().unwrap().join("home/.agents");
        fs::create_dir_all(stage.join("skills/alpha")).unwrap();
        fs::write(stage.join(".skill-lock.json"), lock(None)).unwrap();
        fs::write(cache.join("owner/repo/shared"), b"tampered").unwrap();
        fs::write(stage.join("skills/alpha/SKILL.md"), b"tampered").unwrap();
        let reference = reserved.seal_cache(limits(), &token).unwrap();
        let sealed = root
            .open_managed_source(&reference, limits(), &token)
            .unwrap();
        let error = sealed
            .record_skills_sh_stage(&request, Path::new("home/.agents"), limits(), &token)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("copied source changed after admission"),
            "{error}"
        );
    }

    #[test]
    fn request_keeps_default_and_explicit_refs_and_normalizes_root() {
        assert_eq!(request("SKILL.md", None).source_argument(), "owner/repo");
        assert_eq!(
            request("skills/alpha/SKILL.md", Some("release")).source_argument(),
            "owner/repo/skills/alpha#release"
        );
    }
    #[test]
    fn staged_receipt_reopens_and_refuses_stage_tree_cache_and_row_changes() {
        for change in ["tree", "row", "cache"] {
            let (_temp, root, reference, request) = staged(None, None);
            let token = CancellationToken::default();
            let sealed = root
                .open_managed_source(&reference, limits(), &token)
                .unwrap();
            let stage_ref = sealed
                .record_skills_sh_stage(&request, Path::new("home/.agents"), limits(), &token)
                .unwrap();
            let base = sealed.location.path();
            match change {
                "tree" => fs::write(
                    base.join("stage/home/.agents/skills/alpha/SKILL.md"),
                    b"changed",
                )
                .unwrap(),
                "row" => fs::write(
                    base.join("stage/home/.agents/.skill-lock.json"),
                    lock(Some("main")),
                )
                .unwrap(),
                "cache" => fs::write(
                    base.join("cache/owner/repo/skills/alpha/SKILL.md"),
                    b"changed",
                )
                .unwrap(),
                _ => unreachable!(),
            }
            assert!(
                sealed
                    .read_skills_sh_stage(&stage_ref, &request, limits(), &token)
                    .is_err(),
                "{change}"
            );
        }
    }

    #[test]
    fn source_drift_after_admission_refuses_the_first_receipt() {
        let (_temp, root, reference, request) = staged(None, None);
        let token = CancellationToken::default();
        let sealed = root
            .open_managed_source(&reference, limits(), &token)
            .unwrap();
        fs::write(
            sealed
                .location
                .path()
                .join("cache/owner/repo/skills/alpha/SKILL.md"),
            b"changed",
        )
        .unwrap();
        assert!(sealed
            .record_skills_sh_stage(&request, Path::new("home/.agents"), limits(), &token)
            .is_err());
    }
}
