// ============================================================================
// Skills Module - skill_install_plan
// Turns one explicit Scope × Destination install request into CLI argv and
// copy targets. Universal skills.sh uses `--agent universal` and may also
// pass `--agent claude-code` for a scoped Claude link. Codex is never used
// as Universal transport. Per harness is Copy-only.
// ============================================================================

use serde::{Deserialize, Serialize};

use super::agents::AgentId;
use super::skill_deployment::{SkillDestination, PER_HARNESS_INSTALL_TARGETS};
use super::skill_dto::InstallScope;
use super::skill_fork_registry::AddMethod;

/// One install request used by Add by source and SkillStore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInstallSpec {
    pub scope: InstallScope,
    pub destination: SkillDestination,
    pub project_path: Option<String>,
    /// Harnesses that receive an independent copy (Per harness) or a Claude
    /// Code link (Universal). Empty Universal still writes `.agents/skills`.
    pub harnesses: Vec<AgentId>,
}

/// Universal skills.sh argv. Never includes Codex as a proxy for Universal.
pub fn skills_sh_universal_add_args(
    repo_source: &str,
    skill_name: Option<&str>,
    spec: &SkillInstallSpec,
) -> Result<Vec<String>, String> {
    if spec.destination != SkillDestination::Universal {
        return Err("skills.sh Universal argv is only for the Universal destination".to_string());
    }
    let mut args = vec![
        "skills".to_string(),
        "add".to_string(),
        repo_source.to_string(),
        "--yes".to_string(),
    ];
    match spec.scope {
        InstallScope::Global => args.push("--global".to_string()),
        InstallScope::Project => {
            let path = spec
                .project_path
                .as_deref()
                .ok_or("Project scope needs a project path")?;
            args.push("--cwd".to_string());
            args.push(path.to_string());
        }
    }
    if let Some(name) = skill_name {
        args.push("--skill".to_string());
        args.push(name.to_string());
    }
    args.push("--agent".to_string());
    args.push("universal".to_string());
    if spec.harnesses.contains(&AgentId::ClaudeCode) {
        args.push("--agent".to_string());
        args.push("claude-code".to_string());
    }
    Ok(args)
}

/// Harnesses a Per-harness Copy may write. Unknown or unsupported ids fail.
pub fn per_harness_copy_targets(spec: &SkillInstallSpec) -> Result<Vec<AgentId>, String> {
    if spec.destination != SkillDestination::PerHarness {
        return Err(
            "Per-harness copy targets are only for the Per harness destination".to_string(),
        );
    }
    if spec.harnesses.is_empty() {
        return Err("Per harness needs at least one selected harness".to_string());
    }
    let mut out = Vec::new();
    for agent in &spec.harnesses {
        let cli = agent.cli_name();
        if !PER_HARNESS_INSTALL_TARGETS.contains(&cli) {
            return Err(format!(
                "{} is not a selectable Per harness target",
                agent.display_name()
            ));
        }
        out.push(*agent);
    }
    Ok(out)
}

/// Method allowed for this destination. Per harness is Copy-only.
pub fn allowed_method(
    destination: SkillDestination,
    method: AddMethod,
) -> Result<AddMethod, String> {
    match destination {
        SkillDestination::Universal => Ok(method),
        SkillDestination::PerHarness => {
            if method == AddMethod::Copy {
                Ok(method)
            } else {
                Err("Per harness installation is Copy-only".to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn universal_global() -> SkillInstallSpec {
        SkillInstallSpec {
            scope: InstallScope::Global,
            destination: SkillDestination::Universal,
            project_path: None,
            harnesses: vec![],
        }
    }

    #[test]
    fn universal_skills_sh_uses_agent_universal_and_global() {
        let argv =
            skills_sh_universal_add_args("o/r", Some("find-bugs"), &universal_global()).unwrap();
        assert_eq!(
            argv,
            vec![
                "skills",
                "add",
                "o/r",
                "--yes",
                "--global",
                "--skill",
                "find-bugs",
                "--agent",
                "universal",
            ]
        );
        assert!(!argv.iter().any(|a| a == "codex"));
    }

    #[test]
    fn universal_skills_sh_may_add_claude_code_not_codex() {
        let mut spec = universal_global();
        spec.harnesses = vec![AgentId::ClaudeCode];
        let argv = skills_sh_universal_add_args("o/r", None, &spec).unwrap();
        assert!(argv.windows(2).any(|w| w == ["--agent", "universal"]));
        assert!(argv.windows(2).any(|w| w == ["--agent", "claude-code"]));
        assert!(!argv.iter().any(|a| a == "codex"));
    }

    #[test]
    fn universal_skills_sh_ignores_direct_readers() {
        let mut spec = universal_global();
        spec.harnesses = vec![AgentId::Codex];
        let argv = skills_sh_universal_add_args("o/r", None, &spec).unwrap();
        assert!(!argv.iter().any(|arg| arg == "codex"));
    }

    #[test]
    fn project_universal_uses_cwd_not_global() {
        let spec = SkillInstallSpec {
            scope: InstallScope::Project,
            destination: SkillDestination::Universal,
            project_path: Some("/work/app".to_string()),
            harnesses: vec![],
        };
        let argv = skills_sh_universal_add_args("o/r", None, &spec).unwrap();
        assert!(argv.contains(&"--cwd".to_string()));
        assert!(argv.contains(&"/work/app".to_string()));
        assert!(!argv.contains(&"--global".to_string()));
    }

    #[test]
    fn per_harness_is_copy_only() {
        assert!(allowed_method(SkillDestination::PerHarness, AddMethod::Copy).is_ok());
        let err = allowed_method(SkillDestination::PerHarness, AddMethod::SkillsSh).unwrap_err();
        assert!(err.contains("Copy-only"));
    }

    #[test]
    fn per_harness_accepts_all_selectable_harnesses() {
        let spec = SkillInstallSpec {
            scope: InstallScope::Global,
            destination: SkillDestination::PerHarness,
            project_path: None,
            harnesses: vec![
                AgentId::ClaudeCode,
                AgentId::Codex,
                AgentId::OpenCode,
                AgentId::Pi,
                AgentId::Cursor,
                AgentId::GrokBuild,
            ],
        };
        let targets = per_harness_copy_targets(&spec).unwrap();
        assert_eq!(targets.len(), 6);
    }

    #[test]
    fn per_harness_rejects_empty_and_unknown() {
        let empty = SkillInstallSpec {
            scope: InstallScope::Global,
            destination: SkillDestination::PerHarness,
            project_path: None,
            harnesses: vec![],
        };
        assert!(per_harness_copy_targets(&empty).is_err());
        let unknown = SkillInstallSpec {
            scope: InstallScope::Global,
            destination: SkillDestination::PerHarness,
            project_path: None,
            harnesses: vec![AgentId::Amp],
        };
        assert!(per_harness_copy_targets(&unknown).is_err());
    }
}
