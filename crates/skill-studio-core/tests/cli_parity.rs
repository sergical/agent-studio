// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so the
// same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Unit 5.4 / issue #174: replays the nine `npx skills` runs recorded by
//! `scripts/record-cli-traces.sh` under
//! `crates/skill-studio-core/tests/fixtures/cli-traces/<nn>-<name>/` against
//! `skill_studio_core::ops`, with no network.
//!
//! **What this actually proves.** `InstallMethod::Dotagents`/`SkillsSh`
//! never fetch or write skill bytes themselves - `ops_install_cli.rs`,
//! `ops_update.rs`, and `ops_remove.rs` only build an argv/cwd and hand it to
//! the [`ProcessSpawner`] port, letting the real `npx` CLI write its own
//! files. A replay of one of these calls therefore cannot re-prove the CLI's
//! own fetch/write correctness (there is no CLI here to run); what it CAN
//! prove, and what every test below actually asserts, is (1) that `ops`
//! builds the exact argv/cwd the recorded trace's real `npx` call used, and
//! (2) that once a spawner materializes the same bytes the real CLI left,
//! `ops`'s own surrounding bookkeeping (the destination-exists check,
//! `link_claude_code`'s tolerance of an already-existing link, the final
//! on-disk tree) matches the recorded `after/` state byte for byte - the
//! `docs/action-map/definition-of-done.md` check 4 parity bar.
//!
//! Divergences this comparison found are named, not hidden: see
//! [`KNOWN_DIVERGENCES`]. Comparisons skip exactly the field/path a table
//! entry names; anything else diverging still fails the test.
//!
//! **Which assertions are independently checked, and which are inherently
//! limited by this design.** `command.txt` is written by `run_cli` itself in
//! `scripts/record-cli-traces.sh` - the exact argv/cwd/exit status a real
//! `npx skills` call used, never a hand-typed duplicate of what `ops` is
//! expected to build - so every `assert_argv_matches` call below is a
//! genuine, independent fact about the real CLI, not a comparison of `ops`
//! against itself. `assert_tree_matches_after` and the symlink-resolves
//! check are NOT independent in the same way: the bytes/symlinks a passing
//! test compares against were themselves written back by `ReplaySpawner`
//! materializing the recorded `after/` fixture, not by a real `npx` call -
//! so they prove `ops`'s own bookkeeping around a materialized CLI result
//! (destination checks, lock-file updates, `link_claude_code`'s tolerance of
//! an existing link), not that the CLI itself would still write those same
//! bytes today. Traces 03's `Io`-error assertions and the divergence
//! assertions in 04/08 are independent in the same way as the argv check,
//! since they assert facts about the *recorded* CLI run itself (its exit
//! status, or an argv shape it never received), not about a replay.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use skill_studio_core::dto::{
    InstallFile, InstallMethod, InstallOutcome, InstallRequest, RemoveRequest, UpdateRequest,
};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{AgentId, ProjectRef, RootKind, RootScope, SkillName};
use skill_studio_core::lock_file::{lock_file_path, read_lock_file};
use skill_studio_core::ops;
use skill_studio_core::ports::{
    CancelToken, Ports, ProcessOutput, ProcessSpawner, ProcessSpec, Runtime,
};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

const FIXTURES_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cli-traces");

// ---------------------------------------------------------------------------
// Fixture loading: `command.txt`, `before/`/`after/` (`tree.json` + `files/`).
// ---------------------------------------------------------------------------

/// One `tree.json` entry - a file (with its sha256, checked instead of the
/// raw bytes so a mismatch reports a short digest rather than a file dump),
/// a symlink (its normalized `$HOME`/`$PROJECT`-relative target), or an
/// empty directory.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
struct TreeEntry {
    path: String,
    kind: String,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    target: Option<String>,
}

/// Loads `<dir>/tree.json`, sorted by path (the recorder already sorts it;
/// re-sorting here makes the loader independent of that).
fn load_tree(dir: &Path) -> Vec<TreeEntry> {
    let bytes = std::fs::read(dir.join("tree.json"))
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.join("tree.json").display()));
    let mut entries: Vec<TreeEntry> = serde_json::from_slice(&bytes).unwrap();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

