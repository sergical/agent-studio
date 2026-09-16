use super::{
    event_store::EventStore,
    skill_native_unfork::{
        configure_repository_mirror, fetch_repository_mirror, prepare_stage, NativeUnforkTarget,
    },
    skill_process::{run_controlled_prepared_command_output, AddOperationControl},
    skill_unfork_provider::StagedUnforkProvider,
};
use skill_studio_core::{
    skill_backup_reservation::{BackupCopyLimits, BackupStateRoot, SkillsShReinstallRequest},
    skill_fork_registry::ForkRecord,
    skill_frontmatter_repair::content_fingerprint,
    skill_service::{CancellationToken, ScopedSkillService, SkillScope},
    skill_unfork_preparation::{
        PendingSkillsShUnforkEvent, SkillsShUnforkProviderState, SkillsShUnforkRequest,
    },
};
use std::{fs, path::Path, process::Command, time::Duration};

const LIMITS: BackupCopyLimits = BackupCopyLimits {
    max_bytes: 256 * 1024 * 1024,
    max_entries: 20_000,
    max_depth: 64,
};

fn git(
    arguments: &[&std::ffi::OsStr],
    directory: &Path,
    control: &AddOperationControl,
) -> Result<Vec<u8>, String> {
    let mut command = Command::new("/usr/bin/git");
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", directory)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(directory)
        .args(arguments);
    run_controlled_prepared_command_output(command, control, 64 * 1024)
        .map_err(|error| error.into_message())
}

pub(super) fn apply(
    target: NativeUnforkTarget<'_>,
    record: &ForkRecord,
    provider: StagedUnforkProvider,
) -> Result<(), String> {
    apply_with_fetch(
        target,
        record,
        provider,
        AddOperationControl::bounded_default(),
        &fetch_repository_mirror,
    )
}

