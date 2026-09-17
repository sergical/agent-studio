//! Blocking database exchange for the owning supervisor thread.
//! The caller supplies a trusted private worker command and a complete write lease.
use crate::{
    skill_coordination::{CancellationToken, FinalizedWriteLease},
    skill_event_worker_cleanup::{run_and_reap, wait_for_exit_controlled, ReapedOperation},
    skill_event_worker_dispatch::{send_frame_controlled, DispatchState},
    skill_event_worker_protocol::{EventReply, PreparedEventExchange},
    skill_history_state::HistoryStateRoot,
    skill_history_worker_bootstrap::send_history_directory,
    skill_history_worker_frame::{finish_history_frames, read_json_frame},
    skill_history_worker_process::HistoryWorkerProcess,
    skill_worker_socket::WorkerSocket,
};
use std::{
    io::{Read, Write},
    path::Path,
    process::Command,
    time::Instant,
};

pub struct EventExchangeStartFailure<'scope> {
    pub lease: FinalizedWriteLease<'scope>,
    pub reason: String,
}

/// Retains the consumed lease until confirmed child exit, including on operation
/// panic. The caller retains the returned lease for receipt recovery/file work.
/// This must run on a dedicated supervisor thread; cleanup can outlive a deadline.
pub fn run_event_exchange<'scope>(
    root: &Path,
    lease: FinalizedWriteLease<'scope>,
    mut command: Command,
    request: &PreparedEventExchange,
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<ReapedOperation<'scope, serde_json::Value>, Box<EventExchangeStartFailure<'scope>>> {
    let prepare = (|| {
        lease.validate_state_tree(root)?;
        if cancellation.is_cancelled() || Instant::now() >= deadline {
            return Err("Event exchange stopped before worker launch".into());
        }
        HistoryStateRoot::bind(root).map_err(|error| error.to_string())
    })();
    let state = match prepare {
        Ok(state) => state,
        Err(reason) => return Err(Box::new(EventExchangeStartFailure { lease, reason })),
    };
    let transfer = match state.prepare_directory_transfer() {
        Ok(transfer) => transfer,
        Err(error) => {
            return Err(Box::new(EventExchangeStartFailure {
                lease,
                reason: error.to_string(),
            }))
        }
    };
    let mut child = match HistoryWorkerProcess::spawn(&mut command) {
        Ok(child) => child,
        Err(error) => {
            return Err(Box::new(EventExchangeStartFailure {
                lease,
                reason: error.to_string(),
            }))
        }
    };
    let mut dispatch = DispatchState::NotSent;
    Ok(run_and_reap(&mut child, lease, |child, lease| {
        let result = (|| -> Result<serde_json::Value, String> {
            lease.validate_state_tree(root)?;
            send_history_directory(child.socket(), &transfer).map_err(|error| error.to_string())?;
            let mut socket =
                WorkerSocket::new(child.socket(), || cancellation.is_cancelled(), deadline)
                    .map_err(|error| error.to_string())?;
            let mut ready = [0];
            socket
                .read_exact(&mut ready)
                .map_err(|error| error.to_string())?;
            if ready != *b"R" {
                return Err("Invalid event worker ready signal".into());
            }
            lease.validate_state_tree(root)?;
            transfer.revalidate().map_err(|error| error.to_string())?;
            socket.write_all(&[1]).map_err(|error| error.to_string())?;
            send_frame_controlled(
                &mut socket,
                &mut dispatch,
                1024 * 1024,
                request.request(),
                cancellation,
            )
            .map_err(|error| format!("Event request send failed: {error:?}"))?;
            child
                .socket()
                .shutdown(std::net::Shutdown::Write)
                .map_err(|error| error.to_string())?;
            let reply: EventReply = read_json_frame(&mut socket, 64 * 1024)
                .map_err(|error| format!("Event reply failed: {error:?}"))?;
            finish_history_frames(&mut socket)
                .map_err(|error| format!("Event reply EOF failed: {error:?}"))?;
            let exit = wait_for_exit_controlled(child, cancellation, deadline)
                .map_err(|error| format!("Event worker exit failed: {error:?}"))?;
            if !exit.success() {
                return Err("Event worker exited unsuccessfully".into());
            }
            reply.validate(Some(request))
        })();
        result.unwrap_or_else(|_| match dispatch {
            DispatchState::NotSent => {
                serde_json::json!({"outcome":"not_started","code":"worker_unavailable"})
            }
            DispatchState::MayHaveSent => request.unavailable_reply(),
        })
    }))
}

#[test]
fn start_failure_returns_the_held_lease_without_opening_a_database() {
    use crate::{
        skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect},
        skill_scope::SkillReadScope,
    };
    for cancelled in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
        let lease = CoordinationPlan::new(
            vec![DirectoryEffect::tree(&root, CoordinationMode::Exclusive)],
            None,
        )
        .unwrap()
        .acquire()
        .unwrap()
        .finalize_write(&scope, &[])
        .unwrap();
        let request = crate::skill_event_worker_protocol::prepare_event_request(serde_json::from_value(serde_json::json!({
            "version":1,"action":"apply","exchange_id":"exchange","command_id":"command","event_id":"event",
            "operation":{"kind":"finish_pending","status":"done","inverse":null}
        })).unwrap()).unwrap();
        let token = CancellationToken::default();
        if cancelled {
            token.cancel();
        }
        let result = run_event_exchange(
            &root,
            lease,
            Command::new(root.join("absent-worker")),
            &request,
            &token,
            Instant::now() + std::time::Duration::from_secs(1),
        );
        let failure = match result {
            Err(failure) => failure,
            Ok(_) => panic!("unexpected worker launch"),
        };
        failure.lease.validate_state_tree(&root).unwrap();
        assert!(!root.join("events.sqlite3").exists());
    }
}
