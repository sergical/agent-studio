//! Printing: the `ResultEnvelope` as JSON, or a short human table.

use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;
use skill_studio_core::doctor::DoctorInvariant;
use skill_studio_core::dto::{
    CommandHealth, ConflictReport, Diagnosis, DoctorReport, EventDto, FixApplied, FixSkillOutcome,
    FrontmatterRepairPreview, InstallHarnessResult, InstallOutcome, InstallPreferences, Inventory,
    IssueKind, ParkOutcome, RemoveOutcome, RepairOutcome, RestoreOutcome, ScanRequest,
    SetHarnessEnabledOutcome, Severity, UnparkOutcome, UpdateAllOutcome, UpdateOutcome,
};
use skill_studio_core::harness::{Capabilities, HarnessReport};
use skill_studio_core::ops::ResultEnvelope;
use skill_studio_core::skill_update_check::{Currency, OutdatedRecord};
use skill_studio_host::{SkillUsageRow, UsageReport};
use std::collections::BTreeMap;

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
            "{}  ({} {})",
            skill.name.0,
            skill.deployments.len(),
            if skill.deployments.len() == 1 {
                "copy"
            } else {
                "copies"
            }
        );
        // The id is what `park`, `unpark` and `remove --id` take when a
        // name matches more than one copy.
        for deployment in &skill.deployments {
            println!(
                "  - {}  id {}",
                deployment.path.display(),
                deployment.id.as_str()
            );
        }
    }
    for observation in &inventory.observations {
        println!("note: {}", observation.message);
    }
}

/// Prints `scan`'s table: one line per skill, then each copy's path and id.
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
            "  [{}] {}, {}: {}",
            severity_word(issue.severity),
            issue_kind_words(issue.kind),
            issue.skill.0,
            issue.message
        );
    }
}

fn severity_word(severity: Severity) -> &'static str {
    match severity {
        Severity::Off => "note",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

fn issue_kind_words(kind: IssueKind) -> &'static str {
    match kind {
        IssueKind::BrokenLink => "broken link",
        IssueKind::UnreadableLink => "link cannot be read",
        IssueKind::SpecViolation => "does not follow the skill format",
        IssueKind::RepairableFrontmatter => "header can be fixed",
        IssueKind::Drift => "copies differ",
        IssueKind::Duplicate => "installed twice",
        IssueKind::Parked => "parked",
        IssueKind::Disabled => "turned off",
        IssueKind::RootUnreadable => "folder not read in time",
    }
}

fn doctor_check_words(invariant: DoctorInvariant) -> &'static str {
    match invariant {
        DoctorInvariant::LinkResolvesInRoot => "link points outside its folder",
        DoctorInvariant::RegistryEntryHasFolder => "registry entry has no folder",
        DoctorInvariant::LockfileEntryHasFolder => "lock file entry has no folder",
        DoctorInvariant::NoFolderInTwoStates => "folder is in two states at once",
        DoctorInvariant::QuarantineWithinCap => "recovery folder is over its limit",
        DoctorInvariant::JournalHasNoOpenPlan => "history has an unfinished change",
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

/// Prints `doctor`'s table: every violation found, one line each, or a
/// healthy-home confirmation naming how many skills were checked.
pub fn print_doctor_report_table(envelope: &ResultEnvelope<DoctorReport>) {
    print_errors(envelope);
    let Some(report) = &envelope.data else {
        return;
    };
    if report.violations.is_empty() {
        println!("No problems found ({} skills checked).", report.checked);
        return;
    }
    for violation in &report.violations {
        println!(
            "{}: {} ({})",
            doctor_check_words(violation.invariant),
            violation.path.display(),
            violation.detail
        );
    }
}

/// Prints `usage`'s table: skills not used in the window first, then the
/// used ones, then a count of the unused ones.
pub fn print_usage_table(envelope: &ResultEnvelope<UsageReport>) {
    print_errors(envelope);
    let Some(report) = &envelope.data else {
        return;
    };
    let (unused, used): (Vec<_>, Vec<_>) = report.rows.iter().partition(|row| row.recent_uses == 0);
    let width = report
        .rows
        .iter()
        .map(|row| row.skill.len())
        .max()
        .unwrap_or(0);
    if !unused.is_empty() {
        println!("Not used in the last {} days:", report.days);
        for row in &unused {
            println!("  {:width$}  last used {}", row.skill, last_used(row));
        }
    }
    if !used.is_empty() {
        if !unused.is_empty() {
            println!();
        }
        println!("Used in the last {} days:", report.days);
        for row in &used {
            let agents = row
                .agents
                .iter()
                .map(|(agent, uses)| format!("{agent} {uses}"))
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "  {:width$}  {:>4} {}  last used {}  ({agents})",
                row.skill,
                row.recent_uses,
                if row.recent_uses == 1 { "use " } else { "uses" },
                last_used(row),
            );
        }
    }
    if !report.rows.is_empty() {
        println!();
    }
    println!(
        "{} of {} skills not used in {} days",
        report.unused.len(),
        report.rows.len(),
        report.days
    );
    if report.partial {
        println!("note: some session history could not be read, so some counts may be low.");
    }
}

