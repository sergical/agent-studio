// ============================================================================
// Skills Module
// Skills.sh integration for skill discovery, installation, and management
// ============================================================================

pub mod add_method_defaults;
pub use skill_studio_core::skill_agents as agents;
pub mod api;
pub mod codex_skill_config;
pub mod commands;
pub mod dotagents_ledger;
pub mod event_commands;
pub mod event_store;
pub use skill_studio_core::skill_document as frontmatter;
pub mod gh_cli;
pub mod github_skill_listing;
pub use skill_studio_core::skill_lock_file as lock_file;
pub mod opencode_skill_permission;
pub mod plugins;
pub mod project_discovery;
pub mod provenance;
pub mod skill_add;
pub mod skill_add_operation;
pub mod skill_agent_runner;
pub mod skill_assembly;
pub mod skill_candidate;
pub(crate) mod skill_copy_recovery;
pub mod skill_deployment;
pub mod skill_discovery;
pub mod skill_dto;
pub mod skill_editor;
pub mod skill_fork;
#[cfg(target_os = "macos")]
mod skill_fork_document_history;
pub mod skill_fork_registry;
pub mod skill_frontmatter_repair;
pub mod skill_fs;
pub mod skill_harness_disable;
pub mod skill_independent_copy;
pub mod skill_install_plan;
pub mod skill_invocation;
pub mod skill_invocations;
pub mod skill_lifecycle;
pub mod skill_materialize;
pub mod skill_md_write;
#[cfg(all(target_os = "macos", any(test, feature = "native-fork-repair")))]
mod skill_native_fork;
#[cfg(target_os = "macos")]
mod skill_native_unfork;
pub mod skill_ownership;
pub mod skill_pack;
pub mod skill_park;
pub mod skill_process;
pub(crate) mod skill_project_authority;
pub mod skill_refresh;
pub mod skill_run_history;
pub mod skill_run_target;
pub(crate) mod skill_scope_config;
pub mod skill_trial;
pub mod skill_trust_policy;
#[cfg(target_os = "macos")]
mod skill_unfork_provider;
pub mod skill_update_check;

pub use agents::*;
pub use commands::*;
pub use github_skill_listing::{GithubSkillEntry, GithubSkillListing};
pub use provenance::SourceKind;
pub use skill_dto::*;
pub use skill_refresh::{SkillRefreshState, SkillSnapshot};

pub(crate) mod skill_copy_repair;
pub(crate) mod skill_document_operation;
pub(crate) mod skill_startup_recovery;

#[cfg(target_os = "macos")]
pub(crate) mod skill_document_save;

#[cfg(target_os = "macos")]
mod skill_skills_sh_unfork;
