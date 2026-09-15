use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use skill_studio_core::skill_service::SkillScope;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct DesktopScopeConfig {
    backing_roots: Vec<PathBuf>,
    plugin_ownership_roots: Vec<PathBuf>,
}

const CONFIG_FILE: &str = "skill-studio-scope.json";
const CONFIG_LIMIT: u64 = 64 * 1024;
static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize)]
pub struct DesktopScopeSettings {
    config: DesktopScopeConfig,
    environment_override: bool,
}

fn environment_config() -> Result<Option<String>, String> {
    match std::env::var("SKILL_STUDIO_SCOPE") {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err("SKILL_STUDIO_SCOPE must be UTF-8 JSON".into())
        }
    }
}

fn read_settings(home: &Path, environment: Option<&str>) -> Result<DesktopScopeSettings, String> {
    let text = if let Some(value) = environment {
        Some(value.to_string())
    } else {
        match std::fs::File::open(home.join(".agents").join(CONFIG_FILE)) {
            Ok(file) => {
                let mut text = String::new();
                file.take(CONFIG_LIMIT + 1)
                    .read_to_string(&mut text)
                    .map_err(|error| format!("Couldn't read skill folder settings: {error}"))?;
                Some(text)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("Couldn't open skill folder settings: {error}")),
        }
    };
    let scope = configured_scope(home, &[], text.as_deref())?;
    Ok(DesktopScopeSettings {
        config: DesktopScopeConfig {
            backing_roots: scope.backing_roots,
            plugin_ownership_roots: scope.plugin_ownership_roots,
        },
        environment_override: environment.is_some(),
    })
}

pub(crate) fn desktop_skill_scope(home: &Path, projects: &[PathBuf]) -> Result<SkillScope, String> {
    let settings = read_settings(home, environment_config()?.as_deref())?;
    Ok(SkillScope {
        home: home.to_path_buf(),
        projects: projects
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        backing_roots: settings.config.backing_roots,
        plugin_ownership_roots: settings.config.plugin_ownership_roots,
    })
}

fn save_settings(
    home: &Path,
    config: DesktopScopeConfig,
    environment: Option<&str>,
) -> Result<(), String> {
    if environment.is_some() {
        return Err("Folder settings are controlled by SKILL_STUDIO_SCOPE. Remove that override and restart to edit them here.".into());
    }
    let config = DesktopScopeConfig {
        backing_roots: validated_roots(config.backing_roots)?,
        plugin_ownership_roots: validated_roots(config.plugin_ownership_roots)?,
    };
    for path in config
        .backing_roots
        .iter()
        .chain(&config.plugin_ownership_roots)
    {
        let physical = std::fs::canonicalize(path)
            .map_err(|error| format!("Couldn't open {}: {error}", path.display()))?;
        if !physical.is_dir() || physical.parent().is_none() {
            return Err(format!(
                "Choose a folder below the filesystem root: {}",
                path.display()
            ));
        }
    }
    let bytes = serde_json::to_vec_pretty(&config).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > CONFIG_LIMIT {
        return Err("Skill folder settings exceed 64 KiB".into());
    }
    let parent = home.join(".agents");
    std::fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
    let counter = WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{CONFIG_FILE}.{}.{}.tmp",
        std::process::id(),
        counter
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("Couldn't prepare folder settings: {error}"))?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, parent.join(CONFIG_FILE))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|error| format!("Couldn't save folder settings: {error}"))
}

#[tauri::command]
pub async fn get_skill_scope_settings() -> Result<DesktopScopeSettings, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        read_settings(&home, environment_config()?.as_deref())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn set_skill_scope_settings(
    app: tauri::AppHandle,
    config: DesktopScopeConfig,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        save_settings(&home, config, environment_config()?.as_deref())
    })
    .await
    .map_err(|error| error.to_string())??;
    super::skill_refresh::request_snapshot_rebuild(&app);
    Ok(())
}

