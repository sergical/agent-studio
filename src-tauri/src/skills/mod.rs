// ============================================================================
// Skills Module
// Skills.sh integration for skill discovery, installation, and management
// ============================================================================

pub mod agents;
pub mod api;
pub mod commands;
pub mod frontmatter;
pub mod lock_file;
pub mod plugins;
pub mod project_discovery;
pub mod provenance;
pub mod skill_agent_runner;
pub mod skill_assembly;
pub mod skill_candidate;
pub mod skill_discovery;
pub mod skill_dto;
pub mod skill_invocations;
pub mod skill_refresh;
pub mod skill_run_history;
pub mod skill_run_target;

pub use agents::*;
pub use commands::*;
pub use provenance::SourceKind;
pub use skill_dto::*;
pub use skill_refresh::{SkillRefreshState, SkillSnapshot};
