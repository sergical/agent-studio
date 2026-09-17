use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
};

use skill_studio_core::{
    skill_backup_reservation::{BackupCopyLimits, ReservedManagedSource},
    skill_backup_source::BackupSourceRoot,
    skill_dotagents_ledger::DotagentsReinstallRequest,
    skill_service::CancellationToken,
    skill_unfork_preparation::DotagentsRuntimeRecord,
};

use super::skill_process::{run_controlled_prepared_command_output, AddOperationControl};

const RUNTIME_LIMITS: BackupCopyLimits = BackupCopyLimits {
    max_bytes: 256 * 1024 * 1024,
    max_entries: 100_000,
    max_depth: 32,
};

fn confinement_profile(stage: &Path, cache: &Path) -> Result<String, String> {
    Ok(format!(
        "(version 1)\n(allow default)\n(deny network*)\n(deny file-write*)\n(allow file-write* (literal \"/dev/null\") (subpath {}) (subpath {}))\n",
        serde_json::to_string(stage.to_str().ok_or("Stage path is not UTF-8")?)
            .map_err(|error| error.to_string())?,
        serde_json::to_string(cache.to_str().ok_or("Cache path is not UTF-8")?)
            .map_err(|error| error.to_string())?,
    ))
}

fn ensure_runtime_outside_writes(
    node: &Path,
    modules: &Path,
    stage: &Path,
    cache: &Path,
) -> Result<(), String> {
    for runtime in [node, modules] {
        for writable in [stage, cache] {
            if runtime.starts_with(writable) || writable.starts_with(runtime) {
                return Err("Unfork runtime overlaps its writable stage or cache".into());
            }
        }
    }
    Ok(())
}

/// An explicit, already-provisioned provider runtime. This type does not find,
/// install, update, or persist a runtime.
pub(crate) struct StagedUnforkProvider {
    root: PathBuf,
    node: PathBuf,
    modules: PathBuf,
    record: DotagentsRuntimeRecord,
}

impl StagedUnforkProvider {
    pub(crate) fn record(&self) -> &DotagentsRuntimeRecord {
        &self.record
    }

    pub(crate) fn bind(
        runtime_root: &Path,
        node: &Path,
        record: DotagentsRuntimeRecord,
    ) -> Result<Self, String> {
        let root = runtime_root
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let modules = root.join("node_modules");
        if !root.is_dir() || !modules.is_dir() {
            return Err("Unfork provider runtime root is incomplete".into());
        }
        let node = node.canonicalize().map_err(|error| error.to_string())?;
        if !node.is_file() {
            return Err("Unfork provider Node executable is unavailable".into());
        }
        record.validate()?;
        Ok(Self {
            root,
            node,
            modules,
            record,
        })
    }

