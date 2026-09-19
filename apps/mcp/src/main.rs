#![forbid(unsafe_code)]
#![allow(clippy::print_stdout, clippy::print_stderr)]

//! Thin binary entry point: wires stdio and hands off to
//! [`skill_studio_mcp::SkillStudioServer`]. All tool logic lives in `lib.rs`
//! so an integration test can call a tool method directly without spawning
//! a child process.

use rmcp::transport::stdio;
use rmcp::ServiceExt;
use skill_studio_mcp::SkillStudioServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server = SkillStudioServer;
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
