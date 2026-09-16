#[path = "skill_skills_sh_staged_source.rs"]
mod skills_sh_staged_source;
#[path = "skill_dotagents_staged_source.rs"]
mod staged_source;
use super::{
    read_record_file, same_directory, sync_directory, valid_id, BackupCopyLimits, BackupStateRoot,
};
use crate::{
    skill_backup_copy::{inspect_entry, sync_entry},
    skill_coordination::CancellationToken,
};
use cap_fs_ext::DirExt;
use cap_std::fs::{Dir, OpenOptions};
use serde::{Deserialize, Serialize};
pub use skills_sh_staged_source::{
    SkillsShReinstallRequest, SkillsShStagedSourceReceipt, SkillsShStagedSourceReference,
};
pub use staged_source::{
    DotagentsStagedSourceReceipt, DotagentsStagedSourceReceiptV2, DotagentsStagedSourceReference,
};
use std::{
    ffi::OsStr,
    io,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const SEAL_FILE: &str = "sealed-cache.json";
static SEAL_COUNTER: AtomicU64 = AtomicU64::new(0);

struct ManagedSourceLocation<'root> {
    root: &'root BackupStateRoot,
    container: Dir,
    directory: Dir,
    cache: Dir,
    id: String,
}

/// Dropping the reservation never deletes potentially referenced content.
pub struct ReservedManagedSource<'root> {
    location: ManagedSourceLocation<'root>,
    stage: Dir,
}

/// Data to bind into an operation intent; deserialization alone grants no authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedSourceReference {
    version: u32,
    operation_id: String,
    cache_identity: String,
}

pub struct SealedManagedSource<'root> {
    location: ManagedSourceLocation<'root>,
    reference: ManagedSourceReference,
}

impl ManagedSourceReference {
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub fn validate(&self) -> io::Result<()> {
        if self.version != 1
            || !valid_id(&self.operation_id)
            || !self
                .cache_identity
                .strip_prefix("tree-v1:")
                .is_some_and(|hash| {
                    hash.len() == 64
                        && hash
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
        {
            return Err(io::Error::other("Invalid managed source reference"));
        }
        Ok(())
    }
}

impl BackupStateRoot {
    /// Reserve storage only; the operation service still owns admission, leases and history.
    pub fn reserve_managed_source(&self, id: &str) -> io::Result<ReservedManagedSource<'_>> {
        if !valid_id(id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid managed source operation ID",
            ));
        }
        self.scope.revalidate_roots().map_err(io::Error::other)?;
        let mut container = self.directory.try_clone()?;
        for name in ["skill-studio", "managed-sources"] {
            match container.create_dir(name) {
                Ok(()) => sync_directory(&container)?,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
            container = container.open_dir_nofollow(name)?;
        }
        container.create_dir(id)?;
        let directory = container.open_dir_nofollow(id)?;
        directory.create_dir("cache")?;
        directory.create_dir("stage")?;
        let result = ReservedManagedSource {
            stage: directory.open_dir_nofollow("stage")?,
            location: ManagedSourceLocation {
                root: self,
                cache: directory.open_dir_nofollow("cache")?,
                container,
                directory,
                id: id.into(),
            },
        };
        result.revalidate()?;
        for directory in [
            &result.location.cache,
            &result.stage,
            &result.location.directory,
            &result.location.container,
        ] {
            sync_directory(directory)?;
        }
        result.revalidate()?;
        Ok(result)
    }

    /// Reopen an operation's storage without creating paths or admitting its content.
    pub fn open_managed_source_reservation(
        &self,
        id: &str,
    ) -> io::Result<ReservedManagedSource<'_>> {
        if !valid_id(id) {
            return Err(io::Error::other("Invalid managed source operation ID"));
        }
        self.scope.revalidate_roots().map_err(io::Error::other)?;
        let container = self
            .directory
            .open_dir_nofollow("skill-studio")?
            .open_dir_nofollow("managed-sources")?;
        let directory = container.open_dir_nofollow(id)?;
        let result = ReservedManagedSource {
            stage: directory.open_dir_nofollow("stage")?,
            location: ManagedSourceLocation {
                root: self,
                cache: directory.open_dir_nofollow("cache")?,
                container,
                directory,
                id: id.into(),
            },
        };
        result.revalidate()?;
        Ok(result)
    }

    pub fn open_managed_source(
        &self,
        reference: &ManagedSourceReference,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<SealedManagedSource<'_>> {
        reference.validate()?;
        self.scope.revalidate_roots().map_err(io::Error::other)?;
        let container = self
            .directory
            .open_dir_nofollow("skill-studio")?
            .open_dir_nofollow("managed-sources")?;
        let directory = container.open_dir_nofollow(&reference.operation_id)?;
        let result = SealedManagedSource {
            location: ManagedSourceLocation {
                root: self,
                cache: directory.open_dir_nofollow("cache")?,
                container,
                directory,
                id: reference.operation_id.clone(),
            },
            reference: reference.clone(),
        };
        result.revalidate(limits, cancellation)?;
        Ok(result)
    }
}

