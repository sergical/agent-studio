//! Locates an already extracted repository. This does not confine archive extraction.
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn locate_repository_directory(extracted: &Path, path: &str) -> Result<PathBuf, String> {
    if !matches!(path, "" | ".")
        && (path.contains(['\\', '\0'])
            || path.split('/').any(|part| matches!(part, "" | "." | "..")))
    {
        return Err("Refusing a path outside the repository layout".into());
    }
    let mut entries = fs::read_dir(extracted).map_err(|error| error.to_string())?;
    let top = entries
        .next()
        .ok_or("Archive has no repository root")?
        .map_err(|error| error.to_string())?;
    if entries
        .next()
        .transpose()
        .map_err(|error| error.to_string())?
        .is_some()
        || !top.file_type().map_err(|error| error.to_string())?.is_dir()
    {
        return Err("Archive must contain exactly one directory root".into());
    }
    let root = fs::canonicalize(top.path()).map_err(|error| error.to_string())?;
    let candidate = fs::canonicalize(root.join(path))
        .map_err(|_| "Requested directory was not found in the repository".to_string())?;
    if !candidate.starts_with(&root) || !candidate.is_dir() {
        return Err("Refusing a directory outside the repository root".into());
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_one_repository_root_and_refuses_ambiguous_or_escaping_layouts() {
        let temp = tempfile::tempdir().unwrap();
        let extracted = temp.path().join("extracted");
        let root = extracted.join("owner-repo-commit");
        fs::create_dir_all(root.join("skills/alpha")).unwrap();
        fs::write(root.join("skills/alpha/SKILL.md"), "body").unwrap();
        let canonical = fs::canonicalize(&root).unwrap();
        assert_eq!(
            locate_repository_directory(&extracted, "skills/alpha").unwrap(),
            canonical.join("skills/alpha")
        );
        for path in ["", "."] {
            assert_eq!(
                locate_repository_directory(&extracted, path).unwrap(),
                canonical
            );
        }
        for path in [
            "..",
            "../sibling",
            "/absolute",
            "skills/../alpha",
            "skills//alpha",
            "skills/alpha/SKILL.md",
            "skills\\alpha",
        ] {
            assert!(
                locate_repository_directory(&extracted, path).is_err(),
                "{path}"
            );
        }
        fs::create_dir(extracted.join("sibling")).unwrap();
        assert!(locate_repository_directory(&extracted, "skills/alpha").is_err());
        fs::remove_dir(extracted.join("sibling")).unwrap();
        fs::write(extracted.join("extra-file"), "extra").unwrap();
        assert!(locate_repository_directory(&extracted, "skills/alpha").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_linked_root_and_subdirectory_escape() {
        let temp = tempfile::tempdir().unwrap();
        let extracted = temp.path().join("extracted");
        let outside = temp.path().join("outside");
        fs::create_dir(&extracted).unwrap();
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, extracted.join("root")).unwrap();
        assert!(locate_repository_directory(&extracted, "").is_err());
        fs::remove_file(extracted.join("root")).unwrap();
        fs::create_dir(extracted.join("root")).unwrap();
        std::os::unix::fs::symlink(&outside, extracted.join("root/escape")).unwrap();
        assert!(locate_repository_directory(&extracted, "escape").is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    }
}
