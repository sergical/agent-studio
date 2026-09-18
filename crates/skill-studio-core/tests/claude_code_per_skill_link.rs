// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Real-disk integration tests for `harness::{disable_claude_link,
//! enable_claude_link, claude_link_state}`.
//!
//! Like `park_and_unpark.rs`, these use `skill-studio-host`'s real
//! `RealFs`/`FileLease` rather than the in-memory `FixtureFs`, since
//! `FixtureFs`'s write path refuses every call (see its doc comment in
//! `testing.rs`) - this module's whole job is the symlink mutation.

use std::path::Path;
use std::sync::Arc;

use skill_studio_core::harness::{
    claude_link_state, disable_claude_link, enable_claude_link, ClaudeLinkState,
};
use skill_studio_core::ports::{acquire_exclusive, Ports, Runtime};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::unique_temp_dir;
use skill_studio_core::testing::{FakeClock, FakeIds, NoHistory, RecordingSink};

use skill_studio_host::{FileLease, RealFs};

const UNIVERSAL_ROOT_RELATIVE: &str = ".agents/skills";
const CLAUDE_ROOT_RELATIVE: &str = ".claude/skills";

fn runtime_for(home: &Path) -> Runtime {
    let ports = Ports {
        fs: Arc::new(RealFs::new()),
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history: Arc::new(NoHistory),
        sink: Arc::new(RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(skill_studio_core::harness::HarnessCatalog::builtin()),
    };
    Runtime::new(&RuntimeScope::fixture(home), ports).unwrap()
}

/// Flow: a skill deployed to Claude Code as a per-skill symlink
/// (`~/.claude/skills/gamma -> ~/.agents/skills/gamma`) is disabled, then
/// re-enabled.
/// Expectation: after disabling, the link is gone and `claude_link_state`
/// reads `None`; after re-enabling, the link is back, points at the same
/// target, and `claude_link_state` reads `PerSkill` again.
/// Failure here (the link surviving the "removed" read, or the recreated
/// link missing or pointing at the wrong target) would mean the app tells
/// a person a skill is off for Claude Code while it is still loaded, or
/// silently rewires it to the wrong folder on re-enable.
#[test]
fn claude_code_per_skill_link_switch_writes_and_reads_back_the_removed_link_or_names_the_stale_state(
) {
    let home = unique_temp_dir("claude_per_skill_link");
    let target = home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma");
    std::fs::create_dir_all(&target).unwrap();
    let claude_skills = home.join(CLAUDE_ROOT_RELATIVE);
    std::fs::create_dir_all(&claude_skills).unwrap();
    let link_path = claude_skills.join("gamma");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link_path).unwrap();

    let rt = runtime_for(&home);
    let fs = rt.ports.fs.as_ref();
    assert_eq!(claude_link_state(fs, &link_path), ClaudeLinkState::PerSkill);

    let guard = acquire_exclusive(rt.ports.leases.as_ref(), &rt.scope).unwrap();
    let removed_target = disable_claude_link(fs, &rt.scope, &guard, &link_path).unwrap();
    assert_eq!(removed_target, target);
    assert_eq!(
        claude_link_state(fs, &link_path),
        ClaudeLinkState::None,
        "the link is still readable after disable - the app would report the skill as enabled"
    );
    assert!(
        !link_path.exists(),
        "disable_claude_link must actually remove the symlink from disk"
    );

    enable_claude_link(fs, &rt.scope, &guard, &link_path, &removed_target).unwrap();
    assert_eq!(
        claude_link_state(fs, &link_path),
        ClaudeLinkState::PerSkill,
        "the recreated link was not read back as a per-skill link"
    );
    assert_eq!(
        std::fs::read_link(&link_path).unwrap(),
        target,
        "the recreated link points at the wrong target"
    );
}

/// Flow: `~/.claude/skills` itself is a whole-folder symlink to
/// `~/.agents/skills` (no per-skill links underneath it), and the caller
/// asks to disable one named skill through it.
/// Expectation: `disable_claude_link` refuses rather than silently doing
/// nothing or breaking the shared link.
/// Failure here (a silent no-op, or the whole-dir link getting removed)
/// would turn every other skill off along with the one the caller asked
/// about.
#[test]
fn claude_code_per_skill_link_switch_refuses_on_a_whole_dir_link_or_names_the_silent_no_op() {
    let home = unique_temp_dir("claude_whole_dir_link");
    let universal = home.join(UNIVERSAL_ROOT_RELATIVE);
    std::fs::create_dir_all(universal.join("gamma")).unwrap();
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&universal, claude_dir.join("skills")).unwrap();
    let link_path = home.join(CLAUDE_ROOT_RELATIVE).join("gamma");

    let rt = runtime_for(&home);
    let fs = rt.ports.fs.as_ref();
    assert_eq!(claude_link_state(fs, &link_path), ClaudeLinkState::WholeDir);

    let guard = acquire_exclusive(rt.ports.leases.as_ref(), &rt.scope).unwrap();
    let err = disable_claude_link(fs, &rt.scope, &guard, &link_path).unwrap_err();
    assert_eq!(err.code, skill_studio_core::error::ErrorCode::Unsupported);
    assert!(
        home.join(".claude/skills").is_symlink(),
        "the whole-dir link must survive the refused call untouched"
    );
}
