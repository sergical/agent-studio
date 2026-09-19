// ============================================================================
// Skills Module
// Skills.sh integration for skill discovery, installation, and management
// ============================================================================

pub mod add_method_defaults;
pub mod agents;
pub mod api;
pub mod app_version;
pub mod commands;
pub mod core_content_hash;
pub mod core_runtime;
pub mod data_folder_status;
pub mod error_reporting;
pub mod event_commands;
pub mod event_store;
pub mod frontmatter;
pub mod gh_cli;
pub mod github_skill_listing;
pub mod harness_first_run;
pub mod skill_add_operation;
pub mod skill_agent_runner;
pub mod skill_assembly;
pub mod skill_deployment;
pub mod skill_doctor;
pub mod skill_dto;
pub mod skill_editor;
pub mod skill_fix;
pub mod skill_fork;
pub mod skill_fork_registry;
pub mod skill_frontmatter_repair;
pub mod skill_fs;
pub mod skill_harness_disable;
pub mod skill_independent_copy;
pub mod skill_install;
pub mod skill_invocation;
pub mod skill_lifecycle;
pub mod skill_materialize;
pub mod skill_md_write;
pub mod skill_ownership;
pub mod skill_pack;
pub mod skill_park;
pub mod skill_plugin_lifecycle;
pub mod skill_process;
pub mod skill_project_folders;
pub mod skill_refresh;
pub mod skill_run_history;
pub mod skill_run_target;
pub mod skill_trial;
pub mod skill_trust_policy;
pub mod skill_update_check;
#[cfg(any(test, feature = "testing"))]
pub mod test_support;
pub mod write_lease;

pub use agents::*;
pub use commands::*;
pub use github_skill_listing::{GithubSkillEntry, GithubSkillListing};
pub use skill_dto::*;
pub use skill_refresh::{SkillRefreshState, SkillSnapshot};
pub use skill_studio_core::identity::SourceKind;
