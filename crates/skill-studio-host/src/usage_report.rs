//! Per-skill use counts over a rolling window of days, shared by the CLI's
//! `usage` command and the MCP server's `skill_usage` tool so both answer
//! "which skills did I not use?" the same way.
//!
//! The desktop app owns the use cache (`skill-uses.json` in its app data
//! folder). This module only ever reads it: it seeds an in-memory
//! [`SkillInvocationIndex`] from the cache so a refresh re-reads only what
//! changed since the app last ran, and never saves the result back.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Serialize;
use skill_studio_core::discovery_sources::DiscoverySources;
use skill_studio_core::dto::{Completeness, Inventory};
use skill_studio_core::identity::RootKind;
use skill_studio_core::ops::{OpStatus, Outcome};
use skill_studio_core::skill_uses::{counted_uses, SkillUseFilter};

use crate::fs::RealFs;
use crate::skill_uses::SkillInvocationIndex;

/// The window `usage` and `skill_usage` use when the caller names none.
pub const DEFAULT_USAGE_DAYS: u32 = 30;

/// A refresh reads at most a fixed byte budget. A first run with no desktop
/// cache can need more than one pass to read a large history; this caps the
/// passes so one report stays bounded, and a history still unread after
/// them makes the report partial.
const MAX_REFRESH_PASSES: usize = 8;

/// The desktop app's use cache under the platform data folder (`dirs::data_dir()`,
/// `~/Library/Application Support` on macOS): Tauri's `app_data_dir()` for
/// the `com.skillstudio.app` identifier, joined with the file name the app
/// writes.
pub fn desktop_usage_cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join("com.skillstudio.app").join("skill-uses.json")
}

/// One installed skill's uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillUsageRow {
    /// The skill's name.
    pub skill: String,
    /// Uses in the last `days` days of the report.
    pub recent_uses: u32,
    /// Uses across all the session history that was read.
    pub total_uses: u32,
    /// The most recent use, RFC 3339, or `None` when never used.
    pub last_used: Option<String>,
    /// Uses in the last `days` days, by agent id.
    pub agents: BTreeMap<String, u32>,
}

/// Uses for every installed, not parked skill over one window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsageReport {
    /// The window, in days, that `recent_uses` and `unused` cover.
    pub days: u32,
    /// Skills not used in the window first, by name; then the rest, most
    /// used first.
    pub rows: Vec<SkillUsageRow>,
    /// Names of the skills with no use in the window, in `rows` order.
    pub unused: Vec<String>,
    /// Set when some session history could not be read, so a skill in
    /// `unused` may still have uses that were not counted, or when the scan
    /// skipped a root, so `rows` may miss a skill.
    pub partial: bool,
}

impl Outcome for UsageReport {
    fn status(&self) -> OpStatus {
        if self.partial {
            OpStatus::Partial
        } else {
            OpStatus::Ok
        }
    }
}

/// An in-memory use index that stays alive across reports, so a
/// long-running caller (the MCP server) re-reads only what changed between
/// two calls.
#[derive(Debug, Clone, Default)]
pub struct SkillUsage {
    index: SkillInvocationIndex,
}

impl SkillUsage {
    /// Seeds the index from `cache` when given. Never writes the file.
    pub fn load_read_only(cache: Option<&Path>) -> Self {
        Self {
            index: cache
                .map(SkillInvocationIndex::load_read_only)
                .unwrap_or_default(),
        }
    }

