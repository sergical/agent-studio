//! Retained document destination. Binding requires service-authorized scope;
//! this primitive does not acquire cross-process coordination or record intent.
use crate::{skill_document_write::DocumentWriteFailure, skill_scope::SkillReadScope};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, Metadata, MetadataExt, OpenOptions, OpenOptionsExt};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

pub struct SkillDocumentTarget {
    scope: Arc<SkillReadScope>,
    directory: Dir,
    path: PathBuf,
    child: &'static str,
}

/// Fixed registry destination; it cannot select an arbitrary child filename.
/// Ownership-proof transitions remain the service's responsibility.
pub struct SkillRegistryTarget {
    document: SkillDocumentTarget,
}

impl SkillRegistryTarget {
    pub fn bind(authorized_agents_directory: &Path) -> Result<Self, String> {
        Ok(Self {
            document: SkillDocumentTarget::bind_child(
                authorized_agents_directory,
                "skill-studio.json",
            )?,
        })
    }

    pub fn create(
        &self,
        lease: &mut crate::skill_coordination::FinalizedWriteLease<'_>,
        proposed: &[u8],
    ) -> Result<(), DocumentWriteFailure> {
        lease
            .validate_registry_creation(&self.document.path)
            .map_err(DocumentWriteFailure::BeforeReplace)?;
        lease.record_document(self.document.create_with(proposed, || {}))
    }

    pub fn replace(
        &self,
        lease: &mut crate::skill_coordination::FinalizedWriteLease<'_>,
        expected: &[u8],
        proposed: &[u8],
    ) -> Result<(), DocumentWriteFailure> {
        self.document.replace(lease, expected, proposed)
    }

    pub fn replace_retained(
        &self,
        lease: &mut crate::skill_coordination::FinalizedWriteLease<'_>,
        expected: &[u8],
        proposed: &[u8],
    ) -> Result<(), DocumentWriteFailure> {
        self.replace_retained_with(lease, expected, proposed, || Ok(()))
    }

    fn replace_retained_with(
        &self,
        lease: &mut crate::skill_coordination::FinalizedWriteLease<'_>,
        expected: &[u8],
        proposed: &[u8],
        after_commit: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), DocumentWriteFailure> {
        lease
            .validate_registry_replacement(&self.document.path, expected)
            .map_err(DocumentWriteFailure::BeforeReplace)?;
        let mut published = None;
        let result = self.document.replace_with_hooks(
            expected,
            proposed,
            || {},
            after_commit,
            Some(&mut published),
        );
        if let Err(DocumentWriteFailure::AfterReplace(_)) = &result {
            if let Some(receipt) = published {
                lease.record_document(Ok(receipt))?;
                return result.map(|_| ());
            }
        }
        lease.record_document(result)
    }
}

/// Fixed Codex invocation sidecar under an authorized skill directory.
pub struct CodexInvocationTarget(SkillDocumentTarget);

