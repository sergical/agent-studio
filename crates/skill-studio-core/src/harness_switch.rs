//! Shared pieces of the native per-skill switches that
//! [`crate::ops::set_harness_enabled`] writes, kept free of `std::fs`.
//!
//! The edits themselves live beside their readers so one writer owns each
//! file: Codex's `[[skills.config]]` row is `crate::ops::codex_write_disabled_row`
//! and OpenCode's `permission.skill` deny is
//! `crate::opencode_config::skill_denied_text`. pi, Cursor, and Grok Build
//! have no switch Skill Studio writes; Park is their off path.

use crate::error::{CoreError, ErrorCode};

/// Cap on a harness config file this module reads, matching the order of
/// magnitude `crate::ops::SKILL_MD_MAX_BYTES` uses for `SKILL.md` - these
/// are hand-maintained config files, not data dumps.
pub(crate) const HARNESS_CONFIG_MAX_BYTES: u64 = 1_048_576;

/// `true` when only `opencode.jsonc` exists: Skill Studio never parses that
/// format, so writing `permission.skill` would either create a `.json`
/// sibling `OpenCode` must then merge, or silently drop the user's comments.
pub(crate) fn opencode_refuses_jsonc(
    json_exists: bool,
    jsonc_exists: bool,
) -> Result<(), CoreError> {
    if jsonc_exists && !json_exists {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "OpenCode's config is opencode.jsonc; edit permission.skill by hand",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_refuses_jsonc_only() {
        assert!(opencode_refuses_jsonc(false, true).is_err());
        assert!(opencode_refuses_jsonc(true, true).is_ok());
        assert!(opencode_refuses_jsonc(false, false).is_ok());
    }
}
