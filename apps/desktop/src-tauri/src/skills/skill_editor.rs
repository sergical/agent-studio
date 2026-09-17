// ============================================================================
// skill_editor - which application "Open in editor" hands a skill folder to.
// macOS's `open -t` means the default *text* editor, which is TextEdit on a
// stock machine no matter how many code editors are installed, so the choice
// has to be made explicitly and remembered.
// ============================================================================

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::skills::skill_fork_registry::{read_fork_registry_or_default, write_fork_registry};

/// One editor the user can pick, as offered to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditorOption {
    /// The value to save: the macOS application name (without `.app`), an
    /// absolute path to a `.app` bundle, or the literal `$EDITOR`.
    pub app_name: String,
    /// What the picker shows.
    pub label: String,
}

/// Everything the Settings "Open in editor" card shows in one round trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorChoices {
    /// Label of the Automatic row: "Cursor (first found)", or "System
    /// default" when no known editor is installed.
    pub automatic_label: String,
    /// Known editors found in the Applications folders, then the saved app
    /// when it isn't one of them.
    pub apps: Vec<EditorOption>,
    /// The `$EDITOR` row, when the login shell sets VISUAL or EDITOR.
    /// `app_name` is "$EDITOR", `label` is the command name.
    pub terminal: Option<EditorOption>,
    /// The saved choice, only while it is still usable (installed name,
    /// existing `.app` path, or "$EDITOR" with a terminal editor set);
    /// otherwise `None`, which is also what opening falls back to.
    pub selected: Option<String>,
}

/// What running "Open in editor" actually does, once the saved choice (or
/// its fallback) has been resolved.
pub enum EditorLaunch {
    /// `open -a <app> <folder>` - `app` is a bundle name or a `.app` path.
    /// Empty means no `-a` flag at all, so macOS picks per file type.
    Open(Vec<String>),
    /// Hand `command` to the user's terminal-file handler via a one-shot
    /// script, the way git runs `$EDITOR`.
    Terminal { command: String },
}

/// The editors worth offering, in the order the picker lists them. The first
/// field is the `.app` bundle name; the second is its display label, which
/// differs for VS Code (bundle "Visual Studio Code") and the `JetBrains` IDEs.
const KNOWN_EDITORS: &[(&str, &str)] = &[
    ("Cursor", "Cursor"),
    ("Visual Studio Code", "VS Code"),
    ("VSCodium", "VSCodium"),
    ("Zed", "Zed"),
    ("Windsurf", "Windsurf"),
    ("Sublime Text", "Sublime Text"),
    ("Nova", "Nova"),
    ("BBEdit", "BBEdit"),
    ("IntelliJ IDEA", "IntelliJ IDEA"),
    ("WebStorm", "WebStorm"),
    ("RustRover", "RustRover"),
    ("PyCharm", "PyCharm"),
];

/// Where macOS keeps application bundles, most specific first.
fn application_dirs(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join("Applications"),
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
    ]
}

/// Whether `<name>.app` exists in any of `dirs`.
fn is_installed_in(dirs: &[PathBuf], app_name: &str) -> bool {
    dirs.iter()
        .any(|dir| dir.join(format!("{app_name}.app")).exists())
}

/// Every known editor present in `dirs`, in `KNOWN_EDITORS` order. An empty
/// result is a normal answer, not an error: the picker then offers only the
/// system default.
fn installed_editors_in(dirs: &[PathBuf]) -> Vec<EditorOption> {
    KNOWN_EDITORS
        .iter()
        .filter(|(app_name, _)| is_installed_in(dirs, app_name))
        .map(|(app_name, label)| EditorOption {
            app_name: (*app_name).to_string(),
            label: (*label).to_string(),
        })
        .collect()
}

/// The file stem of a `.app` bundle path: `/Apps/Zed Preview.app` -> `Zed Preview`.
fn app_path_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map_or_else(|| path.to_string(), |s| s.to_string_lossy().to_string())
}

/// The basename of a terminal editor command's first word:
/// `nvim` -> `nvim`, `code --wait` -> `code`, `/opt/homebrew/bin/nvim -p` -> `nvim`.
fn command_label(command: &str) -> String {
    let first_word = command.split_whitespace().next().unwrap_or(command);
    Path::new(first_word).file_name().map_or_else(
        || first_word.to_string(),
        |f| f.to_string_lossy().to_string(),
    )
}

