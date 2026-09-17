//! Read-only provider diagnostics held across external effects. This grants no
//! permission to resume writes with an invalidated selection lease.
use crate::{
    skill_backup_copy::unchanged, skill_backup_source::BackupSourceRoot,
    skill_coordination::CancellationToken, skill_dotagents_ledger::DotagentsDetachState,
    skill_service::PreparedDotagentsForkSelection,
};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, Metadata, MetadataExt, OpenOptions};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io::{self, Read},
    os::unix::ffi::OsStrExt,
    path::Path,
};

const DOCUMENT_LIMIT: usize = 8 * 1024 * 1024;

pub struct DotagentsProviderObservation<'prepared, 'scope> {
    prepared: &'prepared PreparedDotagentsForkSelection<'scope>,
    baseline: ProviderEffectBaseline,
}

pub(crate) struct ProviderEffectBaseline {
    root: BackupSourceRoot,
    membership: SkillMembership,
}

const MEMBERSHIP_LIMIT: usize = 20_000;
const MEMBERSHIP_NAME_BYTES: usize = 16 * 1024 * 1024;

struct SkillMembership {
    directory: Dir,
    selected: OsString,
    before: MembershipSnapshot,
}

struct MembershipSnapshot {
    root: Metadata,
    entries: BTreeMap<OsString, Metadata>,
}

fn same_entry(before: &Metadata, after: &Metadata) -> bool {
    unchanged(before, after) && before.mode() == after.mode() && before.nlink() == after.nlink()
}

impl MembershipSnapshot {
    fn read(directory: &Dir, check: impl Fn() -> io::Result<()>) -> io::Result<Self> {
        check()?;
        let root = directory.dir_metadata()?;
        let mut entries = BTreeMap::new();
        let mut name_bytes = 0usize;
        for entry in directory.entries()? {
            check()?;
            let name = entry?.file_name();
            name_bytes = name_bytes.saturating_add(name.as_bytes().len());
            if entries.len() >= MEMBERSHIP_LIMIT || name_bytes > MEMBERSHIP_NAME_BYTES {
                return Err(io::Error::other(
                    "Skills membership exceeds observation limit",
                ));
            }
            let metadata = directory.symlink_metadata(&name)?;
            if entries.insert(name, metadata).is_some() {
                return Err(io::Error::other(
                    "Skills membership changed during enumeration",
                ));
            }
        }
        for (name, before) in &entries {
            check()?;
            if !same_entry(before, &directory.symlink_metadata(name)?) {
                return Err(io::Error::other("Skill entry changed during enumeration"));
            }
        }
        if !same_entry(&root, &directory.dir_metadata()?) {
            return Err(io::Error::other("Skills parent changed during enumeration"));
        }
        Ok(Self { root, entries })
    }

    fn matches(&self, current: &Self, removed: Option<&OsStr>) -> bool {
        self.root.dev() == current.root.dev()
            && self.root.ino() == current.root.ino()
            && self.root.mode() == current.root.mode()
            && self
                .entries
                .iter()
                .filter(|(name, _)| Some(name.as_os_str()) != removed)
                .all(|(name, before)| {
                    current
                        .entries
                        .get(name)
                        .is_some_and(|after| same_entry(before, after))
                })
            && current
                .entries
                .keys()
                .all(|name| self.entries.contains_key(name))
            && removed.is_none_or(|name| !current.entries.contains_key(name))
    }
}

impl SkillMembership {
    fn bind(provider: &Dir, selected: &OsStr) -> io::Result<Self> {
        let directory = provider.open_dir_nofollow("skills")?;
        let before = MembershipSnapshot::read(&directory, || Ok(()))?;
        if !before.entries.get(selected).is_some_and(Metadata::is_dir) {
            return Err(io::Error::other(
                "Selected skill must be an existing directory",
            ));
        }
        Ok(Self {
            directory,
            selected: selected.to_owned(),
            before,
        })
    }

