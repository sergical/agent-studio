//! Printing: the `ResultEnvelope` as JSON, or a short human table.

use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;
use skill_studio_core::dto::{
    CommandHealth, ConflictReport, Diagnosis, EventDto, FixApplied, FixSkillOutcome,
    FrontmatterRepairPreview, Inventory, RemoveOutcome, RepairOutcome, RestoreOutcome, ScanRequest,
    SetHarnessEnabledOutcome, UpdateAllOutcome, UpdateOutcome,
};
use skill_studio_core::harness::{Capabilities, HarnessReport};
use skill_studio_core::ops::ResultEnvelope;

/// Prints one envelope as a single JSON document with a trailing newline.
/// `println!` supplies the newline; the document itself is compact, so a
/// golden diff or a scripted caller sees exactly one line.
pub fn print_json<T: Serialize>(envelope: &ResultEnvelope<T>) {
    // Every field on a `ResultEnvelope` is one of our own DTOs; the only way
    // `to_string` errs is a non-string map key or a NaN/infinite float,
    // neither of which this envelope ever holds. If it ever does, stdout must
    // stay empty rather than carry a document that is not a `ResultEnvelope`,
    // and the process must not exit as if the command succeeded: EX_SOFTWARE.
    match serde_json::to_string(envelope) {
        Ok(line) => println!("{line}"),
        Err(err) => {
            eprintln!("failed to serialize the result envelope: {err}");
            std::process::exit(70);
        }
    }
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
            .map_or_else(|| "not found".into(), |p| p.display().to_string());
        println!("{}: {path}", tool.name);
    }
}

/// Prints `harnesses`'s table: one line per first-class harness, naming
/// detected state, version, install method, and the evidence behind each.
/// A fact the probe could not prove prints the literal word `Unknown`
/// rather than an empty field.
pub fn print_harnesses_table(envelope: &ResultEnvelope<HarnessReport>) {
    print_errors(envelope);
    let Some(report) = &envelope.data else {
        return;
    };
    for row in &report.harnesses {
        let executable = row
            .executable
            .as_ref()
            .map_or_else(|| "not found".into(), |p| p.display().to_string());
        let version = row.version.value.as_deref().unwrap_or("Unknown");
        let install_method = row.install_method.value.as_deref().unwrap_or("Unknown");
        println!(
            "{}  [{:?}]  executable={executable}  version={version} ({})  install_method={install_method} ({})",
            row.display_name,
            row.state,
            row.version.evidence.source,
            row.install_method.evidence.source,
        );
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
            );
        }
    }
}

/// Prints `fix`'s table: one line per applied repair, then one line per
/// issue it could not repair, then one line per conflict it found (fix never
/// writes into a conflict; it only names both paths).
pub fn print_fix_outcome_table(envelope: &ResultEnvelope<FixSkillOutcome>) {
    print_errors(envelope);
    let Some(outcome) = &envelope.data else {
        return;
    };
    for applied in &outcome.applied {
        let FixApplied::FrontmatterRepair {
            deployment_id,
            event_id,
        } = applied;
        println!("repaired {} (event {})", deployment_id.as_str(), event_id.0);
    }
    for issue in &outcome.unrepaired {
        println!(
            "could not repair {}: {}",
            issue.path.display(),
            issue.message
        );
    }
    for conflict in &outcome.conflicts {
        println!(
            "conflict: {} vs {} ({})",
            conflict.path_a.display(),
            conflict.path_b.display(),
            conflict.message
        );
    }
    if outcome.applied.is_empty() && outcome.unrepaired.is_empty() && outcome.conflicts.is_empty() {
        println!("{} had nothing to fix", outcome.skill.0);
    }
}

