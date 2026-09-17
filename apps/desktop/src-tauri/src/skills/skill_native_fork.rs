use super::event_store::EventStore;
use skill_studio_core::{
    skill_backup_reservation::{BackupCopyLimits, BackupStateRoot},
    skill_deployment::{parse_deployment_id, SkillDestination},
    skill_dotagents_ledger::DotagentsDetachIntent,
    skill_frontmatter_repair::BoundFrontmatterRepairRequest,
    skill_inventory::Deployment,
    skill_ownership::LifecycleOwnerKind,
    skill_scope::SkillReadScope,
    skill_service::{CancellationToken, ScopedSkillService},
    skill_upstream_archive::ArchiveLimits,
    skill_upstream_fetch::fetch_fork_source,
};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(crate) fn supports(deployment: &Deployment, home: &Path) -> bool {
    let Some(id) = parse_deployment_id(&deployment.id) else {
        return false;
    };
    deployment.owner_kind == LifecycleOwnerKind::Dotagents
        && id.scope == "global"
        && id.destination == SkillDestination::Universal
        && Path::new(&deployment.path) == home.join(".agents/skills").join(&id.name)
        && home.join(".agents/agents.lock").is_file()
        && home.join(".agents/agents.toml").is_file()
}

pub(crate) fn apply(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &BoundFrontmatterRepairRequest,
    gh: &Path,
    id: &str,
    cancellation: CancellationToken,
) -> Result<(), String> {
    super::skill_document_operation::check_document_cancellation(&cancellation)?;
    let target = parse_deployment_id(&request.deployment_id).ok_or("Invalid fork deployment ID")?;
    if target.scope != "global" || target.destination != SkillDestination::Universal {
        return Err("Native dotagents fork requires a Global Universal deployment".into());
    }
    let agents = service.scope().home.join(".agents");
    let live = agents.join("skills").join(&target.name);
    let read =
        SkillReadScope::bind(std::slice::from_ref(&agents)).map_err(|error| error.to_string())?;
    let lock = read
        .read(&agents.join("agents.lock"), 8 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    let manifest = read
        .read(&agents.join("agents.toml"), 8 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    let source = DotagentsDetachIntent::from_documents(
        &target.name,
        std::str::from_utf8(&lock).map_err(|error| error.to_string())?,
        std::str::from_utf8(&manifest).map_err(|error| error.to_string())?,
    )?
    .fork_source()?;
    let staging = tempfile::Builder::new()
        .prefix("skill-studio-fork-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let upstream = fetch_fork_source(
        &source,
        gh,
        staging.path(),
        ArchiveLimits::default(),
        deadline,
        &cancellation,
    )?;
    let limits = BackupCopyLimits {
        max_bytes: 256 * 1024 * 1024,
        max_entries: 20_000,
        max_depth: 64,
    };
    let result = (|| {
        let prepared = service
            .prepare_dotagents_fork_selection(
                request,
                &[store.app_data.clone(), live, upstream.path().to_path_buf()],
                Some(deadline.saturating_duration_since(Instant::now())),
                cancellation.clone(),
            )
            .map_err(|error| error.to_string())?;
        let state = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
        let snapshots = prepared.publish_snapshots(&state, id, upstream, limits, &cancellation)?;
        let mut pending = match prepared.begin_fork(
            store,
            snapshots.reference(),
            chrono::Utc::now(),
            limits,
            &cancellation,
        ) {
            Ok(pending) => pending,
            Err(error) => {
                let failure = error.failure().to_string();
                if matches!(
                    error.failure(),
                    skill_studio_core::skill_event_operations::EventWriteFailure::BeforeWrite(_)
                ) {
                    error.verify_operation_absent(store).map_err(|cleanup| {
                        format!("{failure}; fork snapshot cleanup refused: {cleanup}")
                    })?;
                    snapshots.discard().map_err(|cleanup| {
                        format!("{failure}; fork snapshot cleanup refused: {cleanup}")
                    })?;
                }
                return Err(failure);
            }
        };
        pending
            .publish_detach_documents(store, limits, &cancellation)
            .map_err(|error| error.to_string())?;
        pending
            .publish_fork_registry(store, limits, &cancellation)
            .map_err(|error| error.to_string())?;
        pending
            .publish_repair(store, limits, &cancellation)
            .map_err(|error| error.to_string())?;
        pending
            .complete(store, limits, &cancellation)
            .map_err(|error| error.to_string())
    })();
    let Err(error) = result else {
        return Ok(());
    };
    let Some(row) = store.get(id)? else {
        return Err(error);
    };
    if matches!(row.status.as_str(), "pending" | "interrupted") {
        // Durable fork intent can span provider files; finish it from fresh evidence.
        let cleanup = CancellationToken::default();
        service
            .prepare_native_fork_recovery(
                &row,
                store,
                limits,
                Some(Duration::from_secs(30)),
                cleanup.clone(),
            )
            .map_err(|recovery| format!("{error}; fork recovery remains unresolved: {recovery}"))?
            .resume(store, limits, &cleanup)
            .map_err(|recovery| format!("{error}; fork recovery remains unresolved: {recovery}"))?;
    }
    if store.get(id)?.is_some_and(|row| row.status == "done") {
        Ok(())
    } else {
        Err(error)
    }
}
