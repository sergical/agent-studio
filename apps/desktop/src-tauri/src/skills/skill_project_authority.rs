use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const CONFIG_FILE: &str = "skill-studio-projects.json";
const LOCK_FILE: &str = "skill-studio-projects.lock";
const CONFIG_LIMIT: u64 = 64 * 1024;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
struct ProjectAuthority {
    tracked: Vec<PathBuf>,
    excluded: Vec<PathBuf>,
}

fn config_path(home: &Path) -> PathBuf {
    home.join(".agents").join(CONFIG_FILE)
}

fn lock_path(home: &Path) -> PathBuf {
    home.join(".agents").join(LOCK_FILE)
}

fn same_directory(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn validate_roots(home: &Path, roots: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    for root in &roots {
        if !root.is_absolute()
            || root.parent().is_none()
            || same_directory(root, home)
            || root
                .components()
                .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        {
            return Err(format!(
                "Tracked projects require absolute normalized directories below the filesystem root: {}",
                root.display()
            ));
        }
    }
    Ok(roots
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

fn validate(home: &Path, authority: ProjectAuthority) -> Result<ProjectAuthority, String> {
    let tracked = validate_roots(home, authority.tracked)?;
    let excluded = validate_roots(home, authority.excluded)?;
    if tracked
        .iter()
        .any(|root| excluded.iter().any(|path| same_directory(root, path)))
    {
        return Err("Tracked project settings cannot include and exclude the same path".into());
    }
    Ok(ProjectAuthority { tracked, excluded })
}

fn acquire_update_lock(home: &Path, timeout: Duration) -> Result<File, String> {
    let path = lock_path(home);
    let parent = path
        .parent()
        .ok_or("Tracked project lock path has no parent")?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Couldn't create tracked project settings folder: {error}"))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
        .map_err(|error| {
            format!(
                "Couldn't open tracked project lock {}: {error}",
                path.display()
            )
        })?;
    let opened = file
        .metadata()
        .map_err(|error| format!("Couldn't inspect tracked project lock: {error}"))?;
    if !opened.is_file() {
        return Err("Tracked project lock must be a regular file".into());
    }
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err("Tracked project settings are busy".into());
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(format!("Couldn't lock tracked project settings: {error}"));
            }
        }
    }
    let current = std::fs::symlink_metadata(&path)
        .map_err(|error| format!("Couldn't recheck tracked project lock: {error}"))?;
    if !current.is_file() || opened.dev() != current.dev() || opened.ino() != current.ino() {
        return Err("Tracked project lock changed during acquisition".into());
    }
    Ok(file)
}

fn read_unlocked(home: &Path) -> Result<ProjectAuthority, String> {
    let path = config_path(home);
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProjectAuthority::default());
        }
        Err(error) => {
            return Err(format!(
                "Couldn't open tracked project settings {}: {error}",
                path.display()
            ));
        }
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("Couldn't inspect tracked project settings: {error}"))?;
    if !metadata.is_file() || metadata.len() > CONFIG_LIMIT {
        return Err("Tracked project settings must be a regular file no larger than 64 KiB".into());
    }
    let mut bytes = Vec::new();
    file.take(CONFIG_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Couldn't read tracked project settings: {error}"))?;
    if bytes.len() as u64 > CONFIG_LIMIT {
        return Err("Tracked project settings exceed 64 KiB".into());
    }
    let authority: ProjectAuthority = serde_json::from_slice(&bytes)
        .map_err(|_| "Tracked project settings must contain only tracked and excluded arrays")?;
    validate(home, authority)
}

fn write_unlocked(home: &Path, authority: &ProjectAuthority) -> Result<(), String> {
    let path = config_path(home);
    let parent = path
        .parent()
        .ok_or("Tracked project settings path has no parent")?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Couldn't create tracked project settings folder: {error}"))?;
    if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err("Tracked project settings cannot be a symbolic link".into());
    }
    let bytes = serde_json::to_vec_pretty(authority).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > CONFIG_LIMIT {
        return Err("Tracked project settings exceed 64 KiB".into());
    }
    let temporary = parent.join(format!(
        ".{CONFIG_FILE}.{}.{}.tmp",
        std::process::id(),
        WRITE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| format!("Couldn't prepare tracked project settings: {error}"))?;
    let result = (|| -> std::io::Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|error| format!("Couldn't save tracked project settings: {error}"))
}

fn update(
    home: &Path,
    change: impl FnOnce(&mut BTreeSet<PathBuf>, &mut BTreeSet<PathBuf>),
) -> Result<(), String> {
    let _lock = acquire_update_lock(home, LOCK_TIMEOUT)?;
    let before = read_unlocked(home)?;
    let mut tracked = before.tracked.iter().cloned().collect();
    let mut excluded = before.excluded.iter().cloned().collect();
    change(&mut tracked, &mut excluded);
    let after = validate(
        home,
        ProjectAuthority {
            tracked: tracked.into_iter().collect(),
            excluded: excluded.into_iter().collect(),
        },
    )?;
    if after != before {
        write_unlocked(home, &after)?;
    }
    Ok(())
}

