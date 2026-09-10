// ============================================================================
// Skills Module - core_content_hash
// Thin desktop wrappers over `skill_studio_core::ops::skill_content_hash`,
// the recompute-the-live-hash-outside-a-scan entry point mutation guards
// (`skill_add`, `skill_harness_disable`, `skill_independent_copy`,
// `skill_lifecycle`) call before writing over an existing deployment. Owns
// the mapping from `CoreError` to the plain `String` those callers expect,
// and the cancellation bridge from `AddOperationControl` to core's
// `CancelToken`.
// ============================================================================

use std::path::Path;
use std::sync::Arc;

use skill_studio_core::identity::CorrelationId;
use skill_studio_core::ops::skill_content_hash;
use skill_studio_core::ports::{CancelToken, OpContext};

use super::skill_process::AddOperationControl;

/// A missing/unreadable SKILL.md is the only failure mode a synchronous,
/// uncancellable call can hit - core reports it as `ErrorCode::Io`, but
/// this wrapper's callers expect this exact message, not core's own
/// wire-formatted one.
fn missing_skill_md_message(skill_dir: &Path) -> String {
    format!("Could not read {}/SKILL.md", skill_dir.display())
}

/// Recomputes the bounded strong folder hash for `skill_dir`, outside of a
/// scan. Matches `skill_studio_core::ops::skill_content_hash` exactly; this
/// only adds the uncancellable `OpContext` and the desktop's error string.
pub fn live_skill_content_hash(skill_dir: &Path) -> Result<String, String> {
    let fs = skill_studio_host::RealFs::new();
    let ctx = OpContext::uncancellable(CorrelationId("desktop-content-hash".into()));
    skill_content_hash(&fs, &ctx, skill_dir).map_err(|_| missing_skill_md_message(skill_dir))
}

/// Bridges one Add operation's cooperative cancellation
/// (`AddOperationControl`) to core's `CancelToken`, so
/// `ops::skill_content_hash`'s own per-entry/per-chunk checkpoints stop the
/// walk the same way `AddOperationControl::check_message` used to.
struct ControlCancelToken(AddOperationControl);

impl CancelToken for ControlCancelToken {
    fn is_cancelled(&self) -> bool {
        self.0.check().is_err()
    }
}

/// As [`live_skill_content_hash`], but checked against `control` during
/// directory traversal and each streamed file chunk, for use on the Add
/// Skill background worker. A cancellation or deadline expiry surfaces
/// `control`'s own message (`PROCESS_CANCELLED_MESSAGE` /
/// `PROCESS_TIMED_OUT_MESSAGE`), never a core-formatted string, since
/// `skill_add` callers distinguish those literals from everything else.
pub fn live_skill_content_hash_controlled(
    skill_dir: &Path,
    control: &AddOperationControl,
) -> Result<String, String> {
    control.check_message()?;
    let fs = skill_studio_host::RealFs::new();
    let ctx = OpContext {
        correlation_id: CorrelationId("desktop-content-hash".into()),
        cancel: Arc::new(ControlCancelToken(control.clone())),
    };
    skill_content_hash(&fs, &ctx, skill_dir).map_err(|err| {
        if err.code == skill_studio_core::error::ErrorCode::Cancelled {
            control
                .check_message()
                .expect_err("core reported Cancelled, so control must report it too")
        } else {
            missing_skill_md_message(skill_dir)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    use super::super::test_support::write_skill;

    #[test]
    fn already_cancelled_control_yields_cancelled_message() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join("skill");
        write_skill(&skill_dir, "widget");

        let control =
            AddOperationControl::new(Arc::new(AtomicBool::new(true)), Duration::from_secs(300));

        let err = live_skill_content_hash_controlled(&skill_dir, &control).unwrap_err();
        assert_eq!(err, super::super::skill_process::PROCESS_CANCELLED_MESSAGE);
    }

    #[test]
    fn expired_deadline_control_yields_timed_out_message() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join("skill");
        write_skill(&skill_dir, "widget");

        let control = AddOperationControl::with_deadline(
            Arc::new(AtomicBool::new(false)),
            Instant::now() - Duration::from_secs(1),
        );

        let err = live_skill_content_hash_controlled(&skill_dir, &control).unwrap_err();
        assert_eq!(err, super::super::skill_process::PROCESS_TIMED_OUT_MESSAGE);
    }

    #[test]
    fn uncancelled_control_hashes_same_as_uncontrolled() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join("skill");
        write_skill(&skill_dir, "widget");

        let control =
            AddOperationControl::new(Arc::new(AtomicBool::new(false)), Duration::from_secs(300));

        let controlled = live_skill_content_hash_controlled(&skill_dir, &control).unwrap();
        let uncontrolled = live_skill_content_hash(&skill_dir).unwrap();
        assert_eq!(controlled, uncontrolled);
    }
}