/// Writes `snapshot_dir`'s recorded `tree.json` onto `root` - the same
/// bytes/symlinks the real `npx` call left, standing in for what a real CLI
/// run would write. `$HOME`/`$PROJECT` placeholders in a symlink target are
/// resolved against `home_root`/`project_root`.
fn materialize(snapshot_dir: &Path, root: &Path, home_root: &Path, project_root: Option<&Path>) {
    for entry in load_tree(snapshot_dir) {
        let dest = root.join(&entry.path);
        match entry.kind.as_str() {
            "file" => {
                std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
                let bytes = std::fs::read(snapshot_dir.join("files").join(&entry.path)).unwrap();
                std::fs::write(&dest, &bytes).unwrap();
            }
            "dir" => {
                std::fs::create_dir_all(&dest).unwrap();
            }
            "symlink" => {
                std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
                let mut target = entry.target.clone().unwrap();
                target = target.replace("$HOME", &home_root.to_string_lossy());
                if let Some(project_root) = project_root {
                    target = target.replace("$PROJECT", &project_root.to_string_lossy());
                }
                if std::fs::symlink_metadata(&dest).is_err() {
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(target, &dest).unwrap();
                }
            }
            other => panic!("unhandled tree entry kind {other:?} at {}", entry.path),
        }
    }
}

