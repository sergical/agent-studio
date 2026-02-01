// ============================================================================
// Skills Module - Lock File
// Read and parse the skill lock file (~/.agents/.skill-lock.json)
// ============================================================================

use std::fs;
use std::path::PathBuf;

use super::types::{InstalledSkill, SkillLockFile};

/// Get the path to the skill lock file
pub fn get_lock_file_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    Ok(home.join(".agents").join(".skill-lock.json"))
}

/// Read and parse the skill lock file
pub fn read_lock_file() -> Result<SkillLockFile, String> {
    let lock_path = get_lock_file_path()?;

    if !lock_path.exists() {
        // Return empty lock file if it doesn't exist
        return Ok(SkillLockFile {
            version: 3,
            skills: std::collections::HashMap::new(),
        });
    }

    let content = fs::read_to_string(&lock_path)
        .map_err(|e| format!("Failed to read lock file: {}", e))?;

    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse lock file: {}", e))
}

/// Get all installed skills from the lock file
pub fn get_installed_skills() -> Result<Vec<InstalledSkill>, String> {
    let lock_file = read_lock_file()?;

    let skills: Vec<InstalledSkill> = lock_file
        .skills
        .into_iter()
        .map(|(name, entry)| InstalledSkill {
            name,
            source: entry.source,
            source_type: entry.source_type,
            source_url: Some(entry.source_url),
            skill_path: entry.skill_path,
            installed_at: entry.installed_at,
            updated_at: Some(entry.updated_at),
            has_update: false, // TODO: Check for updates via API
        })
        .collect();

    Ok(skills)
}

/// Check if a skill is installed
pub fn is_skill_installed(skill_name: &str) -> Result<bool, String> {
    let lock_file = read_lock_file()?;
    Ok(lock_file.skills.contains_key(skill_name))
}

/// Get details for a specific installed skill
pub fn get_installed_skill(skill_name: &str) -> Result<Option<InstalledSkill>, String> {
    let lock_file = read_lock_file()?;

    Ok(lock_file.skills.get(skill_name).map(|entry| InstalledSkill {
        name: skill_name.to_string(),
        source: entry.source.clone(),
        source_type: entry.source_type.clone(),
        source_url: Some(entry.source_url.clone()),
        skill_path: entry.skill_path.clone(),
        installed_at: entry.installed_at.clone(),
        updated_at: Some(entry.updated_at.clone()),
        has_update: false,
    }))
}