pub(crate) fn track(home: &Path, paths: Vec<String>) -> Result<Vec<String>, String> {
    let paths = validate_roots(home, paths.into_iter().map(PathBuf::from).collect())?;
    update(home, |tracked, excluded| {
        for path in &paths {
            excluded.retain(|excluded| !same_directory(excluded, path));
            tracked.insert(path.clone());
        }
    })?;
    Ok(paths
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect())
}

pub(crate) fn exclude(home: &Path, path: &str) -> Result<(), String> {
    let path = PathBuf::from(path);
    if same_directory(&path, home) {
        return Ok(());
    }
    let path = validate_roots(home, vec![path])?
        .pop()
        .ok_or("Project path is unavailable")?;
    let excluded_path = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    update(home, |tracked, excluded| {
        tracked.retain(|tracked| !same_directory(tracked, &path));
        excluded.retain(|excluded| !same_directory(excluded, &path));
        excluded.insert(excluded_path);
    })
}

pub(crate) fn scoped_projects(
    home: &Path,
    extra: impl IntoIterator<Item = PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let authority = read_unlocked(home)?;
    let mut projects: BTreeSet<_> = super::project_discovery::discover_skill_projects(home)
        .into_iter()
        .collect();
    projects.extend(authority.tracked);
    projects.extend(extra);
    projects.retain(|path| {
        !same_directory(path, home)
            && !authority
                .excluded
                .iter()
                .any(|excluded| same_directory(path, excluded))
    });
    projects
        .into_iter()
        .map(|path| {
            validate_roots(home, vec![path])?
                .pop()
                .ok_or_else(|| "Project path is unavailable".to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn tracked_and_excluded_projects_round_trip_without_event_authority() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("outside-project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&project).unwrap();

        track(home.as_path(), vec![project.to_string_lossy().into_owned()]).unwrap();
        let scoped = scoped_projects(&home, []).unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0], project);
        exclude(&home, project.to_str().unwrap()).unwrap();
        assert!(scoped_projects(&home, []).unwrap().is_empty());
        assert!(scoped_projects(&home, [project.clone()])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn alias_exclusion_applies_to_discovery_tracking_and_extra_scope() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        let alias = temp.path().join("project-alias");
        std::fs::create_dir_all(project.join(".codex/skills")).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::os::unix::fs::symlink(&project, &alias).unwrap();
        std::fs::write(
            home.join(".codex/config.toml"),
            format!(
                "[projects.\"{}\"]\ntrusted = true\n",
                alias.to_string_lossy()
            ),
        )
        .unwrap();
        track(&home, vec![project.to_string_lossy().into_owned()]).unwrap();
        assert!(super::super::project_discovery::discover_skill_projects(&home).contains(&alias));
        let persisted = read_unlocked(&home).unwrap();
        assert_eq!(persisted.tracked.len(), 1);
        assert_eq!(persisted.tracked[0], project);

        exclude(&home, alias.to_str().unwrap()).unwrap();

        assert!(scoped_projects(&home, [project.clone()])
            .unwrap()
            .is_empty());
        let persisted = read_unlocked(&home).unwrap();
        assert!(persisted.tracked.is_empty());
        assert_eq!(
            persisted.excluded,
            [std::fs::canonicalize(&project).unwrap()]
        );
        std::fs::remove_file(alias).unwrap();
        assert!(scoped_projects(&home, [project]).unwrap().is_empty());
    }

    #[test]
    fn malformed_or_linked_settings_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let agents = home.join(".agents");
        std::fs::create_dir_all(&agents).unwrap();
        let path = config_path(&home);
        std::fs::write(&path, b"not json").unwrap();
        assert!(scoped_projects(&home, []).is_err());
        std::fs::remove_file(&path).unwrap();
        let target = temp.path().join("target.json");
        std::fs::write(&target, b"{}").unwrap();
        std::os::unix::fs::symlink(target, path).unwrap();
        assert!(scoped_projects(&home, []).is_err());
    }

    #[test]
    fn non_regular_settings_fail_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let path = config_path(&home);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);

        assert!(scoped_projects(&home, []).is_err());
    }

    #[test]
    fn linked_or_nonregular_lock_fails_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        let path = lock_path(&home);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let target = temp.path().join("lock-target");
        std::fs::write(&target, b"").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(track(&home, vec![project.to_string_lossy().into_owned()]).is_err());
        std::fs::remove_file(&path).unwrap();
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(track(&home, vec![project.to_string_lossy().into_owned()]).is_err());
    }

    #[test]
    fn sidecar_lock_excludes_another_process() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let _lock = acquire_update_lock(&home, Duration::from_secs(1)).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "skills::skill_project_authority::tests::sidecar_lock_child",
                "--ignored",
                "--nocapture",
            ])
            .env("SKILL_STUDIO_PROJECT_LOCK_CHILD_HOME", &home)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "subprocess helper"]
    fn sidecar_lock_child() {
        let Some(home) = std::env::var_os("SKILL_STUDIO_PROJECT_LOCK_CHILD_HOME") else {
            return;
        };
        let error = acquire_update_lock(Path::new(&home), Duration::from_millis(100)).unwrap_err();
        assert!(error.contains("busy"), "{error}");
    }
}