/// Re-walks `root` with the same allow-listed top-level names the recorder
/// itself only ever descends into (`.agents`, `.claude`, `.codex`,
/// `.cursor`, `.config/opencode`, `skills-lock.json`), producing entries in
/// the same shape and sort order as a fixture's own `tree.json` - so the
/// two can be compared directly.
fn walk_allowed(root: &Path, home_root: &Path, project_root: Option<&Path>) -> Vec<TreeEntry> {
    const ALLOWED_TOP: &[&str] = &[
        ".agents",
        ".claude",
        ".codex",
        ".cursor",
        ".config",
        "skills-lock.json",
    ];
    fn normalize(s: &str, home_root: &Path, project_root: Option<&Path>) -> String {
        let mut r = s.replace(&*home_root.to_string_lossy(), "$HOME");
        if let Some(project_root) = project_root {
            r = r.replace(&*project_root.to_string_lossy(), "$PROJECT");
        }
        r
    }
    fn walk(
        root: &Path,
        rel: &Path,
        home_root: &Path,
        project_root: Option<&Path>,
        out: &mut Vec<TreeEntry>,
    ) {
        if rel == Path::new(".config") {
            let opencode = rel.join("opencode");
            if root.join(&opencode).exists() {
                walk(root, &opencode, home_root, project_root, out);
            }
            return;
        }
        let abs = root.join(rel);
        let meta = std::fs::symlink_metadata(&abs).unwrap();
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&abs).unwrap();
            out.push(TreeEntry {
                path: rel.to_string_lossy().into_owned(),
                kind: "symlink".to_string(),
                sha256: None,
                target: Some(normalize(
                    &target.to_string_lossy(),
                    home_root,
                    project_root,
                )),
            });
            return;
        }
        if meta.is_dir() {
            let mut names: Vec<_> = std::fs::read_dir(&abs)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            names.sort();
            if names.is_empty() && !rel.as_os_str().is_empty() {
                out.push(TreeEntry {
                    path: rel.to_string_lossy().into_owned(),
                    kind: "dir".to_string(),
                    sha256: None,
                    target: None,
                });
            }
            for name in names {
                let child = rel.join(&name);
                if rel.as_os_str().is_empty()
                    && !ALLOWED_TOP.contains(&child.to_string_lossy().as_ref())
                {
                    continue;
                }
                walk(root, &child, home_root, project_root, out);
            }
            return;
        }
        let bytes = std::fs::read(&abs).unwrap();
        out.push(TreeEntry {
            path: rel.to_string_lossy().into_owned(),
            kind: "file".to_string(),
            sha256: Some(sha256_hex(&bytes)),
            target: None,
        });
    }
    let mut out = Vec::new();
    if root.exists() {
        walk(root, Path::new(""), home_root, project_root, &mut out);
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Hex sha256, matching the recorder's Node `crypto.createHash("sha256")`
/// digest - `sha2` is already a direct dependency of this crate.
fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Parses `command.txt`: every argv token the trace's real `npx` call used
/// (written by `run_cli` itself in `scripts/record-cli-traces.sh` at the
/// moment it ran the traced call - never hand-typed), then a `--cwd--`
/// marker and the cwd label (`GLOBAL` or `$PROJECT`).
fn load_command(dir: &Path) -> (Vec<String>, Option<String>) {
    let text = std::fs::read_to_string(dir.join("command.txt")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let marker = lines
        .iter()
        .position(|l| *l == "--cwd--")
        .expect("command.txt must have a --cwd-- marker");
    let args: Vec<String> = lines[..marker].iter().map(ToString::to_string).collect();
    let cwd_label = lines.get(marker + 1).map(ToString::to_string);
    (args, cwd_label)
}

/// Reads `<dir>/meta.json`'s `exit_status` field - the real recorded `npx`
/// exit code, so `ReplaySpawner` never lies about a failed run by reporting
/// a fixed 0.
fn load_exit_status(dir: &Path) -> i32 {
    let bytes = std::fs::read(dir.join("meta.json")).unwrap();
    let meta: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    meta["exit_status"]
        .as_i64()
        .expect("meta.json must have an integer exit_status") as i32
}

// ---------------------------------------------------------------------------
// KNOWN_DIVERGENCES: every place this unit found `ops` building a different
// argv, cwd, or on-disk/lock-file result than the real `npx skills` CLI.
// Each entry names exactly what a comparison must skip; a comparison that
// no longer finds the named mismatch fails loudly (see the assertions in
// each test), so a fixed divergence forces this table to be edited too.
// ---------------------------------------------------------------------------

struct Divergence {
    trace: &'static str,
    field: &'static str,
    cli_value: &'static str,
    core_value: &'static str,
    reason: &'static str,
}

const KNOWN_DIVERGENCES: &[Divergence] = &[
    // As of this fix round, `fix/project-install-runs-in-project-dir` has
    // not landed in `main` - these three entries describe every
    // project-scope SkillsSh install (not just a local-folder source): the
    // real CLI has no `--cwd` flag for `add`, so a project-scope install
    // must cd into the project first and pass no `--cwd` token at all
    // (that is what trace 03 now records); `ops_install_cli.rs` instead
    // pushes a `--cwd <path>` argv token the CLI ignores, and never sets
    // `ProcessSpec.cwd` itself.
    Divergence {
        trace: "03-add-local-folder-project",
        field: "args (--cwd token)",
        cli_value: "no --cwd token at all (cd into the project instead)",
        core_value: "--cwd <project path> (a token skills@1.7.0's add silently ignores)",
        reason: "cli_args_and_cwd pushes a --cwd argument for every project-scope SkillsSh install; this affects all such installs, not only a local-folder source",
    },
    Divergence {
        trace: "03-add-local-folder-project",
        field: "ProcessSpec.cwd",
        cli_value: "the process's actual working directory (the CLI has no --cwd flag for `add`)",
        core_value: "None (cli_args_and_cwd never sets a cwd for SkillsSh; it only pushes a --cwd argument the CLI ignores)",
        reason: "a project-scope SkillsSh install replayed through ops installs into the host process's own cwd, not the target project - this affects all project-scope SkillsSh installs, not only a local-folder source",
    },
    Divergence {
        trace: "03-add-local-folder-project",
        field: "lock file path/schema",
        cli_value: "$PROJECT/skills-lock.json, {version:1, skills:{name:{source,sourceType:\"local\",computedHash}}}, no timestamps",
        core_value: "$PROJECT/.agents/.skill-lock.json, {version:3, skills:{name:{source,sourceType,sourceUrl,skillFolderHash,installedAt,updatedAt}}}",
        reason: "lock_file.rs only reads the global schema at the global path; a project-scope local install's lock file is invisible to core",
    },
    Divergence {
        trace: "04-add-two-harnesses",
        field: "InstallOutcome",
        cli_value: "cursor never received an --agent token, so nothing was ever done for it",
        core_value: "InstallOutcome::Installed, as if every requested harness (including cursor) had succeeded",
        reason: "cli_args_and_cwd only ever special-cases claude-code; Codex/OpenCode/pi legitimately need no --agent token at all (they read the shared universal root by design - harness.rs's reads_universal_root is Yes for all three), but cursor is a harness id the catalog does not recognize, and ops::install still reports the overall install as Installed with nothing done for it - the real gap is the silent over-reporting, not the missing token",
    },
];

fn divergence(trace: &str, field: &str) -> Option<&'static Divergence> {
    KNOWN_DIVERGENCES
        .iter()
        .find(|d| d.trace == trace && d.field == field)
}

impl std::fmt::Display for Divergence {
    /// `{d}` renders as the PR-body-ready row: what the real CLI did, what
    /// `ops` does instead, and why - each divergence assertion below
    /// interpolates this so a failure names the whole table entry, not just
    /// which side changed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}: CLI={:?} core={:?} ({})",
            self.trace, self.field, self.cli_value, self.core_value, self.reason
        )
    }
}

