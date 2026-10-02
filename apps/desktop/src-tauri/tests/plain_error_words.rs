// Integration test binaries aren't covered by the lib crate's `cfg_attr(test, allow(...))`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! User-facing errors reach toasts and dialogs unchanged (`callCommand` in
//! `skill-api.ts` passes the message straight through). This test reads the
//! production source of every crate that builds those messages and fails when
//! an error literal uses a developer word instead of plain words (skills,
//! folders, copies, links, agents, projects). The same word list guards the
//! frontend in `tools/oxlint/anti-slop/rules/no-internal-vocabulary.ts`.

use std::fs;
use std::path::{Path, PathBuf};

/// Matched case-insensitively at the start of a word. `canonical` is matched
/// as a whole word so `canonicalize` (a std call name) never trips it.
const BANNED_PREFIXES: &[&str] = &[
    "deployment",
    "lifecycle owner",
    "owner group",
    "materializ",
    "mutable",
    "unambiguous",
    "argv",
    "harness",
];
const BANNED_WHOLE_WORDS: &[&str] = &["canonical"];

/// Code that, earlier in the same statement, makes a string literal an error message.
const ERROR_MARKERS: &[&str] = &[
    "Err(",
    "ok_or(",
    "ok_or_else(",
    "map_err(",
    "CoreError::new(",
    "CoreError::unsupported(",
];

fn source_roots() -> Vec<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    vec![
        manifest.join("src"),
        manifest.join("../../../crates/skill-studio-core/src"),
        manifest.join("../../../crates/skill-studio-host/src"),
    ]
}

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

/// Production code only: everything before the first `#[cfg(test)]`, comments blanked.
fn production_code(source: &str) -> String {
    let end = source.find("#[cfg(test)]").unwrap_or(source.len());
    source[..end]
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("//") {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every non-raw string literal as `(line, text, code before it in the same statement)`.
fn string_literals(code: &str) -> Vec<(usize, String, String)> {
    let bytes = code.as_bytes();
    let mut found = Vec::new();
    let mut statement_start = 0;
    let mut line = 1;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => line += 1,
            b';' | b'{' | b'}' => statement_start = i + 1,
            b'\'' => {
                // A char literal such as '"' or ';'; lifetimes have no closing quote nearby.
                if bytes.get(i + 2) == Some(&b'\'') {
                    i += 2;
                } else if bytes.get(i + 1) == Some(&b'\\') && bytes.get(i + 3) == Some(&b'\'') {
                    i += 3;
                }
            }
            b'"' => {
                let start_line = line;
                let before = code[statement_start..i].to_string();
                let mut text = String::new();
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    if bytes[i] == b'\n' {
                        line += 1;
                    }
                    text.push(bytes[i] as char);
                    i += 1;
                }
                found.push((start_line, text, before));
            }
            _ => {}
        }
        i += 1;
    }
    found
}

/// The text a user would read: format placeholders like `{path}` are removed.
fn visible_text(literal: &str) -> String {
    let mut text = String::new();
    let mut depth = 0;
    for c in literal.chars() {
        match c {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => text.push(c),
            _ => {}
        }
    }
    text.to_lowercase()
}

fn banned_word_in(text: &str) -> Option<String> {
    let words = text
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty());
    for word in words {
        if BANNED_WHOLE_WORDS.contains(&word) {
            return Some(word.to_string());
        }
        if BANNED_PREFIXES
            .iter()
            .any(|prefix| word.starts_with(prefix))
        {
            return Some(word.to_string());
        }
    }
    BANNED_PREFIXES
        .iter()
        .filter(|prefix| prefix.contains(' '))
        .find(|prefix| text.contains(**prefix))
        .map(ToString::to_string)
}

#[test]
fn error_messages_use_plain_words_or_name_the_file_line_and_word() {
    let mut files = Vec::new();
    for root in source_roots() {
        rust_files(&root, &mut files);
    }
    assert!(files.len() > 20, "found only {} source files", files.len());

    let mut offences = Vec::new();
    for file in files {
        let source = fs::read_to_string(&file).expect("read source");
        for (line, literal, before) in string_literals(&production_code(&source)) {
            // A message has more than one word; a lone token is a phase or file name.
            if !literal.contains(' ') || !ERROR_MARKERS.iter().any(|m| before.contains(m)) {
                continue;
            }
            if let Some(word) = banned_word_in(&visible_text(&literal)) {
                offences.push(format!(
                    "{}:{line} says \"{word}\": {literal}",
                    file.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                ));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "error messages must use plain words (skills, folders, copies, links, agents):\n{}",
        offences.join("\n")
    );
}

#[test]
fn banned_word_scan_flags_developer_words_and_ignores_function_names() {
    assert_eq!(
        banned_word_in("not a deployment id"),
        Some("deployment".into())
    );
    assert_eq!(
        banned_word_in("one owner group only"),
        Some("owner group".into())
    );
    assert_eq!(
        banned_word_in("the canonical copy"),
        Some("canonical".into())
    );
    assert_eq!(banned_word_in("failed to canonicalize path"), None);
    assert_eq!(
        banned_word_in(&visible_text("no {deployment_id} here")),
        None
    );
}