    fn observe(
        &self,
        provider: &Dir,
        cancellation: &CancellationToken,
    ) -> io::Result<MembershipSnapshot> {
        let current = MembershipSnapshot::read(&self.directory, || {
            if cancellation.is_cancelled() {
                Err(io::Error::other("Skills membership observation cancelled"))
            } else {
                Ok(())
            }
        })?;
        let named = provider.symlink_metadata("skills")?;
        if !named.is_dir() || !same_entry(&current.root, &named) {
            return Err(io::Error::other("Skills parent was replaced"));
        }
        let removed =
            (!current.entries.contains_key(&self.selected)).then_some(self.selected.as_os_str());
        if !self.before.matches(&current, removed) {
            return Err(io::Error::other("Unexpected skills membership change"));
        }
        Ok(current)
    }
}

struct Document {
    metadata: Option<Metadata>,
    bytes: Option<Vec<u8>>,
}

fn metadata(directory: &Dir, name: &str) -> io::Result<Option<Metadata>> {
    match directory.symlink_metadata(name) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn check_document(directory: &Dir, name: &str, document: &Document) -> io::Result<()> {
    let current = metadata(directory, name)?;
    match (&document.metadata, current) {
        (None, None) => Ok(()),
        (Some(before), Some(after))
            if after.is_file() && after.nlink() == 1 && unchanged(before, &after) =>
        {
            Ok(())
        }
        _ => Err(io::Error::other(
            "Provider document changed during observation",
        )),
    }
}

fn read_document(directory: &Dir, name: &str) -> io::Result<Document> {
    let before = metadata(directory, name)?;
    let Some(before) = before else {
        return Ok(Document {
            metadata: None,
            bytes: None,
        });
    };
    if !before.is_file() || before.nlink() != 1 || before.len() > DOCUMENT_LIMIT as u64 {
        return Err(io::Error::other(
            "Provider document must be a bounded single-link regular file",
        ));
    }
    let mut file = directory.open_with(
        name,
        OpenOptions::new()
            .read(true)
            .follow(FollowSymlinks::No)
            .nonblock(true),
    )?;
    if !unchanged(&before, &file.metadata()?) {
        return Err(io::Error::other("Provider document changed before reading"));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(DOCUMENT_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > DOCUMENT_LIMIT || !unchanged(&before, &file.metadata()?) {
        return Err(io::Error::other("Provider document changed while reading"));
    }
    let document = Document {
        metadata: Some(before),
        bytes: Some(bytes),
    };
    check_document(directory, name, &document)?;
    Ok(document)
}

impl<'prepared, 'scope> DotagentsProviderObservation<'prepared, 'scope> {
    pub(crate) fn bind(
        prepared: &'prepared PreparedDotagentsForkSelection<'scope>,
    ) -> Result<Self, String> {
        Ok(Self {
            prepared,
            baseline: ProviderEffectBaseline::bind(prepared)?,
        })
    }

    /// Keeps the original lease alive but does not rebaseline it. A caller may use
    /// a fresh cleanup token after stopping a cancelled provider process.
    pub fn observe(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<DotagentsDetachState, String> {
        self.observe_with(cancellation, || {})
    }

    fn observe_with(
        &self,
        cancellation: &CancellationToken,
        after_reads: impl FnOnce(),
    ) -> Result<DotagentsDetachState, String> {
        self.baseline
            .observe_with(self.prepared, cancellation, after_reads)
    }
}

impl ProviderEffectBaseline {
    pub(crate) fn bind(prepared: &PreparedDotagentsForkSelection<'_>) -> Result<Self, String> {
        prepared.revalidate().map_err(|error| error.to_string())?;
        let agents = Path::new(&prepared.preview().path)
            .parent()
            .and_then(Path::parent)
            .ok_or("Missing provider root")?;
        let root = BackupSourceRoot::bind(agents).map_err(|error| error.to_string())?;
        let selected = Path::new(&prepared.preview().path)
            .file_name()
            .ok_or("Missing skill name")?;
        let membership = SkillMembership::bind(
            &root.directory().map_err(|error| error.to_string())?,
            selected,
        )
        .map_err(|error| error.to_string())?;
        prepared.revalidate().map_err(|error| error.to_string())?;
        Ok(Self { root, membership })
    }

    #[cfg(feature = "event-store")]
    pub(crate) fn observe(
        &self,
        prepared: &PreparedDotagentsForkSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<DotagentsDetachState, String> {
        self.observe_with(prepared, cancellation, || {})
    }

    fn observe_with(
        &self,
        prepared: &PreparedDotagentsForkSelection<'_>,
        cancellation: &CancellationToken,
        after_reads: impl FnOnce(),
    ) -> Result<DotagentsDetachState, String> {
        let check = || {
            if cancellation.is_cancelled() {
                Err("Provider observation cancelled".to_string())
            } else {
                Ok(())
            }
        };
        check()?;
        let directory = self.root.directory().map_err(|error| error.to_string())?;
        let membership = self
            .membership
            .observe(&directory, cancellation)
            .map_err(|error| error.to_string())?;
        check()?;
        let lock = read_document(&directory, "agents.lock").map_err(|error| error.to_string())?;
        check()?;
        let manifest =
            read_document(&directory, "agents.toml").map_err(|error| error.to_string())?;
        check()?;
        let text = |bytes| std::str::from_utf8(bytes).map_err(|error| error.to_string());
        let state = prepared.detach().observe_document_effects(
            text(prepared.provider_lock())?,
            text(prepared.provider_manifest())?,
            lock.bytes.as_deref().map(text).transpose()?,
            manifest.bytes.as_deref().map(text).transpose()?,
        )?;
        after_reads();
        check_document(&directory, "agents.lock", &lock).map_err(|error| error.to_string())?;
        check_document(&directory, "agents.toml", &manifest).map_err(|error| error.to_string())?;
        let current = self
            .membership
            .observe(&directory, cancellation)
            .map_err(|error| error.to_string())?;
        if !membership.matches(&current, None) || !same_entry(&membership.root, &current.root) {
            return Err("Skills membership changed during provider observation".into());
        }
        self.root.directory().map_err(|error| error.to_string())?;
        check()?;
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect},
        skill_frontmatter_repair::{
            preview_frontmatter_repair, BoundFrontmatterRepairRequest, FrontmatterRepairApplyMode,
        },
        skill_service::{ScopedSkillService, SkillScope},
    };
    use std::{fs, time::Duration};

    #[test]
    fn observes_provider_changes_without_releasing_or_reauthorizing_the_lease() {
        for case in [
            "attached",
            "detached",
            "partial",
            "unrelated",
            "missing",
            "link",
            "root",
            "pair-drift",
            "cancelled",
            "oversized",
            "sibling-added",
            "sibling-removed",
            "sibling-replaced",
            "skills-root",
            "membership-drift",
            "selected-link",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let agents = home.join(".agents");
            let live = agents.join("skills/alpha");
            fs::create_dir_all(&live).unwrap();
            fs::create_dir(agents.join("skills/beta")).unwrap();
            let original = "---\nname: alpha\ndescription: Use when: testing\n---\nbody\n";
            let lock = format!("version = 1\nfuture = 'keep'\n[skills.alpha]\nsource = 'owner/repo'\nresolved_path = 'skills/alpha'\nresolved_commit = '{}'\n", "a".repeat(40));
            let manifest = "version = 1\n[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\n";
            fs::write(live.join("SKILL.md"), original).unwrap();
            fs::write(agents.join("agents.lock"), &lock).unwrap();
            fs::write(agents.join("agents.toml"), manifest).unwrap();
            let mut service = ScopedSkillService::bind(SkillScope {
                home,
                projects: vec![],
                backing_roots: vec![],
                plugin_ownership_roots: vec![],
            })
            .unwrap();
            let inventory = service.scan(None, None).unwrap();
            let deployment = inventory
                .skills
                .iter()
                .flat_map(|skill| &skill.deployments)
                .find(|deployment| deployment.path == live.to_string_lossy())
                .unwrap();
            let preview = preview_frontmatter_repair(deployment, original.as_bytes()).unwrap();
            let request = BoundFrontmatterRepairRequest {
                deployment_id: preview.deployment_id,
                proposal_id: preview.proposal_id,
                expected_content_fingerprint: preview.expected_content_fingerprint,
                mode: FrontmatterRepairApplyMode::ForkAndFix,
            };
            let prepared = service
                .prepare_dotagents_fork_selection(
                    &request,
                    std::slice::from_ref(&live),
                    None,
                    CancellationToken::default(),
                )
                .unwrap();
            let observer = prepared.provider_observation().unwrap();
            let token = CancellationToken::default();
            match case {
                "detached" | "unrelated" => {
                    fs::remove_dir_all(&live).unwrap();
                    fs::write(
                        agents.join("agents.lock"),
                        if case == "unrelated" {
                            "version = 1\n"
                        } else {
                            "version = 1\nfuture = 'keep'\n[skills]\n"
                        },
                    )
                    .unwrap();
                    fs::write(agents.join("agents.toml"), "version = 1\n").unwrap();
                }
                "partial" => {
                    fs::remove_dir_all(&live).unwrap();
                    fs::write(agents.join("agents.toml"), "version = 1\n").unwrap();
                }
                "missing" => {
                    fs::remove_file(agents.join("agents.lock")).unwrap();
                }
                "link" => {
                    let outside = temp.path().join("outside");
                    fs::write(&outside, &lock).unwrap();
                    fs::remove_file(agents.join("agents.lock")).unwrap();
                    std::os::unix::fs::symlink(outside, agents.join("agents.lock")).unwrap();
                }
                "root" => {
                    fs::rename(&agents, agents.with_file_name("old-agents")).unwrap();
                    fs::create_dir(&agents).unwrap();
                }
                "selected-link" => {
                    fs::rename(&live, temp.path().join("old-alpha")).unwrap();
                    std::os::unix::fs::symlink(temp.path().join("old-alpha"), &live).unwrap();
                }
                "sibling-added" => fs::create_dir(agents.join("skills/gamma")).unwrap(),
                "sibling-removed" => fs::remove_dir(agents.join("skills/beta")).unwrap(),
                "sibling-replaced" => {
                    fs::rename(agents.join("skills/beta"), temp.path().join("old-beta")).unwrap();
                    fs::create_dir(agents.join("skills/beta")).unwrap();
                }
                "skills-root" => {
                    fs::rename(agents.join("skills"), agents.join("old-skills")).unwrap();
                    fs::create_dir(agents.join("skills")).unwrap();
                }
                "cancelled" => token.cancel(),
                "oversized" => {
                    fs::write(agents.join("agents.lock"), vec![b' '; DOCUMENT_LIMIT + 1]).unwrap()
                }
                _ => {}
            }
            let result = observer.observe_with(&token, || {
                if case == "membership-drift" {
                    fs::create_dir(agents.join("skills/gamma")).unwrap();
                }
                if case == "pair-drift" {
                    fs::write(agents.join("agents.lock"), "changed after read").unwrap();
                }
            });
            let expected = match case {
                "attached" => Some(DotagentsDetachState::Attached),
                "detached" => Some(DotagentsDetachState::Detached),
                "partial" => Some(DotagentsDetachState::Partial),
                "unrelated" => Some(DotagentsDetachState::Changed),
                "missing" => Some(DotagentsDetachState::Unavailable),
                _ => None,
            };
            if let Some(expected) = expected {
                assert_eq!(result.unwrap(), expected, "{case}");
            } else {
                assert!(result.is_err(), "{case}");
            }
            if matches!(case, "detached" | "partial" | "unrelated") {
                assert!(
                    prepared.revalidate().is_err(),
                    "observation must not authorize writes"
                );
                let contender = CoordinationPlan::new_fixture(
                    vec![DirectoryEffect::entry(
                        agents.join("agents.lock"),
                        CoordinationMode::Exclusive,
                    )],
                    temp.path(),
                    Some(Duration::from_millis(30)),
                )
                .unwrap()
                .acquire();
                assert!(contender.is_err(), "original lock must remain held");
            }
        }
    }
}
