//! A backup source name and its recorded path derived from one retained root.
use crate::{skill_backup_copy::valid_component, skill_scope::SkillReadScope};
use cap_std::fs::{Dir, DirBuilder, DirBuilderExt, MetadataExt};
use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

pub struct BackupSourceRoot {
    scope: Arc<SkillReadScope>,
    directory: Dir,
    path: PathBuf,
}

pub struct BackupSource {
    scope: Arc<SkillReadScope>,
    pub(crate) directory: Dir,
    pub(crate) name: OsString,
    pub(crate) original_path: PathBuf,
}

impl BackupSourceRoot {
    /// The service must authorize this root before binding it. Aliases retain
    /// their lexical identity and must continue to resolve to the bound root.
    pub fn bind(path: &Path) -> io::Result<Self> {
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Backup source root must be an absolute normalized path",
            ));
        }
        let scope =
            Arc::new(SkillReadScope::bind(&[path.to_path_buf()]).map_err(io::Error::other)?);
        let directory = scope.clone_bound_directory(path)?.into();
        Ok(Self {
            scope,
            directory,
            path: path.to_path_buf(),
        })
    }

    /// Selects one child; its absence/presence is checked when copying.
    pub fn select(&self, name: &OsStr) -> io::Result<BackupSource> {
        if !valid_component(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Backup source must name one child",
            ));
        }
        let original_path = self.path.join(name);
        if original_path.to_str().is_none_or(|path| path.len() > 4096) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Backup source path must be UTF-8 and at most 4096 bytes",
            ));
        }
        let selected = BackupSource {
            scope: self.scope.clone(),
            directory: self.directory.try_clone()?,
            name: name.to_owned(),
            original_path,
        };
        selected.revalidate()?;
        Ok(selected)
    }
}

