//! Where `ops::install` writes for a chosen harness set, following `skills`
//! CLI 1.7.0 (`installSkillForAgent`, `createSymlink`, and the mode choice
//! in `add`):
//!
//! - The shared copy lives at `<scope>/.agents/skills/<name>`. Codex,
//!   `OpenCode`, Cursor, and the `universal` pseudo id read it directly.
//! - Claude Code (`.claude/skills`), pi (`.pi/agent/skills` globally,
//!   `.pi/skills` in a project), and Grok Build (`.grok/skills`) have their
//!   own folder. `Link` puts a relative symlink there; `Copy` a real folder.
//! - When the chosen harnesses name one folder or fewer, `Copy` is forced.
//! - A harness folder that already resolves to the shared folder (a
//!   whole-folder link such as `~/.claude/skills -> ~/.agents/skills`) gets
//!   nothing of its own.
//!
//! One rule differs from the CLI on purpose: the CLI refuses nothing when a
//! harness folder is a whole-folder link to some other folder, and writes
//! through it. `install` refuses, so it never writes into a folder the user
//! keeps elsewhere (for example a dotfiles checkout).
//!
//! The CLI also skips a missing `.pi`/`.grok` project folder only for an
//! agent it detected itself; an agent named with `--agent` gets the folder
//! created. `install` skips it for every request, per the brief for this
//! feature, and reports the skip.
//!
//! [`SkillDestination::PerHarness`] (`Copy` only) is Skill Studio's own
//! choice, not the CLI's: every chosen harness gets a real folder in its own
//! skills folder, Codex, `OpenCode`, and Cursor included, and nothing is
//! written to the shared folder. A missing `.pi`/`.grok` project folder is
//! created, not skipped, since the copy is the only place the skill lands.

use std::path::{Component, Path, PathBuf};

use crate::dto::{InstallLinkMode, InstallMethod};
use crate::error::{CoreError, ErrorCode};
use crate::fsops;
use crate::identity::{AgentId, RootScope, SkillDestination, SkillName, UNIVERSAL_ROOT_RELATIVE};
use crate::ports::{FileKind, ScopeFs};
use crate::scope::NormalizedScope;

/// The `--agent` id for the shared `.agents/skills` folder alone - the
/// `skills` CLI's `universal` pseudo agent.
pub const UNIVERSAL_TARGET: &str = "universal";

/// Every id `install` accepts in `InstallRequest::harnesses`.
pub const INSTALL_TARGET_IDS: &[&str] = &[
    UNIVERSAL_TARGET,
    AgentId::CLAUDE_CODE,
    AgentId::CODEX,
    AgentId::OPEN_CODE,
    AgentId::CURSOR,
    AgentId::PI,
    AgentId::GROK_BUILD,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Folder {
    Shared,
    Own {
        /// Relative to the scope root.
        relative: &'static str,
        /// At project scope, the folder whose absence skips a link.
        project_marker: Option<&'static str>,
    },
}

/// The own folder `PerHarness` gives Codex, `OpenCode`, and Cursor, which
/// otherwise read the shared folder.
fn per_harness_folder(harness: &AgentId, global: bool) -> Option<&'static str> {
    match harness.as_str() {
        AgentId::CODEX => Some(".codex/skills"),
        AgentId::OPEN_CODE => Some(if global {
            ".config/opencode/skills"
        } else {
            ".opencode/skills"
        }),
        AgentId::CURSOR => Some(".cursor/skills"),
        _ => None,
    }
}

fn folder_for(
    harness: &AgentId,
    scope: &RootScope,
    destination: SkillDestination,
) -> Result<Folder, CoreError> {
    let global = matches!(scope, RootScope::Global);
    if destination == SkillDestination::PerHarness {
        if let Some(relative) = per_harness_folder(harness, global) {
            return Ok(Folder::Own {
                relative,
                project_marker: None,
            });
        }
    }
    match harness.as_str() {
        UNIVERSAL_TARGET | AgentId::CODEX | AgentId::OPEN_CODE | AgentId::CURSOR => {
            Ok(Folder::Shared)
        }
        AgentId::CLAUDE_CODE => Ok(Folder::Own {
            relative: ".claude/skills",
            project_marker: None,
        }),
        AgentId::PI => Ok(Folder::Own {
            relative: if global {
                ".pi/agent/skills"
            } else {
                ".pi/skills"
            },
            project_marker: (destination == SkillDestination::Universal).then_some(".pi"),
        }),
        AgentId::GROK_BUILD => Ok(Folder::Own {
            relative: ".grok/skills",
            project_marker: (destination == SkillDestination::Universal).then_some(".grok"),
        }),
        other => Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!(
                "install cannot write for `{other}`; use one of: {}",
                INSTALL_TARGET_IDS.join(", ")
            ),
        )),
    }
}

