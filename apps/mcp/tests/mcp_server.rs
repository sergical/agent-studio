//! End-to-end tests: spawns the built `skill-studio-mcp` binary as a real
//! child process over stdio (the same transport a real MCP client uses) and
//! calls tools through `rmcp`'s client. Covers the PR 6 acceptance list:
//! restart equivalence, statelessness within one process, error mapping,
//! progress notifications, and that scope resolution follows the process's
//! environment variables rather than the real machine.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rmcp::model::{CallToolRequestParams, ProgressNotificationParam, ProgressToken};
use rmcp::service::{NotificationContext, RunningService};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use skill_studio_core::testing::fixtures;

fn mcp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_skill-studio-mcp")
}

/// The CLI binary's path. Not a Cargo dependency of this crate (artifact
/// dependencies still need nightly `-Z bindeps`), so this builds it directly
/// via `cargo build -p skill-studio-cli` and locates it next to this crate's
/// own binary, under the shared workspace `target/` directory.
fn cli_bin() -> PathBuf {
    let status = std::process::Command::new(env!("CARGO"))
        .args(["build", "-p", "skill-studio-cli", "--bin", "skill-studio"])
        .status()
        .expect("run cargo build -p skill-studio-cli");
    assert!(status.success(), "failed to build skill-studio-cli");
    Path::new(mcp_bin())
        .parent()
        .unwrap()
        .join(if cfg!(windows) {
            "skill-studio.exe"
        } else {
            "skill-studio"
        })
}

/// Materializes a named fixture (see `skill_studio_core::testing::fixtures`)
/// to a fresh temp directory and returns its canonical path.
fn materialized_fixture(name: &str) -> PathBuf {
    let dir = tempfile::tempdir().unwrap().keep();
    let (_, builder) = fixtures::all()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap();
    builder
        .materialize(&dir)
        .unwrap_or_else(|e| panic!("materialize {name}: {e}"));
    dir.canonicalize().unwrap()
}

/// Counts progress notifications received on a client connection, so tests
/// can assert "at least one" or "none" without threading a channel through.
#[derive(Clone, Default)]
struct CountingClient {
    count: Arc<AtomicUsize>,
}

impl ClientHandler for CountingClient {
    async fn on_progress(
        &self,
        _params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }
}

/// Spawns `skill-studio-mcp` with the given environment and connects a
/// client to it over stdio, exactly as a real MCP client would.
async fn connect(
    env: &[(&str, &str)],
) -> (RunningService<RoleClient, CountingClient>, CountingClient) {
    let client = CountingClient::default();
    let transport =
        TokioChildProcess::new(tokio::process::Command::new(mcp_bin()).configure(|cmd| {
            cmd.stderr(Stdio::null());
            for (key, value) in env {
                cmd.env(key, value);
            }
        }))
        .expect("spawn skill-studio-mcp");
    let running = client
        .clone()
        .serve(transport)
        .await
        .expect("initialize skill-studio-mcp");
    (running, client)
}

fn scan_args() -> serde_json::Map<String, serde_json::Value> {
    serde_json::json!({"skills": [], "timings": false})
        .as_object()
        .unwrap()
        .clone()
}

async fn call_scan(
    client: &RunningService<RoleClient, CountingClient>,
    progress_token: Option<i64>,
) -> serde_json::Value {
    let mut params = CallToolRequestParams::new("scan").with_arguments(scan_args());
    if let Some(token) = progress_token {
        params.meta = Some(rmcp::model::RequestMetaObject::with_progress_token(
            ProgressToken(rmcp::model::NumberOrString::Number(token)),
        ));
    }
    let result = client
        .call_tool(params)
        .await
        .expect("call_tool(scan) should reach the tool, not error at the transport layer");
    result
        .structured_content
        .expect("scan's CallToolResult always carries structured_content (the envelope)")
}

/// Blanks the fields that are fresh per call (`correlation_id`) or per
/// timing run (`data.timings`, when requested), so two otherwise-identical
/// envelopes compare equal.
fn normalize(mut json: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = json.as_object_mut() {
        obj.insert(
            "correlation_id".into(),
            serde_json::Value::String("-".into()),
        );
    }
    json
}

