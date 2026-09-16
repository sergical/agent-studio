use super::{
    event_store::EventStore,
    skill_process::{run_controlled_prepared_command_output, AddOperationControl},
    skill_unfork_provider::StagedUnforkProvider,
};
use skill_studio_core::{
    skill_backup_reservation::{BackupCopyLimits, BackupStateRoot},
    skill_frontmatter_repair::content_fingerprint,
    skill_service::{CancellationToken, ScopedSkillService, SkillScope},
    skill_unfork_preparation::{DotagentsRuntimeRecord, DotagentsUnforkRequest},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const LIMITS: BackupCopyLimits = BackupCopyLimits {
    max_bytes: 256 * 1024 * 1024,
    max_entries: 20_000,
    max_depth: 64,
};

pub(super) struct NativeUnforkTarget<'a> {
    pub(super) home: &'a Path,
    pub(super) app_data: &'a Path,
    pub(super) deployment_id: &'a str,
    pub(super) owner_revision: &'a str,
    pub(super) live: &'a Path,
}

fn runtime_from_root(root: &Path) -> Result<StagedUnforkProvider, String> {
    let record: DotagentsRuntimeRecord = serde_json::from_slice(
        &fs::read(root.join("verified-record.json"))
            .map_err(|_| "Packaged Unfork runtime record is missing")?,
    )
    .map_err(|_| "Packaged Unfork runtime record is invalid")?;
    let provider = StagedUnforkProvider::bind(root, &root.join("bin/node"), record)
        .map_err(|error| format!("Packaged Unfork runtime is invalid: {error}"))?;
    provider
        .verify_materialized_runtime(&AddOperationControl::bounded_default())
        .map_err(|error| format!("Packaged Unfork runtime is invalid: {error}"))?;
    Ok(provider)
}

pub(crate) fn load_packaged_runtime(resource_root: &Path) -> Result<StagedUnforkProvider, String> {
    runtime_from_root(&resource_root.join("unfork-runtime"))
}

