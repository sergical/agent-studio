//! Sanitizer for outbound error reports.
//!
//! Opt-in error reporting (unit 6.4) sends a panic or a command failure to
//! Sentry, but the shape that leaves the machine must never carry a home
//! path, a skill name, a project path, or a file body - those are exactly
//! the strings a Rust panic message or a failed op's context tends to
//! carry. [`sanitize`] is pure: no filesystem, no clock, no network. The
//! caller assembles a [`RawReport`] from whatever it already knows about
//! the failure, plus a [`SensitiveContext`] naming the values that came
//! from this machine, and [`sanitize`] is the only place that is allowed
//! to see both at once.

use serde::{Deserialize, Serialize};

/// Operation names a sanitized report may carry, matching
/// [`crate::ops::Operation`]'s `snake_case` form.
pub const ALLOWED_OPERATIONS: &[&str] = &[
    "skill.scan",
    "skill.diagnose",
    "skill.park",
    "skill.fork",
    "skill.pull_fork_upstream",
    "skill.unfork",
    "skill.enable",
    "skill.disable",
    "skill.install",
    "skill.remove",
    "skill.update",
    "skill.run",
    "skill.repair_frontmatter",
];

/// Dimension keys a sanitized report may carry, and the values each one
/// may take. A dimension with an unknown key, or a key whose value isn't
/// one of these, is dropped rather than passed through partially.
pub const ALLOWED_DIMENSIONS: &[(&str, &[&str])] = &[
    ("outcome", &["success", "failure", "cancelled"]),
    (
        "cause",
        &[
            "io_error",
            "invalid_request",
            "invalid_scope",
            "conflict",
            "not_found",
            "internal",
        ],
    ),
    (
        "phase",
        &["accepted", "running", "committed", "failed", "cancelled"],
    ),
    (
        "error_code",
        &[
            "invalid_request",
            "invalid_scope",
            "not_found",
            "conflict",
            "io_error",
            "internal",
        ],
    ),
];

/// Maximum number of items - dimensions, exceptions, and stack frames
/// together - a sanitized envelope may hold.
pub const MAX_ITEMS: usize = 64;

/// Maximum number of exceptions a sanitized envelope may hold.
pub const MAX_EXCEPTIONS: usize = 8;

/// Maximum serialized size of a sanitized envelope, in bytes.
pub const MAX_BYTES: usize = 256 * 1024;

/// One name/value pair on a report. Only pairs from [`ALLOWED_DIMENSIONS`]
/// survive [`sanitize`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dimension {
    /// The dimension's name, for example `"outcome"`.
    pub key: String,
    /// The dimension's value, for example `"failure"`.
    pub value: String,
}

/// One stack frame as the caller has it, before sanitizing. `absolute_path`
/// may be a real path on the machine that failed; [`sanitize`] never lets
/// it through as given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawFrame {
    /// The frame's source file, if known, as an absolute path.
    pub absolute_path: Option<String>,
    /// The function or symbol name.
    pub function: String,
    /// The instruction address, if known; not sensitive, so it survives
    /// sanitizing unchanged.
    pub instruction_addr: Option<String>,
}

/// One exception as the caller has it, before sanitizing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawException {
    /// The panic message or the error's `Display` text.
    pub message: String,
    /// The exception's stack, outermost frame first.
    pub frames: Vec<RawFrame>,
}

/// A report as the caller assembles it: whatever it knows about the
/// failure, not yet checked against the allow list or the caps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawReport {
    /// The operation that was running, if the failure happened inside one.
    pub operation: Option<String>,
    /// Dimensions describing the failure.
    pub dimensions: Vec<Dimension>,
    /// The exception chain, outermost first.
    pub exceptions: Vec<RawException>,
}

/// Values that must never survive [`sanitize`], gathered by the caller from
/// the same context that produced the failure. Every exact, non-empty
/// occurrence of one of these inside a message or a frame path is redacted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SensitiveContext {
    /// The user's home directory, for example `/Users/alice`.
    pub home_path: Option<String>,
    /// The skill name involved in the failure, if any.
    pub skill_name: Option<String>,
    /// The project path involved in the failure, if any.
    pub project_path: Option<String>,
}

/// One stack frame after sanitizing: reduced to what cannot leak a path or
/// a name. `app_relative_path` keeps only the frame's file name, never its
/// directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackFrame {
    /// The frame's file name only, with every directory component removed.
    pub app_relative_path: Option<String>,
    /// The instruction address, unchanged from [`RawFrame::instruction_addr`].
    pub instruction_addr: Option<String>,
}

/// One exception after sanitizing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SanitizedException {
    /// The exception's message, redacted and reduced to one line.
    pub message: String,
    /// The exception's stack, outermost frame first.
    pub frames: Vec<StackFrame>,
}