/// `true` for a harness with a skills folder of its own (Claude Code, pi,
/// Grok Build), `false` for one that reads the shared folder.
pub(crate) fn has_own_folder(harness: &AgentId) -> bool {
    matches!(
        folder_for(harness, &RootScope::Global, SkillDestination::Universal),
        Ok(Folder::Own { .. })
    )
}

/// The `skills` CLI's `--agent` spelling of a harness id.
pub(crate) fn cli_agent_id(harness: &AgentId) -> &str {
    match harness.as_str() {
        AgentId::OPEN_CODE => "opencode",
        AgentId::GROK_BUILD => "grok",
        other => other,
    }
}

/// The request's harness set without repeats, in first-seen order. An
/// empty set means the shared folder alone.
pub(crate) fn requested_harnesses(harnesses: &[AgentId]) -> Vec<AgentId> {
    if harnesses.is_empty() {
        return vec![AgentId::from(UNIVERSAL_TARGET)];
    }
    let mut out: Vec<AgentId> = Vec::new();
    for harness in harnesses {
        if !out.contains(harness) {
            out.push(harness.clone());
        }
    }
    out
}

/// Fails on the first id `install` cannot write for.
pub(crate) fn validate_harnesses(harnesses: &[AgentId]) -> Result<(), CoreError> {
    for harness in harnesses {
        folder_for(harness, &RootScope::Global, SkillDestination::Universal)?;
    }
    Ok(())
}

/// The mode `install` uses: `Dotagents` always links (its CLI owns the
/// shared copy); one distinct folder or fewer forces `Copy`, the same as
/// the `skills` CLI's `uniqueDirs.size <= 1` rule; otherwise the request's.
pub(crate) fn effective_link_mode(
    method: InstallMethod,
    harnesses: &[AgentId],
    scope: &RootScope,
    requested: InstallLinkMode,
) -> Result<InstallLinkMode, CoreError> {
    let mut folders: Vec<Folder> = Vec::new();
    for harness in harnesses {
        let folder = folder_for(harness, scope, SkillDestination::Universal)?;
        if !folders.contains(&folder) {
            folders.push(folder);
        }
    }
    Ok(match method {
        InstallMethod::Dotagents => InstallLinkMode::Link,
        _ if folders.len() <= 1 => InstallLinkMode::Copy,
        _ => requested,
    })
}

/// One harness's part of an [`InstallPlan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StepAction {
    /// Reads the shared copy at `path`.
    ReadsShared { path: PathBuf },
    /// A relative symlink at `link`, inside the harness folder `dir`.
    Link { dir: PathBuf, link: PathBuf },
    /// A real folder at `path`, inside the harness folder `dir`.
    Copy { dir: PathBuf, path: PathBuf },
    /// Nothing is written.
    Skip { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Step {
    pub harness: AgentId,
    pub action: StepAction,
}

/// Everything `install` will write, decided before the first write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstallPlan {
    pub mode: InstallLinkMode,
    pub universal_root: PathBuf,
    /// `<scope>/.agents/skills/<name>`, when this install writes it.
    pub shared: Option<PathBuf>,
    pub steps: Vec<Step>,
}

impl InstallPlan {
    /// The folder `InstallOutcome::Installed::deployment_path` names: the
    /// shared copy, or else the first harness copy.
    pub fn primary_path(&self) -> Option<&Path> {
        self.shared.as_deref().or_else(|| {
            self.steps.iter().find_map(|s| match &s.action {
                StepAction::Copy { path, .. } => Some(path.as_path()),
                _ => None,
            })
        })
    }