/// Restarting the process between two `scan` calls on the same fixture must
/// give byte-for-byte equal envelopes (once `correlation_id` is blanked):
/// nothing may be cached across a process lifetime.
#[tokio::test]
async fn restart_gives_the_same_envelope_as_the_first_run() {
    let home = materialized_fixture("basic");
    let env = [("SKILL_STUDIO_FIXTURE", home.to_str().unwrap())];

    let (client_a, _) = connect(&env).await;
    let first = normalize(call_scan(&client_a, None).await);
    client_a.cancel().await.ok();

    let (client_b, _) = connect(&env).await;
    let second = normalize(call_scan(&client_b, None).await);
    client_b.cancel().await.ok();

    assert_eq!(first, second, "a restarted server disagreed with itself");
    std::fs::remove_dir_all(&home).ok();
}

/// One process, two calls, a file added in between: the second `scan` must
/// see it. The core never caches a `Runtime` or an `Inventory` between
/// calls.
#[tokio::test]
async fn two_calls_on_one_process_see_a_file_added_in_between() {
    let home = materialized_fixture("basic");
    let env = [("SKILL_STUDIO_FIXTURE", home.to_str().unwrap())];
    let (client, _) = connect(&env).await;

    let first = call_scan(&client, None).await;

    let extra = home.join(".claude/skills/zzz-extra");
    std::fs::create_dir_all(&extra).unwrap();
    std::fs::write(
        extra.join("SKILL.md"),
        b"---\nname: zzz-extra\ndescription: Extra skill added mid-process.\n---\nBody.\n",
    )
    .unwrap();

    let second = call_scan(&client, None).await;
    client.cancel().await.ok();

    assert_ne!(
        first["data"]["skills"], second["data"]["skills"],
        "a second scan on the same process did not see a file added in between"
    );
    std::fs::remove_dir_all(&home).ok();
}

/// A fixture path that does not exist maps to the same `invalid_scope`
/// envelope the CLI prints, never a panic or a bare transport error.
#[tokio::test]
async fn a_nonexistent_fixture_maps_to_an_invalid_scope_envelope() {
    let env = [(
        "SKILL_STUDIO_FIXTURE",
        "/nonexistent/path/skill-studio-mcp-test",
    )];
    let (client, _) = connect(&env).await;
    let result = client
        .call_tool(CallToolRequestParams::new("scan").with_arguments(scan_args()))
        .await
        .expect("call_tool should still succeed at the transport layer");
    client.cancel().await.ok();

    assert_eq!(result.is_error, Some(true));
    let envelope = result.structured_content.expect("error envelope payload");
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["errors"][0]["code"], "invalid_scope");
}

