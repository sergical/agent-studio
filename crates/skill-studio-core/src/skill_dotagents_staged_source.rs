use super::*;
use crate::{
    skill_backup_source::BackupSourceRoot,
    skill_dotagents_ledger::{DotagentsDetachIntent, DotagentsReinstallRequest},
};
use sha2::{Digest, Sha256};

const RECORD: &str = "staged-source.json";
const MAX_RECORD: usize = 24 * 1024 * 1024;
const COPY_CONTRACT: &str = "dotagents-3.0.1-default-node-copy";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DotagentsStagedSourceReference {
    version: u32,
    cache: ManagedSourceReference,
    digest: String,
}

impl DotagentsStagedSourceReference {
    pub fn validate(&self) -> io::Result<()> {
        self.validate_version(1)
    }

    pub(crate) fn validate_for_unfork_version(&self, version: u32) -> io::Result<()> {
        self.validate_version(version)
    }

    fn validate_version(&self, version: u32) -> io::Result<()> {
        self.cache.validate()?;
        if !matches!(version, 1 | 2)
            || self.version != version
            || !self.digest.strip_prefix("sha256:").is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        {
            return Err(io::Error::other("Invalid staged source reference"));
        }
        Ok(())
    }

    pub fn cache(&self) -> &ManagedSourceReference {
        &self.cache
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DotagentsStagedSourceReceipt {
    version: u32,
    cache: ManagedSourceReference,
    copy_contract: String,
    original_rows: String,
    selected_rows: String,
    agents_relative: PathBuf,
    installed_identity: String,
}

impl DotagentsStagedSourceReceipt {
    pub fn selected_rows(&self) -> io::Result<DotagentsDetachIntent> {
        DotagentsDetachIntent::from_record_json(&self.selected_rows).map_err(io::Error::other)
    }
    pub fn installed_identity(&self) -> &str {
        &self.installed_identity
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DotagentsStagedSourceReceiptV2 {
    version: u32,
    cache: ManagedSourceReference,
    copy_contract: String,
    request: DotagentsReinstallRequest,
    selected_rows: String,
    agents_relative: PathBuf,
    installed_identity: String,
}

impl DotagentsStagedSourceReceiptV2 {
    pub fn request(&self) -> &DotagentsReinstallRequest {
        &self.request
    }

    pub fn selected_rows(&self) -> io::Result<DotagentsDetachIntent> {
        DotagentsDetachIntent::from_record_json(&self.selected_rows).map_err(io::Error::other)
    }

    pub fn installed_identity(&self) -> &str {
        &self.installed_identity
    }
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
    fn prepare_dotagents_stage_for_request(
        &self,
        request: &DotagentsReinstallRequest,
        agents_relative: &Path,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<(DotagentsDetachIntent, String)> {
        self.revalidate(limits, cancellation)?;
        if agents_relative.components().count() > limits.max_depth {
            return Err(io::Error::other("Staged provider path exceeds depth limit"));
        }
        let stage = self.location.path().join("stage");
        let stage_directory = self.location.directory.open_dir_nofollow("stage")?;
        let root = BackupSourceRoot::bind(&stage)?;
        if !same_directory(
            &stage_directory.dir_metadata()?,
            &root.directory()?.dir_metadata()?,
        ) {
            return Err(io::Error::other("Staging root changed before observation"));
        }
        let agents = root.select_relative(agents_relative)?;
        if !agents.directory.symlink_metadata(&agents.name)?.is_dir() {
            return Err(io::Error::other("Staged provider root must be a directory"));
        }
        let scope = crate::skill_scope::SkillReadScope::bind(std::slice::from_ref(&stage))
            .map_err(io::Error::other)?;
        let observed = request
            .observe_reinstalled(&scope, &agents.original_path, cancellation)
            .map_err(io::Error::other)?;
        let selected = observed
            .selected_rows(cancellation)
            .map_err(io::Error::other)?;
        request
            .validate_reinstalled_rows(&selected)
            .map_err(io::Error::other)?;
        let installed =
            root.select_relative(&agents_relative.join("skills").join(selected.name()))?;
        let relative_source = request.cache_relative_source();
        let report =
            self.verify_dotagents_copy(&relative_source, &installed, limits, cancellation)?;
        let synced = sync_entry(&installed.directory, &installed.name, limits, cancellation)?;
        if synced.tree_identity != report.tree_identity {
            return Err(io::Error::other("Staged installation changed during sync"));
        }
        for name in ["agents.lock", "agents.toml"] {
            let document = root.select_relative(&agents_relative.join(name))?;
            sync_entry(&document.directory, &document.name, limits, cancellation)?;
            document.revalidate()?;
        }
        let mut parents = vec![stage_directory.try_clone()?];
        for part in agents_relative.components() {
            let directory = parents
                .last()
                .expect("stage directory")
                .open_dir_nofollow(part.as_os_str())?;
            parents.push(directory);
        }
        for directory in parents.iter().rev() {
            if cancellation.is_cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Stage sync cancelled",
                ));
            }
            sync_directory(directory)?;
        }
        observed
            .revalidate(cancellation)
            .map_err(io::Error::other)?;
        installed.revalidate()?;
        agents.revalidate()?;
        if !same_directory(
            &stage_directory.dir_metadata()?,
            &self.location.directory.symlink_metadata("stage")?,
        ) {
            return Err(io::Error::other("Staging root changed during observation"));
        }
        self.revalidate(limits, cancellation)?;
        Ok((selected, report.tree_identity))
    }

    fn prepare_dotagents_stage(
        &self,
        original: &DotagentsDetachIntent,
        agents_relative: &Path,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<DotagentsStagedSourceReceipt> {
        let request = DotagentsReinstallRequest::from_detach(original).map_err(io::Error::other)?;
        let (selected, installed_identity) = self.prepare_dotagents_stage_for_request(
            &request,
            agents_relative,
            limits,
            cancellation,
        )?;
        Ok(DotagentsStagedSourceReceipt {
            version: 1,
            cache: self.reference.clone(),
            copy_contract: COPY_CONTRACT.into(),
            original_rows: original.to_record_json().map_err(io::Error::other)?,
            selected_rows: selected.to_record_json().map_err(io::Error::other)?,
            agents_relative: agents_relative.to_path_buf(),
            installed_identity,
        })
    }

    fn prepare_dotagents_stage_v2(
        &self,
        request: &DotagentsReinstallRequest,
        agents_relative: &Path,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<DotagentsStagedSourceReceiptV2> {
        let (selected, installed_identity) = self.prepare_dotagents_stage_for_request(
            request,
            agents_relative,
            limits,
            cancellation,
        )?;
        Ok(DotagentsStagedSourceReceiptV2 {
            version: 2,
            cache: self.reference.clone(),
            copy_contract: COPY_CONTRACT.into(),
            request: request.clone(),
            selected_rows: selected.to_record_json().map_err(io::Error::other)?,
            agents_relative: agents_relative.to_path_buf(),
            installed_identity,
        })
    }

    /// Opens the fixed candidate slot; its content may be the old tree after exchange.
    pub fn open_dotagents_publication_candidate(
        &self,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<crate::skill_backup_source::BackupSource> {
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

    /// Materializes a separate candidate without changing the recorded stage tree.
    pub fn prepare_dotagents_publication_candidate(
        &self,
        reference: &DotagentsStagedSourceReference,
        original: &DotagentsDetachIntent,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<crate::skill_backup_source::BackupSource> {
        let receipt = self.read_dotagents_stage(reference, original, limits, cancellation)?;
        self.prepare_dotagents_publication_candidate_from_receipt(
            &receipt.agents_relative,
            original.name(),
            &receipt.installed_identity,
            || {
                self.read_dotagents_stage(reference, original, limits, cancellation)
                    .map(|_| ())
            },
            limits,
            cancellation,
        )
    }

    pub fn prepare_dotagents_publication_candidate_v2(
        &self,
        reference: &DotagentsStagedSourceReference,
        request: &DotagentsReinstallRequest,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<crate::skill_backup_source::BackupSource> {
        let receipt = self.read_dotagents_stage_v2(reference, request, limits, cancellation)?;
        self.prepare_dotagents_publication_candidate_from_receipt(
            &receipt.agents_relative,
            request.name(),
            &receipt.installed_identity,
            || {
                self.read_dotagents_stage_v2(reference, request, limits, cancellation)
                    .map(|_| ())
            },
            limits,
            cancellation,
        )
    }

    fn prepare_dotagents_publication_candidate_from_receipt(
        &self,
        agents_relative: &Path,
        name: &str,
        installed_identity: &str,
        verify_stage: impl Fn() -> io::Result<()>,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<crate::skill_backup_source::BackupSource> {
        match self.location.directory.create_dir("publication") {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        sync_directory(&self.location.directory)?;
        let publication = self.location.directory.open_dir_nofollow("publication")?;
        let verify = || -> io::Result<crate::skill_backup_source::BackupSource> {
            let candidate = self.open_dotagents_publication_candidate(limits, cancellation)?;
            let report = sync_entry(&candidate.directory, &candidate.name, limits, cancellation)?;
            if report.tree_identity != installed_identity {
                return Err(io::Error::other(
                    "Publication candidate differs from the staged source",
                ));
            }
            verify_stage()?;
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
        let source = stage.select_relative(&agents_relative.join("skills").join(name))?;
        let report = crate::skill_backup_copy::copy_entry(
            &source.directory,
            &source.name,
            &destination,
            OsStr::new("tree"),
            limits,
            cancellation,
        )?;
        if report.tree_identity != installed_identity {
            return Err(io::Error::other(
                "Publication copy differs from the staged source",
            ));
        }
        let synced = sync_entry(&destination, OsStr::new("tree"), limits, cancellation)?;
        if synced.tree_identity != installed_identity {
            return Err(io::Error::other(
                "Publication candidate changed during sync",
            ));
        }
        sync_directory(&destination)?;
        verify_stage()?;
        if !same_directory(
            &publication.dir_metadata()?,
            &self
                .location
                .directory
                .open_dir_nofollow("publication")?
                .dir_metadata()?,
        ) {
            return Err(io::Error::other("Publication parent changed before rename"));
        }
        if cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Publication preparation cancelled",
            ));
        }
        #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
        rustix::fs::renameat_with(
            &destination,
            "tree",
            &publication,
            "tree",
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::other)?;
        #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
        return Err(io::Error::other(
            "Atomic candidate preparation is unsupported on this platform",
        ));
        sync_directory(&destination)?;
        sync_directory(&publication)?;
        let candidate = verify()?;
        publication.remove_dir(&temporary)?;
        sync_directory(&publication)?;
        Ok(candidate)
    }

    pub fn record_dotagents_stage(
        &self,
        original: &DotagentsDetachIntent,
        agents_relative: &Path,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<DotagentsStagedSourceReference> {
        let receipt =
            self.prepare_dotagents_stage(original, agents_relative, limits, cancellation)?;
        let bytes = serde_json::to_vec(&receipt).map_err(io::Error::other)?;
        self.record_dotagents_stage_bytes(1, &bytes, cancellation, |reference| {
            self.read_dotagents_stage(reference, original, limits, cancellation)
                .map(|_| ())
        })
    }

    pub fn record_dotagents_stage_v2(
        &self,
        request: &DotagentsReinstallRequest,
        agents_relative: &Path,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<DotagentsStagedSourceReference> {
        let receipt =
            self.prepare_dotagents_stage_v2(request, agents_relative, limits, cancellation)?;
        let bytes = serde_json::to_vec(&receipt).map_err(io::Error::other)?;
        self.record_dotagents_stage_bytes(2, &bytes, cancellation, |reference| {
            self.read_dotagents_stage_v2(reference, request, limits, cancellation)
                .map(|_| ())
        })
    }

    fn record_dotagents_stage_bytes(
        &self,
        version: u32,
        bytes: &[u8],
        cancellation: &CancellationToken,
        verify: impl Fn(&DotagentsStagedSourceReference) -> io::Result<()>,
    ) -> io::Result<DotagentsStagedSourceReference> {
        if bytes.len() > MAX_RECORD {
            return Err(io::Error::other("Staged source receipt exceeds limit"));
        }
        let reference = DotagentsStagedSourceReference {
            version,
            cache: self.reference.clone(),
            digest: digest(bytes),
        };
        match self.location.directory.symlink_metadata(RECORD) {
            Ok(_) => {
                verify(&reference)?;
                return Ok(reference);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let temporary = format!(
            ".staged-source-{}-{}",
            std::process::id(),
            SEAL_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let mut file = self
            .location
            .directory
            .open_with(&temporary, OpenOptions::new().write(true).create_new(true))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        self.location.revalidate()?;
        if cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Stage recording cancelled",
            ));
        }
        #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
        rustix::fs::renameat_with(
            &self.location.directory,
            temporary.as_str(),
            &self.location.directory,
            RECORD,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::other)?;
        #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
        return Err(io::Error::other(
            "Atomic stage recording is unsupported on this platform",
        ));
        sync_directory(&self.location.directory)?;
        verify(&reference)?;
        Ok(reference)
    }

    pub fn read_dotagents_stage(
        &self,
        reference: &DotagentsStagedSourceReference,
        original: &DotagentsDetachIntent,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<DotagentsStagedSourceReceipt> {
        let bytes = self.read_dotagents_stage_bytes(reference, 1, cancellation)?;
        let saved: DotagentsStagedSourceReceipt =
            serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if saved.version != 1
            || saved.cache != self.reference
            || saved.copy_contract != COPY_CONTRACT
            || saved.original_rows != original.to_record_json().map_err(io::Error::other)?
        {
            return Err(io::Error::other(
                "Staged source receipt differs from original operation",
            ));
        }
        let current =
            self.prepare_dotagents_stage(original, &saved.agents_relative, limits, cancellation)?;
        if current != saved {
            return Err(io::Error::other("Staged source changed after recording"));
        }
        self.finish_dotagents_stage_read(&bytes)?;
        Ok(saved)
    }

    pub fn read_dotagents_stage_v2(
        &self,
        reference: &DotagentsStagedSourceReference,
        request: &DotagentsReinstallRequest,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<DotagentsStagedSourceReceiptV2> {
        let bytes = self.read_dotagents_stage_bytes(reference, 2, cancellation)?;
        let saved: DotagentsStagedSourceReceiptV2 =
            serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if saved.version != 2
            || saved.cache != self.reference
            || saved.copy_contract != COPY_CONTRACT
            || saved.request != *request
        {
            return Err(io::Error::other(
                "Staged source receipt differs from reinstall request",
            ));
        }
        let current =
            self.prepare_dotagents_stage_v2(request, &saved.agents_relative, limits, cancellation)?;
        if current != saved {
            return Err(io::Error::other("Staged source changed after recording"));
        }
        self.finish_dotagents_stage_read(&bytes)?;
        Ok(saved)
    }

    fn read_dotagents_stage_bytes(
        &self,
        reference: &DotagentsStagedSourceReference,
        version: u32,
        cancellation: &CancellationToken,
    ) -> io::Result<Vec<u8>> {
        reference.validate_version(version)?;
        if reference.cache != self.reference {
            return Err(io::Error::other(
                "Staged source reference names another cache",
            ));
        }
        if cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Stage reading cancelled",
            ));
        }
        self.location.revalidate()?;
        let bytes = read_record_file(&self.location.directory, RECORD, MAX_RECORD)?;
        if digest(&bytes) != reference.digest {
            return Err(io::Error::other(
                "Staged source receipt differs from reference",
            ));
        }
        Ok(bytes)
    }

    fn finish_dotagents_stage_read(&self, bytes: &[u8]) -> io::Result<()> {
        if read_record_file(&self.location.directory, RECORD, MAX_RECORD)? != bytes {
            return Err(io::Error::other(
                "Staged source receipt changed during verification",
            ));
        }
        self.location.revalidate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    fn limits() -> BackupCopyLimits {
        BackupCopyLimits {
            max_bytes: 1024 * 1024,
            max_entries: 50,
            max_depth: 12,
        }
    }

    #[test]
    fn version_one_receipt_wire_bytes_and_digest_are_stable() {
        let receipt = DotagentsStagedSourceReceipt {
            version: 1,
            cache: ManagedSourceReference {
                version: 1,
                operation_id: "operation".into(),
                cache_identity:
                    "tree-v1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .into(),
            },
            copy_contract: COPY_CONTRACT.into(),
            original_rows: "original".into(),
            selected_rows: "selected".into(),
            agents_relative: PathBuf::from("home/.agents"),
            installed_identity:
                "tree-v1:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        };
        const EXPECTED: &[u8] = br#"{"version":1,"cache":{"version":1,"operation_id":"operation","cache_identity":"tree-v1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"copy_contract":"dotagents-3.0.1-default-node-copy","original_rows":"original","selected_rows":"selected","agents_relative":"home/.agents","installed_identity":"tree-v1:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}"#;
        let bytes = serde_json::to_vec(&receipt).unwrap();
        assert_eq!(bytes, EXPECTED);
        assert_eq!(
            digest(&bytes),
            "sha256:41fa4c9ab9b48f0a18fd1abd48adf9f50a8b771c38bbee2f93e886c18c7058dc",
        );
        assert_eq!(
            serde_json::from_slice::<DotagentsStagedSourceReceipt>(EXPECTED).unwrap(),
            receipt,
        );
    }

    #[test]
    fn stage_receipt_round_trip_retry_and_drift_refusal() {
        for change in [
            "none",
            "installed",
            "bookkeeping",
            "receipt",
            "stage-link",
            "cache",
            "original",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = BackupStateRoot::bind(temp.path()).unwrap();
            let reserved = root.reserve_managed_source("operation").unwrap();
            let cache = reserved.cache_path().unwrap();
            let source = cache.join("owner/repo/skills/alpha");
            fs::create_dir_all(&source).unwrap();
            fs::write(source.join("SKILL.md"), b"upstream").unwrap();
            let stage = reserved.stage_path().unwrap();
            let agents = stage.join("home/.agents");
            fs::create_dir_all(agents.join("skills/alpha")).unwrap();
            fs::write(agents.join("skills/alpha/SKILL.md"), b"upstream").unwrap();
            let lock = format!("version = 1\n[skills.alpha]\nsource = 'owner/repo'\nresolved_path = 'skills/alpha'\nresolved_commit = '{}'\n", "a".repeat(40));
            let manifest =
                "version = 1\n[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\nref = 'main'\n";
            let original = DotagentsDetachIntent::from_documents("alpha", &lock, manifest).unwrap();
            let current_lock = lock.replace(&"a".repeat(40), &"b".repeat(40));
            fs::write(agents.join("agents.lock"), &current_lock).unwrap();
            fs::write(agents.join("agents.toml"), manifest).unwrap();
            let token = CancellationToken::default();
            let cache_ref = reserved.seal_cache(limits(), &token).unwrap();
            let sealed = root
                .open_managed_source(&cache_ref, limits(), &token)
                .unwrap();
            let cancelled = CancellationToken::default();
            cancelled.cancel();
            assert!(sealed
                .record_dotagents_stage(&original, Path::new("home/.agents"), limits(), &cancelled)
                .is_err());
            let record_path = cache.parent().unwrap().join(RECORD);
            assert!(!record_path.exists());
            let reference = sealed
                .record_dotagents_stage(&original, Path::new("home/.agents"), limits(), &token)
                .unwrap();
            assert_eq!(
                sealed
                    .record_dotagents_stage(&original, Path::new("home/.agents"), limits(), &token)
                    .unwrap(),
                reference
            );
            let saved = fs::read(&record_path).unwrap();
            let saved_receipt: DotagentsStagedSourceReceipt =
                serde_json::from_slice(&saved).unwrap();
            let expected_v1 = format!(
                "{{\"version\":1,\"cache\":{},\"copy_contract\":\"{}\",\"original_rows\":{},\"selected_rows\":{},\"agents_relative\":\"home/.agents\",\"installed_identity\":{}}}",
                serde_json::to_string(reference.cache()).unwrap(),
                COPY_CONTRACT,
                serde_json::to_string(&original.to_record_json().unwrap()).unwrap(),
                serde_json::to_string(&saved_receipt.selected_rows).unwrap(),
                serde_json::to_string(&saved_receipt.installed_identity).unwrap(),
            );
            assert_eq!(saved, expected_v1.as_bytes());
            assert_eq!(reference.digest, digest(expected_v1.as_bytes()));
            let encoded = serde_json::to_vec(&reference).unwrap();
            drop(sealed);
            drop(reserved);
            drop(root);
            let root = BackupStateRoot::bind(temp.path()).unwrap();
            let reference: DotagentsStagedSourceReference =
                serde_json::from_slice(&encoded).unwrap();
            let sealed = root
                .open_managed_source(reference.cache(), limits(), &token)
                .unwrap();
            let receipt = sealed
                .read_dotagents_stage(&reference, &original, limits(), &token)
                .unwrap();
            assert_eq!(
                receipt
                    .selected_rows()
                    .unwrap()
                    .fork_source()
                    .unwrap()
                    .commit(),
                "b".repeat(40)
            );
            assert!(receipt.installed_identity().starts_with("tree-v1:"));
            match change {
                "none" => {
                    let request = DotagentsReinstallRequest::from_detach(&original).unwrap();
                    assert!(sealed
                        .read_dotagents_stage_v2(&reference, &request, limits(), &token)
                        .is_err());
                    for path in ["../home/.agents", "/home/.agents", "missing"] {
                        assert!(sealed
                            .record_dotagents_stage(&original, Path::new(path), limits(), &token)
                            .is_err());
                    }
                    let mut wrong = reference.clone();
                    wrong.digest = "changed".into();
                    assert!(sealed
                        .read_dotagents_stage(&wrong, &original, limits(), &token)
                        .is_err());
                    assert!(sealed
                        .read_dotagents_stage(&reference, &original, limits(), &cancelled)
                        .is_err());
                    assert_eq!(fs::read(&record_path).unwrap(), saved);
                    continue;
                }
                "installed" => {
                    fs::write(agents.join("skills/alpha/SKILL.md"), b"later edit").unwrap()
                }
                "bookkeeping" => fs::write(
                    agents.join("agents.lock"),
                    format!("{current_lock}provider_note = 'changed'\n"),
                )
                .unwrap(),
                "receipt" => fs::write(&record_path, b"broken").unwrap(),
                "stage-link" => {
                    fs::rename(&stage, temp.path().join("old-stage")).unwrap();
                    symlink(temp.path().join("old-stage"), &stage).unwrap();
                }
                "cache" => fs::write(source.join("SKILL.md"), b"cache drift").unwrap(),
                "original" => {
                    let other = DotagentsDetachIntent::from_documents(
                        "alpha",
                        &lock.replace("owner/repo", "other/repo"),
                        &manifest.replace("owner/repo", "other/repo"),
                    )
                    .unwrap();
                    assert!(sealed
                        .read_dotagents_stage(&reference, &other, limits(), &token)
                        .is_err());
                    assert_eq!(fs::read(&record_path).unwrap(), saved);
                    continue;
                }
                _ => unreachable!(),
            }
            assert!(
                sealed
                    .read_dotagents_stage(&reference, &original, limits(), &token)
                    .is_err(),
                "{change}"
            );
            assert!(
                sealed
                    .record_dotagents_stage(&original, Path::new("home/.agents"), limits(), &token)
                    .is_err(),
                "{change}"
            );
            assert_eq!(
                fs::read(&record_path).unwrap(),
                if change == "receipt" {
                    b"broken".to_vec()
                } else {
                    saved
                },
                "{change}"
            );
        }
    }

    #[test]
    fn stage_receipt_v2_round_trip_and_request_rows_cache_drift_refusal() {
        use crate::skill_fork_registry::{ForkRecord, OriginTool};

        for change in ["none", "request", "rows", "cache"] {
            let temp = tempfile::tempdir().unwrap();
            let root = BackupStateRoot::bind(temp.path()).unwrap();
            let reserved = root.reserve_managed_source("operation-v2").unwrap();
            let cache = reserved.cache_path().unwrap();
            let source = cache.join("github.com/owner/repo/skills/alpha");
            fs::create_dir_all(&source).unwrap();
            fs::write(source.join("SKILL.md"), b"upstream v2").unwrap();
            let stage = reserved.stage_path().unwrap();
            let agents = stage.join("home/.agents");
            fs::create_dir_all(agents.join("skills/alpha")).unwrap();
            fs::write(agents.join("skills/alpha/SKILL.md"), b"upstream v2").unwrap();
            let record = ForkRecord {
                deployment_id: String::new(),
                skill_dir: PathBuf::new(),
                forked_at: String::new(),
                origin_tool: OriginTool::Dotagents,
                origin_source: "git:https://github.com/owner/repo.git".into(),
                repo: "owner/repo".into(),
                path: "skills/alpha".into(),
                declared_ref: Some("main".into()),
                base_commit: "a".repeat(40),
            };
            let request = DotagentsReinstallRequest::from_fork_record(&record, "alpha").unwrap();
            let lock = format!(
                "version = 1\n[skills.alpha]\nsource = '{}'\nresolved_path = 'skills/alpha'\nresolved_commit = '{}'\n",
                record.origin_source,
                "b".repeat(40),
            );
            let manifest = format!(
                "version = 1\n[[skills]]\nname = 'alpha'\nsource = '{}'\nref = 'main'\n",
                record.origin_source,
            );
            fs::write(agents.join("agents.lock"), &lock).unwrap();
            fs::write(agents.join("agents.toml"), &manifest).unwrap();
            let token = CancellationToken::default();
            let cache_reference = reserved.seal_cache(limits(), &token).unwrap();
            let sealed = root
                .open_managed_source(&cache_reference, limits(), &token)
                .unwrap();
            let reference = sealed
                .record_dotagents_stage_v2(&request, Path::new("home/.agents"), limits(), &token)
                .unwrap();
            let encoded = serde_json::to_vec(&reference).unwrap();
            drop(sealed);
            drop(reserved);
            drop(root);
            let root = BackupStateRoot::bind(temp.path()).unwrap();
            let reference: DotagentsStagedSourceReference =
                serde_json::from_slice(&encoded).unwrap();
            assert!(reference.validate().is_err());
            let sealed = root
                .open_managed_source(reference.cache(), limits(), &token)
                .unwrap();
            let receipt = sealed
                .read_dotagents_stage_v2(&reference, &request, limits(), &token)
                .unwrap();
            assert_eq!(receipt.request(), &request);
            assert_eq!(
                receipt
                    .selected_rows()
                    .unwrap()
                    .fork_source()
                    .unwrap()
                    .commit(),
                "b".repeat(40),
            );
            assert!(receipt.installed_identity().starts_with("tree-v1:"));
            let original = DotagentsDetachIntent::from_documents(
                "alpha",
                &lock.replace(&"b".repeat(40), &"a".repeat(40)),
                &manifest,
            )
            .unwrap();
            assert!(sealed
                .read_dotagents_stage(&reference, &original, limits(), &token)
                .is_err());

            match change {
                "none" => {
                    let candidate = sealed
                        .prepare_dotagents_publication_candidate_v2(
                            &reference,
                            &request,
                            limits(),
                            &token,
                        )
                        .unwrap();
                    assert_eq!(
                        fs::read(candidate.original_path.join("SKILL.md")).unwrap(),
                        b"upstream v2",
                    );
                }
                "request" => {
                    let mut changed = record.clone();
                    changed.origin_source = "other/repo".into();
                    changed.repo = "other/repo".into();
                    let changed =
                        DotagentsReinstallRequest::from_fork_record(&changed, "alpha").unwrap();
                    assert!(sealed
                        .read_dotagents_stage_v2(&reference, &changed, limits(), &token)
                        .is_err());
                }
                "rows" => {
                    fs::write(
                        agents.join("agents.lock"),
                        lock.replace(&"b".repeat(40), &"c".repeat(40)),
                    )
                    .unwrap();
                    assert!(sealed
                        .read_dotagents_stage_v2(&reference, &request, limits(), &token)
                        .is_err());
                }
                "cache" => {
                    fs::write(source.join("SKILL.md"), b"cache drift").unwrap();
                    assert!(sealed
                        .read_dotagents_stage_v2(&reference, &request, limits(), &token)
                        .is_err());
                }
                _ => unreachable!(),
            }
        }
    }
}
