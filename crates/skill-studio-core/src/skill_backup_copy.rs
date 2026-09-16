use crate::skill_coordination::CancellationToken;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, Metadata, MetadataExt, OpenOptions};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    io,
    io::{Read, Write},
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Copy)]
pub struct BackupCopyLimits {
    pub max_bytes: u64,
    pub max_entries: u64,
    pub max_depth: usize,
}
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BackupCopyReport {
    pub fingerprint: String,
    pub tree_identity: String,
    pub bytes: u64,
    pub entries: u64,
}
fn refused(message: &str) -> io::Error {
    io::Error::other(message)
}
pub(crate) fn unchanged(a: &Metadata, b: &Metadata) -> bool {
    (
        a.dev(),
        a.ino(),
        a.len(),
        a.mtime(),
        a.mtime_nsec(),
        a.ctime(),
        a.ctime_nsec(),
    ) == (
        b.dev(),
        b.ino(),
        b.len(),
        b.mtime(),
        b.mtime_nsec(),
        b.ctime(),
        b.ctime_nsec(),
    )
}
pub(crate) fn valid_component(name: &OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes != b"."
        && bytes != b".."
        && bytes.len() <= 255
        && !bytes.iter().any(|byte| matches!(byte, b'/' | b'\\' | 0))
}
struct TreeWalker<'a> {
    limits: BackupCopyLimits,
    cancellation: &'a CancellationToken,
    report: BackupCopyReport,
    sync_source: bool,
}
impl TreeWalker<'_> {
    fn check(&self) -> io::Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                crate::skill_coordination::CoordinationFailure::Cancelled,
            ));
        }
        Ok(())
    }
    fn entry(
        &mut self,
        source: &Dir,
        name: &OsStr,
        destination: Option<(&Dir, &OsStr)>,
        depth: usize,
        provider_source_path: Option<&Path>,
    ) -> io::Result<(String, String)> {
        self.check()?;
        if depth > self.limits.max_depth || self.report.entries >= self.limits.max_entries {
            return Err(refused("Backup entry or depth limit exceeded"));
        }
        self.report.entries += 1;
        let before = source.symlink_metadata(name)?;
        let mut hasher = Sha256::new();
        let mut tree = Sha256::new();
        tree.update(b"skill-studio-tree-v1\0");
        let copied = if before.file_type().is_symlink() {
            let original_link = source.read_link_contents(name)?;
            let link = match provider_source_path {
                Some(path) => dotagents_link_target(path, &original_link)?,
                None => original_link,
            };
            tree.update(b"L");
            tree.update((link.as_os_str().as_bytes().len() as u64).to_le_bytes());
            tree.update(link.as_os_str().as_bytes());
            hasher.update(b"L");
            hasher.update(link.to_string_lossy().as_bytes());
            if let Some((destination, target)) = destination {
                destination.symlink_contents(&link, target)?;
                let copied = destination.symlink_metadata(target)?;
                if !copied.file_type().is_symlink()
                    || destination.read_link_contents(target)? != link
                {
                    return Err(refused("Backup destination changed"));
                }
                Some(copied)
            } else {
                None
            }
        } else if before.is_dir() {
            tree.update(b"D");
            tree.update((before.mode() & 0o7777).to_le_bytes());
            hasher.update(b"D");
            let input = source.open_dir_nofollow(name)?;
            if !unchanged(&before, &input.dir_metadata()?) {
                return Err(refused("Backup source changed"));
            }
            let output = destination
                .map(|(directory, target)| {
                    directory.create_dir(target)?;
                    directory.open_dir_nofollow(target)
                })
                .transpose()?;
            let mut names = Vec::new();
            for item in input.entries()? {
                self.check()?;
                if names.len() as u64 >= self.limits.max_entries - self.report.entries {
                    return Err(refused("Backup entry limit exceeded"));
                }
                let name = item?.file_name();
                if provider_source_path.is_none() || name != ".git" {
                    names.push(name);
                }
            }
            names.sort();
            for child in names {
                if !valid_component(&child) {
                    return Err(refused("Unsupported backup entry name"));
                }
                let (fingerprint, child_identity) = self.entry(
                    &input,
                    &child,
                    output
                        .as_ref()
                        .map(|directory| (directory, child.as_os_str())),
                    depth + 1,
                    provider_source_path
                        .map(|path| path.join(&child))
                        .as_deref(),
                )?;
                tree.update((child.as_bytes().len() as u64).to_le_bytes());
                tree.update(child.as_bytes());
                tree.update(child_identity.as_bytes());
                let name = child.to_string_lossy();
                hasher.update((name.len() as u64).to_le_bytes());
                hasher.update(name.as_bytes());
                hasher.update((fingerprint.len() as u64).to_le_bytes());
                hasher.update(fingerprint.as_bytes());
            }
            if let Some(output) = &output {
                let output_directory = output.open(".")?.into_std();
                output_directory
                    .set_permissions(before.permissions().into_std(&output_directory)?)?;
                output_directory.sync_all()?;
            }
            if self.sync_source {
                input.open(".")?.sync_all()?;
            }
            if !unchanged(&before, &input.dir_metadata()?) {
                return Err(refused("Backup source changed"));
            }
            output
                .map(|directory| directory.dir_metadata())
                .transpose()?
        } else if before.is_file() {
            tree.update(b"F");
            tree.update((before.mode() & 0o7777).to_le_bytes());
            hasher.update(b"F");
            hasher.update(before.len().to_le_bytes());
            let mut input = source.open_with(
                name,
                OpenOptions::new()
                    .read(true)
                    .follow(FollowSymlinks::No)
                    .nonblock(true),
            )?;
            if !unchanged(&before, &input.metadata()?) {
                return Err(refused("Backup source changed"));
            }
            if before.len() > self.limits.max_bytes - self.report.bytes {
                return Err(refused("Backup byte limit exceeded"));
            }
            let mut output = destination
                .map(|(directory, target)| {
                    directory.open_with(target, OpenOptions::new().write(true).create_new(true))
                })
                .transpose()?;
            let mut buffer = vec![0_u8; 64 * 1024];
            loop {
                self.check()?;
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                if count as u64 > self.limits.max_bytes - self.report.bytes {
                    return Err(refused("Backup byte limit exceeded"));
                }
                if let Some(output) = &mut output {
                    output.write_all(&buffer[..count])?;
                }
                hasher.update(&buffer[..count]);
                self.report.bytes += count as u64;
            }
            if let Some(output) = &output {
                output.set_permissions(before.permissions())?;
                output.sync_all()?;
            }
            if self.sync_source {
                input.sync_all()?;
            }
            if !unchanged(&before, &input.metadata()?) {
                return Err(refused("Backup source changed"));
            }
            output.map(|file| file.metadata()).transpose()?
        } else {
            return Err(refused("Unsupported backup source entry"));
        };
        self.check()?;
        if let (Some(copied), Some((directory, target))) = (&copied, destination) {
            if !unchanged(copied, &directory.symlink_metadata(target)?) {
                return Err(refused("Backup destination changed"));
            }
            directory.open(".")?.sync_all()?;
        }
        if !unchanged(&before, &source.symlink_metadata(name)?) {
            return Err(refused("Backup source changed"));
        }
        let fingerprint: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if before.is_file() {
            tree.update(fingerprint.as_bytes());
        }
        let tree_digest: String = tree
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok((fingerprint, format!("tree-v1:{tree_digest}")))
    }
}