/// A report ready to leave the machine: every field passed the allow list
/// and the caps in this module.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SanitizedEnvelope {
    /// The operation, kept only when it is in [`ALLOWED_OPERATIONS`].
    pub operation: Option<String>,
    /// The dimensions, kept only when each is in [`ALLOWED_DIMENSIONS`].
    pub dimensions: Vec<Dimension>,
    /// The exceptions, capped at [`MAX_EXCEPTIONS`].
    pub exceptions: Vec<SanitizedException>,
}

/// True when `key`/`value` is an exact match in [`ALLOWED_DIMENSIONS`].
fn is_allowed_dimension(key: &str, value: &str) -> bool {
    ALLOWED_DIMENSIONS
        .iter()
        .any(|(allowed_key, allowed_values)| *allowed_key == key && allowed_values.contains(&value))
}

/// Redacts every exact occurrence of a non-empty [`SensitiveContext`] field
/// in `text`.
fn redact_sensitive(text: &str, sensitive: &SensitiveContext) -> String {
    let mut out = text.to_string();
    for value in [
        &sensitive.home_path,
        &sensitive.skill_name,
        &sensitive.project_path,
    ]
    .into_iter()
    .flatten()
    {
        if !value.is_empty() {
            out = out.replace(value.as_str(), "[redacted]");
        }
    }
    out
}

/// A line beyond the first is a file's body, not a failure message; keeping
/// only the first line and naming the cut is how [`sanitize`] refuses a
/// file body without needing to recognise one by content.
fn strip_body(text: &str) -> String {
    match text.split_once('\n') {
        Some((first_line, _rest)) => format!("{first_line} [body stripped]"),
        None => text.to_string(),
    }
}

/// Redacts every `/Users/<name>`, `/home/<name>`, or `C:\Users\<name>`
/// segment in `text` by pattern, not just the exact [`SensitiveContext`]
/// values - a panic message can quote a path under someone else's home
/// directory (a symlinked skill, a different account) that
/// [`redact_sensitive`] would never catch because it isn't this machine's
/// recorded `home_path`. Only the username segment is replaced; the rest of
/// the path is kept so the message still says where inside home it failed.
fn redact_home_style_paths(text: &str) -> String {
    const PREFIXES: [(&str, char); 3] = [("/Users/", '/'), ("/home/", '/'), ("C:\\Users\\", '\\')];
    let mut out = text.to_string();
    for (prefix, separator) in PREFIXES {
        while let Some(start) = out.find(prefix) {
            let after_prefix = start + prefix.len();
            let end = out[after_prefix..]
                .find(|c: char| c == separator || c.is_whitespace())
                .map_or(out.len(), |offset| after_prefix + offset);
            out.replace_range(start..end, "<home>");
        }
    }
    out
}

/// Sanitizes one exception message: strips any file body, redacts the
/// sensitive context, then redacts any home-style path pattern that isn't
/// one of the sensitive context's exact values.
fn sanitize_message(message: &str, sensitive: &SensitiveContext) -> String {
    redact_home_style_paths(&redact_sensitive(&strip_body(message), sensitive))
}

/// Sanitizes one frame: keeps the instruction address as-is and reduces the
/// path to its file name only, dropping every directory component - so a
/// home path or a project path inside the directory never survives.
fn sanitize_frame(frame: &RawFrame) -> StackFrame {
    let app_relative_path = frame
        .absolute_path
        .as_deref()
        .map(|path| path.rsplit(['/', '\\']).next().unwrap_or(path).to_string());
    StackFrame {
        app_relative_path,
        instruction_addr: frame.instruction_addr.clone(),
    }
}

/// Counts the "items" [`MAX_ITEMS`] caps: every dimension, every exception,
/// and every frame inside every exception.
fn item_count(envelope: &SanitizedEnvelope) -> usize {
    envelope.dimensions.len()
        + envelope.exceptions.len()
        + envelope
            .exceptions
            .iter()
            .map(|e| e.frames.len())
            .sum::<usize>()
}

/// Drops frames, then whole exceptions, then dimensions - in that order,
/// from the end - until `item_count` is at or under [`MAX_ITEMS`].
fn cap_items(envelope: &mut SanitizedEnvelope) {
    while item_count(envelope) > MAX_ITEMS {
        if let Some(exception) = envelope
            .exceptions
            .iter_mut()
            .rev()
            .find(|e| !e.frames.is_empty())
        {
            exception.frames.pop();
        } else if !envelope.exceptions.is_empty() {
            envelope.exceptions.pop();
        } else if !envelope.dimensions.is_empty() {
            envelope.dimensions.pop();
        } else {
            break;
        }
    }
}

