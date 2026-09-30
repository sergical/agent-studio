// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! `usage` counts each installed skill's uses from agent session history,
//! lists the never-used ones first, and ends with a summary line.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn write_skill(dir: &Path, name: &str) {
    let skill_dir = dir.join(name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: A test skill.\n---\nBody.\n"),
    )
    .unwrap();
}

fn days_ago(days: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339()
}

/// One Claude Code transcript line where the model called the Skill tool.
fn claude_skill_line(skill: &str, at: &str) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{at}","cwd":"/work","message":{{"content":[{{"type":"tool_use","name":"Skill","input":{{"skill":"{skill}"}}}}]}}}}"#
    )
}

/// A temp home where `gamma` was used 3 days ago and `idle` 45 days ago.
fn home() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    write_skill(&home.join(".agents/skills"), "gamma");
    write_skill(&home.join(".claude/skills"), "idle");
    let transcripts = home.join(".claude/projects/-work");
    std::fs::create_dir_all(&transcripts).unwrap();
    let lines = [
        claude_skill_line("gamma", &days_ago(3)),
        claude_skill_line("idle", &days_ago(45)),
    ];
    std::fs::write(
        transcripts.join("session.jsonl"),
        format!("{}\n", lines.join("\n")),
    )
    .unwrap();
    (dir, home)
}

fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_skill-studio"))
        .arg("usage")
        .args(args)
        .arg("--home")
        .arg(home)
        .env("HOME", home)
        .env("SKILL_STUDIO_TELEMETRY", "0")
        .output()
        .expect("run skill-studio usage")
}

/// Flow: `usage --json` over a home with one recent and one old use.
/// Expectation: `idle` first and in `unused`; `gamma` with one recent use
/// by Claude Code.
/// A failure means the report counts uses wrong or hides unused skills.
#[test]
fn usage_json_lists_unused_skills_first_with_counts_per_agent() {
    let (_dir, home) = home();
    let output = run(&home, &["--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["operation"], "skill_usage", "{json}");
    let data = &json["data"];
    assert_eq!(data["days"], 30);
    assert_eq!(data["unused"], serde_json::json!(["idle"]), "{data}");
    let rows = data["rows"].as_array().unwrap();
    assert_eq!(rows[0]["skill"], "idle", "{data}");
    assert_eq!(rows[0]["recent_uses"], 0);
    assert_eq!(rows[0]["total_uses"], 1);
    assert_eq!(rows[1]["skill"], "gamma", "{data}");
    assert_eq!(rows[1]["recent_uses"], 1);
    assert_eq!(rows[1]["agents"], serde_json::json!({"claude-code": 1}));
}

/// Flow: `usage` as text, with the default window and with `--days 60`.
/// Expectation: the unused skill comes before the used one, and the
/// summary line counts unused skills for the window asked for.
/// A failure means the text report and the window flag disagree.
#[test]
fn usage_text_ends_with_a_summary_for_the_window() {
    let (_dir, home) = home();
    let stdout = String::from_utf8(run(&home, &[]).stdout).unwrap();
    let idle = stdout.find("idle").unwrap_or_else(|| panic!("{stdout}"));
    let gamma = stdout.find("gamma").unwrap_or_else(|| panic!("{stdout}"));
    assert!(idle < gamma, "unused skills should come first:\n{stdout}");
    assert!(
        stdout.contains("1 of 2 skills not used in 30 days"),
        "{stdout}"
    );

    let stdout = String::from_utf8(run(&home, &["--days", "60"]).stdout).unwrap();
    assert!(
        stdout.contains("0 of 2 skills not used in 60 days"),
        "{stdout}"
    );
}
