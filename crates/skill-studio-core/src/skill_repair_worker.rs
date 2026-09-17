//! Repair uses private event workers while the parent retains file authority.
use crate::{
    skill_coordination::{CancellationToken, FinalizedWriteLease},
    skill_event::{EventDraft, EventStatus},
    skill_event_worker_exchange::run_event_exchange,
    skill_event_worker_protocol::PreparedEventExchange,
    skill_repair_execution::{
        execute_direct_repair_using, RepairEvents, RepairExecutionError, RepairExecutionReceipt,
    },
    skill_service::PreparedRepairSelection,
};
use sha2::{Digest, Sha256};
use std::{path::Path, process::Command, time::Instant};

type LeaseResult<'scope> = (FinalizedWriteLease<'scope>, Result<(), String>);

/// Trusted host configuration, never deserialized from an agent request.
/// Run on the owning operation thread. The state directory must already exist
/// and be included in the selection's lease. Public adapters remain gated on
/// complete restore, recovery, and first-use setup integration.
pub struct RepairEventWorker<'a> {
    pub state_root: &'a Path,
    pub command: &'a dyn Fn() -> Command,
    pub cancellation: &'a CancellationToken,
    pub deadline: Instant,
}

impl RepairEventWorker<'_> {
    pub fn recover(
        &self,
        prepared: crate::skill_service::PreparedRepairEventRecovery<'_>,
    ) -> Result<crate::skill_repair_execution::RepairRecoveryOutcome, RepairExecutionError> {
        crate::skill_repair_execution::recover_direct_repair_using(prepared, self)
    }

    pub fn execute(
        &self,
        selection: PreparedRepairSelection<'_>,
        event_id: &str,
    ) -> Result<RepairExecutionReceipt, RepairExecutionError> {
        execute_direct_repair_using(selection, self, event_id, |_| {})
    }

    pub fn execute_copy(
        &self,
        selection: crate::skill_service::PreparedCopyRepairSelection<'_>,
        event_id: &str,
    ) -> Result<RepairExecutionReceipt, RepairExecutionError> {
        crate::skill_repair_execution::execute_copy_repair_using(selection, self, event_id, |_| {})
    }

    fn exchange<'scope>(
        &self,
        lease: FinalizedWriteLease<'scope>,
        request: &PreparedEventExchange,
    ) -> (
        FinalizedWriteLease<'scope>,
        Result<serde_json::Value, String>,
    ) {
        match run_event_exchange(
            self.state_root,
            lease,
            (self.command)(),
            request,
            self.cancellation,
            self.deadline,
        ) {
            Ok(reaped) => (
                reaped.lease,
                reaped
                    .outcome
                    .map_err(|_| "Event exchange panicked after child cleanup".into()),
            ),
            Err(failure) => (failure.lease, Err(failure.reason)),
        }
    }

    pub(crate) fn commit<'scope>(
        &self,
        lease: FinalizedWriteLease<'scope>,
        request: Result<PreparedEventExchange, String>,
    ) -> LeaseResult<'scope> {
        let request = match request {
            Ok(request) => request,
            Err(error) => return (lease, Err(error)),
        };
        let (mut lease, reply) = self.exchange(lease, &request);
        let mut reply = match reply {
            Ok(reply) => reply,
            Err(error) => return (lease, Err(error)),
        };
        if reply["outcome"] == "unknown" {
            let lookup = match request.receipt_lookup("reconcile") {
                Ok(lookup) => lookup,
                Err(error) => return (lease, Err(error)),
            };
            let (returned, result) = self.exchange(lease, &lookup);
            lease = returned;
            reply = match result {
                Ok(reply) => reply,
                Err(error) => return (lease, Err(error)),
            };
        }
        let result = if reply["outcome"] == "committed" {
            Ok(())
        } else {
            Err(format!("Event commit not confirmed: {}", reply["outcome"]))
        };
        (lease, result)
    }
}

fn command_id(event: &str, phase: &str) -> String {
    Sha256::digest(format!("repair-worker-v1:{event}:{phase}"))
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl RepairEvents for RepairEventWorker<'_> {
    fn recover<'scope>(
        &self,
        lease: FinalizedWriteLease<'scope>,
        event: &crate::skill_repair_recovery_event::RepairRecoveryEvent,
        status: EventStatus,
        inverse: serde_json::Value,
    ) -> LeaseResult<'scope> {
        self.commit(
            lease,
            PreparedEventExchange::finish_recovery(
                "recover".into(),
                command_id(event.id(), "recover"),
                event.snapshot().clone(),
                status,
                Some(inverse),
            ),
        )
    }

    fn state_root(&self) -> &Path {
        self.state_root
    }

    fn ensure_running(&self) -> Result<(), String> {
        if self.cancellation.is_cancelled() || Instant::now() >= self.deadline {
            Err(
                "Repair cancelled or deadline exceeded; any pending intent requires recovery"
                    .into(),
            )
        } else {
            Ok(())
        }
    }

    fn preflight_record(
        &self,
        event: &str,
        timestamp: &str,
        draft: &EventDraft,
    ) -> Result<(), String> {
        PreparedEventExchange::record_pending(
            "record-preflight".into(),
            command_id(event, "record"),
            event.into(),
            timestamp.into(),
            draft.clone(),
        )
        .map(|_| ())
    }

    fn prepare<'scope>(
        &self,
        lease: FinalizedWriteLease<'scope>,
        event: &str,
    ) -> LeaseResult<'scope> {
        let (lease, result) = self.commit(
            lease,
            PreparedEventExchange::initialize(
                "initialize".into(),
                command_id(event, "initialize"),
                event.into(),
            ),
        );
        if let Err(error) = result {
            return (lease, Err(error));
        }
        let request = match PreparedEventExchange::preflight(
            "preflight".into(),
            command_id(event, "preflight"),
            event.into(),
        ) {
            Ok(request) => request,
            Err(error) => return (lease, Err(error)),
        };
        let (lease, reply) = self.exchange(lease, &request);
        let result = reply.and_then(|reply| {
            if reply["outcome"] == "preflight" && reply["recovery_required"] == false {
                Ok(())
            } else {
                Err("Repair requires a confirmed clear recovery preflight".into())
            }
        });
        (lease, result)
    }

    fn record<'scope>(
        &self,
        lease: FinalizedWriteLease<'scope>,
        event: &str,
        timestamp: String,
        draft: EventDraft,
    ) -> LeaseResult<'scope> {
        self.commit(
            lease,
            PreparedEventExchange::record_pending(
                "record".into(),
                command_id(event, "record"),
                event.into(),
                timestamp,
                draft,
            ),
        )
    }

    fn finish<'scope>(
        &self,
        lease: FinalizedWriteLease<'scope>,
        event: &str,
        inverse: Option<serde_json::Value>,
    ) -> LeaseResult<'scope> {
        self.commit(
            lease,
            PreparedEventExchange::finish_pending(
                "finish".into(),
                command_id(event, "finish"),
                event.into(),
                EventStatus::Done,
                inverse,
            ),
        )
    }
}
