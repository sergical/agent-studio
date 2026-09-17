pub mod skill_agents;
pub mod skill_assembly;
pub mod skill_candidate;
pub mod skill_coordination;
pub mod skill_deployment;
pub mod skill_diagnosis;
pub mod skill_discovery;
pub mod skill_document;
pub mod skill_document_target;
pub mod skill_document_write;
pub mod skill_dotagents_ledger;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_fork_creation;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_fork_pull;
pub mod skill_fork_registry;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_fork_transition;
pub mod skill_frontmatter_repair;
pub mod skill_inventory;
pub mod skill_ledger_inventory;
pub mod skill_lock_file;
pub mod skill_ownership;
pub mod skill_plugins;
pub mod skill_project_lock;
pub mod skill_provenance;
pub mod skill_read;
pub mod skill_reconciliation;
pub mod skill_registry_projection;
pub mod skill_scope;
pub mod skill_service;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_skills_sh_fork_creation;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_skills_sh_lock_transition;

#[cfg(all(unix, feature = "event-store"))]
pub mod skill_backup_copy;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_backup_reservation;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_backup_source;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_copy_registry_removal;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_copy_removal;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_copy_trial_expiry;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_event;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_event_binding;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_event_operations;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_event_schema;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_event_statements;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_event_store;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_event_worker_protocol;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_fork_removal;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_history;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_tree_exchange;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_tree_move;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_trial_restore;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_trial_restore_event;

#[cfg(all(unix, feature = "event-store"))]
pub mod skill_copy_repair;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_repair_backup;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_repair_intent;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_repair_recovery_event;

#[cfg(all(unix, feature = "event-store"))]
pub mod skill_repair_execution;

#[cfg(all(any(target_os = "macos", target_os = "linux"), feature = "event-store"))]
pub mod skill_repair_worker;

#[cfg(all(any(target_os = "macos", target_os = "linux"), feature = "event-store"))]
pub mod skill_direct_restore;

#[cfg(all(unix, feature = "event-store"))]
pub mod skill_unfork_preparation;

#[cfg(all(unix, feature = "event-store"))]
pub mod skill_backup_manifest;

#[cfg(all(unix, feature = "event-store"))]
pub mod skill_copy_document_edit;

pub mod skill_copy_move;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_copy_move_intent;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_copy_visibility;

pub mod skill_invocation_edit;

#[cfg(all(unix, feature = "event-store"))]
mod skill_skills_sh_copy;

#[cfg(unix)]
pub mod skill_history_state;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod skill_history_worker_bootstrap;
pub mod skill_history_worker_frame;
#[cfg(unix)]
pub mod skill_history_worker_process;
#[cfg(all(unix, feature = "event-store"))]
mod skill_worker_socket;

#[cfg(all(target_os = "macos", feature = "event-store"))]
pub mod skill_event_native;
#[cfg(all(unix, feature = "event-store"))]
pub mod skill_event_worker_cleanup;
#[cfg(all(unix, feature = "event-store"))]
mod skill_event_worker_dispatch;
#[cfg(all(target_os = "macos", feature = "event-store"))]
pub mod skill_event_worker_entry;
#[cfg(all(any(target_os = "macos", target_os = "linux"), feature = "event-store"))]
pub mod skill_event_worker_exchange;

#[cfg(all(unix, feature = "event-store"))]
pub mod skill_event_files;

#[cfg(all(test, unix, feature = "event-store"))]
mod skill_event_file_authority;