    /// The harnesses this install serves, in request order, without the
    /// skipped ones.
    pub fn served_harnesses(&self) -> Vec<AgentId> {
        self.steps
            .iter()
            .filter(|s| !matches!(s.action, StepAction::Skip { .. }))
            .map(|s| s.harness.clone())
            .collect()
    }

    /// Every path this install creates, shared copy first.
    pub fn written_paths(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.shared.iter().cloned().collect();
        for step in &self.steps {
            match &step.action {
                StepAction::Link { link: path, .. } | StepAction::Copy { path, .. } => {
                    out.push(path.clone());
                }
                StepAction::ReadsShared { .. } | StepAction::Skip { .. } => {}
            }
        }
        out
    }
}

/// Builds the plan for `harnesses` (already passed through
/// [`requested_harnesses`]) under `root`. Reads the disk only to find
/// whole-folder links and missing project folders; writes nothing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_install(
    fs: &dyn ScopeFs,
    root: &Path,
    scope: &RootScope,
    normalized: &NormalizedScope,
    skill: &SkillName,
    harnesses: &[AgentId],
    mode: InstallLinkMode,
    destination: SkillDestination,
) -> Result<InstallPlan, CoreError> {
    let universal_root = root.join(UNIVERSAL_ROOT_RELATIVE);
    let shared_path = universal_root.join(&skill.0);
    let mut shared_needed = mode == InstallLinkMode::Link;
    let mut steps = Vec::new();
    for harness in harnesses {
        let action = match folder_for(harness, scope, destination)? {
            Folder::Shared => {
                shared_needed = true;
                StepAction::ReadsShared {
                    path: shared_path.clone(),
                }
            }
            Folder::Own {
                relative,
                project_marker,
            } => {
                let dir = match scope {
                    RootScope::Global => normalized.global_root_path(Path::new(relative)),
                    RootScope::Project(_) => root.join(relative),
                };
                let marker_missing = matches!(scope, RootScope::Project(_))
                    && project_marker.is_some_and(|m| fs.symlink_metadata(&root.join(m)).is_err());
                if marker_missing {
                    StepAction::Skip {
                        reason: format!(
                            "{harness} has no {} folder in this project",
                            project_marker.unwrap_or_default()
                        ),
                    }
                } else if folder_reaches_shared(fs, harness, &dir, &universal_root)? {
                    if destination == SkillDestination::PerHarness {
                        return Err(CoreError::new(
                            ErrorCode::InvalidRequest,
                            format!(
                                "{} is a link to {}, so a copy for {harness} would land in the \
                                 shared folder; replace the link with a real folder, then install again",
                                dir.display(),
                                universal_root.display(),
                            ),
                        )
                        .at(&dir));
                    }
                    shared_needed = true;
                    StepAction::ReadsShared {
                        path: shared_path.clone(),
                    }
                } else if mode == InstallLinkMode::Link {
                    StepAction::Link {
                        link: dir.join(&skill.0),
                        dir,
                    }
                } else {
                    StepAction::Copy {
                        path: dir.join(&skill.0),
                        dir,
                    }
                }
            }
        };
        steps.push(Step {
            harness: harness.clone(),
            action,
        });
    }
    Ok(InstallPlan {
        mode,
        shared: shared_needed.then_some(shared_path),
        universal_root,
        steps,
    })
}

/// `true` when the harness folder `dir` already resolves to the shared
/// folder, so every skill in it is visible with no per-skill entry. A
/// dangling link is judged by its stored target, since the shared folder
/// may not exist before the first install. Fails when `dir` is a link to
/// any other folder (see the module doc).
fn folder_reaches_shared(
    fs: &dyn ScopeFs,
    harness: &AgentId,
    dir: &Path,
    universal_root: &Path,
) -> Result<bool, CoreError> {
    let is_link = fs
        .symlink_metadata(dir)
        .is_ok_and(|f| f.kind == FileKind::Symlink);
    let same = match (fs.canonicalize(dir), fs.canonicalize(universal_root)) {
        (Ok(resolved), Ok(universal)) => resolved == universal,
        _ => {
            is_link
                && fs.read_link(dir).is_ok_and(|target| {
                    let parent = dir.parent().unwrap_or(dir);
                    fsops::join_lexical(parent, &target)
                        == fsops::join_lexical(Path::new("/"), universal_root)
                })
        }
    };
    if same {
        return Ok(true);
    }
    if !is_link {
        return Ok(false);
    }
    Err(CoreError::new(
        ErrorCode::InvalidRequest,
        format!(
            "{} is a link to another folder, not to {}, so {harness} would not see this skill; \
             point the link at {} or replace it with a real folder, then install again",
            dir.display(),
            universal_root.display(),
            universal_root.display(),
        ),
    )
    .at(dir))
}

