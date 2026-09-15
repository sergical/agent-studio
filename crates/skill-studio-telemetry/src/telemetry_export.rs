use crate::SanitizedEnvelope;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const QUEUE_CAPACITY: usize = 8;
const MAX_SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Queued,
    Full,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushOutcome {
    Drained,
    Failed,
    TimedOut,
}

#[derive(Debug, Clone, Copy)]
pub struct ExportFailure;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportStats {
    pub accepted: u64,
    pub delivered: u64,
    pub failed: u64,
    pub dropped_full: u64,
    pub dropped_closed: u64,
    pub worker_panics: u64,
}

#[derive(Default)]
struct Counters {
    accepted: AtomicU64,
    delivered: AtomicU64,
    failed: AtomicU64,
    dropped_full: AtomicU64,
    dropped_closed: AtomicU64,
    worker_panics: AtomicU64,
}

enum WorkerState {
    Running,
    Finished { panicked: bool },
}
type Completion = Arc<(Mutex<WorkerState>, Condvar)>;

struct WorkerCompletion {
    state: Completion,
    finished: bool,
    counters: Arc<Counters>,
}
impl Drop for WorkerCompletion {
    fn drop(&mut self) {
        if !self.finished {
            self.counters.worker_panics.fetch_add(1, Ordering::Relaxed);
        }
        let (state, changed) = &*self.state;
        *state.lock().unwrap() = WorkerState::Finished {
            panicked: !self.finished,
        };
        changed.notify_all();
    }
}

pub struct TelemetryExporter {
    sender: Mutex<Option<mpsc::SyncSender<SanitizedEnvelope>>>,
    counters: Arc<Counters>,
    completion: Completion,
}

impl TelemetryExporter {
    pub fn start(
        mut sink: impl FnMut(&[u8]) -> Result<(), ExportFailure> + Send + 'static,
    ) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<SanitizedEnvelope>(QUEUE_CAPACITY);
        let counters = Arc::new(Counters::default());
        let completion = Arc::new((Mutex::new(WorkerState::Running), Condvar::new()));
        let state = completion.clone();
        let counts = counters.clone();
        std::thread::Builder::new()
            .name("skill-telemetry-output".into())
            .spawn(move || {
                let mut completed = WorkerCompletion {
                    state,
                    finished: false,
                    counters: counts.clone(),
                };
                for envelope in receiver {
                    let bytes = envelope.into_bytes();
                    let counter = if sink(&bytes).is_ok() {
                        &counts.delivered
                    } else {
                        &counts.failed
                    };
                    let (progress, changed) = &*completed.state;
                    let _progress = progress.lock().unwrap();
                    counter.fetch_add(1, Ordering::Relaxed);
                    changed.notify_all();
                }
                completed.finished = true;
            })?;
        Ok(Self {
            sender: Mutex::new(Some(sender)),
            counters,
            completion,
        })
    }

    pub fn try_enqueue(&self, envelope: SanitizedEnvelope) -> EnqueueOutcome {
        let sender = self.sender.lock().unwrap();
        let result = sender.as_ref().map(|sender| sender.try_send(envelope));
        let (outcome, counter) = match result {
            Some(Ok(())) => (EnqueueOutcome::Queued, &self.counters.accepted),
            Some(Err(mpsc::TrySendError::Full(_))) => {
                (EnqueueOutcome::Full, &self.counters.dropped_full)
            }
            Some(Err(mpsc::TrySendError::Disconnected(_))) | None => {
                (EnqueueOutcome::Closed, &self.counters.dropped_closed)
            }
        };
        counter.fetch_add(1, Ordering::Relaxed);
        outcome
    }

    pub fn flush(&self, budget: Duration) -> FlushOutcome {
        let deadline = Instant::now() + budget.min(MAX_SHUTDOWN_WAIT);
        let target = {
            let _admission = self.sender.lock().unwrap();
            self.counters.accepted.load(Ordering::Relaxed)
        };
        let (state, changed) = &*self.completion;
        let state = state.lock().unwrap();
        let (state, _) = changed
            .wait_timeout_while(
                state,
                deadline.saturating_duration_since(Instant::now()),
                |state| {
                    matches!(state, WorkerState::Running)
                        && self.counters.delivered.load(Ordering::Relaxed)
                            + self.counters.failed.load(Ordering::Relaxed)
                            < target
                },
            )
            .unwrap();
        if matches!(*state, WorkerState::Finished { panicked: true })
            || self.counters.failed.load(Ordering::Relaxed) > 0
        {
            FlushOutcome::Failed
        } else if self.counters.delivered.load(Ordering::Relaxed) >= target {
            FlushOutcome::Drained
        } else {
            FlushOutcome::TimedOut
        }
    }

    pub fn shutdown(&self, budget: Duration) -> FlushOutcome {
        let deadline = Instant::now() + budget.min(MAX_SHUTDOWN_WAIT);
        self.sender.lock().unwrap().take();
        let (state, changed) = &*self.completion;
        let state = state.lock().unwrap();
        let (state, _) = changed
            .wait_timeout_while(
                state,
                deadline.saturating_duration_since(Instant::now()),
                |state| matches!(state, WorkerState::Running),
            )
            .unwrap();
        match *state {
            WorkerState::Running => FlushOutcome::TimedOut,
            WorkerState::Finished { panicked: true } => FlushOutcome::Failed,
            WorkerState::Finished { panicked: false }
                if self.counters.failed.load(Ordering::Relaxed) > 0 =>
            {
                FlushOutcome::Failed
            }
            WorkerState::Finished { .. } => FlushOutcome::Drained,
        }
    }

    pub fn stats(&self) -> ExportStats {
        ExportStats {
            accepted: self.counters.accepted.load(Ordering::Relaxed),
            delivered: self.counters.delivered.load(Ordering::Relaxed),
            failed: self.counters.failed.load(Ordering::Relaxed),
            dropped_full: self.counters.dropped_full.load(Ordering::Relaxed),
            dropped_closed: self.counters.dropped_closed.load(Ordering::Relaxed),
            worker_panics: self.counters.worker_panics.load(Ordering::Relaxed),
        }
    }
}
