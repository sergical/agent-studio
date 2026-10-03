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
    assert!(home
        .join(".agents/skills-parked/universal/solo/SKILL.md")
        .exists());

    let unpark = run(&home, &["unpark", "solo"]);
    assert_eq!(unpark.status.code(), Some(0), "{}", text(&unpark.stderr));
    assert!(home.join(".agents/skills/solo/SKILL.md").exists());
    assert!(!home.join(".agents/skills-parked/universal/solo").exists());
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
    assert!(home
        .join(".agents/skills-parked/universal/gamma/SKILL.md")
        .exists());
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

/// Installs `name` into the home's shared folder, or into `project`'s when
/// given, through `add --method copy`, so Skill Studio records the copy and
/// may remove it.
fn add_copy(home: &Path, project: Option<&Path>, name: &str) {
    std::fs::create_dir_all(home).unwrap();
    if let Some(project) = project {
        std::fs::create_dir_all(project).unwrap();
    }
    let source = tempfile::tempdir().unwrap();
    std::fs::write(
        source.path().join("SKILL.md"),
        format!("---\nname: {name}\ndescription: A test skill.\n---\nBody.\n"),
    )
    .unwrap();
    let mut args = vec!["add", "--method", "copy", "--name", name];
    if let Some(project) = project {
        args.extend(["--project-path", project.to_str().unwrap()]);
    }
    args.push(source.path().to_str().unwrap());
    let add = run(home, &args);
    assert_eq!(add.status.code(), Some(0), "{}", text(&add.stderr));
}

/// `(path, id)` of each copy of `name` that `scan` reports.
fn copies_of(home: &Path, project: &Path, name: &str) -> Vec<(PathBuf, String)> {
    let scan = run(
        home,
        &["scan", "--project", project.to_str().unwrap(), "--json"],
    );
    let json: serde_json::Value = serde_json::from_slice(&scan.stdout).unwrap();
    let skill = json["data"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["name"] == name)
        .unwrap_or_else(|| panic!("scan has no {name}: {json}"));
    skill["deployments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                PathBuf::from(d["path"].as_str().unwrap()),
                d["id"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// Flow: `remove kappa` when the home has a hand-placed `kappa` and the
/// project has a `kappa` that `add --method copy` installed.
/// Expectation: exit 0; only the project copy goes, the output names the
/// recovery folder, and the hand-placed copy stays.
/// A failure means a copy Skill Studio may not remove still counts as a
/// match, so the name is called ambiguous or picks a copy that `remove`
/// then refuses.
#[test]
fn remove_by_name_skips_a_hand_placed_copy_and_removes_the_other() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let home = root.join("home");
    let project = root.join("project");
    write_skill(&home, "kappa");
    add_copy(&home, Some(&project), "kappa");

    let remove = run(
        &home,
        &["remove", "kappa", "--project", project.to_str().unwrap()],
    );
    assert_eq!(remove.status.code(), Some(0), "{}", text(&remove.stderr));
    let stdout = text(&remove.stdout);
    assert!(stdout.contains("recovery folder"), "{stdout}");
    assert!(!project.join(".agents/skills/kappa").exists());
    assert!(home.join(".agents/skills/kappa/SKILL.md").exists());
}

/// Flow: `remove kappa` when the home and the project each have a `kappa`
/// that `add --method copy` installed.
/// Expectation: exit 2, both paths and both ids listed, a message that
/// names `--id`, and nothing removed.
/// A failure means a name could remove the wrong copy.
#[test]
fn remove_with_a_name_that_matches_two_removable_copies_lists_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let home = root.join("home");
    let project = root.join("project");
    add_copy(&home, None, "kappa");
    add_copy(&home, Some(&project), "kappa");
    let copies = copies_of(&home, &project, "kappa");
    assert_eq!(copies.len(), 2, "{copies:?}");

    let remove = run(
        &home,
        &["remove", "kappa", "--project", project.to_str().unwrap()],
    );
    assert_eq!(remove.status.code(), Some(2));
    let stderr = text(&remove.stderr);
    assert!(stderr.contains("--id"), "{stderr}");
    for (path, id) in &copies {
        assert!(
            stderr.contains(&path.display().to_string()),
            "{} not listed:\n{stderr}",
            path.display()
        );
        assert!(stderr.contains(id), "{id} not listed:\n{stderr}");
        assert!(
            path.join("SKILL.md").exists(),
            "{} was removed",
            path.display()
        );
    }
}

/// Flow: `remove solo` when the only `solo` was placed by hand.
/// Expectation: exit 2, a message that says `solo` has no copy that can be
/// removed, and the folder stays.
/// A failure means the user gets the op's internal refusal instead of a
/// plain answer, or a hand-placed skill is deleted.
#[test]
fn remove_by_name_with_only_a_hand_placed_copy_says_none_can_be_removed() {
    let (_dir, home, _project) = home_and_project();
    let remove = run(&home, &["remove", "solo"]);
    assert_eq!(remove.status.code(), Some(2));
    let stderr = text(&remove.stderr);
    assert!(
        stderr.contains("solo has no copy that can be removed"),
        "{stderr}"
    );
    assert!(home.join(".agents/skills/solo/SKILL.md").exists());
}

/// Flow: `unpark solo` when `solo` is on (not parked).
/// Expectation: exit 2, a message that says `solo` has no copy that can be
/// unparked, and the folder stays where it is.
/// A failure means `unpark` by name matches a copy that is not parked.
#[test]
fn unpark_by_name_with_no_parked_copy_says_none_can_be_unparked() {
    let (_dir, home, _project) = home_and_project();
    let unpark = run(&home, &["unpark", "solo"]);
    assert_eq!(unpark.status.code(), Some(2));
    let stderr = text(&unpark.stderr);
    assert!(
        stderr.contains("solo has no copy that can be unparked"),
        "{stderr}"
    );
    assert!(home.join(".agents/skills/solo/SKILL.md").exists());
}
