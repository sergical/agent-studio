// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so the
// same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Unit 4.2 fix round, PR #294: `ops::outdated_target` must pick a skill's
//! `Fork` deployment first, ahead of the `SourceKind::Ord`-driven
//! `min_by_key` pick it otherwise falls back to. `Fork` sorts last in that
//! `Ord` (for unrelated precedence reasons elsewhere), so a forked skill
//! that still has a per-harness link on disk - `classify_owner` reads that
//! link as `Manual`, since the fork rule only ever claims the canonical
//! Universal deployment - would resolve to `Manual` and permanently report
//! `Currency::NotTracked` unless `Fork` is checked before the `Ord` pick.

use std::path::Path;
use std::sync::Arc;

use skill_studio_core::dto::ScanRequest;
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::UNIVERSAL_ROOT_RELATIVE;
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime, ScopeFs};
use skill_studio_core::skill_update_check::{
    CommitInfo, CommitLookup, Currency, PluginManifestLookup, SourceTreeLookup,
};
use skill_studio_core::testing::golden::{ctx, scope_for};
use skill_studio_core::testing::{FakeClock, FakeIds, FakeLease, FixtureBuilder, NoHistory};
use skill_studio_core::CoreError;

const HOME: &str = "/home";

fn skill_md(name: &str) -> Vec<u8> {
    format!("---\nname: {name}\ndescription: fixture skill for the outdated fork precedence check.\n---\nBody.\n")
        .into_bytes()
}

/// A single `CommitLookup` answer for every call - the fork rule and the
/// dotagents rule (if either ever ran) would both hit this.
struct FixedCommitLookup(CommitInfo);

impl CommitLookup for FixedCommitLookup {
    fn latest_commit(&self, _repo: &str, _path: &str) -> Result<Option<CommitInfo>, CoreError> {
        Ok(Some(self.0.clone()))
    }
}

/// Panics if reached: this fixture has no skills.sh candidate, so a tree
/// lookup call would mean the fork target was misclassified.
struct UnusedTreeLookup;

impl SourceTreeLookup for UnusedTreeLookup {
    fn tree_shas_at_head(
        &self,
        repo: &str,
    ) -> Result<std::collections::HashMap<String, String>, CoreError> {
        panic!("no skills.sh target expected a tree lookup, got one for {repo}");
    }
}

/// Panics if reached: this fixture has no plugin deployment.
struct UnusedPluginLookup;

impl PluginManifestLookup for UnusedPluginLookup {
    fn marketplace_version(
        &self,
        marketplace: &str,
        plugin: &str,
    ) -> Result<Option<String>, CoreError> {
        panic!(
            "no plugin target expected a marketplace lookup, got one for {marketplace}/{plugin}"
        );
    }
}

fn runtime(fs: Arc<dyn ScopeFs>) -> Runtime {
    let ports = Ports {
        fs,
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FakeLease::default()),
        history: Arc::new(NoHistory),
        sink: Arc::new(skill_studio_core::testing::RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
    };
    let scope = scope_for("outdated-fork-precedence", Path::new(HOME));
    Runtime::new(&scope, ports).expect("runtime")
}

/// A forked skill (`skill-studio.json`'s `forks` bucket names it, pinned to
/// `base-sha`) whose canonical Universal deployment sits under
/// `.agents/skills/find-bugs`, plus a leftover per-harness link at
/// `.claude/skills/find-bugs` classified `Manual`, plus a same-named
/// `agents.lock` row pinned to a different commit - the ledger a fork must
/// never fall back to.
fn forked_skill_with_a_leftover_manual_link() -> Arc<dyn ScopeFs> {
    Arc::new(
        FixtureBuilder::new()
            .dir(&format!("{HOME}/{UNIVERSAL_ROOT_RELATIVE}/find-bugs"))
            .file(
                &format!("{HOME}/{UNIVERSAL_ROOT_RELATIVE}/find-bugs/SKILL.md"),
                &skill_md("find-bugs"),
            )
            .alias(
                &format!("{HOME}/.claude/skills/find-bugs"),
                &format!("../../{UNIVERSAL_ROOT_RELATIVE}/find-bugs"),
            )
            .file(
                &format!("{HOME}/.agents/agents.lock"),
                br#"
[skills.find-bugs]
source = "getsentry/find-bugs"
resolved_path = "skills/find-bugs"
resolved_commit = "ledger-sha"
"#,
            )
            .file(
                &format!("{HOME}/.agents/skill-studio.json"),
                br#"{
                    "forks": {
                        "find-bugs": {
                            "deployment_id": "",
                            "skill_dir": "",
                            "repo": "getsentry/find-bugs",
                            "path": "skills/find-bugs",
                            "base_commit": "base-sha"
                        }
                    }
                }"#,
            )
            .build_fs(),
    )
}

/// Flow: `ops::outdated` over a forked skill that still has a per-harness
/// link on disk, so `scan` reports both a `Fork`-owned canonical deployment
/// and a `Manual`-owned linked one for the same skill name.
/// Expectation: the record's `installed_commit` is the registry's
/// `base_commit` ("base-sha"), not the ledger's `resolved_commit`
/// ("ledger-sha") and not absent (which `SourceKind::Manual`'s
/// `NotTracked` arm would produce) - proof the fork rule ran at all, not
/// just that it ran correctly once selected.
/// A failure here means `outdated_target` picked the `Manual` deployment
/// over the `Fork` one (this test is red without the `outdated_target` fix:
/// `Fork` sorts last in `SourceKind`'s derived `Ord`, so the shared
/// `min_by_key` alone picks `Manual`).
#[test]
fn a_forked_skill_with_a_leftover_manual_link_reports_the_fork_rules_currency_or_falls_back_to_manual(
) {
    let rt = runtime(forked_skill_with_a_leftover_manual_link());
    let req = ScanRequest {
        skills: Vec::new(),
        timings: false,
    };
    let result = ops::outdated(
        &rt,
        &ctx(),
        &req,
        &UnusedTreeLookup,
        &FixedCommitLookup(CommitInfo {
            sha: "newer-sha".to_string(),
            committed_at: Some("2026-02-01T00:00:00Z".to_string()),
        }),
        &UnusedPluginLookup,
    )
    .expect("outdated");

    let record = result
        .get("find-bugs")
        .expect("find-bugs is in the outdated result");
    assert_eq!(
        record.installed_commit.as_deref(),
        Some("base-sha"),
        "expected the fork's pinned base_commit, got {:?} (currency: {:?})",
        record.installed_commit,
        record.currency
    );
    assert_ne!(
        record.currency,
        Currency::NotTracked,
        "a fork with a recorded upstream must never report NotTracked"
    );
}