/// The date part of a row's last use, or "never".
fn last_used(row: &SkillUsageRow) -> &str {
    row.last_used
        .as_deref()
        .map_or("never", |at| at.get(..10).unwrap_or(at))
}

/// Prints `install`'s table: what got installed, or the trust prompt.
pub fn print_install_outcome_table(envelope: &ResultEnvelope<InstallOutcome>) {
    print_errors(envelope);
    let Some(outcome) = &envelope.data else {
        return;
    };
    match outcome {
        InstallOutcome::Installed {
            skill,
            deployment_path,
            harness_results,
            ..
        } => {
            println!("installed {} at {}", skill.0, deployment_path.display());
            for result in harness_results {
                println!("{}", install_harness_line(result));
            }
        }
        InstallOutcome::NeedsTrust { identity } => {
            println!("needs trust: {identity} (retry with --trust to confirm)");
        }
    }
}

/// One line per harness in `add`'s table, e.g. `linked pi at <path>`.
fn install_harness_line(result: &InstallHarnessResult) -> String {
    match result {
        InstallHarnessResult::ReadsShared { harness, path } => {
            format!("{} reads {}", harness.as_str(), path.display())
        }
        InstallHarnessResult::Linked { harness, path } => {
            format!("linked {} at {}", harness.as_str(), path.display())
        }
        InstallHarnessResult::Copied {
            harness,
            path,
            link_failed,
        } => {
            let why = if *link_failed {
                " (the link failed)"
            } else {
                ""
            };
            format!("copied {} to {}{why}", harness.as_str(), path.display())
        }
        InstallHarnessResult::Skipped { harness, reason } => {
            format!("skipped {}: {reason}", harness.as_str())
        }
    }
}

/// Prints `install-preferences`'s table: the method and harnesses the next
/// `add` pre-selects, and whether they were saved by an earlier install or
/// derived from the environment.
pub fn print_install_preferences_table(envelope: &ResultEnvelope<InstallPreferences>) {
    print_errors(envelope);
    let Some(preferences) = &envelope.data else {
        return;
    };
    let harnesses = preferences
        .harnesses
        .iter()
        .map(skill_studio_core::identity::AgentId::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "method={:?}  harnesses={}  source={}",
        preferences.method,
        if harnesses.is_empty() {
            "-"
        } else {
            &harnesses
        },
        if preferences.saved {
            "saved"
        } else {
            "default"
        },
    );
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
        // A sweep-only row (e.g. `quarantine_prune`) has no skill of its
        // own; print a placeholder instead of leaving the column blank.
        let skill = if event.skill.0.is_empty() {
            "-"
        } else {
            &event.skill.0
        };
        println!(
            "{}  {}  {}  {}  {:?}",
            event.id.0, event.ts, event.kind, skill, event.drift
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
            "Removed {}. A copy is in the recovery folder: {}. Run `undo` to bring it back.",
            outcome.skill.0,
            path.display()
        ),
        None => println!("Removed {}.", outcome.skill.0),
    }
}

/// Prints `park`'s table: the deployment and where its directory now lives.
pub fn print_park_outcome_table(envelope: &ResultEnvelope<ParkOutcome>) {
    print_errors(envelope);
    let Some(outcome) = &envelope.data else {
        return;
    };
    println!(
        "Parked. The skill is now in {}. Run `unpark` to turn it on again.",
        outcome.parked_path.display()
    );
}

