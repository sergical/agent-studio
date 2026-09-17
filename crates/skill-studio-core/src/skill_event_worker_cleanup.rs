//! Exit-before-release ownership for blocking event supervisor work.
use std::sync::{Arc, Mutex};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventProgress {
    Acquiring,
    Running,
    Stopping,
    CleanupPending,
    Recovering,
    Finished,
}
#[derive(Clone)]
pub struct ProgressSink(pub(crate) Arc<Mutex<EventProgress>>);
impl ProgressSink {
    pub(crate) fn set(&self, phase: EventProgress) {
        *self.0.lock().unwrap_or_else(|error| error.into_inner()) = phase;
    }
    pub fn get(&self) -> EventProgress {
        *self.0.lock().unwrap_or_else(|error| error.into_inner())
    }
}

use crate::skill_coordination::FinalizedWriteLease;
use std::{
    io,
    process::{Child, ExitStatus},
    time::Duration,
};

pub(crate) trait ProcessControl {
    fn observe_exit(&mut self) -> io::Result<Option<ExitStatus>>;
    fn request_termination(&mut self) -> io::Result<()>;
}
impl ProcessControl for Child {
    fn observe_exit(&mut self) -> io::Result<Option<ExitStatus>> {
        self.try_wait()
    }
    fn request_termination(&mut self) -> io::Result<()> {
        self.kill()
    }
}

impl ProcessControl for crate::skill_history_worker_process::HistoryWorkerProcess {
    fn observe_exit(&mut self) -> io::Result<Option<ExitStatus>> {
        self.observe_exit()
    }
    fn request_termination(&mut self) -> io::Result<()> {
        self.terminate_and_reap().map(|_| ())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExitWaitStopped {
    Cancelled,
    Deadline,
    Observation,
}

pub(crate) fn wait_for_exit_controlled(
    process: &mut impl ProcessControl,
    cancellation: &crate::skill_coordination::CancellationToken,
    deadline: std::time::Instant,
) -> Result<ExitStatus, ExitWaitStopped> {
    loop {
        if cancellation.is_cancelled() {
            return Err(ExitWaitStopped::Cancelled);
        }
        if std::time::Instant::now() >= deadline {
            return Err(ExitWaitStopped::Deadline);
        }
        if let Some(status) = process
            .observe_exit()
            .map_err(|_| ExitWaitStopped::Observation)?
        {
            return Ok(status);
        }
        std::thread::sleep(
            Duration::from_millis(5)
                .min(deadline.saturating_duration_since(std::time::Instant::now())),
        );
    }
}

// The caller must run this on the owning supervisor thread. An execution
// deadline cannot authorize dropping the lease while the child may still write.
fn reap_before_release<'scope>(
    process: &mut impl ProcessControl,
    lease: FinalizedWriteLease<'scope>,
    progress: Option<&ProgressSink>,
) -> (FinalizedWriteLease<'scope>, ExitStatus) {
    let mut delay = Duration::from_millis(10);
    if let Some(progress) = progress {
        progress.set(EventProgress::Stopping);
    }
    let mut reported_pending = false;
    loop {
        if let Ok(Some(status)) = process.observe_exit() {
            return (lease, status);
        }
        let _ = process.request_termination();
        // A kill error may race with exit. Only observation establishes exit.
        if let Ok(Some(status)) = process.observe_exit() {
            return (lease, status);
        }
        if !reported_pending {
            if let Some(progress) = progress {
                progress.set(EventProgress::CleanupPending);
            }
            reported_pending = true;
        }
        std::thread::sleep(delay);
        delay = (delay * 2).min(Duration::from_secs(1));
    }
}

pub struct ReapedOperation<'scope, T> {
    pub lease: FinalizedWriteLease<'scope>,
    pub exit: ExitStatus,
    pub outcome: std::thread::Result<T>,
}

pub(crate) fn run_and_reap<'scope, P: ProcessControl, T>(
    process: &mut P,
    lease: FinalizedWriteLease<'scope>,
    operation: impl FnOnce(&mut P, &FinalizedWriteLease<'scope>) -> T,
) -> ReapedOperation<'scope, T> {
    run_and_reap_reporting(process, lease, None, operation)
}