/// Whether `value` names a `.app` bundle, case-insensitively - macOS's
/// default filesystem doesn't distinguish `.app`/`.App`/`.APP`.
fn has_app_extension(value: &str) -> bool {
    Path::new(value)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
}

/// Whether `value` is a `.app` bundle path that currently exists.
fn is_existing_app_path(value: &str) -> bool {
    has_app_extension(value) && Path::new(value).is_dir()
}

/// Whether a saved value is still something "Open in editor" can act on.
fn is_selected_usable(value: &str, installed: &[EditorOption], terminal: Option<&str>) -> bool {
    if value == "$EDITOR" {
        terminal.is_some()
    } else if has_app_extension(value) {
        is_existing_app_path(value)
    } else {
        installed.iter().any(|e| e.app_name == value)
    }
}

/// Store the choice in `home`'s registry, checking it against `dirs` and
/// `terminal`. `None` restores the system default.
fn set_preferred_editor_in(
    home: &Path,
    dirs: &[PathBuf],
    terminal: Option<&str>,
    value: Option<String>,
) -> Result<(), String> {
    let normalized = match value {
        None => None,
        Some(v) if v == "$EDITOR" => {
            if terminal.is_none() {
                return Err("No $VISUAL or $EDITOR is set in your login shell.".to_string());
            }
            Some(v)
        }
        Some(v) if has_app_extension(&v) => {
            let path = Path::new(&v);
            if !path.is_absolute() {
                return Err(format!("{v} must be an absolute path."));
            }
            if !path.is_dir() {
                return Err(format!("No app at {v}."));
            }
            // Don't let the same editor show up twice: a picked `.app` that
            // lives in a known Applications folder and matches a known
            // bundle name is saved as that name instead of its full path.
            let stem = app_path_stem(&v);
            let in_known_apps_dir = path
                .parent()
                .is_some_and(|parent| dirs.iter().any(|dir| dir.as_path() == parent));
            if in_known_apps_dir && KNOWN_EDITORS.iter().any(|(name, _)| *name == stem) {
                Some(stem)
            } else {
                Some(v)
            }
        }
        Some(v) if v.contains('/') => {
            return Err(format!("{v} must be an absolute path ending in .app."));
        }
        Some(v) => {
            if !is_installed_in(dirs, &v) {
                return Err(format!("{v} is not installed in Applications."));
            }
            Some(v)
        }
    };
    let mut registry = read_fork_registry_or_default(home);
    registry.preferred_editor = normalized;
    write_fork_registry(home, &registry)
}

pub fn installed_editors(home: &Path) -> Vec<EditorOption> {
    installed_editors_in(&application_dirs(home))
}

pub fn preferred_editor(home: &Path) -> Option<String> {
    read_fork_registry_or_default(home).preferred_editor
}

pub fn set_preferred_editor(home: &Path, value: Option<String>) -> Result<(), String> {
    // Only worth asking the login shell when the new value actually needs it -
    // this runs on every save, and most saves are a known app or a path.
    let terminal = if value.as_deref() == Some("$EDITOR") {
        terminal_editor_command()
    } else {
        None
    };
    set_preferred_editor_in(home, &application_dirs(home), terminal.as_deref(), value)
}

// ----------------------------------------------------------------------------
// $VISUAL / $EDITOR, read from the user's login shell - an app launched from
// the Dock doesn't inherit them, so this has to run a shell and ask.
// ----------------------------------------------------------------------------

const MARKER_START: &str = "__SKILL_STUDIO_ENV_START__";
const MARKER_END: &str = "__SKILL_STUDIO_ENV_END__";
const SHELL_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

fn shell_probe_script() -> String {
    format!(
        "echo {MARKER_START}; echo \"VISUAL=$VISUAL\"; echo \"EDITOR=$EDITOR\"; echo {MARKER_END}"
    )
}

