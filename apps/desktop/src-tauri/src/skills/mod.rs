// ============================================================================
// Skills Module
// Skills.sh integration for skill discovery, installation, and management
// ============================================================================

pub mod add_method_defaults;
pub mod agents;
pub mod api;
pub mod codex_skill_config;
pub mod commands;
pub mod dotagents_ledger;
pub mod event_commands;
pub mod event_store;
pub mod frontmatter;
pub mod gh_cli;
pub mod github_skill_listing;
pub mod lock_file;
pub mod opencode_skill_permission;
pub mod plugins;
pub mod project_discovery;
pub mod provenance;
pub mod skill_add;
pub mod skill_agent_runner;
pub mod skill_assembly;
pub mod skill_candidate;
pub mod skill_deployment;
pub mod skill_discovery;
pub mod skill_dto;
pub mod skill_editor;
pub mod skill_fork;
pub mod skill_fork_registry;
pub mod skill_fs;
pub mod skill_harness_disable;
pub mod skill_install_plan;
pub mod skill_invocation;
pub mod skill_invocations;
pub mod skill_lifecycle;
pub mod skill_materialize;
pub mod skill_ownership;
pub mod skill_pack;
pub mod skill_park;
pub mod skill_refresh;
pub mod skill_run_history;
pub mod skill_run_target;
pub mod skill_trial;
pub mod skill_update_check;

pub use agents::*;
pub use commands::*;
pub use github_skill_listing::{GithubSkillEntry, GithubSkillListing};
pub use provenance::SourceKind;
pub use skill_dto::*;
pub use skill_refresh::{SkillRefreshState, SkillSnapshot};