/// Compares `actual` against `expected`, skipping exactly the paths a
/// `KNOWN_DIVERGENCES` entry names for `trace` via `field` (a path prefix,
/// not a literal field like the argv/lock entries above). Anything else
/// diverging fails with the diverging path named.
fn assert_tree_matches(trace: &str, actual: &[TreeEntry], expected: &[TreeEntry]) {
    let skip_prefix = divergence(trace, "on-disk path");
    let filtered = |entries: &[TreeEntry]| -> Vec<TreeEntry> {
        entries
            .iter()
            .filter(|e| skip_prefix.is_none_or(|d| !e.path.starts_with(d.core_value)))
            .cloned()
            .collect()
    };
    let actual = filtered(actual);
    let expected = filtered(expected);
    assert_eq!(
        actual, expected,
        "{trace}: replayed tree diverged from the recorded after/ tree (see the diff above for the path)"
    );
}

// ---------------------------------------------------------------------------
// Runtime plumbing, matching install.rs/remove.rs's own helpers.
// ---------------------------------------------------------------------------

/// Replays one trace's recorded `after/` state onto disk whenever the
/// spawner is called - standing in for the real `npx` CLI, which unit 5.4
/// cannot invoke during `cargo test` (no network, no real HOME). Also
/// records every call's argv/cwd so each test can diff it against
/// `command.txt`.
struct ReplaySpawner {
    trace_dir: PathBuf,
    home_root: PathBuf,
    project_root: Option<PathBuf>,
    exit_status: i32,
    recorded: Mutex<Vec<(Vec<String>, Option<PathBuf>)>>,
}

impl ProcessSpawner for ReplaySpawner {
    fn run(
        &self,
        spec: &ProcessSpec,
        _cancel: &dyn CancelToken,
    ) -> Result<ProcessOutput, skill_studio_core::CoreError> {
        assert_eq!(spec.program, "npx");
        self.recorded
            .lock()
            .unwrap()
            .push((spec.args.clone(), spec.cwd.clone()));
        let root = spec.cwd.clone().unwrap_or_else(|| self.home_root.clone());
        // A `remove`/`update` call can make a path vanish entirely (a now-
        // empty parent directory `after/tree.json` never names, since only
        // an empty directory gets its own entry - see `snapshot_tree`).
        // Rather than diffing entry by entry, wipe every top-level name
        // `before/` touched and rebuild fresh from `after/` - simple, and
        // exact by construction.
        let before = load_tree(&self.trace_dir.join("before"));
        let mut top_level: Vec<String> = before
            .iter()
            .filter_map(|e| {
                Path::new(&e.path)
                    .components()
                    .next()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
            })
            .collect();
        top_level.sort();
        top_level.dedup();
        for name in top_level {
            let path = root.join(&name);
            std::fs::remove_file(&path)
                .or_else(|_| std::fs::remove_dir_all(&path))
                .ok();
        }
        materialize(
            &self.trace_dir.join("after"),
            &root,
            &self.home_root,
            self.project_root.as_deref(),
        );
        Ok(ProcessOutput {
            status: Some(self.exit_status),
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        })
    }
}

fn runtime_with(home: &Path, project: Option<&Path>, spawner: Arc<dyn ProcessSpawner>) -> Runtime {
    let history_root = home.join(".history");
    let db_path = history_root.join("events.sqlite3");
    let mut scope = RuntimeScope::fixture(home);
    // `fixture()` starts with no covered projects - a project-scope call
    // needs its path named explicitly, or every project-rooted op refuses
    // it as outside the scope (`ports::confine`).
    if let Some(project) = project {
        scope.projects = skill_studio_core::scope::ProjectSelection::Explicit {
            paths: vec![project.to_path_buf()],
        };
    }
    let ports = Ports {
        fs: Arc::new(RealFs::new()),
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history: Arc::new(SqliteHistoryOpener::new(db_path)),
        sink: Arc::new(RecordingSink::default()),
        spawner: Some(spawner),
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
    };
    Runtime::new(&scope, ports).unwrap()
}

