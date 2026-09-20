//! `Dotagents`/`SkillsSh`'s half of `ops_install`: the argv/cwd each CLI
//! wants (see [`cli_args_and_cwd`]) and the spawner call that runs it (see
//! [`install_via_cli`]), split out of `ops_install.rs` so that file stays
//! under the crate's line-count convention - re-exported from `ops_install`,
//! so every other caller's path is unchanged.

use std::path::{Path, PathBuf};

use crate::dto::{InstallMethod, InstallRequest};
use crate::error::{CoreError, ErrorCode};
use crate::identity::{AgentId, RootScope, SkillName, UNIVERSAL_ROOT_RELATIVE};
use crate::ports::{FileKind, OpContext, ProcessSpec, Runtime};

/// The `npx` package an [`InstallMethod`] shells out to, or `None` for
/// `Copy` (which never calls `npx`). A plain lookup rather than an
/// `unreachable!` arm, so a caller that mismatches method and code path
/// gets a typed error instead of a panic.
fn cli_package(method: InstallMethod) -> Option<&'static str> {
    match method {
        InstallMethod::Dotagents => Some("@sentry/dotagents"),
        InstallMethod::SkillsSh => Some("skills"),
        InstallMethod::Copy => None,
    }
}

/// Builds the argv `install_via_cli` hands the spawner, and the process cwd
/// to run it in - ported from the desktop's own builders: skills.sh from
/// `skill_install_plan.rs`'s `skills_sh_universal_add_args` (`npx skills add
/// <source> --yes --global [--skill <n>] --agent universal [--agent
/// claude-code]`, per the request's chosen harnesses; `skills@1.7.0` has no
/// `--cwd` flag, so a project scope runs the process itself with its cwd set
/// to the project path instead - PR #101's fix, ported here), and dotagents
/// from `skill_add.rs`'s `add_via_dotagents` (`npx -y @sentry/dotagents
/// [--project] add <source> [--name <n>]`, also with the process cwd set to
/// the project path for a project scope).
fn cli_args_and_cwd(
    method: InstallMethod,
    source: &str,
    skill: &SkillName,
    scope: &RootScope,
    harnesses: &[AgentId],
) -> (Vec<String>, Option<PathBuf>) {
    match method {
        InstallMethod::SkillsSh => {
            let mut args = vec![
                "skills".to_string(),
                "add".to_string(),
                source.to_string(),
                "--yes".to_string(),
            ];
            let cwd = match scope {
                RootScope::Global => {
                    args.push("--global".to_string());
                    None
                }
                RootScope::Project(project) => Some(project.0.clone()),
            };
            args.push("--skill".to_string());
            args.push(skill.0.clone());
            args.push("--agent".to_string());
            args.push("universal".to_string());
            if harnesses.iter().any(|h| h.as_str() == AgentId::CLAUDE_CODE) {
                args.push("--agent".to_string());
                args.push("claude-code".to_string());
            }
            (args, cwd)
        }
        InstallMethod::Dotagents => {
            let mut args = vec!["-y".to_string(), "@sentry/dotagents".to_string()];
            let cwd = match scope {
                RootScope::Global => None,
                RootScope::Project(project) => {
                    args.push("--project".to_string());
                    Some(project.0.clone())
                }
            };
            args.push("add".to_string());
            args.push(source.to_string());
            args.push("--name".to_string());
            args.push(skill.0.clone());
            (args, cwd)
        }
        InstallMethod::Copy => (Vec::new(), None),
    }
}

