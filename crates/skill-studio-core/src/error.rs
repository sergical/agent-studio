//! Typed errors with stable string codes and fixed exit statuses.
//!
//! Adapters map a [`CoreError`] to a process exit status through
//! [`ErrorCode::exit_status`]. The mapping is pure and is tested by the
//! golden envelope snapshots.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::scope::NormalizedScope;

/// Stable error code.
///
/// Invariant: the serialized name (`snake_case`) and the exit status of a
/// variant never change once released. New variants may be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request itself is malformed (missing field, bad id syntax).
    InvalidRequest,
    /// The scope cannot be used (missing home, override outside fixture mode).
    InvalidScope,
    /// A target id matched more than one deployment or none after re-validation.
    AmbiguousTarget,
    /// The harness has no support for the requested mechanism.
    Unsupported,
    /// A child process or external tool failed.
    ExecutionFailed,
    /// A filesystem or store read or write failed.
    Io,
    /// Another process holds the lease and the wait budget ran out.
    ScopeBusy,
    /// The proposal fingerprint no longer matches the file on disk.
    StaleProposal,
    /// The live content drifted from the fingerprint recorded in history.
    DriftConflict,
    /// The owner of the deployment changed between preview and apply.
    OwnershipChanged,
    /// The event was already reverted by another restore.
    AlreadyReverted,
    /// The operation finished but some roots could not be read in time.
    Incomplete,
    /// The caller cancelled the operation.
    Cancelled,
}

impl ErrorCode {
    /// Returns the process exit status for this code.
    ///
    /// `2` is a caller problem, `3` is a conflict the caller can retry,
    /// `4` is a partial result, `130` is a cancellation.
    pub const fn exit_status(self) -> i32 {
        match self {
            ErrorCode::InvalidRequest
            | ErrorCode::InvalidScope
            | ErrorCode::AmbiguousTarget
            | ErrorCode::Unsupported
            | ErrorCode::ExecutionFailed
            | ErrorCode::Io => 2,
            ErrorCode::ScopeBusy
            | ErrorCode::StaleProposal
            | ErrorCode::DriftConflict
            | ErrorCode::OwnershipChanged
            | ErrorCode::AlreadyReverted => 3,
            ErrorCode::Incomplete => 4,
            ErrorCode::Cancelled => 130,
        }
    }

    /// Returns the stable string form used on the wire and in golden files.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::InvalidScope => "invalid_scope",
            ErrorCode::AmbiguousTarget => "ambiguous_target",
            ErrorCode::Unsupported => "unsupported",
            ErrorCode::ExecutionFailed => "execution_failed",
            ErrorCode::Io => "io",
            ErrorCode::ScopeBusy => "scope_busy",
            ErrorCode::StaleProposal => "stale_proposal",
            ErrorCode::DriftConflict => "drift_conflict",
            ErrorCode::OwnershipChanged => "ownership_changed",
            ErrorCode::AlreadyReverted => "already_reverted",
            ErrorCode::Incomplete => "incomplete",
            ErrorCode::Cancelled => "cancelled",
        }
    }
}

/// Error raised by a core operation.
///
/// Invariant: `message` is written for a person and never contains secrets.
/// `path` holds the absolute path that failed; [`CoreError::sanitized`]
/// rewrites it relative to the scope before it leaves the core.
#[derive(Debug, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct CoreError {
    /// Stable code.
    pub code: ErrorCode,
    /// Human-readable message.
    pub message: String,
    /// Path the error refers to, when there is one.
    pub path: Option<PathBuf>,
    /// Underlying I/O cause, kept for logs only.
    #[source]
    pub source: Option<std::io::Error>,
}

impl CoreError {
    /// Builds an error with a code and a message.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        CoreError {
            code,
            message: message.into(),
            path: None,
            source: None,
        }
    }

    /// Attaches the path the error refers to.
    pub fn at(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Wraps an I/O error under [`ErrorCode::Io`].
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        let path = path.into();
        CoreError {
            code: ErrorCode::Io,
            message: source.to_string(),
            path: Some(path),
            source: Some(source),
        }
    }

    /// Builds the wire form. Absolute paths under the scope home become
    /// `~/...`; paths outside the home stay absolute.
    pub fn sanitized(&self, scope: &NormalizedScope) -> ErrorEntry {
        ErrorEntry {
            code: self.code,
            message: self.message.clone(),
            path: self.path.as_ref().map(|p| scope.display_path(p)),
        }
    }
}

/// Wire form of a [`CoreError`], as carried by the result envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorEntry {
    /// Stable code.
    pub code: ErrorCode,
    /// Human-readable message.
    pub message: String,
    /// Display path relative to the scope home, when there is one.
    pub path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_statuses_match_the_table() {
        assert_eq!(ErrorCode::InvalidRequest.exit_status(), 2);
        assert_eq!(ErrorCode::ScopeBusy.exit_status(), 3);
        assert_eq!(ErrorCode::Incomplete.exit_status(), 4);
        assert_eq!(ErrorCode::Cancelled.exit_status(), 130);
    }

    #[test]
    fn serde_name_equals_as_str() {
        let json = serde_json::to_string(&ErrorCode::DriftConflict).unwrap();
        assert_eq!(json, format!("\"{}\"", ErrorCode::DriftConflict.as_str()));
    }
}
