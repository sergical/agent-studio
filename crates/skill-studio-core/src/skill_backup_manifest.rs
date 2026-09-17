//! Builds the existing backup format from explicit source capabilities. Recorded
//! paths are labels for restore validation; they never authorize filesystem IO.
use crate::{
    skill_backup_reservation::{BackupCopyLimits, BackupStateRoot, ReservedBackup},
    skill_backup_source::BackupSource,
    skill_coordination::CancellationToken,
    skill_event::{BackupEntry, BackupManifest},
};
use std::{ffi::OsStr, io, io::Write};

const MANIFEST_BYTE_LIMIT: usize = 8 * 1024 * 1024;

pub struct BackupManifestBuilder<'root> {
    reservation: ReservedBackup<'root>,
    manifest: BackupManifest,
    remaining: BackupCopyLimits,
    cancellation: CancellationToken,
    sources: Vec<BackupSource>,
    discard_unrecorded: bool,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn check_cancelled(token: &CancellationToken) -> io::Result<()> {
    if token.is_cancelled() {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            crate::skill_coordination::CoordinationFailure::Cancelled,
        ))
    } else {
        Ok(())
    }
}

impl<'root> BackupManifestBuilder<'root> {
    pub fn new(
        root: &'root BackupStateRoot,
        id: &str,
        limits: BackupCopyLimits,
        cancellation: CancellationToken,
    ) -> io::Result<Self> {
        check_cancelled(&cancellation)?;
        if limits.max_depth > 128 {
            return Err(invalid("Backup depth limit exceeds 128"));
        }
        Ok(Self {
            reservation: root.reserve(id)?,
            manifest: BackupManifest::default(),
            remaining: limits,
            cancellation,
            sources: Vec::new(),
            discard_unrecorded: false,
        })
    }

    pub(crate) fn discard_unrecorded_on_failure(mut self) -> Self {
        self.discard_unrecorded = true;
        self
    }

    pub(crate) fn add_absent_invocation_sidecar(
        mut self,
        lease: &crate::skill_coordination::FinalizedWriteLease<'_>,
        path: &std::path::Path,
    ) -> io::Result<Self> {
        let result = (|| -> io::Result<()> {
            check_cancelled(&self.cancellation)?;
            lease
                .validate_invocation_creation(path)
                .map_err(|error| invalid(&error))?;
            let original = path
                .to_str()
                .ok_or_else(|| invalid("Sidecar path is not UTF-8"))?;
            if self.remaining.max_entries == 0 || self.manifest.entries.contains_key(original) {
                return Err(invalid(
                    "Absent sidecar exceeds backup limits or duplicates a source",
                ));
            }
            self.manifest.entries.insert(
                original.to_owned(),
                BackupEntry {
                    relative_path: String::new(),
                    fingerprint: "absent".into(),
                },
            );
            self.remaining.max_entries -= 1;
            self.reservation.revalidate()?;
            check_cancelled(&self.cancellation)?;
            Ok(())
        })();
        match result {
            Ok(()) => Ok(self),
            Err(error) => Err(self.discard_after_failure(error)),
        }
    }

