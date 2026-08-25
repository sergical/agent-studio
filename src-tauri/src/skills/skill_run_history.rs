// ============================================================================
// Skills Module - skill_run_history
// Persists the outcome of every Ask/Audit/Test run against a skill, so the
// skill page and list can show "Last test: passed 2 h ago" without keeping a
// full transcript in memory. Records live at
// <app data dir>/skill-studio/runs/<skill_name>/<id>.json, transcripts at
// <id>.events.jsonl, and a per-skill `last.json` (written alongside each
// record) lets `skill_refresh::build_snapshot` cheaply read the newest
// outcome for every skill without listing every run.
// ============================================================================

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::skill_agent_runner::{HarnessId, SkillAgentEvent};

/// How many run records (and their transcripts) are kept per skill; older
/// ones are deleted when a new one is recorded.
const MAX_RUNS_PER_SKILL: usize = 20;

/// One run's judge-turn verdict, when the action ran one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRunJudge {
    pub passed: bool,
    pub sentence: String,
}

/// One completed run against a skill: ask, audit, or test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRunRecord {
    pub id: String,
    pub skill_name: String,
    pub harness: HarnessId,
    pub action: SkillRunAction,
    pub target_kind: Option<super::skill_run_target::SkillRunTargetKind>,
    pub started_at: String,
    pub duration_ms: u64,
    pub ok: bool,
    pub skill_loaded: super::skill_agent_runner::SkillLoaded,
    pub judge: Option<SkillRunJudge>,
    pub cost_usd: Option<f64>,
    pub final_text: String,
    pub transcript_path: String,
}

/// Which assistant action produced a `SkillRunRecord`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillRunAction {
    Ask,
    Audit,
    Test,
}

/// The cheap per-skill index `build_snapshot` reads for every skill's
/// dashboard/list row, written alongside every full record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRunSummary {
    pub at: String,
    pub harness: HarnessId,
    pub passed: Option<bool>,
}

fn runs_root(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Could not resolve app data dir: {e}"))?;
    Ok(data_dir.join("skill-studio").join("runs"))
}

fn skill_dir(root: &Path, skill_name: &str) -> PathBuf {
    root.join(skill_name)
}

/// Records one run's summary and transcript, then trims the skill's run
/// history down to `MAX_RUNS_PER_SKILL`.
#[tauri::command]
pub fn record_skill_run(
    app: AppHandle,
    record: SkillRunRecord,
    events: Vec<SkillAgentEvent>,
) -> Result<(), String> {
    let root = runs_root(&app)?;
    let dir = skill_dir(&root, &record.skill_name);
    fs::create_dir_all(&dir).map_err(|e| format!("Could not create run history dir: {e}"))?;

    let record_path = dir.join(format!("{}.json", record.id));
    let record_json = serde_json::to_vec_pretty(&record)
        .map_err(|e| format!("Could not serialize run record: {e}"))?;
    fs::write(&record_path, record_json).map_err(|e| format!("Could not write run record: {e}"))?;

    let events_path = dir.join(format!("{}.events.jsonl", record.id));
    let mut events_text = String::new();
    for event in &events {
        events_text.push_str(
            &serde_json::to_string(event).map_err(|e| format!("Could not serialize event: {e}"))?,
        );
        events_text.push('\n');
    }
    fs::write(&events_path, events_text).map_err(|e| format!("Could not write run events: {e}"))?;

    let summary = SkillRunSummary {
        at: record.started_at.clone(),
        harness: record.harness,
        passed: record.judge.as_ref().map(|j| j.passed).or(Some(record.ok)),
    };
    let summary_json = serde_json::to_vec_pretty(&summary)
        .map_err(|e| format!("Could not serialize run summary: {e}"))?;
    fs::write(dir.join("last.json"), summary_json)
        .map_err(|e| format!("Could not write last.json: {e}"))?;

    trim_run_history(&dir, MAX_RUNS_PER_SKILL)?;
    Ok(())
}

/// Deletes the oldest `.json`/`.events.jsonl` record pairs in `dir` beyond
/// `keep`, ordered by the record id (a run id, always chronological because
/// it's the same UUID v4/v7-ish token the frontend generates per run start -
/// sorted lexically by file mtime instead, which is monotonic regardless of
/// the id's own shape).
fn trim_run_history(dir: &Path, keep: usize) -> Result<(), String> {
    let mut records: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(dir)
        .map_err(|e| format!("Could not list run history: {e}"))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .map(|ext| ext == "json")
                .unwrap_or(false)
                && entry
                    .path()
                    .file_stem()
                    .map(|s| s != "last")
                    .unwrap_or(true)
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    records.sort_by_key(|(modified, _)| *modified);

    if records.len() > keep {
        for (_, path) in &records[..records.len() - keep] {
            let _ = fs::remove_file(path);
            if let Some(stem) = path.file_stem() {
                let events_path =
                    path.with_file_name(format!("{}.events.jsonl", stem.to_string_lossy()));
                let _ = fs::remove_file(events_path);
            }
        }
    }
    Ok(())
}

