// ============================================================================
// skill_editor - which application "Open in editor" hands a skill folder to.
// macOS's `open -t` means the default *text* editor, which is TextEdit on a
// stock machine no matter how many code editors are installed, so the choice
// has to be made explicitly and remembered.
// ============================================================================

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::skills::skill_fork_registry::{read_fork_registry_or_default, write_fork_registry};

/// One editor the user can pick, as offered to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditorOption {
    /// The macOS application name, without `.app` - what `open -a` takes.
    pub app_name: String,
    /// What the picker shows.
    pub label: String,
}

/// The editors worth offering, in the order the picker lists them. The first
/// field is the `.app` bundle name; the second is its display label, which
/// differs for VS Code (bundle "Visual Studio Code") and the JetBrains IDEs.
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
    ("Xcode", "Xcode"),
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

/// Store the choice in `home`'s registry, checking it against `dirs`. `None`
/// restores the system default. An app name that isn't installed is refused
/// rather than saved, so the setting can never point at something `open -a`
/// will fail on later.
fn set_preferred_editor_in(
    home: &Path,
    dirs: &[PathBuf],
    app_name: Option<String>,
) -> Result<(), String> {
    if let Some(name) = &app_name {
        if !is_installed_in(dirs, name) {
            return Err(format!("{name} is not installed in Applications."));
        }
    }
    let mut registry = read_fork_registry_or_default(home);
    registry.preferred_editor = app_name;
    write_fork_registry(home, &registry)
}

pub fn installed_editors(home: &Path) -> Vec<EditorOption> {
    installed_editors_in(&application_dirs(home))
}

pub fn preferred_editor(home: &Path) -> Option<String> {
    read_fork_registry_or_default(home).preferred_editor
}

pub fn set_preferred_editor(home: &Path, app_name: Option<String>) -> Result<(), String> {
    set_preferred_editor_in(home, &application_dirs(home), app_name)
}

/// The `open` arguments for handing a path to an editor: `-a <App>` for the
/// chosen one, else the first installed known editor, else no flag at all so
/// macOS picks per file type. `-t` is deliberately not used - it means
/// TextEdit on a stock machine and refuses folders outright.
pub fn open_editor_args(preferred: Option<&str>, installed: &[EditorOption]) -> Vec<String> {
    let app = preferred
        .map(str::to_string)
        .or_else(|| installed.first().map(|e| e.app_name.clone()));
    match app {
        Some(app) => vec!["-a".to_string(), app],
        None => Vec::new(),
    }
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
        let err = set_preferred_editor_in(home.path(), &dirs, Some("Cursor".to_string()))
            .expect_err("uninstalled editor must be refused");
        assert!(err.contains("not installed"), "{err}");
        assert_eq!(preferred_editor(home.path()), None);
    }

    #[test]
    fn a_saved_choice_round_trips_and_clears() {
        let home = tempfile::tempdir().expect("temp home");
        let dirs = fake_applications(home.path(), &["Zed"]);
        set_preferred_editor_in(home.path(), &dirs, Some("Zed".to_string())).expect("save");
        assert_eq!(preferred_editor(home.path()), Some("Zed".to_string()));
        set_preferred_editor_in(home.path(), &dirs, None).expect("clear");
        assert_eq!(preferred_editor(home.path()), None);
    }

    #[test]
    fn open_args_prefer_the_choice_then_the_first_installed_editor() {
        let zed = EditorOption {
            app_name: "Zed".into(),
            label: "Zed".into(),
        };
        let installed = vec![zed.clone()];
        assert_eq!(open_editor_args(None, &[]), Vec::<String>::new());
        assert_eq!(
            open_editor_args(None, &installed),
            vec!["-a".to_string(), "Zed".to_string()]
        );
        assert_eq!(
            open_editor_args(Some("Cursor"), &installed),
            vec!["-a".to_string(), "Cursor".to_string()]
        );
    }
}