pub(crate) fn run_and_reap_reporting<'scope, P: ProcessControl, T>(
    process: &mut P,
    lease: FinalizedWriteLease<'scope>,
    progress: Option<&ProgressSink>,
    operation: impl FnOnce(&mut P, &FinalizedWriteLease<'scope>) -> T,
) -> ReapedOperation<'scope, T> {
    // The closure borrows both owners; unwinding cannot drop either one.
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(process, &lease)));
    let (lease, exit) = reap_before_release(process, lease, progress);
    ReapedOperation {
        lease,
        exit,
        outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect},
        skill_scope::SkillReadScope,
    };
    use std::{
        collections::VecDeque,
        os::unix::process::ExitStatusExt,
        path::{Path, PathBuf},
    };

    enum Step {
        Unknown,
        Running,
        Exited,
    }
    struct FaultedProcess {
        root: PathBuf,
        steps: VecDeque<Step>,
        termination_calls: usize,
    }
    fn competing(root: &Path) -> CoordinationPlan {
        CoordinationPlan::new_fixture(
            vec![DirectoryEffect::tree(root, CoordinationMode::Exclusive)],
            root,
            Some(Duration::from_millis(10)),
        )
        .unwrap()
    }
    impl ProcessControl for FaultedProcess {
        fn observe_exit(&mut self) -> io::Result<Option<ExitStatus>> {
            assert!(
                competing(&self.root).acquire().is_err(),
                "lease released before exit proof"
            );
            match self
                .steps
                .pop_front()
                .expect("unexpected status observation")
            {
                Step::Unknown => Err(io::Error::other("injected wait failure")),
                Step::Running => Ok(None),
                Step::Exited => Ok(Some(ExitStatus::from_raw(0))),
            }
        }
        fn request_termination(&mut self) -> io::Result<()> {
            assert!(competing(&self.root).acquire().is_err());
            self.termination_calls += 1;
            Err(io::Error::other("injected kill failure"))
        }
    }
    fn run(steps: Vec<Step>, expected_termination_calls: usize) {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
        let lease = competing(&root)
            .acquire()
            .unwrap()
            .finalize_write(&scope, &[])
            .unwrap();
        let mut process = FaultedProcess {
            root: root.clone(),
            steps: steps.into(),
            termination_calls: 0,
        };
        let reaped = run_and_reap(&mut process, lease, |_, _| -> () {
            panic!("injected operation panic");
        });
        assert!(reaped.outcome.is_err());
        let lease = reaped.lease;
        assert!(reaped.exit.success());
        assert!(process.steps.is_empty());
        assert_eq!(process.termination_calls, expected_termination_calls);
        assert!(competing(&root).acquire().is_err());
        drop(lease);
        CoordinationPlan::new_fixture(
            vec![DirectoryEffect::tree(&root, CoordinationMode::Exclusive)],
            &root,
            Some(Duration::from_secs(1)),
        )
        .unwrap()
        .acquire()
        .unwrap();
    }
    #[test]
    fn cleanup_retains_lease_through_observation_and_kill_failures() {
        run(
            vec![Step::Unknown, Step::Unknown, Step::Running, Step::Exited],
            2,
        );
    }
    #[test]
    fn cleanup_accepts_confirmed_exit_after_failed_kill() {
        run(vec![Step::Running, Step::Exited], 1);
    }
    #[test]
    fn cleanup_does_not_kill_an_already_reaped_child() {
        run(vec![Step::Exited], 0);
    }
}

#[test]
#[ignore = "private cleanup child"]
fn cleanup_child() {
    std::thread::sleep(Duration::from_secs(30));
}

#[test]
fn cleanup_reaps_real_child_before_releasing_lease() {
    use crate::{
        skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect},
        skill_scope::SkillReadScope,
    };
    use std::process::{Command, Stdio};
    for panic_operation in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
        let plan = || {
            CoordinationPlan::new_fixture(
                vec![DirectoryEffect::tree(&root, CoordinationMode::Exclusive)],
                &root,
                Some(Duration::from_millis(10)),
            )
            .unwrap()
        };
        let lease = plan()
            .acquire()
            .unwrap()
            .finalize_write(&scope, &[])
            .unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "skill_event_worker_cleanup::cleanup_child",
                "--ignored",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let reaped = run_and_reap(&mut child, lease, |_, lease| {
            lease.validate_state_tree(&root).unwrap();
            assert!(plan().acquire().is_err());
            assert!(!panic_operation, "injected operation panic");
            42
        });
        assert_eq!(reaped.outcome.is_err(), panic_operation);
        if let Ok(value) = reaped.outcome {
            assert_eq!(value, 42);
        }
        let lease = reaped.lease;
        assert!(!reaped.exit.success());
        assert_eq!(child.try_wait().unwrap(), Some(reaped.exit));
        assert!(plan().acquire().is_err());
        drop(lease);
        assert!(plan().acquire().is_ok());
    }
}

#[test]
fn exit_wait_checks_control_before_process_observation() {
    struct Unobserved;
    impl ProcessControl for Unobserved {
        fn observe_exit(&mut self) -> io::Result<Option<ExitStatus>> {
            panic!("control should stop observation")
        }
        fn request_termination(&mut self) -> io::Result<()> {
            panic!("wait does not own cleanup")
        }
    }
    let cancellation = crate::skill_coordination::CancellationToken::default();
    assert_eq!(
        wait_for_exit_controlled(&mut Unobserved, &cancellation, std::time::Instant::now()),
        Err(ExitWaitStopped::Deadline)
    );
    cancellation.cancel();
    assert_eq!(
        wait_for_exit_controlled(
            &mut Unobserved,
            &cancellation,
            std::time::Instant::now() + Duration::from_secs(1)
        ),
        Err(ExitWaitStopped::Cancelled)
    );
}