/// A trace's on-disk fixture directory plus the temp home/project this
/// test replays it into.
struct TraceCtx {
    dir: PathBuf,
    home: PathBuf,
    project: Option<PathBuf>,
    spawner: Arc<ReplaySpawner>,
    rt: Runtime,
}

fn load_trace(trace_name: &str, scope: &RootScope) -> TraceCtx {
    let dir = PathBuf::from(FIXTURES_ROOT).join(trace_name);
    let home = unique_temp_dir(&format!("cli_parity_{trace_name}"));
    std::fs::create_dir_all(&home).unwrap();
    let project = match scope {
        RootScope::Global => None,
        RootScope::Project(p) => {
            std::fs::create_dir_all(&p.0).unwrap();
            Some(p.0.clone())
        }
    };
    materialize(
        &dir.join("before"),
        project.as_deref().unwrap_or(&home),
        &home,
        project.as_deref(),
    );
    let spawner = Arc::new(ReplaySpawner {
        trace_dir: dir.clone(),
        home_root: home.clone(),
        project_root: project.clone(),
        exit_status: load_exit_status(&dir),
        recorded: Mutex::new(Vec::new()),
    });
    let rt = runtime_with(&home, project.as_deref(), spawner.clone());
    TraceCtx {
        dir,
        home,
        project,
        spawner,
        rt,
    }
}

impl TraceCtx {
    fn root(&self) -> &Path {
        self.project.as_deref().unwrap_or(&self.home)
    }

    /// Asserts the single recorded `npx` call's argv/cwd match
    /// `command.txt`, skipping exactly the tokens `KNOWN_DIVERGENCES` names
    /// for this trace.
    fn assert_argv_matches(&self, trace_name: &str) {
        let (mut expected_args, cwd_label) = load_command(&self.dir);
        // Any `$PROJECT`/`$LOCAL_SKILL_DIR` placeholder left in a recorded
        // argv token (e.g. the source argv for a local-folder install) is
        // left as the literal placeholder, since [`install_request`]'s
        // callers pass that same literal as `source`. `$PROJECT` can still
        // appear standalone if a real project-scope value must be resolved
        // for comparison; nothing currently needs that, so this loop is a
        // no-op today and exists to keep the placeholder handling in one
        // place if a future trace's argv needs it.
        for arg in &mut expected_args {
            if arg == "$PROJECT" {
                *arg = self
                    .project
                    .as_ref()
                    .expect("$PROJECT token in command.txt needs a project scope")
                    .to_string_lossy()
                    .into_owned();
            }
        }
        let recorded = self.spawner.recorded.lock().unwrap();
        assert_eq!(
            recorded.len(),
            1,
            "{trace_name}: npx must be called exactly once"
        );
        let (raw_args, cwd) = &recorded[0];
        let mut args = raw_args.clone();
        if let Some(d) = divergence(trace_name, "args (--cwd token)") {
            // `ops` sends a `--cwd <path>` token the real CLI never
            // received (see KNOWN_DIVERGENCES) - strip it from the actual
            // argv before comparing, so everything else still matches
            // exactly. The position lookup is itself the check that the
            // divergence still reproduces.
            let pos = args
                .iter()
                .position(|a| a == "--cwd")
                .unwrap_or_else(|| panic!("{d} no longer reproduces - update KNOWN_DIVERGENCES"));
            args.remove(pos + 1); // the path
            args.remove(pos); // "--cwd"
        }
        assert_eq!(
            args, expected_args,
            "{trace_name}: ops's argv drifted from the recorded npx call"
        );
        if let Some(d) = divergence(trace_name, "ProcessSpec.cwd") {
            assert_eq!(
                cwd, &None,
                "{d} no longer reproduces - update KNOWN_DIVERGENCES"
            );
        } else {
            let expected_cwd = match cwd_label.as_deref() {
                Some("GLOBAL") => None,
                Some("$PROJECT") => self.project.clone(),
                other => panic!("unrecognized cwd label {other:?}"),
            };
            assert_eq!(
                cwd, &expected_cwd,
                "{trace_name}: ops's cwd drifted from the recorded npx call"
            );
        }
    }

    fn assert_tree_matches_after(&self, trace_name: &str) {
        let actual = walk_allowed(self.root(), &self.home, self.project.as_deref());
        let expected = load_tree(&self.dir.join("after"));
        assert_tree_matches(trace_name, &actual, &expected);
        self.assert_symlinks_resolve(&expected);
    }

