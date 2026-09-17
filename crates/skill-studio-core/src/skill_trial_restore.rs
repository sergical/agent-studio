//! Restores one retained trial tree without registering it as a new trial.

use crate::{
    skill_backup_copy::{copy_entry, inspect_entry, BackupCopyLimits},
    skill_backup_source::{BackupSource, BackupSourceRoot},
    skill_coordination::{CancellationToken, CoordinationMode, CoordinationPlan, DirectoryEffect},
    skill_scope::SkillReadScope,
    skill_tree_move::TreeMoveFailure,
};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{OsStr, OsString},
    fs,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

const RESTORE_COORDINATION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialRestoreReceipt {
    pub name: String,
    pub target: PathBuf,
}

#[derive(Debug)]
pub enum TrialRestoreError {
    BeforePublication(String),
    PublicationUncertain { name: String, message: String },
    PublishedLinkFailure { name: String, message: String },
}

impl TrialRestoreError {
    pub fn publication_possible(&self) -> bool {
        !matches!(self, Self::BeforePublication(_))
    }
}

impl std::fmt::Display for TrialRestoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BeforePublication(message) => formatter.write_str(message),
            Self::PublicationUncertain { name, message } => write!(
                formatter,
                "Restore of {name} may have completed, but publication could not be verified: {message}"
            ),
            Self::PublishedLinkFailure { name, message } => write!(
                formatter,
                "{name} was restored, but its Claude Code link could not be prepared: {message}"
            ),
        }
    }
}

impl std::error::Error for TrialRestoreError {}