/// Parses `VISUAL=`/`EDITOR=` lines between the marker lines, tolerating any
/// banner text an interactive login shell prints before or after them.
/// `VISUAL` wins when both are set; empty values on either side count as unset.
fn parse_terminal_editor(output: &str) -> Option<String> {
    let start = output.find(MARKER_START)?;
    let after_start = &output[start + MARKER_START.len()..];
    let end = after_start.find(MARKER_END).unwrap_or(after_start.len());
    let body = &after_start[..end];

    let mut visual = None;
    let mut editor = None;
    for line in body.lines() {
        if let Some(v) = line.strip_prefix("VISUAL=") {
            if !v.is_empty() {
                visual = Some(v.to_string());
            }
        } else if let Some(e) = line.strip_prefix("EDITOR=") {
            if !e.is_empty() {
                editor = Some(e.to_string());
            }
        }
    }
    visual.or(editor)
}

/// Runs `command` and returns everything printed on stdout up to and
/// including the first line containing `end_marker`, or `None` on a spawn
/// failure, a broken pipe, or `timeout`.
///
/// Reads stdout on a helper thread so a login shell's rc files can't hang
/// this forever: if one of them starts a background process that inherits
/// stdout (ssh-agent, gpg-agent, a prompt daemon, ...), that process keeps
/// the pipe's write end open long after the shell itself exits, and a plain
/// `read_to_string` would then block waiting for an end-of-file that never
/// comes. Stopping as soon as `end_marker` is seen sidesteps that, and also
/// avoids blocking on a banner larger than the pipe buffer. The child is
/// killed and reaped either way; the reader thread is left to finish (or
/// block) on its own instead of being joined, since joining could still hang
/// on that same held-open pipe.
fn run_with_timeout(mut command: Command, end_marker: &str, timeout: Duration) -> Option<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    let end_marker = end_marker.to_string();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut collected = String::new();
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let is_end_line = line.contains(&end_marker);
                    collected.push_str(&line);
                    if is_end_line {
                        break;
                    }
                }
            }
        }
        let _ = tx.send(collected);
    });

    let result = rx.recv_timeout(timeout).ok();
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// The login shell command that prints `$VISUAL`/`$EDITOR`. Both are removed
/// from the environment it inherits, so only the shell's own startup files
/// set them: `npm run` exports `EDITOR=vi` when none is set, and a dev build
/// started through it would otherwise show that fallback as the user's editor.
fn login_shell_probe(shell: &str) -> Command {
    let mut command = Command::new(shell);
    command
        .env_remove("VISUAL")
        .env_remove("EDITOR")
        .arg("-lic")
        .arg(shell_probe_script());
    command
}

/// Runs the login shell once to read `$VISUAL`/`$EDITOR`. stdin is closed so
/// an interactive shell can't block on a prompt.
fn read_login_shell_editor() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let output = run_with_timeout(login_shell_probe(&shell), MARKER_END, SHELL_PROBE_TIMEOUT)?;
    parse_terminal_editor(&output)
}

/// Cached for the process lifetime - the login shell is only worth asking once.
static TERMINAL_EDITOR: OnceLock<Option<String>> = OnceLock::new();

fn terminal_editor_command() -> Option<String> {
    TERMINAL_EDITOR.get_or_init(read_login_shell_editor).clone()
}

// ----------------------------------------------------------------------------
// The Settings card's state and the launch decision behind "Open in editor".
// ----------------------------------------------------------------------------

fn editor_choices_from(
    dirs: &[PathBuf],
    saved: Option<String>,
    terminal_command: Option<&str>,
) -> EditorChoices {
    let installed = installed_editors_in(dirs);

    let automatic_label = match installed.first() {
        Some(first) => format!("{} (first found)", first.label),
        None => "System default".to_string(),
    };

    let mut apps = installed.clone();
    if let Some(saved_value) = &saved {
        if is_existing_app_path(saved_value) && !apps.iter().any(|e| e.app_name == *saved_value) {
            apps.push(EditorOption {
                app_name: saved_value.clone(),
                label: app_path_stem(saved_value),
            });
        }
    }

    let terminal = terminal_command.map(|command| EditorOption {
        app_name: "$EDITOR".to_string(),
        label: command_label(command),
    });

    let selected = saved.filter(|value| is_selected_usable(value, &installed, terminal_command));

    EditorChoices {
        automatic_label,
        apps,
        terminal,
        selected,
    }
}

