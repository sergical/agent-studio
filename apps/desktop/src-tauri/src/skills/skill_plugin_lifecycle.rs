// ============================================================================
// Skills Module - skill_plugin_lifecycle
// Claude Code plugin actions: disable, enable, and uninstall a plugin
// through the scriptable `claude plugin` CLI. Every skill a plugin ships
// moves together, since Claude Code tracks the switch per plugin, not per
// skill. Codex has no plugin CLI - plugins there are managed with
// `/plugins` inside a Codex session, so this module only ever runs `claude`.
// ============================================================================

use std::path::Path;

use super::skill_add::CommandRunner;

const CLAUDE_CLI: &str = "claude";

/// `claude plugin disable|enable <plugin_id> -s user`.
pub fn plugin_set_enabled_args(plugin_id: &str, enabled: bool) -> Vec<String> {
    vec![
        "plugin".to_string(),
        (if enabled { "enable" } else { "disable" }).to_string(),
        plugin_id.to_string(),
        "-s".to_string(),
        "user".to_string(),
    ]
}

/// `claude plugin uninstall <plugin_id> -s user -y`.
pub fn plugin_uninstall_args(plugin_id: &str) -> Vec<String> {
    vec![
        "plugin".to_string(),
        "uninstall".to_string(),
        plugin_id.to_string(),
        "-s".to_string(),
        "user".to_string(),
        "-y".to_string(),
    ]
}

/// True for `<plugin>@<marketplace>`, both halves non-empty and built only
/// from the characters a plugin cache directory name allows (letters,
/// digits, `.`, `_`, `-`) - the same id core's `PluginSourceDto` and the
/// desktop's `PluginInfo::id` build as `"{plugin}@{marketplace}"`.
fn is_valid_plugin_id(plugin_id: &str) -> bool {
    let Some((plugin, marketplace)) = plugin_id.split_once('@') else {
        return false;
    };
    let is_id_part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    is_id_part(plugin) && is_id_part(marketplace)
}

/// Rewrites a spawn failure for a missing `claude` binary into a message the
/// user can act on. `CommandRunner::run` reports every failure as a plain
/// string, so this matches on the OS's "no such program" wording rather than
/// an error kind.
fn friendly_error(message: String) -> String {
    if message.contains("No such file or directory") || message.contains("cannot find the file") {
        "Claude Code CLI (`claude`) was not found on PATH.".to_string()
    } else {
        message
    }
}

/// Both Tauri commands in this module refuse to run against anything but
/// Claude Code: Codex has no plugin CLI, and plugins there are managed with
/// `/plugins` inside a Codex session instead.
pub fn require_claude_code_harness(harness: &str) -> Result<(), String> {
    if harness == "Claude Code" {
        Ok(())
    } else {
        Err("Only Claude Code plugins can be managed from Skill Studio.".to_string())
    }
}

fn require_valid_plugin_id(plugin_id: &str) -> Result<(), String> {
    if is_valid_plugin_id(plugin_id) {
        Ok(())
    } else {
        Err(format!(
            "Not a plugin id in \"<plugin>@<marketplace>\" form: {plugin_id}"
        ))
    }
}

/// Runs `claude plugin disable|enable` for one plugin.
pub fn set_plugin_enabled_with(
    runner: &dyn CommandRunner,
    plugin_id: &str,
    enabled: bool,
) -> Result<(), String> {
    require_valid_plugin_id(plugin_id)?;
    runner
        .run(
            CLAUDE_CLI,
            &plugin_set_enabled_args(plugin_id, enabled),
            None::<&Path>,
        )
        .map_err(friendly_error)
}

/// Runs `claude plugin uninstall` for one plugin.
pub fn uninstall_plugin_with(runner: &dyn CommandRunner, plugin_id: &str) -> Result<(), String> {
    require_valid_plugin_id(plugin_id)?;
    runner
        .run(CLAUDE_CLI, &plugin_uninstall_args(plugin_id), None::<&Path>)
        .map_err(friendly_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRunner {
        calls: Mutex<Vec<(String, Vec<String>)>>,
        fail: Option<String>,
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, program: &str, args: &[String], _cwd: Option<&Path>) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push((program.to_string(), args.to_vec()));
            match &self.fail {
                Some(err) => Err(err.clone()),
                None => Ok(()),
            }
        }
    }

    #[test]
    fn disable_builds_the_claude_disable_command() {
        let runner = FakeRunner::default();
        set_plugin_enabled_with(&runner, "codex@anthropics", false).unwrap();
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "claude");
        assert_eq!(
            calls[0].1,
            vec!["plugin", "disable", "codex@anthropics", "-s", "user"]
        );
    }

    #[test]
    fn enable_builds_the_claude_enable_command() {
        let runner = FakeRunner::default();
        set_plugin_enabled_with(&runner, "codex@anthropics", true).unwrap();
        let calls = runner.calls.lock().unwrap();
        assert_eq!(
            calls[0].1,
            vec!["plugin", "enable", "codex@anthropics", "-s", "user"]
        );
    }

    #[test]
    fn uninstall_builds_the_claude_uninstall_command() {
        let runner = FakeRunner::default();
        uninstall_plugin_with(&runner, "codex@anthropics").unwrap();
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls[0].0, "claude");
        assert_eq!(
            calls[0].1,
            vec![
                "plugin",
                "uninstall",
                "codex@anthropics",
                "-s",
                "user",
                "-y"
            ]
        );
    }

    #[test]
    fn invalid_plugin_id_is_rejected_without_running_anything() {
        let runner = FakeRunner::default();
        let err = set_plugin_enabled_with(&runner, "not-a-plugin-id", true).unwrap_err();
        assert!(err.contains("plugin"));
        assert!(runner.calls.lock().unwrap().is_empty());

        let err = uninstall_plugin_with(&runner, "codex@").unwrap_err();
        assert!(err.contains("plugin"));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn non_claude_code_harness_is_rejected() {
        assert!(require_claude_code_harness("Claude Code").is_ok());
        let err = require_claude_code_harness("Codex").unwrap_err();
        assert!(err.contains("Claude Code"));
    }
}