/// Prints `split`'s table: one line per copy, then the update note.
pub fn print_split_outcome_table(envelope: &ResultEnvelope<skill_studio_core::dto::SplitOutcome>) {
    print_errors(envelope);
    let Some(outcome) = &envelope.data else {
        return;
    };
    for copy in &outcome.copies {
        println!(
            "{} {} -> {}",
            outcome.skill.0,
            copy.harness.as_str(),
            copy.path.display()
        );
    }
    println!("{}", outcome.update_note);
}

/// Prints `unpark`'s table: the deployment and where its directory now lives.
pub fn print_unpark_outcome_table(envelope: &ResultEnvelope<UnparkOutcome>) {
    print_errors(envelope);
    let Some(outcome) = &envelope.data else {
        return;
    };
    println!(
        "Turned on again. The skill is back in {}.",
        outcome.restored_path.display()
    );
}

/// Prints `outdated`'s table: one `NAME CURRENCY LATEST_SHA` row per skill,
/// sorted by name (`outdated`'s result is already a `BTreeMap`, so this is
/// free). `LATEST_SHA` is the first 7 characters of `latest_commit` - a
/// dotagents commit SHA or a skills.sh tree SHA - or `-` when the check
/// never resolved one.
pub fn print_outdated_table(envelope: &ResultEnvelope<BTreeMap<String, OutdatedRecord>>) {
    print_errors(envelope);
    let Some(outcome) = &envelope.data else {
        return;
    };
    for (name, record) in outcome {
        let label = match record.currency {
            Currency::UpToDate => "up_to_date",
            Currency::UpdateAvailable => "update_available",
            Currency::NotTracked => "not_tracked",
            Currency::Unknown => "unknown",
        };
        let latest_sha = record
            .latest_commit
            .as_deref()
            .map_or("-".to_string(), |sha| sha.chars().take(7).collect());
        println!("{name}\t{label}\t{latest_sha}");
    }
}

/// Prints `sweep_quarantine`'s table: it has no outcome payload, so a
/// success just confirms the sweep ran; errors already went to stderr via
/// `print_errors`.
pub fn print_sweep_quarantine_table(envelope: &ResultEnvelope<()>) {
    print_errors(envelope);
    if envelope.data.is_some() {
        println!("quarantine swept");
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
        ("install_request", || {
            schemars::schema_for!(skill_studio_core::dto::InstallRequest)
        }),
        ("install_outcome", || schemars::schema_for!(InstallOutcome)),
        ("install_preferences_request", || {
            schemars::schema_for!(skill_studio_core::dto::InstallPreferencesRequest)
        }),
        ("install_preferences", || {
            schemars::schema_for!(skill_studio_core::dto::InstallPreferences)
        }),
        ("doctor_request", || {
            schemars::schema_for!(skill_studio_core::dto::DoctorRequest)
        }),
        ("doctor_report", || schemars::schema_for!(DoctorReport)),
        ("park_request", || {
            schemars::schema_for!(skill_studio_core::dto::ParkRequest)
        }),
        ("park_outcome", || {
            schemars::schema_for!(skill_studio_core::dto::ParkOutcome)
        }),
        ("split_request", || {
            schemars::schema_for!(skill_studio_core::dto::SplitRequest)
        }),
        ("split_outcome", || {
            schemars::schema_for!(skill_studio_core::dto::SplitOutcome)
        }),
        ("unpark_request", || {
            schemars::schema_for!(skill_studio_core::dto::UnparkRequest)
        }),
        ("unpark_outcome", || {
            schemars::schema_for!(skill_studio_core::dto::UnparkOutcome)
        }),
        ("set_harness_enabled_request", || {
            schemars::schema_for!(skill_studio_core::dto::SetHarnessEnabledRequest)
        }),
        ("set_harness_enabled_outcome", || {
            schemars::schema_for!(skill_studio_core::dto::SetHarnessEnabledOutcome)
        }),
        (
            "outdated_result",
            || schemars::schema_for!(BTreeMap<String, OutdatedRecord>),
        ),
        ("sweep_quarantine_request", || {
            schemars::schema_for!(skill_studio_core::dto::SweepQuarantineRequest)
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