impl BackupSource {
    pub(crate) fn create_directory(&self) -> io::Result<()> {
        self.revalidate()?;
        match self.directory.symlink_metadata(&self.name) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Destination is occupied",
                ))
            }
        }
        self.directory
            .create_dir_with(&self.name, DirBuilder::new().mode(0o700))?;
        self.directory.open(".")?.sync_all()?;
        self.private_directory_device()?
            .ok_or_else(|| io::Error::other("Created private directory is missing"))?;
        Ok(())
    }

    pub(crate) fn private_directory_device(&self) -> io::Result<Option<u64>> {
        self.revalidate()?;
        match self.directory.symlink_metadata(&self.name) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
            Ok(metadata)
                if metadata.is_dir()
                    && !metadata.file_type().is_symlink()
                    && metadata.mode() & 0o777 == 0o700 =>
            {
                Ok(Some(metadata.dev()))
            }
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                Err(io::Error::other("Expected private directory permissions"))
            }
            Ok(_) => Err(io::Error::other("Expected path is not a directory")),
        }
    }

    pub(crate) fn exact_symlink_target(&self) -> io::Result<Option<PathBuf>> {
        self.revalidate()?;
        match self.directory.symlink_metadata(&self.name) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
            Ok(metadata) if metadata.file_type().is_symlink() => {
                self.directory.read_link_contents(&self.name).map(Some)
            }
            Ok(_) => Err(io::Error::other("Expected path is not a symlink")),
        }
    }

    pub(crate) fn move_exact_symlink_to(
        &self,
        destination: &BackupSource,
        expected: &Path,
    ) -> io::Result<()> {
        self.revalidate()?;
        destination.revalidate()?;
        if self.exact_symlink_target()?.as_deref() != Some(expected) {
            return Err(io::Error::other("Symlink target changed"));
        }
        if destination.exact_symlink_target()?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Symlink stage is occupied",
            ));
        }
        rustix::fs::renameat_with(
            &self.directory,
            &self.name,
            &destination.directory,
            &destination.name,
            rustix::fs::RenameFlags::NOREPLACE,
        )?;
        let moved = destination.exact_symlink_target();
        if moved.as_ref().ok().and_then(|target| target.as_deref()) != Some(expected) {
            let _ = rustix::fs::renameat_with(
                &destination.directory,
                &destination.name,
                &self.directory,
                &self.name,
                rustix::fs::RenameFlags::NOREPLACE,
            );
            return Err(moved
                .err()
                .unwrap_or_else(|| io::Error::other("Moved symlink target changed")));
        }
        for parent in [&self.directory, &destination.directory] {
            parent.open(".")?.sync_all()?;
        }
        self.revalidate()?;
        destination.revalidate()
    }

    pub(crate) fn restore_exact_symlink_from(
        &self,
        stage: &BackupSource,
        expected: &Path,
    ) -> io::Result<()> {
        self.revalidate()?;
        stage.revalidate()?;
        match self.exact_symlink_target()? {
            Some(actual) if actual == expected => return Ok(()),
            Some(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Symlink path is occupied",
                ))
            }
            None => {}
        }
        match stage.exact_symlink_target()? {
            Some(actual) if actual == expected => {
                rustix::fs::renameat_with(
                    &stage.directory,
                    &stage.name,
                    &self.directory,
                    &self.name,
                    rustix::fs::RenameFlags::NOREPLACE,
                )?;
            }
            Some(_) => return Err(io::Error::other("Staged symlink target changed")),
            None => return self.restore_absent_symlink(expected),
        }
        for parent in [&self.directory, &stage.directory] {
            parent.open(".")?.sync_all()?;
        }
        self.revalidate()?;
        stage.revalidate()
    }

    pub(crate) fn restore_absent_symlink(&self, target: &Path) -> io::Result<()> {
        self.revalidate()?;
        match self.directory.symlink_metadata(&self.name) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
            Ok(metadata)
                if metadata.file_type().is_symlink()
                    && self.directory.read_link_contents(&self.name)? == target =>
            {
                return Ok(())
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Symlink path is occupied",
                ))
            }
        }
        self.directory.symlink_contents(target, &self.name)?;
        self.directory.open(".")?.sync_all()?;
        self.revalidate()
    }

    pub(crate) fn resolved_path(&self) -> io::Result<PathBuf> {
        self.revalidate()?;
        let parent = self
            .original_path
            .parent()
            .ok_or_else(|| io::Error::other("Backup source has no parent"))?;
        Ok(self
            .scope
            .resolved_dir_path(parent)
            .map_err(io::Error::other)?
            .join(&self.name))
    }

    pub(crate) fn revalidate(&self) -> io::Result<()> {
        self.scope.revalidate_roots().map_err(io::Error::other)?;
        let parent = self
            .original_path
            .parent()
            .ok_or_else(|| io::Error::other("Missing source parent"))?;
        let (_, current) = self
            .scope
            .resolved_path_metadata(parent)
            .map_err(io::Error::other)?;
        let retained = self.directory.dir_metadata()?;
        if (current.dev(), current.ino()) != (retained.dev(), retained.ino()) {
            return Err(io::Error::other("Backup source parent changed"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn absolute_symlink_targets_round_trip_and_repointing_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root_path = temp.path().canonicalize().unwrap();
        let expected = root_path.join("expected-target");
        let replacement = root_path.join("replacement-target");
        std::fs::create_dir(&expected).unwrap();
        std::fs::create_dir(&replacement).unwrap();
        symlink(&expected, root_path.join("reader")).unwrap();
        let root = BackupSourceRoot::bind(&root_path).unwrap();
        let reader = root.select(OsStr::new("reader")).unwrap();
        let stage = root.select(OsStr::new("stage")).unwrap();

        assert_eq!(
            reader.exact_symlink_target().unwrap(),
            Some(expected.clone())
        );
        reader.move_exact_symlink_to(&stage, &expected).unwrap();
        assert_eq!(
            stage.exact_symlink_target().unwrap(),
            Some(expected.clone())
        );
        reader
            .restore_exact_symlink_from(&stage, &expected)
            .unwrap();
        assert_eq!(
            std::fs::read_link(root_path.join("reader")).unwrap(),
            expected
        );

        std::fs::remove_file(root_path.join("reader")).unwrap();
        reader.restore_absent_symlink(&expected).unwrap();
        std::fs::remove_file(root_path.join("reader")).unwrap();
        symlink(&replacement, root_path.join("reader")).unwrap();
        assert_eq!(
            reader.restore_absent_symlink(&expected).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(reader.move_exact_symlink_to(&stage, &expected).is_err());
        assert_eq!(
            std::fs::read_link(root_path.join("reader")).unwrap(),
            replacement
        );
    }
}