    pub(crate) fn verify_materialized_runtime(
        &self,
        control: &AddOperationControl,
    ) -> Result<(), String> {
        self.verify_runtime_bytes(control)?;
        let mut libraries = Command::new("/usr/bin/otool");
        libraries.env_clear().arg("-L").arg(&self.node);
        let libraries = String::from_utf8(
            run_controlled_prepared_command_output(libraries, control, 64 * 1024)
                .map_err(|error| error.into_message())?,
        )
        .map_err(|_| "Node linked-library report is not UTF-8")?;
        let paths: Vec<_> = libraries
            .lines()
            .skip(1)
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                line.trim()
                    .split(" (compatibility version")
                    .next()
                    .unwrap_or_default()
            })
            .collect();
        if paths.is_empty()
            || paths.iter().any(|path| {
                !(path.starts_with("/usr/lib/") || path.starts_with("/System/Library/"))
                    || Path::new(path)
                        .components()
                        .any(|part| part == std::path::Component::ParentDir)
            })
        {
            return Err("Node links to a non-system library".into());
        }
        Ok(())
    }

    fn verify_runtime_bytes(&self, control: &AddOperationControl) -> Result<(), String> {
        control.check().map_err(|error| error.into_message())?;
        let node_parent = self.node.parent().ok_or("Node executable has no parent")?;
        let node_root = BackupSourceRoot::bind(node_parent).map_err(|error| error.to_string())?;
        let modules_root = BackupSourceRoot::bind(&self.root).map_err(|error| error.to_string())?;
        self.record.verify_materialized_bytes(
            &node_root
                .select(self.node.file_name().ok_or("Node executable has no name")?)
                .map_err(|error| error.to_string())?,
            &modules_root
                .select(OsStr::new("node_modules"))
                .map_err(|error| error.to_string())?,
            RUNTIME_LIMITS,
            &CancellationToken::default(),
        )?;
        control.check().map_err(|error| error.into_message())
    }

    /// Runs only the fixed `dotagents --global add` request in an existing
    /// operation stage. Success requires the command's process group to exit;
    /// the resulting source still needs core verification before publication.
    pub(crate) fn run_staged_add(
        &self,
        reservation: &ReservedManagedSource<'_>,
        request: &DotagentsReinstallRequest,
        control: &AddOperationControl,
    ) -> Result<Vec<u8>, String> {
        let mut arguments = vec![
            "--global".into(),
            "add".into(),
            request.source().into(),
            "--name".into(),
            request.name().into(),
        ];
        if let Some(reference) = request.declared_ref() {
            arguments.extend(["--ref".into(), reference.into()]);
        }
        self.run_staged_request(reservation, &arguments, false, control)
    }

    pub(crate) fn skills_sh_record(
        &self,
    ) -> skill_studio_core::skill_unfork_preparation::SkillsShRuntimeRecord {
        skill_studio_core::skill_unfork_preparation::SkillsShRuntimeRecord {
            provider_version: "1.5.25".into(),
            provider_tree_identity: self.record.provider_tree_identity.clone(),
            node_version: self.record.node_version.clone(),
            node_content_digest: self.record.node_content_digest.clone(),
            copy_contract: "skills-1.5.25-global-universal-copy".into(),
        }
    }

    pub(crate) fn run_skills_sh_staged_add(
        &self,
        reservation: &ReservedManagedSource<'_>,
        request: &skill_studio_core::skill_backup_reservation::SkillsShReinstallRequest,
        control: &AddOperationControl,
    ) -> Result<Vec<u8>, String> {
        let arguments = vec![
            "add".into(),
            request.source_argument(),
            "--global".into(),
            "--agent".into(),
            "universal".into(),
            "--skill".into(),
            request.name().into(),
            "--copy".into(),
            "--yes".into(),
            "--full-depth".into(),
        ];
        self.run_staged_request(reservation, &arguments, true, control)
    }

    fn run_staged_request(
        &self,
        reservation: &ReservedManagedSource<'_>,
        arguments: &[String],
        skills_sh: bool,
        control: &AddOperationControl,
    ) -> Result<Vec<u8>, String> {
        reservation
            .revalidate()
            .map_err(|error| error.to_string())?;
        self.verify_materialized_runtime(control)?;
        let stage = reservation
            .stage_path()
            .map_err(|error| error.to_string())?;
        let cache = reservation
            .cache_path()
            .map_err(|error| error.to_string())?;
        ensure_runtime_outside_writes(&self.node, &self.modules, &stage, &cache)?;
        let home = stage.join("home");
        let agents = home.join(".agents");
        for path in [
            &stage,
            &cache,
            &home,
            &agents,
            &stage.join("tmp"),
            &stage.join("config"),
            &stage.join("data"),
            &stage.join("xdg-state"),
            &stage.join("xdg-cache"),
        ] {
            if !path.is_dir() {
                return Err("Unfork provider stage is incomplete".into());
            }
        }
        let profile = confinement_profile(&stage, &cache)?;
        let mut version = self.confined_command(&stage, &cache, &home, &agents, &profile)?;
        version.arg("--version");
        let version = String::from_utf8(
            run_controlled_prepared_command_output(version, control, 64 * 1024)
                .map_err(|error| error.into_message())?,
        )
        .map_err(|_| "Node version report is not UTF-8")?;
        if version.trim() != self.record.node_version {
            return Err("Node reported a version different from the saved runtime".into());
        }
        self.verify_runtime_bytes(control)?;
        let mut command = self.confined_command(&stage, &cache, &home, &agents, &profile)?;
        let entry = if skills_sh {
            let scope = skill_studio_core::skill_scope::SkillReadScope::bind(std::slice::from_ref(
                &self.modules,
            ))
            .map_err(|error| error.to_string())?;
            let package: serde_json::Value = serde_json::from_slice(
                &scope
                    .read(&self.modules.join("skills/package.json"), 64 * 1024)
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            if package["name"] != "skills"
                || package["version"] != "1.5.25"
                || scope
                    .read(&self.modules.join("skills/dist/cli.mjs"), 8 * 1024 * 1024)
                    .map_err(|error| error.to_string())?
                    .is_empty()
            {
                return Err("Packaged skills.sh runtime differs from its fixed contract".into());
            }
            command.env_remove("XDG_STATE_HOME");
            "skills/dist/cli.mjs"
        } else {
            "@sentry/dotagents/dist/cli/index.js"
        };
        command.arg(self.modules.join(entry)).args(arguments);
        let output = run_controlled_prepared_command_output(command, control, 64 * 1024)
            .map_err(|error| error.into_message())?;
        reservation
            .revalidate()
            .map_err(|error| error.to_string())?;
        self.verify_materialized_runtime(control)?;
        Ok(output)
    }

    fn confined_command(
        &self,
        stage: &Path,
        cache: &Path,
        home: &Path,
        agents: &Path,
        profile: &str,
    ) -> Result<Command, String> {
        let node_parent = self.node.parent().ok_or("Node executable has no parent")?;
        let mut command = Command::new("/usr/bin/sandbox-exec");
        command
            .arg("-p")
            .arg(profile)
            .arg(&self.node)
            .env_clear()
            .current_dir(stage)
            .env("PATH", format!("{}:/usr/bin:/bin", node_parent.display()))
            .env("HOME", home)
            .env("DOTAGENTS_HOME", agents)
            .env("DOTAGENTS_STATE_DIR", cache)
            .env("TMPDIR", stage.join("tmp"))
            .env("XDG_CONFIG_HOME", stage.join("config"))
            .env("XDG_DATA_HOME", stage.join("data"))
            .env("XDG_STATE_HOME", stage.join("xdg-state"))
            .env("XDG_CACHE_HOME", stage.join("xdg-cache"))
            .env("CI", "1")
            .env("NO_COLOR", "1")
            .env("LANG", "en_US.UTF-8")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", stage.join("config/gitconfig"))
            .env("GIT_TERMINAL_PROMPT", "0");
        Ok(command)
    }

    #[cfg(test)]
    pub(crate) fn assert_stage_denies_outside_write(
        &self,
        reservation: &ReservedManagedSource<'_>,
        outside: &Path,
        control: &AddOperationControl,
    ) -> Result<(), String> {
        let stage = reservation
            .stage_path()
            .map_err(|error| error.to_string())?;
        let cache = reservation
            .cache_path()
            .map_err(|error| error.to_string())?;
        ensure_runtime_outside_writes(&self.node, &self.modules, &stage, &cache)?;
        let home = stage.join("home");
        let agents = home.join(".agents");
        let profile = confinement_profile(&stage, &cache)?;
        let mut command = self.confined_command(&stage, &cache, &home, &agents, &profile)?;
        command
            .args(["-e", "try { require('node:fs').writeFileSync(process.argv[1], 'changed'); process.exitCode = 1; } catch (e) { if (!['EPERM', 'EACCES'].includes(e.code)) throw e; console.log('WRITE_DENIED'); }"])
            .arg(outside);
        if run_controlled_prepared_command_output(command, control, 64 * 1024)
            .map_err(|error| error.into_message())?
            != b"WRITE_DENIED\n"
        {
            return Err("Unfork provider sandbox did not deny an outside write".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ensure_runtime_outside_writes;
    use std::path::Path;

    #[test]
    fn runtime_cannot_overlap_provider_writes() {
        let node = Path::new("/runtime/node");
        let modules = Path::new("/runtime/node_modules");
        let stage = Path::new("/operation/stage");
        let cache = Path::new("/operation/cache");
        assert!(ensure_runtime_outside_writes(node, modules, stage, cache).is_ok());
        for writable in [stage, cache] {
            assert!(
                ensure_runtime_outside_writes(&writable.join("node"), modules, stage, cache)
                    .is_err()
            );
            assert!(ensure_runtime_outside_writes(
                node,
                &writable.join("node_modules"),
                stage,
                cache
            )
            .is_err());
        }
        assert!(
            ensure_runtime_outside_writes(node, modules, &modules.join("stage"), cache).is_err()
        );
        assert!(ensure_runtime_outside_writes(node, modules, stage, modules).is_err());
        assert!(ensure_runtime_outside_writes(
            node,
            modules,
            Path::new("/runtime/node_modules-other"),
            cache
        )
        .is_ok());
    }
}