/// Every run recorded for `skill_name`, newest first, without transcripts.
#[tauri::command]
pub fn list_skill_runs(app: AppHandle, skill_name: String) -> Result<Vec<SkillRunRecord>, String> {
    let root = runs_root(&app)?;
    let dir = skill_dir(&root, &skill_name);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut records: Vec<SkillRunRecord> = fs::read_dir(&dir)
        .map_err(|e| format!("Could not list run history: {e}"))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .map(|ext| ext == "json")
                .unwrap_or(false)
                && entry
                    .path()
                    .file_stem()
                    .map(|s| s != "last")
                    .unwrap_or(true)
        })
        .filter_map(|entry| fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<SkillRunRecord>(&bytes).ok())
        .collect();
    records.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(records)
}

/// The transcript events recorded for run `id`, across every skill (the id
/// is a UUID, so a single directory scan first locates its skill folder).
#[tauri::command]
pub fn read_skill_run_events(
    app: AppHandle,
    skill_name: String,
    id: String,
) -> Result<Vec<SkillAgentEvent>, String> {
    let root = runs_root(&app)?;
    let path = skill_dir(&root, &skill_name).join(format!("{id}.events.jsonl"));
    let text = fs::read_to_string(&path).map_err(|e| format!("Could not read run events: {e}"))?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(|e| format!("Could not parse event: {e}")))
        .collect()
}

/// Reads every skill's `last.json` under `root`, for `skill_refresh::build_snapshot`
/// to fill `SkillSnapshot::last_test_by_skill` without listing every run.
pub fn read_last_test_index(
    root: &Path,
    skill_names: &[String],
) -> std::collections::HashMap<String, SkillRunSummary> {
    let mut index = std::collections::HashMap::new();
    for name in skill_names {
        let path = skill_dir(root, name).join("last.json");
        let Ok(bytes) = fs::read(&path) else { continue };
        if let Ok(summary) = serde_json::from_slice::<SkillRunSummary>(&bytes) {
            index.insert(name.clone(), summary);
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_record(id: &str, started_at: &str) -> SkillRunRecord {
        SkillRunRecord {
            id: id.to_string(),
            skill_name: "demo".to_string(),
            harness: HarnessId::ClaudeCode,
            action: SkillRunAction::Test,
            target_kind: None,
            started_at: started_at.to_string(),
            duration_ms: 100,
            ok: true,
            skill_loaded: super::super::skill_agent_runner::SkillLoaded::Yes,
            judge: Some(SkillRunJudge {
                passed: true,
                sentence: "It did the thing.".to_string(),
            }),
            cost_usd: Some(0.01),
            final_text: "done".to_string(),
            transcript_path: format!("{id}.events.jsonl"),
        }
    }

    #[test]
    fn trim_run_history_keeps_only_the_newest() {
        let dir = tempdir().unwrap();
        for i in 0..5 {
            let record = sample_record(
                &format!("run-{i}"),
                &format!("2024-01-0{}T00:00:00Z", i + 1),
            );
            fs::write(
                dir.path().join(format!("run-{i}.json")),
                serde_json::to_vec(&record).unwrap(),
            )
            .unwrap();
            fs::write(dir.path().join(format!("run-{i}.events.jsonl")), "").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        trim_run_history(dir.path(), 2).unwrap();

        let remaining: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
            .collect();
        assert_eq!(remaining.len(), 2);
    }

    #[test]
    fn read_last_test_index_reads_per_skill_summary() {
        let root = tempdir().unwrap();
        let dir = skill_dir(root.path(), "demo");
        fs::create_dir_all(&dir).unwrap();
        let summary = SkillRunSummary {
            at: "2024-01-01T00:00:00Z".to_string(),
            harness: HarnessId::Codex,
            passed: Some(false),
        };
        fs::write(dir.join("last.json"), serde_json::to_vec(&summary).unwrap()).unwrap();

        let index = read_last_test_index(root.path(), &["demo".to_string(), "other".to_string()]);
        assert_eq!(index.len(), 1);
        assert_eq!(index["demo"].passed, Some(false));
    }
}
