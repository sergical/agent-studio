//! Request-correlated candidate content, not an independent Git commit attestation.
use crate::{
    skill_backup_copy::inspect_entry,
    skill_backup_reservation::BackupCopyLimits,
    skill_backup_source::{BackupSource, BackupSourceRoot},
    skill_coordination::CancellationToken,
    skill_dotagents_ledger::DotagentsForkSource,
    skill_process_stream::{run_to_writer, ProcessStreamLimits},
    skill_upstream_archive::{extract_gzip, ArchiveLimits},
};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{File, OpenOptions};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Seek, SeekFrom, Write},
    path::Path,
    process::Command,
    time::Instant,
};

pub struct FetchedForkSource {
    request: DotagentsForkSource,
    archive_digest: String,
    tree_identity: String,
    source: BackupSource,
}

impl FetchedForkSource {
    pub fn request(&self) -> &DotagentsForkSource {
        &self.request
    }
    pub fn archive_digest(&self) -> &str {
        &self.archive_digest
    }
    pub fn tree_identity(&self) -> &str {
        &self.tree_identity
    }
    pub fn path(&self) -> &Path {
        &self.source.original_path
    }

    pub(crate) fn into_matching_source(
        self,
        expected: &DotagentsForkSource,
    ) -> Result<(BackupSource, String), String> {
        if &self.request != expected {
            return Err("Fetched source does not match the prepared fork request".into());
        }
        self.source
            .revalidate()
            .map_err(|error| error.to_string())?;
        Ok((self.source, self.tree_identity))
    }
}