pub fn restore_trial_backup(
    home: &Path,
    trash_path: &str,
    limits: BackupCopyLimits,
) -> Result<TrialRestoreReceipt, TrialRestoreError> {
    let cancellation = CancellationToken::default();
    restore_trial_backup_with(
        home,
        trash_path,
        limits,
        RestoreControl::legacy(unique_stage_name(), &cancellation),
        |_, _| Ok(()),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RestoreEntryIdentity {
    pub device: u64,
    pub inode: u64,
}

pub(crate) struct RestoreControl<'a> {
    pub stage_name: OsString,
    pub expected_name: Option<&'a str>,
    pub expected_tree: Option<&'a str>,
    pub expected_stage: Option<RestoreEntryIdentity>,
    pub state_roots: &'a [PathBuf],
    pub timeout: Option<Duration>,
    pub cancellation: &'a CancellationToken,
}

impl<'a> RestoreControl<'a> {
    fn legacy(stage_name: OsString, cancellation: &'a CancellationToken) -> Self {
        Self {
            stage_name,
            expected_name: None,
            expected_tree: None,
            expected_stage: None,
            state_roots: &[],
            timeout: Some(RESTORE_COORDINATION_TIMEOUT),
            cancellation,
        }
    }
}

pub(crate) fn restore_trial_backup_with(
    home: &Path,
    trash_path: &str,
    limits: BackupCopyLimits,
    mut control: RestoreControl<'_>,
    mut checkpoint: impl FnMut(
        RestoreCheckpoint,
        &crate::skill_coordination::FinalizedWriteLease<'_>,
    ) -> Result<(), TrialRestoreError>,
) -> Result<TrialRestoreReceipt, TrialRestoreError> {
    let (trash_root, trash, name) = validate_trash_path(home, trash_path)?;
    if control
        .expected_name
        .is_some_and(|expected| expected != name)
    {
        return Err(before(
            "Retained backup name differs from the restore intent".into(),
        ));
    }
    let shared_requested = home.join(".agents/skills");
    fs::create_dir_all(&shared_requested).map_err(|error| {
        before(format!(
            "Failed to create {}: {error}",
            shared_requested.display()
        ))
    })?;
    let shared = fs::canonicalize(&shared_requested).map_err(|error| {
        before(format!(
            "Failed to resolve {}: {error}",
            shared_requested.display()
        ))
    })?;
    let target = shared.join(&name);
    let source_root =
        BackupSourceRoot::bind(&trash_root).map_err(|error| before(error.to_string()))?;
    let relative = trash
        .strip_prefix(&trash_root)
        .map_err(|_| before("Trash path is not under ~/.agents/skills-trash".into()))?;
    let source = source_root
        .select_relative(relative)
        .map_err(|error| before(error.to_string()))?;
    let legacy_expected = if control.expected_tree.is_none() {
        Some(
            inspect_entry(
                &source.directory,
                &source.name,
                limits,
                control.cancellation,
            )
            .map_err(|error| before(error.to_string()))?
            .tree_identity,
        )
    } else {
        None
    };
    let shared_root = BackupSourceRoot::bind(&shared).map_err(|error| before(error.to_string()))?;
    while control.stage_name == OsStr::new(&name) {
        control.stage_name = unique_stage_name();
    }
    let stage = shared.join(&control.stage_name);
    let mut scope_roots = vec![trash_root.clone(), shared.clone()];
    scope_roots.extend_from_slice(control.state_roots);
    scope_roots.sort();
    scope_roots.dedup();
    let restore_scope =
        SkillReadScope::bind(&scope_roots).map_err(|error| before(error.to_string()))?;
    let state_effects = || {
        control
            .state_roots
            .iter()
            .cloned()
            .map(|path| DirectoryEffect::tree(path, CoordinationMode::Exclusive))
    };
    let copy_plan = CoordinationPlan::new_cancellable(
        [
            DirectoryEffect::tree(trash.clone(), CoordinationMode::Exclusive),
            DirectoryEffect::entry(stage.clone(), CoordinationMode::Exclusive),
            DirectoryEffect::entry(target.clone(), CoordinationMode::Exclusive),
        ]
        .into_iter()
        .chain(state_effects())
        .collect(),
        control.timeout,
        control.cancellation.clone(),
    )
    .map_err(|error| before(error.to_string()))?;
    let copy_lease = copy_plan
        .acquire()
        .map_err(|error| before(error.to_string()))?
        .finalize_write(&restore_scope, &[])
        .map_err(|error| before(error.to_string()))?;
    checkpoint(RestoreCheckpoint::BeforeCopy, &copy_lease)?;
    source
        .revalidate()
        .map_err(|error| before(error.to_string()))?;
    let current = inspect_entry(
        &source.directory,
        &source.name,
        limits,
        control.cancellation,
    )
    .map_err(|error| phase_error(control.expected_stage, &name, error.to_string()))?;
    let expected_tree = control
        .expected_tree
        .or(legacy_expected.as_deref())
        .unwrap_or(current.tree_identity.as_str())
        .to_owned();
    if current.tree_identity != expected_tree {
        return Err(phase_error(
            control.expected_stage,
            &name,
            "Retained backup changed before the restore copy started".into(),
        ));
    }
    let target_metadata = fs::symlink_metadata(&target);
    let published = match target_metadata {
        Ok(metadata) => match control.expected_stage {
            Some(expected) if restore_identity(&metadata) == expected => {
                let published = shared_root
                    .select(OsStr::new(&name))
                    .map_err(|error| uncertain(&name, error.to_string()))?;
                let report = inspect_entry(
                    &published.directory,
                    &published.name,
                    limits,
                    control.cancellation,
                )
                .map_err(|error| uncertain(&name, error.to_string()))?;
                if report.tree_identity != expected_tree {
                    return Err(uncertain(
                        &name,
                        "Published restore target differs from its saved content".into(),
                    ));
                }
                true
            }
            Some(_) => {
                return Err(uncertain(
                    &name,
                    "Published restore target has a different physical identity".into(),
                ))
            }
            None => {
                return Err(before(format!(
                    "`{name}` already exists in `~/.agents/skills`"
                )))
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(before(error.to_string())),
    };
    if published {
        drop(copy_lease);
        prepare_claude_link(
            home,
            &shared_requested,
            &shared_root,
            &shared_root
                .select(OsStr::new(&name))
                .map_err(|error| uncertain(&name, error.to_string()))?,
            control.expected_stage.ok_or_else(|| {
                uncertain(
                    &name,
                    "Published restore has no saved staging identity".into(),
                )
            })?,
            &expected_tree,
            &name,
            limits,
            &control,
            |lease| checkpoint(RestoreCheckpoint::BeforeLink, lease),
        )
        .map_err(|message| TrialRestoreError::PublishedLinkFailure {
            name: name.clone(),
            message,
        })?;
        return Ok(TrialRestoreReceipt { name, target });
    }

    let stage_identity = match fs::symlink_metadata(&stage) {
        Ok(metadata) => match control.expected_stage {
            Some(expected) if restore_identity(&metadata) == expected => {
                let existing = shared_root
                    .select(&control.stage_name)
                    .map_err(|error| uncertain(&name, error.to_string()))?;
                let report = inspect_entry(
                    &existing.directory,
                    &existing.name,
                    limits,
                    control.cancellation,
                )
                .map_err(|error| uncertain(&name, error.to_string()))?;
                if report.tree_identity != expected_tree {
                    return Err(uncertain(
                        &name,
                        "Saved restore staging entry changed".into(),
                    ));
                }
                expected
            }
            Some(_) => {
                return Err(uncertain(
                    &name,
                    "Saved restore staging entry has a different physical identity".into(),
                ))
            }
            None => return Err(before("Restore staging entry already exists".into())),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if control.expected_stage.is_some() {
                return Err(uncertain(
                    &name,
                    "Saved restore staging entry is missing".into(),
                ));
            }
            let copied = copy_entry(
                &source.directory,
                &source.name,
                &shared_root
                    .directory()
                    .map_err(|error| before(error.to_string()))?,
                &control.stage_name,
                limits,
                control.cancellation,
            )
            .map_err(|error| before(error.to_string()))?;
            if copied.tree_identity != expected_tree {
                return Err(before(
                    "Restore staging copy differs from the retained backup".into(),
                ));
            }
            entry_identity(&stage).map_err(before)?
        }
        Err(error) => return Err(before(error.to_string())),
    };
    checkpoint(RestoreCheckpoint::StageReady(stage_identity), &copy_lease)?;
    source
        .revalidate()
        .map_err(|error| before(error.to_string()))?;
    let staged = shared_root
        .select(&control.stage_name)
        .map_err(|error| uncertain(&name, error.to_string()))?;
    if entry_identity(&stage).map_err(|message| uncertain(&name, message))? != stage_identity
        || inspect_entry(
            &staged.directory,
            &staged.name,
            limits,
            control.cancellation,
        )
        .map_err(|error| uncertain(&name, error.to_string()))?
        .tree_identity
            != expected_tree
    {
        return Err(uncertain(
            &name,
            "Restore staging entry changed after its identity was saved".into(),
        ));
    }
    drop(copy_lease);

    let staged = shared_root
        .select(&control.stage_name)
        .map_err(|error| staged_error(&control, &name, error.to_string()))?;
    let destination = shared_root
        .select(OsStr::new(&name))
        .map_err(|error| staged_error(&control, &name, error.to_string()))?;
    let move_plan = CoordinationPlan::new_cancellable(
        [
            DirectoryEffect::tree(trash, CoordinationMode::Exclusive),
            DirectoryEffect::tree(stage.clone(), CoordinationMode::Exclusive),
            DirectoryEffect::entry(stage, CoordinationMode::Exclusive),
            DirectoryEffect::entry(target.clone(), CoordinationMode::Exclusive),
        ]
        .into_iter()
        .chain(state_effects())
        .collect(),
        control.timeout,
        control.cancellation.clone(),
    )
    .map_err(|error| staged_error(&control, &name, error.to_string()))?;
    let lease = move_plan
        .acquire()
        .map_err(|error| staged_error(&control, &name, error.to_string()))?
        .finalize_write(&restore_scope, &[])
        .map_err(|error| staged_error(&control, &name, error.to_string()))?;
    checkpoint(RestoreCheckpoint::BeforePublish, &lease)?;
    source
        .revalidate()
        .map_err(|error| staged_error(&control, &name, error.to_string()))?;
    let current = inspect_entry(
        &source.directory,
        &source.name,
        limits,
        control.cancellation,
    )
    .map_err(|error| staged_error(&control, &name, error.to_string()))?;
    if current.tree_identity != expected_tree {
        return Err(staged_error(
            &control,
            &name,
            "Retained backup changed after staging the restore".into(),
        ));
    }
    if entry_identity(&staged.original_path)
        .map_err(|message| staged_error(&control, &name, message))?
        != stage_identity
    {
        return Err(staged_error(
            &control,
            &name,
            "Saved restore staging entry changed before publication".into(),
        ));
    }
    let staged_report = inspect_entry(
        &staged.directory,
        &staged.name,
        limits,
        control.cancellation,
    )
    .map_err(|error| staged_error(&control, &name, error.to_string()))?;
    if staged_report.tree_identity != expected_tree {
        return Err(staged_error(
            &control,
            &name,
            "Restore staging verification failed".into(),
        ));
    }
    staged
        .move_verified_tree(
            &destination,
            &expected_tree,
            lease,
            limits,
            control.cancellation,
        )
        .map_err(|error| match error {
            TreeMoveFailure::BeforeMove(message) => staged_error(&control, &name, message),
            TreeMoveFailure::MayHaveMoved(message) => TrialRestoreError::PublicationUncertain {
                name: name.clone(),
                message,
            },
        })?;

    prepare_claude_link(
        home,
        &shared_requested,
        &shared_root,
        &destination,
        stage_identity,
        &expected_tree,
        &name,
        limits,
        &control,
        |lease| checkpoint(RestoreCheckpoint::BeforeLink, lease),
    )
    .map_err(|message| TrialRestoreError::PublishedLinkFailure {
        name: name.clone(),
        message,
    })?;
    Ok(TrialRestoreReceipt { name, target })
}

#[allow(clippy::too_many_arguments)]
fn prepare_claude_link(
    home: &Path,
    shared: &Path,
    shared_root: &BackupSourceRoot,
    published: &BackupSource,
    expected_identity: RestoreEntryIdentity,
    expected_tree: &str,
    name: &str,
    limits: BackupCopyLimits,
    control: &RestoreControl<'_>,
    before_effect: impl FnOnce(
        &crate::skill_coordination::FinalizedWriteLease<'_>,
    ) -> Result<(), TrialRestoreError>,
) -> Result<(), String> {
    let claude = home.join(".claude/skills");
    let link = claude.join(name);
    let mut effects = vec![
        DirectoryEffect::tree(published.original_path.clone(), CoordinationMode::Exclusive),
        DirectoryEffect::entry(claude.clone(), CoordinationMode::Exclusive),
        DirectoryEffect::entry(link.clone(), CoordinationMode::Exclusive),
    ];
    effects.extend(
        control
            .state_roots
            .iter()
            .cloned()
            .map(|path| DirectoryEffect::tree(path, CoordinationMode::Exclusive)),
    );
    let mut roots = vec![home.to_path_buf()];
    roots.extend_from_slice(control.state_roots);
    roots.sort();
    roots.dedup();
    let scope = SkillReadScope::bind(&roots).map_err(|error| error.to_string())?;
    let lease =
        CoordinationPlan::new_cancellable(effects, control.timeout, control.cancellation.clone())
            .map_err(|error| error.to_string())?
            .acquire()
            .map_err(|error| error.to_string())?
            .finalize_write(&scope, &[])
            .map_err(|error| error.to_string())?;
    before_effect(&lease).map_err(|error| error.to_string())?;
    if entry_identity(&published.original_path)? != expected_identity {
        return Err("Published restore target has a different physical identity".into());
    }
    verify_published_target(
        shared_root,
        published,
        expected_tree,
        limits,
        control.cancellation,
    )?;
    match fs::symlink_metadata(&claude) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return verify_published_target(
                shared_root,
                published,
                expected_tree,
                limits,
                control.cancellation,
            )
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(&claude)
                .map_err(|error| format!("Failed to create {}: {error}", claude.display()))?;
        }
        Err(error) => return Err(format!("Failed to inspect {}: {error}", claude.display())),
    }

    let claude_root = BackupSourceRoot::bind(&claude).map_err(|error| error.to_string())?;
    verify_published_target(
        shared_root,
        published,
        expected_tree,
        limits,
        control.cancellation,
    )?;
    let claude_directory = claude_root.directory().map_err(|error| error.to_string())?;
    match claude_directory.symlink_metadata(name) {
        Ok(_) => {
            claude_root.directory().map_err(|error| error.to_string())?;
            return Ok(());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Failed to inspect {}: {error}", link.display())),
    }
    let target = relative_path_between(&claude, shared).join(name);
    #[cfg(unix)]
    claude_directory
        .symlink_contents(&target, name)
        .map_err(|error| format!("Failed to symlink {}: {error}", link.display()))?;
    #[cfg(not(unix))]
    return Err("Symlinking is only supported on Unix".into());
    claude_root.directory().map_err(|error| error.to_string())?;
    let metadata = claude_directory
        .symlink_metadata(name)
        .map_err(|error| format!("Failed to verify {}: {error}", link.display()))?;
    if !metadata.file_type().is_symlink()
        || claude_directory
            .read_link_contents(name)
            .map_err(|error| format!("Failed to verify {}: {error}", link.display()))?
            != target
    {
        return Err(format!("Claude Code link changed at {}", link.display()));
    }
    verify_published_target(
        shared_root,
        published,
        expected_tree,
        limits,
        control.cancellation,
    )
}

fn verify_published_target(
    shared_root: &BackupSourceRoot,
    published: &BackupSource,
    expected_tree: &str,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    shared_root.directory().map_err(|error| error.to_string())?;
    published.revalidate().map_err(|error| error.to_string())?;
    let report = inspect_entry(&published.directory, &published.name, limits, cancellation)
        .map_err(|error| error.to_string())?;
    if report.tree_identity != expected_tree {
        return Err("Published restore changed before Claude Code link creation".into());
    }
    Ok(())
}

fn relative_path_between(from: &Path, to: &Path) -> PathBuf {
    let from_parts: Vec<_> = from.components().collect();
    let to_parts: Vec<_> = to.components().collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..from_parts.len() {
        relative.push("..");
    }
    for part in &to_parts[common..] {
        relative.push(part);
    }
    relative
}

fn validate_trash_path(
    home: &Path,
    trash_path: &str,
) -> Result<(PathBuf, PathBuf, String), TrialRestoreError> {
    let trash_root = fs::canonicalize(home.join(".agents/skills-trash"))
        .map_err(|error| before(format!("Failed to resolve ~/.agents/skills-trash: {error}")))?;
    let requested = PathBuf::from(trash_path);
    if !requested.is_absolute()
        || requested
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(before("Invalid trash path".into()));
    }
    let trash = fs::canonicalize(&requested)
        .map_err(|error| before(format!("Invalid trash path: {error}")))?;
    if !trash.starts_with(&trash_root) || trash == trash_root {
        return Err(before(
            "Trash path is not under ~/.agents/skills-trash".into(),
        ));
    }
    let name_path = if trash.file_name() == Some(OsStr::new("backup")) {
        trash
            .parent()
            .ok_or_else(|| before("Invalid trash path".into()))?
    } else {
        trash.as_path()
    };
    let name = name_path
        .file_name()
        .and_then(OsStr::to_str)
        .and_then(strip_trash_suffix)
        .ok_or_else(|| before("Could not determine the skill's name from its trash path".into()))?;
    if !valid_skill_name(&name) {
        return Err(before("Invalid skill directory name".into()));
    }
    Ok((trash_root, trash, name))
}

fn strip_trash_suffix(dir_name: &str) -> Option<String> {
    if let Some((prefix, suffix)) = dir_name.rsplit_once("--v2-") {
        let valid = suffix
            .split_once('-')
            .is_some_and(|(deployment, invocation)| {
                deployment.len() == 16
                    && deployment
                        .chars()
                        .all(|character| character.is_ascii_hexdigit())
                    && invocation.len() == 26
                    && invocation
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric())
            });
        if valid {
            return strip_trash_suffix(prefix);
        }
    }
    let suffix_start = dir_name.char_indices().nth_back(15)?.0;
    let suffix = &dir_name[suffix_start..];
    let bytes = suffix.as_bytes();
    if bytes.len() != 16
        || bytes[0] != b'-'
        || bytes[9] != b'-'
        || !bytes[1..9].iter().all(u8::is_ascii_digit)
        || !bytes[10..].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    dir_name
        .strip_suffix(suffix)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

pub(crate) fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && Path::new(name).components().count() == 1
}

