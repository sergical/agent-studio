//! Streaming extraction into caller-owned, empty staging storage.
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions, Permissions};
use flate2::read::MultiGzDecoder;
use std::{
    collections::HashSet,
    ffi::OsStr,
    io::{self, Read, Write},
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug)]
pub struct ArchiveLimits {
    pub max_compressed_bytes: u64,
    pub max_expanded_bytes: u64,
    pub max_file_bytes: u64,
    pub max_entries: usize,
    pub max_depth: usize,
    pub max_path_bytes: usize,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_compressed_bytes: 64 * 1024 * 1024,
            max_expanded_bytes: 256 * 1024 * 1024,
            max_file_bytes: 64 * 1024 * 1024,
            max_entries: 20_000,
            max_depth: 64,
            max_path_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub struct ArchiveReport {
    pub repository_root: PathBuf,
    pub file_bytes: u64,
    pub entries: usize,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

struct CheckedReader<'a, R, F> {
    inner: R,
    remaining: u64,
    check: &'a F,
}
impl<R: Read, F: Fn() -> io::Result<()>> Read for CheckedReader<'_, R, F> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        (self.check)()?;
        if output.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            return match self.inner.read(&mut [0])? {
                0 => Ok(0),
                _ => Err(invalid("Archive stream byte limit exceeded")),
            };
        }
        let count = output.len().min(self.remaining.min(64 * 1024) as usize);
        let read = self.inner.read(&mut output[..count])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}

fn entry_path(bytes: &[u8], directory: bool, depth: usize) -> io::Result<PathBuf> {
    let bytes = if directory {
        bytes.strip_suffix(b"/").unwrap_or(bytes)
    } else {
        bytes
    };
    if bytes.len() > 4096 {
        return Err(invalid("Archive entry path is too long"));
    }
    let components = bytes.split(|byte| *byte == b'/').collect::<Vec<_>>();
    if components.len() > depth.min(128)
        || components.iter().any(|part| {
            part.is_empty()
                || *part == b"."
                || *part == b".."
                || part.len() > 255
                || part.contains(&0)
                || part.contains(&b'\\')
        })
    {
        return Err(invalid("Unsafe archive entry path"));
    }
    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

fn charge(total: &mut usize, count: usize, limit: usize) -> io::Result<()> {
    *total = total
        .checked_add(count)
        .filter(|value| *value <= limit)
        .ok_or_else(|| invalid("Archive entry or path budget exceeded"))?;
    Ok(())
}

fn directory(root: &Dir, path: &Path, entries: &mut usize, limit: usize) -> io::Result<Dir> {
    let mut current = root.try_clone()?;
    for component in path.components() {
        let name = component.as_os_str();
        match current.symlink_metadata(name) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                charge(entries, 1, limit)?;
                current.create_dir(name)?;
            }
            Err(error) => return Err(error),
        }
        current = current.open_dir_nofollow(name)?;
    }
    Ok(current)
}

fn validate_link(path: &Path, target: &[u8]) -> io::Result<()> {
    if target.is_empty()
        || target.len() > 4096
        || target.starts_with(b"/")
        || target.contains(&0)
        || target.contains(&b'\\')
    {
        return Err(invalid("Unsafe archive symlink target"));
    }
    let mut depth = path.components().count() - 1;
    for part in target.split(|byte| *byte == b'/') {
        match part {
            b"" | b"." => {}
            b".." if depth > 1 => depth -= 1,
            b".." => return Err(invalid("Archive symlink escapes repository")),
            _ => depth += 1,
        }
    }
    Ok(())
}

