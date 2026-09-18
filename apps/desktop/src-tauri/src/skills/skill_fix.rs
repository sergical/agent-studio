// ============================================================================
// Skills Module - skill_fix
// "Fix" runs the doctor invariants in `docs/action-map/lifecycle-states.md`
// for one skill by name and reports anything it repaired plus anything it
// couldn't; a conflict (two differing copies of the same skill) is reported,
// never merged. `fix_skill` is a thin adapter over
// `skill_studio_core::ops::fix_skill`, the same function the CLI's `fix`
// subcommand and the MCP server's `fix` tool call, so all three surfaces
// leave the same disk state.
// ============================================================================

use skill_studio_core::dto::{FixSkillOutcome, FixSkillRequest};
use skill_studio_core::identity::{CorrelationId, SkillName};
use skill_studio_core::ops::{self, Operation, ResultEnvelope};
use skill_studio_core::ports::OpContext;
use tauri::Manager;

use super::skill_refresh::{self, SkillRefreshState, SkillSnapshot};

#[tauri::command]
pub async fn fix_skill(skill: String, app: tauri::AppHandle) -> Result<FixSkillOutcome, String> {
    crate::timing_log::time_command_blocking(&app, "fix_skill", move || {
        let rt = super::core_runtime::build_runtime_write()?;
        let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
        let result = ops::fix_skill(
            &rt,
            &ctx,
            &FixSkillRequest {
                skill: SkillName(skill),
            },
        );
        let envelope = ResultEnvelope::from_result(Operation::FixSkill, &rt.scope, &ctx, result);
        super::core_runtime::to_command_result(envelope)
    })
    .await
}

/// The check `open_conflict_paths` applies to each path before asking the
/// editor to open it - the same containment `require_snapshot_owns_path`
/// (`commands.rs`) wraps around a live `tauri::State`, pulled out here as a
/// plain function of a snapshot so it's testable without one.
fn require_conflict_path_is_owned(
    snapshot: Option<&SkillSnapshot>,
    path: &std::path::Path,
) -> Result<(), String> {
    match snapshot {
        Some(snapshot) if skill_refresh::snapshot_owns_path(snapshot, path) => Ok(()),
        _ => Err(format!(
            "Path is not an installed skill: {}",
            path.display()
        )),
    }
}

/// Opens a conflict's two differing paths side by side in the user's chosen
/// editor. Writes nothing to either path itself; the caller already has both
/// paths from a `ConflictSummary` in a `FixSkillOutcome`. Each path must
/// belong to an installed skill in the current snapshot - the same guard
/// `open_skill_path` (`commands.rs`) applies - so the webview can't send an
/// arbitrary path for us to open.
#[tauri::command]
pub async fn open_conflict_paths(paths: Vec<String>, app: tauri::AppHandle) -> Result<(), String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "open_conflict_paths", move || {
        let refresh_state = app.state::<SkillRefreshState>();
        let snapshot = refresh_state.snapshot.read().ok().and_then(|g| g.clone());
        let paths: Vec<std::path::PathBuf> =
            paths.into_iter().map(std::path::PathBuf::from).collect();
        for path in &paths {
            require_conflict_path_is_owned(snapshot.as_ref(), path)?;
        }
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        super::skill_editor::open_paths_in_editor(&home, &paths)
    })
    .await
}

#[cfg(test)]
mod tests {
    use skill_studio_core::testing::golden::ctx;

    use super::*;
    use crate::skills::core_runtime::build_runtime_write_at;

    /// Row F1 (unit 3.7b review round 1): a skill whose only issue is
    /// invalid YAML frontmatter is the one issue `ops::fix_skill` actually
    /// repairs, so the row's "Fix" action must reach it. This proves the
    /// repair through the same runtime construction `fix_skill` (the Tauri
    /// command above) uses, not just `ops::fix_skill` in isolation.
    #[test]
    fn fix_skill_on_invalid_yaml_returns_a_repaired_outcome_through_the_desktop_adapter() {
        let home = tempfile::tempdir().expect("temp home");
        let skills_dir = home.path().join(".claude/skills/bad-yaml");
        std::fs::create_dir_all(&skills_dir).expect("skill dir");
        std::fs::write(
            skills_dir.join("SKILL.md"),
            b"---\nname: bad-yaml\ndescription: Use this: when needed\n---\nBody.\n",
        )
        .expect("write SKILL.md");

        let rt = build_runtime_write_at(home.path(), &home.path().join(".skill-studio"))
            .expect("desktop runtime");
        let outcome = ops::fix_skill(
            &rt,
            &ctx(),
            &FixSkillRequest {
                skill: SkillName("bad-yaml".to_string()),
            },
        )
        .expect("fix_skill");

        // `derive_issues` also emits a `SpecViolation` issue for every
        // `spec_violations` entry regardless of whether a `RepairableFrontmatter`
        // issue repairs the same root cause, so the pre-repair YAML message
        // still appears in `unrepaired` too - that duplication is
        // pre-existing and out of scope here; what this test pins is that
        // the repair itself lands (`applied`) and is on disk.
        assert_eq!(outcome.applied.len(), 1, "{outcome:?}");
        let repaired = std::fs::read_to_string(skills_dir.join("SKILL.md")).expect("read back");
        assert!(repaired.contains("description: |-"), "{repaired}");
    }

    /// Row F5 (unit 3.7b review round 1): `open_conflict_paths` must not open
    /// whatever path the webview sends - only a path inside an installed
    /// skill's own snapshot, the same containment `open_skill_path`
    /// (`commands.rs`) applies. Proves the refusal at the pure-helper level
    /// `open_conflict_paths` itself delegates to.
    #[test]
    fn open_conflict_paths_refuses_a_path_outside_the_snapshot() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dep_dir = tmp.path().join("foo");
        std::fs::create_dir_all(&dep_dir).expect("dep dir");
        let outside = tmp.path().join("outside.md");
        std::fs::write(&outside, "body").expect("write outside");

        let snapshot = crate::skills::test_support::fixture_snapshot_owning(&dep_dir);

        assert!(require_conflict_path_is_owned(Some(&snapshot), &outside).is_err());
        assert!(require_conflict_path_is_owned(None, &outside).is_err());
    }
}