pub(super) fn prepare_stage(
    reservation: &skill_studio_core::skill_backup_reservation::ReservedManagedSource<'_>,
) -> Result<(), String> {
    let stage = reservation.stage_path().map_err(|e| e.to_string())?;
    for name in [
        "home/.agents",
        "tmp",
        "config",
        "data",
        "xdg-state",
        "xdg-cache",
    ] {
        fs::create_dir_all(stage.join(name)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn fetch_git_mirror(
    request: &skill_studio_core::skill_dotagents_ledger::DotagentsReinstallRequest,
    reservation: &skill_studio_core::skill_backup_reservation::ReservedManagedSource<'_>,
    control: &AddOperationControl,
) -> Result<PathBuf, String> {
    let cache = reservation.cache_path().map_err(|e| e.to_string())?;
    let mirror = cache.join("upstream.git");
    fetch_repository_mirror(request.repo(), &mirror, control)?;
    Ok(mirror)
}

pub(super) fn fetch_repository_mirror(
    repo: &str,
    mirror: &Path,
    control: &AddOperationControl,
) -> Result<(), String> {
    let gh = super::skill_update_check::resolve_gh_binary()
        .ok_or("Unfork source fetch requires the configured GitHub CLI")?;
    let mut command = Command::new(gh);
    command
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["repo", "clone", repo])
        .arg(mirror)
        .args(["--", "--mirror"]);
    run_controlled_prepared_command_output(command, control, 64 * 1024)
        .map_err(|error| format!("Unfork source fetch failed: {}", error.into_message()))?;
    Ok(())
}

pub(super) fn configure_repository_mirror(
    reservation: &skill_studio_core::skill_backup_reservation::ReservedManagedSource<'_>,
    repo: &str,
    mirror: &Path,
) -> Result<(), String> {
    let stage = reservation.stage_path().map_err(|e| e.to_string())?;
    let url = reqwest::Url::from_file_path(mirror).map_err(|_| "Unfork mirror URL is invalid")?;
    let aliases = [
        format!("https://github.com/{repo}"),
        format!("https://github.com/{repo}.git"),
        format!("http://github.com/{repo}"),
        format!("http://github.com/{repo}.git"),
        format!("git@github.com:{repo}"),
        format!("git@github.com:{repo}.git"),
        format!("ssh://git@github.com/{repo}"),
        format!("ssh://git@github.com/{repo}.git"),
        format!("ssh://github.com/{repo}"),
        format!("ssh://github.com/{repo}.git"),
    ];
    let aliases = aliases
        .into_iter()
        .map(|alias| format!("\tinsteadOf = {alias}\n"))
        .collect::<String>();
    fs::write(
        stage.join("config/gitconfig"),
        format!("[url \"{url}\"]\n{aliases}[protocol \"file\"]\n\tallow = always\n",),
    )
    .map_err(|e| e.to_string())
}

pub(crate) fn recover_pending(
    scope: SkillScope,
    store: &EventStore,
    row: &super::event_store::EventRow,
) -> Result<(), String> {
    use skill_studio_core::skill_unfork_preparation::UnforkProviderState;

    if row.kind == "unfork_skills_sh" {
        return recover_skills_sh_pending(scope, store, row);
    }

    let event =
        skill_studio_core::skill_unfork_preparation::PendingDotagentsUnforkEvent::from_row(row)?;
    match event.intent().provider_state() {
        UnforkProviderState::NotStarted => resolve_unstarted(scope, store, &event),
        UnforkProviderState::MayHaveStarted => abandon(scope, store, &event),
        UnforkProviderState::SourceVerified { .. } => {
            let token = CancellationToken::default();
            let mut service = ScopedSkillService::bind(scope).map_err(|error| error.to_string())?;
            let prepared = service
                .prepare_dotagents_unfork_resume(
                    &event,
                    store,
                    LIMITS,
                    Some(Duration::from_secs(30)),
                    token.clone(),
                )
                .map_err(|error| error.to_string())?;
            let publishing = prepared
                .begin_publication(store, &event, LIMITS, &token)
                .map_err(|error| error.to_string())?;
            drop(prepared);
            service
                .resume_unfork_publication(
                    &publishing,
                    store,
                    LIMITS,
                    Some(Duration::from_secs(30)),
                    token,
                )
                .map_err(|error| error.to_string())
        }
        UnforkProviderState::Publishing { .. } => ScopedSkillService::bind(scope)
            .map_err(|error| error.to_string())?
            .resume_unfork_publication(
                &event,
                store,
                LIMITS,
                Some(Duration::from_secs(30)),
                CancellationToken::default(),
            )
            .map_err(|error| error.to_string()),
    }
}

fn abandon(
    scope: SkillScope,
    store: &EventStore,
    event: &skill_studio_core::skill_unfork_preparation::PendingDotagentsUnforkEvent,
) -> Result<(), String> {
    let token = CancellationToken::default();
    let mut service = ScopedSkillService::bind(scope).map_err(|e| e.to_string())?;
    service
        .prepare_dotagents_unfork_resume(
            event,
            store,
            LIMITS,
            Some(Duration::from_secs(30)),
            token.clone(),
        )
        .map_err(|e| e.to_string())?
        .abandon_may_have_started(store, event, LIMITS, &token)
        .map_err(|e| e.to_string())
}

fn recover_skills_sh_pending(
    scope: SkillScope,
    store: &EventStore,
    row: &super::event_store::EventRow,
) -> Result<(), String> {
    use skill_studio_core::skill_unfork_preparation::{
        PendingSkillsShUnforkEvent, SkillsShUnforkProviderState,
    };
    let event = PendingSkillsShUnforkEvent::from_row(row)?;
    let token = CancellationToken::default();
    let mut service = ScopedSkillService::bind(scope).map_err(|error| error.to_string())?;
    match event.intent().provider_state() {
        SkillsShUnforkProviderState::NotStarted | SkillsShUnforkProviderState::MayHaveStarted => {
            service
                .prepare_skills_sh_unfork_resume(
                    &event,
                    store,
                    LIMITS,
                    Some(Duration::from_secs(30)),
                    token.clone(),
                )
                .map_err(|error| error.to_string())?
                .resolve_skills_sh_unapplied(store, &event, LIMITS, &token)
                .map_err(|error| error.to_string())
        }
        SkillsShUnforkProviderState::SourceVerified { .. } => {
            let prepared = service
                .prepare_skills_sh_unfork_resume(
                    &event,
                    store,
                    LIMITS,
                    Some(Duration::from_secs(30)),
                    token.clone(),
                )
                .map_err(|error| error.to_string())?;
            let publishing = prepared
                .begin_skills_sh_publication(store, &event, LIMITS, &token)
                .map_err(|error| error.to_string())?;
            drop(prepared);
            service.resume_skills_sh_unfork_publication(
                &publishing,
                store,
                LIMITS,
                Some(Duration::from_secs(30)),
                token,
            )
        }
        SkillsShUnforkProviderState::Publishing { .. } => service
            .resume_skills_sh_unfork_publication(
                &event,
                store,
                LIMITS,
                Some(Duration::from_secs(30)),
                token,
            ),
    }
}

fn resolve_unstarted(
    scope: SkillScope,
    store: &EventStore,
    event: &skill_studio_core::skill_unfork_preparation::PendingDotagentsUnforkEvent,
) -> Result<(), String> {
    let token = CancellationToken::default();
    let mut service = ScopedSkillService::bind(scope).map_err(|e| e.to_string())?;
    service
        .prepare_dotagents_unfork_resume(
            event,
            store,
            LIMITS,
            Some(Duration::from_secs(30)),
            token.clone(),
        )
        .map_err(|e| e.to_string())?
        .resolve_unstarted(store, event, LIMITS, &token)
        .map_err(|e| e.to_string())
}

fn resolve_unstarted_after(
    scope: SkillScope,
    store: &EventStore,
    event: &skill_studio_core::skill_unfork_preparation::PendingDotagentsUnforkEvent,
    error: String,
) -> String {
    match resolve_unstarted(scope, store, event) {
        Ok(()) => error,
        Err(cleanup) => format!("{error}; Unfork cleanup remains unresolved: {cleanup}"),
    }
}

pub(crate) fn apply(
    home: &Path,
    app_data: &Path,
    deployment_id: String,
    owner_revision: String,
    live: &Path,
    provider: StagedUnforkProvider,
) -> Result<(), String> {
    apply_with_source_fetch(
        NativeUnforkTarget {
            home,
            app_data,
            deployment_id: &deployment_id,
            owner_revision: &owner_revision,
            live,
        },
        provider,
        AddOperationControl::bounded_default(),
        &fetch_git_mirror,
    )
}

fn apply_with_source_fetch(
    target: NativeUnforkTarget<'_>,
    provider: StagedUnforkProvider,
    control: AddOperationControl,
    fetch_source: &dyn Fn(
        &skill_studio_core::skill_dotagents_ledger::DotagentsReinstallRequest,
        &skill_studio_core::skill_backup_reservation::ReservedManagedSource<'_>,
        &AddOperationControl,
    ) -> Result<PathBuf, String>,
) -> Result<(), String> {
    let token = control.cancellation_token();
    let scope = SkillScope {
        home: target.home.to_path_buf(),
        projects: vec![],
        backing_roots: vec![],
        plugin_ownership_roots: vec![],
    };
    let store = EventStore::open(target.app_data)?;
    let request = DotagentsUnforkRequest {
        deployment_id: target.deployment_id.to_owned(),
        expected_owner_revision: target.owner_revision.to_owned(),
        expected_document_fingerprint: content_fingerprint(
            &fs::read(target.live.join("SKILL.md")).map_err(|e| e.to_string())?,
        ),
    };
    let operation_id = format!("desktop-unfork-{}", ulid::Ulid::new());
    let mut service = ScopedSkillService::bind(scope.clone()).map_err(|e| e.to_string())?;
    let prepared = service
        .prepare_current_dotagents_unfork(
            &request,
            &store,
            LIMITS,
            Some(Duration::from_secs(30)),
            token.clone(),
        )
        .map_err(|e| e.to_string())?;
    let pending = prepared
        .record_native_pending(
            &store,
            &operation_id,
            provider.record().clone(),
            LIMITS,
            &token,
        )
        .map_err(|e| e.to_string())?;
    drop(prepared);
    let root = BackupStateRoot::bind(target.app_data).map_err(|e| e.to_string())?;
    let reservation = root
        .open_managed_source_reservation(&operation_id)
        .map_err(|e| e.to_string())?;
    let prelaunch = (|| {
        control.check_message()?;
        prepare_stage(&reservation)?;
        let resumed = service
            .prepare_dotagents_unfork_resume(
                &pending,
                &store,
                LIMITS,
                Some(Duration::from_secs(30)),
                token.clone(),
            )
            .map_err(|e| e.to_string())?;
        let reinstall = resumed.reinstall_request().clone();
        drop(resumed);
        let mirror = fetch_source(&reinstall, &reservation, &control)?;
        configure_repository_mirror(&reservation, reinstall.repo(), &mirror)?;
        Ok::<_, String>(reinstall)
    })();
    let reinstall = match prelaunch {
        Ok(reinstall) => reinstall,
        Err(error) => return Err(resolve_unstarted_after(scope, &store, &pending, error)),
    };
    if let Err(error) = control.check_message() {
        return Err(resolve_unstarted_after(scope, &store, &pending, error));
    }
    let started = service
        .prepare_dotagents_unfork_resume(
            &pending,
            &store,
            LIMITS,
            Some(Duration::from_secs(30)),
            token.clone(),
        )
        .map_err(|e| e.to_string())
        .and_then(|prepared| {
            prepared
                .mark_provider_may_have_started(&store, &pending, LIMITS, &token)
                .map_err(|e| e.to_string())
        });
    let started = match started {
        Ok(started) => started,
        Err(error) => return Err(resolve_unstarted_after(scope, &store, &pending, error)),
    };
    let result = (|| {
        provider.run_staged_add(&reservation, &reinstall, &control)?;
        let resumed = service
            .prepare_dotagents_unfork_resume(
                &started,
                &store,
                LIMITS,
                Some(Duration::from_secs(30)),
                token.clone(),
            )
            .map_err(|e| e.to_string())?;
        let reference = reservation
            .seal_cache(LIMITS, &token)
            .map_err(|e| e.to_string())?;
        let sealed = root
            .open_managed_source(&reference, LIMITS, &token)
            .map_err(|e| e.to_string())?;
        let staged = sealed
            .record_dotagents_stage_v2(
                resumed.reinstall_request(),
                Path::new("home/.agents"),
                LIMITS,
                &token,
            )
            .map_err(|e| e.to_string())?;
        let verified = resumed
            .record_verified_source(&store, &started, &staged, LIMITS, &token)
            .map_err(|e| e.to_string())?;
        let publishing = resumed
            .begin_publication(&store, &verified, LIMITS, &token)
            .map_err(|e| e.to_string())?;
        drop(resumed);
        service
            .resume_unfork_publication(
                &publishing,
                &store,
                LIMITS,
                Some(Duration::from_secs(30)),
                token,
            )
            .map_err(|e| e.to_string())
    })();
    if let Err(error) = result {
        let error = control.check_message().err().unwrap_or(error);
        let current = store.get(&operation_id).ok().flatten().and_then(|row| {
            skill_studio_core::skill_unfork_preparation::PendingDotagentsUnforkEvent::from_row(&row)
                .ok()
        });
        if !current.as_ref().is_some_and(|event| {
            matches!(
                event.intent().provider_state(),
                skill_studio_core::skill_unfork_preparation::UnforkProviderState::MayHaveStarted
            )
        }) {
            return Err(error);
        }
        return match abandon(scope, &store, current.as_ref().unwrap()) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!(
                "{error}; Unfork cleanup remains unresolved: {cleanup}"
            )),
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        apply_with_source_fetch, fetch_git_mirror, load_packaged_runtime, NativeUnforkTarget,
    };
    use crate::skills::{event_store::EventStore, skill_process::AddOperationControl};
    use skill_studio_core::{
        skill_backup_reservation::BackupStateRoot,
        skill_deployment::{deployment_id, SkillDestination},
        skill_fork_registry::{
            deployment_trial_key, trial_key, AddMethod, ForkRecord, OriginTool, TrialRecord,
            TrialScope, TrialStatus,
        },
        skill_service::{ScopedSkillService, SkillScope},
        skill_unfork_preparation::{DotagentsRuntimeRecord, UnforkProviderState},
    };
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };

    struct ExecutorFixture {
        _temp: tempfile::TempDir,
        home: PathBuf,
        app_data: PathBuf,
        live: PathBuf,
        deployment_id: String,
        owner_revision: String,
        repository: PathBuf,
        registry_before: Vec<u8>,
        live_before: Vec<u8>,
    }

    impl ExecutorFixture {
        fn new(name: &str, legacy: bool, include_source: bool) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let app_data = temp.path().join("data");
            let agents = home.join(".agents");
            let live = agents.join("skills").join(name);
            fs::create_dir_all(&live).unwrap();
            fs::write(
                live.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: edited local fork\n---\nedited current\n"),
            )
            .unwrap();
            fs::write(live.join("local-edit.txt"), b"keep in before snapshot").unwrap();
            let sibling = agents.join("skills/sibling");
            fs::create_dir_all(&sibling).unwrap();
            fs::write(
                sibling.join("SKILL.md"),
                b"---\nname: sibling\ndescription: untouched\n---\nsibling\n",
            )
            .unwrap();
            fs::write(
                agents.join("agents.lock"),
                format!(
                    "version = 1\n[skills.sibling]\nsource = 'owner/other'\nresolved_path = 'skills/sibling'\nresolved_commit = '{}'\n",
                    "c".repeat(40)
                ),
            )
            .unwrap();
            fs::write(
                agents.join("agents.toml"),
                "version = 1\n[[skills]]\nname = 'sibling'\nsource = 'owner/other'\n",
            )
            .unwrap();

            let canonical_id = deployment_id(
                name,
                "global",
                SkillDestination::Universal,
                "universal",
                None,
                &live,
            );
            let record = ForkRecord {
                deployment_id: canonical_id.clone(),
                skill_dir: live.clone(),
                forked_at: "2026-09-14T00:00:00Z".into(),
                origin_tool: OriginTool::Dotagents,
                origin_source: "owner/repo".into(),
                repo: "owner/repo".into(),
                path: format!("skills/{name}"),
                declared_ref: Some("main".into()),
                base_commit: "a".repeat(40),
            };
            let sibling_record = ForkRecord {
                deployment_id: deployment_id(
                    "sibling",
                    "global",
                    SkillDestination::Universal,
                    "universal",
                    None,
                    &sibling,
                ),
                skill_dir: sibling.clone(),
                origin_source: "owner/other".into(),
                repo: "owner/other".into(),
                path: "skills/sibling".into(),
                declared_ref: None,
                ..record.clone()
            };
            let selected_trial = TrialRecord {
                deployment_id: canonical_id.clone(),
                started_at: "2026-09-14T00:00:00Z".into(),
                expires_at: "2026-09-15T00:00:00Z".into(),
                status: TrialStatus::Active,
                method: AddMethod::Dotagents,
                scope: TrialScope::Global,
                project_path: None,
                skill_dir: live.clone(),
                deployment_fingerprint: String::new(),
                claude_link: None,
                claude_link_target: None,
            };
            let unrelated_trial = TrialRecord {
                deployment_id: String::new(),
                method: AddMethod::Copy,
                scope: TrialScope::Project,
                project_path: Some("/unrelated".into()),
                skill_dir: PathBuf::from("/unrelated/.agents/skills/other"),
                ..selected_trial.clone()
            };
            let mut raw_record = serde_json::to_value(&record).unwrap();
            raw_record["future_owner_field"] = serde_json::json!({"keep": true});
            if legacy {
                raw_record.as_object_mut().unwrap().remove("deployment_id");
                raw_record.as_object_mut().unwrap().remove("skill_dir");
            }
            let registry = serde_json::json!({
                "version": 4,
                "forks": {
                    name: raw_record,
                    "sibling": serde_json::to_value(sibling_record).unwrap(),
                },
                "trials": {
                    trial_key(TrialScope::Global, name): serde_json::to_value(&selected_trial).unwrap(),
                    deployment_trial_key(&canonical_id): serde_json::to_value(&selected_trial).unwrap(),
                    format!("project/{name}"): serde_json::to_value(unrelated_trial).unwrap(),
                },
                "future_registry_field": {"keep": name},
            });
            fs::write(
                agents.join("skill-studio.json"),
                serde_json::to_vec_pretty(&registry).unwrap(),
            )
            .unwrap();

            let repository = temp.path().join("repository");
            fs::create_dir_all(&repository).unwrap();
            git(&repository, &["init", "--initial-branch=main"]);
            if include_source {
                let source = repository.join("skills").join(name);
                fs::create_dir_all(&source).unwrap();
                fs::write(
                    source.join("SKILL.md"),
                    format!("---\nname: {name}\ndescription: restored upstream\n---\nupstream\n"),
                )
                .unwrap();
                fs::write(source.join("upstream.txt"), b"provider content").unwrap();
            } else {
                fs::write(repository.join("README.md"), b"missing selected source").unwrap();
            }
            git(&repository, &["add", "."]);
            git(
                &repository,
                &[
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "commit",
                    "-m",
                    "fixture",
                ],
            );

            let scope = SkillScope {
                home: home.clone(),
                projects: vec![],
                backing_roots: vec![],
                plugin_ownership_roots: vec![],
            };
            let mut service = ScopedSkillService::bind(scope).unwrap();
            let inventory = service.scan(None, None).unwrap();
            let deployment = inventory
                .skills
                .iter()
                .flat_map(|skill| &skill.deployments)
                .find(|deployment| deployment.id == canonical_id)
                .unwrap();
            let owner_revision = deployment.owner_revision.clone().unwrap();
            let registry_before = fs::read(agents.join("skill-studio.json")).unwrap();
            let live_before = fs::read(live.join("SKILL.md")).unwrap();
            Self {
                _temp: temp,
                home,
                app_data,
                live,
                deployment_id: canonical_id,
                owner_revision,
                repository,
                registry_before,
                live_before,
            }
        }

        fn apply(
            &self,
            control: AddOperationControl,
            fetch: &dyn Fn(
                &skill_studio_core::skill_dotagents_ledger::DotagentsReinstallRequest,
                &skill_studio_core::skill_backup_reservation::ReservedManagedSource<'_>,
                &AddOperationControl,
            ) -> Result<PathBuf, String>,
        ) -> Result<(), String> {
            apply_with_source_fetch(
                NativeUnforkTarget {
                    home: &self.home,
                    app_data: &self.app_data,
                    deployment_id: &self.deployment_id,
                    owner_revision: &self.owner_revision,
                    live: &self.live,
                },
                provider_fixture(),
                control,
                fetch,
            )
        }

        fn assert_unchanged(&self) {
            assert_eq!(
                fs::read(self.live.join("SKILL.md")).unwrap(),
                self.live_before
            );
            assert_eq!(
                fs::read(self.home.join(".agents/skill-studio.json")).unwrap(),
                self.registry_before
            );
        }
    }

    fn git(directory: &Path, args: &[&str]) {
        let status = Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .current_dir(directory)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed with {status}");
    }

    fn provider_fixture() -> super::StagedUnforkProvider {
        let root = PathBuf::from(
            std::env::var_os("SKILL_STUDIO_RUNTIME_FIXTURE")
                .expect("explicit copied runtime fixture required"),
        )
        .canonicalize()
        .unwrap();
        let record =
            serde_json::from_slice(&fs::read(root.join("verified-record.json")).unwrap()).unwrap();
        super::StagedUnforkProvider::bind(&root, &root.join("bin/node"), record).unwrap()
    }

    fn mirror_fetch<'a>(
        repository: &'a Path,
    ) -> impl Fn(
        &skill_studio_core::skill_dotagents_ledger::DotagentsReinstallRequest,
        &skill_studio_core::skill_backup_reservation::ReservedManagedSource<'_>,
        &AddOperationControl,
    ) -> Result<PathBuf, String>
           + 'a {
        move |request, reservation, control| {
            assert_eq!(request.repo(), "owner/repo");
            let mirror = reservation
                .cache_path()
                .map_err(|error| error.to_string())?
                .join("upstream.git");
            let mut command = Command::new("/usr/bin/git");
            command
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .args(["clone", "--mirror"])
                .arg(repository)
                .arg(&mirror);
            super::run_controlled_prepared_command_output(command, control, 64 * 1024)
                .map_err(|error| error.into_message())?;
            Ok(mirror)
        }
    }

    fn unfork_rows(app_data: &Path) -> Vec<skill_studio_core::skill_event::EventRow> {
        let store = EventStore::open(app_data).unwrap();
        let mut statement = store
            .conn
            .prepare("SELECT id FROM events WHERE kind = 'unfork_dotagents' ORDER BY rowid")
            .unwrap();
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        ids.into_iter()
            .map(|id| store.get(&id).unwrap().unwrap())
            .collect()
    }

    #[test]
    fn operation_deadline_cancels_before_post_provider_sealing() {
        let temp = tempfile::tempdir().unwrap();
        let root = BackupStateRoot::bind(temp.path()).unwrap();
        let reservation = root.reserve_managed_source("expired-operation").unwrap();
        let cache = reservation.cache_path().unwrap();
        fs::write(cache.join("provider-output"), b"retained output").unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        let control = AddOperationControl::with_deadline(Arc::clone(&flag), Instant::now());
        let error = reservation
            .seal_cache(super::LIMITS, &control.cancellation_token())
            .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
        assert!(!cache.parent().unwrap().join("sealed-cache.json").exists());
        assert_eq!(
            fs::read(cache.join("provider-output")).unwrap(),
            b"retained output"
        );
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn packaged_runtime_loader_rejects_missing_and_invalid_records() {
        let temp = tempfile::tempdir().unwrap();
        assert!(load_packaged_runtime(temp.path()).is_err());
        let runtime = temp.path().join("unfork-runtime");
        fs::create_dir(&runtime).unwrap();
        fs::write(runtime.join("verified-record.json"), b"not json").unwrap();
        assert!(load_packaged_runtime(temp.path()).is_err());

        fs::create_dir_all(runtime.join("bin")).unwrap();
        fs::create_dir(runtime.join("node_modules")).unwrap();
        std::os::unix::fs::symlink("/bin/echo", runtime.join("bin/node")).unwrap();
        let record = DotagentsRuntimeRecord {
            provider_version: "3.0.1".into(),
            provider_tree_identity: format!("tree-v1:{}", "0".repeat(64)),
            node_version: "v24.19.0".into(),
            node_content_digest: format!("sha256:{}", "0".repeat(64)),
            copy_contract: "dotagents-3.0.1-default-node-copy".into(),
        };
        fs::write(
            runtime.join("verified-record.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            load_packaged_runtime(temp.path()),
            Err(error) if error.contains("runtime is invalid")
        ));
    }

    #[test]
    #[ignore = "requires GitHub access through the configured gh CLI"]
    fn desktop_native_unfork_fetches_github_source() {
        let temp = tempfile::tempdir().unwrap();
        let root = BackupStateRoot::bind(temp.path()).unwrap();
        let reservation = root.reserve_managed_source("github-source-fetch").unwrap();
        let request = serde_json::from_value(serde_json::json!({
            "name": "skillet-authoring",
            "source": "getsentry/skillet",
            "repo": "getsentry/skillet",
            "path": "skills/skillet-authoring",
            "declared_ref": null
        }))
        .unwrap();
        let mirror = fetch_git_mirror(
            &request,
            &reservation,
            &AddOperationControl::bounded_default(),
        )
        .unwrap();

        let status = Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .args(["--git-dir"])
            .arg(&mirror)
            .args(["cat-file", "-e", "HEAD:skills/skillet-authoring/SKILL.md"])
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    #[ignore = "requires the admitted provider runtime and macOS sandbox-exec"]
    fn desktop_native_unfork_executor_completes_modern_and_legacy_forks() {
        for legacy in [false, true] {
            let name = if legacy {
                "executor-legacy"
            } else {
                "executor-modern"
            };
            let fixture = ExecutorFixture::new(name, legacy, true);
            fixture
                .apply(
                    AddOperationControl::bounded_default(),
                    &mirror_fetch(&fixture.repository),
                )
                .unwrap();
            let rows = unfork_rows(&fixture.app_data);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].status, "done");
            assert_eq!(
                rows[0].payload["v2"]["execution_contract"],
                "macos-sandbox-exec-stage-cache-v1"
            );
            assert_eq!(
                fs::read_to_string(fixture.live.join("SKILL.md")).unwrap(),
                format!("---\nname: {name}\ndescription: restored upstream\n---\nupstream\n")
            );
            assert_eq!(
                fs::read(
                    fixture
                        .app_data
                        .join("backups")
                        .join(&rows[0].id)
                        .join("live-tree/local-edit.txt")
                )
                .unwrap(),
                b"keep in before snapshot"
            );
            let registry: serde_json::Value = serde_json::from_slice(
                &fs::read(fixture.home.join(".agents/skill-studio.json")).unwrap(),
            )
            .unwrap();
            assert!(registry["forks"].get(name).is_none());
            assert!(registry["forks"].get("sibling").is_some());
            assert!(registry["trials"]
                .get(trial_key(TrialScope::Global, name))
                .is_none());
            assert!(registry["trials"]
                .get(deployment_trial_key(&fixture.deployment_id))
                .is_none());
            assert_eq!(
                registry["trials"][format!("project/{name}")]["method"],
                "copy"
            );
            assert_eq!(registry["future_registry_field"]["keep"], name);
            assert_eq!(
                fs::read(fixture.home.join(".agents/skills/sibling/SKILL.md")).unwrap(),
                b"---\nname: sibling\ndescription: untouched\n---\nsibling\n"
            );
            let store = EventStore::open(&fixture.app_data).unwrap();
            assert_eq!(
                store
                    .conn
                    .query_row(
                        "SELECT count(*) FROM events WHERE kind = 'repair_dotagents_fork'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    #[ignore = "requires the admitted provider runtime and macOS sandbox-exec"]
    fn desktop_native_unfork_executor_records_prelaunch_and_provider_failures() {
        let prelaunch = ExecutorFixture::new("prelaunch-failure", false, true);
        let fetch_error = prelaunch
            .apply(AddOperationControl::bounded_default(), &|_, _, _| {
                Err("synthetic source fetch failure".into())
            })
            .unwrap_err();
        assert!(fetch_error.contains("synthetic source fetch failure"));
        prelaunch.assert_unchanged();
        let cancelled_before_preparation = prelaunch
            .apply(
                AddOperationControl::new(Arc::new(AtomicBool::new(true)), Duration::from_secs(10)),
                &|_, _, _| panic!("cancelled preparation must not fetch"),
            )
            .unwrap_err();
        assert!(cancelled_before_preparation.contains("cancelled"));
        assert_eq!(unfork_rows(&prelaunch.app_data).len(), 1);
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_during_fetch = Arc::clone(&cancel_flag);
        let prelaunch_cancel = prelaunch
            .apply(
                AddOperationControl::new(cancel_flag, Duration::from_secs(10)),
                &move |_, _, _| {
                    cancel_during_fetch.store(true, Ordering::SeqCst);
                    Err("synthetic prelaunch cancellation".into())
                },
            )
            .unwrap_err();
        assert!(prelaunch_cancel.contains("cancellation"));
        prelaunch.assert_unchanged();
        let prelaunch_rows = unfork_rows(&prelaunch.app_data);
        assert_eq!(prelaunch_rows.len(), 2);
        assert!(prelaunch_rows
            .iter()
            .all(|row| { row.status == "failed" && row.payload["provider"] == "not_started" }));

        let provider_failure = ExecutorFixture::new("provider-failure", false, false);
        let failed = provider_failure
            .apply(
                AddOperationControl::bounded_default(),
                &mirror_fetch(&provider_failure.repository),
            )
            .unwrap_err();
        assert!(!failed.is_empty());
        provider_failure.assert_unchanged();

        let valid_repository = provider_failure._temp.path().join("valid-repository");
        fs::create_dir_all(valid_repository.join("skills/provider-failure")).unwrap();
        fs::write(
            valid_repository.join("skills/provider-failure/SKILL.md"),
            "---\nname: provider-failure\ndescription: restored\n---\nrestored\n",
        )
        .unwrap();
        git(&valid_repository, &["init", "--initial-branch=main"]);
        git(&valid_repository, &["add", "."]);
        git(
            &valid_repository,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-m",
                "fixture",
            ],
        );

        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_after_marker = Arc::clone(&cancelled);
        let marker_app_data = provider_failure.app_data.clone();
        let marker_watcher = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let connection =
                rusqlite::Connection::open(marker_app_data.join("events.sqlite3")).unwrap();
            while Instant::now() < deadline {
                let marked = connection
                    .query_row(
                        "SELECT payload FROM events WHERE kind = 'unfork_dotagents' AND status = 'pending' ORDER BY rowid DESC LIMIT 1",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .ok()
                    .and_then(|payload| serde_json::from_str::<serde_json::Value>(&payload).ok())
                    .is_some_and(|payload| payload["provider"] == "may_have_started");
                if marked {
                    std::thread::sleep(Duration::from_millis(100));
                    cancel_after_marker.store(true, Ordering::SeqCst);
                    return true;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            false
        });
        let error = provider_failure
            .apply(
                AddOperationControl::new(Arc::clone(&cancelled), Duration::from_secs(10)),
                &mirror_fetch(&valid_repository),
            )
            .unwrap_err();
        assert!(marker_watcher.join().unwrap());
        assert!(error.contains("cancelled"), "{error}");
        let cancelled_row = unfork_rows(&provider_failure.app_data).pop().unwrap();
        assert_eq!(cancelled_row.status, "failed", "{error}: {cancelled_row:?}");
        provider_failure.assert_unchanged();

        let fetch = mirror_fetch(&valid_repository);
        let error = provider_failure
            .apply(
                AddOperationControl::with_deadline(
                    Arc::new(AtomicBool::new(false)),
                    Instant::now() + Duration::from_secs(2),
                ),
                &move |request, reservation, control| {
                    let mirror = fetch(request, reservation, control)?;
                    std::thread::sleep(Duration::from_millis(2_100));
                    Ok(mirror)
                },
            )
            .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        provider_failure.assert_unchanged();

        provider_failure
            .apply(
                AddOperationControl::bounded_default(),
                &mirror_fetch(&valid_repository),
            )
            .unwrap();
        let rows = unfork_rows(&provider_failure.app_data);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows.iter().filter(|row| row.status == "failed").count(), 3);
        assert_eq!(rows.iter().filter(|row| row.status == "done").count(), 1);
        let mut ids = rows.iter().map(|row| &row.id).collect::<Vec<_>>();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 4);
        assert_eq!(rows[0].payload["provider"], "may_have_started");
        assert_eq!(rows[1].payload["provider"], "may_have_started");
        assert_eq!(rows[2].payload["provider"], "not_started");
        for row in rows.iter().filter(|row| row.status == "failed") {
            let retained = provider_failure
                .app_data
                .join("skill-studio/managed-sources")
                .join(&row.id);
            assert!(retained.join("stage").is_dir());
            assert!(retained.join("cache").is_dir());
        }
    }

    #[test]
    #[ignore = "requires the admitted provider runtime and macOS sandbox-exec"]
    fn desktop_native_unfork_executor_recovers_interrupted_publication() {
        let fixture = ExecutorFixture::new("publication-recovery", false, true);
        let fetch = mirror_fetch(&fixture.repository);
        let app_data = fixture.app_data.clone();
        let error = fixture
            .apply(
                AddOperationControl::bounded_default(),
                &move |request, reservation, control| {
                    let mirror = fetch(request, reservation, control)?;
                    EventStore::open(&app_data)
                        .unwrap()
                        .conn
                        .execute_batch(
                            "CREATE TRIGGER reject_unfork_completion BEFORE UPDATE OF status ON events WHEN OLD.kind = 'unfork_dotagents' AND NEW.status = 'done' BEGIN SELECT RAISE(FAIL, 'injected completion interruption'); END;",
                        )
                        .unwrap();
                    Ok(mirror)
                },
            )
            .unwrap_err();
        assert!(error.contains("injected completion interruption"));
        let store = EventStore::open(&fixture.app_data).unwrap();
        let row = unfork_rows(&fixture.app_data).pop().unwrap();
        assert_eq!(row.status, "pending");
        let event =
            skill_studio_core::skill_unfork_preparation::PendingDotagentsUnforkEvent::from_row(
                &row,
            )
            .unwrap();
        assert!(matches!(
            event.intent().provider_state(),
            UnforkProviderState::Publishing { .. }
        ));
        store
            .conn
            .execute_batch("DROP TRIGGER reject_unfork_completion")
            .unwrap();
        let scope = SkillScope {
            home: fixture.home.clone(),
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        };
        assert_eq!(
            crate::skills::skill_startup_recovery::recover_all(scope.clone(), &store).unwrap(),
            1
        );
        assert_eq!(
            crate::skills::skill_startup_recovery::recover_all(scope, &store).unwrap(),
            0
        );
        assert_eq!(store.get(&row.id).unwrap().unwrap().status, "done");
    }
}