    /// Consuming self prevents publication after an add failure. Source paths
    /// come from the bound root, never from a separate caller-supplied label.
    pub fn add_source(mut self, source: BackupSource) -> io::Result<Self> {
        let result = (|| -> io::Result<()> {
            check_cancelled(&self.cancellation)?;
            source.revalidate()?;
            let original = source
                .original_path
                .to_str()
                .expect("validated source path");
            let name = source.name.as_os_str();
            if self.manifest.entries.contains_key(original) {
                return Err(invalid("Backup source must be unique"));
            }
            if self.remaining.max_entries == 0 {
                return Err(invalid("Backup entry limit exceeded"));
            }
            let entry = match source.directory.symlink_metadata(name) {
                Ok(_) => {
                    let relative_path =
                        format!("{}-{}", self.manifest.entries.len(), name.to_string_lossy());
                    let report = self.reservation.copy_entry(
                        &source.directory,
                        name,
                        OsStr::new(&relative_path),
                        self.remaining,
                        &self.cancellation,
                    )?;
                    self.remaining.max_bytes -= report.bytes;
                    self.remaining.max_entries -= report.entries;
                    BackupEntry {
                        relative_path,
                        fingerprint: report.fingerprint,
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.remaining.max_entries -= 1;
                    BackupEntry {
                        relative_path: String::new(),
                        fingerprint: "absent".into(),
                    }
                }
                Err(error) => return Err(error),
            };
            self.manifest.entries.insert(original.to_owned(), entry);
            source.revalidate()?;
            self.sources.push(source);
            self.reservation.revalidate()?;
            check_cancelled(&self.cancellation)?;
            Ok(())
        })();
        match result {
            Ok(()) => Ok(self),
            Err(error) => Err(self.discard_after_failure(error)),
        }
    }

    pub(crate) fn discard_after_failure(self, error: io::Error) -> io::Error {
        if !self.discard_unrecorded {
            return error;
        }
        match self.reservation.discard() {
            Ok(()) => error,
            Err(cleanup) => io::Error::other(format!("{error}; backup cleanup refused: {cleanup}")),
        }
    }

    /// Writes one new manifest after all copies succeed. Pre-event callers may
    /// opt into exact-reservation cleanup; existing callers retain partial evidence.
    pub fn finish(self) -> io::Result<BackupManifest> {
        self.finish_retained()
            .map(|(manifest, _reservation)| manifest)
    }

    pub(crate) fn finish_retained(self) -> io::Result<(BackupManifest, ReservedBackup<'root>)> {
        let result = (|| {
            check_cancelled(&self.cancellation)?;
            for source in &self.sources {
                source.revalidate()?;
            }
            let mut output = ManifestBuffer(Vec::new());
            serde_json::to_writer_pretty(&mut output, &self.manifest).map_err(io::Error::other)?;
            self.reservation
                .write_new_file("manifest.json", &output.0)?;
            for source in &self.sources {
                source.revalidate()?;
            }
            check_cancelled(&self.cancellation)
        })();
        match result {
            Ok(()) => {
                let Self {
                    reservation,
                    manifest,
                    ..
                } = self;
                Ok((manifest, reservation))
            }
            Err(error) => Err(self.discard_after_failure(error)),
        }
    }
}

struct ManifestBuffer(Vec<u8>);
impl Write for ManifestBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MANIFEST_BYTE_LIMIT - self.0.len() {
            return Err(invalid("Backup manifest exceeds byte limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_backup_source::BackupSourceRoot;
    use std::fs;
    fn limits() -> BackupCopyLimits {
        BackupCopyLimits {
            max_bytes: 1024,
            max_entries: 10,
            max_depth: 10,
        }
    }

    #[test]
    fn publishes_existing_manifest_format_for_present_and_missing_sources() {
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("SKILL.md"), b"skill bytes").unwrap();
        let directory = BackupSourceRoot::bind(source.path()).unwrap();
        let state = tempfile::tempdir().unwrap();
        let root = BackupStateRoot::bind(state.path()).unwrap();
        let manifest =
            BackupManifestBuilder::new(&root, "fixture", limits(), CancellationToken::default())
                .unwrap()
                .add_source(directory.select(OsStr::new("SKILL.md")).unwrap())
                .unwrap()
                .add_source(directory.select(OsStr::new("missing")).unwrap())
                .unwrap()
                .finish()
                .unwrap();
        let path = state.path().join("backups/fixture");
        let encoded = fs::read(path.join("manifest.json")).unwrap();
        let decoded: BackupManifest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            serde_json::to_value(&manifest).unwrap()
        );
        assert_eq!(
            manifest.entries[source.path().join("missing").to_str().unwrap()].fingerprint,
            "absent"
        );
        assert_eq!(fs::read(path.join("0-SKILL.md")).unwrap(), b"skill bytes");
        assert!(!path.join("1-missing").exists());
        #[cfg(feature = "event-store")]
        {
            use crate::skill_event::{EventDraft, EventStatus, InverseOp};
            use crate::skill_event_store::{fingerprint_path, EventStore};
            let store = EventStore::open(state.path()).unwrap();
            let original = source.path().join("SKILL.md");
            fs::write(&original, b"edited").unwrap();
            let inverse = InverseOp::RestoreBackup {
                path: original.clone(),
                pre_fingerprint: manifest.entries[original.to_str().unwrap()]
                    .fingerprint
                    .clone(),
                post_fingerprint: Some(fingerprint_path(&original)),
            };
            store
                .record(
                    "fixture",
                    EventDraft {
                        kind: "edit".into(),
                        skill: "fixture".into(),
                        harness: None,
                        scope: Some("global".into()),
                        project_path: None,
                        payload: serde_json::json!({}),
                        inverse: Some(serde_json::to_value(inverse).unwrap()),
                        backup_dir: Some("backups/fixture".into()),
                        restorable: true,
                    },
                )
                .unwrap();
            store.finish("fixture", EventStatus::Done).unwrap();
            store.restore("fixture", false).unwrap();
            assert_eq!(fs::read(&original).unwrap(), b"skill bytes");
        }
    }