impl CodexInvocationTarget {
    pub fn create(
        authorized_skill_directory: &Path,
        lease: &mut crate::skill_coordination::FinalizedWriteLease<'_>,
        proposed: &[u8],
    ) -> Result<(), DocumentWriteFailure> {
        use cap_fs_ext::DirExt;
        let path = authorized_skill_directory.join("agents/openai.yaml");
        lease
            .validate_invocation_creation(&path)
            .map_err(DocumentWriteFailure::BeforeReplace)?;
        let parent_was_absent = lease.invocation_parent_was_absent(&path);
        let mut created_directory = false;
        let result = (|| -> Result<DocumentReceipt, DocumentWriteFailure> {
            let before = DocumentWriteFailure::BeforeReplace;
            if proposed.len() > MAX_DOCUMENT_BYTES {
                return Err(before("Document exceeds the 8 MiB write limit".into()));
            }
            let scope = SkillReadScope::bind(&[authorized_skill_directory.to_path_buf()])
                .map_err(|error| before(error.to_string()))?;
            let parent: Dir = scope
                .clone_bound_directory(authorized_skill_directory)
                .map_err(|error| before(error.to_string()))?
                .into();
            match parent.create_dir("agents") {
                Ok(()) => created_directory = true,
                Err(error)
                    if error.kind() == std::io::ErrorKind::AlreadyExists && !parent_was_absent => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(before("Invocation parent appeared before creation".into()));
                }
                Err(error) => return Err(before(error.to_string())),
            }
            let directory = if created_directory {
                Some(
                    parent
                        .open_dir_nofollow("agents")
                        .map_err(|error| before(error.to_string()))?,
                )
            } else {
                None
            };
            parent
                .open(".")
                .map_err(|error| before(error.to_string()))?
                .sync_all()
                .map_err(|error| before(error.to_string()))?;
            scope
                .revalidate_roots()
                .map_err(|error| before(error.to_string()))?;
            let target = Self::bind(authorized_skill_directory).map_err(before)?;
            if let Some(directory) = directory {
                let original = directory
                    .dir_metadata()
                    .map_err(|error| before(error.to_string()))?;
                let current = target
                    .0
                    .directory
                    .dir_metadata()
                    .map_err(|error| before(error.to_string()))?;
                if original.dev() != current.dev() || original.ino() != current.ino() {
                    return Err(before("Invocation parent changed during creation".into()));
                }
            }
            lease.validate_invocation_creation(&path).map_err(before)?;
            target.0.create_with(proposed, || {})
        })()
        .map_err(|error| {
            if created_directory {
                DocumentWriteFailure::AfterReplace(error.to_string())
            } else {
                error
            }
        });
        lease.record_document(result)
    }

    pub fn bind(authorized_skill_directory: &Path) -> Result<Self, String> {
        SkillDocumentTarget::bind_child(&authorized_skill_directory.join("agents"), "openai.yaml")
            .map(Self)
    }

    pub fn replace(
        &self,
        lease: &mut crate::skill_coordination::FinalizedWriteLease<'_>,
        expected: &[u8],
        proposed: &[u8],
    ) -> Result<(), DocumentWriteFailure> {
        self.0.replace(lease, expected, proposed)
    }

    pub fn remove(
        &self,
        lease: &mut crate::skill_coordination::FinalizedWriteLease<'_>,
        expected: &[u8],
    ) -> Result<(), DocumentWriteFailure> {
        lease
            .validate_document(&self.0.path)
            .map_err(|error| DocumentWriteFailure::BeforeReplace(error.to_string()))?;
        lease.record_document(self.0.remove_with(expected, || {}, || Ok(())))
    }
}

pub(crate) struct DocumentReceipt {
    scope: Arc<SkillReadScope>,
    directory: Dir,
    pub(crate) path: PathBuf,
    metadata: Option<Metadata>,
    child: &'static str,
}

impl DocumentReceipt {
    pub(crate) fn verify_absence(&self) -> Result<(), String> {
        if self.metadata.is_some() {
            return Err("Publication receipt does not record absence".into());
        }
        self.revalidate()
    }