/// Drops frames, then whole exceptions, then dimensions - in that order,
/// from the end - until the serialized envelope is at or under
/// [`MAX_BYTES`]. Serializing to measure is what "per envelope" means;
/// nothing here assumes a particular wire format beyond JSON.
fn cap_bytes(envelope: &mut SanitizedEnvelope) {
    while serde_json::to_vec(envelope).map_or(0, |bytes| bytes.len()) > MAX_BYTES {
        if let Some(exception) = envelope
            .exceptions
            .iter_mut()
            .rev()
            .find(|e| !e.frames.is_empty())
        {
            exception.frames.pop();
        } else if !envelope.exceptions.is_empty() {
            envelope.exceptions.pop();
        } else if !envelope.dimensions.is_empty() {
            envelope.dimensions.pop();
        } else {
            break;
        }
    }
}

/// Sanitizes `raw` against the allow list, the sensitive context, and the
/// caps. The result is safe to serialize and send: no operation or
/// dimension outside the allow list, no home path, skill name, project
/// path, or file body in any message or frame, and no more than
/// [`MAX_EXCEPTIONS`] exceptions, [`MAX_ITEMS`] items, or [`MAX_BYTES`]
/// bytes once serialized.
pub fn sanitize(raw: &RawReport, sensitive: &SensitiveContext) -> SanitizedEnvelope {
    let operation = raw
        .operation
        .as_deref()
        .filter(|op| ALLOWED_OPERATIONS.contains(op))
        .map(String::from);

    let dimensions: Vec<Dimension> = raw
        .dimensions
        .iter()
        .filter(|d| is_allowed_dimension(&d.key, &d.value))
        .cloned()
        .collect();

    let exceptions: Vec<SanitizedException> = raw
        .exceptions
        .iter()
        .take(MAX_EXCEPTIONS)
        .map(|e| SanitizedException {
            message: sanitize_message(&e.message, sensitive),
            frames: e.frames.iter().map(sanitize_frame).collect(),
        })
        .collect();

    let mut envelope = SanitizedEnvelope {
        operation,
        dimensions,
        exceptions,
    };
    cap_items(&mut envelope);
    cap_bytes(&mut envelope);
    envelope
}

#[cfg(test)]
mod tests {
    use super::*;

    /// guards: a panic message built from a real failure (home path in a
    /// frame, skill name and a whole SKILL.md body in the exception
    /// message) reaching Sentry unredacted.
    #[test]
    fn sanitizer_strips_home_path_skill_name_and_file_body_or_names_the_leaked_field() {
        let sensitive = SensitiveContext {
            home_path: Some("/Users/alice".to_string()),
            skill_name: Some("my-private-skill".to_string()),
            project_path: Some("/Users/alice/work/secret-project".to_string()),
        };
        let file_body =
            "---\nname: my-private-skill\n---\nInternal rollout notes for secret-project.";
        let raw = RawReport {
            operation: Some("skill.scan".to_string()),
            dimensions: vec![],
            exceptions: vec![RawException {
                message: format!(
                    "failed to read /Users/alice/.claude/skills/my-private-skill/SKILL.md\n{file_body}"
                ),
                frames: vec![RawFrame {
                    absolute_path: Some(
                        "/Users/alice/work/secret-project/src/scan.rs".to_string(),
                    ),
                    function: "skill_studio_core::ops::scan".to_string(),
                    instruction_addr: Some("0x1000abcd".to_string()),
                }],
            }],
        };

        let sanitized = sanitize(&raw, &sensitive);
        let exception = &sanitized.exceptions[0];

        assert!(
            !exception.message.contains("/Users/alice"),
            "home path leaked in exception.message: {}",
            exception.message
        );
        assert!(
            !exception.message.contains("my-private-skill"),
            "skill name leaked in exception.message: {}",
            exception.message
        );
        assert!(
            !exception.message.contains("secret-project"),
            "project path leaked in exception.message: {}",
            exception.message
        );
        assert!(
            !exception.message.contains("Internal rollout notes"),
            "file body leaked in exception.message: {}",
            exception.message
        );
        let frame = &exception.frames[0];
        assert_eq!(
            frame.app_relative_path.as_deref(),
            Some("scan.rs"),
            "frame.app_relative_path kept a directory component: {frame:?}"
        );
    }