impl ManagedSourceLocation<'_> {
    fn path(&self) -> PathBuf {
        self.root
            .path
            .join("skill-studio/managed-sources")
            .join(&self.id)
    }

    fn revalidate(&self) -> io::Result<()> {
        self.root
            .scope
            .revalidate_roots()
            .map_err(io::Error::other)?;
        let container = self
            .root
            .directory
            .open_dir_nofollow("skill-studio")?
            .open_dir_nofollow("managed-sources")?;
        if !same_directory(&container.dir_metadata()?, &self.container.dir_metadata()?)
            || !same_directory(
                &container.symlink_metadata(&self.id)?,
                &self.directory.dir_metadata()?,
            )
            || !same_directory(
                &self.directory.symlink_metadata("cache")?,
                &self.cache.dir_metadata()?,
            )
        {
            return Err(io::Error::other("Managed source directory binding changed"));
        }
        self.root.scope.revalidate_roots().map_err(io::Error::other)
    }

    fn read_reference(&self) -> io::Result<ManagedSourceReference> {
        self.revalidate()?;
        let reference: ManagedSourceReference =
            serde_json::from_slice(&read_record_file(&self.directory, SEAL_FILE, 4096)?)
                .map_err(io::Error::other)?;
        reference.validate()?;
        if reference.operation_id != self.id {
            return Err(io::Error::other(
                "Managed source operation differs from seal",
            ));
        }
        self.revalidate()?;
        Ok(reference)
    }

    fn verify(
        &self,
        reference: &ManagedSourceReference,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<()> {
        self.revalidate()?;
        let report = inspect_entry(&self.directory, OsStr::new("cache"), limits, cancellation)?;
        if report.tree_identity != reference.cache_identity {
            return Err(io::Error::other(
                "Managed source cache changed after sealing",
            ));
        }
        self.revalidate()
    }
}