    /// For every symlink `after/tree.json` names, asserts it actually
    /// resolves on disk (`std::fs::metadata` follows the link) - the
    /// functional property that matters, independent of whatever exact
    /// string form the target happens to be stored in.
    fn assert_symlinks_resolve(&self, expected: &[TreeEntry]) {
        for entry in expected.iter().filter(|e| e.kind == "symlink") {
            let dest = self.root().join(&entry.path);
            std::fs::metadata(&dest).unwrap_or_else(|e| {
                panic!(
                    "{}: symlink at {} does not resolve: {e}",
                    entry.path,
                    dest.display()
                )
            });
        }
    }
}

fn install_request(
    skill: &str,
    source: &str,
    scope: RootScope,
    harnesses: Vec<AgentId>,
) -> InstallRequest {
    InstallRequest {
        skill: SkillName(skill.to_string()),
        method: InstallMethod::SkillsSh,
        scope,
        harnesses,
        files: Vec::<InstallFile>::new(),
        source: Some(source.to_string()),
        trust_identity: None,
        trust_confirmed: true,
        save_as_preference: false,
    }
}

fn resolve_deployment_id(rt: &Runtime, skill: &str) -> skill_studio_core::identity::DeploymentId {
    let inventory = ops::scan(rt, &ctx(), &skill_studio_core::dto::ScanRequest::default()).unwrap();
    inventory
        .skills
        .iter()
        .find(|s| s.name.0 == skill)
        .and_then(|s| {
            s.deployments
                .iter()
                .find(|d| d.root.kind == RootKind::Universal)
        })
        .unwrap_or_else(|| panic!("no universal deployment found for {skill}"))
        .id
        .clone()
}

// ---------------------------------------------------------------------------
// Trace 01: add from GitHub, global scope, Claude Code harness.
// ---------------------------------------------------------------------------
#[test]
fn cli_add_github_global_claude_code_matches_the_recorded_trace_or_names_the_diverging_field() {
    let trace_name = "01-add-github-global-claude-code";
    let tc = load_trace(trace_name, &RootScope::Global);
    let req = install_request(
        "academy-guide",
        "anthropics/skills",
        RootScope::Global,
        vec![AgentId::from(AgentId::CLAUDE_CODE)],
    );
    let outcome = ops::install(&tc.rt, &ctx(), &req).unwrap();
    assert!(matches!(outcome, InstallOutcome::Installed { .. }));
    tc.assert_argv_matches(trace_name);
    tc.assert_tree_matches_after(trace_name);
    std::fs::remove_dir_all(&tc.home).ok();
}

// ---------------------------------------------------------------------------
// Trace 02: add from a skills.sh slug, global scope, no extra harness.
// ---------------------------------------------------------------------------
#[test]
fn cli_add_skillssh_slug_global_matches_the_recorded_trace_or_names_the_diverging_field() {
    let trace_name = "02-add-skillssh-slug-global";
    let tc = load_trace(trace_name, &RootScope::Global);
    let req = install_request(
        "web-design-guidelines",
        "vercel-labs/agent-skills@web-design-guidelines",
        RootScope::Global,
        Vec::new(),
    );
    let outcome = ops::install(&tc.rt, &ctx(), &req).unwrap();
    assert!(matches!(outcome, InstallOutcome::Installed { .. }));
    tc.assert_argv_matches(trace_name);
    tc.assert_tree_matches_after(trace_name);
    std::fs::remove_dir_all(&tc.home).ok();
}

