use crate::{
    skill_backup_copy::{inspect_entry, sync_entry},
    skill_backup_reservation::BackupCopyLimits,
    skill_backup_source::BackupSource,
    skill_coordination::{CancellationToken, FinalizedWriteLease},
};
use cap_std::fs::MetadataExt;

#[derive(Debug)]
pub enum TreeMoveFailure {
    BeforeMove(String),
    MayHaveMoved(String),
}

#[derive(Clone, Copy)]
enum MoveCheckpoint {
    BeforeRename,
    AfterRename,
}

impl BackupSource {
    /// The service must persist intent first. Consuming the lease prevents its
    /// old directory observations from authorizing subsequent publication.
    pub fn move_verified_tree(
        &self,
        destination: &BackupSource,
        expected_tree: &str,
        lease: FinalizedWriteLease<'_>,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), TreeMoveFailure> {
        self.move_with_checkpoint(
            destination,
            expected_tree,
            lease,
            limits,
            cancellation,
            |_| Ok(()),
        )
    }

    fn move_with_checkpoint(
        &self,
        destination: &BackupSource,
        expected_tree: &str,
        lease: FinalizedWriteLease<'_>,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
        checkpoint: impl Fn(MoveCheckpoint) -> Result<(), String>,
    ) -> Result<(), TreeMoveFailure> {
        let before = TreeMoveFailure::BeforeMove;
        let validate = || -> Result<(), String> {
            lease.validate_tree_move(&self.original_path, &destination.original_path)?;
            self.revalidate().map_err(|error| error.to_string())?;
            destination
                .revalidate()
                .map_err(|error| error.to_string())?;
            let source_path = self.resolved_path().map_err(|error| error.to_string())?;
            let target_path = destination
                .resolved_path()
                .map_err(|error| error.to_string())?;
            if source_path.starts_with(&target_path) || target_path.starts_with(&source_path) {
                return Err("Tree move requires disjoint source and destination paths".into());
            }
            let source = self
                .directory
                .symlink_metadata(&self.name)
                .map_err(|error| error.to_string())?;
            if !source.is_dir()
                || source.dev()
                    != destination
                        .directory
                        .dir_metadata()
                        .map_err(|error| error.to_string())?
                        .dev()
            {
                return Err("Tree move requires a directory on the destination filesystem".into());
            }
            match destination.directory.symlink_metadata(&destination.name) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.to_string()),
                Ok(_) => Err("Tree move destination is occupied".into()),
            }
        };
        validate().map_err(before)?;
        let report = sync_entry(&self.directory, &self.name, limits, cancellation)
            .map_err(|error| before(error.to_string()))?;
        if report.tree_identity != expected_tree {
            return Err(before("Tree move source content changed".into()));
        }
        validate().map_err(before)?;
        if cancellation.is_cancelled() {
            return Err(before("Tree move cancelled".into()));
        }
        checkpoint(MoveCheckpoint::BeforeRename).map_err(before)?;
        #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
        rustix::fs::renameat_with(
            &self.directory,
            &self.name,
            &destination.directory,
            &destination.name,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(|error| {
            if error == rustix::io::Errno::EXIST {
                before(error.to_string())
            } else {
                TreeMoveFailure::MayHaveMoved(error.to_string())
            }
        })?;
        #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
        return Err(before(
            "Atomic non-replacing tree move is unsupported on this platform".into(),
        ));

        let after = TreeMoveFailure::MayHaveMoved;
        checkpoint(MoveCheckpoint::AfterRename).map_err(after)?;
        for parent in [&self.directory, &destination.directory] {
            parent
                .open(".")
                .and_then(|dir| dir.sync_all())
                .map_err(|error| after(error.to_string()))?;
        }
        self.revalidate()
            .map_err(|error| after(error.to_string()))?;
        destination
            .revalidate()
            .map_err(|error| after(error.to_string()))?;
        match self.directory.symlink_metadata(&self.name) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(after(error.to_string())),
            Ok(_) => {
                return Err(after(
                    "Tree move source is present after publication".into(),
                ))
            }
        }
        let report = inspect_entry(
            &destination.directory,
            &destination.name,
            limits,
            cancellation,
        )
        .map_err(|error| after(error.to_string()))?;
        if report.tree_identity != expected_tree {
            return Err(after("Moved tree differs from expected content".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        skill_backup_source::BackupSourceRoot,
        skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect},
        skill_scope::SkillReadScope,
    };
    use std::{ffi::OsStr, fs, path::PathBuf, time::Duration};

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        scope: SkillReadScope,
        source: BackupSource,
        destination: BackupSource,
        identity: String,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(temp.path()).unwrap();
            fs::create_dir(root.join("source")).unwrap();
            fs::create_dir(root.join("holding")).unwrap();
            fs::write(root.join("source/SKILL.md"), "original").unwrap();
            fs::write(root.join("unrelated"), "keep").unwrap();
            let sources = BackupSourceRoot::bind(&root).unwrap();
            let destinations = BackupSourceRoot::bind(&root.join("holding")).unwrap();
            let source = sources.select(OsStr::new("source")).unwrap();
            let destination = destinations.select(OsStr::new("moved")).unwrap();
            let identity = inspect_entry(
                &source.directory,
                &source.name,
                Self::limits(),
                &CancellationToken::default(),
            )
            .unwrap()
            .tree_identity;
            let scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
            Self {
                _temp: temp,
                root,
                scope,
                source,
                destination,
                identity,
            }
        }
        fn limits() -> BackupCopyLimits {
            BackupCopyLimits {
                max_bytes: 1024 * 1024,
                max_entries: 100,
                max_depth: 8,
            }
        }
        fn lease(&self, omit: Option<usize>) -> FinalizedWriteLease<'_> {
            let effects = [
                DirectoryEffect::tree(self.root.join("source"), CoordinationMode::Exclusive),
                DirectoryEffect::entry(self.root.join("source"), CoordinationMode::Exclusive),
                DirectoryEffect::entry(
                    self.root.join("holding/moved"),
                    CoordinationMode::Exclusive,
                ),
            ]
            .into_iter()
            .enumerate()
            .filter_map(|(index, effect)| (Some(index) != omit).then_some(effect))
            .collect();
            CoordinationPlan::new_fixture(effects, &self.root, Some(Duration::from_secs(2)))
                .unwrap()
                .acquire()
                .unwrap()
                .finalize_write(&self.scope, &[])
                .unwrap()
        }
        fn run(&self) -> Result<(), TreeMoveFailure> {
            self.source.move_verified_tree(
                &self.destination,
                &self.identity,
                self.lease(None),
                Self::limits(),
                &CancellationToken::default(),
            )
        }
    }

    #[test]
    fn moves_verified_directory_without_changing_siblings() {
        let f = Fixture::new();
        f.run().unwrap();
        assert!(!f.root.join("source").exists());
        assert_eq!(
            fs::read(f.root.join("holding/moved/SKILL.md")).unwrap(),
            b"original"
        );
        assert_eq!(fs::read(f.root.join("unrelated")).unwrap(), b"keep");
    }

    #[test]
    fn requires_all_three_exclusive_effects() {
        for omit in 0..3 {
            let f = Fixture::new();
            let result = f.source.move_verified_tree(
                &f.destination,
                &f.identity,
                f.lease(Some(omit)),
                Fixture::limits(),
                &CancellationToken::default(),
            );
            assert!(matches!(result, Err(TreeMoveFailure::BeforeMove(_))));
            assert!(f.root.join("source/SKILL.md").is_file());
            assert!(!f.root.join("holding/moved").exists());
        }
    }

    #[test]
    fn refuses_content_changes_and_cancellation_before_move() {
        for cancelled in [false, true] {
            let f = Fixture::new();
            let token = CancellationToken::default();
            if cancelled {
                token.cancel();
            } else {
                fs::write(f.root.join("source/SKILL.md"), "external").unwrap();
            }
            let result = f.source.move_verified_tree(
                &f.destination,
                &f.identity,
                f.lease(None),
                Fixture::limits(),
                &token,
            );
            assert!(matches!(result, Err(TreeMoveFailure::BeforeMove(_))));
            assert!(f.root.join("source/SKILL.md").is_file());
            assert!(!f.root.join("holding/moved").exists());
        }
    }

    #[test]
    fn does_not_replace_destination_created_after_validation() {
        let f = Fixture::new();
        let result = f.source.move_with_checkpoint(
            &f.destination,
            &f.identity,
            f.lease(None),
            Fixture::limits(),
            &CancellationToken::default(),
            |stage| {
                if matches!(stage, MoveCheckpoint::BeforeRename) {
                    fs::create_dir(f.root.join("holding/moved")).unwrap();
                    fs::write(f.root.join("holding/moved/external"), "keep").unwrap();
                }
                Ok(())
            },
        );
        assert!(matches!(result, Err(TreeMoveFailure::BeforeMove(_))));
        assert!(f.root.join("source/SKILL.md").is_file());
        assert_eq!(
            fs::read(f.root.join("holding/moved/external")).unwrap(),
            b"keep"
        );
    }

    #[test]
    fn replaced_destination_parent_is_refused() {
        let f = Fixture::new();
        let lease = f.lease(None);
        fs::rename(f.root.join("holding"), f.root.join("held-parent")).unwrap();
        fs::create_dir(f.root.join("holding")).unwrap();
        let result = f.source.move_verified_tree(
            &f.destination,
            &f.identity,
            lease,
            Fixture::limits(),
            &CancellationToken::default(),
        );
        assert!(matches!(result, Err(TreeMoveFailure::BeforeMove(_))));
        assert!(f.root.join("source/SKILL.md").is_file());
        assert!(!f.root.join("holding/moved").exists());
        assert!(!f.root.join("held-parent/moved").exists());
    }

    #[test]
    fn symlink_source_and_dangling_destination_are_refused() {
        for source_link in [false, true] {
            let f = Fixture::new();
            if source_link {
                fs::rename(f.root.join("source"), f.root.join("held-source")).unwrap();
                std::os::unix::fs::symlink("held-source", f.root.join("source")).unwrap();
            } else {
                std::os::unix::fs::symlink("missing", f.root.join("holding/moved")).unwrap();
            }
            assert!(matches!(f.run(), Err(TreeMoveFailure::BeforeMove(_))));
            assert!(f.root.join("source/SKILL.md").is_file());
            if source_link {
                assert_eq!(
                    fs::read_link(f.root.join("source")).unwrap(),
                    PathBuf::from("held-source")
                );
            } else {
                assert_eq!(
                    fs::read_link(f.root.join("holding/moved")).unwrap(),
                    PathBuf::from("missing")
                );
            }
        }
    }

    #[test]
    fn source_removed_after_validation_requires_fresh_observation() {
        let f = Fixture::new();
        let result = f.source.move_with_checkpoint(
            &f.destination,
            &f.identity,
            f.lease(None),
            Fixture::limits(),
            &CancellationToken::default(),
            |stage| {
                if matches!(stage, MoveCheckpoint::BeforeRename) {
                    fs::rename(f.root.join("source"), f.root.join("external-move")).unwrap();
                }
                Ok(())
            },
        );
        assert!(matches!(result, Err(TreeMoveFailure::MayHaveMoved(_))));
        assert!(!f.root.join("holding/moved").exists());
        assert_eq!(
            fs::read(f.root.join("external-move/SKILL.md")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn failure_after_rename_reports_possible_publication() {
        let f = Fixture::new();
        let result = f.source.move_with_checkpoint(
            &f.destination,
            &f.identity,
            f.lease(None),
            Fixture::limits(),
            &CancellationToken::default(),
            |stage| {
                if matches!(stage, MoveCheckpoint::AfterRename) {
                    Err("simulated failure after rename".into())
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(result, Err(TreeMoveFailure::MayHaveMoved(_))));
        assert!(!f.root.join("source").exists());
        assert_eq!(
            fs::read(f.root.join("holding/moved/SKILL.md")).unwrap(),
            b"original"
        );
    }
}
