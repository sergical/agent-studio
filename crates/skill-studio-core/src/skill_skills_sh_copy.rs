use crate::{skill_backup_copy::BackupCopyLimits, skill_coordination::CancellationToken};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, MetadataExt, OpenOptions};
use sha2::{Digest, Sha256};
use std::os::unix::ffi::OsStrExt;
use std::{
    collections::BTreeSet,
    ffi::OsString,
    io::{self, Read},
    path::{Path, PathBuf},
};

pub(crate) fn verify_copy(
    cache: &Dir,
    source: &Path,
    installed: &Dir,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> io::Result<()> {
    projection(cache, source, Some(installed), limits, cancellation).map(|_| ())
}

pub(crate) fn source_identity(
    cache: &Dir,
    source: &Path,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> io::Result<String> {
    projection(cache, source, None, limits, cancellation)
}

fn projection(
    cache: &Dir,
    source: &Path,
    installed: Option<&Dir>,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> io::Result<String> {
    let mut verifier = CopyVerifier {
        cache,
        limits,
        cancellation,
        entries: 0,
        bytes: 0,
        ancestors: BTreeSet::new(),
        digest: Sha256::new(),
    };
    verifier.digest.update(b"skills-copy-projection-v1\0");
    verifier.directory(source, installed, true, 0)?;
    Ok(format!(
        "sha256:{}",
        verifier
            .digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

struct CopyVerifier<'a> {
    cache: &'a Dir,
    limits: BackupCopyLimits,
    cancellation: &'a CancellationToken,
    entries: u64,
    bytes: u64,
    ancestors: BTreeSet<PathBuf>,
    digest: Sha256,
}

impl CopyVerifier<'_> {
    fn check(&self, depth: usize) -> io::Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Skills.sh copy verification cancelled",
            ));
        }
        if depth > self.limits.max_depth
            || self.entries > self.limits.max_entries
            || self.bytes > self.limits.max_bytes
        {
            return Err(io::Error::other(
                "Skills.sh copy verification limit exceeded",
            ));
        }
        Ok(())
    }

    fn directory(
        &mut self,
        source: &Path,
        installed: Option<&Dir>,
        filtered: bool,
        depth: usize,
    ) -> io::Result<()> {
        self.check(depth)?;
        let resolved = self.cache.canonicalize(source)?;
        if !self.ancestors.insert(resolved.clone()) {
            return Err(io::Error::other(
                "Skills.sh source contains a directory link cycle",
            ));
        }
        let input = self.cache.open_dir(&resolved)?;
        let mut expected = BTreeSet::new();
        let mut names = BTreeSet::new();
        for entry in input.entries()? {
            self.entries += 1;
            self.check(depth + 1)?;
            names.insert(entry?.file_name());
        }
        self.digest.update(b"D");
        for name in names {
            let metadata = input.symlink_metadata(&name)?;
            if filtered
                && (name == "metadata.json"
                    || (metadata.is_dir()
                        && matches!(
                            name.to_str(),
                            Some(".git" | "__pycache__" | "__pypackages__")
                        )))
            {
                continue;
            }
            let child = resolved.join(&name);
            let target = match self.cache.canonicalize(&child) {
                Ok(path) => path,
                Err(error)
                    if filtered
                        && metadata.file_type().is_symlink()
                        && error.kind() == io::ErrorKind::NotFound =>
                {
                    continue
                }
                Err(error) => return Err(error),
            };
            let source_metadata = self.cache.symlink_metadata(&target)?;
            let actual = installed
                .map(|directory| directory.symlink_metadata(&name))
                .transpose()?;
            if actual
                .as_ref()
                .is_some_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(io::Error::other(
                    "Skills.sh copy contains an unexpected link",
                ));
            }
            let copied_directory = !filtered || metadata.file_type().is_symlink();
            self.digest
                .update((name.as_bytes().len() as u64).to_le_bytes());
            self.digest.update(name.as_bytes());
            if source_metadata.is_dir() && actual.as_ref().is_none_or(|metadata| metadata.is_dir())
            {
                if copied_directory
                    && actual.as_ref().is_some_and(|metadata| {
                        source_metadata.mode() & 0o777 != metadata.mode() & 0o777
                    })
                {
                    return Err(io::Error::other("Skills.sh copied directory mode differs"));
                }
                self.digest.update(
                    if copied_directory {
                        source_metadata.mode() & 0o777
                    } else {
                        0
                    }
                    .to_le_bytes(),
                );
                let output = installed
                    .map(|directory| directory.open_dir_nofollow(&name))
                    .transpose()?;
                self.directory(&target, output.as_ref(), !copied_directory, depth + 1)?;
            } else if source_metadata.is_file()
                && actual.as_ref().is_none_or(|metadata| metadata.is_file())
            {
                if actual.as_ref().is_some_and(|metadata| {
                    source_metadata.len() != metadata.len()
                        || source_metadata.mode() & 0o777 != metadata.mode() & 0o777
                }) {
                    return Err(io::Error::other("Skills.sh copied file metadata differs"));
                }
                self.bytes = self
                    .bytes
                    .checked_add(source_metadata.len())
                    .ok_or_else(|| io::Error::other("Skills.sh copy byte limit exceeded"))?;
                self.check(depth + 1)?;
                let options = OpenOptions::new()
                    .read(true)
                    .follow(FollowSymlinks::No)
                    .clone();
                let mut left = self.cache.open_with(&target, &options)?;
                let mut right = installed
                    .map(|directory| directory.open_with(&name, &options))
                    .transpose()?;
                self.digest.update(b"F");
                self.digest
                    .update((source_metadata.mode() & 0o777).to_le_bytes());
                self.digest.update(source_metadata.len().to_le_bytes());
                let mut remaining = source_metadata.len();
                let mut left_buffer = [0; 8192];
                let mut right_buffer = [0; 8192];
                while remaining > 0 {
                    self.check(depth + 1)?;
                    let count = remaining.min(left_buffer.len() as u64) as usize;
                    left.read_exact(&mut left_buffer[..count])?;
                    self.digest.update(&left_buffer[..count]);
                    if let Some(right) = &mut right {
                        right.read_exact(&mut right_buffer[..count])?;
                        if left_buffer[..count] != right_buffer[..count] {
                            return Err(io::Error::other("Skills.sh copied file content differs"));
                        }
                    }
                    remaining -= count as u64;
                }
                if left.read(&mut left_buffer[..1])? != 0
                    || right
                        .as_mut()
                        .map(|file| file.read(&mut right_buffer[..1]))
                        .transpose()?
                        .is_some_and(|count| count != 0)
                {
                    return Err(io::Error::other(
                        "Skills.sh copied file changed during verification",
                    ));
                }
            } else {
                return Err(io::Error::other("Skills.sh copied entry type differs"));
            }
            expected.insert(name);
        }
        self.digest.update(b"E");
        if let Some(installed) = installed {
            let mut actual_names = BTreeSet::<OsString>::new();
            for entry in installed.entries()? {
                self.check(depth)?;
                if actual_names.len() as u64 >= self.limits.max_entries {
                    return Err(io::Error::other("Skills.sh installed entry limit exceeded"));
                }
                actual_names.insert(entry?.file_name());
            }
            if expected != actual_names {
                return Err(io::Error::other(
                    "Skills.sh copied directory membership differs",
                ));
            }
        }
        self.ancestors.remove(&resolved);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{symlink, PermissionsExt},
    };

    #[test]
    #[ignore = "requires retained pinned skills.sh copy-contract fixture"]
    fn pinned_provider_output_matches_copy_contract() {
        let fixture = PathBuf::from(
            std::env::var_os("SKILL_STUDIO_COPY_FIXTURE").expect("copy-contract fixture path"),
        );
        let cache =
            Dir::open_ambient_dir(fixture.join("source"), cap_std::ambient_authority()).unwrap();
        let installed = Dir::open_ambient_dir(
            fixture.join("stage/home/.agents/skills/unfork-probe"),
            cap_std::ambient_authority(),
        )
        .unwrap();
        verify_copy(
            &cache,
            Path::new("skills/probe"),
            &installed,
            BackupCopyLimits {
                max_bytes: 1024 * 1024,
                max_entries: 100,
                max_depth: 20,
            },
            &CancellationToken::default(),
        )
        .unwrap();
    }

    #[test]
    fn copy_rules_accept_filtered_files_and_dereferenced_links_but_reject_drift() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let output = temp.path().join("output");
        fs::create_dir_all(cache.join("skill/__pycache__")).unwrap();
        fs::create_dir_all(cache.join("shared")).unwrap();
        fs::create_dir_all(output.join("linked-dir")).unwrap();
        fs::write(cache.join("skill/metadata.json"), "excluded").unwrap();
        fs::write(cache.join("skill/__pycache__/cache"), "excluded").unwrap();
        fs::write(cache.join("shared/metadata.json"), "included through cp").unwrap();
        fs::write(
            output.join("linked-dir/metadata.json"),
            "included through cp",
        )
        .unwrap();
        fs::write(cache.join("shared/script"), "executable").unwrap();
        fs::set_permissions(
            cache.join("shared/script"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        fs::copy(cache.join("shared/script"), output.join("script")).unwrap();
        fs::copy(
            cache.join("shared/script"),
            output.join("linked-dir/script"),
        )
        .unwrap();
        symlink("../shared/script", cache.join("skill/script")).unwrap();
        symlink("../shared", cache.join("skill/linked-dir")).unwrap();
        symlink("missing", cache.join("skill/broken")).unwrap();
        fs::set_permissions(cache.join("skill"), fs::Permissions::from_mode(0o700)).unwrap();
        let cache_dir = Dir::open_ambient_dir(&cache, cap_std::ambient_authority()).unwrap();
        let output_dir = Dir::open_ambient_dir(&output, cap_std::ambient_authority()).unwrap();
        let limits = BackupCopyLimits {
            max_bytes: 1000,
            max_entries: 30,
            max_depth: 10,
        };
        let token = CancellationToken::default();
        let verify = || verify_copy(&cache_dir, Path::new("skill"), &output_dir, limits, &token);
        verify().unwrap();
        for limited in [
            BackupCopyLimits {
                max_bytes: 1,
                ..limits
            },
            BackupCopyLimits {
                max_entries: 1,
                ..limits
            },
            BackupCopyLimits {
                max_depth: 0,
                ..limits
            },
        ] {
            assert!(
                verify_copy(&cache_dir, Path::new("skill"), &output_dir, limited, &token).is_err()
            );
        }
        fs::set_permissions(output.join("script"), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(verify().is_err());
        fs::set_permissions(output.join("script"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(output.join("script"), "unexpected").unwrap();
        assert!(verify().is_err());
        fs::copy(cache.join("shared/script"), output.join("script")).unwrap();
        fs::write(output.join("extra"), "unexpected").unwrap();
        assert!(verify().is_err());
        fs::remove_file(output.join("extra")).unwrap();
        symlink("../../outside", cache.join("skill/escape")).unwrap();
        assert!(verify().is_err());
        fs::remove_file(cache.join("skill/escape")).unwrap();
        symlink(".", cache.join("skill/cycle")).unwrap();
        fs::create_dir(output.join("cycle")).unwrap();
        assert!(verify().is_err());
    }
}
