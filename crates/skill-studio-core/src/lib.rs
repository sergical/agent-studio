//! Skill Studio core.
//!
//! This crate holds the rules that every Skill Studio surface shares: the
//! desktop app, the command line, and the local MCP server. It knows nothing
//! about Tauri, terminals, or sockets. Adapters give it a [`scope::RuntimeScope`]
//! and a set of [`ports::Ports`]; the core returns typed data or a typed error.
//!
//! Invariants that hold for the whole crate:
//!
//! - The core never reads the `HOME` environment variable and never resolves
//!   the user's home directory itself. Every operation takes an explicit
//!   [`scope::RuntimeScope`].
//! - Every public data transfer type derives `serde` and `schemars::JsonSchema`
//!   so TypeScript types and MCP tool schemas are generated from Rust.
//! - Every error carries an [`error::ErrorCode`] with a stable string form and
//!   a fixed exit status.
//! - Harness capability facts live in [`harness::HarnessCatalog`] data, not in
//!   adapter code.
//!
//! See `docs/spec-core-primitives.md` for the design and the migration order.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod dto;
pub mod error;
pub mod events;
pub mod frontmatter;
pub mod frontmatter_repair;
pub mod harness;
pub mod identity;
pub mod lock_file;
pub mod ops;
mod ownership;
pub mod ports;
pub mod scope;
pub mod snapshot;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

/// Version of the wire contract (envelope, DTOs, MCP tool schemas).
///
/// Invariant: this number increases only when a field is removed or its
/// meaning changes. Adding an optional field does not change it.
pub const SCHEMA_VERSION: u32 = 1;

pub use error::{CoreError, ErrorCode, ErrorEntry};
pub use ops::{OpStatus, Outcome, ResultEnvelope};
pub use ports::{OpContext, Ports, Runtime};
pub use scope::{NormalizedScope, RuntimeScope, ScopeId};

#[cfg(test)]
mod home_free_tests {
    //! Guards the crate-wide invariant documented above: nothing under
    //! `src/` may read `HOME` or call into the `dirs` crate. Every path this
    //! crate touches must come from an explicit [`scope::RuntimeScope`].
    use std::fs;
    use std::path::Path;

    fn walk(dir: &Path, out: &mut Vec<(std::path::PathBuf, String)>) {
        for entry in fs::read_dir(dir).expect("read_dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let content = fs::read_to_string(&path).expect("read source file");
                out.push((path, content));
            }
        }
    }

    #[test]
    fn crate_never_touches_home_env_or_dirs_crate() {
        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&src_dir, &mut files);
        assert!(
            !files.is_empty(),
            "expected to find source files under {src_dir:?}"
        );

        // Built from fragments so this test's own source doesn't trip the
        // scan it performs on every other file.
        let forbidden = [
            ["dir", "s::"].concat(),
            ["home", "_dir"].concat(),
            ["std::env::var(\"", "HOME", "\")"].concat(),
        ];
        for (path, content) in &files {
            for needle in &forbidden {
                assert!(
                    !content.contains(needle),
                    "{path:?} contains forbidden token `{needle}`; the core must never read HOME directly"
                );
            }
        }
    }
}