pub fn editor_choices(home: &Path) -> EditorChoices {
    editor_choices_from(
        &application_dirs(home),
        preferred_editor(home),
        terminal_editor_command().as_deref(),
    )
}

/// The launch decision for a saved choice (or its fallback): the first
/// installed known editor, else no `-a` flag at all so macOS picks per file
/// type. `-t` is deliberately not used - it means `TextEdit` on a stock machine
/// and refuses folders outright.
///
/// `terminal_editor` is a closure rather than an already-read value: it
/// starts the user's login shell, which can take up to `SHELL_PROBE_TIMEOUT`,
/// so it must only run when the saved choice is actually `$EDITOR` - not on
/// every "Open in editor" for a known app or path.
fn resolve_editor_launch(
    selected: Option<&str>,
    installed: &[EditorOption],
    terminal_editor: impl FnOnce() -> Option<String>,
) -> EditorLaunch {
    match selected {
        Some("$EDITOR") => {
            if let Some(command) = terminal_editor() {
                return EditorLaunch::Terminal { command };
            }
        }
        Some(value) if is_existing_app_path(value) => {
            return EditorLaunch::Open(vec!["-a".to_string(), value.to_string()]);
        }
        Some(value) if installed.iter().any(|e| e.app_name == value) => {
            return EditorLaunch::Open(vec!["-a".to_string(), value.to_string()]);
        }
        _ => {}
    }
    match installed.first() {
        Some(first) => EditorLaunch::Open(vec!["-a".to_string(), first.app_name.clone()]),
        None => EditorLaunch::Open(Vec::new()),
    }
}

pub fn editor_launch(home: &Path) -> EditorLaunch {
    resolve_editor_launch(
        preferred_editor(home).as_deref(),
        &installed_editors(home),
        terminal_editor_command,
    )
}

/// Single-quotes `value` for a POSIX shell, escaping any `'` as `'\''`.
fn shell_single_quote(value: &str) -> String {
    value.replace('\'', "'\\''")
}

/// Where the script should `cd` to and what it should hand the editor: the
/// path itself and `.` for a folder, or its parent and the file's own name
/// for a file - `cd`-ing into a file's parent instead of the file "opens the
/// file" the way a `SKILL.md` link expects, not its containing folder.
fn terminal_launch_target(path: &Path) -> (PathBuf, String) {
    if path.is_dir() {
        (path.to_path_buf(), ".".to_string())
    } else {
        let dir = path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let name = path
            .file_name()
            .map_or_else(|| ".".to_string(), |f| f.to_string_lossy().to_string());
        (dir, name)
    }
}

/// The `.command` script that `open` hands to the user's terminal-file
/// handler (Terminal by default): it removes itself, `cd`s into `folder`,
/// then `eval`s the editor command against `target` the way git runs
/// `$EDITOR`, so an argument-bearing value like `code --wait` still works.
/// `target` is kept in its own single-quoted variable `T` and referenced as
/// `"$T"` rather than interpolated into the double-quoted `eval` string
/// directly - otherwise a `$(...)` or backtick in a file name would expand
/// before `eval` runs.
fn terminal_launch_script(folder: &Path, target: &str, command: &str) -> String {
    format!(
        "#!/bin/sh\nrm -f -- \"$0\"\ncd -- '{}' || exit 1\nED='{}'\nT='{}'\neval \"exec $ED \\\"$T\\\"\"\n",
        shell_single_quote(&folder.display().to_string()),
        shell_single_quote(command),
        shell_single_quote(target)
    )
}