    /// Refreshes the index from `home`'s agent session history, in memory
    /// only, and counts uses of every skill in `inventory` that is not
    /// parked over the `days` days before `now`.
    pub fn report(
        &mut self,
        home: &Path,
        inventory: &Inventory,
        days: u32,
        now: DateTime<Utc>,
    ) -> UsageReport {
        let sources = DiscoverySources::read(&RealFs::new(), home);
        let mut refresh = self.index.refresh(home, &sources);
        for _ in 1..MAX_REFRESH_PASSES {
            if !refresh.incomplete || refresh.bytes_read == 0 {
                break;
            }
            refresh = self.index.refresh(home, &sources);
        }

        let installed = active_skill_names(inventory);
        let filter = SkillUseFilter {
            known_skills: &installed,
            sources: &sources,
        };
        let cutoff = now - chrono::Duration::days(i64::from(days));
        let mut by_skill: BTreeMap<&str, Tally> = installed
            .iter()
            .map(|name| (name.as_str(), Tally::default()))
            .collect();
        for use_ in counted_uses(self.index.all_uses(), &filter) {
            let Some(tally) =
                installed_name(&use_.skill, &installed).and_then(|name| by_skill.get_mut(name))
            else {
                continue;
            };
            tally.total += 1;
            if tally.last_used.is_none_or(|last| use_.at > last) {
                tally.last_used = Some(use_.at);
            }
            if use_.at >= cutoff {
                tally.recent += 1;
                *tally.agents.entry(use_.harness.clone()).or_insert(0) += 1;
            }
        }

        let mut rows: Vec<SkillUsageRow> = by_skill
            .into_iter()
            .map(|(skill, tally)| SkillUsageRow {
                skill: skill.to_string(),
                recent_uses: tally.recent,
                total_uses: tally.total,
                last_used: tally.last_used.map(|at| at.to_rfc3339()),
                agents: tally.agents,
            })
            .collect();
        // Stable sort over name-ordered rows: unused skills keep name order.
        rows.sort_by(|a, b| {
            (a.recent_uses > 0)
                .cmp(&(b.recent_uses > 0))
                .then(b.recent_uses.cmp(&a.recent_uses))
        });
        let unused = rows
            .iter()
            .filter(|row| row.recent_uses == 0)
            .map(|row| row.skill.clone())
            .collect();

        UsageReport {
            days,
            rows,
            unused,
            partial: refresh.incomplete || inventory.completeness == Completeness::Partial,
        }
    }
}

#[derive(Default)]
struct Tally {
    recent: u32,
    total: u32,
    last_used: Option<DateTime<Utc>>,
    agents: BTreeMap<String, u32>,
}

/// One-shot report for a caller that exits right after (the CLI): loads the
/// desktop cache read-only when `cache` is given, refreshes in memory, and
/// drops the index.
pub fn usage_report(
    home: &Path,
    inventory: &Inventory,
    days: u32,
    cache: Option<&Path>,
) -> UsageReport {
    SkillUsage::load_read_only(cache).report(home, inventory, days, Utc::now())
}

/// Names of skills with at least one deployment outside the parked root: a
/// parked skill is already off for every agent, so it is not a candidate
/// for "unused".
fn active_skill_names(inventory: &Inventory) -> BTreeSet<String> {
    inventory
        .skills
        .iter()
        .filter(|skill| {
            skill
                .deployments
                .iter()
                .any(|deployment| deployment.root.kind != RootKind::Parked)
        })
        .map(|skill| skill.name.0.clone())
        .collect()
}

/// The installed name a recorded use counts toward: the name itself, or the
/// `base` of a plugin-qualified `prefix:base` name, the same rule
/// `SkillUseFilter` applies.
fn installed_name<'a>(recorded: &'a str, installed: &BTreeSet<String>) -> Option<&'a str> {
    if installed.contains(recorded) {
        return Some(recorded);
    }
    recorded
        .rsplit_once(':')
        .map(|(_, base)| base)
        .filter(|base| installed.contains(*base))
}

#[cfg(test)]
mod tests {
    use super::*;
    use skill_studio_core::dto::ScanRequest;
    use skill_studio_core::harness::HarnessCatalog;
    use skill_studio_core::identity::CorrelationId;
    use skill_studio_core::ops;
    use skill_studio_core::ports::{OpContext, Runtime};
    use skill_studio_core::scope::ProjectSelection;
    use skill_studio_core::RuntimeScope;
    use std::fs;
    use std::sync::Arc;