    pub(crate) fn read(&self, limit: usize) -> Result<Vec<u8>, String> {
        self.revalidate()?;
        let Some(expected) = &self.metadata else {
            return Err("Published document is absent".into());
        };
        if expected.len() > limit as u64 {
            return Err("Published document exceeds its read limit".into());
        }
        let mut file = self
            .directory
            .open_with(
                self.child,
                OpenOptions::new()
                    .read(true)
                    .follow(FollowSymlinks::No)
                    .nonblock(true),
            )
            .map_err(|error| error.to_string())?;
        let before = file.metadata().map_err(|error| error.to_string())?;
        if !same_file(expected, &before) {
            return Err("Published document identity changed".into());
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() > limit
            || !same_file(
                expected,
                &file.metadata().map_err(|error| error.to_string())?,
            )
            || !same_file(
                expected,
                &self
                    .directory
                    .symlink_metadata(self.child)
                    .map_err(|error| error.to_string())?,
            )
        {
            return Err("Published document changed during the operation".into());
        }
        Ok(bytes)
    }

    pub(crate) fn revalidate_alias(
        &self,
        scope: &SkillReadScope,
        alias: &Path,
    ) -> Result<(), String> {
        self.revalidate()?;
        let (target, _) = scope
            .resolved_path_metadata(&self.path)
            .map_err(|error| error.to_string())?;
        let (resolved, current) = scope
            .resolved_path_metadata(alias)
            .map_err(|error| error.to_string())?;
        if resolved != target
            || !self
                .metadata
                .as_ref()
                .is_some_and(|before| same_file(before, &current))
        {
            return Err("Published document alias changed during the operation".into());
        }
        Ok(())
    }

    pub(crate) fn verify_content(&self, expected: &[u8]) -> Result<(), String> {
        if expected.len() > MAX_DOCUMENT_BYTES {
            return Err("Published document exceeds its limit".into());
        }
        self.revalidate()?;
        let target = SkillDocumentTarget {
            scope: self.scope.clone(),
            directory: self
                .directory
                .try_clone()
                .map_err(|error| error.to_string())?,
            path: self.path.clone(),
            child: self.child,
        };
        let current = target.verify(expected)?;
        if !self
            .metadata
            .as_ref()
            .is_some_and(|metadata| same_file(metadata, &current))
        {
            return Err("Published document identity changed".into());
        }
        self.revalidate()
    }

    pub(crate) fn revalidate(&self) -> Result<(), String> {
        self.scope
            .revalidate_roots()
            .map_err(|error| error.to_string())?;
        match (&self.metadata, self.directory.symlink_metadata(self.child)) {
            (Some(before), Ok(current)) if same_file(before, &current) => Ok(()),
            (None, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err("Published document changed during the operation".into()),
        }
    }
}

fn same_identity(left: &Metadata, right: &Metadata) -> bool {
    left.is_file() && right.is_file() && left.dev() == right.dev() && left.ino() == right.ino()
}

fn same_file(left: &Metadata, right: &Metadata) -> bool {
    left.is_file()
        && right.is_file()
        && left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

impl SkillDocumentTarget {
    /// Bind the authorized existing parent, retaining its handle and alias checks.
    pub fn bind(parent: &Path) -> Result<Self, String> {
        Self::bind_child(parent, "SKILL.md")
    }

    fn bind_child(parent: &Path, child: &'static str) -> Result<Self, String> {
        if !parent.is_absolute() {
            return Err("Document parent must be absolute".into());
        }
        let scope = SkillReadScope::bind(&[parent.to_path_buf()]).map_err(|e| e.to_string())?;
        let directory = scope
            .clone_bound_directory(parent)
            .map_err(|e| e.to_string())?
            .into();
        Ok(Self {
            scope: Arc::new(scope),
            directory,
            path: parent.join(child),
            child,
        })
    }

    fn verify(&self, expected: &[u8]) -> Result<Metadata, String> {
        self.scope.revalidate_roots().map_err(|e| e.to_string())?;
        let mut file = self
            .directory
            .open_with(
                self.child,
                OpenOptions::new()
                    .read(true)
                    .follow(FollowSymlinks::No)
                    .nonblock(true),
            )
            .map_err(|e| e.to_string())?;
        let before = file.metadata().map_err(|e| e.to_string())?;
        if !before.is_file() || before.len() != expected.len() as u64 {
            return Err("Target is not the expected regular document".into());
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(expected.len() as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes != expected
            || !same_file(&before, &file.metadata().map_err(|e| e.to_string())?)
            || !same_file(
                &before,
                &self
                    .directory
                    .symlink_metadata(self.child)
                    .map_err(|e| e.to_string())?,
            )
        {
            return Err("Document changed before replacement".into());
        }
        Ok(before)
    }

    /// Requires the caller's complete mutation lease and durable intent. Checks
    /// are discrete: an external writer can still race the final check/rename.
    pub fn replace(
        &self,
        lease: &mut crate::skill_coordination::FinalizedWriteLease<'_>,
        expected: &[u8],
        proposed: &[u8],
    ) -> Result<(), DocumentWriteFailure> {
        lease
            .validate_document(&self.path)
            .map_err(|error| DocumentWriteFailure::BeforeReplace(error.to_string()))?;
        let result = self.replace_with(expected, proposed, || {});
        lease.record_document(result)
    }

    fn remove_with(
        &self,
        expected: &[u8],
        before_commit: impl FnOnce(),
        after_commit: impl FnOnce() -> Result<(), String>,
    ) -> Result<DocumentReceipt, DocumentWriteFailure> {
        let mut removed = false;
        let result = (|| -> Result<DocumentReceipt, String> {
            if expected.len() > MAX_DOCUMENT_BYTES {
                return Err("Document exceeds the 8 MiB write limit".into());
            }
            let original = self.verify(expected)?;
            before_commit();
            if !same_file(&original, &self.verify(expected)?) {
                return Err("Document changed before removal".into());
            }
            self.directory
                .remove_file(self.child)
                .map_err(|error| error.to_string())?;
            removed = true;
            after_commit()?;
            self.directory
                .open(".")
                .map_err(|error| error.to_string())?
                .sync_all()
                .map_err(|error| error.to_string())?;
            let receipt = DocumentReceipt {
                scope: self.scope.clone(),
                directory: self
                    .directory
                    .try_clone()
                    .map_err(|error| error.to_string())?,
                path: self.path.clone(),
                metadata: None,
                child: self.child,
            };
            receipt.revalidate()?;
            Ok(receipt)
        })();
        result.map_err(|error| {
            if removed {
                DocumentWriteFailure::AfterReplace(error)
            } else {
                DocumentWriteFailure::BeforeReplace(error)
            }
        })
    }

    fn create_with(
        &self,
        proposed: &[u8],
        before_commit: impl FnOnce(),
    ) -> Result<DocumentReceipt, DocumentWriteFailure> {
        self.create_with_hooks(proposed, before_commit, || Ok(()))
    }

    fn create_with_hooks(
        &self,
        proposed: &[u8],
        before_commit: impl FnOnce(),
        after_commit: impl FnOnce() -> Result<(), String>,
    ) -> Result<DocumentReceipt, DocumentWriteFailure> {
        let mut published = false;
        let mut owned_temp = None;
        let result = (|| -> Result<DocumentReceipt, String> {
            if proposed.len() > MAX_DOCUMENT_BYTES {
                return Err("Document exceeds the 8 MiB write limit".into());
            }
            self.scope
                .revalidate_roots()
                .map_err(|error| error.to_string())?;
            match self.directory.symlink_metadata(self.child) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err("Registry destination is not absent".into()),
            }
            let name = format!(
                ".{}.tmp-{}-{}",
                self.child,
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            );
            let mut file = self
                .directory
                .open_with(
                    &name,
                    OpenOptions::new().write(true).create_new(true).mode(0o600),
                )
                .map_err(|error| error.to_string())?;
            owned_temp = Some((
                name.clone(),
                file.metadata().map_err(|error| error.to_string())?,
            ));
            file.write_all(proposed)
                .map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            let staged = file.metadata().map_err(|error| error.to_string())?;
            before_commit();
            self.scope
                .revalidate_roots()
                .map_err(|error| error.to_string())?;
            if !same_file(
                &staged,
                &self
                    .directory
                    .symlink_metadata(&name)
                    .map_err(|error| error.to_string())?,
            ) {
                return Err("Staged registry changed before publication".into());
            }
            self.rename_new_registry(&name)?;
            published = true;
            owned_temp = None;
            after_commit()?;
            self.directory
                .open(".")
                .map_err(|error| error.to_string())?
                .sync_all()
                .map_err(|error| error.to_string())?;
            let metadata = self.verify(proposed)?;
            if metadata.nlink() != 1 || !same_identity(&staged, &metadata) {
                return Err("Created registry identity changed".into());
            }
            Ok(DocumentReceipt {
                scope: self.scope.clone(),
                directory: self
                    .directory
                    .try_clone()
                    .map_err(|error| error.to_string())?,
                path: self.path.clone(),
                metadata: Some(metadata),
                child: self.child,
            })
        })();
        if !published {
            if let Some((name, original)) = owned_temp {
                if self
                    .directory
                    .symlink_metadata(&name)
                    .is_ok_and(|current| same_identity(&original, &current))
                {
                    let _ = self.directory.remove_file(name);
                }
            }
        }
        result.map_err(|error| {
            if published {
                DocumentWriteFailure::AfterReplace(error)
            } else {
                DocumentWriteFailure::BeforeReplace(error)
            }
        })
    }

    fn rename_new_registry(&self, staged_name: &str) -> Result<(), String> {
        #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
        {
            rustix::fs::renameat_with(
                &self.directory,
                staged_name,
                &self.directory,
                self.child,
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(|error| error.to_string())
        }
        #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
        {
            let _ = staged_name;
            Err("Atomic registry creation is unsupported on this platform".into())
        }
    }

    fn replace_with(
        &self,
        expected: &[u8],
        proposed: &[u8],
        before_commit: impl FnOnce(),
    ) -> Result<DocumentReceipt, DocumentWriteFailure> {
        self.replace_with_hooks(expected, proposed, before_commit, || Ok(()), None)
    }

    fn replace_with_hooks(
        &self,
        expected: &[u8],
        proposed: &[u8],
        before_commit: impl FnOnce(),
        after_commit: impl FnOnce() -> Result<(), String>,
        retained: Option<&mut Option<DocumentReceipt>>,
    ) -> Result<DocumentReceipt, DocumentWriteFailure> {
        let mut replaced = false;
        let mut owned_temp = None;
        let result = (|| -> Result<DocumentReceipt, String> {
            if expected.len() > MAX_DOCUMENT_BYTES || proposed.len() > MAX_DOCUMENT_BYTES {
                return Err("Document exceeds the 8 MiB write limit".into());
            }
            let original = self.verify(expected)?;
            let name = format!(
                ".{}.tmp-{}-{}",
                self.child,
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            );
            let mut file = self
                .directory
                .open_with(&name, OpenOptions::new().write(true).create_new(true))
                .map_err(|e| e.to_string())?;
            owned_temp = Some((name.clone(), file.metadata().map_err(|e| e.to_string())?));
            file.write_all(proposed).map_err(|e| e.to_string())?;
            file.set_permissions(original.permissions())
                .map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            let staged = file.metadata().map_err(|e| e.to_string())?;
            before_commit();
            if !same_file(&original, &self.verify(expected)?) {
                return Err("Document identity changed before replacement".into());
            }
            if !same_file(
                &staged,
                &self
                    .directory
                    .symlink_metadata(&name)
                    .map_err(|e| e.to_string())?,
            ) {
                return Err("Staged document changed before replacement".into());
            }
            self.directory
                .rename(&name, &self.directory, self.child)
                .map_err(|e| e.to_string())?;
            replaced = true;
            let published = self.verify(proposed)?;
            if published.nlink() != 1 || !same_identity(&staged, &published) {
                return Err("Replacement document identity changed".into());
            }
            let retained_receipt = DocumentReceipt {
                scope: self.scope.clone(),
                directory: self
                    .directory
                    .try_clone()
                    .map_err(|error| error.to_string())?,
                path: self.path.clone(),
                metadata: Some(published),
                child: self.child,
            };
            if let Some(retained) = retained {
                *retained = Some(retained_receipt);
            }
            after_commit()?;
            self.directory
                .open(".")
                .map_err(|e| e.to_string())?
                .sync_all()
                .map_err(|e| e.to_string())?;
            let published = self.verify(proposed)?;
            if published.nlink() != 1 || !same_identity(&staged, &published) {
                return Err("Replacement document identity changed".into());
            }
            Ok(DocumentReceipt {
                scope: self.scope.clone(),
                directory: self
                    .directory
                    .try_clone()
                    .map_err(|error| error.to_string())?,
                path: self.path.clone(),
                metadata: Some(published),
                child: self.child,
            })
        })();
        if !replaced {
            if let Some((name, original)) = owned_temp {
                if self
                    .directory
                    .symlink_metadata(&name)
                    .is_ok_and(|current| same_identity(&original, &current))
                {
                    let _ = self.directory.remove_file(name);
                }
            }
        }
        result.map_err(|message| {
            if replaced {
                DocumentWriteFailure::AfterReplace(message)
            } else {
                DocumentWriteFailure::BeforeReplace(message)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{symlink, PermissionsExt},
    };

    #[cfg(feature = "event-store")]
    #[test]
    fn absent_sidecar_backup_does_not_create_parent_and_creation_requires_proof() {
        use crate::skill_backup_reservation::{BackupCopyLimits, BackupStateRoot};
        use crate::skill_backup_source::BackupSourceRoot;
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        for planned in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let skill = temp.path().join("sample");
            let state = temp.path().join("state");
            fs::create_dir_all(&skill).unwrap();
            fs::create_dir_all(&state).unwrap();
            let document = skill.join("SKILL.md");
            let sidecar = skill.join("agents/openai.yaml");
            fs::write(&document, b"original").unwrap();
            let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
            let mut lease = CoordinationPlan::new_fixture(
                vec![
                    DirectoryEffect::tree(&skill, CoordinationMode::Exclusive),
                    DirectoryEffect::tree(&state, CoordinationMode::Exclusive),
                ],
                temp.path(),
                None,
            )
            .unwrap()
            .acquire()
            .unwrap()
            .finalize_write(&scope, std::slice::from_ref(&document))
            .unwrap();
            if planned {
                lease = lease.retain_absent_invocation_sidecar(&sidecar).unwrap();
            }
            let backup = BackupStateRoot::bind(&state).unwrap();
            let source = BackupSourceRoot::bind(&skill)
                .unwrap()
                .select(std::ffi::OsStr::new("SKILL.md"))
                .unwrap();
            let result = lease.backup_documents_with_absent_sidecar_prepared(
                &backup,
                "sidecar",
                vec![source],
                BackupCopyLimits {
                    max_bytes: 64,
                    max_entries: 2,
                    max_depth: 0,
                },
                Some(&sidecar),
            );
            assert!(!skill.join("agents").exists());
            if planned {
                let manifest = result.unwrap();
                let entry = manifest.entries.get(sidecar.to_str().unwrap()).unwrap();
                assert_eq!(entry.fingerprint, "absent");
                assert_eq!(entry.relative_path, "");
                CodexInvocationTarget::create(&skill, &mut lease, b"policy: {}\n").unwrap();
                assert_eq!(fs::read(&sidecar).unwrap(), b"policy: {}\n");
                lease.revalidate().unwrap();
                lease
                    .validate_invocation_output(&sidecar, Some(b"policy: {}\n"))
                    .unwrap();
                assert!(CodexInvocationTarget::create(&skill, &mut lease, b"second").is_err());
                fs::write(&sidecar, b"external").unwrap();
                assert!(lease.revalidate().is_err());
            } else {
                assert!(result.is_err());
                assert!(
                    CodexInvocationTarget::create(&skill, &mut lease, b"policy: {}\n").is_err()
                );
                assert!(!skill.join("agents").exists());
            }
        }
    }

    #[test]
    fn absent_sidecar_proof_refuses_a_linked_agents_parent() {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("sample");
        let redirected = temp.path().join("other");
        fs::create_dir(&skill).unwrap();
        fs::create_dir(&redirected).unwrap();
        symlink(&redirected, skill.join("agents")).unwrap();
        let document = skill.join("SKILL.md");
        fs::write(&document, b"original").unwrap();
        let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
        let lease = CoordinationPlan::new_fixture(
            vec![DirectoryEffect::tree(
                temp.path(),
                CoordinationMode::Exclusive,
            )],
            temp.path(),
            None,
        )
        .unwrap()
        .acquire()
        .unwrap()
        .finalize_write(&scope, &[document])
        .unwrap();
        assert!(lease
            .retain_absent_invocation_sidecar(&skill.join("agents/openai.yaml"))
            .is_err());
        assert!(!redirected.join("openai.yaml").exists());
    }

    #[test]
    fn absent_sidecar_creation_refuses_an_externally_added_parent() {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        let temp = tempfile::tempdir().unwrap();
        let document = temp.path().join("SKILL.md");
        let sidecar = temp.path().join("agents/openai.yaml");
        fs::write(&document, b"original").unwrap();
        let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
        let mut lease = CoordinationPlan::new_fixture(
            vec![DirectoryEffect::tree(
                temp.path(),
                CoordinationMode::Exclusive,
            )],
            temp.path(),
            None,
        )
        .unwrap()
        .acquire()
        .unwrap()
        .finalize_write(&scope, &[document])
        .unwrap()
        .retain_absent_invocation_sidecar(&sidecar)
        .unwrap();
        fs::create_dir(temp.path().join("agents")).unwrap();
        fs::write(temp.path().join("agents/external.txt"), b"external").unwrap();
        assert!(CodexInvocationTarget::create(temp.path(), &mut lease, b"policy: {}\n").is_err());
        assert!(!sidecar.exists());
        assert_eq!(
            fs::read(temp.path().join("agents/external.txt")).unwrap(),
            b"external"
        );
    }

    #[test]
    fn codex_sidecar_replacement_and_removal_require_the_planned_file() {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        for remove in [false, true] {
            for planned in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                fs::create_dir(temp.path().join("agents")).unwrap();
                let path = temp.path().join("agents/openai.yaml");
                let skill = temp.path().join("SKILL.md");
                fs::write(&path, b"original").unwrap();
                fs::write(&skill, b"untouched").unwrap();
                let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
                let selected = if planned { &path } else { &skill };
                let mut lease = CoordinationPlan::new_fixture(
                    vec![DirectoryEffect::tree(
                        temp.path(),
                        CoordinationMode::Exclusive,
                    )],
                    temp.path(),
                    None,
                )
                .unwrap()
                .acquire()
                .unwrap()
                .finalize_write(&scope, std::slice::from_ref(selected))
                .unwrap();
                let target = CodexInvocationTarget::bind(temp.path()).unwrap();
                let result = if remove {
                    target.remove(&mut lease, b"original")
                } else {
                    target.replace(&mut lease, b"original", b"proposed")
                };
                if planned {
                    result.unwrap();
                    lease.revalidate().unwrap();
                    if remove {
                        assert!(!path.exists());
                    } else {
                        assert_eq!(fs::read(&path).unwrap(), b"proposed");
                    }
                    fs::write(&path, b"external").unwrap();
                    assert!(lease.revalidate().is_err());
                } else {
                    assert!(matches!(
                        result,
                        Err(DocumentWriteFailure::BeforeReplace(_))
                    ));
                    assert_eq!(fs::read(&path).unwrap(), b"original");
                }
                assert_eq!(fs::read(&skill).unwrap(), b"untouched");
            }
        }
    }

    #[test]
    fn retained_registry_receipt_allows_rollback_after_post_rename_error_but_refuses_retargeting() {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("skill-studio.json");
        fs::write(&path, b"original").unwrap();
        let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
        let guard = CoordinationPlan::new(
            vec![DirectoryEffect::entry(&path, CoordinationMode::Exclusive)],
            Some(std::time::Duration::from_secs(5)),
        )
        .unwrap()
        .acquire()
        .unwrap();
        let mut lease = guard
            .finalize_write(&scope, std::slice::from_ref(&path))
            .unwrap();
        let target = SkillRegistryTarget::bind(temp.path()).unwrap();
        let result = target.replace_retained_with(&mut lease, b"original", b"proposed", || {
            Err("injected durability failure".into())
        });
        assert!(matches!(result, Err(DocumentWriteFailure::AfterReplace(_))));
        assert_eq!(
            lease.read_retained(&path, MAX_DOCUMENT_BYTES).unwrap(),
            b"proposed"
        );
        lease.revalidate().unwrap();

        target
            .replace_retained(&mut lease, b"proposed", b"original")
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"original");
        let replacement = temp.path().join("replacement.json");
        fs::write(&replacement, b"original").unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert!(lease.revalidate().is_err());
        assert!(matches!(
            target.replace_retained(&mut lease, b"original", b"overwrite"),
            Err(DocumentWriteFailure::BeforeReplace(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), b"original");
    }

    #[test]
    fn sidecar_removal_refuses_drift_and_marks_post_unlink_failure() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("agents")).unwrap();
        let path = temp.path().join("agents/openai.yaml");
        fs::write(&path, b"original").unwrap();
        let target = CodexInvocationTarget::bind(temp.path()).unwrap();
        let result = target.0.remove_with(
            b"original",
            || fs::write(&path, b"external").unwrap(),
            || Ok(()),
        );
        assert!(matches!(
            result,
            Err(DocumentWriteFailure::BeforeReplace(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), b"external");
        let result =
            target
                .0
                .remove_with(b"external", || {}, || Err("injected sync failure".into()));
        assert!(matches!(result, Err(DocumentWriteFailure::AfterReplace(_))));
        assert!(!path.exists());
    }

    #[test]
    fn registry_creation_never_replaces_a_late_file_or_link() {
        use std::os::unix::fs::MetadataExt as _;
        for case in ["success", "file", "link", "parent"] {
            let temp = tempfile::tempdir().unwrap();
            let parent = temp.path().join("agents");
            fs::create_dir(&parent).unwrap();
            let target = SkillRegistryTarget::bind(&parent).unwrap();
            let path = parent.join("skill-studio.json");
            let result = target.document.create_with(b"new registry", || match case {
                "file" => fs::write(&path, "keep existing").unwrap(),
                "link" => std::os::unix::fs::symlink("missing-target", &path).unwrap(),
                "parent" => {
                    fs::rename(&parent, temp.path().join("old-agents")).unwrap();
                    fs::create_dir(&parent).unwrap();
                }
                _ => {}
            });
            if case == "success" {
                let receipt = result.unwrap();
                receipt.revalidate().unwrap();
                assert_eq!(fs::read(&path).unwrap(), b"new registry");
                assert_eq!(fs::metadata(&path).unwrap().nlink(), 1);
                assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
            } else {
                assert!(
                    matches!(result, Err(DocumentWriteFailure::BeforeReplace(_))),
                    "{case}"
                );
                match case {
                    "file" => assert_eq!(fs::read(&path).unwrap(), b"keep existing"),
                    "link" => {
                        assert_eq!(fs::read_link(&path).unwrap(), Path::new("missing-target"))
                    }
                    "parent" => assert!(!path.exists()),
                    _ => unreachable!(),
                }
            }
            let inspected = if case == "parent" {
                temp.path().join("old-agents")
            } else {
                parent
            };
            assert!(fs::read_dir(inspected).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".skill-studio.json.tmp-")));
        }
    }

    #[test]
    fn registry_publication_consumes_temp_name_before_durability_failure() {
        use std::os::unix::fs::MetadataExt as _;
        for fail_after_publication in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let target = SkillRegistryTarget::bind(temp.path()).unwrap();
            let path = temp.path().join("skill-studio.json");
            let result = target.document.create_with_hooks(
                b"registry",
                || {},
                || {
                    let entries = fs::read_dir(temp.path())
                        .unwrap()
                        .map(|entry| entry.unwrap().file_name())
                        .collect::<Vec<_>>();
                    assert_eq!(entries, vec![std::ffi::OsString::from("skill-studio.json")]);
                    assert_eq!(fs::read(&path).unwrap(), b"registry");
                    assert_eq!(fs::metadata(&path).unwrap().nlink(), 1);
                    if fail_after_publication {
                        Err("Injected failure before directory sync".into())
                    } else {
                        Ok(())
                    }
                },
            );
            if fail_after_publication {
                assert!(matches!(result, Err(DocumentWriteFailure::AfterReplace(_))));
            } else {
                result.unwrap().revalidate().unwrap();
            }
            assert_eq!(fs::read(&path).unwrap(), b"registry");
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn registry_replacement_requires_planned_file_and_retains_receipt() {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        for planned in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let registry = temp.path().join("skill-studio.json");
            let skill = temp.path().join("SKILL.md");
            fs::write(&registry, b"original registry").unwrap();
            fs::write(&skill, b"skill content").unwrap();
            let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
            let planned_path = if planned { &registry } else { &skill };
            let mut lease = CoordinationPlan::new_fixture(
                vec![DirectoryEffect::tree(
                    temp.path(),
                    CoordinationMode::Exclusive,
                )],
                temp.path(),
                None,
            )
            .unwrap()
            .acquire()
            .unwrap()
            .finalize_write(&scope, std::slice::from_ref(planned_path))
            .unwrap();
            let target = SkillRegistryTarget::bind(temp.path()).unwrap();
            let result = target.replace(&mut lease, b"original registry", b"updated registry");
            if planned {
                result.unwrap();
                lease.revalidate().unwrap();
                assert_eq!(fs::read(&registry).unwrap(), b"updated registry");
                fs::write(&registry, b"external edit").unwrap();
                assert!(lease.revalidate().is_err());
            } else {
                assert!(matches!(
                    result,
                    Err(DocumentWriteFailure::BeforeReplace(_))
                ));
                assert_eq!(fs::read(&registry).unwrap(), b"original registry");
            }
            assert_eq!(fs::read(&skill).unwrap(), b"skill content");
        }
    }

    #[test]
    fn registry_target_refuses_parent_replacement_before_commit() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("agents");
        let moved = temp.path().join("moved");
        fs::create_dir(&parent).unwrap();
        fs::write(parent.join("skill-studio.json"), b"old").unwrap();
        let target = SkillRegistryTarget::bind(&parent).unwrap();
        let result = target.document.replace_with(b"old", b"new", || {
            fs::rename(&parent, &moved).unwrap();
            fs::create_dir(&parent).unwrap();
            fs::write(parent.join("skill-studio.json"), b"external").unwrap();
        });
        assert!(matches!(
            result,
            Err(DocumentWriteFailure::BeforeReplace(_))
        ));
        assert_eq!(
            fs::read(parent.join("skill-studio.json")).unwrap(),
            b"external"
        );
        assert_eq!(fs::read(moved.join("skill-studio.json")).unwrap(), b"old");
        assert_eq!(fs::read_dir(moved).unwrap().count(), 1);
    }

    #[test]
    fn retained_replace_preserves_permissions_and_refuses_drift_and_links() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let target = SkillDocumentTarget::bind(temp.path()).unwrap();
        target.replace_with(b"old", b"new", || {}).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(matches!(
            target.replace_with(b"old", b"bad", || {}),
            Err(DocumentWriteFailure::BeforeReplace(_))
        ));
        fs::rename(&path, temp.path().join("other")).unwrap();
        symlink("other", &path).unwrap();
        assert!(target.replace_with(b"new", b"bad", || {}).is_err());
        assert_eq!(fs::read(temp.path().join("other")).unwrap(), b"new");
    }

    #[test]
    fn parent_replacement_before_commit_preserves_both_documents() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("skill");
        let moved = temp.path().join("moved");
        fs::create_dir(&parent).unwrap();
        fs::write(parent.join("SKILL.md"), b"old").unwrap();
        let target = SkillDocumentTarget::bind(&parent).unwrap();
        let result = target.replace_with(b"old", b"new", || {
            fs::rename(&parent, &moved).unwrap();
            fs::create_dir(&parent).unwrap();
            fs::write(parent.join("SKILL.md"), b"external").unwrap();
        });
        assert!(matches!(
            result,
            Err(DocumentWriteFailure::BeforeReplace(_))
        ));
        assert_eq!(fs::read(parent.join("SKILL.md")).unwrap(), b"external");
        assert_eq!(fs::read(moved.join("SKILL.md")).unwrap(), b"old");
        assert_eq!(fs::read_dir(moved).unwrap().count(), 1);
    }

    #[test]
    fn substituted_temporary_entry_is_neither_published_nor_removed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        fs::write(&path, b"old").unwrap();
        let target = SkillDocumentTarget::bind(temp.path()).unwrap();
        let result = target.replace_with(b"old", b"new", || {
            let staged = fs::read_dir(temp.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(".SKILL.md.tmp-")
                })
                .unwrap();
            fs::rename(&staged, temp.path().join("saved-stage")).unwrap();
            fs::write(staged, b"external").unwrap();
        });
        assert!(matches!(
            result,
            Err(DocumentWriteFailure::BeforeReplace(_))
        ));
        assert_eq!(fs::read(path).unwrap(), b"old");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 3);
    }

    #[test]
    fn edit_after_staging_is_preserved_and_temp_is_removed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        fs::write(&path, b"old").unwrap();
        let target = SkillDocumentTarget::bind(temp.path()).unwrap();
        assert!(target
            .replace_with(b"old", b"new", || {
                fs::write(&path, b"edit").unwrap();
            })
            .is_err());
        assert_eq!(fs::read(path).unwrap(), b"edit");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
