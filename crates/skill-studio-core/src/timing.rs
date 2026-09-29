//! Per-operation timing.
//!
//! Every op in [`crate::ops`] measures itself against the [`crate::ports::Clock`]
//! port, never `std::time::Instant`: a fake clock in a test then measures the
//! same code path a real one does in production. [`step`] and [`op_timing`]
//! are the two points an op touches — one per named section of work, one for
//! the call as a whole — so instrumentation reads as a pair of calls around
//! the section it times rather than a separate stopwatch type to thread
//! through.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::ports::Clock;

/// Elapsed time for one named section of an op call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StepTiming {
    /// Section name, unique within one [`OpTiming::steps`].
    pub name: String,
    /// Elapsed milliseconds.
    pub elapsed_ms: u64,
    /// Name of the step this one is a cumulative sub-time of, when it is
    /// one (e.g. `dir_walk` is time already counted inside `roots_walk`).
    /// `#[serde(default, ...)]` so `timing.jsonl` written before this field
    /// existed still deserializes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// Elapsed time for one whole op call, plus the named steps inside it.
///
/// Invariant: every entry in `steps` measures a subset of the call, so its
/// `elapsed_ms` never exceeds this `elapsed_ms`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpTiming {
    /// Operation name, matching [`crate::ops::Operation`]'s `snake_case` form.
    pub op: String,
    /// Elapsed milliseconds for the whole call.
    pub elapsed_ms: u64,
    /// Named sections measured inside the call, in the order they ran.
    pub steps: Vec<StepTiming>,
}

/// Measures one named section: `since` is an earlier `clock.monotonic()`
/// reading taken where the section started. `name` takes only `&'static
/// str`, not `&str`, so every call site is a literal a reader can grep for -
/// the same guarantee `StepTiming.name` (kept as `String` for `timing.jsonl`)
/// cannot make by itself.
pub fn step(clock: &dyn Clock, name: &'static str, since: Duration) -> StepTiming {
    StepTiming {
        name: name.to_string(),
        elapsed_ms: clock.monotonic().saturating_sub(since).as_millis() as u64,
        parent: None,
    }
}

/// Builds the whole-call [`OpTiming`]: `since` is the `clock.monotonic()`
/// reading taken when the op started.
pub fn op_timing(clock: &dyn Clock, op: &str, since: Duration, steps: Vec<StepTiming>) -> OpTiming {
    OpTiming {
        op: op.to_string(),
        elapsed_ms: clock.monotonic().saturating_sub(since).as_millis() as u64,
        steps,
    }
}
