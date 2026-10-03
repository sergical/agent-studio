//! Constants for reading the per-skill settings that agents keep in their
//! own config files. Skill Studio only reads them; Park is the only off.

/// Cap on a harness config file this crate reads, matching the order of
/// magnitude `crate::ops::SKILL_MD_MAX_BYTES` uses for `SKILL.md` - these
/// are hand-maintained config files, not data dumps.
pub(crate) const HARNESS_CONFIG_MAX_BYTES: u64 = 1_048_576;

/// The `skillOverrides` value Claude Code reads as "hidden from Claude and
/// the `/` menu".
pub(crate) const CLAUDE_SKILL_OVERRIDE_OFF: &str = "off";
