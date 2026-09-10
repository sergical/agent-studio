//! Printing: the `ResultEnvelope` as JSON, or a short human table.

use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;
use skill_studio_core::dto::{
    Diagnosis, EventDto, FrontmatterRepairPreview, Inventory, RepairOutcome, RestoreOutcome,
    ScanRequest,
};
use skill_studio_core::harness::Capabilities;
use skill_studio_core::ops::ResultEnvelope;

/// Prints one envelope as a single JSON document with a trailing newline.
/// `println!` supplies the newline; the document itself is compact, so a
/// golden diff or a scripted caller sees exactly one line.
pub fn print_json<T: Serialize>(envelope: &ResultEnvelope<T>) {
    println!("{}", serde_json::to_string(envelope).unwrap());
}

fn print_errors(envelope: &ResultEnvelope<impl Serialize>) {
    for error in &envelope.errors {
        let path = error
            .path
            .as_ref()
            .map(|p| format!(" ({p})"))
            .unwrap_or_default();
        eprintln!("{}: {}{path}", error.code.as_str(), error.message);
    }
}

fn print_inventory(inventory: &Inventory) {
    if inventory.skills.is_empty() {
        println!("No skills found.");
    }
    for skill in &inventory.skills {
        println!(
            "{}  ({} deployment{})",
            skill.name.0,
            skill.deployments.len(),
            if skill.deployments.len() == 1 {
                ""
            } else {
                "s"
            }
        );
        for deployment in &skill.deployments {
            println!("  - {}", deployment.path.display());
        }
    }
    for observation in &inventory.observations {
        println!("note: {}", observation.message);
    }
}

/// Prints `scan`'s table: one line per skill, its deployment paths.
pub fn print_scan_table(envelope: &ResultEnvelope<Inventory>) {
    print_errors(envelope);
    if let Some(inventory) = &envelope.data {
        print_inventory(inventory);
    }
}

/// Prints `diagnose`'s table: the inventory, then one line per issue.
pub fn print_diagnose_table(envelope: &ResultEnvelope<Diagnosis>) {
    print_errors(envelope);
    let Some(diagnosis) = &envelope.data else {
        return;
    };
    print_inventory(&diagnosis.inventory);
    if diagnosis.issues.is_empty() {
        println!("No issues.");
        return;
    }
    println!("Issues:");
    for issue in &diagnosis.issues {
        println!(
            "  [{:?}] {:?} {}: {}",
            issue.severity, issue.kind, issue.skill.0, issue.message
        );
    }
}

/// Prints `capabilities`'s table: one line per harness, one per tool.
pub fn print_capabilities_table(envelope: &ResultEnvelope<Capabilities>) {
    print_errors(envelope);
    let Some(caps) = &envelope.data else {
        return;
    };
    for report in &caps.harnesses {
        let observed = report
            .observed
            .as_ref()
            .map(|o| format!(", config_present={}", o.config_present))
            .unwrap_or_default();
        println!("{}{observed}", report.harness.as_str());
    }
    for tool in &caps.tools {
        let path = tool
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not found".into());
        println!("{}: {path}", tool.name);
    }
}

/// Prints `preview-repair`'s table: the proposal id, the reason, and a diff.
pub fn print_repair_preview_table(envelope: &ResultEnvelope<FrontmatterRepairPreview>) {
    print_errors(envelope);
    let Some(preview) = &envelope.data else {
        return;
    };
    println!("proposal {}: {}", preview.proposal_id.0, preview.reason);
    println!("{}", preview.diff);
}

/// Prints `apply-repair`'s table: one line naming what happened.
pub fn print_repair_outcome_table(envelope: &ResultEnvelope<RepairOutcome>) {
    print_errors(envelope);
    let Some(outcome) = &envelope.data else {
        return;
    };
    match outcome {
        RepairOutcome::Applied {
            event_id,
            deployment_id,
        } => println!(
            "applied to {} (event {})",
            deployment_id.as_str(),
            event_id.0
        ),
        RepairOutcome::AlreadyApplied { deployment_id } => {
            println!(
                "{} already had the proposed content",
                deployment_id.as_str()
            )
        }
    }
}

/// Prints `events`'s table: one line per event, newest first.
pub fn print_events_table(envelope: &ResultEnvelope<Vec<EventDto>>) {
    print_errors(envelope);
    let Some(events) = &envelope.data else {
        return;
    };
    if events.is_empty() {
        println!("No events.");
    }
    for event in events {
        println!(
            "{}  {}  {}  {}  {:?}",
            event.id.0, event.ts, event.kind, event.skill.0, event.drift
        );
    }
}

/// Prints `restore`'s table: which paths were put back.
pub fn print_restore_outcome_table(envelope: &ResultEnvelope<RestoreOutcome>) {
    print_errors(envelope);
    let Some(outcome) = &envelope.data else {
        return;
    };
    println!(
        "reverted {} (restore event {})",
        outcome.reverted_event_id.0, outcome.restore_event_id.0
    );
    for path in &outcome.restored_paths {
        println!("  - {}", path.display());
    }
}

/// Writes one JSON Schema file per request/result DTO the CLI's implemented
/// operations (`scan`, `diagnose`, `capabilities`) use, into `out` (default
/// `crates/skill-studio-core/schema`).
pub fn write_schemas(out: Option<PathBuf>) -> ExitCode {
    let out = out.unwrap_or_else(|| PathBuf::from("crates/skill-studio-core/schema"));
    if let Err(err) = std::fs::create_dir_all(&out) {
        eprintln!("could not create {}: {err}", out.display());
        return ExitCode::from(2);
    }
    type SchemaFn = fn() -> schemars::Schema;
    let schemas: &[(&str, SchemaFn)] = &[
        ("scan_request", || schemars::schema_for!(ScanRequest)),
        ("inventory", || schemars::schema_for!(Inventory)),
        ("diagnosis", || schemars::schema_for!(Diagnosis)),
        ("capabilities_request", || {
            schemars::schema_for!(skill_studio_core::dto::CapabilitiesRequest)
        }),
        ("capabilities", || schemars::schema_for!(Capabilities)),
        ("repair_preview_request", || {
            schemars::schema_for!(skill_studio_core::dto::RepairPreviewRequest)
        }),
        ("frontmatter_repair_preview", || {
            schemars::schema_for!(FrontmatterRepairPreview)
        }),
        ("repair_apply_request", || {
            schemars::schema_for!(skill_studio_core::dto::RepairApplyRequest)
        }),
        ("repair_outcome", || schemars::schema_for!(RepairOutcome)),
        ("list_events_request", || {
            schemars::schema_for!(skill_studio_core::dto::ListEventsRequest)
        }),
        ("event_dto", || schemars::schema_for!(EventDto)),
        ("restore_request", || {
            schemars::schema_for!(skill_studio_core::dto::RestoreRequest)
        }),
        ("restore_outcome", || schemars::schema_for!(RestoreOutcome)),
    ];
    for (name, build) in schemas {
        let schema = build();
        let path = out.join(format!("{name}.schema.json"));
        let text = serde_json::to_string_pretty(&schema).unwrap();
        if let Err(err) = std::fs::write(&path, format!("{text}\n")) {
            eprintln!("could not write {}: {err}", path.display());
            return ExitCode::from(2);
        }
        println!("wrote {}", path.display());
    }
    ExitCode::SUCCESS
}
