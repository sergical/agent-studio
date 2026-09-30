// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! `skill-studio mcp` serves MCP over stdio: stdout carries JSON-RPC and
//! nothing else.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Flow: spawn `skill-studio mcp`, send `initialize`, `initialized` and
/// `tools/list`, then close stdin.
/// Expectation: the server names itself `skill-studio`, lists
/// `skill_usage`, and every stdout line is a JSON-RPC message.
/// A failure means an agent cannot start the server from the CLI, or
/// stray output breaks the channel.
#[test]
fn mcp_subcommand_answers_initialize_and_tools_list_over_stdio() {
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_skill-studio"))
        .arg("mcp")
        .env("HOME", home.path())
        .env("SKILL_STUDIO_HOME", home.path())
        .env("SKILL_STUDIO_TELEMETRY", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn skill-studio mcp");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let (lines_tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if lines_tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let next = || -> serde_json::Value {
        let line = lines
            .recv_timeout(Duration::from_secs(60))
            .expect("the server should answer within a minute");
        serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {line}"))
    };

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"cli-test","version":"0"}}}}}}"#
    )
    .unwrap();
    let init = next();
    assert_eq!(init["id"], 1, "{init}");
    assert_eq!(
        init["result"]["serverInfo"]["name"], "skill-studio",
        "{init}"
    );

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#).unwrap();
    let list = next();
    assert_eq!(list["id"], 2, "{list}");
    let tools = list["result"]["tools"].as_array().unwrap();
    assert!(
        tools.iter().any(|tool| tool["name"] == "skill_usage"),
        "{list}"
    );

    drop(stdin);
    for _ in 0..600 {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let status = child.try_wait().unwrap();
    if status.is_none() {
        child.kill().ok();
    }
    assert!(
        status.is_some_and(|s| s.success()),
        "the server should exit 0 when stdin closes, got {status:?}"
    );
    while let Ok(line) = lines.try_recv() {
        serde_json::from_str::<serde_json::Value>(&line)
            .unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {line}"));
    }
}
