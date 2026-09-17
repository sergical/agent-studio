//! Discovery sources: which harnesses' own project histories project
//! discovery may read, saved once in `~/.agents/skill-studio.json` so the
//! desktop, the CLI, and the MCP server all honour the same choice.

use std::collections::BTreeMap;
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ownership::{skill_studio_json_path, OWNERSHIP_LEDGER_MAX_BYTES};
use crate::ports::ScopeFs;

/// Per-harness switches for project discovery, as recorded under the
/// `discovery` key of `~/.agents/skill-studio.json`, keyed by harness id
/// ([`AgentId`](crate::identity::AgentId) wire names such as `claude-code`).
///
/// A harness with no entry is enabled, so a harness added to discovery
/// later needs no migration. Entries for ids no discovery source uses are
/// ignored and kept when the section is written back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct DiscoverySources(BTreeMap<String, bool>);

/// Only the `discovery` key [`DiscoverySources::read`] needs.
#[derive(Debug, Deserialize, Default)]
struct RawSkillStudioJson {
    #[serde(default)]
    discovery: DiscoverySources,
}

impl DiscoverySources {
    /// Reads `<home>/.agents/skill-studio.json`'s `discovery` key.
    ///
    /// A missing, unreadable, oversize, or malformed file - or a
    /// `discovery` section that is not an object of booleans - reads as
    /// every harness enabled, the same as a home that never changed the
    /// setting.
    pub fn read(fs: &dyn ScopeFs, home: &Path) -> Self {
        let path = skill_studio_json_path(home);
        let Ok(bytes) = fs.read_capped(&path, OWNERSHIP_LEDGER_MAX_BYTES) else {
            return Self::default();
        };
        let Ok(raw) = serde_json::from_slice::<RawSkillStudioJson>(&bytes) else {
            return Self::default();
        };
        raw.discovery
    }

    /// False only when `harness` is explicitly switched off.
    pub fn is_enabled(&self, harness: &str) -> bool {
        self.0.get(harness).copied().unwrap_or(true)
    }

    /// Switches `harness` on or off. Switching on removes the entry rather
    /// than storing `true`, so the file only ever lists harnesses that are
    /// off.
    pub fn set(&mut self, harness: &str, enabled: bool) {
        if enabled {
            self.0.remove(harness);
        } else {
            self.0.insert(harness.to_string(), false);
        }
    }

    /// True when no harness has an entry.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureBuilder;

    const HOME: &str = "/home/u";
    const REGISTRY: &str = "/home/u/.agents/skill-studio.json";

    fn read(registry: &[u8]) -> DiscoverySources {
        let fs = FixtureBuilder::new().file(REGISTRY, registry).build_fs();
        DiscoverySources::read(&fs, Path::new(HOME))
    }

    #[test]
    fn a_missing_file_enables_every_harness() {
        let fs = FixtureBuilder::new().dir(HOME).build_fs();
        let sources = DiscoverySources::read(&fs, Path::new(HOME));
        assert!(sources.is_empty());
        assert!(sources.is_enabled("claude-code"));
    }

    #[test]
    fn a_missing_section_or_key_reads_as_enabled() {
        assert!(read(br#"{"version":4}"#).is_enabled("codex"));
        let sources = read(br#"{"discovery":{"codex":false}}"#);
        assert!(!sources.is_enabled("codex"));
        assert!(sources.is_enabled("pi"));
    }

    #[test]
    fn an_explicit_true_reads_as_enabled() {
        assert!(read(br#"{"discovery":{"cursor":true}}"#).is_enabled("cursor"));
    }

    #[test]
    fn a_malformed_file_or_section_enables_every_harness() {
        assert!(read(b"not json").is_empty());
        assert!(read(br#"{"discovery":{"codex":"off"}}"#).is_empty());
        assert!(read(br#"{"discovery":["codex"]}"#).is_empty());
    }

    #[test]
    fn unknown_keys_are_ignored_and_kept() {
        let mut sources = read(br#"{"discovery":{"future-harness":false}}"#);
        assert!(sources.is_enabled("claude-code"));
        sources.set("codex", false);
        assert_eq!(
            serde_json::to_value(&sources).unwrap(),
            serde_json::json!({ "codex": false, "future-harness": false })
        );
    }

    #[test]
    fn set_switches_a_harness_off_and_back_on() {
        let mut sources = DiscoverySources::default();
        sources.set("open-code", false);
        assert!(!sources.is_enabled("open-code"));
        sources.set("open-code", true);
        assert!(sources.is_enabled("open-code"));
        assert!(sources.is_empty());
    }
}