struct ArchiveWriter {
    file: File,
    digest: Sha256,
}
impl Write for ArchiveWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.file.write(bytes)?;
        self.digest.update(&bytes[..written]);
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// The caller authorizes `gh` and owns the empty staging directory until the source
/// is consumed. It must discard staging after failure. Environment isolation is separate.
pub fn fetch_fork_source(
    request: &DotagentsForkSource,
    gh: &Path,
    staging: &Path,
    limits: ArchiveLimits,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<FetchedForkSource, String> {
    let check = || {
        if cancellation.is_cancelled() {
            Err("Upstream fetch cancelled".to_string())
        } else if Instant::now() >= deadline {
            Err("Upstream fetch deadline exceeded".to_string())
        } else {
            Ok(())
        }
    };
    check()?;
    let root = BackupSourceRoot::bind(staging).map_err(|error| error.to_string())?;
    let directory = root.directory().map_err(|error| error.to_string())?;
    if directory
        .entries()
        .map_err(|error| error.to_string())?
        .next()
        .transpose()
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Err("Upstream staging must be empty".into());
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    let mut writer = ArchiveWriter {
        file: directory
            .open_with("repository.tar.gz", &options)
            .map_err(|error| error.to_string())?,
        digest: Sha256::new(),
    };
    let mut command = Command::new(gh);
    command.args([
        "api",
        "--hostname",
        "github.com",
        &format!("repos/{}/tarball/{}", request.repo(), request.commit()),
    ]);
    run_to_writer(
        &mut command,
        &mut writer,
        ProcessStreamLimits {
            stdout_bytes: limits.max_compressed_bytes,
            stderr_bytes: 64 * 1024,
            deadline,
        },
        check,
    )?;
    check()?;
    writer
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    directory
        .create_dir("extracted")
        .map_err(|error| error.to_string())?;
    let extracted = directory
        .open_dir_nofollow("extracted")
        .map_err(|error| error.to_string())?;
    let report = extract_gzip(&mut writer.file, &extracted, limits, || {
        check().map_err(io::Error::other)
    })
    .map_err(|error| error.to_string())?;
    let relative = Path::new("extracted")
        .join(report.repository_root)
        .join(request.path());
    let source = root
        .select_relative(&relative)
        .map_err(|error| error.to_string())?;
    if !source
        .directory
        .symlink_metadata(&source.name)
        .map_err(|error| error.to_string())?
        .is_dir()
    {
        return Err("Requested upstream skill is not a directory entry".into());
    }
    let tree = inspect_entry(
        &source.directory,
        &source.name,
        BackupCopyLimits {
            max_bytes: limits.max_expanded_bytes,
            max_entries: limits.max_entries as u64,
            max_depth: limits.max_depth,
        },
        cancellation,
    )
    .map_err(|error| error.to_string())?;
    source.revalidate().map_err(|error| error.to_string())?;
    check()?;
    let digest = writer.digest.finalize();
    Ok(FetchedForkSource {
        request: request.clone(),
        archive_digest: format!(
            "sha256:{}",
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ),
        tree_identity: tree.tree_identity,
        source,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::skill_dotagents_ledger::DotagentsDetachIntent;
    use flate2::{write::GzEncoder, Compression};
    use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

    pub(crate) fn fetch_fixture(
        staging: &Path,
        request: &DotagentsForkSource,
        content: &[u8],
    ) -> FetchedForkSource {
        let temp = tempfile::tempdir().unwrap();
        let gzip = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = tar::Builder::new(gzip);
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                Path::new("repo").join(request.path()).join("SKILL.md"),
                content,
            )
            .unwrap();
        let bytes = builder.into_inner().unwrap().finish().unwrap();
        let archive = temp.path().join("archive.gz");
        fs::write(&archive, &bytes).unwrap();
        let executable = temp.path().join("gh-fixture");
        let archive_quote = archive.to_str().unwrap().replace('\'', "'\"'\"'");
        let script = format!("#!/bin/sh\n[ \"$#\" -eq 4 ] || exit 11\n[ \"$1\" = api ] || exit 12\n[ \"$2\" = --hostname ] || exit 13\n[ \"$3\" = github.com ] || exit 14\n[ \"$4\" = 'repos/{}/tarball/{}' ] || exit 15\nexec /bin/cat '{archive_quote}'\n", request.repo(), request.commit());
        fs::write(&executable, script).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let result = fetch_fork_source(
            request,
            &executable,
            staging,
            ArchiveLimits::default(),
            Instant::now() + Duration::from_secs(3),
            &CancellationToken::default(),
        )
        .unwrap();
        let expected = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(result.archive_digest(), format!("sha256:{expected}"));
        result
    }

    fn request(path: &str) -> DotagentsForkSource {
        let lock = format!("[skills.alpha]\nsource = 'owner/repo'\nresolved_commit = '{}'\nresolved_path = '{path}'\n", "a".repeat(40));
        DotagentsDetachIntent::from_documents(
            "alpha",
            &lock,
            "[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\n",
        )
        .unwrap()
        .fork_source()
        .unwrap()
    }

    #[test]
    fn binds_request_digest_root_and_nested_source_handles() {
        for path in ["", "skills/alpha"] {
            let temp = tempfile::tempdir().unwrap();
            let request = request(path);
            let fetched = fetch_fixture(temp.path(), &request, b"upstream");
            assert_eq!(fetched.request(), &request);
            assert!(fetched.tree_identity().starts_with("tree-v1:"));
            assert_eq!(
                fs::read(fetched.path().join("SKILL.md")).unwrap(),
                b"upstream"
            );
            let (source, _) = fetched.into_matching_source(&request).unwrap();
            source.revalidate().unwrap();
        }
    }

    #[test]
    fn refuses_cancelled_or_nonempty_staging_before_executing() {
        let temp = tempfile::tempdir().unwrap();
        let cancellation = CancellationToken::default();
        cancellation.cancel();
        let result = fetch_fork_source(
            &request("skills/alpha"),
            Path::new("/missing"),
            temp.path(),
            ArchiveLimits::default(),
            Instant::now() + Duration::from_secs(3),
            &cancellation,
        );
        assert!(result.err().unwrap().contains("cancelled"));
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
        fs::write(temp.path().join("keep"), "original").unwrap();
        let result = fetch_fork_source(
            &request("skills/alpha"),
            Path::new("/missing"),
            temp.path(),
            ArchiveLimits::default(),
            Instant::now() + Duration::from_secs(3),
            &CancellationToken::default(),
        );
        assert!(result.err().unwrap().contains("empty"));
        assert_eq!(
            fs::read_to_string(temp.path().join("keep")).unwrap(),
            "original"
        );
    }
}