fn unique_stage_name() -> OsString {
    static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);
    OsString::from(format!(
        ".trial-restore-stage-{}-{}",
        std::process::id(),
        NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn before(message: String) -> TrialRestoreError {
    TrialRestoreError::BeforePublication(message)
}

fn uncertain(name: &str, message: String) -> TrialRestoreError {
    TrialRestoreError::PublicationUncertain {
        name: name.into(),
        message,
    }
}

fn phase_error(
    expected_stage: Option<RestoreEntryIdentity>,
    name: &str,
    message: String,
) -> TrialRestoreError {
    if expected_stage.is_some() {
        uncertain(name, message)
    } else {
        before(message)
    }
}

fn staged_error(control: &RestoreControl<'_>, name: &str, message: String) -> TrialRestoreError {
    if control.state_roots.is_empty() {
        before(message)
    } else {
        uncertain(name, message)
    }
}

fn restore_identity(metadata: &fs::Metadata) -> RestoreEntryIdentity {
    RestoreEntryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

pub(crate) fn entry_identity(path: &Path) -> Result<RestoreEntryIdentity, String> {
    fs::symlink_metadata(path)
        .map(|metadata| restore_identity(&metadata))
        .map_err(|error| error.to_string())
}

#[derive(Clone, Copy)]
pub(crate) enum RestoreCheckpoint {
    BeforeCopy,
    StageReady(RestoreEntryIdentity),
    BeforePublish,
    BeforeLink,
    AfterLink,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::Cell,
        os::unix::fs::{symlink, PermissionsExt},
        thread,
    };

    fn limits() -> BackupCopyLimits {
        BackupCopyLimits {
            max_bytes: 1024 * 1024,
            max_entries: 100,
            max_depth: 16,
        }
    }

    fn legacy_backup(home: &Path, name: &str) -> PathBuf {
        let path = home
            .join(".agents/skills-trash")
            .join(format!("{name}-20260101-120000"));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("SKILL.md"), "body").unwrap();
        path
    }

    fn v2_backup(home: &Path, name: &str) -> PathBuf {
        let invocation = home.join(".agents/skills-trash").join(format!(
            "{name}-20260101-120000--v2-0123456789abcdef-01ARZ3NDEKTSV4RRFFQ69G5FAV"
        ));
        let backup = invocation.join("backup");
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("SKILL.md"), "body").unwrap();
        backup
    }

    #[test]
    fn restore_waits_for_an_inventory_reader_before_publishing() {
        let temp = tempfile::tempdir().unwrap();
        let backup = v2_backup(temp.path(), "queued");
        let shared = temp.path().join(".agents/skills");
        fs::create_dir_all(&shared).unwrap();
        let (ready, waiting) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let _guard = CoordinationPlan::new(
                vec![DirectoryEffect::tree(shared, CoordinationMode::Shared)],
                Some(Duration::from_secs(5)),
            )
            .unwrap()
            .acquire()
            .unwrap();
            ready.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(2300));
        });
        waiting.recv().unwrap();
        let restored = restore_trial_backup(temp.path(), &backup.to_string_lossy(), limits());
        reader.join().unwrap();
        let restored = restored.unwrap();
        assert_eq!(fs::read(restored.target.join("SKILL.md")).unwrap(), b"body");
        assert_eq!(fs::read(backup.join("SKILL.md")).unwrap(), b"body");
    }

    #[test]
    fn restores_legacy_and_actual_v2_backups_as_global_untracked_skills() {
        let temp = tempfile::tempdir().unwrap();
        let legacy = legacy_backup(temp.path(), "détecteur");
        let receipt =
            restore_trial_backup(temp.path(), &legacy.to_string_lossy(), limits()).unwrap();
        assert_eq!(receipt.name, "détecteur");
        assert!(receipt.target.join("SKILL.md").is_file());

        let v2 = v2_backup(temp.path(), "other");
        let receipt = restore_trial_backup(temp.path(), &v2.to_string_lossy(), limits()).unwrap();
        assert_eq!(receipt.name, "other");
        assert!(receipt.target.join("SKILL.md").is_file());
        assert!(v2.join("SKILL.md").is_file());
        assert!(!temp.path().join(".agents/skill-studio").exists());
    }

    #[test]
    fn restores_names_containing_the_version_marker_in_both_layouts() {
        for versioned in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let name = "foo--v2-bar";
            let backup = if versioned {
                v2_backup(temp.path(), name)
            } else {
                legacy_backup(temp.path(), name)
            };
            let receipt =
                restore_trial_backup(temp.path(), &backup.to_string_lossy(), limits()).unwrap();
            assert_eq!(receipt.name, name);
            assert_eq!(fs::read(receipt.target.join("SKILL.md")).unwrap(), b"body");
            assert!(backup.join("SKILL.md").is_file());
        }
    }

    #[test]
    fn preserves_literal_links_cycles_and_unix_modes() {
        let temp = tempfile::tempdir().unwrap();
        let backup = legacy_backup(temp.path(), "links");
        fs::create_dir(backup.join("directory")).unwrap();
        fs::write(backup.join("directory/item"), "content").unwrap();
        fs::write(backup.join("resource"), "content").unwrap();
        fs::set_permissions(backup.join("resource"), fs::Permissions::from_mode(0o640)).unwrap();
        symlink("directory", backup.join("literal-directory")).unwrap();
        symlink("resource", backup.join("literal-file")).unwrap();
        symlink("missing", backup.join("dangling")).unwrap();
        symlink("cycle-b", backup.join("cycle-a")).unwrap();
        symlink("cycle-a", backup.join("cycle-b")).unwrap();

        let target = restore_trial_backup(temp.path(), &backup.to_string_lossy(), limits())
            .unwrap()
            .target;
        for (link, expected) in [
            ("literal-directory", "directory"),
            ("literal-file", "resource"),
            ("dangling", "missing"),
            ("cycle-a", "cycle-b"),
            ("cycle-b", "cycle-a"),
        ] {
            assert_eq!(
                fs::read_link(target.join(link)).unwrap(),
                PathBuf::from(expected)
            );
        }
        assert_eq!(
            fs::metadata(target.join("resource"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }

    #[test]
    fn rejects_outside_malformed_and_overlong_sources() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(".agents/skills-trash")).unwrap();
        let outside = temp.path().join("outside-20260101-120000");
        fs::create_dir(&outside).unwrap();
        let error =
            restore_trial_backup(temp.path(), &outside.to_string_lossy(), limits()).unwrap_err();
        assert!(error.to_string().contains("not under"));

        let traversal = temp
            .path()
            .join(".agents/skills-trash/sub/../valid-20260101-120000");
        assert!(matches!(
            restore_trial_backup(temp.path(), &traversal.to_string_lossy(), limits()),
            Err(TrialRestoreError::BeforePublication(_))
        ));

        let overlong = format!("{}-20260101-120000", "x".repeat(129));
        let overlong_unicode = format!("{}-20260101-120000", "é".repeat(65));
        for name in [
            "bad-20260101-12000",
            "bad-20260101-120000--v2-0123456789abcdeg-01ARZ3NDEKTSV4RRFFQ69G5FAV",
            overlong.as_str(),
            overlong_unicode.as_str(),
        ] {
            let path = temp.path().join(".agents/skills-trash").join(name);
            fs::create_dir_all(&path).unwrap();
            assert!(matches!(
                restore_trial_backup(temp.path(), &path.to_string_lossy(), limits()),
                Err(TrialRestoreError::BeforePublication(_))
            ));
        }
    }

    #[test]
    fn staging_name_collision_never_publishes_before_the_move() {
        let temp = tempfile::tempdir().unwrap();
        let name = format!(".trial-restore-stage-{}-0", std::process::id());
        let backup = legacy_backup(temp.path(), &name);
        let target = temp.path().join(".agents/skills").join(&name);
        let staged = Cell::new(false);
        let cancellation = CancellationToken::default();
        let receipt = restore_trial_backup_with(
            temp.path(),
            &backup.to_string_lossy(),
            limits(),
            RestoreControl::legacy(OsString::from(&name), &cancellation),
            |checkpoint, _| {
                if matches!(checkpoint, RestoreCheckpoint::StageReady(_)) {
                    staged.set(true);
                    assert!(fs::symlink_metadata(&target).is_err());
                }
                Ok(())
            },
        )
        .unwrap();
        assert!(staged.get());
        assert_eq!(receipt.target, fs::canonicalize(&target).unwrap());
        assert_eq!(fs::read(target.join("SKILL.md")).unwrap(), b"body");
        assert!(backup.join("SKILL.md").is_file());
        assert!(
            fs::symlink_metadata(temp.path().join(".claude/skills").join(name))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn refuses_occupied_and_raced_targets_without_replacing_them() {
        for kind in ["directory", "file", "dangling"] {
            let temp = tempfile::tempdir().unwrap();
            let backup = legacy_backup(temp.path(), "taken");
            let target = temp.path().join(".agents/skills/taken");
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            match kind {
                "directory" => fs::create_dir(&target).unwrap(),
                "file" => fs::write(&target, "existing").unwrap(),
                "dangling" => symlink("missing", &target).unwrap(),
                _ => unreachable!(),
            }
            let before = fs::symlink_metadata(&target).unwrap().file_type();
            let error =
                restore_trial_backup(temp.path(), &backup.to_string_lossy(), limits()).unwrap_err();
            assert!(error.to_string().contains("already exists"));
            assert_eq!(fs::symlink_metadata(&target).unwrap().file_type(), before);
            assert!(backup.join("SKILL.md").is_file());
        }

        let temp = tempfile::tempdir().unwrap();
        let backup = legacy_backup(temp.path(), "raced");
        let target = temp.path().join(".agents/skills/raced");
        let injected = Cell::new(false);
        let cancellation = CancellationToken::default();
        let error = restore_trial_backup_with(
            temp.path(),
            &backup.to_string_lossy(),
            limits(),
            RestoreControl::legacy(unique_stage_name(), &cancellation),
            |checkpoint, _| {
                if matches!(checkpoint, RestoreCheckpoint::StageReady(_)) && !injected.replace(true)
                {
                    fs::write(&target, "racer").unwrap();
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert!(matches!(error, TrialRestoreError::BeforePublication(_)));
        assert_eq!(fs::read_to_string(&target).unwrap(), "racer");
        assert!(backup.join("SKILL.md").is_file());
    }

    #[test]
    fn refuses_source_drift_before_copy_and_before_publish() {
        for checkpoint_to_change in [
            RestoreCheckpoint::BeforeCopy,
            RestoreCheckpoint::StageReady(RestoreEntryIdentity {
                device: 0,
                inode: 0,
            }),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let backup = legacy_backup(temp.path(), "drift");
            let changed = Cell::new(false);
            let cancellation = CancellationToken::default();
            let error = restore_trial_backup_with(
                temp.path(),
                &backup.to_string_lossy(),
                limits(),
                RestoreControl::legacy(unique_stage_name(), &cancellation),
                |checkpoint, _| {
                    if std::mem::discriminant(&checkpoint)
                        == std::mem::discriminant(&checkpoint_to_change)
                        && !changed.replace(true)
                    {
                        fs::write(backup.join("SKILL.md"), "changed").unwrap();
                    }
                    Ok(())
                },
            )
            .unwrap_err();
            assert!(matches!(error, TrialRestoreError::BeforePublication(_)));
            assert!(fs::symlink_metadata(temp.path().join(".agents/skills/drift")).is_err());
            assert_eq!(
                fs::read_to_string(backup.join("SKILL.md")).unwrap(),
                "changed"
            );
        }
    }

    #[test]
    fn bounded_copy_failure_keeps_backup_and_final_target_absent() {
        let temp = tempfile::tempdir().unwrap();
        let backup = legacy_backup(temp.path(), "bounded");
        fs::write(backup.join("large"), vec![0_u8; 64]).unwrap();
        let error = restore_trial_backup(
            temp.path(),
            &backup.to_string_lossy(),
            BackupCopyLimits {
                max_bytes: 1,
                ..limits()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("limit"));
        assert!(backup.join("large").is_file());
        assert!(fs::symlink_metadata(temp.path().join(".agents/skills/bounded")).is_err());
    }

    #[test]
    fn applies_claude_link_rules_without_replacing_existing_entries() {
        let absent = tempfile::tempdir().unwrap();
        let backup = legacy_backup(absent.path(), "created");
        restore_trial_backup(absent.path(), &backup.to_string_lossy(), limits()).unwrap();
        assert_eq!(
            fs::read_link(absent.path().join(".claude/skills/created")).unwrap(),
            PathBuf::from("../../.agents/skills/created")
        );

        let existing = tempfile::tempdir().unwrap();
        let backup = legacy_backup(existing.path(), "preserved");
        fs::create_dir_all(existing.path().join(".claude/skills")).unwrap();
        symlink("missing", existing.path().join(".claude/skills/preserved")).unwrap();
        restore_trial_backup(existing.path(), &backup.to_string_lossy(), limits()).unwrap();
        assert_eq!(
            fs::read_link(existing.path().join(".claude/skills/preserved")).unwrap(),
            PathBuf::from("missing")
        );

        let whole_dir = tempfile::tempdir().unwrap();
        let backup = legacy_backup(whole_dir.path(), "covered");
        fs::create_dir_all(whole_dir.path().join(".claude")).unwrap();
        symlink("../.agents/skills", whole_dir.path().join(".claude/skills")).unwrap();
        restore_trial_backup(whole_dir.path(), &backup.to_string_lossy(), limits()).unwrap();
        assert!(whole_dir
            .path()
            .join(".claude/skills/covered/SKILL.md")
            .is_file());
    }

    #[test]
    fn reports_published_link_failure_and_preserves_content_and_backup() {
        let temp = tempfile::tempdir().unwrap();
        let backup = legacy_backup(temp.path(), "linked");
        fs::write(temp.path().join(".claude"), "blocking file").unwrap();
        let error =
            restore_trial_backup(temp.path(), &backup.to_string_lossy(), limits()).unwrap_err();
        assert!(matches!(
            error,
            TrialRestoreError::PublishedLinkFailure { ref name, .. } if name == "linked"
        ));
        assert!(error.publication_possible());
        assert!(temp.path().join(".agents/skills/linked/SKILL.md").is_file());
        assert!(backup.join("SKILL.md").is_file());
        assert!(!TrialRestoreError::BeforePublication("no move".into()).publication_possible());
        assert!(TrialRestoreError::PublicationUncertain {
            name: "linked".into(),
            message: "unknown".into(),
        }
        .publication_possible());
    }

    #[test]
    fn cancellation_interrupts_an_active_backup_walk_without_publication() {
        let temp = tempfile::tempdir().unwrap();
        let backup = legacy_backup(temp.path(), "cancelled-walk");
        fs::write(backup.join("large"), vec![0_u8; 64 * 1024 * 1024]).unwrap();
        let cancellation = CancellationToken::default();
        let stop = cancellation.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(1));
            stop.cancel();
        });
        let error = restore_trial_backup_with(
            temp.path(),
            &backup.to_string_lossy(),
            BackupCopyLimits {
                max_bytes: 128 * 1024 * 1024,
                ..limits()
            },
            RestoreControl::legacy(unique_stage_name(), &cancellation),
            |_, _| Ok(()),
        )
        .unwrap_err();
        canceller.join().unwrap();
        assert!(error.to_string().contains("cancelled"));
        assert!(!temp.path().join(".agents/skills/cancelled-walk").exists());
        assert!(backup.join("SKILL.md").is_file());
    }
}