    /// A fixed "now" keeps every window boundary in these tests exact.
    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-30T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn days_ago(days: i64) -> String {
        (now() - chrono::Duration::days(days)).to_rfc3339()
    }

    fn write_skill(dir: &Path, name: &str) {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: A test skill.\n---\nBody.\n"),
        )
        .unwrap();
    }

    /// One Claude Code transcript line where the model called the Skill tool.
    fn claude_skill_line(skill: &str, at: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{at}","cwd":"/work","message":{{"content":[{{"type":"tool_use","name":"Skill","input":{{"skill":"{skill}"}}}}]}}}}"#
        )
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        home: PathBuf,
    }

    /// A home with three skills and one Claude Code transcript:
    /// - `gamma` (universal root) used 3 and 60 days ago, once through a
    ///   plugin-qualified `tools:gamma` name;
    /// - `idle` (Claude Code root) used once, 45 days ago;
    /// - `resting` (parked root) used 2 days ago, but parked;
    /// - `ghost`, used 1 day ago, is not installed anywhere.
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        write_skill(&home.join(".agents/skills"), "gamma");
        write_skill(&home.join(".claude/skills"), "idle");
        write_skill(&home.join(".agents/skills-parked"), "resting");

        let transcript_dir = home.join(".claude/projects/-work");
        fs::create_dir_all(&transcript_dir).unwrap();
        let lines = [
            claude_skill_line("gamma", &days_ago(3)),
            claude_skill_line("tools:gamma", &days_ago(60)),
            claude_skill_line("idle", &days_ago(45)),
            claude_skill_line("resting", &days_ago(2)),
            claude_skill_line("ghost", &days_ago(1)),
        ];
        fs::write(
            transcript_dir.join("session.jsonl"),
            format!("{}\n", lines.join("\n")),
        )
        .unwrap();
        Fixture { _dir: dir, home }
    }

    fn scan(home: &Path) -> Inventory {
        let mut scope = RuntimeScope::live(home.to_path_buf(), home.join(".skill-studio/history"));
        scope.projects = ProjectSelection::Explicit { paths: Vec::new() };
        let ports = crate::default_ports(
            home.join(".skill-studio/leases"),
            Arc::new(HarnessCatalog::builtin()),
        );
        let rt = Runtime::new(&scope, ports).unwrap();
        let ctx = OpContext::uncancellable(CorrelationId("usage-test".into()));
        ops::scan(&rt, &ctx, &ScanRequest::default()).unwrap()
    }

    fn row<'a>(report: &'a UsageReport, skill: &str) -> &'a SkillUsageRow {
        report
            .rows
            .iter()
            .find(|row| row.skill == skill)
            .expect("every installed skill has a row")
    }

    /// Flow: report a 30-day window over the fixture home.
    /// Expectation: `gamma` has 1 recent use by claude-code and 2 in total
    /// (the plugin-qualified use folds into it); `idle` has none recent, 1
    /// in total, and is the only `unused` name, listed first; the parked
    /// `resting` and the uninstalled `ghost` get no row.
    /// Failure means the CLI and MCP would name the wrong skills as safe to
    /// park.
    #[test]
    fn a_thirty_day_report_counts_recent_uses_and_lists_idle_skills_first() {
        let fixture = fixture();
        let inventory = scan(&fixture.home);

        let report = SkillUsage::default().report(&fixture.home, &inventory, 30, now());

        assert_eq!(report.days, 30);
        assert!(!report.partial, "{report:?}");
        let names: Vec<&str> = report.rows.iter().map(|r| r.skill.as_str()).collect();
        assert_eq!(names, ["idle", "gamma"], "{report:?}");
        assert_eq!(report.unused, ["idle"]);

        let gamma = row(&report, "gamma");
        assert_eq!(gamma.recent_uses, 1);
        assert_eq!(gamma.total_uses, 2);
        assert_eq!(gamma.last_used.as_deref(), Some(days_ago(3).as_str()));
        assert_eq!(
            gamma.agents,
            BTreeMap::from([("claude-code".to_string(), 1)])
        );

        let idle = row(&report, "idle");
        assert_eq!(idle.recent_uses, 0);
        assert_eq!(idle.total_uses, 1);
        assert_eq!(idle.last_used.as_deref(), Some(days_ago(45).as_str()));
        assert!(idle.agents.is_empty());
    }

    /// Flow: report a 90-day window over the same home.
    /// Expectation: `idle`'s 45-day-old use now counts, so nothing is unused
    /// and `gamma` (2 recent) sorts before `idle` (1 recent).
    /// Failure means `days` is ignored and the window is fixed.
    #[test]
    fn a_wider_window_counts_older_uses() {
        let fixture = fixture();
        let inventory = scan(&fixture.home);

        let report = SkillUsage::default().report(&fixture.home, &inventory, 90, now());

        assert!(report.unused.is_empty(), "{report:?}");
        assert_eq!(row(&report, "gamma").recent_uses, 2);
        assert_eq!(row(&report, "idle").recent_uses, 1);
        assert_eq!(report.rows[0].skill, "gamma");
    }

    /// Flow: seed from a desktop cache file, then report; separately, seed
    /// from a corrupt cache file.
    /// Expectation: both files are byte-for-byte unchanged afterwards and no
    /// `.corrupt` sibling appears; the report still counts the transcript.
    /// Failure means the CLI or MCP server wrote to the desktop app's cache.
    #[test]
    fn a_report_never_writes_the_desktop_cache() {
        let fixture = fixture();
        let inventory = scan(&fixture.home);
        let cache_dir = tempfile::tempdir().unwrap();

        let valid = cache_dir.path().join("skill-uses.json");
        SkillInvocationIndex::default().save(&valid).unwrap();
        let valid_before = fs::read(&valid).unwrap();
        let report =
            SkillUsage::load_read_only(Some(&valid)).report(&fixture.home, &inventory, 30, now());
        assert_eq!(row(&report, "gamma").recent_uses, 1);
        assert_eq!(fs::read(&valid).unwrap(), valid_before);

        let corrupt = cache_dir.path().join("corrupt.json");
        fs::write(&corrupt, b"{not json").unwrap();
        let report =
            SkillUsage::load_read_only(Some(&corrupt)).report(&fixture.home, &inventory, 30, now());
        assert_eq!(row(&report, "gamma").recent_uses, 1);
        assert_eq!(fs::read(&corrupt).unwrap(), b"{not json");
        assert!(!cache_dir.path().join("corrupt.json.corrupt").exists());
    }

    /// Flow: report once, append a new use to the transcript, report again
    /// with the same `SkillUsage`.
    /// Expectation: the second report sees the new use.
    /// Failure means the MCP server, which keeps one `SkillUsage` for its
    /// whole life, would serve stale counts.
    #[test]
    fn a_kept_index_picks_up_new_uses() {
        let fixture = fixture();
        let inventory = scan(&fixture.home);
        let mut usage = SkillUsage::default();
        assert_eq!(
            usage.report(&fixture.home, &inventory, 30, now()).unused,
            ["idle"]
        );

        let transcript = fixture.home.join(".claude/projects/-work/session.jsonl");
        let mut content = fs::read_to_string(&transcript).unwrap();
        content.push_str(&claude_skill_line("idle", &days_ago(1)));
        content.push('\n');
        fs::write(&transcript, content).unwrap();

        let report = usage.report(&fixture.home, &inventory, 30, now());
        assert!(report.unused.is_empty(), "{report:?}");
        assert_eq!(row(&report, "idle").recent_uses, 1);
    }
}