    /// guards: a home path that isn't this machine's recorded
    /// `SensitiveContext.home_path` - a panic quoting another account's home
    /// directory, or the exact panic-shaped report `install_panic_hook`
    /// builds - surviving `redact_sensitive`'s exact-match check.
    #[test]
    fn sanitizer_strips_home_path_by_pattern_when_not_the_sensitive_context_value_or_names_the_leaked_field(
    ) {
        let sensitive = SensitiveContext {
            home_path: Some("/Users/alice".to_string()),
            skill_name: None,
            project_path: None,
        };
        let raw = RawReport {
            operation: Some("skill.scan".to_string()),
            dimensions: vec![],
            exceptions: vec![RawException {
                message: "No such file: /Users/bob/.claude/skills/x/SKILL.md".to_string(),
                frames: vec![],
            }],
        };

        let sanitized = sanitize(&raw, &sensitive);
        let message = &sanitized.exceptions[0].message;

        assert!(
            !message.contains("/Users/"),
            "a home path outside SensitiveContext.home_path leaked: {message}"
        );

        // Same shape `install_panic_hook` builds: an empty `SensitiveContext`
        // (a panic hook has no operation-scoped skill name or project path to
        // pass) and a message quoting this process's own home directory.
        let hook_shaped = RawReport {
            operation: None,
            dimensions: vec![],
            exceptions: vec![RawException {
                message: "panicked at /Users/alice/src/x.rs:12:5: index out of bounds".to_string(),
                frames: vec![],
            }],
        };
        let sanitized = sanitize(&hook_shaped, &SensitiveContext::default());
        assert!(
            !sanitized.exceptions[0].message.contains("/Users/"),
            "a panic-shaped report with no SensitiveContext.home_path set still leaked a home path: {}",
            sanitized.exceptions[0].message
        );
    }

    /// guards: an operation name or a dimension value picked up from a
    /// caller that doesn't know the allow list (a raw error message used
    /// as the operation, or a free-form cause string) reaching Sentry.
    #[test]
    fn sanitizer_drops_operation_names_and_dimensions_outside_the_allow_list() {
        let raw = RawReport {
            operation: Some("skill.totally_unlisted_op".to_string()),
            dimensions: vec![
                Dimension {
                    key: "outcome".to_string(),
                    value: "failure".to_string(),
                },
                Dimension {
                    key: "cause".to_string(),
                    value: "disk is full, path /Users/alice/.claude".to_string(),
                },
                Dimension {
                    key: "not_a_known_key".to_string(),
                    value: "anything".to_string(),
                },
            ],
            exceptions: vec![],
        };

        let sanitized = sanitize(&raw, &SensitiveContext::default());

        assert_eq!(
            sanitized.operation, None,
            "unlisted operation survived: {sanitized:?}"
        );
        assert_eq!(
            sanitized.dimensions,
            vec![Dimension {
                key: "outcome".to_string(),
                value: "failure".to_string()
            }],
            "an out-of-list dimension survived: {:?}",
            sanitized.dimensions
        );
    }

    /// guards: an envelope with an unbounded number of frames, exceptions,
    /// or a huge message reaching the queue and, from there, the network -
    /// each cap gets its own oversized fixture so a bug that only enforces
    /// one of the three still fails this test.
    #[test]
    fn sanitizer_caps_envelope_items_exceptions_and_bytes_at_64_8_and_256kib() {
        let many_frames = RawReport {
            operation: None,
            dimensions: vec![],
            exceptions: vec![RawException {
                message: "panic".to_string(),
                frames: (0..200)
                    .map(|i| RawFrame {
                        absolute_path: Some(format!("/Users/alice/src/file_{i}.rs")),
                        function: format!("f{i}"),
                        instruction_addr: None,
                    })
                    .collect(),
            }],
        };
        let sanitized = sanitize(&many_frames, &SensitiveContext::default());
        assert!(
            item_count(&sanitized) <= MAX_ITEMS,
            "item cap not enforced: {} items",
            item_count(&sanitized)
        );

        let many_exceptions = RawReport {
            operation: None,
            dimensions: vec![],
            exceptions: (0..20)
                .map(|i| RawException {
                    message: format!("exception {i}"),
                    frames: vec![],
                })
                .collect(),
        };
        let sanitized = sanitize(&many_exceptions, &SensitiveContext::default());
        assert!(
            sanitized.exceptions.len() <= MAX_EXCEPTIONS,
            "exception cap not enforced: {} exceptions",
            sanitized.exceptions.len()
        );

        let huge_message = RawReport {
            operation: None,
            dimensions: vec![],
            exceptions: vec![RawException {
                message: "x".repeat(1024 * 1024),
                frames: vec![],
            }],
        };
        let sanitized = sanitize(&huge_message, &SensitiveContext::default());
        let bytes = serde_json::to_vec(&sanitized).expect("serialize");
        assert!(
            bytes.len() <= MAX_BYTES,
            "byte cap not enforced: {} bytes",
            bytes.len()
        );
    }
}
