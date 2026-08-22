// ============================================================================
// Skills Module - Tauri Commands
// IPC commands for skill discovery, installation, and management
// ============================================================================

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use super::agents::{AgentId, AgentTarget};
use super::api;
use super::lock_file;
use super::project_discovery;
use super::skill_assembly;
use super::skill_discovery;
use super::skill_dto::{
    InstallRequest, InstallResult, InstalledSkill, PaginatedSkillsResponse, SkillSearchResult,
};

/// Search for skills on skills.sh
#[tauri::command]
pub async fn search_skills(
    query: String,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<PaginatedSkillsResponse, String> {
    api::search_skills(&query, limit, offset).await
}

/// Get popular skills (sorted by install count)
#[tauri::command]
pub async fn get_popular_skills(
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<PaginatedSkillsResponse, String> {
    api::get_popular_skills(limit, offset).await
}

/// Get skill details from skills.sh
#[tauri::command]
pub async fn get_skill_details(skill_id: String) -> Result<SkillSearchResult, String> {
    api::get_skill_details(&skill_id).await
}

/// Get all installed skills, merging what's found on disk for the four
/// first-class agents (Claude Code, Codex, OpenCode, pi) with the
/// ~/.agents/.skill-lock.json entries. Project-scoped skills are discovered
/// for `project_discovery::discover_skill_projects` in addition to any
/// project paths the caller passes in.
#[tauri::command]
pub fn get_installed_skills(
    project_paths: Option<Vec<String>>,
) -> Result<Vec<InstalledSkill>, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;

    let mut paths: BTreeSet<PathBuf> = project_discovery::discover_skill_projects(&home)
        .into_iter()
        .collect();
    paths.extend(
        project_paths
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from),
    );
    let project_paths: Vec<PathBuf> = paths.into_iter().collect();

    let candidates = skill_discovery::discover_skill_candidates(&home, &project_paths);
    let lock = lock_file::read_lock_file()?;
    Ok(skill_assembly::assemble_installed_skills(candidates, &lock))
}

/// List project directories discovered from Codex config and Claude Code
/// transcripts that have a first-class agent's skill directory.
#[tauri::command]
pub fn list_skill_projects() -> Result<Vec<String>, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    Ok(project_discovery::discover_skill_projects(&home)
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect())
}

/// Check if a skill is installed
#[tauri::command]
pub fn is_skill_installed(skill_name: String) -> Result<bool, String> {
    lock_file::is_skill_installed(&skill_name)
}

/// Get all supported agent targets
#[tauri::command]
pub fn get_agent_targets() -> Vec<AgentTarget> {
    let home = dirs::home_dir().unwrap_or_default();
    let home_str = home.to_string_lossy();

    AgentId::all()
        .into_iter()
        .map(|id| AgentTarget {
            name: id.display_name().to_string(),
            project_path: id.project_path().to_string(),
            global_path: format!("{}/{}", home_str, id.global_path()),
            id,
        })
        .collect()
}

/// Install a skill using npx skills CLI
#[tauri::command]
pub async fn install_skill(request: InstallRequest) -> Result<InstallResult, String> {
    // Parse skill_source - could be "owner/repo" or "owner/repo/skill-name"
    // or just "skill-name" for well-known skills
    let (repo_source, skill_name) = parse_skill_source(&request.skill_source);

    let mut args = vec!["skills".to_string(), "add".to_string(), repo_source.clone()];

    // Always add --yes for non-interactive mode
    args.push("--yes".to_string());

    // Add scope flag
    if request.scope == super::skill_dto::InstallScope::Global {
        args.push("--global".to_string());
    } else if let Some(ref project_path) = request.project_path {
        args.push("--cwd".to_string());
        args.push(project_path.clone());
    }

    // Add specific skill if we have one (for multi-skill repos)
    if let Some(ref name) = skill_name {
        args.push("--skill".to_string());
        args.push(name.clone());
    }

    // Add agent targets if specified
    if !request.agents.is_empty() {
        for agent in &request.agents {
            args.push("--agent".to_string());
            args.push(agent.cli_name().to_string());
        }
    }

    // Log the command for debugging
    eprintln!("[install_skill] Running: npx {}", args.join(" "));

    // Execute npx skills command
    let output = Command::new("npx")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to execute npx skills: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    eprintln!("[install_skill] Exit code: {:?}", output.status.code());
    eprintln!("[install_skill] stdout: {}", stdout);
    eprintln!("[install_skill] stderr: {}", stderr);

    if output.status.success() {
        // Use parsed skill name or fallback
        let result_name = skill_name.unwrap_or_else(|| {
            repo_source
                .split('/')
                .next_back()
                .unwrap_or(&repo_source)
                .to_string()
        });

        Ok(InstallResult {
            success: true,
            skill_name: result_name,
            installed_path: None,
            error: None,
        })
    } else {
        Ok(InstallResult {
            success: false,
            skill_name: request.skill_source.clone(),
            installed_path: None,
            error: Some(if stderr.is_empty() { stdout } else { stderr }),
        })
    }
}

/// Parse skill source into (repo, optional skill name)
/// Examples:
///   "vercel-labs/skills" -> ("vercel-labs/skills", None)
///   "obra/superpowers/brainstorming" -> ("obra/superpowers", Some("brainstorming"))
///   "sentry-cli" -> ("sentry-cli", None) - for well-known skills
fn parse_skill_source(source: &str) -> (String, Option<String>) {
    let parts: Vec<&str> = source.split('/').collect();
    match parts.len() {
        // Well-known skill or single name
        0 | 1 => (source.to_string(), None),
        // owner/repo format
        2 => (source.to_string(), None),
        // owner/repo/skill-name format
        _ => {
            let repo = format!("{}/{}", parts[0], parts[1]);
            let skill = parts[2..].join("/");
            (repo, Some(skill))
        }
    }
}

/// Remove a skill using npx skills CLI
#[tauri::command]
pub async fn remove_skill(skill_name: String, global: bool) -> Result<InstallResult, String> {
    let mut args = vec![
        "skills".to_string(),
        "remove".to_string(),
        skill_name.clone(),
    ];

    // Add --yes for non-interactive mode (CLI has its own confirmation prompt)
    args.push("--yes".to_string());

    if global {
        args.push("--global".to_string());
    }

    // Log the command for debugging
    eprintln!("[remove_skill] Running: npx {}", args.join(" "));

    let output = Command::new("npx")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to execute npx skills: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    eprintln!("[remove_skill] Exit code: {:?}", output.status.code());
    eprintln!("[remove_skill] stdout: {}", stdout);
    eprintln!("[remove_skill] stderr: {}", stderr);

    if output.status.success() {
        Ok(InstallResult {
            success: true,
            skill_name,
            installed_path: None,
            error: None,
        })
    } else {
        Ok(InstallResult {
            success: false,
            skill_name,
            installed_path: None,
            error: Some(if stderr.is_empty() { stdout } else { stderr }),
        })
    }
}

/// Update a skill using npx skills CLI
#[tauri::command]
pub async fn update_skill(skill_name: String, global: bool) -> Result<InstallResult, String> {
    let mut args = vec![
        "skills".to_string(),
        "update".to_string(),
        skill_name.clone(),
    ];

    if global {
        args.push("--global".to_string());
    }

    let output = Command::new("npx")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to execute npx skills: {}", e))?;

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        Ok(InstallResult {
            success: true,
            skill_name,
            installed_path: None,
            error: None,
        })
    } else {
        Ok(InstallResult {
            success: false,
            skill_name,
            installed_path: None,
            error: Some(stderr),
        })
    }
}