/// The caller must own staging exclusively and discard it on any error. Reads must
/// be interruptible by the caller if the underlying input can block indefinitely.
pub fn extract_gzip<R: Read, F: Fn() -> io::Result<()>>(
    input: R,
    staging: &Dir,
    limits: ArchiveLimits,
    check: F,
) -> io::Result<ArchiveReport> {
    check()?;
    if staging.entries()?.next().transpose()?.is_some() {
        return Err(invalid("Archive staging directory must be empty"));
    }
    let compressed = CheckedReader {
        inner: input,
        remaining: limits.max_compressed_bytes,
        check: &check,
    };
    let expanded = CheckedReader {
        inner: MultiGzDecoder::new(compressed),
        remaining: limits.max_expanded_bytes,
        check: &check,
    };
    let mut archive = tar::Archive::new(expanded);
    let mut paths = HashSet::new();
    let mut path_bytes = 0;
    let mut entries = 0;
    let mut headers = 0;
    let mut file_bytes = 0_u64;
    let mut repository_root: Option<PathBuf> = None;
    let mut directory_modes = Vec::new();
    for entry in archive.entries()? {
        check()?;
        charge(&mut headers, 1, limits.max_entries)?;
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if kind.is_pax_global_extensions() {
            io::copy(&mut entry, &mut io::sink())?;
            continue;
        }
        if !kind.is_file() && !kind.is_dir() && !kind.is_symlink() {
            return Err(invalid("Unsupported archive entry type"));
        }
        let path = entry_path(&entry.path_bytes(), kind.is_dir(), limits.max_depth)?;
        charge(
            &mut path_bytes,
            path.as_os_str().len(),
            limits.max_path_bytes,
        )?;
        if !paths.insert(path.clone()) {
            return Err(invalid("Duplicate archive entry"));
        }
        let root = PathBuf::from(path.components().next().unwrap().as_os_str());
        if repository_root
            .as_ref()
            .is_some_and(|expected| expected != &root)
            || (path == root && !kind.is_dir())
        {
            return Err(invalid("Archive must contain one repository directory"));
        }
        repository_root = Some(root);
        if kind.is_dir() {
            if entry.size() != 0 {
                return Err(invalid("Archive directory contains data"));
            }
            directory(staging, &path, &mut entries, limits.max_entries)?;
            directory_modes.push((path, entry.header().mode()? & 0o777));
            continue;
        }
        if entry.size() > limits.max_file_bytes {
            return Err(invalid("Archive file byte limit exceeded"));
        }
        let parent = directory(
            staging,
            path.parent().unwrap(),
            &mut entries,
            limits.max_entries,
        )?;
        let name = path.file_name().unwrap();
        charge(&mut entries, 1, limits.max_entries)?;
        if kind.is_symlink() {
            if entry.size() != 0 {
                return Err(invalid("Archive symlink contains data"));
            }
            let target = entry
                .link_name_bytes()
                .ok_or_else(|| invalid("Missing symlink target"))?;
            validate_link(&path, &target)?;
            charge(&mut path_bytes, target.len(), limits.max_path_bytes)?;
            parent.symlink_contents(Path::new(OsStr::from_bytes(&target)), name)?;
        } else {
            let mut options = OpenOptions::new();
            options
                .write(true)
                .create_new(true)
                .follow(FollowSymlinks::No);
            let mut output = parent.open_with(name, &options)?;
            let mut buffer = [0_u8; 64 * 1024];
            let mut copied = 0;
            loop {
                check()?;
                let count = entry.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                output.write_all(&buffer[..count])?;
                copied += count as u64;
            }
            if copied != entry.size() {
                return Err(invalid("Truncated archive file"));
            }
            file_bytes = file_bytes
                .checked_add(copied)
                .ok_or_else(|| invalid("Archive size overflow"))?;
            output.set_permissions(Permissions::from_std(std::fs::Permissions::from_mode(
                entry.header().mode()? & 0o777,
            )))?;
        }
    }
    // Consume the gzip trailer to check its checksum and reject a hidden second tar.
    let mut rest = archive.into_inner();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = rest.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if buffer[..read].iter().any(|byte| *byte != 0) {
            return Err(invalid("Unexpected data after archive end"));
        }
    }
    directory_modes.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (path, mode) in directory_modes {
        check()?;
        directory(staging, &path, &mut entries, limits.max_entries)?.set_permissions(
            ".",
            Permissions::from_std(std::fs::Permissions::from_mode(mode)),
        )?;
    }
    check()?;
    Ok(ArchiveReport {
        repository_root: repository_root
            .ok_or_else(|| invalid("Archive has no repository root"))?,
        file_bytes,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use std::cell::Cell;

    struct Member<'a> {
        path: &'a str,
        kind: u8,
        content: &'a [u8],
        link: Option<&'a str>,
    }
    fn file(path: &str) -> Member<'_> {
        Member {
            path,
            kind: b'0',
            content: b"skill body",
            link: None,
        }
    }
    fn link<'a>(path: &'a str, target: &'a str) -> Member<'a> {
        Member {
            path,
            kind: b'2',
            content: b"",
            link: Some(target),
        }
    }
    fn archive(members: &[Member<'_>]) -> Vec<u8> {
        let gzip = GzEncoder::new(Vec::new(), Compression::default());
        let mut tar = tar::Builder::new(gzip);
        for member in members {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::new(member.kind));
            header.set_size(member.content.len() as u64);
            header.set_mode(if member.kind == b'5' { 0o750 } else { 0o755 });
            if let Some(target) = member.link {
                header.set_link_name(target).unwrap();
            }
            // Raw names deliberately bypass the builder's path validation.
            assert!(member.path.len() < 100);
            header.as_mut_bytes()[..member.path.len()].copy_from_slice(member.path.as_bytes());
            header.set_cksum();
            tar.append(&header, member.content).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }
    fn staging(temp: &tempfile::TempDir) -> Dir {
        Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap()
    }

    #[test]
    fn extracts_files_empty_directories_modes_and_relative_links() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = archive(&[
            file("repo/skills/SKILL.md"),
            Member {
                path: "repo/empty/",
                kind: b'5',
                content: b"",
                link: None,
            },
            link("repo/empty/readme", "../skills/SKILL.md"),
        ]);
        let report = extract_gzip(
            &bytes[..],
            &staging(&temp),
            ArchiveLimits::default(),
            || Ok(()),
        )
        .unwrap();
        assert_eq!(report.repository_root, Path::new("repo"));
        assert_eq!(report.entries, 5);
        assert_eq!(
            std::fs::metadata(temp.path().join("repo/empty"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o750
        );
        assert_eq!(report.file_bytes, 10);
        assert_eq!(
            std::fs::read(temp.path().join("repo/skills/SKILL.md")).unwrap(),
            b"skill body"
        );
        assert_eq!(
            std::fs::metadata(temp.path().join("repo/skills/SKILL.md"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            std::fs::read_link(temp.path().join("repo/empty/readme")).unwrap(),
            Path::new("../skills/SKILL.md")
        );
        assert!(extract_gzip(
            &bytes[..],
            &staging(&temp),
            ArchiveLimits::default(),
            || Ok(())
        )
        .is_err());
    }

    #[test]
    fn rejects_unsafe_paths_types_duplicates_and_link_parents() {
        for members in [
            vec![file("../escape")],
            vec![file("/escape")],
            vec![file("repo/../escape")],
            vec![file("repo\\escape")],
            vec![file("repo/a"), file("repo/a")],
            vec![file("repo/a"), file("other/a")],
            vec![link("repo/link", "../../escape")],
            vec![link("repo/link", "/tmp/escape")],
            vec![link("repo/link", "sub"), file("repo/link/escape")],
            vec![Member {
                path: "repo/device",
                kind: b'3',
                content: b"",
                link: None,
            }],
            vec![Member {
                path: "repo/hard",
                kind: b'1',
                content: b"",
                link: Some("repo/a"),
            }],
        ] {
            let temp = tempfile::tempdir().unwrap();
            let stage = temp.path().join("stage");
            std::fs::create_dir(&stage).unwrap();
            std::fs::write(temp.path().join("escape"), "untouched").unwrap();
            let dir = Dir::open_ambient_dir(&stage, cap_std::ambient_authority()).unwrap();
            assert!(extract_gzip(
                &archive(&members)[..],
                &dir,
                ArchiveLimits::default(),
                || Ok(())
            )
            .is_err());
            assert_eq!(
                std::fs::read_to_string(temp.path().join("escape")).unwrap(),
                "untouched"
            );
        }
    }

    #[test]
    fn enforces_stream_file_entry_depth_path_and_cancellation_limits() {
        let body = vec![b'x'; 128 * 1024];
        let bytes = archive(&[Member {
            path: "repo/sub/large",
            kind: b'0',
            content: &body,
            link: None,
        }]);
        let defaults = ArchiveLimits::default();
        for limits in [
            ArchiveLimits {
                max_compressed_bytes: bytes.len() as u64 - 1,
                ..defaults
            },
            ArchiveLimits {
                max_expanded_bytes: 4096,
                ..defaults
            },
            ArchiveLimits {
                max_file_bytes: 4096,
                ..defaults
            },
            ArchiveLimits {
                max_entries: 1,
                ..defaults
            },
            ArchiveLimits {
                max_depth: 2,
                ..defaults
            },
            ArchiveLimits {
                max_path_bytes: 4,
                ..defaults
            },
        ] {
            let temp = tempfile::tempdir().unwrap();
            assert!(extract_gzip(&bytes[..], &staging(&temp), limits, || Ok(())).is_err());
        }
        let temp = tempfile::tempdir().unwrap();
        let calls = Cell::new(0);
        let error = extract_gzip(&bytes[..], &staging(&temp), defaults, || {
            calls.set(calls.get() + 1);
            if calls.get() > 12 {
                Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(calls.get() > 12);
    }

    #[test]
    fn accepts_gnu_long_names_and_global_pax_metadata() {
        let gzip = GzEncoder::new(Vec::new(), Compression::default());
        let mut tar = tar::Builder::new(gzip);
        let mut global = tar::Header::new_ustar();
        let metadata = b"19 comment=fixture\n";
        global.set_entry_type(tar::EntryType::new(b'g'));
        global.set_size(metadata.len() as u64);
        global.set_mode(0o644);
        global.set_cksum();
        tar.append_data(&mut global, "pax_global_header", &metadata[..])
            .unwrap();
        let path = format!("repo/{}/SKILL.md", "a".repeat(180));
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o4755);
        header.set_cksum();
        tar.append_data(&mut header, &path, &b"body"[..]).unwrap();
        let bytes = tar.into_inner().unwrap().finish().unwrap();
        let temp = tempfile::tempdir().unwrap();
        extract_gzip(
            &bytes[..],
            &staging(&temp),
            ArchiveLimits::default(),
            || Ok(()),
        )
        .unwrap();
        assert_eq!(std::fs::read(temp.path().join(&path)).unwrap(), b"body");
        assert_eq!(
            std::fs::metadata(temp.path().join(path))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o755
        );
    }

    #[test]
    fn validates_gzip_trailer_and_rejects_a_second_archive() {
        let bytes = archive(&[file("repo/SKILL.md")]);
        let mut corrupt = bytes.clone();
        let length = corrupt.len();
        corrupt[length - 8] ^= 1;
        let mut concatenated = bytes.clone();
        concatenated.extend_from_slice(&bytes);
        for input in [&bytes[..bytes.len() - 1], &corrupt[..], &concatenated[..]] {
            let temp = tempfile::tempdir().unwrap();
            assert!(
                extract_gzip(input, &staging(&temp), ArchiveLimits::default(), || Ok(())).is_err()
            );
        }
    }
}
