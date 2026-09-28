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
    // Consent lives on the real machine, same as the CLI's own startup
    // resolution in `apps/cli/src/main.rs`; there is no scope flag here to
    // point it anywhere else.
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
    let registry_telemetry_enabled = skill_studio_host::telemetry::consent_from_registry(&home);
    let consent =
        skill_studio_host::telemetry::Consent::new(skill_studio_host::telemetry::resolve_consent(
            std::env::var("SKILL_STUDIO_TELEMETRY").ok(),
            registry_telemetry_enabled,
        ));
    let _telemetry_guard = skill_studio_host::telemetry::init(
        skill_studio_host::telemetry::Surface::Mcp,
        env!("CARGO_PKG_VERSION"),
        consent,
    );

    let server = SkillStudioServer;
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