/// Prints `conflicts`'s table: one line per differing copy pair, naming
/// both paths.
pub fn print_conflict_report_table(envelope: &ResultEnvelope<ConflictReport>) {
    print_errors(envelope);
    let Some(report) = &envelope.data else {
        return;
    };
    if report.conflicts.is_empty() {
        println!("no conflicts");
        return;
    }
    for conflict in &report.conflicts {
        println!(
            "{}: {} vs {} ({})",
            conflict.skill.0,
            conflict.path_a.display(),
            conflict.path_b.display(),
            conflict.message
        );
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

/// Prints `update`'s table: the deployment refreshed and its tree hash
/// before and after.
pub fn print_update_outcome_table(envelope: &ResultEnvelope<UpdateOutcome>) {
    print_errors(envelope);
    let Some(outcome) = &envelope.data else {
        return;
    };
    println!(
        "updated {} (event {}): {} -> {}",
        outcome.skill.0, outcome.event_id.0, outcome.tree_hash_before, outcome.tree_hash_after
    );
}

/// Prints `update`'s batch table: one line per skill, `outcome` when it
/// succeeded, the matching `errors` entry when it did not.
pub fn print_update_all_outcome_table(envelope: &ResultEnvelope<UpdateAllOutcome>) {
    print_errors(envelope);
    let Some(outcome) = &envelope.data else {
        return;
    };
    for item in &outcome.items {
        if let Some(o) = &item.outcome {
            println!(
                "updated {} (event {}): {} -> {}",
                item.skill.0, o.event_id.0, o.tree_hash_before, o.tree_hash_after
            );
        } else {
            let message = outcome
                .errors
                .get(&item.skill.0)
                .map_or("unknown error", String::as_str);
            println!("failed to update {}: {}", item.skill.0, message);
        }
    }
}

/// Prints `set-harness-enabled`'s table: how many of the harness's paths
/// for this skill were toggled, out of how many it needed to touch.
pub fn print_set_harness_enabled_outcome_table(
    envelope: &ResultEnvelope<SetHarnessEnabledOutcome>,
) {
    print_errors(envelope);
    let Some(outcome) = &envelope.data else {
        return;
    };
    println!(
        "{} on {}: {} of {} path(s) toggled",
        outcome.skill.0,
        outcome.harness.as_str(),
        outcome.toggled,
        outcome.total,
    );
}

/// Prints `remove`'s table: the skill removed and, when the deployment was
/// `Copy`/`Fork` (quarantined rather than deleted), where its bytes landed.
pub fn print_remove_outcome_table(envelope: &ResultEnvelope<RemoveOutcome>) {
    print_errors(envelope);
    let Some(outcome) = &envelope.data else {
        return;
    };
    match &outcome.quarantine_path {
        Some(path) => println!(
            "removed {} -> quarantined at {}",
            outcome.skill.0,
            path.display()
        ),
        None => println!("removed {}", outcome.skill.0),
    }
}

/// Prints `health`'s table: `COMMAND COUNT FAILURES P50_MS P95_MS
/// LAST_ERROR`, one row per command, in the rollup's own (command-name)
/// order.
pub fn print_health_table(rows: &[CommandHealth]) {
    if rows.is_empty() {
        println!("No commands recorded in the last 7 days.");
        return;
    }
    println!("COMMAND COUNT FAILURES P50_MS P95_MS LAST_ERROR");
    for row in rows {
        println!(
            "{} {} {} {} {} {}",
            row.command,
            row.count,
            row.failures,
            row.p50_ms,
            row.p95_ms,
            row.last_error.as_deref().unwrap_or("-"),
        );
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
        ("fix_skill_request", || {
            schemars::schema_for!(skill_studio_core::dto::FixSkillRequest)
        }),
        ("fix_skill_outcome", || {
            schemars::schema_for!(FixSkillOutcome)
        }),
        ("diagnose_conflict_request", || {
            schemars::schema_for!(skill_studio_core::dto::DiagnoseConflictRequest)
        }),
        ("conflict_report", || schemars::schema_for!(ConflictReport)),
        ("remove_request", || {
            schemars::schema_for!(skill_studio_core::dto::RemoveRequest)
        }),
        ("remove_outcome", || schemars::schema_for!(RemoveOutcome)),
        ("update_request", || {
            schemars::schema_for!(skill_studio_core::dto::UpdateRequest)
        }),
        ("update_outcome", || schemars::schema_for!(UpdateOutcome)),
        ("update_all_request", || {
            schemars::schema_for!(skill_studio_core::dto::UpdateAllRequest)
        }),
        ("update_all_outcome", || {
            schemars::schema_for!(UpdateAllOutcome)
        }),
    ];
    for (name, build) in schemas {
        let schema = build();
        let path = out.join(format!("{name}.schema.json"));
        // `schema` is a `schemars::Schema`, which is always representable as
        // JSON; there is no error path this fallback would ever exercise.
        let text = serde_json::to_string_pretty(&schema)
            .unwrap_or_else(|e| format!("{{\"error\":\"failed to serialize the schema: {e}\"}}"));
        if let Err(err) = std::fs::write(&path, format!("{text}\n")) {
            eprintln!("could not write {}: {err}", path.display());
            return ExitCode::from(2);
        }
        println!("wrote {}", path.display());
    }
    ExitCode::SUCCESS
}