pub(crate) fn copy_entry(
    source: &Dir,
    name: &OsStr,
    destination: &Dir,
    target: &OsStr,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> io::Result<BackupCopyReport> {
    if !valid_component(name) || !valid_component(target) || limits.max_depth > 128 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid backup name or depth limit",
        ));
    }
    let mut copier = TreeWalker {
        limits,
        cancellation,
        report: BackupCopyReport::default(),
        sync_source: false,
    };
    let (fingerprint, tree_identity) =
        copier.entry(source, name, Some((destination, target)), 0, None)?;
    copier.report.fingerprint = fingerprint;
    copier.report.tree_identity = tree_identity;
    Ok(copier.report)
}

pub(crate) fn verify_partial_copy(
    source: &Dir,
    source_name: &OsStr,
    partial: &Dir,
    partial_name: &OsStr,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> io::Result<BackupCopyReport> {
    fn compare(
        source: &Dir,
        partial: &Dir,
        source_name: &OsStr,
        partial_name: &OsStr,
        depth: usize,
        remaining: &mut BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> io::Result<()> {
        if cancellation.is_cancelled() || depth > remaining.max_depth || remaining.max_entries == 0
        {
            return Err(refused("Partial copy verification limit or cancellation"));
        }
        remaining.max_entries -= 1;
        let expected = source.symlink_metadata(source_name)?;
        let actual = partial.symlink_metadata(partial_name)?;
        if expected.file_type().is_symlink() && actual.file_type().is_symlink() {
            if source.read_link_contents(source_name)?
                != partial.read_link_contents(partial_name)?
            {
                return Err(refused("Partial copy has a changed symlink"));
            }
        } else if expected.is_dir() && actual.is_dir() {
            let input = source.open_dir_nofollow(source_name)?;
            let output = partial.open_dir_nofollow(partial_name)?;
            for entry in output.entries()? {
                let child = entry?.file_name();
                if !valid_component(&child) {
                    return Err(refused("Invalid partial copy name"));
                }
                compare(
                    &input,
                    &output,
                    &child,
                    &child,
                    depth + 1,
                    remaining,
                    cancellation,
                )?;
            }
            if !unchanged(&actual, &output.dir_metadata()?) {
                return Err(refused(
                    "Partial copy directory changed during verification",
                ));
            }
        } else if expected.is_file() && actual.is_file() && actual.nlink() == 1 {
            if actual.len() > expected.len() || actual.len() > remaining.max_bytes {
                return Err(refused(
                    "Partial copy file exceeds its source or byte limit",
                ));
            }
            remaining.max_bytes -= actual.len();
            let options = OpenOptions::new()
                .read(true)
                .follow(FollowSymlinks::No)
                .nonblock(true)
                .clone();
            let mut input = source.open_with(source_name, &options)?;
            let mut output = partial.open_with(partial_name, &options)?;
            if !unchanged(&expected, &input.metadata()?) || !unchanged(&actual, &output.metadata()?)
            {
                return Err(refused("Partial copy file binding changed"));
            }
            let mut left = actual.len();
            let mut expected_bytes = [0u8; 65536];
            let mut actual_bytes = [0u8; 65536];
            while left > 0 {
                if cancellation.is_cancelled() {
                    return Err(refused("Partial copy verification cancelled"));
                }
                let count = left.min(expected_bytes.len() as u64) as usize;
                input.read_exact(&mut expected_bytes[..count])?;
                output.read_exact(&mut actual_bytes[..count])?;
                if expected_bytes[..count] != actual_bytes[..count] {
                    return Err(refused("Partial copy contains changed bytes"));
                }
                left -= count as u64;
            }
            if !unchanged(&expected, &input.metadata()?) || !unchanged(&actual, &output.metadata()?)
            {
                return Err(refused("Partial copy file changed during verification"));
            }
        } else {
            return Err(refused("Partial copy entry differs from its source type"));
        }
        if !unchanged(&expected, &source.symlink_metadata(source_name)?)
            || !unchanged(&actual, &partial.symlink_metadata(partial_name)?)
        {
            return Err(refused("Partial copy entry changed during verification"));
        }
        Ok(())
    }
    let before = inspect_entry(partial, partial_name, limits, cancellation)?;
    let mut remaining = limits;
    compare(
        source,
        partial,
        source_name,
        partial_name,
        0,
        &mut remaining,
        cancellation,
    )?;
    let after = inspect_entry(partial, partial_name, limits, cancellation)?;
    if before != after {
        return Err(refused("Partial copy changed during verification"));
    }
    Ok(after)
}

pub(crate) fn inspect_entry(
    source: &Dir,
    name: &OsStr,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> io::Result<BackupCopyReport> {
    if !valid_component(name) || limits.max_depth > 128 {
        return Err(refused("Invalid backup name or depth limit"));
    }
    let mut walker = TreeWalker {
        limits,
        cancellation,
        report: BackupCopyReport::default(),
        sync_source: false,
    };
    let (fingerprint, tree_identity) = walker.entry(source, name, None, 0, None)?;
    walker.report.fingerprint = fingerprint;
    walker.report.tree_identity = tree_identity;
    Ok(walker.report)
}

pub(crate) fn sync_entry(
    source: &Dir,
    name: &OsStr,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> io::Result<BackupCopyReport> {
    if !valid_component(name) || limits.max_depth > 128 {
        return Err(refused("Invalid backup name or depth limit"));
    }
    let mut walker = TreeWalker {
        limits,
        cancellation,
        report: BackupCopyReport::default(),
        sync_source: true,
    };
    let (fingerprint, tree_identity) = walker.entry(source, name, None, 0, None)?;
    walker.check()?;
    source.open(".")?.sync_all()?;
    walker.check()?;
    walker.report.fingerprint = fingerprint;
    walker.report.tree_identity = tree_identity;
    Ok(walker.report)
}

fn dotagents_link_target(source: &Path, target: &Path) -> io::Result<PathBuf> {
    if target.is_absolute() {
        return Ok(target.to_path_buf());
    }
    let parent = source
        .parent()
        .ok_or_else(|| refused("Provider source has no parent"))?;
    let mut result = PathBuf::new();
    for part in parent.join(target).components() {
        match part {
            Component::RootDir => result.push(Path::new("/")),
            Component::Normal(name) => result.push(name),
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            Component::Prefix(_) => return Err(refused("Unsupported provider source path")),
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_backup_reservation::BackupStateRoot;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn limits() -> BackupCopyLimits {
        BackupCopyLimits {
            max_bytes: 1_000_000,
            max_entries: 20,
            max_depth: 10,
        }
    }

    #[test]
    fn nested_copy_preserves_bytes_file_and_directory_modes_and_literal_links() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir_all(source.join("tree/nested")).unwrap();
        fs::create_dir(source.join("tree/empty")).unwrap();
        for (name, mode) in [
            ("tree", 0o700),
            ("tree/nested", 0o750),
            ("tree/empty", 0o710),
        ] {
            fs::set_permissions(source.join(name), fs::Permissions::from_mode(mode)).unwrap();
        }
        let bytes = vec![17_u8; 200_000];
        fs::write(source.join("tree/nested/data"), &bytes).unwrap();
        fs::set_permissions(
            source.join("tree/nested/data"),
            fs::Permissions::from_mode(0o751),
        )
        .unwrap();
        std::os::unix::fs::symlink("/outside/missing", source.join("tree/link")).unwrap();
        let input = Dir::open_ambient_dir(&source, cap_std::ambient_authority()).unwrap();
        let token = CancellationToken::default();
        let before_sync = inspect_entry(&input, OsStr::new("tree"), limits(), &token).unwrap();
        let synced = sync_entry(&input, OsStr::new("tree"), limits(), &token).unwrap();
        assert_eq!(synced, before_sync);
        assert_eq!(
            inspect_entry(&input, OsStr::new("tree"), limits(), &token).unwrap(),
            before_sync
        );
        let state = temp.path().join("state");
        fs::create_dir(&state).unwrap();
        let root = BackupStateRoot::bind(&state).unwrap();
        let backup = root.reserve("copy").unwrap();
        let report = backup
            .copy_entry(
                &input,
                OsStr::new("tree"),
                OsStr::new("saved"),
                limits(),
                &CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(report.bytes, 200_000);
        assert_eq!(report.entries, 5);
        assert_eq!(report.fingerprint.len(), 64);
        let saved = state.join("backups/copy/saved");
        for (name, mode) in [("", 0o700), ("nested", 0o750), ("empty", 0o710)] {
            assert_eq!(
                fs::metadata(saved.join(name)).unwrap().permissions().mode() & 0o777,
                mode
            );
        }
        assert_eq!(fs::read(saved.join("nested/data")).unwrap(), bytes);
        assert_eq!(
            fs::metadata(saved.join("nested/data"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o751
        );
        assert_eq!(
            fs::read_link(saved.join("link")).unwrap(),
            std::path::PathBuf::from("/outside/missing")
        );
        assert_eq!(fs::read(source.join("tree/nested/data")).unwrap(), bytes);
        assert!(backup
            .copy_entry(
                &input,
                OsStr::new("tree"),
                OsStr::new("saved"),
                limits(),
                &CancellationToken::default()
            )
            .is_err());
    }

    #[test]
    fn tree_identity_distinguishes_modes_raw_link_targets_and_tree_content() {
        use std::os::unix::ffi::OsStringExt;
        for change in [
            "file-mode",
            "directory-mode",
            "link",
            "bytes",
            "empty-directory",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source");
            fs::create_dir_all(source.join("tree")).unwrap();
            fs::write(source.join("tree/file"), "original").unwrap();
            fs::set_permissions(source.join("tree/file"), fs::Permissions::from_mode(0o600))
                .unwrap();
            fs::set_permissions(source.join("tree"), fs::Permissions::from_mode(0o700)).unwrap();
            let raw = |byte| std::ffi::OsString::from_vec(vec![b'x', byte]);
            std::os::unix::fs::symlink(raw(0x80), source.join("tree/link")).unwrap();
            let input = Dir::open_ambient_dir(&source, cap_std::ambient_authority()).unwrap();
            let state = temp.path().join("state");
            fs::create_dir(&state).unwrap();
            let root = BackupStateRoot::bind(&state).unwrap();
            let copy = |id| {
                root.reserve(id)
                    .unwrap()
                    .copy_entry(
                        &input,
                        OsStr::new("tree"),
                        OsStr::new("saved"),
                        limits(),
                        &CancellationToken::default(),
                    )
                    .unwrap()
            };
            let before = copy("before");
            let verification = inspect_entry(
                &input,
                OsStr::new("tree"),
                limits(),
                &CancellationToken::default(),
            )
            .unwrap();
            assert_eq!(verification, before);
            assert_eq!(copy("same").tree_identity, before.tree_identity);
            match change {
                "file-mode" => {
                    fs::set_permissions(source.join("tree/file"), fs::Permissions::from_mode(0o700))
                        .unwrap()
                }
                "directory-mode" => {
                    fs::set_permissions(source.join("tree"), fs::Permissions::from_mode(0o750))
                        .unwrap()
                }
                "link" => {
                    fs::remove_file(source.join("tree/link")).unwrap();
                    std::os::unix::fs::symlink(raw(0x81), source.join("tree/link")).unwrap();
                }
                "bytes" => fs::write(source.join("tree/file"), "modified").unwrap(),
                _ => fs::create_dir(source.join("tree/empty")).unwrap(),
            }
            let after = copy("after");
            assert_ne!(before.tree_identity, after.tree_identity, "{change}");
            assert!(after.tree_identity.starts_with("tree-v1:"));
            if matches!(change, "file-mode" | "directory-mode" | "link") {
                assert_eq!(
                    before.fingerprint, after.fingerprint,
                    "legacy format remains unchanged"
                );
            }
        }
    }

    #[test]
    fn saved_tree_verification_refuses_drift_and_respects_limits_without_writes() {
        for change in ["none", "bytes", "mode", "link", "entry", "limit", "cancel"] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source");
            fs::create_dir_all(source.join("tree")).unwrap();
            fs::write(source.join("tree/file"), "original").unwrap();
            std::os::unix::fs::symlink("file", source.join("tree/link")).unwrap();
            let input = Dir::open_ambient_dir(&source, cap_std::ambient_authority()).unwrap();
            let state = temp.path().join("state");
            fs::create_dir(&state).unwrap();
            let root = BackupStateRoot::bind(&state).unwrap();
            let operation = root.reserve("snapshot").unwrap();
            let expected = operation
                .copy_entry(
                    &input,
                    OsStr::new("tree"),
                    OsStr::new("saved"),
                    limits(),
                    &CancellationToken::default(),
                )
                .unwrap();
            let saved = state.join("backups/snapshot/saved");
            let token = CancellationToken::default();
            let mut budget = limits();
            match change {
                "bytes" => fs::write(saved.join("file"), "modified").unwrap(),
                "mode" => fs::set_permissions(&saved, fs::Permissions::from_mode(0o711)).unwrap(),
                "link" => {
                    fs::remove_file(saved.join("link")).unwrap();
                    std::os::unix::fs::symlink("missing", saved.join("link")).unwrap();
                }
                "entry" => fs::create_dir(saved.join("empty")).unwrap(),
                "limit" => budget.max_bytes = 1,
                "cancel" => token.cancel(),
                _ => {}
            }
            let before = fs::metadata(saved.join("file")).unwrap();
            let directory_before = fs::metadata(&saved).unwrap();
            let result = operation.verify_entry(
                OsStr::new("saved"),
                &expected.tree_identity,
                budget,
                &token,
            );
            assert_eq!(result.is_ok(), change == "none", "{change}");
            if let Ok(actual) = result {
                assert_eq!(actual, expected);
            }
            let after = fs::metadata(saved.join("file")).unwrap();
            let directory_after = fs::metadata(&saved).unwrap();
            use std::os::unix::fs::MetadataExt;
            let stamp = |metadata: &fs::Metadata| {
                (
                    metadata.ino(),
                    metadata.len(),
                    metadata.mode(),
                    metadata.mtime(),
                    metadata.mtime_nsec(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                )
            };
            assert_eq!(stamp(&before), stamp(&after));
            assert_eq!(stamp(&directory_before), stamp(&directory_after));
            assert_eq!(fs::read(source.join("tree/file")).unwrap(), b"original");
        }
    }

    #[cfg(feature = "event-store")]
    #[test]
    fn streamed_fingerprints_match_existing_backup_format() {
        use crate::skill_event_store::fingerprint_path_checked;
        let temp = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("tree/empty-directory")).unwrap();
        fs::write(temp.path().join("empty-file"), b"").unwrap();
        fs::write(temp.path().join("tree/unicode-é"), vec![81_u8; 150_001]).unwrap();
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::ffi::OsStringExt;
            let raw_name = std::ffi::OsString::from_vec(vec![b'x', 0xff]);
            fs::write(temp.path().join("tree").join(raw_name), b"raw name").unwrap();
        }
        std::os::unix::fs::symlink("/outside/not-read", temp.path().join("link")).unwrap();
        std::os::unix::fs::symlink("../empty-file", temp.path().join("tree/relative-link"))
            .unwrap();
        let input = Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        let root = BackupStateRoot::bind(state.path()).unwrap();
        for (index, name) in ["empty-file", "tree", "link"].iter().enumerate() {
            let id = format!("case-{index}");
            let operation = root.reserve(&id).unwrap();
            let report = operation
                .copy_entry(
                    &input,
                    OsStr::new(name),
                    OsStr::new("saved"),
                    limits(),
                    &CancellationToken::default(),
                )
                .unwrap();
            let expected = fingerprint_path_checked(&temp.path().join(name))
                .unwrap()
                .unwrap();
            assert_eq!(report.fingerprint, expected);
            assert_eq!(
                fingerprint_path_checked(&state.path().join("backups").join(id).join("saved"))
                    .unwrap(),
                Some(expected)
            );
        }
    }

    #[test]
    fn copy_limits_cancellation_and_names_refuse_without_success() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("tree/nested")).unwrap();
        fs::write(temp.path().join("tree/nested/data"), b"data").unwrap();
        let source = Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        let state = tempfile::tempdir().unwrap();
        let root = BackupStateRoot::bind(state.path()).unwrap();
        for (id, budget) in [
            (
                "bytes",
                BackupCopyLimits {
                    max_bytes: 3,
                    ..limits()
                },
            ),
            (
                "entries",
                BackupCopyLimits {
                    max_entries: 2,
                    ..limits()
                },
            ),
            (
                "depth",
                BackupCopyLimits {
                    max_depth: 1,
                    ..limits()
                },
            ),
        ] {
            assert!(sync_entry(
                &source,
                OsStr::new("tree"),
                budget,
                &CancellationToken::default()
            )
            .is_err());
            assert!(root
                .reserve(id)
                .unwrap()
                .copy_entry(
                    &source,
                    OsStr::new("tree"),
                    OsStr::new("saved"),
                    budget,
                    &CancellationToken::default()
                )
                .is_err());
        }
        let cancelled = CancellationToken::default();
        cancelled.cancel();
        assert_eq!(
            sync_entry(&source, OsStr::new("tree"), limits(), &cancelled)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Interrupted
        );
        let backup = root.reserve("cancelled").unwrap();
        let error = backup
            .copy_entry(
                &source,
                OsStr::new("tree"),
                OsStr::new("saved"),
                limits(),
                &cancelled,
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(crate::skill_coordination::PreparedContentError::from(error).is_cancelled());
        assert!(!state.path().join("backups/cancelled/saved").exists());
        for (from, to) in [
            ("../tree", "saved"),
            ("tree", "../escaped"),
            ("/tree", "saved"),
        ] {
            assert!(backup
                .copy_entry(
                    &source,
                    OsStr::new(from),
                    OsStr::new(to),
                    limits(),
                    &CancellationToken::default()
                )
                .is_err());
        }
        assert_eq!(
            fs::read(temp.path().join("tree/nested/data")).unwrap(),
            b"data"
        );
    }
}