    #[test]
    fn aggregate_limits_and_duplicate_labels_cannot_publish_partial_backups() {
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("first"), b"1234").unwrap();
        fs::write(source.path().join("second"), b"5678").unwrap();
        let directory = BackupSourceRoot::bind(source.path()).unwrap();
        let state = tempfile::tempdir().unwrap();
        let root = BackupStateRoot::bind(state.path()).unwrap();
        for (id, budget, next) in [
            (
                "bytes",
                BackupCopyLimits {
                    max_bytes: 7,
                    ..limits()
                },
                "second",
            ),
            (
                "entries",
                BackupCopyLimits {
                    max_entries: 1,
                    ..limits()
                },
                "second",
            ),
            ("duplicate", limits(), "first"),
        ] {
            let builder =
                BackupManifestBuilder::new(&root, id, budget, CancellationToken::default())
                    .unwrap()
                    .add_source(directory.select(OsStr::new("first")).unwrap())
                    .unwrap();
            assert!(builder
                .add_source(directory.select(OsStr::new(next)).unwrap())
                .is_err());
            assert!(!state
                .path()
                .join("backups")
                .join(id)
                .join("manifest.json")
                .exists());
        }
    }

    #[test]
    fn replaced_source_root_refuses_copy_and_manifest_publication() {
        for after_copy in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source");
            fs::create_dir(&source).unwrap();
            fs::write(source.join("document"), b"original").unwrap();
            let bound = BackupSourceRoot::bind(&source).unwrap();
            let selected = bound.select(OsStr::new("document")).unwrap();
            let state = tempfile::tempdir().unwrap();
            let root = BackupStateRoot::bind(state.path()).unwrap();
            let builder = BackupManifestBuilder::new(
                &root,
                "fixture",
                limits(),
                CancellationToken::default(),
            )
            .unwrap();
            let mut selected = Some(selected);
            let builder = if after_copy {
                builder.add_source(selected.take().unwrap()).unwrap()
            } else {
                builder
            };
            fs::rename(&source, temp.path().join("retained-source")).unwrap();
            fs::create_dir(&source).unwrap();
            fs::write(source.join("document"), b"replacement").unwrap();
            if after_copy {
                assert!(builder.finish().is_err());
            } else {
                assert!(builder.add_source(selected.unwrap()).is_err());
                assert!(!state.path().join("backups/fixture/0-document").exists());
            }
            assert!(!state.path().join("backups/fixture/manifest.json").exists());
            assert_eq!(fs::read(source.join("document")).unwrap(), b"replacement");
        }
    }

    #[test]
    fn source_alias_keeps_its_label_and_cannot_be_retargeted() {
        let temp = tempfile::tempdir().unwrap();
        let physical = temp.path().join("physical");
        let alias = temp.path().join("alias");
        fs::create_dir(&physical).unwrap();
        fs::write(physical.join("document"), b"original").unwrap();
        std::os::unix::fs::symlink(&physical, &alias).unwrap();
        let source = BackupSourceRoot::bind(&alias).unwrap();
        for name in ["../document", "/document", "nested/document"] {
            assert!(source.select(OsStr::new(name)).is_err());
        }
        let state = tempfile::tempdir().unwrap();
        let root = BackupStateRoot::bind(state.path()).unwrap();
        let manifest =
            BackupManifestBuilder::new(&root, "stable", limits(), CancellationToken::default())
                .unwrap()
                .add_source(source.select(OsStr::new("document")).unwrap())
                .unwrap()
                .finish()
                .unwrap();
        assert!(manifest
            .entries
            .contains_key(alias.join("document").to_str().unwrap()));
        assert!(!manifest
            .entries
            .contains_key(physical.join("document").to_str().unwrap()));
        let selected = source.select(OsStr::new("document")).unwrap();
        fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(state.path(), &alias).unwrap();
        let builder =
            BackupManifestBuilder::new(&root, "retargeted", limits(), CancellationToken::default())
                .unwrap();
        assert!(builder.add_source(selected).is_err());
        assert!(!state
            .path()
            .join("backups/retargeted/manifest.json")
            .exists());
    }

    #[test]
    fn manifest_buffer_refuses_overflow_without_appending_it() {
        let mut buffer = ManifestBuffer(Vec::new());
        buffer.write_all(&vec![b'x'; MANIFEST_BYTE_LIMIT]).unwrap();
        assert!(buffer.write_all(b"overflow").is_err());
        assert_eq!(buffer.0.len(), MANIFEST_BYTE_LIMIT);
        assert!(buffer.0.iter().all(|byte| *byte == b'x'));
    }

    #[test]
    fn pre_event_cleanup_removes_owned_failures_and_preserves_replacements() {
        for case in ["copy-error", "cancel", "replacement"] {
            let source = tempfile::tempdir().unwrap();
            fs::write(source.path().join("first"), b"preserved").unwrap();
            let directory = BackupSourceRoot::bind(source.path()).unwrap();
            let state = tempfile::tempdir().unwrap();
            let root = BackupStateRoot::bind(state.path()).unwrap();
            let token = CancellationToken::default();
            let mut budget = limits();
            if case == "copy-error" {
                budget.max_bytes = 1;
            }
            let builder = BackupManifestBuilder::new(&root, case, budget, token.clone())
                .unwrap()
                .discard_unrecorded_on_failure()
                .add_source(directory.select(OsStr::new("first")).unwrap());
            let backup = state.path().join("backups").join(case);
            if case == "copy-error" {
                assert!(builder.is_err());
                assert!(!backup.exists());
                continue;
            }
            let builder = builder.unwrap();
            if case == "replacement" {
                fs::rename(&backup, state.path().join("original-backup")).unwrap();
                fs::create_dir(&backup).unwrap();
                fs::write(backup.join("unrelated"), b"retain replacement").unwrap();
            }
            token.cancel();
            assert!(builder.finish().is_err());
            if case == "replacement" {
                assert_eq!(
                    fs::read(backup.join("unrelated")).unwrap(),
                    b"retain replacement"
                );
                assert_eq!(
                    fs::read(state.path().join("original-backup/0-first")).unwrap(),
                    b"preserved"
                );
            } else {
                assert!(!backup.exists());
            }
            assert_eq!(fs::read(source.path().join("first")).unwrap(), b"preserved");
        }
    }

    #[test]
    fn cancelled_publication_keeps_copied_bytes_without_a_manifest() {
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("first"), b"preserved").unwrap();
        let directory = BackupSourceRoot::bind(source.path()).unwrap();
        let state = tempfile::tempdir().unwrap();
        let root = BackupStateRoot::bind(state.path()).unwrap();
        let token = CancellationToken::default();
        let builder = BackupManifestBuilder::new(&root, "cancelled", limits(), token.clone())
            .unwrap()
            .add_source(directory.select(OsStr::new("first")).unwrap())
            .unwrap();
        token.cancel();
        let error = builder.finish().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(crate::skill_coordination::PreparedContentError::from(error).is_cancelled());
        assert!(!state
            .path()
            .join("backups/cancelled/manifest.json")
            .exists());
        assert_eq!(
            fs::read(state.path().join("backups/cancelled/0-first")).unwrap(),
            b"preserved"
        );
    }
}
