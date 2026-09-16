use super::DotagentsRuntimeRecord;
use crate::{
    skill_backup_copy::{inspect_entry, unchanged},
    skill_backup_reservation::BackupCopyLimits,
    skill_backup_source::BackupSource,
    skill_coordination::CancellationToken,
    skill_scope::SkillReadScope,
};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{MetadataExt, OpenOptions};
use sha2::{Digest, Sha256};
use std::io::Read;

impl DotagentsRuntimeRecord {
    /// Verifies a materialized runtime snapshot, not its origin or confinement.
    /// The adapter must still confirm Node's reported version and recheck before spawn.
    pub fn verify_materialized_bytes(
        &self,
        node: &BackupSource,
        modules: &BackupSource,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), String> {
        self.validate()?;
        let check = || {
            if cancellation.is_cancelled() {
                Err("Runtime verification cancelled".to_string())
            } else {
                Ok(())
            }
        };
        check()?;
        node.revalidate().map_err(|error| error.to_string())?;
        modules.revalidate().map_err(|error| error.to_string())?;
        let before = node
            .directory
            .symlink_metadata(&node.name)
            .map_err(|error| error.to_string())?;
        if !before.is_file() || before.mode() & 0o111 == 0 || before.len() > limits.max_bytes {
            return Err("Node must be a bounded regular executable".into());
        }
        let mut executable = node
            .directory
            .open_with(
                &node.name,
                OpenOptions::new()
                    .read(true)
                    .follow(FollowSymlinks::No)
                    .nonblock(true),
            )
            .map_err(|error| error.to_string())?;
        if !unchanged(
            &before,
            &executable.metadata().map_err(|error| error.to_string())?,
        ) {
            return Err("Node changed while opening".into());
        }
        let mut hash = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        let mut read = 0_u64;
        loop {
            check()?;
            let count = executable
                .read(&mut buffer)
                .map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            read = read
                .checked_add(count as u64)
                .ok_or("Node byte count overflow")?;
            if read > limits.max_bytes {
                return Err("Node exceeds its byte limit".into());
            }
            hash.update(&buffer[..count]);
        }
        let digest: String = hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if self.node_content_digest != format!("sha256:{digest}") {
            return Err("Node bytes differ from the saved runtime".into());
        }
        if !modules
            .directory
            .symlink_metadata(&modules.name)
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            return Err("Provider runtime must be a materialized directory".into());
        }
        let verify_tree = || {
            modules.revalidate().map_err(|error| error.to_string())?;
            let tree = inspect_entry(&modules.directory, &modules.name, limits, cancellation)
                .map_err(|error| error.to_string())?;
            if tree.tree_identity != self.provider_tree_identity {
                return Err("Provider runtime bytes or metadata changed".to_string());
            }
            Ok(())
        };
        verify_tree()?;
        let scope = SkillReadScope::bind(std::slice::from_ref(&modules.original_path))
            .map_err(|error| error.to_string())?;
        let mut pending = vec![(modules.original_path.clone(), 0_usize)];
        let mut remaining = limits
            .max_entries
            .checked_sub(1)
            .ok_or("Runtime entry budget exceeded")?;
        while let Some((path, depth)) = pending.pop() {
            check()?;
            if depth > limits.max_depth {
                return Err("Runtime depth budget exceeded".into());
            }
            let listing = scope
                .read_dir(&path, usize::try_from(remaining).unwrap_or(usize::MAX))
                .map_err(|error| error.to_string())?;
            if !listing.issues.is_empty() {
                return Err("Runtime inventory is incomplete".into());
            }
            remaining = remaining
                .checked_sub(listing.entries.len() as u64)
                .ok_or("Runtime entry budget exceeded")?;
            for entry in listing.entries {
                check()?;
                if entry.metadata.file_type().is_symlink() || entry.raw_link_target.is_err() {
                    return Err("Runtime snapshot must materialize every link".into());
                }
                if entry.metadata.is_dir() {
                    pending.push((path.join(entry.name), depth + 1));
                }
            }
        }
        for name in ["dotagents", "dotagents-lib"] {
            let path = modules
                .original_path
                .join("@sentry")
                .join(name)
                .join("package.json");
            let bytes = scope
                .read(&path, 64 * 1024)
                .map_err(|error| error.to_string())?;
            let package: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            if package["name"].as_str() != Some(format!("@sentry/{name}").as_str())
                || package["version"].as_str() != Some(self.provider_version.as_str())
            {
                return Err("Provider package differs from its saved runtime version".into());
            }
        }
        let entry = scope
            .read(
                &modules
                    .original_path
                    .join("@sentry/dotagents/dist/cli/index.js"),
                8 * 1024 * 1024,
            )
            .map_err(|error| error.to_string())?;
        if entry.is_empty() {
            return Err("Provider entry point is empty".into());
        }
        verify_tree()?;
        if !unchanged(
            &before,
            &executable.metadata().map_err(|error| error.to_string())?,
        ) || !unchanged(
            &before,
            &node
                .directory
                .symlink_metadata(&node.name)
                .map_err(|error| error.to_string())?,
        ) {
            return Err("Node changed during runtime verification".into());
        }
        node.revalidate().map_err(|error| error.to_string())?;
        scope
            .revalidate_roots()
            .map_err(|error| error.to_string())?;
        check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_backup_source::BackupSourceRoot;
    use std::{
        ffi::OsStr,
        fs,
        os::unix::fs::{symlink, PermissionsExt},
    };

