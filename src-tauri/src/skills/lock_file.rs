// ============================================================================
// Skills Module - Lock File
// Read and parse the skill lock file (~/.agents/.skill-lock.json)
// ============================================================================

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Installed skill entry in lock file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledSkillEntry {
    pub source: String,
    #[serde(rename = "sourceType")]
    pub source_type: String,
    #[serde(rename = "sourceUrl")]
    pub source_url: String,
    #[serde(rename = "skillPath", default)]
    pub skill_path: Option<String>,
    #[serde(rename = "skillFolderHash")]
    pub skill_folder_hash: String,
    #[serde(rename = "installedAt")]
    pub installed_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// Lock file structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillLockFile {
    pub version: u32,
    pub skills: std::collections::HashMap<String, InstalledSkillEntry>,
}

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

    let content =
        fs::read_to_string(&lock_path).map_err(|e| format!("Failed to read lock file: {}", e))?;

    serde_json::from_str(&content).map_err(|e| format!("Failed to parse lock file: {}", e))
}

/// Check whether a skill name is recorded in the lock file
pub fn is_skill_installed(skill_name: &str) -> Result<bool, String> {
    let lock_file = read_lock_file()?;
    Ok(lock_file.skills.contains_key(skill_name))
}