impl ReservedManagedSource<'_> {
    pub fn operation_id(&self) -> &str {
        &self.location.id
    }

    pub fn cache_path(&self) -> io::Result<PathBuf> {
        self.revalidate()?;
        self.location
            .root
            .scope
            .resolved_dir_path(&self.location.path().join("cache"))
            .map_err(io::Error::other)
    }
    pub fn stage_path(&self) -> io::Result<PathBuf> {
        self.revalidate()?;
        self.location
            .root
            .scope
            .resolved_dir_path(&self.location.path().join("stage"))
            .map_err(io::Error::other)
    }

    pub fn revalidate(&self) -> io::Result<()> {
        self.location.revalidate()?;
        if !same_directory(
            &self.location.directory.symlink_metadata("stage")?,
            &self.stage.dir_metadata()?,
        ) {
            return Err(io::Error::other(
                "Managed source staging directory binding changed",
            ));
        }
        Ok(())
    }

    /// Caller must stop the provider and retain operation coordination before sealing.
    pub fn seal_cache(
        &self,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<ManagedSourceReference> {
        self.revalidate()?;
        if cancellation.is_cancelled() {
            return Err(io::Error::other("Managed source sealing cancelled"));
        }
        match self.location.directory.symlink_metadata(SEAL_FILE) {
            Ok(_) => {
                let reference = self.location.read_reference()?;
                let report = sync_entry(
                    &self.location.directory,
                    OsStr::new("cache"),
                    limits,
                    cancellation,
                )?;
                if report.tree_identity != reference.cache_identity {
                    return Err(io::Error::other(
                        "Managed source cache changed after sealing",
                    ));
                }
                self.location.verify(&reference, limits, cancellation)?;
                self.revalidate()?;
                return Ok(reference);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let report = sync_entry(
            &self.location.directory,
            OsStr::new("cache"),
            limits,
            cancellation,
        )?;
        let reference = ManagedSourceReference {
            version: 1,
            operation_id: self.location.id.clone(),
            cache_identity: report.tree_identity,
        };
        let bytes = serde_json::to_vec(&reference).map_err(io::Error::other)?;
        let temporary = format!(
            ".cache-seal-{}-{}",
            std::process::id(),
            SEAL_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let mut file = self
            .location
            .directory
            .open_with(&temporary, OpenOptions::new().write(true).create_new(true))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        self.revalidate()?;
        self.location.verify(&reference, limits, cancellation)?;
        #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
        rustix::fs::renameat_with(
            &self.location.directory,
            temporary.as_str(),
            &self.location.directory,
            SEAL_FILE,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::other)?;
        #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
        return Err(io::Error::other(
            "Atomic source sealing is unsupported on this platform",
        ));
        sync_directory(&self.location.directory)?;
        if self.location.read_reference()? != reference {
            return Err(io::Error::other(
                "Managed source seal differs from publication",
            ));
        }
        self.location.verify(&reference, limits, cancellation)?;
        Ok(reference)
    }
}

impl SealedManagedSource<'_> {
    pub fn reference(&self) -> &ManagedSourceReference {
        &self.reference
    }

    pub fn revalidate(
        &self,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<()> {
        if self.location.read_reference()? != self.reference {
            return Err(io::Error::other(
                "Managed source seal differs from expected reference",
            ));
        }
        self.location
            .verify(&self.reference, limits, cancellation)?;
        self.validate_cache_links(limits, cancellation)?;
        self.location.verify(&self.reference, limits, cancellation)
    }

    pub fn verify_dotagents_copy(
        &self,
        source_relative: &Path,
        installed: &crate::skill_backup_source::BackupSource,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<super::BackupCopyReport> {
        self.revalidate(limits, cancellation)?;
        installed.revalidate()?;
        let cache = self.location.path().join("cache");
        let installed_path = installed.resolved_path()?;
        let cache_path = self
            .location
            .root
            .scope
            .resolved_dir_path(&cache)
            .map_err(io::Error::other)?;
        if installed_path.starts_with(&cache_path) || cache_path.starts_with(&installed_path) {
            return Err(io::Error::other(
                "Provider copy source and installation overlap",
            ));
        }
        let root = crate::skill_backup_source::BackupSourceRoot::bind(&cache_path)?;
        let source = root.select_relative(source_relative)?;
        let expected =
            crate::skill_backup_copy::inspect_dotagents_copy(&source, limits, cancellation)?;
        let actual = inspect_entry(&installed.directory, &installed.name, limits, cancellation)?;
        if actual.tree_identity != expected.tree_identity {
            return Err(io::Error::other(
                "Installed tree differs from the provider copy",
            ));
        }
        self.revalidate(limits, cancellation)?;
        installed.revalidate()?;
        let after = inspect_entry(&installed.directory, &installed.name, limits, cancellation)?;
        if after.tree_identity != actual.tree_identity {
            return Err(io::Error::other(
                "Installed tree changed during provider verification",
            ));
        }
        Ok(actual)
    }

    fn validate_cache_links(
        &self,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<()> {
        let cache = self.location.path().join("cache");
        let scope = crate::skill_scope::SkillReadScope::bind(std::slice::from_ref(&cache))
            .map_err(io::Error::other)?;
        let mut remaining = limits
            .max_entries
            .checked_sub(1)
            .ok_or_else(|| io::Error::other("Cache link entry limit exceeded"))?;
        let mut pending = vec![(cache, 0_usize)];
        while let Some((path, depth)) = pending.pop() {
            if cancellation.is_cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Cache link validation cancelled",
                ));
            }
            if depth > limits.max_depth {
                return Err(io::Error::other("Cache link depth limit exceeded"));
            }
            let budget = usize::try_from(remaining).unwrap_or(usize::MAX);
            let listing = scope.read_dir(&path, budget).map_err(io::Error::other)?;
            if !listing.issues.is_empty() {
                return Err(io::Error::other(
                    "Cache link inventory is incomplete or changed",
                ));
            }
            remaining = remaining
                .checked_sub(listing.entries.len() as u64)
                .ok_or_else(|| io::Error::other("Cache link entry limit exceeded"))?;
            for entry in listing.entries {
                if cancellation.is_cancelled() {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "Cache link validation cancelled",
                    ));
                }
                let child = path.join(&entry.name);
                if entry.raw_link_target.is_err() {
                    return Err(io::Error::other("Cache link could not be observed"));
                }
                if entry.metadata.file_type().is_symlink() {
                    let (_, metadata) = scope
                        .resolved_path_metadata(&child)
                        .map_err(io::Error::other)?;
                    if !metadata.is_file() && !metadata.is_dir() {
                        return Err(io::Error::other(
                            "Cache link target is not a file or directory",
                        ));
                    }
                }
                if entry.metadata.is_dir() {
                    pending.push((child, depth + 1));
                }
            }
        }
        scope.revalidate_roots().map_err(io::Error::other)
    }

    pub fn cache_path(
        &self,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<PathBuf> {
        self.revalidate(limits, cancellation)?;
        self.location
            .root
            .scope
            .resolved_dir_path(&self.location.path().join("cache"))
            .map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    fn limits() -> BackupCopyLimits {
        BackupCopyLimits {
            max_bytes: 4096,
            max_entries: 30,
            max_depth: 8,
        }
    }

    #[test]
    fn reservation_reopens_without_creation_and_refuses_replaced_bindings() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("skill-studio/managed-sources/resume");
        {
            let root = BackupStateRoot::bind(temp.path()).unwrap();
            assert!(root.open_managed_source_reservation("missing").is_err());
            assert!(!temp.path().join("skill-studio").exists());
            let reserved = root.reserve_managed_source("resume").unwrap();
            assert_eq!(
                reserved.cache_path().unwrap(),
                path.join("cache").canonicalize().unwrap()
            );
            assert_eq!(
                reserved.stage_path().unwrap(),
                path.join("stage").canonicalize().unwrap()
            );
            fs::write(
                reserved.cache_path().unwrap().join("partial"),
                b"provider output",
            )
            .unwrap();
            fs::write(
                reserved.stage_path().unwrap().join("partial"),
                b"stage output",
            )
            .unwrap();
        }
        let root = BackupStateRoot::bind(temp.path()).unwrap();
        let reopened = root.open_managed_source_reservation("resume").unwrap();
        assert_eq!(
            fs::read(reopened.cache_path().unwrap().join("partial")).unwrap(),
            b"provider output"
        );
        assert_eq!(
            fs::read(reopened.stage_path().unwrap().join("partial")).unwrap(),
            b"stage output"
        );
        for name in ["cache", "stage"] {
            let original = path.join(name);
            let moved = path.join(format!("{name}-moved"));
            fs::rename(&original, &moved).unwrap();
            assert!(root.open_managed_source_reservation("resume").is_err());
            assert!(!original.exists());
            symlink(&moved, &original).unwrap();
            assert!(root.open_managed_source_reservation("resume").is_err());
            assert!(reopened.revalidate().is_err());
            fs::remove_file(&original).unwrap();
            fs::rename(&moved, &original).unwrap();
        }
        reopened.revalidate().unwrap();
        for id in ["", "../resume", "missing"] {
            assert!(root.open_managed_source_reservation(id).is_err());
        }
        assert!(!path.parent().unwrap().join("missing").exists());
        let moved = path.with_file_name("resume-moved");
        fs::rename(&path, &moved).unwrap();
        symlink(&moved, &path).unwrap();
        assert!(root.open_managed_source_reservation("resume").is_err());
        assert!(reopened.revalidate().is_err());
    }

    #[test]
    fn sealed_cache_reopens_after_stage_cleanup_and_preserves_links_and_modes() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let token = CancellationToken::default();
        let root = BackupStateRoot::bind(temp.path()).unwrap();
        let reserved = root.reserve_managed_source("operation").unwrap();
        let cache = reserved.cache_path().unwrap();
        let stage = reserved.stage_path().unwrap();
        fs::create_dir(cache.join("shared")).unwrap();
        fs::create_dir(cache.join("skill")).unwrap();
        fs::write(cache.join("shared/run"), b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(cache.join("shared/run"), fs::Permissions::from_mode(0o755)).unwrap();
        symlink("../shared/run", cache.join("skill/run")).unwrap();
        symlink("../shared", cache.join("skill/resources")).unwrap();
        symlink(cache.join("shared/run"), cache.join("skill/absolute-run")).unwrap();
        let reference = reserved.seal_cache(limits(), &token).unwrap();
        assert_eq!(reserved.seal_cache(limits(), &token).unwrap(), reference);
        assert_eq!(reference.operation_id(), "operation");
        let encoded = serde_json::to_vec(&reference).unwrap();
        drop(reserved);
        drop(root);
        fs::remove_dir_all(stage).unwrap();
        let root = BackupStateRoot::bind(temp.path()).unwrap();
        let persisted = serde_json::from_slice(&encoded).unwrap();
        let sealed = root
            .open_managed_source(&persisted, limits(), &token)
            .unwrap();
        assert_eq!(sealed.reference(), &reference);
        assert_eq!(sealed.cache_path(limits(), &token).unwrap(), cache);
        assert_eq!(
            fs::read(cache.join("skill/run")).unwrap(),
            b"#!/bin/sh\nexit 0\n"
        );
        assert!(cache.join("skill/resources/run").is_file());
        assert_eq!(
            fs::metadata(cache.join("skill/run"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }

    #[test]
    fn verifies_dotagents_copy_projection_and_refuses_selected_tree_drift() {
        verify_copy_fixture(None);
    }

    #[test]
    #[ignore = "requires explicit paths to Node and an inspected cached dotagents copy helper"]
    fn verifies_cached_dotagents_copy_runtime() {
        let node = std::env::var_os("SKILL_STUDIO_FIXTURE_NODE").expect("fixture Node path");
        let helper =
            std::env::var_os("SKILL_STUDIO_FIXTURE_COPY_HELPER").expect("fixture helper path");
        verify_copy_fixture(Some((Path::new(&node), Path::new(&helper))));
    }

    fn verify_copy_fixture(provider: Option<(&Path, &Path)>) {
        use crate::skill_backup_source::BackupSourceRoot;
        use std::os::unix::fs::PermissionsExt;
        for change in [
            "none",
            "bytes",
            "mode",
            "missing",
            "extra",
            "git",
            "relative-link",
            "retarget",
            "cache",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = BackupStateRoot::bind(temp.path()).unwrap();
            let reserved = root.reserve_managed_source("operation").unwrap();
            let cache = reserved.cache_path().unwrap();
            let selected = cache.join("repository/skill");
            let shared = cache.join("repository/shared");
            fs::create_dir_all(selected.join("nested/.git")).unwrap();
            fs::create_dir(selected.join(".git")).unwrap();
            fs::create_dir(&shared).unwrap();
            fs::write(selected.join(".git/config"), b"excluded").unwrap();
            fs::write(selected.join("nested/.git/config"), b"excluded").unwrap();
            fs::write(shared.join("tool"), b"shared").unwrap();
            fs::write(selected.join("SKILL.md"), b"source").unwrap();
            fs::write(selected.join("entry.sh"), b"executable").unwrap();
            fs::set_permissions(selected.join("entry.sh"), fs::Permissions::from_mode(0o755))
                .unwrap();
            symlink("../shared/tool", selected.join("run")).unwrap();
            symlink(shared.join("tool"), selected.join("absolute")).unwrap();
            let stage = reserved.stage_path().unwrap();
            let installed = stage.join("installed");
            fs::create_dir_all(installed.join("nested")).unwrap();
            fs::write(installed.join("SKILL.md"), b"source").unwrap();
            fs::write(installed.join("entry.sh"), b"executable").unwrap();
            fs::set_permissions(
                installed.join("entry.sh"),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            symlink(shared.join("tool"), installed.join("run")).unwrap();
            symlink(shared.join("tool"), installed.join("absolute")).unwrap();
            if let Some((node, helper)) = provider {
                assert!(node.is_absolute() && helper.is_absolute());
                let output = std::process::Command::new(node)
                    .args(["--input-type=module", "-e", "import {pathToFileURL} from 'node:url'; const {copyDir} = await import(pathToFileURL(process.argv[1]).href); await copyDir(process.argv[2], process.argv[3]);"])
                    .arg(helper).arg(&selected).arg(&installed)
                    .env_clear().env("HOME", &stage).env("TMPDIR", &stage)
                    .output().unwrap();
                assert!(
                    output.status.success(),
                    "provider copy failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let token = CancellationToken::default();
            let reference = reserved.seal_cache(limits(), &token).unwrap();
            let sealed = root
                .open_managed_source(&reference, limits(), &token)
                .unwrap();
            let installed_root = BackupSourceRoot::bind(&stage).unwrap();
            let candidate = installed_root.select(OsStr::new("installed")).unwrap();
            match change {
                "none" => {}
                "bytes" => fs::write(installed.join("SKILL.md"), b"changed").unwrap(),
                "mode" => fs::set_permissions(
                    installed.join("entry.sh"),
                    fs::Permissions::from_mode(0o644),
                )
                .unwrap(),
                "missing" => fs::remove_file(installed.join("SKILL.md")).unwrap(),
                "extra" => fs::write(installed.join("extra"), b"unexpected").unwrap(),
                "git" => fs::create_dir(installed.join(".git")).unwrap(),
                "relative-link" => {
                    fs::remove_file(installed.join("run")).unwrap();
                    symlink("../shared/tool", installed.join("run")).unwrap();
                }
                "retarget" => {
                    fs::remove_file(installed.join("run")).unwrap();
                    symlink(selected.join("SKILL.md"), installed.join("run")).unwrap();
                }
                "cache" => fs::write(shared.join("tool"), b"cache drift").unwrap(),
                _ => unreachable!(),
            }
            let result = sealed.verify_dotagents_copy(
                Path::new("repository/skill"),
                &candidate,
                limits(),
                &token,
            );
            if change == "none" {
                let report = result.unwrap();
                assert_eq!(report.entries, 6);
                assert_eq!(report.bytes, 16);
                for invalid in [
                    "../repository/skill",
                    "repository/../skill",
                    "/repository/skill",
                    "missing",
                ] {
                    assert!(sealed
                        .verify_dotagents_copy(Path::new(invalid), &candidate, limits(), &token)
                        .is_err());
                }
                let cache_root = BackupSourceRoot::bind(&cache).unwrap();
                let overlapping = cache_root.select(OsStr::new("repository")).unwrap();
                assert!(sealed
                    .verify_dotagents_copy(
                        Path::new("repository/skill"),
                        &overlapping,
                        limits(),
                        &token
                    )
                    .is_err());
                let cancelled = CancellationToken::default();
                cancelled.cancel();
                assert!(sealed
                    .verify_dotagents_copy(
                        Path::new("repository/skill"),
                        &candidate,
                        limits(),
                        &cancelled
                    )
                    .is_err());
            } else {
                assert!(result.is_err(), "{change}");
            }
        }
    }

    #[test]
    fn sealed_cache_refuses_escaping_dangling_and_cyclic_link_targets() {
        for kind in [
            "absolute",
            "relative",
            "chain",
            "directory-chain",
            "missing",
            "cycle",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let state = temp.path().join("state");
            fs::create_dir(&state).unwrap();
            let outside = temp.path().join("outside");
            fs::create_dir(&outside).unwrap();
            fs::write(outside.join("secret"), b"unchanged").unwrap();
            let root = BackupStateRoot::bind(&state).unwrap();
            let reserved = root.reserve_managed_source("operation").unwrap();
            let cache = reserved.cache_path().unwrap();
            fs::create_dir(cache.join("skill")).unwrap();
            match kind {
                "absolute" => symlink(outside.join("secret"), cache.join("skill/link")).unwrap(),
                "relative" => {
                    symlink("../../../../../../outside/secret", cache.join("skill/link")).unwrap()
                }
                "chain" => {
                    symlink(outside.join("secret"), cache.join("intermediate")).unwrap();
                    symlink("../intermediate", cache.join("skill/link")).unwrap();
                }
                "directory-chain" => {
                    symlink(&outside, cache.join("intermediate")).unwrap();
                    symlink("../intermediate/secret", cache.join("skill/link")).unwrap();
                }
                "missing" => symlink("not-present", cache.join("skill/link")).unwrap(),
                "cycle" => {
                    symlink("second", cache.join("skill/link")).unwrap();
                    symlink("link", cache.join("skill/second")).unwrap();
                }
                _ => unreachable!(),
            }
            let token = CancellationToken::default();
            let reference = reserved.seal_cache(limits(), &token).unwrap();
            let record = fs::read(cache.parent().unwrap().join(SEAL_FILE)).unwrap();
            assert!(
                root.open_managed_source(&reference, limits(), &token)
                    .is_err(),
                "{kind}"
            );
            assert_eq!(
                fs::read(cache.parent().unwrap().join(SEAL_FILE)).unwrap(),
                record
            );
            assert_eq!(fs::read(outside.join("secret")).unwrap(), b"unchanged");
        }
    }

    #[test]
    fn cache_drift_refuses_reopen_revalidation_and_reseal_without_overwrite() {
        use std::os::unix::fs::PermissionsExt;
        for change in ["bytes", "mode", "link", "directory"] {
            let temp = tempfile::tempdir().unwrap();
            let token = CancellationToken::default();
            let root = BackupStateRoot::bind(temp.path()).unwrap();
            let reserved = root.reserve_managed_source("operation").unwrap();
            let cache = reserved.cache_path().unwrap();
            fs::write(cache.join("document"), b"before").unwrap();
            fs::set_permissions(cache.join("document"), fs::Permissions::from_mode(0o644)).unwrap();
            symlink("document", cache.join("link")).unwrap();
            let reference = reserved.seal_cache(limits(), &token).unwrap();
            let sealed = root
                .open_managed_source(&reference, limits(), &token)
                .unwrap();
            let seal = cache.parent().unwrap().join(SEAL_FILE);
            let before = fs::read(&seal).unwrap();
            match change {
                "bytes" => fs::write(cache.join("document"), b"after").unwrap(),
                "mode" => {
                    fs::set_permissions(cache.join("document"), fs::Permissions::from_mode(0o755))
                        .unwrap()
                }
                "link" => {
                    fs::remove_file(cache.join("link")).unwrap();
                    symlink("other", cache.join("link")).unwrap();
                }
                "directory" => {
                    fs::rename(&cache, cache.with_file_name("original-cache")).unwrap();
                    fs::create_dir(&cache).unwrap();
                }
                _ => unreachable!(),
            }
            assert!(sealed.revalidate(limits(), &token).is_err(), "{change}");
            assert!(
                root.open_managed_source(&reference, limits(), &token)
                    .is_err(),
                "{change}"
            );
            assert!(reserved.seal_cache(limits(), &token).is_err(), "{change}");
            assert_eq!(fs::read(seal).unwrap(), before, "{change}");
        }
    }

    #[test]
    fn invalid_missing_and_replaced_seals_grant_no_access_or_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let token = CancellationToken::default();
        let root = BackupStateRoot::bind(temp.path()).unwrap();
        let reserved = root.reserve_managed_source("operation").unwrap();
        let cache = reserved.cache_path().unwrap();
        let reference = reserved.seal_cache(limits(), &token).unwrap();
        let sealed = root
            .open_managed_source(&reference, limits(), &token)
            .unwrap();
        let seal = cache.parent().unwrap().join(SEAL_FILE);
        let original = fs::read(&seal).unwrap();
        let mut missing = reference.clone();
        missing.operation_id = "missing".into();
        assert!(root
            .open_managed_source(&missing, limits(), &token)
            .is_err());
        assert!(!cache
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("missing")
            .exists());
        for id in ["../escape", "", "a/b"] {
            let mut invalid = reference.clone();
            invalid.operation_id = id.into();
            assert!(root
                .open_managed_source(&invalid, limits(), &token)
                .is_err());
        }
        for changed in [b"{}".as_slice(), b"not json".as_slice()] {
            fs::write(&seal, changed).unwrap();
            assert!(sealed.revalidate(limits(), &token).is_err());
            assert!(reserved.seal_cache(limits(), &token).is_err());
            assert_eq!(fs::read(&seal).unwrap(), changed);
        }
        fs::write(&seal, &original).unwrap();
        let mut mismatched = reference.clone();
        mismatched.cache_identity = format!("tree-v1:{}", "0".repeat(64));
        assert!(root
            .open_managed_source(&mismatched, limits(), &token)
            .is_err());
        fs::remove_file(&seal).unwrap();
        assert!(sealed.revalidate(limits(), &token).is_err());
        let outside = temp.path().join("outside-seal");
        fs::write(&outside, &original).unwrap();
        symlink(&outside, &seal).unwrap();
        assert!(root
            .open_managed_source(&reference, limits(), &token)
            .is_err());
        assert!(reserved.seal_cache(limits(), &token).is_err());
        assert_eq!(fs::read(outside).unwrap(), original);
    }

    #[test]
    fn cancellation_and_budgets_leave_no_seal_and_retry_ignores_partial_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let root = BackupStateRoot::bind(temp.path()).unwrap();
        let reserved = root.reserve_managed_source("operation").unwrap();
        let cache = reserved.cache_path().unwrap();
        fs::write(cache.join("document"), b"content").unwrap();
        let seal = cache.parent().unwrap().join(SEAL_FILE);
        let token = CancellationToken::default();
        token.cancel();
        assert!(reserved.seal_cache(limits(), &token).is_err());
        assert!(!seal.exists());
        let token = CancellationToken::default();
        for budget in [
            BackupCopyLimits {
                max_bytes: 1,
                ..limits()
            },
            BackupCopyLimits {
                max_entries: 0,
                ..limits()
            },
            BackupCopyLimits {
                max_depth: 0,
                ..limits()
            },
        ] {
            assert!(reserved.seal_cache(budget, &token).is_err());
            assert!(!seal.exists());
        }
        let partial = cache.parent().unwrap().join(".cache-seal-abandoned");
        fs::write(&partial, b"partial").unwrap();
        let reference = reserved.seal_cache(limits(), &token).unwrap();
        assert_eq!(fs::read(partial).unwrap(), b"partial");
        root.open_managed_source(&reference, limits(), &token)
            .unwrap();
        token.cancel();
        assert!(root
            .open_managed_source(&reference, limits(), &token)
            .is_err());
        assert!(reserved.seal_cache(limits(), &token).is_err());
    }

    #[test]
    fn reserves_stable_distinct_paths_without_overwrite_or_drop_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let root = BackupStateRoot::bind(temp.path()).unwrap();
        for invalid in ["", ".", "..", "../other", "a/b", "a\\b"] {
            assert!(root.reserve_managed_source(invalid).is_err());
        }
        assert!(!temp.path().join("skill-studio").exists());
        let reserved = root.reserve_managed_source("operation-a").unwrap();
        assert_eq!(reserved.operation_id(), "operation-a");
        let cache = reserved.cache_path().unwrap();
        let stage = reserved.stage_path().unwrap();
        assert_eq!(
            cache,
            temp.path()
                .join("skill-studio/managed-sources/operation-a/cache")
                .canonicalize()
                .unwrap()
        );
        assert_eq!(
            stage,
            temp.path()
                .join("skill-studio/managed-sources/operation-a/stage")
                .canonicalize()
                .unwrap()
        );
        fs::write(cache.join("retained"), b"source bytes").unwrap();
        reserved.revalidate().unwrap();
        assert!(
            matches!(root.reserve_managed_source("operation-a"), Err(error) if error.kind() == io::ErrorKind::AlreadyExists)
        );
        let other = root.reserve_managed_source("operation-b").unwrap();
        assert_ne!(other.cache_path().unwrap(), cache);
        drop(reserved);
        assert_eq!(fs::read(cache.join("retained")).unwrap(), b"source bytes");
        assert!(stage.is_dir());
    }

    #[test]
    fn refuses_replaced_roots_children_and_container_symlinks() {
        for target in ["state", "container", "operation", "cache", "stage"] {
            let temp = tempfile::tempdir().unwrap();
            let state = temp.path().join("state");
            fs::create_dir(&state).unwrap();
            let root = BackupStateRoot::bind(&state).unwrap();
            let reserved = root.reserve_managed_source("operation").unwrap();
            fs::write(reserved.cache_path().unwrap().join("document"), b"source").unwrap();
            let token = CancellationToken::default();
            let reference = reserved.seal_cache(limits(), &token).unwrap();
            let sealed = root
                .open_managed_source(&reference, limits(), &token)
                .unwrap();
            let replaced = match target {
                "state" => state.clone(),
                "container" => state.join("skill-studio/managed-sources"),
                "operation" => state.join("skill-studio/managed-sources/operation"),
                "cache" => reserved.cache_path().unwrap(),
                "stage" => reserved.stage_path().unwrap(),
                _ => unreachable!(),
            };
            fs::rename(&replaced, temp.path().join("retained-original")).unwrap();
            fs::create_dir(&replaced).unwrap();
            assert!(reserved.revalidate().is_err(), "{target}");
            assert!(reserved.cache_path().is_err(), "{target}");
            assert!(reserved.stage_path().is_err(), "{target}");
            if target == "stage" {
                sealed.revalidate(limits(), &token).unwrap();
                root.open_managed_source(&reference, limits(), &token)
                    .unwrap();
            } else {
                assert!(sealed.revalidate(limits(), &token).is_err(), "{target}");
                assert!(
                    root.open_managed_source(&reference, limits(), &token)
                        .is_err(),
                    "{target}"
                );
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        let state = temp.path().join("state");
        fs::create_dir(&outside).unwrap();
        fs::create_dir(&state).unwrap();
        fs::write(outside.join("canary"), b"unchanged").unwrap();
        symlink(&outside, state.join("skill-studio")).unwrap();
        let root = BackupStateRoot::bind(&state).unwrap();
        assert!(root.reserve_managed_source("operation").is_err());
        assert!(!outside.join("managed-sources").exists());
        assert_eq!(fs::read(outside.join("canary")).unwrap(), b"unchanged");
    }
}
