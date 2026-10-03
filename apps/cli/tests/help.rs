// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The help text is the first thing a new user reads. It must list every
//! user command, keep the dev commands out of sight, and use none of the
//! words that only make sense inside this codebase.

use std::process::Command;

const USER_COMMANDS: &[&str] = &[
    "scan",
    "diagnose",
    "conflicts",
    "usage",
    "park",
    "unpark",
    "enable",
    "disable",
    "remove",
    "fix",
    "undo",
    "events",
    "restore",
    "add",
    "outdated",
    "update",
    "mcp",
];

const HIDDEN_COMMANDS: &[&str] = &[
    "doctor",
    "split",
    "capabilities",
    "harnesses",
    "preview-repair",
    "apply-repair",
    "install-preferences",
    "sweep-quarantine",
    "schema",
    "health",
    "watch",
];

fn help(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_skill-studio"))
        .args(args)
        .arg("--help")
        .output()
        .expect("run skill-studio --help");
    assert!(output.status.success(), "{args:?} --help should exit 0");
    String::from_utf8(output.stdout).expect("help is UTF-8")
}

/// The command names in the `Commands:` section of the top-level help.
fn listed_commands(help: &str) -> Vec<String> {
    help.lines()
        .skip_while(|line| !line.starts_with("Commands:"))
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

fn assert_plain_words(what: &str, text: &str) {
    let lower = text.to_lowercase();
    for word in ["harness", "deployment", "invariant", "docs/"] {
        assert!(
            !lower.contains(word),
            "{what} uses the internal word {word:?}:\n{text}"
        );
    }
    assert!(!text.contains("DTO"), "{what} uses the word DTO:\n{text}");
}

/// Flow: `skill-studio --help`.
/// Expectation: every user command is listed and no dev command is.
/// A failure means a user cannot find a command, or sees one that is not
/// meant for them.
#[test]
fn top_level_help_lists_every_user_command_and_no_hidden_one() {
    let text = help(&[]);
    let listed = listed_commands(&text);
    for command in USER_COMMANDS {
        assert!(
            listed.iter().any(|c| c == command),
            "{command} is missing from the help:\n{text}"
        );
    }
    for command in HIDDEN_COMMANDS {
        assert!(
            !listed.iter().any(|c| c == command),
            "{command} should be hidden from the help:\n{text}"
        );
    }
    assert!(
        !text.contains("--fixture"),
        "--fixture should be hidden:\n{text}"
    );
}

/// Flow: `--help` for the top level and for each user command.
/// Expectation: none of them uses an internal word.
/// A failure means user-facing help leaks a word the user cannot know.
#[test]
fn help_text_uses_no_internal_words() {
    assert_plain_words("top-level help", &help(&[]));
    for command in USER_COMMANDS {
        let text = help(&[command]);
        assert_plain_words(&format!("`{command} --help`"), &text);
        assert!(
            !text.contains("--fixture"),
            "`{command} --help` shows --fixture:\n{text}"
        );
    }
}