/// Writes `terminal_launch_script`'s output to a uniquely named, owner-only
/// executable file in the temp dir and returns its path. `path` is the skill
/// path "Open in editor" was given - a folder or a `SKILL.md` file.
pub fn write_terminal_launch_script(path: &Path, command: &str) -> Result<PathBuf, String> {
    let (folder, target) = terminal_launch_target(path);
    let script = terminal_launch_script(&folder, &target, command);
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("skill-studio-edit-{pid}-{nanos}.command"));
    std::fs::write(&path, script).map_err(|e| format!("Failed to write launch script: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("Failed to make launch script executable: {e}"))?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lone fake Applications folder, so a test never depends on what is
    /// actually installed on the machine running it.
    fn fake_applications(dir: &Path, apps: &[&str]) -> Vec<PathBuf> {
        for app in apps {
            std::fs::create_dir_all(dir.join(format!("{app}.app"))).expect("bundle");
        }
        vec![dir.to_path_buf()]
    }

    #[test]
    fn only_installed_editors_are_offered() {
        let home = tempfile::tempdir().expect("temp home");
        let dirs = fake_applications(home.path(), &["Zed"]);
        let offered = installed_editors_in(&dirs);
        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0].app_name, "Zed");
    }

    #[test]
    fn an_uninstalled_editor_is_refused_rather_than_saved() {
        let home = tempfile::tempdir().expect("temp home");
        let dirs = fake_applications(home.path(), &[]);
        let err = set_preferred_editor_in(home.path(), &dirs, None, Some("Cursor".to_string()))
            .expect_err("uninstalled editor must be refused");
        assert!(err.contains("not installed"), "{err}");
        assert_eq!(preferred_editor(home.path()), None);
    }

    #[test]
    fn a_saved_choice_round_trips_and_clears() {
        let home = tempfile::tempdir().expect("temp home");
        let dirs = fake_applications(home.path(), &["Zed"]);
        set_preferred_editor_in(home.path(), &dirs, None, Some("Zed".to_string())).expect("save");
        assert_eq!(preferred_editor(home.path()), Some("Zed".to_string()));
        set_preferred_editor_in(home.path(), &dirs, None, None).expect("clear");
        assert_eq!(preferred_editor(home.path()), None);
    }

    #[test]
    fn open_args_prefer_the_choice_then_the_first_installed_editor() {
        let zed = EditorOption {
            app_name: "Zed".into(),
            label: "Zed".into(),
        };
        let cursor = EditorOption {
            app_name: "Cursor".into(),
            label: "Cursor".into(),
        };
        let installed = vec![zed.clone(), cursor];
        assert!(matches!(
            resolve_editor_launch(None, &[], || None),
            EditorLaunch::Open(args) if args.is_empty()
        ));
        assert!(matches!(
            resolve_editor_launch(None, &installed, || None),
            EditorLaunch::Open(args) if args == vec!["-a".to_string(), "Zed".to_string()]
        ));
        assert!(matches!(
            resolve_editor_launch(Some("Cursor"), &installed, || None),
            EditorLaunch::Open(args) if args == vec!["-a".to_string(), "Cursor".to_string()]
        ));
    }

    #[test]
    fn editor_var_only_asks_the_shell_when_it_is_the_saved_choice() {
        let installed: Vec<EditorOption> = vec![];

        let mut asked = false;
        let launch = resolve_editor_launch(Some("$EDITOR"), &installed, || {
            asked = true;
            Some("nvim".to_string())
        });
        assert!(asked, "must ask the shell for $EDITOR");
        assert!(matches!(launch, EditorLaunch::Terminal { command } if command == "nvim"));

        let mut asked_again = false;
        let launch = resolve_editor_launch(Some("Zed"), &installed, || {
            asked_again = true;
            None
        });
        assert!(
            !asked_again,
            "must not ask the shell for a non-$EDITOR choice"
        );
        assert!(matches!(launch, EditorLaunch::Open(args) if args.is_empty()));
    }

    #[test]
    fn only_xcode_installed_leaves_no_apps_and_system_default() {
        let home = tempfile::tempdir().expect("temp home");
        let dirs = fake_applications(home.path(), &["Xcode"]);
        let choices = editor_choices_from(&dirs, None, None);
        assert!(choices.apps.is_empty());
        assert_eq!(choices.automatic_label, "System default");
    }

    #[test]
    fn one_known_editor_labels_automatic_as_first_found() {
        let home = tempfile::tempdir().expect("temp home");
        let dirs = fake_applications(home.path(), &["Cursor"]);
        let choices = editor_choices_from(&dirs, None, None);
        assert_eq!(choices.automatic_label, "Cursor (first found)");
    }

    #[test]
    fn a_saved_app_path_outside_the_known_list_is_offered_and_selected() {
        let home = tempfile::tempdir().expect("temp home");
        let dirs = fake_applications(home.path(), &[]);
        let custom = home.path().join("Zed Preview.app");
        std::fs::create_dir_all(&custom).expect("bundle");
        let saved = custom.to_string_lossy().to_string();

        let choices = editor_choices_from(&dirs, Some(saved.clone()), None);
        assert_eq!(choices.apps.len(), 1);
        assert_eq!(choices.apps[0].app_name, saved);
        assert_eq!(choices.apps[0].label, "Zed Preview");
        assert_eq!(choices.selected, Some(saved.clone()));

        let launch = resolve_editor_launch(Some(&saved), &choices.apps, || None);
        assert!(matches!(
            launch,
            EditorLaunch::Open(args) if args == vec!["-a".to_string(), saved]
        ));
    }

    #[test]
    fn a_saved_path_that_no_longer_exists_is_not_selected_and_falls_back() {
        let home = tempfile::tempdir().expect("temp home");
        let dirs = fake_applications(home.path(), &["Zed"]);
        let missing = home.path().join("Gone.app").to_string_lossy().to_string();

        let choices = editor_choices_from(&dirs, Some(missing.clone()), None);
        assert!(choices.apps.iter().all(|e| e.app_name != missing));
        assert_eq!(choices.selected, None);

        let installed = installed_editors_in(&dirs);
        let launch = resolve_editor_launch(Some(&missing), &installed, || None);
        assert!(matches!(
            launch,
            EditorLaunch::Open(args) if args == vec!["-a".to_string(), "Zed".to_string()]
        ));
    }

    #[test]
    fn picking_a_known_editors_app_bundle_saves_its_name_not_its_path() {
        let home = tempfile::tempdir().expect("temp home");
        let dirs = fake_applications(home.path(), &["Cursor"]);
        let path = dirs[0].join("Cursor.app").to_string_lossy().to_string();

        set_preferred_editor_in(home.path(), &dirs, None, Some(path)).expect("save");
        assert_eq!(preferred_editor(home.path()), Some("Cursor".to_string()));
    }

    #[test]
    fn set_preferred_editor_refuses_a_missing_app_path() {
        let home = tempfile::tempdir().expect("temp home");
        let dirs = fake_applications(home.path(), &[]);
        let missing = home.path().join("Nope.app").to_string_lossy().to_string();
        let err = set_preferred_editor_in(home.path(), &dirs, None, Some(missing)).unwrap_err();
        assert!(err.contains("No app at"), "{err}");
    }

    #[test]
    fn set_preferred_editor_refuses_a_relative_path() {
        let home = tempfile::tempdir().expect("temp home");
        let dirs = fake_applications(home.path(), &[]);
        let err =
            set_preferred_editor_in(home.path(), &dirs, None, Some("relative/dir".to_string()))
                .unwrap_err();
        assert!(err.contains("absolute path"), "{err}");
    }

    #[test]
    fn set_preferred_editor_refuses_editor_var_when_unset() {
        let home = tempfile::tempdir().expect("temp home");
        let dirs = fake_applications(home.path(), &[]);
        let err = set_preferred_editor_in(home.path(), &dirs, None, Some("$EDITOR".to_string()))
            .unwrap_err();
        assert!(err.contains("$VISUAL or $EDITOR"), "{err}");

        set_preferred_editor_in(
            home.path(),
            &dirs,
            Some("nvim"),
            Some("$EDITOR".to_string()),
        )
        .expect("accepted once a terminal editor is set");
        assert_eq!(preferred_editor(home.path()), Some("$EDITOR".to_string()));
    }

    #[test]
    fn marker_parsing_ignores_banner_noise_and_prefers_visual() {
        let output =
            format!("Welcome to zsh!\n{MARKER_START}\nVISUAL=nvim\nEDITOR=vi\n{MARKER_END}\nbye\n");
        assert_eq!(parse_terminal_editor(&output), Some("nvim".to_string()));
    }

    #[test]
    fn marker_parsing_falls_back_to_editor_when_visual_is_empty() {
        let output = format!("{MARKER_START}\nVISUAL=\nEDITOR=vi\n{MARKER_END}\n");
        assert_eq!(parse_terminal_editor(&output), Some("vi".to_string()));
    }

    #[test]
    fn marker_parsing_returns_none_when_both_are_empty() {
        let output = format!("{MARKER_START}\nVISUAL=\nEDITOR=\n{MARKER_END}\n");
        assert_eq!(parse_terminal_editor(&output), None);
    }

    #[test]
    fn marker_parsing_returns_none_without_markers() {
        assert_eq!(parse_terminal_editor("VISUAL=nvim\n"), None);
    }

    #[test]
    fn command_label_is_the_basename_of_the_first_word() {
        assert_eq!(command_label("nvim"), "nvim");
        assert_eq!(command_label("code --wait"), "code");
        assert_eq!(command_label("/opt/homebrew/bin/nvim -p"), "nvim");
    }

    #[test]
    fn terminal_launch_script_escapes_the_folder_target_and_command() {
        let folder = Path::new("/Users/me/My Skills/it's mine");
        let script = terminal_launch_script(folder, "sub's dir/$(x).md", "code --wait");
        assert!(script.contains("cd -- '/Users/me/My Skills/it'\\''s mine' || exit 1"));
        assert!(script.contains("ED='code --wait'"));
        assert!(script.starts_with("#!/bin/sh\n"));
        // The target lives in its own single-quoted variable, referenced as
        // `"$T"` rather than interpolated straight into the `eval` string -
        // otherwise a `$(...)` or backtick in a file name would expand before
        // `eval` runs.
        assert!(script.contains("T='sub'\\''s dir/$(x).md'"));
        assert!(script.contains("eval \"exec $ED \\\"$T\\\"\""));
        // The literal `$(x)` must appear only inside the single-quoted `T='…'`
        // assignment, never inside the `eval` line itself.
        let eval_line = script
            .lines()
            .find(|line| line.starts_with("eval "))
            .expect("eval line");
        assert!(!eval_line.contains("$(x)"));
    }

    #[test]
    fn terminal_launch_target_for_a_folder_is_dot() {
        let home = tempfile::tempdir().expect("temp home");
        let (dir, target) = terminal_launch_target(home.path());
        assert_eq!(dir, home.path());
        assert_eq!(target, ".");
    }

    #[test]
    fn terminal_launch_target_for_a_file_cds_into_its_parent() {
        let home = tempfile::tempdir().expect("temp home");
        let file = home.path().join("SKILL.md");
        std::fs::write(&file, "content").expect("file");
        let (dir, target) = terminal_launch_target(&file);
        assert_eq!(dir, home.path());
        assert_eq!(target, "SKILL.md");
    }

    #[test]
    fn login_shell_probe_drops_an_inherited_editor() {
        let probe = login_shell_probe("/bin/zsh");
        let removed: Vec<_> = probe
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_os_string())
            .collect();
        assert!(removed.contains(&"VISUAL".into()), "{removed:?}");
        assert!(removed.contains(&"EDITOR".into()), "{removed:?}");
    }

    #[test]
    fn run_with_timeout_stops_at_the_end_marker_even_with_stdout_held_open() {
        // The trailing `sleep 10 &` simulates an rc file that leaves a
        // background process (ssh-agent, gpg-agent, ...) attached to the
        // same stdout pipe: the shell itself exits immediately, but the pipe's
        // write end stays open, so waiting for end-of-file would hang.
        let script = format!(
            "echo {MARKER_START}; echo VISUAL=nvim; echo EDITOR=vi; echo {MARKER_END}; sleep 10 &"
        );
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(script);
        let start = std::time::Instant::now();
        let output = run_with_timeout(command, MARKER_END, Duration::from_secs(5));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        let output = output.expect("markers were printed before the hang");
        assert_eq!(parse_terminal_editor(&output), Some("nvim".to_string()));
    }

    #[test]
    fn run_with_timeout_gives_up_after_the_timeout() {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg("sleep 10");
        let start = std::time::Instant::now();
        let output = run_with_timeout(command, MARKER_END, Duration::from_millis(200));
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(output, None);
    }
}
