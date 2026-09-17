// ============================================================================
// Skills Module - skill_ownership
// Resolves skills.sh and dotagents ownership against the matching
// scope/root/ledger entry. Same-named deployments elsewhere do not inherit
// ownership. Aggregate grouping by skill name stays presentation-only.
// ============================================================================

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::dotagents_ledger::{self, DotagentsSkill};
use super::lock_file::{self, SkillLockFile};
use super::skill_deployment::encode_id_path;
use super::skill_dto::InstallScope;

/// The owner allowed to change a deployment. Read-only kinds use `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LifecycleOwnerKind {
    SkillsSh,
    Dotagents,
    Copy,
    Fork,
    Plugin,
    InRepo,
    #[default]
    Manual,
    WildcardDotagents,
    Ambiguous,
}

impl LifecycleOwnerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SkillsSh => "skills-sh",
            Self::Dotagents => "dotagents",
            Self::Copy => "copy",
            Self::Fork => "fork",
            Self::Plugin => "plugin",
            Self::InRepo => "in-repo",
            Self::Manual => "manual",
            Self::WildcardDotagents => "wildcard-dotagents",
            Self::Ambiguous => "ambiguous",
        }
    }

    /// True when Skill Studio may run an owner adapter against this kind.
    pub fn is_mutable(self) -> bool {
        matches!(
            self,
            Self::SkillsSh | Self::Dotagents | Self::Copy | Self::Fork
        )
    }
}

/// One ledger that can own Universal deployments in a given `.agents` root.
#[derive(Debug, Clone)]
pub struct OwnershipLedgers {
    pub agents_dir: PathBuf,
    pub scope: InstallScope,
    pub project_path: Option<PathBuf>,
    pub lock: SkillLockFile,
    pub dotagents: Vec<DotagentsSkill>,
}

/// Load the home Universal ledger (`~/.agents`) plus one ledger per project
/// that has `.agents/agents.toml`, `agents.lock`, or `.skill-lock.json`.
pub fn load_ownership_ledgers(home: &Path, project_paths: &[PathBuf]) -> Vec<OwnershipLedgers> {
    let mut out = Vec::new();
    out.push(read_ledgers(
        home.join(".agents"),
        InstallScope::Global,
        None,
    ));
    for project in project_paths {
        let agents_dir = project.join(".agents");
        if agents_dir.join("agents.toml").exists()
            || agents_dir.join("agents.lock").exists()
            || agents_dir.join(".skill-lock.json").exists()
        {
            out.push(read_ledgers(
                agents_dir,
                InstallScope::Project,
                Some(project.clone()),
            ));
        }
    }
    out
}

fn read_ledgers(
    agents_dir: PathBuf,
    scope: InstallScope,
    project_path: Option<PathBuf>,
) -> OwnershipLedgers {
    let lock = lock_file::read_lock_file_at(&agents_dir.join(".skill-lock.json")).unwrap_or(
        SkillLockFile {
            version: 3,
            skills: HashMap::new(),
        },
    );
    let dotagents = dotagents_ledger::read_dotagents_ledger(&agents_dir).unwrap_or_default();
    OwnershipLedgers {
        agents_dir,
        scope,
        project_path,
        lock,
        dotagents,
    }
}

pub fn owner_id_for(ledger: &OwnershipLedgers, name: &str) -> String {
    match (&ledger.scope, &ledger.project_path) {
        (InstallScope::Global, _) => format!("owner:v1/global/{name}"),
        (InstallScope::Project, Some(path)) => {
            format!(
                "owner:v1/project/{}/{}",
                encode_id_path(&path.to_string_lossy()),
                name
            )
        }
        (InstallScope::Project, None) => format!("owner:v1/project/-/{name}"),
    }
}

/// Parse `owner:v1/global/<name>` or `owner:v1/project/<encoded>/<name>`.
pub fn parse_owner_id(id: &str) -> Option<ParsedOwnerId> {
    let rest = id.strip_prefix("owner:v1/")?;
    if let Some(name) = rest.strip_prefix("global/") {
        return Some(ParsedOwnerId {
            scope: InstallScope::Global,
            project_path: None,
            name: name.to_string(),
        });
    }
    let rest = rest.strip_prefix("project/")?;
    let (encoded, name) = rest.rsplit_once('/')?;
    let project_path = if encoded == "-" {
        None
    } else {
        Some(encoded.replace("%2F", "/").replace("%25", "%"))
    };
    Some(ParsedOwnerId {
        scope: InstallScope::Project,
        project_path,
        name: name.to_string(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedOwnerId {
    pub scope: InstallScope,
    pub project_path: Option<String>,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_id_round_trips_project() {
        let parsed = parse_owner_id("owner:v1/project/%2Fwork%2Fapp/find-bugs").unwrap();
        assert_eq!(parsed.scope, InstallScope::Project);
        assert_eq!(parsed.project_path.as_deref(), Some("/work/app"));
        assert_eq!(parsed.name, "find-bugs");
    }
}