// ---------------------------------------------------------------------------
// Trace 03: add a local folder, project scope - the recorded, real-CLI
// divergence: `cli_args_and_cwd` never sets a process cwd for a SkillsSh
// install (only a `--cwd` argv token the real CLI ignores for `add`), so the
// bytes land wherever the host process's own cwd happens to be, never at
// `req.scope`'s project path - and `install_via_cli`'s own post-call check
// looks for the destination at that project path regardless. The real,
// user-visible divergence is therefore that `ops::install` itself fails
// every project-scope SkillsSh install with an `Io` error, not that it
// silently installs to the wrong place - see KNOWN_DIVERGENCES.
// ---------------------------------------------------------------------------
#[test]
fn cli_add_local_folder_project_fails_the_whole_install_or_names_the_fix() {
    let trace_name = "03-add-local-folder-project";
    let project = ProjectRef(unique_temp_dir(&format!("cli_parity_{trace_name}_project")));
    let scope = RootScope::Project(project.clone());
    let tc = load_trace(trace_name, &scope);
    // `command.txt` records the source argv token as the literal
    // `$LOCAL_SKILL_DIR` placeholder (the recorder normalizes the temp
    // local-folder path it used, the same way it normalizes $HOME/$PROJECT)
    // - the `ReplaySpawner` never reads this value, it only materializes
    // the recorded `after/` bytes, so using that same literal string here
    // keeps the argv comparison meaningful without needing a real path.
    let req = install_request("my-local-skill", "$LOCAL_SKILL_DIR", scope, Vec::new());
    let err = ops::install(&tc.rt, &ctx(), &req).unwrap_err();
    assert_eq!(
        err.code,
        skill_studio_core::error::ErrorCode::Io,
        "if this now succeeds, ops_install_cli.rs started setting a cwd for SkillsSh - update KNOWN_DIVERGENCES and this test"
    );
    tc.assert_argv_matches(trace_name);

    // The bytes still land somewhere - under $HOME, the spawner's fallback
    // when `ProcessSpec.cwd` is `None` - even though the op as a whole
    // reports failure, since the CLI call itself already ran and wrote its
    // files before `install_via_cli`'s destination check rejects the result.
    assert!(
        tc.home
            .join(".agents/skills/my-local-skill/SKILL.md")
            .exists(),
        "the divergence's own destination (under $HOME, not $PROJECT) must exist"
    );
    assert!(
        !tc.project
            .as_ref()
            .unwrap()
            .join(".agents/skills/my-local-skill/SKILL.md")
            .exists(),
        "the requested $PROJECT destination must stay empty - this is exactly what the Io error above reports"
    );

    // The second known divergence: core's lock_file.rs cannot see the real
    // CLI's project-scope local lock file (wrong path and schema) - a fresh
    // read at the path core actually looks for must come back empty.
    let lock = read_lock_file(
        tc.rt.ports.fs.as_ref(),
        &lock_file_path(tc.project.as_ref().unwrap()),
    )
    .unwrap();
    assert!(
        lock.skills.is_empty(),
        "core's lock_file.rs must not see the real CLI's $PROJECT/skills-lock.json - if it now does, update KNOWN_DIVERGENCES"
    );

    std::fs::remove_dir_all(&tc.home).ok();
    std::fs::remove_dir_all(tc.project.as_ref().unwrap()).ok();
}

// ---------------------------------------------------------------------------
// Trace 04: add requesting two harnesses, one of which (`cursor`) the
// harness catalog does not recognize - the recorded CLI call never received
// an `--agent cursor` token (correctly: `cli_args_and_cwd` only ever
// special-cases Claude Code, and an unrecognized harness has nothing to
// send), yet `ops::install` still reports the overall install as
// `Installed`, as if `cursor` had been set up too. That silent
// over-reporting is the known divergence - see `KNOWN_DIVERGENCES`.
// ---------------------------------------------------------------------------
#[test]
fn cli_add_two_harnesses_reports_installed_even_for_an_unrecognized_harness_or_names_the_fix() {
    let trace_name = "04-add-two-harnesses";
    let tc = load_trace(trace_name, &RootScope::Global);
    let req = install_request(
        "brand-guidelines",
        "anthropics/skills",
        RootScope::Global,
        vec![AgentId::from(AgentId::CLAUDE_CODE), AgentId::from("cursor")],
    );
    let outcome = ops::install(&tc.rt, &ctx(), &req).unwrap();
    assert!(
        matches!(outcome, InstallOutcome::Installed { .. }),
        "{} no longer reproduces - update KNOWN_DIVERGENCES",
        divergence(trace_name, "InstallOutcome").unwrap()
    );
    tc.assert_argv_matches(trace_name);
    tc.assert_tree_matches_after(trace_name);
    std::fs::remove_dir_all(&tc.home).ok();
}

// ---------------------------------------------------------------------------
// Trace 05: add with the shared root only, global scope.
// ---------------------------------------------------------------------------
#[test]
fn cli_add_shared_root_only_matches_the_recorded_trace_or_names_the_diverging_field() {
    let trace_name = "05-add-shared-root-only";
    let tc = load_trace(trace_name, &RootScope::Global);
    let req = install_request(
        "web-artifacts-builder",
        "anthropics/skills",
        RootScope::Global,
        Vec::new(),
    );
    let outcome = ops::install(&tc.rt, &ctx(), &req).unwrap();
    assert!(matches!(outcome, InstallOutcome::Installed { .. }));
    tc.assert_argv_matches(trace_name);
    tc.assert_tree_matches_after(trace_name);
    std::fs::remove_dir_all(&tc.home).ok();
}

