//! Verifies the fixed single-document repair backup through retained handles.
//! This is read evidence, not permission to restore or finalize an event.
use crate::{
    skill_coordination::FinalizedWriteLease, skill_event::BackupManifest,
    skill_event_store::fingerprint_regular_bytes, skill_repair_recovery_event::RepairRecoveryEvent,
    skill_scope::SkillReadScope, skill_service::MAX_REPAIR_DOCUMENT_BYTES,
};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, Metadata, MetadataExt, OpenOptions};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub struct VerifiedRepairBackup {
    scope: SkillReadScope,
    state_path: PathBuf,
    state: Dir,
    container: Dir,
    directory: Dir,
    id: String,
    manifest_metadata: Metadata,
    document_metadata: Metadata,
    original: Vec<u8>,
}

fn same_identity(before: &Metadata, after: &Metadata) -> bool {
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.is_dir() == after.is_dir()
        && before.is_file() == after.is_file()
        && !before.file_type().is_symlink()
        && !after.file_type().is_symlink()
}
fn same_file(before: &Metadata, after: &Metadata) -> bool {
    before.is_file()
        && after.is_file()
        && before.nlink() == 1
        && after.nlink() == 1
        && same_identity(before, after)
        && before.len() == after.len()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}
fn read_regular(directory: &Dir, name: &str, limit: usize) -> Result<(Vec<u8>, Metadata), String> {
    let mut file = directory
        .open_with(
            name,
            OpenOptions::new()
                .read(true)
                .follow(FollowSymlinks::No)
                .nonblock(true),
        )
        .map_err(|error| error.to_string())?;
    let before = file.metadata().map_err(|error| error.to_string())?;
    if !before.is_file() || before.nlink() != 1 || before.len() > limit as u64 {
        return Err("Repair backup entry must be a bounded single-link regular file".into());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > limit
        || !same_file(
            &before,
            &file.metadata().map_err(|error| error.to_string())?,
        )
        || !same_file(
            &before,
            &directory
                .symlink_metadata(name)
                .map_err(|error| error.to_string())?,
        )
    {
        return Err("Repair backup changed while reading".into());
    }
    Ok((bytes, before))
}

impl VerifiedRepairBackup {
    pub fn read(
        authorized_state: &Path,
        event: &RepairRecoveryEvent,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<Self, String> {
        let result = Self::read_document(
            authorized_state,
            event.id(),
            &event.intent().path.join("SKILL.md"),
            event.original_file_fingerprint(),
            lease,
        )?;
        event.intent().validate_original(result.original())?;
        Ok(result)
    }

    pub(crate) fn read_document(
        authorized_state: &Path,
        event_id: &str,
        document: &Path,
        expected_fingerprint: &str,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<Self, String> {
        if !crate::skill_backup_reservation::valid_id(event_id) {
            return Err("Invalid backup event ID".into());
        }
        lease.validate_state_tree(authorized_state)?;
        let scope = SkillReadScope::bind(&[authorized_state.to_path_buf()])
            .map_err(|error| error.to_string())?;
        let state: Dir = scope
            .clone_bound_directory(authorized_state)
            .map_err(|error| error.to_string())?
            .into();
        let container = state
            .open_dir_nofollow("backups")
            .map_err(|error| error.to_string())?;
        let directory = container
            .open_dir_nofollow(event_id)
            .map_err(|error| error.to_string())?;
        let (manifest_bytes, manifest_metadata) =
            read_regular(&directory, "manifest.json", 64 * 1024)?;
        let manifest: BackupManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|_| "Malformed repair backup manifest")?;
        let label = document.to_path_buf();
        let entry = manifest
            .entries
            .get(label.to_str().ok_or("Repair path is not UTF-8")?)
            .ok_or("Repair backup manifest has no matching document")?;
        if manifest.entries.len() != 1
            || entry.relative_path != "0-SKILL.md"
            || entry.fingerprint != expected_fingerprint
        {
            return Err("Repair backup manifest is inconsistent with its event".into());
        }
        let (original, document_metadata) =
            read_regular(&directory, "0-SKILL.md", MAX_REPAIR_DOCUMENT_BYTES)?;
        if fingerprint_regular_bytes(&original) != entry.fingerprint {
            return Err("Repair backup bytes do not match their fingerprint".into());
        }
        let result = Self {
            scope,
            state_path: authorized_state.to_path_buf(),
            state,
            container,
            directory,
            id: event_id.into(),
            manifest_metadata,
            document_metadata,
            original,
        };
        result.revalidate(lease)?;
        Ok(result)
    }

    pub fn original(&self) -> &[u8] {
        &self.original
    }

    pub fn revalidate(&self, lease: &FinalizedWriteLease<'_>) -> Result<(), String> {
        lease.validate_state_tree(&self.state_path)?;
        self.scope
            .revalidate_roots()
            .map_err(|error| error.to_string())?;
        if !same_identity(
            &self
                .state
                .symlink_metadata("backups")
                .map_err(|error| error.to_string())?,
            &self
                .container
                .dir_metadata()
                .map_err(|error| error.to_string())?,
        ) || !same_identity(
            &self
                .container
                .symlink_metadata(&self.id)
                .map_err(|error| error.to_string())?,
            &self
                .directory
                .dir_metadata()
                .map_err(|error| error.to_string())?,
        ) || !same_file(
            &self.manifest_metadata,
            &self
                .directory
                .symlink_metadata("manifest.json")
                .map_err(|error| error.to_string())?,
        ) || !same_file(
            &self.document_metadata,
            &self
                .directory
                .symlink_metadata("0-SKILL.md")
                .map_err(|error| error.to_string())?,
        ) {
            return Err("Repair backup binding or contents changed".into());
        }
        Ok(())
    }
}

/// Original evidence for a copy repair's two fixed backup entries.
pub struct VerifiedCopyRepairBackup {
    document: VerifiedRepairBackup,
    registry_metadata: Metadata,
    registry_original: Vec<u8>,
}

enum CopyBackupPhase {
    BeforeRepair,
    BeforeUndo,
}

impl VerifiedCopyRepairBackup {
    pub fn read(
        authorized_state: &Path,
        event: &crate::skill_repair_recovery_event::CopyRepairRecoveryEvent,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<Self, String> {
        Self::read_bound(
            authorized_state,
            event.id(),
            event.intent(),
            lease,
            CopyBackupPhase::BeforeRepair,
        )
    }

    pub fn read_undo_source(
        authorized_state: &Path,
        source: &crate::skill_repair_recovery_event::CopyRepairUndoSource,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<Self, String> {
        Self::read_bound(
            authorized_state,
            source.id(),
            source.intent(),
            lease,
            CopyBackupPhase::BeforeRepair,
        )
    }

    fn read_bound(
        authorized_state: &Path,
        event_id: &str,
        intent: &crate::skill_copy_repair::CopyRepairIntent,
        lease: &FinalizedWriteLease<'_>,
        phase: CopyBackupPhase,
    ) -> Result<Self, String> {
        intent.validate_record()?;
        let result = Self::read_documents(
            authorized_state,
            event_id,
            &intent.document.path.join("SKILL.md"),
            &intent.registry_path,
            lease,
            MAX_REPAIR_DOCUMENT_BYTES,
        )?;
        let (original, registry_original) = result.originals();
        match phase {
            CopyBackupPhase::BeforeRepair => {
                intent.document.validate_original(original)?;
                intent.validate_registry_original(registry_original)?;
            }
            CopyBackupPhase::BeforeUndo => {
                intent.validate_repaired_backup(original, registry_original)?;
            }
        }
        result.revalidate(lease)?;
        Ok(result)
    }

    pub(crate) fn read_edit(
        authorized_state: &Path,
        event_id: &str,
        intent: &crate::skill_copy_document_edit::CopyDocumentEditIntent,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<Self, String> {
        intent.validate_record()?;
        let result = Self::read_documents(
            authorized_state,
            event_id,
            &intent.transition().before().path.join("SKILL.md"),
            intent.registry_path(),
            lease,
            crate::skill_copy_document_edit::MAX_COPY_DOCUMENT_EDIT_BYTES,
        )?;
        let (document, registry) = result.originals();
        intent.validate_originals(document, registry)?;
        result.revalidate(lease)?;
        Ok(result)
    }

    fn read_documents(
        authorized_state: &Path,
        event_id: &str,
        document_path: &Path,
        registry_path: &Path,
        lease: &FinalizedWriteLease<'_>,
        document_limit: usize,
    ) -> Result<Self, String> {
        if !crate::skill_backup_reservation::valid_id(event_id) {
            return Err("Invalid copy repair backup ID".into());
        }
        lease.validate_state_tree(authorized_state)?;
        let scope = SkillReadScope::bind(&[authorized_state.to_path_buf()])
            .map_err(|error| error.to_string())?;
        let state: Dir = scope
            .clone_bound_directory(authorized_state)
            .map_err(|error| error.to_string())?
            .into();
        let container = state
            .open_dir_nofollow("backups")
            .map_err(|error| error.to_string())?;
        let directory = container
            .open_dir_nofollow(event_id)
            .map_err(|error| error.to_string())?;
        let (manifest_bytes, manifest_metadata) =
            read_regular(&directory, "manifest.json", 64 * 1024)?;
        let manifest: BackupManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|error| error.to_string())?;
        if manifest.entries.len() != 2 {
            return Err("Copy repair requires exactly two backup entries".into());
        }
        let (original, document_metadata) = read_regular(&directory, "0-SKILL.md", document_limit)?;
        let (registry_original, registry_metadata) =
            read_regular(&directory, "1-skill-studio.json", 8 * 1024 * 1024)?;
        for (path, name, bytes) in [
            (document_path, "0-SKILL.md", original.as_slice()),
            (
                registry_path,
                "1-skill-studio.json",
                registry_original.as_slice(),
            ),
        ] {
            let entry = manifest
                .entries
                .get(path.to_str().ok_or("Copy backup label is not UTF-8")?)
                .ok_or("Copy backup manifest target is missing")?;
            if entry.relative_path != name || entry.fingerprint != fingerprint_regular_bytes(bytes)
            {
                return Err("Copy backup manifest does not match original bytes".into());
            }
        }
        let result = Self {
            document: VerifiedRepairBackup {
                scope,
                state_path: authorized_state.into(),
                state,
                container,
                directory,
                id: event_id.into(),
                manifest_metadata,
                document_metadata,
                original,
            },
            registry_metadata,
            registry_original,
        };
        result.revalidate(lease)?;
        Ok(result)
    }

    pub fn originals(&self) -> (&[u8], &[u8]) {
        (self.document.original(), &self.registry_original)
    }

    pub fn revalidate(&self, lease: &FinalizedWriteLease<'_>) -> Result<(), String> {
        self.document.revalidate(lease)?;
        let metadata = self
            .document
            .directory
            .symlink_metadata("1-skill-studio.json")
            .map_err(|error| error.to_string())?;
        if !same_file(&self.registry_metadata, &metadata) {
            return Err("Copy repair registry backup changed".into());
        }
        Ok(())
    }
}

/// Retained original repair and pre-undo evidence for a validated linked pair.
pub struct VerifiedCopyUndoBackups {
    repair: VerifiedCopyRepairBackup,
    undo: VerifiedCopyRepairBackup,
}

impl VerifiedCopyUndoBackups {
    pub fn read(
        authorized_state: &Path,
        event: &crate::skill_repair_recovery_event::CopyUndoRecoveryEvent,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<Self, String> {
        Self::read_pair(
            authorized_state,
            &event.source().id,
            &event.undo().id,
            event.intent(),
            lease,
        )
    }
    pub fn read_redo_source(
        authorized_state: &Path,
        source: &crate::skill_repair_recovery_event::CopyRepairRedoSource,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<Self, String> {
        Self::read_pair(
            authorized_state,
            &source.source().id,
            &source.undo().id,
            source.intent(),
            lease,
        )
    }
    fn read_pair(
        authorized_state: &Path,
        repair_id: &str,
        undo_id: &str,
        intent: &crate::skill_copy_repair::CopyRepairIntent,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<Self, String> {
        let repair = VerifiedCopyRepairBackup::read_bound(
            authorized_state,
            repair_id,
            intent,
            lease,
            CopyBackupPhase::BeforeRepair,
        )?;
        let undo = VerifiedCopyRepairBackup::read_bound(
            authorized_state,
            undo_id,
            intent,
            lease,
            CopyBackupPhase::BeforeUndo,
        )?;
        let result = Self { repair, undo };
        result.revalidate(lease)?;
        Ok(result)
    }
    pub fn repair_originals(&self) -> (&[u8], &[u8]) {
        self.repair.originals()
    }
    pub fn undo_originals(&self) -> (&[u8], &[u8]) {
        self.undo.originals()
    }
    pub fn revalidate(&self, lease: &FinalizedWriteLease<'_>) -> Result<(), String> {
        self.repair.revalidate(lease)?;
        self.undo.revalidate(lease)
    }
}

/// Retained history and fresh redo backups for a validated three-event chain.
pub struct VerifiedCopyRedoBackups {
    history: VerifiedCopyUndoBackups,
    redo: VerifiedCopyRepairBackup,
}

impl VerifiedCopyRedoBackups {
    pub fn read(
        authorized_state: &Path,
        event: &crate::skill_repair_recovery_event::CopyRedoRecoveryEvent,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<Self, String> {
        let history = VerifiedCopyUndoBackups::read_pair(
            authorized_state,
            &event.source().id,
            &event.undo().id,
            event.prior_intent(),
            lease,
        )?;
        let redo = VerifiedCopyRepairBackup::read_bound(
            authorized_state,
            &event.redo().id,
            &event.intent().repair,
            lease,
            CopyBackupPhase::BeforeRepair,
        )?;
        let result = Self { history, redo };
        result.revalidate(lease)?;
        Ok(result)
    }
    pub fn originals(&self) -> (&[u8], &[u8]) {
        self.redo.originals()
    }
    pub fn revalidate(&self, lease: &FinalizedWriteLease<'_>) -> Result<(), String> {
        self.history.revalidate(lease)?;
        self.redo.revalidate(lease)
    }
}