/// The path from the folder `from_dir` to `to`, as `..` steps and names.
/// Both must be absolute and free of `..`; `install` passes resolved paths
/// so a `/var` to `/private/var` alias cannot add stray steps.
pub(crate) fn relative_path(from_dir: &Path, to: &Path) -> PathBuf {
    let from: Vec<Component> = from_dir.components().collect();
    let target: Vec<Component> = to.components().collect();
    let common = from.iter().zip(&target).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for part in &target[common..] {
        out.push(part.as_os_str());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(raw: &[&'static str]) -> Vec<AgentId> {
        raw.iter().map(|s| AgentId::from(*s)).collect()
    }

    /// `effective_link_mode_forces_copy_for_one_folder_and_keeps_the_choice_for_two_or_names_the_cli_rule_it_broke`:
    /// the `skills` CLI's `uniqueDirs.size <= 1` rule. Claude Code alone and
    /// Codex+Cursor (both the shared folder) are one folder, so `Copy` is
    /// forced; Claude Code+pi are two, so the request's `Link` stands.
    /// `Dotagents` always links.
    #[test]
    fn effective_link_mode_forces_copy_for_one_folder_and_keeps_the_choice_for_two_or_names_the_cli_rule_it_broke(
    ) {
        let g = RootScope::Global;
        let link = InstallLinkMode::Link;
        let mode = |m, h: &[&'static str]| effective_link_mode(m, &ids(h), &g, link).unwrap();
        assert_eq!(
            mode(InstallMethod::Copy, &["claude-code"]),
            InstallLinkMode::Copy
        );
        assert_eq!(
            mode(InstallMethod::SkillsSh, &["codex", "cursor"]),
            InstallLinkMode::Copy
        );
        assert_eq!(
            mode(InstallMethod::Copy, &["claude-code", "pi"]),
            InstallLinkMode::Link
        );
        assert_eq!(
            mode(InstallMethod::Dotagents, &["claude-code"]),
            InstallLinkMode::Link
        );
    }

    /// `relative_path_walks_up_to_the_common_folder_or_names_the_absolute_link`:
    /// the stored target of `~/.claude/skills/x` must be
    /// `../../.agents/skills/x`, the same string the recorded CLI trace 01
    /// holds.
    #[test]
    fn relative_path_walks_up_to_the_common_folder_or_names_the_absolute_link() {
        assert_eq!(
            relative_path(
                Path::new("/h/.claude/skills"),
                Path::new("/h/.agents/skills/x")
            ),
            PathBuf::from("../../.agents/skills/x")
        );
        assert_eq!(
            relative_path(
                Path::new("/h/.pi/agent/skills"),
                Path::new("/h/.agents/skills/x")
            ),
            PathBuf::from("../../../.agents/skills/x")
        );
    }

    /// `cli_agent_id_uses_the_skills_cli_spelling_or_names_the_unknown_agent`:
    /// `open-code` and `grok-build` are `opencode` and `grok` to the CLI;
    /// the CLI rejects the core spelling with "Invalid agents".
    #[test]
    fn cli_agent_id_uses_the_skills_cli_spelling_or_names_the_unknown_agent() {
        let spelled: Vec<String> = ids(INSTALL_TARGET_IDS)
            .iter()
            .map(|h| cli_agent_id(h).to_string())
            .collect();
        assert_eq!(
            spelled,
            [
                "universal",
                "claude-code",
                "codex",
                "opencode",
                "cursor",
                "pi",
                "grok"
            ]
        );
    }
}
