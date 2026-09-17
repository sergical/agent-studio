use super::event_store::{EventRow, EventStore};
use skill_studio_core::{
    skill_backup_reservation::BackupCopyLimits,
    skill_repair_execution::RepairRecoveryOutcome,
    skill_service::{CancellationToken, ScopedSkillService, SkillScope},
};
use std::time::Duration;

pub(crate) fn is_fork_event(kind: &str) -> bool {
    matches!(kind, "repair_dotagents_fork" | "restore_fork_document")
}

fn limits() -> BackupCopyLimits {
    BackupCopyLimits {
        max_bytes: 256 * 1024 * 1024,
        max_entries: 20_000,
        max_depth: 64,
    }
}

pub(crate) fn ensure_merge_base(
    scope: SkillScope,
    store: &EventStore,
    name: &str,
) -> Result<(), String> {
    use skill_studio_core::{
        skill_fork_registry::{fork_snapshot_dir, read_fork_registry},
        skill_fork_repair_intent::DotagentsForkRepairIntent,
    };
    let base = fork_snapshot_dir(&store.app_data, name);
    match std::fs::symlink_metadata(&base) {
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Cannot inspect {name}'s merge base: {error}")),
    }
    let registry = read_fork_registry(&scope.home)?;
    let record = registry
        .forks
        .get(name)
        .ok_or_else(|| format!("`{name}` is not forked"))?;
    let expected = serde_json::to_value(record).map_err(|error| error.to_string())?;
    let ids = store.conn.prepare(
        "SELECT id FROM events WHERE kind = 'repair_dotagents_fork' AND status = 'done' AND skill = ?1 ORDER BY rowid DESC LIMIT 1001",
    ).map_err(|error| error.to_string())?
        .query_map([name], |row| row.get::<_, String>(0)).map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())?;
    if ids.len() > 1000 {
        return Err("Fork history exceeds merge-base lookup limit".into());
    }
    let mut source = None;
    for id in ids {
        let row = store
            .get(&id)?
            .ok_or("Fork history changed during merge-base lookup")?;
        let intent: DotagentsForkRepairIntent = serde_json::from_value(row.payload.clone())
            .map_err(|error| format!("Invalid fork history {id}: {error}"))?;
        intent.validate_for_operation(&id)?;
        if serde_json::to_value(intent.registry().record()).map_err(|error| error.to_string())?
            == expected
        {
            if source.is_some() {
                return Err("Multiple fork events match the current merge base".into());
            }
            source = Some(row);
        }
    }
    let source = source.ok_or("No completed fork event matches the current missing merge base")?;
    let _transaction = super::skill_md_write::begin_skill_md_write_transaction()?;
    let mut service = ScopedSkillService::bind(scope).map_err(|error| error.to_string())?;
    let cancellation = CancellationToken::default();
    let prepared = service
        .prepare_fork_merge_base(
            &source,
            store,
            limits(),
            Some(Duration::from_secs(30)),
            cancellation.clone(),
        )
        .map_err(|error| error.to_string())?;
    prepared.publish(store, limits(), &cancellation)?;
    Ok(())
}