/// A call with a progress token receives at least one progress
/// notification.
///
/// `rmcp`'s own client (used everywhere else in this file) always attaches
/// its own progress token to every request, so it cannot exercise the "no
/// token" half of this test; that half drives the server directly over raw
/// stdio JSON-RPC, the one place in this file that does so.
#[tokio::test]
async fn a_progress_token_yields_at_least_one_notification() {
    let home = materialized_fixture("basic");
    let env = [("SKILL_STUDIO_FIXTURE", home.to_str().unwrap())];

    let (with_token, counter) = connect(&env).await;
    call_scan(&with_token, Some(1)).await;
    with_token.cancel().await.ok();
    assert!(
        counter.count.load(Ordering::SeqCst) >= 1,
        "expected at least one progress notification when a token was supplied"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// A `tools/call` request with no `_meta.progressToken` at all receives no
/// `notifications/progress` before its response.
#[test]
fn no_progress_token_yields_no_notification() {
    use std::io::{BufRead, Write};

    let home = materialized_fixture("basic");
    let mut child = std::process::Command::new(mcp_bin())
        .env("SKILL_STUDIO_FIXTURE", home.to_str().unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn skill-studio-mcp");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());

    let mut send = |value: serde_json::Value| {
        writeln!(stdin, "{value}").unwrap();
    };
    let mut recv = || {
        let mut line = String::new();
        stdout.read_line(&mut line).expect("read a response line");
        serde_json::from_str::<serde_json::Value>(&line)
            .unwrap_or_else(|e| panic!("not one JSON document: {e}\n{line}"))
    };

    send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "raw-test-client", "version": "0.0.0"}
        }
    }));
    recv();
    send(serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": "scan", "arguments": {"skills": [], "timings": false}}
    }));

    let mut progress_notifications = 0usize;
    loop {
        let message = recv();
        if message["id"] == 2 {
            break;
        }
        if message["method"] == "notifications/progress" {
            progress_notifications += 1;
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    std::fs::remove_dir_all(&home).ok();

    assert_eq!(
        progress_notifications, 0,
        "expected no progress notifications when no token was supplied"
    );
}

/// Lists every path under `dir`, relative to it, sorted; `None` when `dir`
/// doesn't exist. Snapshots the real ambient data root around a
/// `SKILL_STUDIO_HOME`-scoped run, mirroring
/// `apps/cli/tests/envelope.rs::snapshot_tree`.
fn snapshot_tree(dir: &Path) -> Option<Vec<PathBuf>> {
    if !dir.exists() {
        return None;
    }
    fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    Some(out)
}

fn real_ambient_data_root() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("skill-studio");
        }
    }
    dirs::home_dir().unwrap().join(".local/share/skill-studio")
}

/// The same hazard the CLI's `--home` flag has: `SKILL_STUDIO_HOME` must
/// derive its own history root from itself, never from the real machine's
/// ambient XDG data root. Scope resolution follows the scope (the
/// environment the server was started with), never the process.
#[tokio::test]
async fn skill_studio_home_env_var_never_touches_the_real_data_root() {
    let real_root = real_ambient_data_root();
    let before = snapshot_tree(&real_root);

    let home = tempfile::tempdir().unwrap().keep();
    let dir = home.join(".agents/skills/zeta-bad");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        b"---\nname: zeta-bad\ndescription: Use this: when needed\n---\nBody.\n",
    )
    .unwrap();
    std::fs::write(
        home.join(".agents/.skill-lock.json"),
        br#"{"version":3,"skills":{"zeta-bad":{"source":"owner/zeta-bad","sourceType":"github","sourceUrl":"https://github.com/owner/zeta-bad","skillFolderHash":"deadbeef","installedAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}}}"#,
    )
    .unwrap();
    let home = home.canonicalize().unwrap();

    let env = [("SKILL_STUDIO_HOME", home.to_str().unwrap())];
    let (client, _) = connect(&env).await;
    let scan = call_scan(&client, None).await;
    let history_root = scan["scope"]["history_root"].as_str().unwrap();
    client.cancel().await.ok();

    assert!(
        Path::new(history_root).starts_with(&home),
        "history_root {history_root} is not under {}",
        home.display()
    );

    let after = snapshot_tree(&real_root);
    assert_eq!(
        before,
        after,
        "an env-scoped run touched the real ambient data root {}",
        real_root.display()
    );
    std::fs::remove_dir_all(&home).ok();
}

/// Runs `skill-studio` (the CLI) and returns its parsed stdout envelope.
fn run_cli(args: &[&str]) -> serde_json::Value {
    let output = std::process::Command::new(cli_bin())
        .args(args)
        .output()
        .expect("run skill-studio");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not one JSON document: {e}\n{stdout}"))
}