    #[test]
    #[ignore = "requires an explicitly prepared copied local runtime fixture"]
    fn verifies_copied_local_provider_runtime() {
        let fixture = std::path::PathBuf::from(
            std::env::var_os("SKILL_STUDIO_RUNTIME_FIXTURE").expect("fixture root"),
        );
        let input: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.join("input.json")).unwrap()).unwrap();
        let node_path = std::path::PathBuf::from(input["node"].as_str().unwrap());
        let node_root = BackupSourceRoot::bind(node_path.parent().unwrap()).unwrap();
        let node = node_root.select(node_path.file_name().unwrap()).unwrap();
        let root = BackupSourceRoot::bind(&fixture).unwrap();
        let modules = root.select(OsStr::new("node_modules")).unwrap();
        let limits = BackupCopyLimits {
            max_bytes: 256 * 1024 * 1024,
            max_entries: 100_000,
            max_depth: 32,
        };
        let token = CancellationToken::default();
        let tree = inspect_entry(&modules.directory, &modules.name, limits, &token).unwrap();
        let record = DotagentsRuntimeRecord {
            provider_version: input["provider_version"].as_str().unwrap().into(),
            provider_tree_identity: tree.tree_identity,
            node_version: input["node_version"].as_str().unwrap().into(),
            node_content_digest: input["node_content_digest"].as_str().unwrap().into(),
            copy_contract: "dotagents-3.0.1-default-node-copy".into(),
        };
        record
            .verify_materialized_bytes(&node, &modules, limits, &token)
            .unwrap();
        let saved = serde_json::to_vec_pretty(&record).unwrap();
        fs::write(fixture.join("verified-record.json"), &saved).unwrap();
        let reread: DotagentsRuntimeRecord = serde_json::from_slice(&saved).unwrap();
        reread
            .verify_materialized_bytes(&node, &modules, limits, &token)
            .unwrap();
        println!(
            "Verified provider runtime: {} entries, {} bytes; Node {}",
            tree.entries, tree.bytes, record.node_version
        );
        println!("{}", String::from_utf8(saved).unwrap());
    }

    #[test]
    fn runtime_bytes_bind_node_dependencies_packages_and_materialized_layout() {
        let temp = tempfile::tempdir().unwrap();
        let modules = temp.path().join("node_modules");
        for name in ["dotagents", "dotagents-lib"] {
            let package = modules.join("@sentry").join(name);
            fs::create_dir_all(&package).unwrap();
            fs::write(
                package.join("package.json"),
                serde_json::to_vec(&serde_json::json!({
                    "name":format!("@sentry/{name}"), "version":"3.0.1"
                }))
                .unwrap(),
            )
            .unwrap();
        }
        let entry = modules.join("@sentry/dotagents/dist/cli");
        fs::create_dir_all(&entry).unwrap();
        fs::write(entry.join("index.js"), b"fixture provider entry").unwrap();
        fs::write(modules.join("dependency.js"), b"dependency").unwrap();
        let node_path = temp.path().join("node");
        fs::write(&node_path, b"fixture executable bytes").unwrap();
        fs::set_permissions(&node_path, fs::Permissions::from_mode(0o755)).unwrap();
        let root = BackupSourceRoot::bind(temp.path()).unwrap();
        let node = root.select(OsStr::new("node")).unwrap();
        let provider = root.select(OsStr::new("node_modules")).unwrap();
        let token = CancellationToken::default();
        let limits = BackupCopyLimits {
            max_bytes: 65536,
            max_entries: 64,
            max_depth: 8,
        };
        let mut record = DotagentsRuntimeRecord {
            provider_version: "3.0.1".into(),
            provider_tree_identity: inspect_entry(
                &provider.directory,
                &provider.name,
                limits,
                &token,
            )
            .unwrap()
            .tree_identity,
            node_version: "v26.8.2".into(),
            node_content_digest:
                "sha256:f67bea1e29bf7fa00d04549495d9d5d3bf5fc92aa36a59fc471f792d6c8b153c".into(),
            copy_contract: "dotagents-3.0.1-default-node-copy".into(),
        };
        record
            .verify_materialized_bytes(&node, &provider, limits, &token)
            .unwrap();
        for path in [
            &node_path,
            &modules.join("dependency.js"),
            &entry.join("index.js"),
        ] {
            let bytes = fs::read(path).unwrap();
            fs::write(path, b"changed").unwrap();
            assert!(record
                .verify_materialized_bytes(&node, &provider, limits, &token)
                .is_err());
            fs::write(path, bytes).unwrap();
        }
        fs::set_permissions(&node_path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(record
            .verify_materialized_bytes(&node, &provider, limits, &token)
            .is_err());
        fs::set_permissions(&node_path, fs::Permissions::from_mode(0o755)).unwrap();
        let cancelled = CancellationToken::default();
        cancelled.cancel();
        assert!(record
            .verify_materialized_bytes(&node, &provider, limits, &cancelled)
            .is_err());
        assert!(record
            .verify_materialized_bytes(
                &node,
                &provider,
                BackupCopyLimits {
                    max_bytes: 1,
                    ..limits
                },
                &token
            )
            .is_err());
        symlink("dependency.js", modules.join("linked.js")).unwrap();
        record.provider_tree_identity =
            inspect_entry(&provider.directory, &provider.name, limits, &token)
                .unwrap()
                .tree_identity;
        assert!(record
            .verify_materialized_bytes(&node, &provider, limits, &token)
            .is_err());
        fs::remove_file(modules.join("linked.js")).unwrap();
        fs::write(
            modules.join("@sentry/dotagents-lib/package.json"),
            b"{\"name\":\"@sentry/dotagents-lib\",\"version\":\"9.0.0\"}",
        )
        .unwrap();
        record.provider_tree_identity =
            inspect_entry(&provider.directory, &provider.name, limits, &token)
                .unwrap()
                .tree_identity;
        assert!(record
            .verify_materialized_bytes(&node, &provider, limits, &token)
            .is_err());
    }
}