pub(crate) fn restore(
    service: &mut ScopedSkillService,
    store: &EventStore,
    source: &EventRow,
    force: bool,
    id: &str,
    cancellation: CancellationToken,
) -> Result<(), String> {
    if !is_fork_event(&source.kind) {
        return Err("Event is not a native fork document operation".into());
    }
    super::skill_document_operation::check_document_cancellation(&cancellation)?;
    let result = (|| {
        let prepared = service
            .prepare_fork_document_restore(
                source,
                store,
                force,
                limits(),
                Some(Duration::from_secs(30)),
                cancellation.clone(),
            )
            .map_err(|error| error.to_string())?;
        let pending = prepared
            .record_intent(store, id, limits(), &cancellation)
            .map_err(|error| error.to_string())?;
        pending
            .execute(store, limits(), &cancellation)
            .map(|_| ())
            .map_err(|error| error.to_string())
    })();
    let Err(error) = result else {
        return Ok(());
    };
    let Some(row) = store.get(id)? else {
        return Err(error);
    };
    if matches!(row.status.as_str(), "pending" | "interrupted") {
        let target_id = row
            .payload
            .get("target_event")
            .and_then(serde_json::Value::as_str)
            .ok_or("Fork restore history has no source event")?;
        let source = store
            .get(target_id)?
            .ok_or("Fork restore source is unavailable")?;
        let cleanup = CancellationToken::default();
        let recovery = service
            .prepare_fork_document_restore_recovery(
                &source,
                &row,
                store,
                limits(),
                Some(Duration::from_secs(30)),
                cleanup.clone(),
            )
            .map_err(|recovery| format!("{error}; recovery remains unresolved: {recovery}"))?;
        let recovered = if cancellation.is_cancelled() {
            recovery.cancel_unapplied(store, &cleanup)
        } else {
            recovery.recover(store, &cleanup)
        }
        .map_err(|recovery| format!("{error}; recovery remains unresolved: {recovery}"))?;
        if recovered == RepairRecoveryOutcome::NotApplied {
            return Err(error);
        }
    }
    if store.get(id)?.is_some_and(|row| row.status == "done") {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(test)]
#[path = "skill_unfork_provider_fixture.rs"]
mod provider_fixture;

#[cfg(test)]
mod tests {
    use super::*;
    use skill_studio_core::{
        skill_frontmatter_repair::BoundFrontmatterRepairRequest,
        skill_frontmatter_repair::{preview_frontmatter_repair, FrontmatterRepairApplyMode},
    };
    use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

    struct FixtureTextMerge;
    impl skill_studio_core::skill_fork_pull::ForkPullTextMerge for FixtureTextMerge {
        fn merge(
            &self,
            mine: &[u8],
            base: &[u8],
            theirs: &[u8],
            _: &Path,
            _: u64,
        ) -> Result<skill_studio_core::skill_fork_pull::ForkPullTextMergeResult, String> {
            use skill_studio_core::skill_fork_pull::ForkPullTextMergeResult;
            if mine == base {
                return Ok(ForkPullTextMergeResult::Clean(theirs.to_vec()));
            }
            if theirs == base {
                return Ok(ForkPullTextMergeResult::Clean(mine.to_vec()));
            }
            let mut output = b"<<<<<<< mine\n".to_vec();
            output.extend_from_slice(mine);
            output.extend_from_slice(b"=======\n");
            output.extend_from_slice(theirs);
            output.extend_from_slice(b">>>>>>> theirs\n");
            Ok(ForkPullTextMergeResult::Conflicts(output))
        }
    }

    fn pull_fork_upstream_durably(
        service: &mut ScopedSkillService,
        store: &EventStore,
        to_commit: char,
        upstream: &[u8],
    ) -> skill_studio_core::skill_fork_pull::ForkPullResult {
        use skill_studio_core::{
            skill_fork_pull::{
                commit_fork_pull, prepare_fork_pull_inputs, ForkPullPreparationOutcome,
                ForkPullRequest,
            },
            skill_ownership::LifecycleOwnerKind,
        };
        let inventory = service
            .scan(
                Some(&std::collections::BTreeSet::from(["alpha".into()])),
                None,
            )
            .unwrap();
        let deployment = inventory
            .skills
            .iter()
            .flat_map(|skill| &skill.deployments)
            .find(|deployment| deployment.owner_kind == LifecycleOwnerKind::Fork)
            .unwrap();
        let request = ForkPullRequest {
            deployment_id: deployment.id.clone(),
            expected_owner_revision: deployment.owner_revision.clone().unwrap(),
            to_commit: to_commit.to_string().repeat(40),
        };
        let preparation = match prepare_fork_pull_inputs(
            service,
            store,
            &request,
            limits(),
            Some(Duration::from_secs(10)),
            CancellationToken::default(),
        )
        .unwrap()
        {
            ForkPullPreparationOutcome::Prepared(preparation) => preparation,
            ForkPullPreparationOutcome::UpToDate(_) => panic!("fixture Pull must advance"),
        };
        let upstream_dir = tempfile::tempdir_in(&store.app_data).unwrap();
        fs::write(upstream_dir.path().join("SKILL.md"), upstream).unwrap();
        commit_fork_pull(
            service,
            store,
            preparation,
            upstream_dir.path(),
            &FixtureTextMerge,
            limits(),
            Some(Duration::from_secs(10)),
            CancellationToken::default(),
        )
        .unwrap()
    }
    struct FixtureReinstall<'a> {
        expected: &'a skill_studio_core::skill_fork_registry::ForkRecord,
        calls: std::cell::Cell<usize>,
        fail: bool,
    }
    impl super::super::skill_fork::LedgerTool for FixtureReinstall<'_> {
        fn remove(
            &self,
            _: skill_studio_core::skill_fork_registry::OriginTool,
            _: &str,
        ) -> Result<(), String> {
            panic!("Unfork must reinstall, not remove through the provider");
        }
        fn reinstall(
            &self,
            record: &skill_studio_core::skill_fork_registry::ForkRecord,
            name: &str,
        ) -> Result<(), String> {
            assert_eq!(name, "alpha");
            assert_eq!(
                serde_json::to_value(record).unwrap(),
                serde_json::to_value(self.expected).unwrap()
            );
            self.calls.set(self.calls.get() + 1);
            if self.fail {
                return Err("fixture provider refused reinstall".into());
            }
            fs::write(
                record.skill_dir.join("SKILL.md"),
                b"provider restored document\n",
            )
            .map_err(|error| error.to_string())
        }
    }

    #[test]
    fn desktop_native_fork_history_round_trip_and_startup_recovery() {
        native_fork_round_trip(false, None);
    }

    #[test]
    fn desktop_native_fork_recovers_failed_apply_completion() {
        native_fork_round_trip(true, None);
    }

    #[test]
    #[ignore = "requires a copied system-linked Node/provider fixture and sandbox-exec permission"]
    fn desktop_real_provider_unfork_publication_and_startup() {
        let provider = provider_fixture::ProviderFixture::load();
        native_fork_round_trip(false, Some(&provider));
    }

    fn native_fork_round_trip(
        fail_completion: bool,
        provider: Option<&provider_fixture::ProviderFixture>,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let agents = home.join(".agents");
        let live = agents.join("skills/alpha");
        fs::create_dir_all(&live).unwrap();
        let original = b"---\nname: alpha\ndescription: Use when: testing\n---\nbody\n";
        fs::write(live.join("SKILL.md"), original).unwrap();
        let lock = "version = 1\n[skills.alpha]\nsource = 'owner/repo'\nresolved_path = 'skills/alpha'\nresolved_commit = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'\n";
        let manifest = "version = 1\n[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\n";
        fs::write(agents.join("agents.lock"), lock).unwrap();
        fs::write(agents.join("agents.toml"), manifest).unwrap();
        fs::write(
            agents.join("skill-studio.json"),
            "{\"version\":4,\"future\":true}\n",
        )
        .unwrap();
        let archive_root = temp.path().join("archive-tree");
        fs::create_dir_all(archive_root.join("repo/skills/alpha")).unwrap();
        fs::write(
            archive_root.join("repo/skills/alpha/SKILL.md"),
            b"upstream snapshot",
        )
        .unwrap();
        let gh = temp.path().join("fixture-gh");
        let archive = temp.path().join("fixture-gh.archive");
        assert!(Command::new("/usr/bin/tar")
            .env("COPYFILE_DISABLE", "1")
            .args(["--format=ustar", "-czf"])
            .arg(&archive)
            .arg("-C")
            .arg(&archive_root)
            .arg("repo")
            .status()
            .unwrap()
            .success());
        fs::write(
            &gh,
            b"#!/bin/sh\n[ \"$1\" = api ] || exit 11\nexec /bin/cat \"$0.archive\"\n",
        )
        .unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o700)).unwrap();
        let store = EventStore::open(&temp.path().join("state")).unwrap();
        store
            .conn
            .pragma_update(None, "synchronous", "FULL")
            .unwrap();
        let scope = SkillScope {
            home: home.clone(),
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        };
        let mut service = ScopedSkillService::bind(scope.clone()).unwrap();
        let inventory = service.scan(None, None).unwrap();
        let deployment = inventory
            .skills
            .iter()
            .flat_map(|skill| &skill.deployments)
            .find(|deployment| deployment.path == live.to_string_lossy())
            .unwrap();
        assert!(super::super::skill_native_fork::supports(
            deployment,
            &scope.home
        ));
        let mut other_owner = deployment.clone();
        other_owner.owner_kind = skill_studio_core::skill_ownership::LifecycleOwnerKind::SkillsSh;
        assert!(!super::super::skill_native_fork::supports(
            &other_owner,
            &scope.home
        ));
        let mut other_path = deployment.clone();
        other_path.path = temp
            .path()
            .join("project/.agents/skills/alpha")
            .to_string_lossy()
            .into_owned();
        assert!(!super::super::skill_native_fork::supports(
            &other_path,
            &scope.home
        ));
        fs::rename(agents.join("agents.toml"), agents.join("agents.toml.saved")).unwrap();
        assert!(!super::super::skill_native_fork::supports(
            deployment,
            &scope.home
        ));
        fs::rename(agents.join("agents.toml.saved"), agents.join("agents.toml")).unwrap();
        let preview = preview_frontmatter_repair(deployment, original).unwrap();
        let repaired = preview.proposed_content.clone();
        let request = BoundFrontmatterRepairRequest {
            deployment_id: preview.deployment_id,
            proposal_id: preview.proposal_id,
            expected_content_fingerprint: preview.expected_content_fingerprint,
            mode: FrontmatterRepairApplyMode::ForkAndFix,
        };
        let cancelled_apply = CancellationToken::default();
        cancelled_apply.cancel();
        assert!(super::super::skill_native_fork::apply(
            &mut service,
            &store,
            &request,
            &gh,
            "cancelled-fork",
            cancelled_apply
        )
        .is_err());
        assert!(store.get("cancelled-fork").unwrap().is_none());
        assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), original);
        assert_eq!(
            fs::read_to_string(agents.join("agents.lock")).unwrap(),
            lock
        );
        if fail_completion {
            store.conn.execute_batch("CREATE TRIGGER refuse_forward_completion BEFORE UPDATE OF status ON events WHEN NEW.id = 'fork' AND NEW.status = 'done' BEGIN SELECT RAISE(ABORT, 'injected forward completion failure'); END;").unwrap();
        }
        {
            let _transaction =
                super::super::skill_md_write::begin_skill_md_write_transaction().unwrap();
            let result = super::super::skill_native_fork::apply(
                &mut service,
                &store,
                &request,
                &gh,
                "fork",
                CancellationToken::default(),
            );
            if fail_completion {
                assert!(result
                    .unwrap_err()
                    .contains("fork recovery remains unresolved"));
                assert_eq!(store.get("fork").unwrap().unwrap().status, "pending");
                assert_eq!(
                    fs::read(live.join("SKILL.md")).unwrap(),
                    repaired.as_bytes()
                );
            } else {
                result.unwrap();
            }
        }
        if fail_completion {
            store
                .conn
                .execute_batch("DROP TRIGGER refuse_forward_completion")
                .unwrap();
            super::super::skill_startup_recovery::recover_all_with_worker(
                scope.clone(),
                &store,
                &|| panic!("Fork recovery launched a direct worker"),
            )
            .unwrap();
        }
        assert_eq!(
            store.get("fork").unwrap().unwrap().kind,
            "repair_dotagents_fork"
        );
        assert_eq!(store.get("fork").unwrap().unwrap().status, "done");
        assert_eq!(
            fs::read(live.join("SKILL.md")).unwrap(),
            repaired.as_bytes()
        );
        let registry = fs::read(agents.join("skill-studio.json")).unwrap();
        let provider_lock = fs::read(agents.join("agents.lock")).unwrap();
        let provider_manifest = fs::read(agents.join("agents.toml")).unwrap();
        let mut source_id = "fork";
        for (id, expected) in [("undo", original.as_slice()), ("redo", repaired.as_bytes())] {
            let source = store.get(source_id).unwrap().unwrap();
            let _transaction =
                super::super::skill_md_write::begin_skill_md_write_transaction().unwrap();
            if id == "redo" {
                store.conn.execute_batch("CREATE TRIGGER refuse_desktop_completion BEFORE UPDATE OF restorable ON events WHEN NEW.id = 'redo' BEGIN SELECT RAISE(ABORT, 'injected desktop completion failure'); END;").unwrap();
            }
            let result = restore(
                &mut service,
                &store,
                &source,
                false,
                id,
                CancellationToken::default(),
            );
            if id == "redo" {
                assert!(result.unwrap_err().contains("recovery remains unresolved"));
                assert_eq!(store.get(id).unwrap().unwrap().status, "pending");
                assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), expected);
                store
                    .conn
                    .execute_batch("DROP TRIGGER refuse_desktop_completion")
                    .unwrap();
                drop(_transaction);
                super::super::skill_startup_recovery::recover_all_with_worker(
                    scope.clone(),
                    &store,
                    &|| panic!("Fork recovery launched a direct worker"),
                )
                .unwrap();
            } else {
                result.unwrap();
            }
            assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), expected);
            assert_eq!(store.get(id).unwrap().unwrap().status, "done");
            assert_eq!(
                store
                    .get(source_id)
                    .unwrap()
                    .unwrap()
                    .reverted_by
                    .as_deref(),
                Some(id)
            );
            source_id = id;
        }
        let source = store.get(source_id).unwrap().unwrap();
        let local_edit = b"user edit before forced restore";
        fs::write(live.join("SKILL.md"), local_edit).unwrap();
        assert!(restore(
            &mut service,
            &store,
            &source,
            false,
            "refused-local-edit",
            CancellationToken::default()
        )
        .is_err());
        assert!(store.get("refused-local-edit").unwrap().is_none());
        assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), local_edit);
        for (id, expected, force) in [
            ("forced-undo", original.as_slice(), true),
            ("recover-user-edit", local_edit.as_slice(), false),
        ] {
            let source = store.get(source_id).unwrap().unwrap();
            let _transaction =
                super::super::skill_md_write::begin_skill_md_write_transaction().unwrap();
            restore(
                &mut service,
                &store,
                &source,
                force,
                id,
                CancellationToken::default(),
            )
            .unwrap();
            assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), expected);
            assert_eq!(
                store
                    .get(source_id)
                    .unwrap()
                    .unwrap()
                    .reverted_by
                    .as_deref(),
                Some(id)
            );
            source_id = id;
        }
        assert_eq!(
            fs::read(store.app_data.join("backups/forced-undo/0-SKILL.md")).unwrap(),
            local_edit
        );
        let source = store.get(source_id).unwrap().unwrap();
        let cancelled = CancellationToken::default();
        cancelled.cancel();
        assert!(restore(
            &mut service,
            &store,
            &source,
            false,
            "pre-cancel",
            cancelled
        )
        .is_err());
        assert!(store.get("pre-cancel").unwrap().is_none());
        assert!(store.get(source_id).unwrap().unwrap().reverted_by.is_none());
        let prepared = service
            .prepare_fork_document_restore(
                &source,
                &store,
                false,
                limits(),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        drop(
            prepared
                .record_intent(
                    &store,
                    "startup-undo",
                    limits(),
                    &CancellationToken::default(),
                )
                .unwrap(),
        );
        store
            .conn
            .execute(
                "UPDATE events SET status = 'interrupted' WHERE id = 'startup-undo'",
                [],
            )
            .unwrap();
        super::super::skill_startup_recovery::recover_all_with_worker(
            scope.clone(),
            &store,
            &|| panic!("Fork recovery launched a direct worker"),
        )
        .unwrap();
        super::super::skill_startup_recovery::recover_all_with_worker(
            scope.clone(),
            &store,
            &|| panic!("Fork recovery launched a direct worker"),
        )
        .unwrap();
        assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), original);
        assert_eq!(store.get("startup-undo").unwrap().unwrap().status, "done");
        assert_eq!(
            fs::read(agents.join("skill-studio.json")).unwrap(),
            registry
        );
        assert_eq!(fs::read(agents.join("agents.lock")).unwrap(), provider_lock);
        assert_eq!(
            fs::read(agents.join("agents.toml")).unwrap(),
            provider_manifest
        );
        assert_eq!(
            fs::read(store.app_data.join("backups/undo/0-SKILL.md")).unwrap(),
            repaired.as_bytes()
        );
        let fork = store.get("fork").unwrap().unwrap();
        assert!(fork.reverted_by.is_some());
        let projection = temp.path().join("merge-base-projection");
        fs::create_dir(&projection).unwrap();
        let output =
            skill_studio_core::skill_backup_reservation::BackupStateRoot::bind(&projection)
                .unwrap();
        let prepared = service
            .prepare_fork_merge_base(
                &fork,
                &store,
                limits(),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(prepared.event_id(), "fork");
        let base = prepared
            .publish(&store, limits(), &CancellationToken::default())
            .unwrap();
        assert_eq!(
            base,
            skill_studio_core::skill_fork_registry::fork_snapshot_dir(&store.app_data, "alpha")
        );
        assert_eq!(
            fs::read(base.join("SKILL.md")).unwrap(),
            b"upstream snapshot"
        );
        assert_eq!(
            prepared
                .publish(&store, limits(), &CancellationToken::default())
                .unwrap(),
            base
        );
        fs::write(base.join("SKILL.md"), b"different cached base").unwrap();
        assert!(prepared
            .publish(&store, limits(), &CancellationToken::default())
            .is_err());
        assert_eq!(
            fs::read(base.join("SKILL.md")).unwrap(),
            b"different cached base"
        );
        assert_eq!(
            fs::read(store.app_data.join("backups/fork/upstream-tree/SKILL.md")).unwrap(),
            b"upstream snapshot"
        );
        prepared
            .copy_to_staging(&store, &output, limits(), &CancellationToken::default())
            .unwrap();
        assert_eq!(
            fs::read(projection.join("base/SKILL.md")).unwrap(),
            b"upstream snapshot"
        );
        let stale_output = temp.path().join("stale-projection");
        fs::create_dir(&stale_output).unwrap();
        let stale_output =
            skill_studio_core::skill_backup_reservation::BackupStateRoot::bind(&stale_output)
                .unwrap();
        store
            .conn
            .execute(
                "UPDATE events SET reverted_by = 'changed-claim' WHERE id = 'fork'",
                [],
            )
            .unwrap();
        assert!(prepared
            .copy_to_staging(
                &store,
                &stale_output,
                limits(),
                &CancellationToken::default()
            )
            .is_err());
        assert!(!temp.path().join("stale-projection/base").exists());
        drop(prepared);
        store
            .conn
            .execute(
                "UPDATE events SET reverted_by = ?1 WHERE id = 'fork'",
                [fork.reverted_by.as_deref()],
            )
            .unwrap();
        let mut changed: serde_json::Value = serde_json::from_slice(&registry).unwrap();
        changed["forks"]["alpha"]["base_commit"] = serde_json::json!("b".repeat(40));
        fs::write(
            agents.join("skill-studio.json"),
            serde_json::to_vec(&changed).unwrap(),
        )
        .unwrap();
        assert!(service
            .prepare_fork_merge_base(
                &fork,
                &store,
                limits(),
                Some(Duration::from_secs(10)),
                CancellationToken::default()
            )
            .is_err());
        fs::write(agents.join("skill-studio.json"), &registry).unwrap();
        assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), original);
        assert_eq!(
            serde_json::to_value(store.get("fork").unwrap().unwrap()).unwrap(),
            serde_json::to_value(fork).unwrap()
        );
        fs::remove_dir_all(&base).unwrap();
        let source = store.get("fork").unwrap().unwrap();
        store
            .conn
            .execute("UPDATE events SET payload = '{}' WHERE id = 'fork'", [])
            .unwrap();
        assert!(ensure_merge_base(scope.clone(), &store, "alpha")
            .unwrap_err()
            .contains("Invalid fork history"));
        assert!(!base.exists());
        store
            .conn
            .execute(
                "UPDATE events SET payload = ?1 WHERE id = 'fork'",
                [serde_json::to_string(&source.payload).unwrap()],
            )
            .unwrap();
        store
            .conn
            .execute("UPDATE events SET status = 'failed' WHERE id = 'fork'", [])
            .unwrap();
        assert!(ensure_merge_base(scope.clone(), &store, "alpha")
            .unwrap_err()
            .contains("No completed fork event matches"));
        assert!(!base.exists());
        store
            .conn
            .execute("UPDATE events SET status = 'done' WHERE id = 'fork'", [])
            .unwrap();
        let mut duplicate = source.payload.clone();
        duplicate["snapshots"]["operation_id"] = serde_json::json!("duplicate-fork");
        store.conn.execute(
            "INSERT INTO events (id, ts, kind, skill, scope, payload, status, restorable) VALUES ('duplicate-fork', '2026-09-13', 'repair_dotagents_fork', 'alpha', 'global', ?1, 'done', 0)",
            [serde_json::to_string(&duplicate).unwrap()],
        ).unwrap();
        assert!(ensure_merge_base(scope.clone(), &store, "alpha")
            .unwrap_err()
            .contains("Multiple fork events match"));
        assert!(!base.exists());
        store
            .conn
            .execute("DELETE FROM events WHERE id = 'duplicate-fork'", [])
            .unwrap();
        store.conn.execute_batch(
            "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 1000) INSERT INTO events (id, ts, kind, skill, payload, status) SELECT 'limit-fixture-' || x, '2026-09-13', 'repair_dotagents_fork', 'alpha', '{}', 'done' FROM n;",
        ).unwrap();
        assert!(ensure_merge_base(scope.clone(), &store, "alpha")
            .unwrap_err()
            .contains("lookup limit"));
        assert!(!base.exists());
        store
            .conn
            .execute("DELETE FROM events WHERE id LIKE 'limit-fixture-%'", [])
            .unwrap();
        assert_eq!(
            fs::read(agents.join("skill-studio.json")).unwrap(),
            registry
        );
        assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), original);
        assert_eq!(
            serde_json::to_value(store.get("fork").unwrap().unwrap()).unwrap(),
            serde_json::to_value(source).unwrap()
        );
        ensure_merge_base(scope.clone(), &store, "alpha").unwrap();
        assert_eq!(
            fs::read(base.join("SKILL.md")).unwrap(),
            b"upstream snapshot"
        );
        let mine = if fail_completion {
            b"local diverged\n".as_slice()
        } else {
            b"upstream snapshot".as_slice()
        };
        fs::write(live.join("SKILL.md"), mine).unwrap();
        let result = pull_fork_upstream_durably(&mut service, &store, 'b', b"upstream changed\n");
        assert_eq!(result.to_commit, "b".repeat(40));
        assert_eq!(
            result.conflicts,
            if fail_completion {
                vec!["SKILL.md".to_string()]
            } else {
                vec![]
            }
        );
        if fail_completion {
            assert!(fs::read_to_string(live.join("SKILL.md"))
                .unwrap()
                .contains("<<<<<<<"));
        } else {
            assert_eq!(
                fs::read(live.join("SKILL.md")).unwrap(),
                b"upstream changed\n"
            );
            ensure_merge_base(scope.clone(), &store, "alpha").unwrap();
            let result =
                pull_fork_upstream_durably(&mut service, &store, 'c', b"second upstream\n");
            assert!(result.conflicts.is_empty());
            assert_eq!(result.from_commit, "b".repeat(40));
            assert_eq!(result.to_commit, "c".repeat(40));
            assert_eq!(
                fs::read(live.join("SKILL.md")).unwrap(),
                b"second upstream\n"
            );
        }
        let advanced_registry = fs::read(agents.join("skill-studio.json")).unwrap();
        let advanced_live = fs::read(live.join("SKILL.md")).unwrap();
        if !fail_completion {
            let source = store.get("startup-undo").unwrap().unwrap();
            assert!(restore(
                &mut service,
                &store,
                &source,
                false,
                "post-pull-refused",
                CancellationToken::default(),
            )
            .is_err());
            assert!(store.get("post-pull-refused").unwrap().is_none());
            assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), advanced_live);

            let _transaction =
                super::super::skill_md_write::begin_skill_md_write_transaction().unwrap();
            let prepared = service
                .prepare_fork_document_restore(
                    &source,
                    &store,
                    true,
                    limits(),
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let restored = prepared.restore_content().to_vec();
            drop(
                prepared
                    .record_intent(
                        &store,
                        "post-pull-restore",
                        limits(),
                        &CancellationToken::default(),
                    )
                    .unwrap(),
            );
            let intent = store.get("post-pull-restore").unwrap().unwrap();
            let claimed_source = store.get("startup-undo").unwrap().unwrap();
            assert_eq!(
                intent.payload["registry_admission"]["selected"]["base_commit"],
                serde_json::json!("c".repeat(40)),
            );
            let mut legacy_payload = intent.payload.clone();
            legacy_payload
                .as_object_mut()
                .unwrap()
                .remove("registry_admission");
            let legacy: skill_studio_core::skill_fork_document_restore::ForkDocumentRestoreIntent =
                serde_json::from_value(legacy_payload.clone()).unwrap();
            assert_eq!(serde_json::to_value(legacy).unwrap(), legacy_payload);

            let mut changed_registry: serde_json::Value =
                serde_json::from_slice(&advanced_registry).unwrap();
            changed_registry["forks"]["alpha"]["base_commit"] = serde_json::json!("d".repeat(40));
            fs::write(
                agents.join("skill-studio.json"),
                serde_json::to_vec(&changed_registry).unwrap(),
            )
            .unwrap();
            let error = service
                .prepare_fork_document_restore_recovery(
                    &claimed_source,
                    &intent,
                    &store,
                    limits(),
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .err()
                .expect("changed registry must refuse recovery")
                .to_string();
            assert!(error.contains("restore admission"), "{error}");
            fs::write(agents.join("skill-studio.json"), &advanced_registry).unwrap();
            store
                .conn
                .execute(
                    "UPDATE events SET payload = ?1 WHERE id = 'post-pull-restore'",
                    [serde_json::to_string(&legacy_payload).unwrap()],
                )
                .unwrap();
            let legacy = store.get("post-pull-restore").unwrap().unwrap();
            let error = service
                .prepare_fork_document_restore_recovery(
                    &claimed_source,
                    &legacy,
                    &store,
                    limits(),
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .err()
                .expect("changed registry must refuse recovery")
                .to_string();
            assert!(error.contains("since the repair"), "{error}");
            store
                .conn
                .execute(
                    "UPDATE events SET payload = ?1, status = 'interrupted' WHERE id = 'post-pull-restore'",
                    [serde_json::to_string(&intent.payload).unwrap()],
                )
                .unwrap();
            drop(_transaction);
            super::super::skill_startup_recovery::recover_all_with_worker(
                scope.clone(),
                &store,
                &|| panic!("Fork restore recovery launched a direct worker"),
            )
            .unwrap();
            assert_eq!(
                store.get("post-pull-restore").unwrap().unwrap().status,
                "done"
            );
            assert_eq!(restored, local_edit);
            assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), local_edit);
            let _transaction =
                super::super::skill_md_write::begin_skill_md_write_transaction().unwrap();
            restore(
                &mut service,
                &store,
                &store.get("post-pull-restore").unwrap().unwrap(),
                false,
                "post-pull-redo",
                CancellationToken::default(),
            )
            .unwrap();
            assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), advanced_live);
            assert_eq!(
                fs::read(agents.join("skill-studio.json")).unwrap(),
                advanced_registry
            );
            assert_eq!(fs::read(agents.join("agents.lock")).unwrap(), provider_lock);
            assert_eq!(
                fs::read(agents.join("agents.toml")).unwrap(),
                provider_manifest
            );
        }
        fs::remove_dir_all(&base).unwrap();
        assert!(ensure_merge_base(scope.clone(), &store, "alpha")
            .unwrap_err()
            .contains("No completed fork event matches"));
        assert!(!base.exists());
        assert_eq!(
            fs::read(agents.join("skill-studio.json")).unwrap(),
            advanced_registry
        );
        assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), advanced_live);
        assert_eq!(
            fs::read(store.app_data.join("backups/fork/upstream-tree/SKILL.md")).unwrap(),
            b"upstream snapshot"
        );
        let sibling = home.join("project/.agents/skills/alpha");
        fs::create_dir_all(&sibling).unwrap();
        fs::write(sibling.join("SKILL.md"), b"project sibling").unwrap();
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("SKILL.md"), b"advanced base cache").unwrap();
        let current = skill_studio_core::skill_fork_registry::read_fork_registry(&home).unwrap();
        let record = current.forks.get("alpha").unwrap();
        let ledger = FixtureReinstall {
            expected: record,
            calls: std::cell::Cell::new(0),
            fail: true,
        };
        let history = serde_json::to_value(store.get("fork").unwrap().unwrap()).unwrap();
        let error =
            super::super::skill_fork::unfork_skill_with(&home, &store.app_data, "alpha", &ledger)
                .unwrap_err();
        assert!(error.contains("fixture provider refused"));
        assert_eq!(ledger.calls.get(), 1);
        assert_eq!(
            fs::read(agents.join("skill-studio.json")).unwrap(),
            advanced_registry
        );
        assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), advanced_live);
        assert_eq!(
            fs::read(base.join("SKILL.md")).unwrap(),
            b"advanced base cache"
        );
        let ledger = FixtureReinstall {
            fail: false,
            ..ledger
        };
        super::super::skill_fork::unfork_skill_with(&home, &store.app_data, "alpha", &ledger)
            .unwrap();
        assert_eq!(ledger.calls.get(), 2);
        assert!(
            !skill_studio_core::skill_fork_registry::read_fork_registry(&home)
                .unwrap()
                .forks
                .contains_key("alpha")
        );
        assert!(!base.exists());
        assert_eq!(
            fs::read(live.join("SKILL.md")).unwrap(),
            b"provider restored document\n"
        );
        assert_eq!(
            fs::read(sibling.join("SKILL.md")).unwrap(),
            b"project sibling"
        );
        assert_eq!(
            serde_json::to_value(store.get("fork").unwrap().unwrap()).unwrap(),
            history
        );
        assert_eq!(
            fs::read(store.app_data.join("backups/fork/upstream-tree/SKILL.md")).unwrap(),
            b"upstream snapshot"
        );
        assert_eq!(
            fs::read(store.app_data.join("backups/undo/0-SKILL.md")).unwrap(),
            repaired.as_bytes()
        );
        assert!(ensure_merge_base(scope.clone(), &store, "alpha")
            .unwrap_err()
            .contains("is not forked"));
        fs::write(agents.join("skill-studio.json"), &advanced_registry).unwrap();
        fs::write(live.join("SKILL.md"), &advanced_live).unwrap();
        fs::write(agents.join("agents.lock"), &provider_lock).unwrap();
        fs::write(agents.join("agents.toml"), &provider_manifest).unwrap();
        verify_unfork_startup(scope, &store, fail_completion, provider);
    }
    fn verify_unfork_startup(
        scope: SkillScope,
        store: &EventStore,
        stop_after_verification: bool,
        provider: Option<&provider_fixture::ProviderFixture>,
    ) {
        use skill_studio_core::{
            skill_backup_reservation::BackupStateRoot,
            skill_unfork_preparation::{DotagentsRuntimeRecord, DotagentsUnforkRequest},
        };
        let mut service = ScopedSkillService::bind(scope.clone()).unwrap();
        let inventory = service
            .scan(
                Some(&std::collections::BTreeSet::from(["alpha".into()])),
                None,
            )
            .unwrap();
        let deployment = inventory
            .skills
            .iter()
            .flat_map(|skill| &skill.deployments)
            .find(|deployment| {
                deployment.owner_kind
                    == skill_studio_core::skill_ownership::LifecycleOwnerKind::Fork
            })
            .unwrap();
        let live = std::path::Path::new(&deployment.path).to_path_buf();
        let request = DotagentsUnforkRequest {
            deployment_id: deployment.id.clone(),
            expected_owner_revision: deployment.owner_revision.clone().unwrap(),
            expected_document_fingerprint:
                skill_studio_core::skill_frontmatter_repair::content_fingerprint(
                    &fs::read(live.join("SKILL.md")).unwrap(),
                ),
        };
        let token = CancellationToken::default();
        let prepared = service
            .prepare_current_dotagents_unfork(
                &request,
                store,
                limits(),
                Some(Duration::from_secs(30)),
                token.clone(),
            )
            .unwrap();
        let runtime = provider
            .map(|fixture| fixture.record.clone())
            .unwrap_or_else(|| DotagentsRuntimeRecord {
                provider_version: "3.0.1".into(),
                provider_tree_identity: format!("tree-v1:{}", "a".repeat(64)),
                node_version: "v26.8.2".into(),
                node_content_digest: format!("sha256:{}", "b".repeat(64)),
                copy_contract: "dotagents-3.0.1-default-node-copy".into(),
            });
        let unstarted = prepared
            .record_pending(
                store,
                "desktop-unstarted-unfork",
                runtime.clone(),
                limits(),
                &token,
            )
            .unwrap();
        let cancelled = CancellationToken::default();
        cancelled.cancel();
        assert!(prepared
            .resolve_unstarted(store, &unstarted, limits(), &cancelled)
            .is_err());
        let before = fs::read(live.join("SKILL.md")).unwrap();
        drop(prepared);
        fs::write(live.join("SKILL.md"), b"edit after unstarted intent").unwrap();
        assert!(
            super::super::skill_startup_recovery::recover_all_with_worker(
                scope.clone(),
                store,
                &|| panic!("Changed unstarted intent launched provider")
            )
            .is_err()
        );
        assert_eq!(
            store
                .get("desktop-unstarted-unfork")
                .unwrap()
                .unwrap()
                .status,
            "pending"
        );
        assert_eq!(
            fs::read(live.join("SKILL.md")).unwrap(),
            b"edit after unstarted intent"
        );
        fs::write(live.join("SKILL.md"), &before).unwrap();
        super::super::skill_startup_recovery::recover_all_with_worker(
            scope.clone(),
            store,
            &|| panic!("Unstarted recovery launched provider"),
        )
        .unwrap();
        assert_eq!(
            store
                .get("desktop-unstarted-unfork")
                .unwrap()
                .unwrap()
                .status,
            "failed"
        );
        assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), before);
        assert!(store
            .app_data
            .join("backups/desktop-unstarted-unfork")
            .exists());
        let prepared = service
            .prepare_current_dotagents_unfork(
                &request,
                store,
                limits(),
                Some(Duration::from_secs(30)),
                token.clone(),
            )
            .unwrap();
        let pending = prepared
            .record_pending(store, "desktop-unfork", runtime, limits(), &token)
            .unwrap();
        let marked = prepared
            .mark_provider_may_have_started(store, &pending, limits(), &token)
            .unwrap();
        drop(prepared);
        assert!(
            super::super::skill_startup_recovery::recover_all_with_worker(
                scope.clone(),
                store,
                &|| panic!("Uncertain provider was relaunched")
            )
            .is_err()
        );
        assert_eq!(
            store.get("desktop-unfork").unwrap().unwrap().payload,
            marked.event().payload
        );
        assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), before);
        let root = BackupStateRoot::bind(&store.app_data).unwrap();
        let reservation = root
            .open_managed_source_reservation("desktop-unfork")
            .unwrap();
        let document = b"---\nname: alpha\ndescription: restored upstream\n---\nbody\n";
        if let Some(provider) = provider {
            let prepared = service
                .prepare_dotagents_unfork_resume(
                    &marked,
                    store,
                    limits(),
                    Some(Duration::from_secs(30)),
                    token.clone(),
                )
                .unwrap();
            let original = prepared.reinstall_request().clone();
            drop(prepared);
            provider.stage(&reservation, &original, document);
            assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), before);
        } else {
            let cache_source = reservation
                .cache_path()
                .unwrap()
                .join("owner/repo/skills/alpha");
            let agents = reservation.stage_path().unwrap().join("home/.agents");
            let staged_tree = agents.join("skills/alpha");
            fs::create_dir_all(&cache_source).unwrap();
            fs::create_dir_all(&staged_tree).unwrap();
            for path in [&cache_source, &staged_tree] {
                fs::write(path.join("SKILL.md"), document).unwrap();
                fs::write(path.join("resource.txt"), b"retained cache resource").unwrap();
            }
            std::os::unix::fs::symlink("resource.txt", cache_source.join("link.txt")).unwrap();
            std::os::unix::fs::symlink(
                cache_source.join("resource.txt"),
                staged_tree.join("link.txt"),
            )
            .unwrap();
            fs::write(agents.join("agents.lock"), "version = 1\n[skills.alpha]\nsource = 'owner/repo'\nresolved_path = 'skills/alpha'\nresolved_commit = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'\n").unwrap();
            fs::write(
                agents.join("agents.toml"),
                "version = 1\n[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\n",
            )
            .unwrap();
        }
        let prepared = service
            .prepare_dotagents_unfork_resume(
                &marked,
                store,
                limits(),
                Some(Duration::from_secs(30)),
                token.clone(),
            )
            .unwrap();
        let reference = reservation.seal_cache(limits(), &token).unwrap();
        let sealed = root
            .open_managed_source(&reference, limits(), &token)
            .unwrap();
        let staged = sealed
            .record_dotagents_stage_v2(
                prepared.reinstall_request(),
                std::path::Path::new("home/.agents"),
                limits(),
                &token,
            )
            .unwrap();
        let verified = prepared
            .record_verified_source(store, &marked, &staged, limits(), &token)
            .unwrap();
        if !stop_after_verification {
            prepared
                .begin_publication(store, &verified, limits(), &token)
                .unwrap();
        }
        drop(prepared);
        store
            .conn
            .execute(
                "UPDATE events SET status = 'interrupted' WHERE id = 'desktop-unfork'",
                [],
            )
            .unwrap();
        assert!(!is_fork_event("unfork_dotagents"));
        super::super::skill_startup_recovery::recover_all_with_worker(
            scope.clone(),
            store,
            &|| panic!("Unfork recovery launched provider worker"),
        )
        .unwrap();
        assert_eq!(store.get("desktop-unfork").unwrap().unwrap().status, "done");
        assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), document);
        assert_eq!(
            fs::read(live.join("link.txt")).unwrap(),
            b"retained cache resource"
        );
        assert!(
            !skill_studio_core::skill_fork_registry::read_fork_registry(&scope.home)
                .unwrap()
                .forks
                .contains_key("alpha")
        );
        super::super::skill_startup_recovery::recover_all_with_worker(scope, store, &|| {
            panic!("empty recovery launched worker")
        })
        .unwrap();
    }
}