/// `watch --json` and a fresh MCP `scan` must agree on the current state:
/// starts `watch` against a fixture, mutates a skill through the CLI's
/// `preview-repair`/`apply-repair`, and checks that the inventory `watch`
/// reports after the change is the same one a fresh MCP `scan` reports.
///
/// There is no shared numeric revision authority between an independent
/// `watch` process and an independent MCP process (each MCP call is a fresh,
/// stateless `Runtime`), so "report the same revision" is checked as "report
/// the same state": the post-mutation inventories must be equal, and must
/// differ from the pre-mutation one.
#[tokio::test]
async fn watch_and_a_fresh_mcp_scan_agree_after_a_cli_mutation() {
    let home = materialized_fixture("manual_repairable_frontmatter");

    let mut watch = std::process::Command::new(cli_bin())
        .args(["watch", "--json", "--fixture", home.to_str().unwrap()])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn skill-studio watch");
    let stdout = watch.stdout.take().unwrap();
    let (line_tx, line_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });
    let recv_line = |rx: &std::sync::mpsc::Receiver<String>| {
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("watch line before timeout")
    };

    let initial: serde_json::Value = serde_json::from_str(&recv_line(&line_rx)).unwrap();
    let initial_inventory = initial["inventory"].clone();
    assert_ne!(initial_inventory, serde_json::Value::Null);

    let deployment_id = initial_inventory["skills"][0]["deployments"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let preview = run_cli(&[
        "preview-repair",
        "--fixture",
        home.to_str().unwrap(),
        "--deployment-id",
        &deployment_id,
        "--json",
    ]);
    assert_eq!(preview["status"], "ok", "{preview:?}");
    let preview_path = home.join("preview.json");
    std::fs::write(
        &preview_path,
        serde_json::to_string(&preview["data"]).unwrap(),
    )
    .unwrap();
    let apply = run_cli(&[
        "apply-repair",
        "--fixture",
        home.to_str().unwrap(),
        "--preview-json",
        preview_path.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(apply["status"], "ok", "{apply:?}");

    // `watch` polls; wait for the change line that follows the mutation.
    let _change: serde_json::Value = serde_json::from_str(&recv_line(&line_rx)).unwrap();
    let _ = watch.kill();
    let _ = watch.wait();

    let watch_scan = run_cli(&["scan", "--fixture", home.to_str().unwrap(), "--json"]);

    let env = [("SKILL_STUDIO_FIXTURE", home.to_str().unwrap())];
    let (client, _) = connect(&env).await;
    let mcp_scan = call_scan(&client, None).await;
    client.cancel().await.ok();

    assert_ne!(
        mcp_scan["data"]["skills"], initial_inventory["skills"],
        "the mutation through the CLI did not change what a fresh scan sees"
    );
    assert_eq!(
        mcp_scan["data"]["skills"], watch_scan["data"]["skills"],
        "watch and a fresh MCP scan disagreed on the post-mutation state"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// An agent calls a tool with whatever the tool's schema says is required and
/// nothing more. Every request field that has a sensible default must
/// therefore be optional in the published schema, and the tool must accept an
/// empty argument object. Without this, `scan` with `{}` fails inside rmcp's
/// parameter deserialization, before the handler runs, so the caller gets a
/// bare string instead of a `ResultEnvelope`.
#[tokio::test]
async fn tools_that_need_no_input_accept_an_empty_argument_object() {
    let home = materialized_fixture("basic");
    let (client, _) = connect(&[("SKILL_STUDIO_HOME", home.to_str().unwrap())]).await;

    let tools = client.list_all_tools().await.expect("list tools");
    let no_input = ["scan", "diagnose", "capabilities", "list_events"];
    for name in no_input {
        let schema = &tools
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name} is missing from tools/list"))
            .input_schema;
        let required = schema.get("required").and_then(|r| r.as_array());
        assert!(
            required.is_none_or(|r| r.is_empty()),
            "{name} publishes required fields {required:?}, so a caller cannot omit them"
        );

        let result = client
            .call_tool(
                CallToolRequestParams::new(name)
                    .with_arguments(serde_json::Map::<String, serde_json::Value>::new()),
            )
            .await
            .unwrap_or_else(|e| panic!("{name} with no arguments failed at the transport: {e}"));
        let Some(envelope) = result.structured_content.clone() else {
            panic!("{name} with no arguments returned no envelope: {result:?}");
        };
        assert_eq!(
            envelope.get("status").and_then(|s| s.as_str()),
            Some("ok"),
            "{name} with no arguments did not succeed: {envelope}"
        );
    }

    client.cancel().await.ok();
    std::fs::remove_dir_all(&home).ok();
}
