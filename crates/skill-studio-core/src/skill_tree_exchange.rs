use crate::{
    skill_backup_copy::{inspect_entry, sync_entry},
    skill_backup_reservation::BackupCopyLimits,
    skill_backup_source::BackupSource,
    skill_coordination::{CancellationToken, FinalizedWriteLease},
};
use cap_std::fs::MetadataExt;

#[derive(Debug)]
pub enum TreeExchangeFailure {
    BeforeExchange(String),
    MayHaveExchanged(String),
}

impl BackupSource {
    /// Exchanges two existing trees, retaining the old destination at the source.
    /// The service must persist intent first. The old lease cannot authorize later effects.
    pub fn exchange_verified_tree(
        &self,
        destination: &BackupSource,
        expected_source: &str,
        expected_destination: &str,
        lease: &FinalizedWriteLease<'_>,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), TreeExchangeFailure> {
        let before = TreeExchangeFailure::BeforeExchange;
        let validate = || -> Result<(), String> {
            lease.validate_state_tree(&self.original_path)?;
            lease.validate_state_tree(&destination.original_path)?;
            self.revalidate().map_err(|error| error.to_string())?;
            destination
                .revalidate()
                .map_err(|error| error.to_string())?;
            let source_path = self.resolved_path().map_err(|error| error.to_string())?;
            let target_path = destination
                .resolved_path()
                .map_err(|error| error.to_string())?;
            if source_path.starts_with(&target_path) || target_path.starts_with(&source_path) {
                return Err("Tree exchange requires disjoint trees".into());
            }
            let source = self
                .directory
                .symlink_metadata(&self.name)
                .map_err(|error| error.to_string())?;
            let target = destination
                .directory
                .symlink_metadata(&destination.name)
                .map_err(|error| error.to_string())?;
            if !source.is_dir() || !target.is_dir() || source.dev() != target.dev() {
                return Err("Tree exchange requires directories on the same filesystem".into());
            }
            Ok(())
        };
        validate().map_err(before)?;
        for (tree, expected) in [(self, expected_source), (destination, expected_destination)] {
            let report = sync_entry(&tree.directory, &tree.name, limits, cancellation)
                .map_err(|error| before(error.to_string()))?;
            if report.tree_identity != expected {
                return Err(before("Tree exchange content changed".into()));
            }
        }
        validate().map_err(before)?;
        if cancellation.is_cancelled() {
            return Err(before("Tree exchange cancelled".into()));
        }
        #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
        rustix::fs::renameat_with(
            &self.directory,
            &self.name,
            &destination.directory,
            &destination.name,
            rustix::fs::RenameFlags::EXCHANGE,
        )
        .map_err(|error| before(error.to_string()))?;
        #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
        return Err(before(
            "Atomic tree exchange is unsupported on this platform".into(),
        ));

        let after = TreeExchangeFailure::MayHaveExchanged;
        let source_sync = self.directory.open(".").and_then(|dir| dir.sync_all());
        let target_sync = destination
            .directory
            .open(".")
            .and_then(|dir| dir.sync_all());
        source_sync.map_err(|error| after(error.to_string()))?;
        target_sync.map_err(|error| after(error.to_string()))?;
        for (tree, expected) in [(self, expected_destination), (destination, expected_source)] {
            tree.revalidate()
                .map_err(|error| after(error.to_string()))?;
            let report = inspect_entry(&tree.directory, &tree.name, limits, cancellation)
                .map_err(|error| after(error.to_string()))?;
            if report.tree_identity != expected {
                return Err(after(
                    "Exchanged tree differs from the expected content".into(),
                ));
            }
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
    use std::{ffi::OsStr, fs, time::Duration};

    #[test]
    fn exchanges_verified_trees_retains_old_content_and_requires_both_domains() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(fixture.path()).unwrap();
        for (name, content) in [("candidate", "new upstream"), ("live", "user fork")] {
            fs::create_dir(root.join(name)).unwrap();
            fs::write(root.join(name).join("SKILL.md"), content).unwrap();
        }
        let sources = BackupSourceRoot::bind(&root).unwrap();
        let candidate = sources.select(OsStr::new("candidate")).unwrap();
        let live = sources.select(OsStr::new("live")).unwrap();
        let scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
        let token = CancellationToken::default();
        let limits = BackupCopyLimits {
            max_bytes: 1024 * 1024,
            max_entries: 100,
            max_depth: 8,
        };
        let new_id = inspect_entry(&candidate.directory, &candidate.name, limits, &token)
            .unwrap()
            .tree_identity;
        let old_id = inspect_entry(&live.directory, &live.name, limits, &token)
            .unwrap()
            .tree_identity;
        let plan = |both: bool| {
            let mut effects = vec![DirectoryEffect::tree(
                root.join("candidate"),
                CoordinationMode::Exclusive,
            )];
            if both {
                effects.push(DirectoryEffect::tree(
                    root.join("live"),
                    CoordinationMode::Exclusive,
                ));
            }
            CoordinationPlan::new_fixture(effects, &root, Some(Duration::from_secs(2)))
                .unwrap()
                .acquire()
                .unwrap()
                .finalize_write(&scope, &[])
                .unwrap()
        };
        {
            let lease = plan(false);
            assert!(matches!(
                candidate.exchange_verified_tree(&live, &new_id, &old_id, &lease, limits, &token),
                Err(TreeExchangeFailure::BeforeExchange(_))
            ));
        }
        {
            let lease = plan(true);
            assert!(matches!(
                candidate
                    .exchange_verified_tree(&candidate, &new_id, &new_id, &lease, limits, &token),
                Err(TreeExchangeFailure::BeforeExchange(_))
            ));
            let too_small = BackupCopyLimits {
                max_bytes: 1,
                ..limits
            };
            assert!(matches!(
                candidate
                    .exchange_verified_tree(&live, &new_id, &old_id, &lease, too_small, &token),
                Err(TreeExchangeFailure::BeforeExchange(_))
            ));
            let cancelled = CancellationToken::default();
            cancelled.cancel();
            for (new, old, token) in [
                ("wrong", old_id.as_str(), &token),
                (new_id.as_str(), "wrong", &token),
                (new_id.as_str(), old_id.as_str(), &cancelled),
            ] {
                assert!(matches!(
                    candidate.exchange_verified_tree(&live, new, old, &lease, limits, token),
                    Err(TreeExchangeFailure::BeforeExchange(_))
                ));
                assert_eq!(fs::read(root.join("live/SKILL.md")).unwrap(), b"user fork");
                assert_eq!(
                    fs::read(root.join("candidate/SKILL.md")).unwrap(),
                    b"new upstream"
                );
            }
            candidate
                .exchange_verified_tree(&live, &new_id, &old_id, &lease, limits, &token)
                .unwrap();
            assert_eq!(
                fs::read(root.join("live/SKILL.md")).unwrap(),
                b"new upstream"
            );
            assert_eq!(
                fs::read(root.join("candidate/SKILL.md")).unwrap(),
                b"user fork"
            );
            assert!(lease.revalidate().is_err());
        }
        let fresh = plan(true);
        candidate
            .exchange_verified_tree(&live, &old_id, &new_id, &fresh, limits, &token)
            .unwrap();
        assert_eq!(fs::read(root.join("live/SKILL.md")).unwrap(), b"user fork");
    }
}