fn apply_with_fetch(
    target: NativeUnforkTarget<'_>,
    record: &ForkRecord,
    provider: StagedUnforkProvider,
    control: AddOperationControl,
    fetch: &dyn Fn(&str, &Path, &AddOperationControl) -> Result<(), String>,
) -> Result<(), String> {
    let name = target
        .live
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Unfork skill name is unavailable")?;
    let policy = SkillsShReinstallRequest::from_fork_record(record, name, &"0".repeat(40))?;
    control.check_message()?;
    let live_root = skill_studio_core::skill_backup_source::BackupSourceRoot::bind(
        target
            .live
            .parent()
            .ok_or("Unfork live parent is missing")?,
    )
    .map_err(|error| error.to_string())?;
    let live_source = live_root
        .select(
            target
                .live
                .file_name()
                .ok_or("Unfork live name is missing")?,
        )
        .map_err(|error| error.to_string())?;
    let initial_tree = live_source
        .inspect(LIMITS, &control.cancellation_token())
        .map_err(|error| error.to_string())?
        .tree_identity;
    let initial_document = content_fingerprint(
        &fs::read(target.live.join("SKILL.md")).map_err(|error| error.to_string())?,
    );
    let fetched = tempfile::Builder::new()
        .prefix("skill-studio-unfork-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let mirror = fetched.path().join("upstream.git");
    fetch(policy.repo(), &mirror, &control)?;
    let revision = format!("{}^{{commit}}", policy.declared_ref().unwrap_or("HEAD"));
    let commit = git(
        &[
            "--git-dir".as_ref(),
            mirror.as_os_str(),
            "rev-parse".as_ref(),
            "--verify".as_ref(),
            "--end-of-options".as_ref(),
            revision.as_ref(),
        ],
        fetched.path(),
        &control,
    )?;
    let commit = std::str::from_utf8(&commit)
        .map_err(|_| "Fetched commit is not UTF-8")?
        .trim();
    let request = SkillsShUnforkRequest {
        deployment_id: target.deployment_id.into(),
        expected_owner_revision: target.owner_revision.into(),
        expected_document_fingerprint: initial_document,
        resolved_commit: commit.into(),
    };
    let scope = SkillScope {
        home: target.home.into(),
        projects: vec![],
        backing_roots: vec![],
        plugin_ownership_roots: vec![],
    };
    let mut service = ScopedSkillService::bind(scope.clone()).map_err(|error| error.to_string())?;
    let store = EventStore::open(target.app_data)?;
    let token = control.cancellation_token();
    let prepared = service
        .prepare_current_skills_sh_unfork(
            &request,
            &store,
            LIMITS,
            Some(Duration::from_secs(30)),
            token.clone(),
        )
        .map_err(|error| error.to_string())?;
    if prepared.live_identity() != initial_tree {
        return Err("Unfork live tree changed during source fetch".into());
    }
    let reinstall = prepared.reinstall_request().clone();
    if reinstall.repo() != policy.repo()
        || reinstall.path() != policy.path()
        || reinstall.declared_ref() != policy.declared_ref()
    {
        return Err("Unfork source policy changed during fetch".into());
    }
    let operation = format!("desktop-unfork-{}", ulid::Ulid::new());
    let pending = prepared
        .record_skills_sh_pending(
            &store,
            &operation,
            provider.skills_sh_record(),
            LIMITS,
            &token,
        )
        .map_err(|error| error.to_string())?;
    drop(prepared);
    let result = (|| {
        let root = BackupStateRoot::bind(target.app_data).map_err(|error| error.to_string())?;
        let reservation = root
            .open_managed_source_reservation(&operation)
            .map_err(|error| error.to_string())?;
        prepare_stage(&reservation)?;
        let cache = reservation
            .cache_path()
            .map_err(|error| error.to_string())?;
        let retained_mirror = cache.join("upstream.git");
        git(
            &[
                "clone".as_ref(),
                "--mirror".as_ref(),
                "--no-hardlinks".as_ref(),
                mirror.as_os_str(),
                retained_mirror.as_os_str(),
            ],
            &cache,
            &control,
        )?;
        let checkout = cache.join(reinstall.repo());
        fs::create_dir_all(checkout.parent().ok_or("Missing source parent")?)
            .map_err(|error| error.to_string())?;
        git(
            &[
                "clone".as_ref(),
                "--no-hardlinks".as_ref(),
                "--no-checkout".as_ref(),
                retained_mirror.as_os_str(),
                checkout.as_os_str(),
            ],
            &cache,
            &control,
        )?;
        git(
            &[
                "checkout".as_ref(),
                "--detach".as_ref(),
                reinstall.resolved_commit().as_ref(),
            ],
            &checkout,
            &control,
        )?;
        configure_repository_mirror(&reservation, reinstall.repo(), &retained_mirror)?;
        reservation
            .admit_skills_sh_source(&reinstall, LIMITS, &token)
            .map_err(|error| error.to_string())?;
        let started = service
            .prepare_skills_sh_unfork_resume(
                &pending,
                &store,
                LIMITS,
                Some(Duration::from_secs(30)),
                token.clone(),
            )
            .map_err(|error| error.to_string())?
            .mark_skills_sh_provider_may_have_started(&store, &pending, LIMITS, &token)
            .map_err(|error| error.to_string())?;
        provider.run_skills_sh_staged_add(&reservation, &reinstall, &control)?;
        let resumed = service
            .prepare_skills_sh_unfork_resume(
                &started,
                &store,
                LIMITS,
                Some(Duration::from_secs(30)),
                token.clone(),
            )
            .map_err(|error| error.to_string())?;
        let reference = reservation
            .seal_cache(LIMITS, &token)
            .map_err(|error| error.to_string())?;
        let sealed = root
            .open_managed_source(&reference, LIMITS, &token)
            .map_err(|error| error.to_string())?;
        let staged = sealed
            .record_skills_sh_stage(&reinstall, Path::new("home/.agents"), LIMITS, &token)
            .map_err(|error| error.to_string())?;
        let verified = resumed
            .record_skills_sh_verified_source(&store, &started, &staged, LIMITS, &token)
            .map_err(|error| error.to_string())?;
        let publishing = resumed
            .begin_skills_sh_publication(&store, &verified, LIMITS, &token)
            .map_err(|error| error.to_string())?;
        drop(resumed);
        service.resume_skills_sh_unfork_publication(
            &publishing,
            &store,
            LIMITS,
            Some(Duration::from_secs(30)),
            token,
        )
    })();
    if let Err(error) = result {
        let error = control.check_message().err().unwrap_or(error);
        let cleanup = (|| {
            let current = store.get(&operation)?.ok_or("Unfork event disappeared")?;
            let event = PendingSkillsShUnforkEvent::from_row(&current)?;
            if !matches!(
                event.intent().provider_state(),
                SkillsShUnforkProviderState::NotStarted
                    | SkillsShUnforkProviderState::MayHaveStarted
            ) {
                return Err("Verified Unfork requires publication recovery".into());
            }
            let mut service = ScopedSkillService::bind(scope).map_err(|error| error.to_string())?;
            let token = CancellationToken::default();
            service
                .prepare_skills_sh_unfork_resume(
                    &event,
                    &store,
                    LIMITS,
                    Some(Duration::from_secs(30)),
                    token.clone(),
                )
                .map_err(|error| error.to_string())?
                .resolve_skills_sh_unapplied(&store, &event, LIMITS, &token)
                .map_err(|error| error.to_string())
        })();
        return Err(match cleanup {
            Ok(()) => error,
            Err(cleanup) => format!("{error}; Unfork recovery remains unresolved: {cleanup}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use skill_studio_core::{
        skill_deployment::{deployment_id, SkillDestination},
        skill_fork_registry::OriginTool,
    };

    #[test]
    #[ignore = "requires explicit pinned Node and combined provider runtime"]
    fn native_skills_sh_unfork_restores_and_recovers_with_real_provider() {
        let runtime = std::path::PathBuf::from(
            std::env::var_os("SKILL_STUDIO_RUNTIME_FIXTURE").expect("runtime fixture"),
        );
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        enum Case {
            InterruptedCompletion,
            Legacy,
            InvalidRuntime,
            LiveEdit,
            CancelBeforeFetch,
            CancelAfterMarker,
            FetchDeadline,
            MissingRef,
        }
        for case in [
            Case::InterruptedCompletion,
            Case::Legacy,
            Case::InvalidRuntime,
            Case::LiveEdit,
            Case::CancelBeforeFetch,
            Case::CancelAfterMarker,
            Case::FetchDeadline,
            Case::MissingRef,
        ] {
            let legacy = case == Case::Legacy;
            let tampered_runtime = case == Case::InvalidRuntime;
            let edit_during_fetch = case == Case::LiveEdit;
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let agents = home.join(".agents");
            let live = agents.join("skills/alpha");
            let repository = temp.path().join("repository");
            fs::create_dir_all(repository.join("skills/alpha")).unwrap();
            fs::create_dir_all(&live).unwrap();
            let upstream = b"---\nname: alpha\ndescription: restored upstream\n---\nupstream\n";
            fs::write(repository.join("skills/alpha/SKILL.md"), upstream).unwrap();
            let control = AddOperationControl::bounded_default();
            for arguments in [
                vec!["init", "-b", "main"],
                vec!["add", "."],
                vec![
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "-m",
                    "fixture",
                ],
            ] {
                let arguments: Vec<&std::ffi::OsStr> =
                    arguments.iter().map(AsRef::as_ref).collect();
                git(&arguments, &repository, &control).unwrap();
            }
            fs::write(live.join("SKILL.md"), b"local edits").unwrap();
            fs::write(
                agents.join(".skill-lock.json"),
                b"{\"version\":3,\"future\":true,\"skills\":{}}",
            )
            .unwrap();
            let id = deployment_id(
                "alpha",
                "global",
                SkillDestination::Universal,
                "universal",
                None,
                &live,
            );
            let mut record = ForkRecord {
                deployment_id: id.clone(),
                skill_dir: live.clone(),
                forked_at: "2026-09-16T00:00:00Z".into(),
                origin_tool: OriginTool::SkillsSh,
                origin_source: "owner/repo".into(),
                repo: "owner/repo".into(),
                path: "skills/alpha/SKILL.md".into(),
                declared_ref: if case == Case::MissingRef {
                    Some("absent-ref".into())
                } else if legacy {
                    Some("main".into())
                } else {
                    None
                },
                base_commit: "a".repeat(40),
            };
            let mut raw = serde_json::to_value(&record).unwrap();
            if legacy {
                raw.as_object_mut().unwrap().remove("deployment_id");
                raw.as_object_mut().unwrap().remove("skill_dir");
                record.deployment_id.clear();
                record.skill_dir = std::path::PathBuf::new();
            }
            fs::write(
                agents.join("skill-studio.json"),
                serde_json::to_vec(
                    &serde_json::json!({"forks":{"alpha":raw},"trials":{},"future":"keep"}),
                )
                .unwrap(),
            )
            .unwrap();
            let scope = SkillScope {
                home: home.clone(),
                projects: vec![],
                backing_roots: vec![],
                plugin_ownership_roots: vec![],
            };
            let inventory = ScopedSkillService::bind(scope.clone())
                .unwrap()
                .scan(None, None)
                .unwrap();
            let revision = inventory
                .skills
                .iter()
                .flat_map(|skill| &skill.deployments)
                .find(|deployment| deployment.id == id)
                .unwrap()
                .owner_revision
                .clone()
                .unwrap();
            let app_data = temp.path().join("state");
            let store = EventStore::open(&app_data).unwrap();
            if case == Case::InterruptedCompletion {
                store.conn.execute_batch("CREATE TRIGGER interrupt_unfork BEFORE UPDATE OF status ON events WHEN NEW.status = 'done' BEGIN SELECT RAISE(ABORT, 'injected interruption'); END;").unwrap();
            }
            let mut runtime_record: skill_studio_core::skill_unfork_preparation::DotagentsRuntimeRecord = serde_json::from_slice(&fs::read(runtime.join("verified-record.json")).unwrap()).unwrap();
            if tampered_runtime {
                runtime_record.provider_tree_identity = format!("tree-v1:{}", "0".repeat(64));
            }
            let provider =
                StagedUnforkProvider::bind(&runtime, &runtime.join("bin/node"), runtime_record)
                    .unwrap();
            let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                case == Case::CancelBeforeFetch,
            ));
            let watcher = if case == Case::CancelAfterMarker {
                let database = app_data.join("events.sqlite3");
                let cancel = std::sync::Arc::clone(&cancelled);
                Some(std::thread::spawn(move || {
                    let connection = rusqlite::Connection::open(database).unwrap();
                    let deadline = std::time::Instant::now() + Duration::from_secs(15);
                    while std::time::Instant::now() < deadline {
                        let payload = connection.query_row(
                            "SELECT payload FROM events WHERE kind='unfork_skills_sh' AND status='pending'",
                            [], |row| row.get::<_, String>(0),
                        ).ok();
                        if payload
                            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                            .is_some_and(|value| value["provider"] == "may_have_started")
                        {
                            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                            return true;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    false
                }))
            } else {
                None
            };
            let operation_control = AddOperationControl::new(
                std::sync::Arc::clone(&cancelled),
                if case == Case::FetchDeadline {
                    Duration::from_millis(250)
                } else {
                    Duration::from_secs(30)
                },
            );
            let result = apply_with_fetch(
                NativeUnforkTarget {
                    home: &home,
                    app_data: &app_data,
                    deployment_id: &id,
                    owner_revision: &revision,
                    live: &live,
                },
                &record,
                provider,
                operation_control,
                &|repo, mirror, control| {
                    assert_ne!(case, Case::CancelBeforeFetch);
                    assert_eq!(repo, "owner/repo");
                    if case == Case::FetchDeadline {
                        std::thread::sleep(Duration::from_millis(300));
                        control.check_message()?;
                    }
                    git(
                        &[
                            "clone".as_ref(),
                            "--mirror".as_ref(),
                            "--no-hardlinks".as_ref(),
                            repository.as_os_str(),
                            mirror.as_os_str(),
                        ],
                        temp.path(),
                        control,
                    )
                    .map(|_| ())?;
                    if edit_during_fetch {
                        fs::write(live.join("new-resource"), "external resource")
                            .map_err(|error| error.to_string())?;
                    }
                    Ok(())
                },
            );
            if let Some(watcher) = watcher {
                assert!(watcher.join().unwrap(), "provider marker was not observed");
            }
            if matches!(
                case,
                Case::CancelBeforeFetch
                    | Case::CancelAfterMarker
                    | Case::FetchDeadline
                    | Case::MissingRef
            ) {
                let error = result.unwrap_err();
                if matches!(case, Case::CancelBeforeFetch | Case::CancelAfterMarker) {
                    assert!(error.contains("cancel"), "{case:?}: {error}");
                }
                if case == Case::FetchDeadline {
                    assert!(
                        error.contains("timed out") || error.contains("cancel"),
                        "{error}"
                    );
                    assert!(!cancelled.load(std::sync::atomic::Ordering::SeqCst));
                }
                assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), b"local edits");
                let registry: serde_json::Value =
                    serde_json::from_slice(&fs::read(agents.join("skill-studio.json")).unwrap())
                        .unwrap();
                assert!(registry["forks"].get("alpha").is_some());
                assert_eq!(
                    fs::read(agents.join(".skill-lock.json")).unwrap(),
                    b"{\"version\":3,\"future\":true,\"skills\":{}}"
                );
                let count: i64 = store
                    .conn
                    .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(
                    count,
                    i64::from(case == Case::CancelAfterMarker),
                    "{case:?}"
                );
                if case == Case::CancelAfterMarker {
                    let status: String = store
                        .conn
                        .query_row("SELECT status FROM events", [], |row| row.get(0))
                        .unwrap();
                    assert_eq!(status, "failed");
                }
                continue;
            }
            if edit_during_fetch {
                assert!(result
                    .unwrap_err()
                    .contains("live tree changed during source fetch"));
                assert_eq!(
                    fs::read(live.join("new-resource")).unwrap(),
                    b"external resource"
                );
                assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), b"local edits");
                let count: i64 = store
                    .conn
                    .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(count, 0);
                continue;
            }
            if tampered_runtime {
                assert!(result
                    .unwrap_err()
                    .contains("Provider runtime bytes or metadata changed"));
                assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), b"local edits");
                let status: String = store
                    .conn
                    .query_row(
                        "SELECT status FROM events WHERE kind='unfork_skills_sh'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(status, "failed");
                let registry: serde_json::Value =
                    serde_json::from_slice(&fs::read(agents.join("skill-studio.json")).unwrap())
                        .unwrap();
                assert!(registry["forks"].get("alpha").is_some());
                continue;
            }
            if legacy {
                result.unwrap();
            } else {
                assert!(result.unwrap_err().contains("injected interruption"));
                store
                    .conn
                    .execute_batch("DROP TRIGGER interrupt_unfork;")
                    .unwrap();
                assert_eq!(
                    super::super::skill_startup_recovery::recover_all(scope.clone(), &store)
                        .unwrap(),
                    1
                );
                assert_eq!(
                    super::super::skill_startup_recovery::recover_all(scope, &store).unwrap(),
                    0
                );
            }
            assert_eq!(fs::read(live.join("SKILL.md")).unwrap(), upstream);
            let lock: serde_json::Value =
                serde_json::from_slice(&fs::read(agents.join(".skill-lock.json")).unwrap())
                    .unwrap();
            assert_eq!(lock["future"], true);
            assert_eq!(lock["skills"]["alpha"]["source"], "owner/repo");
            assert_eq!(
                lock["skills"]["alpha"]
                    .get("ref")
                    .and_then(serde_json::Value::as_str),
                record.declared_ref.as_deref()
            );
            let registry: serde_json::Value =
                serde_json::from_slice(&fs::read(agents.join("skill-studio.json")).unwrap())
                    .unwrap();
            assert!(registry["forks"].get("alpha").is_none());
            assert_eq!(registry["future"], "keep");
            let count: i64 = store
                .conn
                .query_row(
                    "SELECT count(*) FROM events WHERE kind='unfork_skills_sh' AND status='done'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1);
        }
    }
}