// ---------------------------------------------------------------------------
// Trace 06: update when the source changed - project scope.
// ---------------------------------------------------------------------------
#[test]
fn cli_update_newer_source_matches_the_recorded_trace_or_names_the_diverging_field() {
    let trace_name = "06-update-newer-source";
    let project = ProjectRef(unique_temp_dir(&format!("cli_parity_{trace_name}_project")));
    let scope = RootScope::Project(project.clone());
    let tc = load_trace(trace_name, &scope);
    let req = UpdateRequest {
        skill: SkillName("academy-guide".to_string()),
        method: InstallMethod::SkillsSh,
        scope,
        files: Vec::new(),
        source: Some("/does/not/matter/for/a/fake/spawner".to_string()),
        ref_pin: None,
    };
    ops::update(&tc.rt, &ctx(), &req).unwrap();
    tc.assert_argv_matches(trace_name);
    tc.assert_tree_matches_after(trace_name);
    std::fs::remove_dir_all(&tc.home).ok();
    std::fs::remove_dir_all(tc.project.as_ref().unwrap()).ok();
}

// ---------------------------------------------------------------------------
// Trace 07: update when nothing changed - project scope.
// ---------------------------------------------------------------------------
#[test]
fn cli_update_already_current_matches_the_recorded_trace_or_names_the_diverging_field() {
    let trace_name = "07-update-already-current";
    let project = ProjectRef(unique_temp_dir(&format!("cli_parity_{trace_name}_project")));
    let scope = RootScope::Project(project.clone());
    let tc = load_trace(trace_name, &scope);
    let req = UpdateRequest {
        skill: SkillName("academy-guide".to_string()),
        method: InstallMethod::SkillsSh,
        scope,
        files: Vec::new(),
        source: Some("/does/not/matter/for/a/fake/spawner".to_string()),
        ref_pin: None,
    };
    ops::update(&tc.rt, &ctx(), &req).unwrap();
    tc.assert_argv_matches(trace_name);
    tc.assert_tree_matches_after(trace_name);
    std::fs::remove_dir_all(&tc.home).ok();
    std::fs::remove_dir_all(tc.project.as_ref().unwrap()).ok();
}

// ---------------------------------------------------------------------------
// Trace 08: remove naming one of two harnesses - the real CLI removes the
// whole deployment regardless, matching `RemoveRequest`'s own lack of a
// harness field, so this is a plain match, not a divergence.
// ---------------------------------------------------------------------------
#[test]
fn cli_remove_one_named_harness_matches_the_recorded_trace_or_names_the_diverging_field() {
    let trace_name = "08-remove-one-harness";
    let tc = load_trace(trace_name, &RootScope::Global);
    let deployment_id = resolve_deployment_id(&tc.rt, "academy-guide");
    ops::remove(&tc.rt, &ctx(), &RemoveRequest { deployment_id }).unwrap();
    tc.assert_argv_matches(trace_name);
    tc.assert_tree_matches_after(trace_name);
    std::fs::remove_dir_all(&tc.home).ok();
}

// ---------------------------------------------------------------------------
// Trace 09: remove the last deployment - the lock entry goes entirely.
// ---------------------------------------------------------------------------
#[test]
fn cli_remove_last_deployment_matches_the_recorded_trace_or_names_the_diverging_field() {
    let trace_name = "09-remove-last-deployment";
    let tc = load_trace(trace_name, &RootScope::Global);
    let deployment_id = resolve_deployment_id(&tc.rt, "academy-guide");
    ops::remove(&tc.rt, &ctx(), &RemoveRequest { deployment_id }).unwrap();
    tc.assert_argv_matches(trace_name);
    tc.assert_tree_matches_after(trace_name);
    let lock = read_lock_file(tc.rt.ports.fs.as_ref(), &lock_file_path(&tc.home)).unwrap();
    assert!(
        !lock.skills.contains_key("academy-guide"),
        "the removed skill's lock entry must be gone"
    );
    std::fs::remove_dir_all(&tc.home).ok();
}
