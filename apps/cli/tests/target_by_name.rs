// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! `park`, `unpark` and `remove` take a skill name. A name that matches one
//! copy acts on it; a name that matches more than one lists them and asks
//! for `--id`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn write_skill(root: &Path, name: &str) {
    let dir = root.join(".agents/skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: A test skill.\n---\nBody.\n"),
    )
    .unwrap();
}

/// A temp home with `solo` and `gamma` in the shared folder, and a temp
/// project that also has `gamma` in its shared folder.
fn home_and_project() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let home = root.join("home");
    let project = root.join("project");
    write_skill(&home, "solo");
    write_skill(&home, "gamma");
    write_skill(&project, "gamma");
    (dir, home, project)
}

fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_skill-studio"))
        .args(args)
        .arg("--home")
        .arg(home)
        .env("HOME", home)
        .env("SKILL_STUDIO_TELEMETRY", "0")
        .output()
        .expect("run skill-studio")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Flow: `park solo`, then `unpark solo`, by name only.
/// Expectation: each exits 0; the folder moves to the parked folder and
/// back.
/// A failure means a user without `--json` still cannot park a skill.
#[test]
fn park_and_unpark_by_name_move_the_one_matching_copy() {
    let (_dir, home, _project) = home_and_project();

    let park = run(&home, &["park", "solo"]);
    assert_eq!(park.status.code(), Some(0), "{}", text(&park.stderr));
    assert!(!home.join(".agents/skills/solo").exists());
    assert!(home.join(".agents/skills-parked/solo/SKILL.md").exists());

    let unpark = run(&home, &["unpark", "solo"]);
    assert_eq!(unpark.status.code(), Some(0), "{}", text(&unpark.stderr));
    assert!(home.join(".agents/skills/solo/SKILL.md").exists());
    assert!(!home.join(".agents/skills-parked/solo").exists());
}

/// Flow: `scan`, human output.
/// Expectation: each copy's line carries `id <id>`.
/// A failure means a user has no way to find the id `--id` needs.
#[test]
fn scan_prints_an_id_for_each_copy() {
    let (_dir, home, _project) = home_and_project();
    let scan = run(&home, &["scan"]);
    let stdout = text(&scan.stdout);
    let solo_line = stdout
        .lines()
        .find(|line| line.contains(".agents/skills/solo"))
        .unwrap_or_else(|| panic!("no line for solo:\n{stdout}"));
    assert!(solo_line.contains("  id "), "{solo_line}");
}

/// Flow: `park gamma` when the home and a project both have `gamma`.
/// Expectation: exit 2, both paths and both ids listed, a message that
/// names `--id`, and nothing moved. Then `park --id <home copy>` parks
/// only that copy.
/// A failure means a name could park the wrong copy, or an ambiguous name
/// gives no way forward.
#[test]
fn park_with_a_name_that_matches_two_copies_lists_them_and_asks_for_an_id() {
    let (_dir, home, project) = home_and_project();
    let project_arg = project.to_str().unwrap();

    let scan = run(&home, &["scan", "--project", project_arg, "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&scan.stdout).unwrap();
    let gamma = json["data"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["name"] == "gamma")
        .unwrap();
    let copies: Vec<(String, String)> = gamma["deployments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["path"].as_str().unwrap().to_string(),
                d["id"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(copies.len(), 2, "{gamma:?}");

    let park = run(&home, &["park", "gamma", "--project", project_arg]);
    assert_eq!(park.status.code(), Some(2));
    let stderr = text(&park.stderr);
    assert!(stderr.contains("--id"), "{stderr}");
    for (path, id) in &copies {
        assert!(stderr.contains(path), "{path} not listed:\n{stderr}");
        assert!(stderr.contains(id), "{id} not listed:\n{stderr}");
    }
    assert!(home.join(".agents/skills/gamma").exists());
    assert!(project.join(".agents/skills/gamma").exists());

    let home_copy = home.join(".agents/skills/gamma");
    let (_, home_id) = copies
        .iter()
        .find(|(path, _)| Path::new(path) == home_copy)
        .unwrap();
    let park = run(&home, &["park", "--id", home_id, "--project", project_arg]);
    assert_eq!(park.status.code(), Some(0), "{}", text(&park.stderr));
    assert!(home.join(".agents/skills-parked/gamma/SKILL.md").exists());
    assert!(project.join(".agents/skills/gamma").exists());
}

/// Flow: `park` with a name no skill has.
/// Expectation: exit 2 and a message that names the skill.
/// A failure means a typo reads as success or as a crash.
#[test]
fn park_with_an_unknown_name_says_so() {
    let (_dir, home, _project) = home_and_project();
    let park = run(&home, &["park", "nope"]);
    assert_eq!(park.status.code(), Some(2));
    assert!(text(&park.stderr).contains("nope"));
}