fn validated_roots(roots: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    if roots.len() > 64
        || roots.iter().any(|path| {
            !path.is_absolute()
                || path.parent().is_none()
                || path
                    .to_str()
                    .is_none_or(|text| text.is_empty() || text.contains('\0'))
                || path
                    .components()
                    .any(|component| matches!(component, Component::ParentDir))
        })
    {
        return Err("Desktop scope permits at most 64 absolute non-root paths per list, without parent traversal".into());
    }
    Ok(roots
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

pub(crate) fn configured_scope(
    home: &Path,
    projects: &[PathBuf],
    config: Option<&str>,
) -> Result<SkillScope, String> {
    let config = match config {
        Some(value) => {
            if value.len() > 64 * 1024 {
                return Err("Skill folder settings exceed 64 KiB".into());
            }
            let object: serde_json::Value = serde_json::from_str(value)
                .map_err(|_| "Skill folder settings must be a JSON object")?;
            if !object.is_object() {
                return Err("Skill folder settings must be a JSON object".into());
            }
            serde_json::from_value::<DesktopScopeConfig>(object).map_err(|_| "Skill folder settings must contain only backing_roots and plugin_ownership_roots arrays")?
        }
        None => DesktopScopeConfig::default(),
    };
    Ok(SkillScope {
        home: home.to_path_buf(),
        projects: projects
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        backing_roots: validated_roots(config.backing_roots)?,
        plugin_ownership_roots: validated_roots(config.plugin_ownership_roots)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use skill_studio_core::{
        skill_ownership::LifecycleOwnerKind, skill_service::ScopedSkillService,
    };

    #[test]
    fn saved_folders_round_trip_remove_and_respect_environment_override() {
        let home = tempfile::tempdir().unwrap();
        let backing = tempfile::tempdir().unwrap();
        let owner = tempfile::tempdir().unwrap();
        assert_eq!(
            read_settings(home.path(), None).unwrap().config,
            DesktopScopeConfig::default()
        );
        let config = DesktopScopeConfig {
            backing_roots: vec![backing.path().to_path_buf()],
            plugin_ownership_roots: vec![owner.path().to_path_buf()],
        };
        save_settings(home.path(), config.clone(), None).unwrap();
        assert_eq!(read_settings(home.path(), None).unwrap().config, config);
        let overridden = read_settings(home.path(), Some("{}")).unwrap();
        assert!(overridden.environment_override);
        assert_eq!(overridden.config, DesktopScopeConfig::default());
        assert!(save_settings(home.path(), DesktopScopeConfig::default(), Some("{}")).is_err());
        assert_eq!(read_settings(home.path(), None).unwrap().config, config);
        save_settings(home.path(), DesktopScopeConfig::default(), None).unwrap();
        assert_eq!(
            read_settings(home.path(), None).unwrap().config,
            DesktopScopeConfig::default()
        );
    }

    #[test]
    fn invalid_folder_save_preserves_previous_settings_and_cleans_temporary_file() {
        let home = tempfile::tempdir().unwrap();
        let original = DesktopScopeConfig::default();
        save_settings(home.path(), original.clone(), None).unwrap();
        for root in [
            PathBuf::from("relative"),
            PathBuf::from("/"),
            home.path().join("missing"),
        ] {
            assert!(save_settings(
                home.path(),
                DesktopScopeConfig {
                    backing_roots: vec![root],
                    plugin_ownership_roots: vec![],
                },
                None
            )
            .is_err());
            assert_eq!(read_settings(home.path(), None).unwrap().config, original);
        }
        let path = home.path().join(".agents").join(CONFIG_FILE);
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("keep"), "unchanged").unwrap();
        assert!(save_settings(home.path(), original, None).is_err());
        assert_eq!(
            std::fs::read_to_string(path.join("keep")).unwrap(),
            "unchanged"
        );
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[test]
    fn malformed_or_oversized_settings_are_not_silently_defaulted() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".agents")).unwrap();
        let path = home.path().join(".agents").join(CONFIG_FILE);
        for contents in ["{".to_string(), " ".repeat(CONFIG_LIMIT as usize + 1)] {
            std::fs::write(&path, contents).unwrap();
            assert!(read_settings(home.path(), None).is_err());
        }
    }

    #[test]
    fn scope_configuration_is_explicit_bounded_and_deduplicated() {
        let default = configured_scope(Path::new("/home"), &[], None).unwrap();
        assert!(default.backing_roots.is_empty());
        assert!(default.plugin_ownership_roots.is_empty());
        let scope = configured_scope(
            Path::new("/home"),
            &[],
            Some(r#"{"backing_roots":["/data","/data"],"plugin_ownership_roots":["/plugins"]}"#),
        )
        .unwrap();
        assert_eq!(scope.backing_roots, vec![PathBuf::from("/data")]);
        assert_eq!(
            scope.plugin_ownership_roots,
            vec![PathBuf::from("/plugins")]
        );
        for config in [
            r#"{"home":"/other"}"#,
            r#"{"backing_roots":["relative"]}"#,
            r#"{"plugin_ownership_roots":["/"]}"#,
            r#"{"backing_roots":["/data/../other"]}"#,
            r#"{"plugin_ownership_roots":["/bad\u0000"]}"#,
            "null",
            "[]",
        ] {
            assert!(configured_scope(Path::new("/home"), &[], Some(config)).is_err());
        }
        let many = serde_json::json!({"backing_roots": vec!["/data";65]}).to_string();
        assert!(configured_scope(Path::new("/home"), &[], Some(&many)).is_err());
        assert!(configured_scope(Path::new("/home"), &[], Some(&" ".repeat(65537))).is_err());
    }

    #[test]
    fn local_document_edits_do_not_require_git_ancestry_outside_home() {
        use skill_studio_core::skill_provenance::SourceKind;
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let skill = home.join(".agents/skills/local");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: local\ndescription: local fixture\n---\nbody\n",
        )
        .unwrap();
        // A repository above HOME must neither be read nor reported absent.
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        for failed_ledger in [false, true] {
            if failed_ledger {
                std::fs::write(home.join(".agents/.skill-lock.json"), "{").unwrap();
            }
            let mut service =
                ScopedSkillService::bind(configured_scope(&home, &[], None).unwrap()).unwrap();
            let inventory = service
                .scan(None, Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let found = inventory
                .skills
                .iter()
                .find(|skill| skill.name == "local")
                .unwrap();
            let deployment = &found.deployments[0];
            assert_eq!(found.source_kind, SourceKind::Unknown);
            assert_eq!(
                deployment.owner_kind,
                if failed_ledger {
                    LifecycleOwnerKind::Unknown
                } else {
                    LifecycleOwnerKind::Manual
                }
            );
            assert_eq!(
                super::super::commands::check_skill_md_deployment_write_allowed(deployment).is_ok(),
                !failed_ledger
            );
            assert!(
                !deployment.owner_kind.is_mutable(),
                "local editing must not enable manager actions"
            );
        }
    }

    #[test]
    fn explicit_ownership_root_enables_external_project_without_hiding_plugins() {
        for plugin in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let owner = temp.path().join("owner");
            let project = owner.join("project");
            let skill = project.join(".codex/skills/sample");
            std::fs::create_dir(&home).unwrap();
            std::fs::create_dir_all(&skill).unwrap();
            std::fs::create_dir(project.join(".git")).unwrap();
            std::fs::write(
                skill.join("SKILL.md"),
                "---\nname: sample\ndescription: fixture\n---\nbody\n",
            )
            .unwrap();
            if plugin {
                std::fs::create_dir(owner.join(".claude-plugin")).unwrap();
                std::fs::write(
                    owner.join(".claude-plugin/plugin.json"),
                    r#"{"name":"owner-plugin"}"#,
                )
                .unwrap();
            }
            let timeout = Some(std::time::Duration::from_secs(10));
            let mut truncated = ScopedSkillService::bind(
                configured_scope(&home, std::slice::from_ref(&project), None).unwrap(),
            )
            .unwrap();
            let inventory = truncated.scan(None, timeout).unwrap();
            assert_eq!(
                inventory.skills[0].deployments[0].owner_kind,
                LifecycleOwnerKind::Unknown
            );
            let config = serde_json::json!({"plugin_ownership_roots":[owner]}).to_string();
            let mut service = ScopedSkillService::bind(
                configured_scope(&home, std::slice::from_ref(&project), Some(&config)).unwrap(),
            )
            .unwrap();
            let inventory = service.scan(None, timeout).unwrap();
            let deployment = &inventory.skills[0].deployments[0];
            assert_eq!(
                deployment.owner_kind,
                if plugin {
                    LifecycleOwnerKind::Plugin
                } else {
                    LifecycleOwnerKind::InRepo
                }
            );
        }
    }
}
