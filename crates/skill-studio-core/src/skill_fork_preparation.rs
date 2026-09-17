//! Copies candidate fork inputs under the unchanged selection lease.
use crate::{
    skill_backup_copy::inspect_entry,
    skill_backup_reservation::{BackupCopyLimits, BackupStateRoot},
    skill_backup_source::BackupSourceRoot,
    skill_coordination::{CancellationToken, FinalizedWriteLease},
    skill_fork_snapshot::{ForkSnapshotIdentities, ForkSnapshotReceipt},
    skill_service::PreparedDotagentsForkSelection,
};
use std::{collections::BTreeMap, ffi::OsStr, path::Path};

pub(crate) fn publish<'root>(
    prepared: &PreparedDotagentsForkSelection<'_>,
    lease: &FinalizedWriteLease<'_>,
    state: &'root BackupStateRoot,
    id: &str,
    upstream: crate::skill_upstream_fetch::FetchedForkSource,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<crate::skill_service::PublishedForkSnapshots<'root>, String> {
    if !crate::skill_backup_reservation::valid_id(id) {
        return Err("Invalid fork backup ID".into());
    }
    let (upstream, expected_upstream_tree) =
        upstream.into_matching_source(&prepared.fork_source()?)?;
    let live_path = Path::new(&prepared.preview().path);
    lease.validate_state_tree(&state.path)?;
    lease.validate_state_tree(live_path)?;
    lease.validate_state_tree(&upstream.original_path)?;
    let skills = live_path.parent().ok_or("Skill has no parent")?;
    let agents = skills.parent().ok_or("Skill has no ownership root")?;
    let live_root = BackupSourceRoot::bind(skills).map_err(|error| error.to_string())?;
    let live = live_root
        .select(live_path.file_name().ok_or("Skill has no name")?)
        .map_err(|error| error.to_string())?;
    for source in [&live, &upstream] {
        if !source
            .directory
            .symlink_metadata(&source.name)
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            return Err("Fork snapshot source must be a directory entry".into());
        }
    }
    let documents = BackupSourceRoot::bind(agents).map_err(|error| error.to_string())?;
    let select = |name| {
        documents
            .select(OsStr::new(name))
            .map_err(|error| error.to_string())
    };
    let mut sources = vec![
        ("live-tree", live, None),
        ("upstream-tree", upstream, None),
        (
            "provider-lock",
            select("agents.lock")?,
            Some(prepared.provider_lock()),
        ),
        (
            "provider-manifest",
            select("agents.toml")?,
            Some(prepared.provider_manifest()),
        ),
    ];
    if let Some(bytes) = prepared.registry_before() {
        sources.push(("registry-before", select("skill-studio.json")?, Some(bytes)));
    }
    let destination = state
        .resolved_path()
        .map_err(|error| error.to_string())?
        .join("backups")
        .join(id);
    let mut before = BTreeMap::new();
    for (name, source, _) in &sources {
        let resolved = source.resolved_path().map_err(|error| error.to_string())?;
        if destination.starts_with(&resolved) || resolved.starts_with(&destination) {
            return Err("Fork snapshot source and destination overlap".into());
        }
        let report = inspect_entry(&source.directory, &source.name, limits, cancellation)
            .map_err(|error| error.to_string())?;
        if *name == "upstream-tree" && report.tree_identity != expected_upstream_tree {
            return Err("Fetched upstream tree changed before snapshot publication".into());
        }
        before.insert(*name, report.tree_identity);
    }
    prepared.revalidate().map_err(|error| error.to_string())?;
    let operation = state.reserve(id).map_err(|error| error.to_string())?;
    let published = (|| {
        let mut remaining = limits;
        let mut identities = BTreeMap::new();
        for (name, source, original) in &sources {
            prepared.revalidate().map_err(|error| error.to_string())?;
            source.revalidate().map_err(|error| error.to_string())?;
            let report = operation
                .copy_entry(
                    &source.directory,
                    &source.name,
                    OsStr::new(name),
                    remaining,
                    cancellation,
                )
                .map_err(|error| error.to_string())?;
            remaining.max_bytes = remaining
                .max_bytes
                .checked_sub(report.bytes)
                .ok_or("Fork backup byte budget exceeded")?;
            remaining.max_entries = remaining
                .max_entries
                .checked_sub(report.entries)
                .ok_or("Fork backup entry budget exceeded")?;
            if before.get(name) != Some(&report.tree_identity) {
                return Err("Fork input changed before copying".into());
            }
            if let Some(original) = original {
                if operation
                    .read_record(name, 8 * 1024 * 1024)
                    .map_err(|error| error.to_string())?
                    != *original
                {
                    return Err("Fork document differs from prepared original".into());
                }
            }
            identities.insert(*name, report.tree_identity);
        }
        for (name, source, _) in &sources {
            source.revalidate().map_err(|error| error.to_string())?;
            let current = inspect_entry(&source.directory, &source.name, limits, cancellation)
                .map_err(|error| error.to_string())?;
            if identities.get(name) != Some(&current.tree_identity) {
                return Err("Fork input changed after copying".into());
            }
        }
        prepared.revalidate().map_err(|error| error.to_string())?;
        let take = |name| {
            identities
                .get(name)
                .cloned()
                .ok_or_else(|| format!("Missing fork snapshot {name}"))
        };
        let reference = ForkSnapshotReceipt::publish(
            &operation,
            &prepared.preview().deployment_id,
            prepared.detach(),
            ForkSnapshotIdentities {
                live_tree: take("live-tree")?,
                upstream_tree: take("upstream-tree")?,
                provider_lock: take("provider-lock")?,
                provider_manifest: take("provider-manifest")?,
                registry_before: identities.get("registry-before").cloned(),
            },
            limits,
            cancellation,
        )?;
        prepared.revalidate().map_err(|error| error.to_string())?;
        Ok(reference)
    })();
    match published {
        Ok(reference) => Ok(crate::skill_service::PublishedForkSnapshots {
            reference,
            backup: operation,
        }),
        Err(error) => match operation.discard() {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!("{error}; fork snapshot cleanup refused: {cleanup}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        skill_frontmatter_repair::{
            preview_frontmatter_repair, BoundFrontmatterRepairRequest, FrontmatterRepairApplyMode,
        },
        skill_service::{ScopedSkillService, SkillScope},
    };
    use std::fs;

    #[cfg(feature = "event-store")]
    fn assert_native_progress(
        pending: &crate::skill_pending_fork::PendingDotagentsFork<'_>,
        agents: &Path,
        state: &Path,
        expected: crate::skill_native_fork_progress::NativeForkProgress,
    ) {
        use crate::skill_native_fork_progress::{classify_native_fork, NativeForkDocuments};
        let optional = |path: &Path| match fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!("{error}"),
        };
        let backup = state.join("backups/fork");
        let original_manifest = fs::read(backup.join("provider-manifest")).unwrap();
        let original_lock = fs::read(backup.join("provider-lock")).unwrap();
        let original_registry = optional(&backup.join("registry-before"));
        let original_skill = fs::read(backup.join("live-tree/SKILL.md")).unwrap();
        let manifest = fs::read(agents.join("agents.toml")).unwrap();
        let lock = fs::read(agents.join("agents.lock")).unwrap();
        let registry = optional(&agents.join("skill-studio.json"));
        let skill = fs::read(agents.join("skills/alpha/SKILL.md")).unwrap();
        let original = NativeForkDocuments {
            manifest: &original_manifest,
            lock: &original_lock,
            registry: original_registry.as_deref(),
            skill: &original_skill,
        };
        let current = NativeForkDocuments {
            manifest: &manifest,
            lock: &lock,
            registry: registry.as_deref(),
            skill: &skill,
        };
        assert_eq!(
            classify_native_fork(pending.event().intent(), &original, &current).unwrap(),
            expected
        );
    }

    #[cfg(feature = "event-store")]
    fn run_pending_provider_fixture(
        pending: &crate::skill_pending_fork::PendingDotagentsFork<'_>,
        store: &crate::skill_event_store::EventStore,
        agents: &Path,
        limits: BackupCopyLimits,
        partial: bool,
    ) {
        use crate::skill_dotagents_ledger::DotagentsDetachState;
        use crate::skill_process_stream::{run_to_writer, ProcessStreamLimits};
        use std::{
            process::Command,
            time::{Duration, Instant},
        };
        let token = CancellationToken::default();
        pending
            .validate_before_execution(store, limits, &token)
            .unwrap();
        let mut child = Command::new("/bin/sh");
        child.env_clear().current_dir(agents).args([
            "-c",
            "printf '%s' \"$$\"; printf 'version = 1\\n' > \"$1/agents.toml\"; /bin/rm -rf -- \"$1/skills/alpha\"; if [ \"$2\" = partial ]; then exit 23; fi; printf 'version = 1\\n[skills]\\n' > \"$1/agents.lock\"",
            "provider-fixture",
        ]).arg(agents).arg(if partial { "partial" } else { "success" });
        let mut output = Vec::new();
        let result = run_to_writer(
            &mut child,
            &mut output,
            ProcessStreamLimits {
                stdout_bytes: 64,
                stderr_bytes: 1024,
                deadline: Instant::now() + Duration::from_secs(5),
            },
            || {
                if token.is_cancelled() {
                    Err("cancelled".into())
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result.is_ok(), !partial, "{result:?}");
        let pid: i32 = std::str::from_utf8(&output).unwrap().parse().unwrap();
        // SAFETY: signal zero checks existence without signalling the completed fixture child.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        assert_eq!(
            pending
                .observe_provider(&CancellationToken::default())
                .unwrap(),
            if partial {
                DotagentsDetachState::Partial
            } else {
                DotagentsDetachState::Detached
            }
        );
        assert!(pending.revalidate_selection().is_err());
        let row = store.get(pending.event().id()).unwrap().unwrap();
        assert_eq!(row.status, "pending");
        assert!(!row.restorable);
        assert!(row.inverse.is_none());
    }

    #[cfg(feature = "event-store")]
    fn dispatch_native_fork(
        service: &mut ScopedSkillService,
        store: &crate::skill_event_store::EventStore,
    ) {
        use crate::skill_repair_execution::{
            recover_next_repair, RepairRecoveryOutcome, RepairRecoveryStep,
        };
        let step = recover_next_repair(
            service,
            store,
            Some(std::time::Duration::from_secs(10)),
            CancellationToken::default(),
        )
        .unwrap();
        assert!(
            matches!(step, RepairRecoveryStep::Resolved { ref event_id, outcome: RepairRecoveryOutcome::Applied } if event_id == "fork")
        );
        assert!(matches!(
            recover_next_repair(
                service,
                store,
                Some(std::time::Duration::from_secs(10)),
                CancellationToken::default()
            )
            .unwrap(),
            RepairRecoveryStep::Idle
        ));
    }

    #[cfg(feature = "event-store")]
    fn resume_native_fork(
        service: &mut ScopedSkillService,
        store: &crate::skill_event_store::EventStore,
        limits: BackupCopyLimits,
        expected: crate::skill_native_fork_progress::NativeForkProgress,
        agents: &std::path::Path,
        repaired: &str,
    ) {
        let row = store.get("fork").unwrap().unwrap();
        let recovery = service
            .prepare_native_fork_recovery(
                &row,
                store,
                limits,
                Some(std::time::Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(recovery.progress(), expected);
        let cancelled = CancellationToken::default();
        cancelled.cancel();
        let before = fs::read(agents.join("skills/alpha/SKILL.md")).unwrap();
        assert!(matches!(
            recovery.resume(store, limits, &cancelled),
            Err(crate::skill_event_operations::EventWriteFailure::BeforeWrite(_))
        ));
        assert_eq!(store.get("fork").unwrap().unwrap().status, "pending");
        assert_eq!(
            fs::read(agents.join("skills/alpha/SKILL.md")).unwrap(),
            before
        );
        let recovery = service
            .prepare_native_fork_recovery(
                &row,
                store,
                limits,
                Some(std::time::Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(recovery.progress(), expected);
        use crate::skill_native_fork_progress::NativeForkProgress;
        let recovery = if expected == NativeForkProgress::Prepared {
            let mut recovery = recovery;
            for next in [
                NativeForkProgress::ManifestPublished,
                NativeForkProgress::ProviderDetached,
                NativeForkProgress::RegistryPublished,
                NativeForkProgress::RepairPublished,
            ] {
                let token = CancellationToken::default();
                let mut publications = 0;
                let result = recovery.resume_with(store, limits, &token, |progress| {
                    publications += 1;
                    assert_eq!(progress, next);
                    token.cancel();
                });
                assert_eq!(publications, 1);
                assert!(matches!(
                    result,
                    Err(crate::skill_event_operations::EventWriteFailure::MayHaveWritten(_))
                ));
                let pending = store.get("fork").unwrap().unwrap();
                assert_eq!(pending.status, "pending");
                assert!(!pending.restorable);
                assert!(pending.inverse.is_none());
                recovery = service
                    .prepare_native_fork_recovery(
                        &pending,
                        store,
                        limits,
                        Some(std::time::Duration::from_secs(10)),
                        CancellationToken::default(),
                    )
                    .unwrap();
                assert_eq!(recovery.progress(), next);
            }
            store
                .conn
                .execute(
                    "UPDATE events SET status = 'interrupted' WHERE id = 'fork'",
                    [],
                )
                .unwrap();
            assert!(matches!(
                recovery.resume(store, limits, &CancellationToken::default()),
                Err(crate::skill_event_operations::EventWriteFailure::BeforeWrite(_))
            ));
            let interrupted = store.get("fork").unwrap().unwrap();
            assert_eq!(interrupted.status, "interrupted");
            let recovery = service
                .prepare_native_fork_recovery(
                    &interrupted,
                    store,
                    limits,
                    Some(std::time::Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            assert_eq!(recovery.progress(), NativeForkProgress::RepairPublished);
            recovery
        } else {
            recovery
        };
        drop(recovery);
        dispatch_native_fork(service, store);
        let completed = store.get("fork").unwrap().unwrap();
        assert_eq!(completed.status, "done");
        assert!(!completed.restorable);
        assert!(completed.inverse.is_none());
        assert_eq!(
            fs::read(agents.join("skills/alpha/SKILL.md")).unwrap(),
            repaired.as_bytes()
        );
        let registry: serde_json::Value =
            serde_json::from_slice(&fs::read(agents.join("skill-studio.json")).unwrap()).unwrap();
        assert_eq!(registry["forks"]["alpha"]["base_commit"], "a".repeat(40));
        assert!(service
            .prepare_native_fork_recovery(
                &completed,
                store,
                limits,
                Some(std::time::Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .is_err());
    }

    #[test]
    fn publishes_prepared_sources_and_refuses_unplanned_drift_overlap_and_budget() {
        #[cfg(feature = "event-store")]
        let mut verified_chain_steps = 0;
        #[cfg(feature = "event-store")]
        let mut verified_unfork_preparations = 0;
        for case in [
            "present",
            "native-present",
            "native-absent",
            "native-restore-finish-failure",
            "native-restore-recover-before",
            "native-restore-cancel-before",
            "native-cancel-present",
            "native-recover-present",
            "native-recover-absent",
            "native-detached-present",
            "native-detached-absent",
            "native-detached-conflict",
            "native-resume-prepared",
            "native-dispatch-prepared",
            "native-resume-detached",
            "native-resume-registry",
            "native-resume-absent",
            "absent",
            "pending-cancelled",
            "pending-corrupt",
            "unplanned",
            "drift",
            "overlap",
            "budget",
            "upstream-drift",
            "request-mismatch",
            "parent-replaced",
        ] {
            #[cfg(feature = "event-store")]
            let verify_restore_chain = case == "native-absent";
            #[cfg(feature = "event-store")]
            let verify_unfork_preparation = case == "native-present";
            #[cfg(feature = "event-store")]
            let restore_cancel_before = case == "native-restore-cancel-before";
            #[cfg(feature = "event-store")]
            let restore_recover_before = case == "native-restore-recover-before";
            #[cfg(feature = "event-store")]
            let restore_finish_failure = case == "native-restore-finish-failure";
            #[cfg(feature = "event-store")]
            let dispatch_direct = case == "native-dispatch-prepared";
            #[cfg(feature = "event-store")]
            let resume_stage = match case {
                "native-dispatch-prepared" => Some("Prepared"),
                "native-resume-prepared" | "native-resume-absent" => Some("Prepared"),
                "native-resume-detached" => Some("ProviderDetached"),
                "native-resume-registry" => Some("RegistryPublished"),
                _ => None,
            };
            #[cfg(feature = "event-store")]
            let recover_detached = case.starts_with("native-detached-");
            #[cfg(feature = "event-store")]
            let competing_owner = case == "native-detached-conflict";
            #[cfg(feature = "event-store")]
            let recover_repaired = case.starts_with("native-recover-");
            let case = match case {
                "native-dispatch-prepared"
                | "native-restore-finish-failure"
                | "native-restore-recover-before"
                | "native-restore-cancel-before" => "native-present",
                "native-resume-prepared" | "native-resume-detached" | "native-resume-registry" => {
                    "native-present"
                }
                "native-resume-absent" => "native-absent",
                "native-detached-present" | "native-detached-conflict" => "native-present",
                "native-detached-absent" => "native-absent",
                "native-recover-present" => "native-present",
                "native-recover-absent" => "native-absent",
                case => case,
            };
            let cancel_between = case == "native-cancel-present";
            let case = if cancel_between {
                "native-present"
            } else {
                case
            };
            let native = case.starts_with("native-");
            let case = case.strip_prefix("native-").unwrap_or(case);
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let agents = home.join(".agents");
            let live = agents.join("skills/alpha");
            fs::create_dir_all(&live).unwrap();
            let document = "---\nname: alpha\ndescription: Use when: testing\n---\nbody\n";
            let lock = "version = 1\n[skills.alpha]\nsource = 'owner/repo'\nresolved_path = 'skills/alpha'\nresolved_commit = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'\n";
            let manifest = "version = 1\n[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\n";
            fs::write(live.join("SKILL.md"), document).unwrap();
            std::os::unix::fs::symlink("SKILL.md", live.join("document-link")).unwrap();
            fs::write(agents.join("agents.lock"), lock).unwrap();
            fs::write(agents.join("agents.toml"), manifest).unwrap();
            if case != "absent" {
                fs::write(
                    agents.join("skill-studio.json"),
                    "{ \"version\": 4, \"future\": true }\n",
                )
                .unwrap();
            }
            let staging = temp.path().join("staging");
            fs::create_dir(&staging).unwrap();
            let fetch_lock = if case == "request-mismatch" {
                lock.replace(&"a".repeat(40), &"b".repeat(40))
            } else {
                lock.to_string()
            };
            let fork_request =
                crate::skill_dotagents_ledger::DotagentsDetachIntent::from_documents(
                    "alpha",
                    &fetch_lock,
                    manifest,
                )
                .unwrap()
                .fork_source()
                .unwrap();
            let source = crate::skill_upstream_fetch::tests::fetch_fixture(
                &staging,
                &fork_request,
                b"upstream",
            );
            let upstream = source.path().to_path_buf();
            let state_path = if case == "overlap" {
                live.join("state")
            } else {
                temp.path().join("state")
            };
            fs::create_dir(&state_path).unwrap();
            let state = BackupStateRoot::bind(&state_path).unwrap();
            #[cfg(feature = "event-store")]
            let event_store = {
                let store = crate::skill_event_store::EventStore::open(&state_path).unwrap();
                store
                    .conn
                    .pragma_update(None, "synchronous", "FULL")
                    .unwrap();
                store
            };

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
            let preview = preview_frontmatter_repair(deployment, document.as_bytes()).unwrap();
            let repaired = preview.proposed_content.clone();
            let request = BoundFrontmatterRepairRequest {
                deployment_id: preview.deployment_id,
                proposal_id: preview.proposal_id,
                expected_content_fingerprint: preview.expected_content_fingerprint,
                mode: FrontmatterRepairApplyMode::ForkAndFix,
            };
            let mut trees = vec![state_path.clone(), live.clone()];
            if case != "unplanned" {
                trees.push(upstream.clone());
            }
            let prepared = service
                .prepare_dotagents_fork_selection(
                    &request,
                    &trees,
                    None,
                    CancellationToken::default(),
                )
                .unwrap();
            if case == "drift" {
                fs::write(agents.join("agents.toml"), "changed").unwrap();
            }
            if case == "upstream-drift" {
                fs::write(upstream.join("SKILL.md"), "changed upstream").unwrap();
            }
            if case == "parent-replaced" {
                let parent = upstream.parent().unwrap();
                fs::rename(parent, parent.with_file_name("previous-skills")).unwrap();
                fs::create_dir_all(&upstream).unwrap();
                fs::write(upstream.join("SKILL.md"), "upstream").unwrap();
            }
            let limits = BackupCopyLimits {
                max_bytes: if case == "budget" {
                    lock.len() as u64
                } else {
                    4096
                },
                max_entries: 32,
                max_depth: 8,
            };
            let result = prepared.publish_snapshots(
                &state,
                "fork",
                source,
                limits,
                &CancellationToken::default(),
            );
            assert_eq!(
                result.is_ok(),
                matches!(
                    case,
                    "present" | "absent" | "pending-cancelled" | "pending-corrupt"
                ),
                "{case}: {:?}",
                result.as_ref().err()
            );
            if let Ok(snapshots) = result {
                let reference = snapshots.reference();
                let existing = state.open_existing("fork").unwrap();
                let receipt = ForkSnapshotReceipt::read(
                    &existing,
                    &reference,
                    limits,
                    &CancellationToken::default(),
                )
                .unwrap();
                let intent = prepared
                    .fork_intent(
                        reference.clone(),
                        chrono::DateTime::parse_from_rfc3339("2026-09-11T00:00:00Z")
                            .unwrap()
                            .with_timezone(&chrono::Utc),
                    )
                    .unwrap();
                intent.validate_snapshot_source(&receipt).unwrap();
                assert_eq!(intent.registry().record().base_commit, "a".repeat(40));
                assert_eq!(intent.registry().record().skill_dir, live);
                assert_eq!(
                    intent.repair().expected_content_fingerprint,
                    prepared.preview().expected_content_fingerprint
                );
                let mut wrong_reference = serde_json::to_value(&reference).unwrap();
                wrong_reference["deployment_id"] = serde_json::json!("wrong-deployment");
                assert!(prepared
                    .fork_intent(
                        serde_json::from_value(wrong_reference).unwrap(),
                        chrono::Utc::now()
                    )
                    .is_err());
                assert_eq!(receipt.registry_before_present(), case != "absent");
                assert_eq!(
                    fs::read(state_path.join("backups/fork/provider-lock")).unwrap(),
                    lock.as_bytes()
                );
                assert_eq!(
                    fs::read(state_path.join("backups/fork/live-tree/SKILL.md")).unwrap(),
                    document.as_bytes()
                );
                assert_eq!(
                    fs::read_link(state_path.join("backups/fork/live-tree/document-link")).unwrap(),
                    Path::new("SKILL.md")
                );
                #[cfg(feature = "event-store")]
                {
                    let saved = state_path.join("backups/fork/live-tree/SKILL.md");
                    fs::write(&saved, "changed backup").unwrap();
                    let failure = prepared
                        .record_fork(
                            &event_store,
                            reference.clone(),
                            chrono::Utc::now(),
                            limits,
                            &CancellationToken::default(),
                        )
                        .unwrap_err();
                    assert!(matches!(
                        failure,
                        crate::skill_event_operations::EventWriteFailure::BeforeWrite(_)
                    ));
                    assert!(event_store.get("fork").unwrap().is_none());
                    fs::write(&saved, document).unwrap();
                    let cancelled = CancellationToken::default();
                    cancelled.cancel();
                    assert!(matches!(
                        prepared.record_fork(
                            &event_store,
                            reference.clone(),
                            chrono::Utc::now(),
                            limits,
                            &cancelled
                        ),
                        Err(crate::skill_event_operations::EventWriteFailure::BeforeWrite(_))
                    ));
                    assert!(event_store.get("fork").unwrap().is_none());
                    let token = CancellationToken::default();
                    if case == "pending-cancelled" {
                        token.cancel();
                    }
                    if case == "pending-corrupt" {
                        fs::write(&saved, "corrupt before begin").unwrap();
                    }
                    let assert_held = || {
                        use crate::skill_coordination::{
                            CoordinationMode, CoordinationPlan, DirectoryEffect,
                        };
                        assert!(CoordinationPlan::new_fixture(
                            vec![DirectoryEffect::entry(
                                agents.join("agents.lock"),
                                CoordinationMode::Exclusive
                            )],
                            temp.path(),
                            Some(std::time::Duration::from_millis(30)),
                        )
                        .unwrap()
                        .acquire()
                        .is_err());
                    };
                    match prepared.begin_fork(
                        &event_store,
                        reference.clone(),
                        chrono::Utc::now(),
                        limits,
                        &token,
                    ) {
                        Ok(mut pending) => {
                            assert!(matches!(case, "present" | "absent"));
                            pending.revalidate_selection().unwrap();
                            assert_eq!(
                                pending
                                    .observe_provider(&CancellationToken::default())
                                    .unwrap(),
                                crate::skill_dotagents_ledger::DotagentsDetachState::Attached
                            );
                            pending
                                .validate_before_execution(&event_store, limits, &token)
                                .unwrap();
                            fs::write(&saved, "changed after pending insertion").unwrap();
                            assert!(pending
                                .validate_before_execution(&event_store, limits, &token)
                                .is_err());
                            fs::write(&saved, document).unwrap();
                            pending
                                .validate_before_execution(&event_store, limits, &token)
                                .unwrap();
                            event_store
                                .conn
                                .execute(
                                    "UPDATE events SET status = 'interrupted' WHERE id = 'fork'",
                                    [],
                                )
                                .unwrap();
                            assert!(pending
                                .validate_before_execution(&event_store, limits, &token)
                                .is_err());
                            event_store
                                .conn
                                .execute(
                                    "UPDATE events SET status = 'pending' WHERE id = 'fork'",
                                    [],
                                )
                                .unwrap();
                            let cancelled = CancellationToken::default();
                            cancelled.cancel();
                            assert!(pending
                                .validate_before_execution(&event_store, limits, &cancelled)
                                .is_err());
                            pending
                                .validate_before_execution(&event_store, limits, &token)
                                .unwrap();
                            let persisted: crate::skill_fork_repair_intent::DotagentsForkRepairIntent = serde_json::from_value(event_store.get("fork").unwrap().unwrap().payload).unwrap();
                            assert_eq!(
                                persisted.provider_documents(),
                                pending.event().intent().provider_documents()
                            );
                            assert!(persisted.provider_documents().is_some());
                            persisted
                                .validate_saved_provider_documents(lock, manifest)
                                .unwrap();
                            let mut changed = serde_json::to_value(&persisted).unwrap();
                            changed["provider_documents"]["lock"] =
                                serde_json::json!("# unexpected comment\nversion = 1\n");
                            let changed: crate::skill_fork_repair_intent::DotagentsForkRepairIntent = serde_json::from_value(changed).unwrap();
                            changed.validate_for_operation("fork").unwrap();
                            assert!(changed
                                .validate_saved_provider_documents(lock, manifest)
                                .is_err());
                            assert_eq!(pending.event().id(), "fork");
                            let row = event_store.get("fork").unwrap().unwrap();
                            assert_eq!(row.status, "pending");
                            assert!(!row.restorable);
                            assert!(row.inverse.is_none());
                            assert_eq!(
                                pending.event().intent().registry().record().base_commit,
                                "a".repeat(40)
                            );
                            if native {
                                use crate::skill_native_fork_progress::NativeForkProgress;
                                use std::os::unix::fs::MetadataExt;
                                assert_native_progress(
                                    &pending,
                                    &agents,
                                    &state_path,
                                    NativeForkProgress::Prepared,
                                );
                                if resume_stage == Some("Prepared") {
                                    drop(pending);
                                    if dispatch_direct {
                                        let original = event_store.get("fork").unwrap().unwrap();
                                        let mut legacy = original.clone();
                                        legacy.payload["version"] = serde_json::json!(1);
                                        legacy
                                            .payload
                                            .as_object_mut()
                                            .unwrap()
                                            .remove("provider_documents");
                                        crate::skill_fork_repair_intent::DotagentsForkRecoveryEvent::from_row(&legacy).unwrap();
                                        event_store
                                            .conn
                                            .execute(
                                                "UPDATE events SET payload = ?1 WHERE id = 'fork'",
                                                [serde_json::to_string(&legacy.payload).unwrap()],
                                            )
                                            .unwrap();
                                        let error =
                                            crate::skill_repair_execution::recover_next_repair(
                                                &mut service,
                                                &event_store,
                                                Some(std::time::Duration::from_secs(10)),
                                                CancellationToken::default(),
                                            )
                                            .unwrap_err();
                                        assert_eq!(error.event_id, "fork");
                                        assert_eq!(error.stage, crate::skill_repair_execution::RepairExecutionStage::Prepare);
                                        assert!(error.message.contains("Native recovery requires"));
                                        assert_eq!(
                                            event_store.get("fork").unwrap().unwrap().status,
                                            "pending"
                                        );
                                        assert_eq!(
                                            fs::read(live.join("SKILL.md")).unwrap(),
                                            document.as_bytes()
                                        );
                                        event_store
                                            .conn
                                            .execute(
                                                "UPDATE events SET payload = ?1 WHERE id = 'fork'",
                                                [serde_json::to_string(&original.payload).unwrap()],
                                            )
                                            .unwrap();
                                        dispatch_native_fork(&mut service, &event_store);
                                        assert_eq!(
                                            fs::read(live.join("SKILL.md")).unwrap(),
                                            repaired.as_bytes()
                                        );
                                        continue;
                                    }
                                    resume_native_fork(
                                        &mut service,
                                        &event_store,
                                        limits,
                                        NativeForkProgress::Prepared,
                                        &agents,
                                        &repaired,
                                    );
                                    continue;
                                }
                                let before = fs::metadata(&live).unwrap();
                                if cancel_between {
                                    let error = pending
                                        .publish_detach_documents_with(
                                            &event_store,
                                            limits,
                                            &token,
                                            || token.cancel(),
                                        )
                                        .unwrap_err();
                                    assert!(matches!(error, crate::skill_document_write::DocumentWriteFailure::AfterReplace(_)));
                                    assert_eq!(
                                        fs::read_to_string(agents.join("agents.lock")).unwrap(),
                                        lock
                                    );
                                    assert_eq!(pending.observe_provider(&CancellationToken::default()).unwrap(), crate::skill_dotagents_ledger::DotagentsDetachState::Partial);
                                    pending.revalidate_selection().unwrap();
                                    assert_native_progress(
                                        &pending,
                                        &agents,
                                        &state_path,
                                        NativeForkProgress::ManifestPublished,
                                    );
                                    assert_eq!(
                                        fs::read(live.join("SKILL.md")).unwrap(),
                                        document.as_bytes()
                                    );
                                    assert_eq!(
                                        event_store.get("fork").unwrap().unwrap().status,
                                        "pending"
                                    );
                                    assert_held();
                                    let row = event_store.get("fork").unwrap().unwrap();
                                    drop(pending);
                                    let recovery = service
                                        .prepare_native_fork_recovery(
                                            &row,
                                            &event_store,
                                            limits,
                                            Some(std::time::Duration::from_secs(10)),
                                            CancellationToken::default(),
                                        )
                                        .unwrap();
                                    assert_eq!(recovery.event_id(), "fork");
                                    assert_eq!(
                                        recovery.progress(),
                                        NativeForkProgress::ManifestPublished
                                    );
                                    recovery.revalidate().unwrap();
                                    assert_held();
                                    recovery
                                        .resume(&event_store, limits, &CancellationToken::default())
                                        .unwrap();
                                    assert_eq!(
                                        event_store.get("fork").unwrap().unwrap().status,
                                        "done"
                                    );
                                    assert_eq!(
                                        fs::read(live.join("SKILL.md")).unwrap(),
                                        repaired.as_bytes()
                                    );
                                    continue;
                                }
                                assert!(pending
                                    .publish_fork_registry(&event_store, limits, &token)
                                    .is_err());
                                assert!(pending
                                    .publish_repair(&event_store, limits, &token)
                                    .is_err());
                                assert!(pending.complete(&event_store, limits, &token).is_err());
                                pending
                                    .publish_detach_documents(&event_store, limits, &token)
                                    .unwrap();
                                assert_native_progress(
                                    &pending,
                                    &agents,
                                    &state_path,
                                    NativeForkProgress::ProviderDetached,
                                );
                                if resume_stage == Some("ProviderDetached") {
                                    drop(pending);
                                    resume_native_fork(
                                        &mut service,
                                        &event_store,
                                        limits,
                                        NativeForkProgress::ProviderDetached,
                                        &agents,
                                        &repaired,
                                    );
                                    continue;
                                }
                                if recover_detached {
                                    let row = event_store.get("fork").unwrap().unwrap();
                                    drop(pending);
                                    if competing_owner {
                                        fs::write(agents.join(".skill-lock.json"), serde_json::to_vec(&serde_json::json!({
                                            "version": 3, "skills": {"alpha": {
                                                "source": "other/repo", "sourceType": "github",
                                                "sourceUrl": "https://github.com/other/repo",
                                                "skillFolderHash": "hash", "installedAt": "now", "updatedAt": "now"
                                            }}
                                        })).unwrap()).unwrap();
                                    }
                                    let recovery = service.prepare_native_fork_recovery(
                                        &row,
                                        &event_store,
                                        limits,
                                        Some(std::time::Duration::from_secs(10)),
                                        CancellationToken::default(),
                                    );
                                    if competing_owner {
                                        assert!(
                                            matches!(recovery, Err(crate::skill_service::WritePreparationError::InvalidRepairSelection(ref message)) if message.contains("competing"))
                                        );
                                    } else {
                                        let recovery = recovery.unwrap();
                                        assert_eq!(
                                            recovery.progress(),
                                            NativeForkProgress::ProviderDetached
                                        );
                                        recovery.revalidate().unwrap();
                                        assert_held();
                                        drop(recovery);
                                        let unrelated = "# unrelated edit\nversion = 1\n";
                                        fs::write(agents.join("agents.toml"), unrelated).unwrap();
                                        assert!(service
                                            .prepare_native_fork_recovery(
                                                &row,
                                                &event_store,
                                                limits,
                                                Some(std::time::Duration::from_secs(10)),
                                                CancellationToken::default(),
                                            )
                                            .is_err());
                                        assert_eq!(
                                            fs::read_to_string(agents.join("agents.toml")).unwrap(),
                                            unrelated
                                        );
                                    }
                                    assert_eq!(
                                        event_store.get("fork").unwrap().unwrap().status,
                                        "pending"
                                    );
                                    assert_eq!(
                                        fs::read(live.join("SKILL.md")).unwrap(),
                                        document.as_bytes()
                                    );
                                    continue;
                                }
                                pending.revalidate_selection().unwrap();
                                assert_eq!(
                                    pending.observe_provider(&token).unwrap(),
                                    crate::skill_dotagents_ledger::DotagentsDetachState::Detached
                                );
                                let after = fs::metadata(&live).unwrap();
                                assert_eq!(
                                    (before.dev(), before.ino()),
                                    (after.dev(), after.ino())
                                );
                                assert_eq!(
                                    fs::read(live.join("SKILL.md")).unwrap(),
                                    document.as_bytes()
                                );
                                let proposal =
                                    pending.event().intent().provider_documents().unwrap();
                                assert_eq!(
                                    fs::read_to_string(agents.join("agents.lock")).unwrap(),
                                    proposal.lock()
                                );
                                assert_eq!(
                                    fs::read_to_string(agents.join("agents.toml")).unwrap(),
                                    proposal.manifest()
                                );
                                let row = event_store.get("fork").unwrap().unwrap();
                                assert_eq!(row.status, "pending");
                                assert!(!row.restorable);
                                pending
                                    .publish_fork_registry(&event_store, limits, &token)
                                    .unwrap();
                                assert_native_progress(
                                    &pending,
                                    &agents,
                                    &state_path,
                                    NativeForkProgress::RegistryPublished,
                                );
                                if resume_stage == Some("RegistryPublished") {
                                    drop(pending);
                                    resume_native_fork(
                                        &mut service,
                                        &event_store,
                                        limits,
                                        NativeForkProgress::RegistryPublished,
                                        &agents,
                                        &repaired,
                                    );
                                    continue;
                                }
                                pending.revalidate_selection().unwrap();
                                let registry_bytes =
                                    fs::read(agents.join("skill-studio.json")).unwrap();
                                let registry: serde_json::Value =
                                    serde_json::from_slice(&registry_bytes).unwrap();
                                assert_eq!(
                                    registry["forks"]["alpha"]["base_commit"],
                                    "a".repeat(40)
                                );
                                if case == "present" {
                                    assert_eq!(registry["future"], true);
                                }
                                assert_eq!(
                                    fs::read(live.join("SKILL.md")).unwrap(),
                                    document.as_bytes()
                                );
                                assert_eq!(
                                    event_store.get("fork").unwrap().unwrap().status,
                                    "pending"
                                );
                                assert!(pending
                                    .publish_fork_registry(&event_store, limits, &token)
                                    .is_err());
                                assert!(pending.complete(&event_store, limits, &token).is_err());
                                pending
                                    .publish_repair(&event_store, limits, &token)
                                    .unwrap();
                                assert_native_progress(
                                    &pending,
                                    &agents,
                                    &state_path,
                                    NativeForkProgress::RepairPublished,
                                );
                                pending.revalidate_selection().unwrap();
                                assert_eq!(
                                    fs::read(live.join("SKILL.md")).unwrap(),
                                    repaired.as_bytes()
                                );
                                if recover_repaired {
                                    let row = event_store.get("fork").unwrap().unwrap();
                                    drop(pending);
                                    let recovery = service
                                        .prepare_native_fork_recovery(
                                            &row,
                                            &event_store,
                                            limits,
                                            Some(std::time::Duration::from_secs(10)),
                                            CancellationToken::default(),
                                        )
                                        .unwrap();
                                    assert_eq!(
                                        recovery.progress(),
                                        NativeForkProgress::RepairPublished
                                    );
                                    recovery.revalidate().unwrap();
                                    assert_held();
                                    recovery
                                        .resume(&event_store, limits, &CancellationToken::default())
                                        .unwrap();
                                    assert_eq!(
                                        event_store.get("fork").unwrap().unwrap().status,
                                        "done"
                                    );
                                    assert_eq!(
                                        fs::read(live.join("SKILL.md")).unwrap(),
                                        repaired.as_bytes()
                                    );
                                    continue;
                                }
                                event_store.conn.execute("UPDATE events SET status = 'interrupted' WHERE id = 'fork'", []).unwrap();
                                assert!(pending.complete(&event_store, limits, &token).is_err());
                                event_store
                                    .conn
                                    .execute(
                                        "UPDATE events SET status = 'pending' WHERE id = 'fork'",
                                        [],
                                    )
                                    .unwrap();
                                pending.complete(&event_store, limits, &token).unwrap();
                                let completed = event_store.get("fork").unwrap().unwrap();
                                assert_eq!(completed.status, "done");
                                assert!(!completed.restorable);
                                assert!(completed.inverse.is_none());
                                assert!(pending.complete(&event_store, limits, &token).is_err());
                                let published_lock = fs::read(agents.join("agents.lock")).unwrap();
                                fs::write(agents.join("agents.lock"), "changed published file")
                                    .unwrap();
                                assert!(pending.revalidate_selection().is_err());
                                fs::write(agents.join("agents.lock"), &published_lock).unwrap();
                                drop(pending);
                                if verify_unfork_preparation {
                                    crate::skill_unfork_preparation::verify_preparation_fixture(
                                        &mut service,
                                        &event_store,
                                        &completed,
                                        limits,
                                    );
                                    verified_unfork_preparations += 1;
                                }
                                let restore = service
                                    .prepare_fork_document_restore(
                                        &completed,
                                        &event_store,
                                        false,
                                        limits,
                                        Some(std::time::Duration::from_secs(10)),
                                        CancellationToken::default(),
                                    )
                                    .unwrap();
                                assert_eq!(restore.event_id(), "fork");
                                assert_eq!(restore.current_content(), repaired.as_bytes());
                                assert_eq!(restore.restore_content(), document.as_bytes());
                                restore
                                    .revalidate(&event_store, limits, &CancellationToken::default())
                                    .unwrap();
                                assert_held();
                                assert_eq!(
                                    fs::read(agents.join("skill-studio.json")).unwrap(),
                                    registry_bytes
                                );
                                assert_eq!(
                                    fs::read(live.join("SKILL.md")).unwrap(),
                                    repaired.as_bytes()
                                );
                                assert_eq!(
                                    serde_json::to_value(event_store.get("fork").unwrap().unwrap())
                                        .unwrap(),
                                    serde_json::to_value(&completed).unwrap()
                                );
                                event_store
                                    .conn
                                    .execute(
                                        "UPDATE events SET reverted_by = 'other' WHERE id = 'fork'",
                                        [],
                                    )
                                    .unwrap();
                                assert!(restore
                                    .revalidate(&event_store, limits, &CancellationToken::default())
                                    .is_err());
                                event_store
                                    .conn
                                    .execute(
                                        "UPDATE events SET reverted_by = NULL WHERE id = 'fork'",
                                        [],
                                    )
                                    .unwrap();
                                drop(restore);
                                fs::write(live.join("SKILL.md"), "local edit").unwrap();
                                assert!(service
                                    .prepare_fork_document_restore(
                                        &completed,
                                        &event_store,
                                        false,
                                        limits,
                                        Some(std::time::Duration::from_secs(10)),
                                        CancellationToken::default(),
                                    )
                                    .is_err());
                                let forced = service
                                    .prepare_fork_document_restore(
                                        &completed,
                                        &event_store,
                                        true,
                                        limits,
                                        Some(std::time::Duration::from_secs(10)),
                                        CancellationToken::default(),
                                    )
                                    .unwrap();
                                assert_eq!(forced.current_content(), b"local edit");
                                assert_eq!(forced.restore_content(), document.as_bytes());
                                fs::write(live.join("SKILL.md"), "later edit").unwrap();
                                assert!(forced
                                    .revalidate(&event_store, limits, &CancellationToken::default())
                                    .is_err());
                                drop(forced);
                                let mut changed_registry: serde_json::Value =
                                    serde_json::from_slice(&registry_bytes).unwrap();
                                changed_registry["forks"]["alpha"]["origin_source"] =
                                    serde_json::json!("other/repo");
                                fs::write(
                                    agents.join("skill-studio.json"),
                                    serde_json::to_vec(&changed_registry).unwrap(),
                                )
                                .unwrap();
                                assert!(service
                                    .prepare_fork_document_restore(
                                        &completed,
                                        &event_store,
                                        true,
                                        limits,
                                        Some(std::time::Duration::from_secs(10)),
                                        CancellationToken::default(),
                                    )
                                    .is_err());
                                fs::write(agents.join("skill-studio.json"), &registry_bytes)
                                    .unwrap();
                                fs::write(live.join("SKILL.md"), &repaired).unwrap();
                                let cancel_after_backup = CancellationToken::default();
                                let restore = service
                                    .prepare_fork_document_restore(
                                        &completed,
                                        &event_store,
                                        false,
                                        limits,
                                        Some(std::time::Duration::from_secs(10)),
                                        CancellationToken::default(),
                                    )
                                    .unwrap();
                                assert!(matches!(restore.record_intent_after_backup(
                                    &event_store, "cancel-after-backup", limits, &cancel_after_backup,
                                    || cancel_after_backup.cancel(),
                                ), Err(crate::skill_event_operations::EventWriteFailure::BeforeWrite(_))));
                                assert!(event_store.get("cancel-after-backup").unwrap().is_none());
                                assert!(!state_path.join("backups/cancel-after-backup").exists());
                                assert_eq!(
                                    fs::read(live.join("SKILL.md")).unwrap(),
                                    repaired.as_bytes()
                                );
                                event_store.conn.execute_batch("CREATE TRIGGER refuse_fork_restore_claim BEFORE UPDATE OF reverted_by ON events WHEN NEW.reverted_by IS NOT NULL BEGIN SELECT RAISE(ABORT, 'injected claim failure'); END;").unwrap();
                                let restore = service
                                    .prepare_fork_document_restore(
                                        &completed,
                                        &event_store,
                                        false,
                                        limits,
                                        Some(std::time::Duration::from_secs(10)),
                                        CancellationToken::default(),
                                    )
                                    .unwrap();
                                assert!(restore
                                    .record_intent(
                                        &event_store,
                                        "rejected-restore",
                                        limits,
                                        &CancellationToken::default()
                                    )
                                    .is_err());
                                assert!(event_store.get("rejected-restore").unwrap().is_none());
                                assert!(state_path.join("backups/rejected-restore").is_dir());
                                assert!(event_store
                                    .get("fork")
                                    .unwrap()
                                    .unwrap()
                                    .reverted_by
                                    .is_none());
                                event_store
                                    .conn
                                    .execute_batch("DROP TRIGGER refuse_fork_restore_claim")
                                    .unwrap();
                                let restore = service
                                    .prepare_fork_document_restore(
                                        &completed,
                                        &event_store,
                                        false,
                                        limits,
                                        Some(std::time::Duration::from_secs(10)),
                                        CancellationToken::default(),
                                    )
                                    .unwrap();
                                let pending_restore = restore
                                    .record_intent(
                                        &event_store,
                                        "restore-fork",
                                        limits,
                                        &CancellationToken::default(),
                                    )
                                    .unwrap();
                                assert_eq!(
                                    pending_restore.source_event().reverted_by.as_deref(),
                                    Some("restore-fork")
                                );
                                assert_eq!(pending_restore.event().status, "pending");
                                assert_eq!(pending_restore.event().kind, "restore_fork_document");
                                assert_eq!(pending_restore.current_content(), repaired.as_bytes());
                                assert_eq!(pending_restore.restore_content(), document.as_bytes());
                                let intent: crate::skill_fork_document_restore::ForkDocumentRestoreIntent = serde_json::from_value(pending_restore.event().payload.clone()).unwrap();
                                intent.validate_record("restore-fork").unwrap();
                                assert!(intent.validate_record("fork").is_err());
                                assert_eq!(
                                    fs::read(live.join("SKILL.md")).unwrap(),
                                    repaired.as_bytes()
                                );
                                assert_eq!(
                                    fs::read(agents.join("skill-studio.json")).unwrap(),
                                    registry_bytes
                                );
                                assert_eq!(
                                    fs::read(agents.join("agents.lock")).unwrap(),
                                    published_lock
                                );
                                assert_held();
                                let check_pending = || {
                                    pending_restore.revalidate(
                                        &event_store,
                                        limits,
                                        &CancellationToken::default(),
                                    )
                                };
                                check_pending().unwrap();
                                event_store
                                    .conn
                                    .execute(
                                        "UPDATE events SET reverted_by = NULL WHERE id = ?1",
                                        [&completed.id],
                                    )
                                    .unwrap();
                                assert!(check_pending().is_err());
                                event_store.conn.execute(
                                    "UPDATE events SET reverted_by = 'restore-fork' WHERE id = ?1",
                                    [&completed.id],
                                ).unwrap();
                                event_store.conn.execute(
                                    "UPDATE events SET status = 'done' WHERE id = 'restore-fork'", [],
                                ).unwrap();
                                assert!(check_pending().is_err());
                                event_store.conn.execute(
                                    "UPDATE events SET status = 'pending' WHERE id = 'restore-fork'", [],
                                ).unwrap();
                                check_pending().unwrap();
                                if restore_cancel_before {
                                    let cancelled = CancellationToken::default();
                                    cancelled.cancel();
                                    assert!(pending_restore
                                        .execute(&event_store, limits, &cancelled)
                                        .is_err());
                                    let source = event_store.get(&completed.id).unwrap().unwrap();
                                    let row = event_store.get("restore-fork").unwrap().unwrap();
                                    event_store.conn.execute_batch("CREATE TRIGGER refuse_cancel_claim BEFORE UPDATE OF reverted_by ON events WHEN OLD.reverted_by IS NOT NULL AND NEW.reverted_by IS NULL BEGIN SELECT RAISE(ABORT, 'injected cancel claim failure'); END;").unwrap();
                                    let recovery = service
                                        .prepare_fork_document_restore_recovery(
                                            &source,
                                            &row,
                                            &event_store,
                                            limits,
                                            Some(std::time::Duration::from_secs(10)),
                                            CancellationToken::default(),
                                        )
                                        .unwrap();
                                    assert!(recovery
                                        .cancel_unapplied(
                                            &event_store,
                                            &CancellationToken::default()
                                        )
                                        .is_err());
                                    assert_eq!(
                                        event_store.get("restore-fork").unwrap().unwrap().status,
                                        "pending"
                                    );
                                    assert_eq!(
                                        event_store
                                            .get(&completed.id)
                                            .unwrap()
                                            .unwrap()
                                            .reverted_by
                                            .as_deref(),
                                        Some("restore-fork")
                                    );
                                    event_store
                                        .conn
                                        .execute_batch("DROP TRIGGER refuse_cancel_claim")
                                        .unwrap();
                                    let recovery = service
                                        .prepare_fork_document_restore_recovery(
                                            &source,
                                            &row,
                                            &event_store,
                                            limits,
                                            Some(std::time::Duration::from_secs(10)),
                                            CancellationToken::default(),
                                        )
                                        .unwrap();
                                    assert_eq!(recovery.cancel_unapplied(&event_store, &CancellationToken::default()).unwrap(), crate::skill_repair_execution::RepairRecoveryOutcome::NotApplied);
                                    assert_eq!(
                                        event_store.get("restore-fork").unwrap().unwrap().status,
                                        "failed"
                                    );
                                    assert!(event_store
                                        .get(&completed.id)
                                        .unwrap()
                                        .unwrap()
                                        .reverted_by
                                        .is_none());
                                    assert_eq!(
                                        fs::read(live.join("SKILL.md")).unwrap(),
                                        repaired.as_bytes()
                                    );
                                    assert_eq!(
                                        fs::read(agents.join("skill-studio.json")).unwrap(),
                                        registry_bytes
                                    );
                                    assert_eq!(
                                        fs::read(agents.join("agents.lock")).unwrap(),
                                        published_lock
                                    );
                                    drop(
                                        service
                                            .prepare_fork_document_restore(
                                                &completed,
                                                &event_store,
                                                false,
                                                limits,
                                                Some(std::time::Duration::from_secs(10)),
                                                CancellationToken::default(),
                                            )
                                            .unwrap(),
                                    );
                                    continue;
                                }
                                if verify_restore_chain
                                    || restore_finish_failure
                                    || restore_recover_before
                                {
                                    if restore_finish_failure {
                                        event_store.conn.execute_batch("CREATE TRIGGER refuse_restore_completion BEFORE UPDATE OF restorable ON events WHEN NEW.id = 'restore-fork' BEGIN SELECT RAISE(ABORT, 'injected completion failure'); END;").unwrap();
                                    }
                                    let result = if restore_recover_before {
                                        drop(pending_restore);
                                        event_store.conn.execute("UPDATE events SET status = 'interrupted' WHERE id = 'restore-fork'", []).unwrap();
                                        let source =
                                            event_store.get(&completed.id).unwrap().unwrap();
                                        let restore =
                                            event_store.get("restore-fork").unwrap().unwrap();
                                        fs::write(live.join("SKILL.md"), "unrelated local edit")
                                            .unwrap();
                                        assert!(service
                                            .prepare_fork_document_restore_recovery(
                                                &source,
                                                &restore,
                                                &event_store,
                                                limits,
                                                Some(std::time::Duration::from_secs(10)),
                                                CancellationToken::default()
                                            )
                                            .is_err());
                                        assert_eq!(
                                            fs::read_to_string(live.join("SKILL.md")).unwrap(),
                                            "unrelated local edit"
                                        );
                                        fs::write(live.join("SKILL.md"), &repaired).unwrap();
                                        let recovery = service
                                            .prepare_fork_document_restore_recovery(
                                                &source,
                                                &restore,
                                                &event_store,
                                                limits,
                                                Some(std::time::Duration::from_secs(10)),
                                                CancellationToken::default(),
                                            )
                                            .unwrap();
                                        assert_held();
                                        drop(recovery);
                                        let step =
                                            crate::skill_repair_execution::recover_next_repair(
                                                &mut service,
                                                &event_store,
                                                Some(std::time::Duration::from_secs(10)),
                                                CancellationToken::default(),
                                            )
                                            .unwrap();
                                        assert!(
                                            matches!(step, crate::skill_repair_execution::RepairRecoveryStep::Resolved { ref event_id, outcome: crate::skill_repair_execution::RepairRecoveryOutcome::Applied } if event_id == "restore-fork")
                                        );
                                        assert!(matches!(
                                            crate::skill_repair_execution::recover_next_repair(
                                                &mut service,
                                                &event_store,
                                                Some(std::time::Duration::from_secs(10)),
                                                CancellationToken::default()
                                            )
                                            .unwrap(),
                                            crate::skill_repair_execution::RepairRecoveryStep::Idle
                                        ));
                                        Ok(restore.id)
                                    } else {
                                        pending_restore
                                            .execute(
                                                &event_store,
                                                limits,
                                                &CancellationToken::default(),
                                            )
                                            .map(|receipt| receipt.event_id)
                                    };
                                    let row = event_store.get("restore-fork").unwrap().unwrap();
                                    if restore_finish_failure {
                                        assert!(
                                            matches!(result, Err(ref error) if error.stage == crate::skill_repair_execution::RepairExecutionStage::Finish)
                                        );
                                        assert_eq!(row.status, "pending");
                                        assert!(row.inverse.is_none());
                                        assert!(!row.restorable);
                                        event_store
                                            .conn
                                            .execute_batch("DROP TRIGGER refuse_restore_completion")
                                            .unwrap();
                                        let source =
                                            event_store.get(&completed.id).unwrap().unwrap();
                                        let recovery = service
                                            .prepare_fork_document_restore_recovery(
                                                &source,
                                                &row,
                                                &event_store,
                                                limits,
                                                Some(std::time::Duration::from_secs(10)),
                                                CancellationToken::default(),
                                            )
                                            .unwrap();
                                        assert_held();
                                        assert_eq!(recovery.cancel_unapplied(&event_store, &CancellationToken::default()).unwrap(), crate::skill_repair_execution::RepairRecoveryOutcome::Applied);
                                        assert_eq!(
                                            event_store
                                                .get("restore-fork")
                                                .unwrap()
                                                .unwrap()
                                                .status,
                                            "done"
                                        );
                                        assert!(service
                                            .prepare_fork_document_restore_recovery(
                                                &source,
                                                &row,
                                                &event_store,
                                                limits,
                                                Some(std::time::Duration::from_secs(10)),
                                                CancellationToken::default()
                                            )
                                            .is_err());
                                    } else {
                                        assert_eq!(result.unwrap(), "restore-fork");
                                        assert_eq!(row.status, "done");
                                        assert!(row.restorable);
                                        let inverse: crate::skill_event::InverseOp =
                                            serde_json::from_value(row.inverse.unwrap()).unwrap();
                                        assert!(
                                            matches!(inverse, crate::skill_event::InverseOp::RestoreBackup { path, post_fingerprint: Some(_), .. } if path == live.join("SKILL.md"))
                                        );
                                    }
                                    assert_eq!(
                                        fs::read(live.join("SKILL.md")).unwrap(),
                                        document.as_bytes()
                                    );
                                    assert_eq!(
                                        fs::read(agents.join("skill-studio.json")).unwrap(),
                                        registry_bytes
                                    );
                                    assert_eq!(
                                        fs::read(agents.join("agents.lock")).unwrap(),
                                        published_lock
                                    );
                                    assert_eq!(
                                        fs::read(
                                            event_store
                                                .app_data
                                                .join("backups/restore-fork/0-SKILL.md")
                                        )
                                        .unwrap(),
                                        repaired.as_bytes()
                                    );
                                    assert_eq!(
                                        event_store
                                            .get(&completed.id)
                                            .unwrap()
                                            .unwrap()
                                            .reverted_by
                                            .as_deref(),
                                        Some("restore-fork")
                                    );
                                    if verify_restore_chain {
                                        let mut source_id = "restore-fork";
                                        for (id, expected, force) in [
                                            ("redo-fork", repaired.as_bytes(), false),
                                            ("undo-again", document.as_bytes(), false),
                                            ("forced-redo", repaired.as_bytes(), true),
                                            (
                                                "restore-local-edit",
                                                b"local edit before forced redo".as_slice(),
                                                false,
                                            ),
                                        ] {
                                            let source =
                                                event_store.get(source_id).unwrap().unwrap();
                                            if force {
                                                fs::write(
                                                    live.join("SKILL.md"),
                                                    b"local edit before forced redo",
                                                )
                                                .unwrap();
                                                assert!(service
                                                    .prepare_fork_document_restore(
                                                        &source,
                                                        &event_store,
                                                        false,
                                                        limits,
                                                        Some(std::time::Duration::from_secs(10)),
                                                        CancellationToken::default()
                                                    )
                                                    .is_err());
                                            }
                                            let prepared = service
                                                .prepare_fork_document_restore(
                                                    &source,
                                                    &event_store,
                                                    force,
                                                    limits,
                                                    Some(std::time::Duration::from_secs(10)),
                                                    CancellationToken::default(),
                                                )
                                                .unwrap();
                                            assert_eq!(prepared.event_id(), source_id);
                                            assert_eq!(prepared.restore_content(), expected);
                                            let pending = prepared
                                                .record_intent(
                                                    &event_store,
                                                    id,
                                                    limits,
                                                    &CancellationToken::default(),
                                                )
                                                .unwrap();
                                            if id == "restore-local-edit" {
                                                drop(pending);
                                                let step = crate::skill_repair_execution::recover_next_repair(&mut service, &event_store, Some(std::time::Duration::from_secs(10)), CancellationToken::default()).unwrap();
                                                assert!(
                                                    matches!(step, crate::skill_repair_execution::RepairRecoveryStep::Resolved { event_id, .. } if event_id == id)
                                                );
                                            } else {
                                                pending
                                                    .execute(
                                                        &event_store,
                                                        limits,
                                                        &CancellationToken::default(),
                                                    )
                                                    .unwrap();
                                            }
                                            assert_eq!(
                                                fs::read(live.join("SKILL.md")).unwrap(),
                                                expected
                                            );
                                            assert_eq!(
                                                fs::read(agents.join("skill-studio.json")).unwrap(),
                                                registry_bytes
                                            );
                                            assert_eq!(
                                                fs::read(agents.join("agents.lock")).unwrap(),
                                                published_lock
                                            );
                                            assert_eq!(
                                                event_store
                                                    .get(source_id)
                                                    .unwrap()
                                                    .unwrap()
                                                    .reverted_by
                                                    .as_deref(),
                                                Some(id)
                                            );
                                            assert_eq!(
                                                event_store.get(id).unwrap().unwrap().status,
                                                "done"
                                            );
                                            assert!(service
                                                .prepare_fork_document_restore(
                                                    &source,
                                                    &event_store,
                                                    true,
                                                    limits,
                                                    Some(std::time::Duration::from_secs(10)),
                                                    CancellationToken::default()
                                                )
                                                .is_err());
                                            verified_chain_steps += 1;
                                            source_id = id;
                                        }
                                        let source = event_store.get(source_id).unwrap().unwrap();
                                        event_store.conn.execute("UPDATE events SET reverted_by = 'wrong-claim' WHERE id = 'redo-fork'", []).unwrap();
                                        assert!(service
                                            .prepare_fork_document_restore(
                                                &source,
                                                &event_store,
                                                true,
                                                limits,
                                                Some(std::time::Duration::from_secs(10)),
                                                CancellationToken::default()
                                            )
                                            .is_err());
                                        assert_eq!(
                                            fs::read(live.join("SKILL.md")).unwrap(),
                                            b"local edit before forced redo"
                                        );
                                    }
                                    continue;
                                }
                                fs::write(
                                    event_store.app_data.join("backups/restore-fork/0-SKILL.md"),
                                    "corrupted overwritten document",
                                )
                                .unwrap();
                                assert!(check_pending().is_err());
                                assert_eq!(
                                    fs::read(live.join("SKILL.md")).unwrap(),
                                    repaired.as_bytes()
                                );
                                assert_eq!(
                                    fs::read(agents.join("skill-studio.json")).unwrap(),
                                    registry_bytes
                                );
                                drop(pending_restore);
                                assert!(service
                                    .prepare_fork_document_restore(
                                        &completed,
                                        &event_store,
                                        false,
                                        limits,
                                        Some(std::time::Duration::from_secs(10)),
                                        CancellationToken::default(),
                                    )
                                    .is_err());
                                continue;
                            } else {
                                run_pending_provider_fixture(
                                    &pending,
                                    &event_store,
                                    &agents,
                                    limits,
                                    case == "absent",
                                );
                            }
                            assert_held();
                        }
                        Err(failure) => {
                            assert!(
                                matches!(case, "pending-cancelled" | "pending-corrupt"),
                                "{}",
                                failure.failure()
                            );
                            assert_eq!(failure.operation_id(), "fork");
                            assert!(matches!(
                                failure.failure(),
                                crate::skill_event_operations::EventWriteFailure::BeforeWrite(_)
                            ));
                            failure.revalidate_selection().unwrap();
                            assert!(event_store.get("fork").unwrap().is_none());
                            if case == "pending-cancelled" {
                                failure.verify_operation_absent(&event_store).unwrap();
                                snapshots.discard().unwrap();
                                assert!(!state_path.join("backups/fork").exists());
                            }
                            assert_held();
                        }
                    }
                }
                #[cfg(not(feature = "event-store"))]
                prepared.revalidate().unwrap();
            } else {
                assert!(!state_path.join("backups/fork/fork-snapshots.json").exists());
                if case != "budget" {
                    assert!(!state_path.join("backups").exists());
                }
            }
            if cfg!(feature = "event-store") && !native && matches!(case, "present" | "absent") {
                assert!(!live.exists());
                assert_eq!(
                    fs::read(state_path.join("backups/fork/live-tree/SKILL.md")).unwrap(),
                    document.as_bytes()
                );
            } else {
                assert_eq!(
                    fs::read(live.join("SKILL.md")).unwrap(),
                    if native && cfg!(feature = "event-store") {
                        repaired.as_bytes()
                    } else {
                        document.as_bytes()
                    }
                );
            }
            assert_eq!(
                fs::read(upstream.join("SKILL.md")).unwrap(),
                if case == "upstream-drift" {
                    b"changed upstream".as_slice()
                } else {
                    b"upstream".as_slice()
                }
            );
        }
        #[cfg(feature = "event-store")]
        assert_eq!(
            verified_chain_steps, 4,
            "The complete undo/redo chain must execute"
        );
        #[cfg(feature = "event-store")]
        assert_eq!(
            verified_unfork_preparations, 1,
            "Unfork preparation checks must execute"
        );
    }
}