/// A project scope becomes the spawned process's cwd (`cli_args_and_cwd`),
/// so a missing or non-directory project path must fail before the spawn,
/// with a message that names it - otherwise it surfaces later as an opaque
/// "npx: no such file or directory" from the shell itself. Called from
/// `ops_install::install` before `ensure_dir_all` runs: that call's own
/// `mkdir -p` on `<project>/.agents/skills` would otherwise silently create
/// a missing project directory as a side effect, masking the very fault
/// this check exists to catch.
pub(crate) fn validate_cli_project_path(
    rt: &Runtime,
    req: &InstallRequest,
) -> Result<(), CoreError> {
    if cli_package(req.method).is_none() {
        return Ok(());
    }
    let RootScope::Project(project) = &req.scope else {
        return Ok(());
    };
    let is_dir = rt
        .ports
        .fs
        .canonicalize(&project.0)
        .ok()
        .and_then(|resolved| rt.ports.fs.symlink_metadata(&resolved).ok())
        .is_some_and(|facts| facts.kind == FileKind::Dir);
    if !is_dir {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!(
                "the project path does not exist or is not a directory: {}",
                project.0.display()
            ),
        )
        .at(&project.0));
    }
    Ok(())
}

/// `Dotagents`/`SkillsSh`: runs `req.method`'s argv (see
/// [`cli_args_and_cwd`]) through the process-spawner port and checks the
/// destination now exists. The CLI writes its own files directly - see
/// `ops_install`'s module doc for why this op does not stage-and-swap them.
pub(crate) fn install_via_cli(
    rt: &Runtime,
    ctx: &OpContext,
    req: &InstallRequest,
    destination: &Path,
) -> Result<(), CoreError> {
    let Some(source) = req.source.as_deref() else {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "a dotagents/skills.sh install needs a source",
        ));
    };
    if cli_package(req.method).is_none() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "install_via_cli is never called for Copy",
        ));
    }
    let spawner = rt.ports.spawner.as_ref().ok_or_else(|| {
        CoreError::new(
            ErrorCode::Unsupported,
            "this host build has no process spawner; dotagents/skills.sh installs are not available",
        )
    })?;
    let (args, cwd) = cli_args_and_cwd(req.method, source, &req.skill, &req.scope, &req.harnesses);
    // For a project-scope install, `home_fallback` is where a `--cwd`-less
    // `npx skills add` (this op's own former bug, or a spawner that quietly
    // drops `cwd`) would land the skill instead of the project - recorded
    // before the spawn so the destination-missing check below can tell
    // "landed at the fallback" from "just failed".
    let home_fallback = match &req.scope {
        RootScope::Project(_) => Some(
            rt.scope
                .home
                .lexical
                .join(UNIVERSAL_ROOT_RELATIVE)
                .join(&req.skill.0),
        ),
        RootScope::Global => None,
    };
    let home_fallback_existed_before = home_fallback
        .as_deref()
        .is_some_and(|path| rt.ports.fs.symlink_metadata(path).is_ok());
    let spec = ProcessSpec {
        program: "npx".to_string(),
        args,
        cwd,
        env: Vec::new(),
        timeout_ms: 120_000,
    };
    let output = spawner.run(&spec, ctx.cancel.as_ref())?;
    if output.status != Some(0) {
        return Err(CoreError::new(
            ErrorCode::Io,
            format!("npx exited with {:?}: {}", output.status, output.stderr),
        ));
    }
    if rt.ports.fs.symlink_metadata(destination).is_err() {
        if let Some(fallback) = &home_fallback {
            let fallback_now_exists = rt.ports.fs.symlink_metadata(fallback).is_ok();
            if fallback_now_exists && !home_fallback_existed_before {
                return Err(CoreError::new(
                    ErrorCode::Io,
                    format!(
                        "the CLI installed to {} instead of the project",
                        fallback.display()
                    ),
                )
                .at(destination));
            }
        }
        return Err(CoreError::new(
            ErrorCode::Io,
            "the CLI did not create the expected destination",
        )
        .at(destination));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::ProjectRef;

    /// `cli_args_and_cwd_matches_the_fixed_desktop_builders_or_names_the_drifted_argv`
    /// (R5): table test over {global, project} x {`SkillsSh`, `Dotagents`} x
    /// {no harnesses, Claude Code harness} - nothing else in this crate
    /// references `cli_args_and_cwd`, so a drift from the desktop's own
    /// builders (`skill_install_plan.rs:36-62` for skills.sh,
    /// `skill_add.rs:391-399` for dotagents) would otherwise go unnoticed
    /// until a real `npx` call failed. Named "fixed" rather than "verbatim":
    /// the desktop's own skills.sh builder still emits a nonexistent `--cwd`
    /// flag (see PR #101 / `fix/project-install-runs-in-project-dir`), so
    /// this table pins the corrected argv, not the desktop's as-is one.
    #[test]
    fn cli_args_and_cwd_matches_the_fixed_desktop_builders_or_names_the_drifted_argv() {
        let skill = SkillName("alpha".to_string());
        let none: Vec<AgentId> = Vec::new();
        let claude_code = vec![AgentId::from(AgentId::CLAUDE_CODE)];
        let project = RootScope::Project(ProjectRef(PathBuf::from("/proj")));

        // R5's own drift check, not a domain type anything else needs -
        // named here purely to satisfy clippy's `type_complexity`.
        type Case<'a> = (
            &'a str,
            InstallMethod,
            &'a RootScope,
            &'a [AgentId],
            Vec<&'a str>,
            Option<PathBuf>,
        );
        let cases: Vec<Case> = vec![
            (
                "skills.sh global, no harnesses",
                InstallMethod::SkillsSh,
                &RootScope::Global,
                &none,
                vec![
                    "skills",
                    "add",
                    "src",
                    "--yes",
                    "--global",
                    "--skill",
                    "alpha",
                    "--agent",
                    "universal",
                ],
                None,
            ),
            (
                "skills.sh global, claude code",
                InstallMethod::SkillsSh,
                &RootScope::Global,
                &claude_code,
                vec![
                    "skills",
                    "add",
                    "src",
                    "--yes",
                    "--global",
                    "--skill",
                    "alpha",
                    "--agent",
                    "universal",
                    "--agent",
                    "claude-code",
                ],
                None,
            ),
            (
                "skills.sh project, no harnesses",
                InstallMethod::SkillsSh,
                &project,
                &none,
                vec![
                    "skills",
                    "add",
                    "src",
                    "--yes",
                    "--skill",
                    "alpha",
                    "--agent",
                    "universal",
                ],
                Some(PathBuf::from("/proj")),
            ),
            (
                "skills.sh project, claude code",
                InstallMethod::SkillsSh,
                &project,
                &claude_code,
                vec![
                    "skills",
                    "add",
                    "src",
                    "--yes",
                    "--skill",
                    "alpha",
                    "--agent",
                    "universal",
                    "--agent",
                    "claude-code",
                ],
                Some(PathBuf::from("/proj")),
            ),
            (
                "dotagents global, no harnesses",
                InstallMethod::Dotagents,
                &RootScope::Global,
                &none,
                vec!["-y", "@sentry/dotagents", "add", "src", "--name", "alpha"],
                None,
            ),
            (
                "dotagents global, claude code",
                InstallMethod::Dotagents,
                &RootScope::Global,
                &claude_code,
                vec!["-y", "@sentry/dotagents", "add", "src", "--name", "alpha"],
                None,
            ),
            (
                "dotagents project, no harnesses",
                InstallMethod::Dotagents,
                &project,
                &none,
                vec![
                    "-y",
                    "@sentry/dotagents",
                    "--project",
                    "add",
                    "src",
                    "--name",
                    "alpha",
                ],
                Some(PathBuf::from("/proj")),
            ),
            (
                "dotagents project, claude code",
                InstallMethod::Dotagents,
                &project,
                &claude_code,
                vec![
                    "-y",
                    "@sentry/dotagents",
                    "--project",
                    "add",
                    "src",
                    "--name",
                    "alpha",
                ],
                Some(PathBuf::from("/proj")),
            ),
        ];

        for (label, method, scope, harnesses, expected_args, expected_cwd) in cases {
            let (args, cwd) = cli_args_and_cwd(method, "src", &skill, scope, harnesses);
            let expected_args: Vec<String> = expected_args.into_iter().map(String::from).collect();
            assert_eq!(args, expected_args, "{label}: argv");
            assert_eq!(cwd, expected_cwd, "{label}: cwd");
        }
    }
}
