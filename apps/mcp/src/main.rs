#![forbid(unsafe_code)]

//! Thin binary entry point over [`skill_studio_mcp::run_stdio`]. All tool
//! logic lives in `lib.rs` so an integration test can call a tool method
//! directly without spawning a child process, and so the `skill-studio mcp`
//! subcommand runs the same server.

fn main() -> anyhow::Result<()> {
    skill_studio_mcp::run_stdio()
}
