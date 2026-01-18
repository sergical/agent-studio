// ============================================================================
// Agent Studio - Commands Module
// Comprehensive Claude Code entity discovery and management
// ============================================================================

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::SystemTime;

// ============================================================================
// Type Definitions
// ============================================================================

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BaseEntity {
    pub id: String,
    pub name: String,
    pub path: String,
    pub scope: String,  // "global" or "project"
    pub project_path: Option<String>,
    pub is_symlink: bool,
    pub symlink_target: Option<String>,
    pub content: Option<String>,
    pub last_modified: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SettingsEntity {
    #[serde(flatten)]
    pub base: BaseEntity,
    #[serde(rename = "type")]
    pub entity_type: String,  // "settings"
    pub variant: String,  // "global", "project", "local"
    pub parsed: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MemoryEntity {
    #[serde(flatten)]
    pub base: BaseEntity,
    #[serde(rename = "type")]
    pub entity_type: String,  // "memory"
    pub variant: String,  // "root" or "dotclaude"
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentEntity {
    #[serde(flatten)]
    pub base: BaseEntity,
    #[serde(rename = "type")]
    pub entity_type: String,  // "agent"
    pub frontmatter: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkillEntity {
    #[serde(flatten)]
    pub base: BaseEntity,
    #[serde(rename = "type")]
    pub entity_type: String,  // "skill"
    pub skill_dir: String,
    pub frontmatter: Option<HashMap<String, serde_json::Value>>,
    pub supporting_files: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommandEntity {
    #[serde(flatten)]
    pub base: BaseEntity,
    #[serde(rename = "type")]
    pub entity_type: String,  // "command"
    pub namespace: Option<String>,
    pub frontmatter: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HookDefinition {
    #[serde(rename = "type")]
    pub hook_type: String,
    pub command: Option<String>,
    pub prompt: Option<String>,
    pub timeout: Option<u32>,
    pub once: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HookMatcher {
    pub matcher: Option<String>,
    pub hooks: Vec<HookDefinition>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HookEntity {
    pub id: String,
    #[serde(rename = "type")]
    pub entity_type: String,  // "hook"
    pub event: String,
    pub matcher: Option<String>,
    pub hooks: Vec<HookDefinition>,
    pub source: String,  // "global", "project", "local"
    pub source_path: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PluginEntity {
    #[serde(flatten)]
    pub base: BaseEntity,
    #[serde(rename = "type")]
    pub entity_type: String,  // "plugin"
    pub plugin_dir: String,
    pub manifest: Option<serde_json::Value>,
    pub has_commands: bool,
    pub has_agents: bool,
    pub has_skills: bool,
    pub has_hooks: bool,
    pub has_mcp: bool,
    pub has_lsp: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpServerConfig {
    #[serde(rename = "type")]
    pub transport_type: Option<String>,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub url: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpServerEntity {
    pub id: String,
    #[serde(rename = "type")]
    pub entity_type: String,  // "mcp"
    pub name: String,
    pub scope: String,
    pub transport: String,
    pub config: McpServerConfig,
    pub source_path: String,
    pub is_from_plugin: bool,
    pub plugin_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DuplicateEntity {
    pub id: String,
    pub path: String,
    pub scope: String,
    pub project_path: Option<String>,
    pub precedence: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DuplicateGroup {
    pub name: String,
    pub entity_type: String,
    pub entities: Vec<DuplicateEntity>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SymlinkInfo {
    pub path: String,
    pub target: String,
    pub target_exists: bool,
    pub entity_type: Option<String>,
    pub entity_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EntityCounts {
    pub settings: u32,
    pub memory: u32,
    pub agents: u32,
    pub skills: u32,
    pub commands: u32,
    pub plugins: u32,
    pub hooks: u32,
    pub mcp: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProjectInfo {
    pub path: String,
    pub name: String,
    pub has_claude_dir: bool,
    pub has_mcp_json: bool,
    pub has_claude_md: bool,
    pub has_root_claude_md: bool,
    pub entity_counts: EntityCounts,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiscoveryResult {
    pub global_config_path: String,
    pub projects: Vec<ProjectInfo>,
    pub settings: Vec<SettingsEntity>,
    pub memory: Vec<MemoryEntity>,
    pub agents: Vec<AgentEntity>,
    pub skills: Vec<SkillEntity>,
    pub commands: Vec<CommandEntity>,
    pub hooks: Vec<HookEntity>,
    pub plugins: Vec<PluginEntity>,
    pub mcp_servers: Vec<McpServerEntity>,
    pub duplicates: Vec<DuplicateGroup>,
    pub symlinks: Vec<SymlinkInfo>,
    pub discovered_at: u64,
}

// ============================================================================
// Legacy Types (backward compatibility)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConfigFile {
    pub path: String,
    pub name: String,
    pub tool: String,
    pub scope: String,
    pub file_type: String,
    pub exists: bool,
    pub content: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentFile {
    pub path: String,
    pub name: String,
    pub tool: String,
    pub scope: String,
    pub frontmatter: Option<HashMap<String, serde_json::Value>>,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiscoveredConfigs {
    pub claude_code: Vec<ConfigFile>,
    pub opencode: Vec<ConfigFile>,
    pub agents_md: Vec<ConfigFile>,
    pub agents: Vec<AgentFile>,
    pub skills: Vec<AgentFile>,
}

// ============================================================================
// Utility Functions
// ============================================================================

fn get_home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

fn get_config_dir() -> Option<PathBuf> {
    dirs::config_dir()
}

fn generate_id(prefix: &str, path: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    format!("{}_{:x}", prefix, hasher.finish())
}

fn get_last_modified(path: &PathBuf) -> u64 {
    path.metadata()
        .and_then(|m| m.modified())
        .map(|t| t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_millis() as u64)
        .unwrap_or(0)
}

fn is_symlink_with_target(path: &PathBuf) -> (bool, Option<String>) {
    if path.is_symlink() {
        let target = fs::read_link(path)
            .map(|p| p.to_string_lossy().to_string())
            .ok();
        (true, target)
    } else {
        (false, None)
    }
}

fn read_file_content(path: &PathBuf) -> Option<String> {
    if path.exists() {
        fs::read_to_string(path).ok()
    } else {
        None
    }
}

fn parse_frontmatter(content: &str) -> (Option<HashMap<String, serde_json::Value>>, String) {
    if content.starts_with("---") {
        if let Some(end_idx) = content[3..].find("---") {
            let yaml_str = &content[3..end_idx + 3];
            let body = &content[end_idx + 6..].trim_start();
            
            if let Ok(frontmatter) = serde_yaml::from_str::<HashMap<String, serde_json::Value>>(yaml_str) {
                return (Some(frontmatter), body.to_string());
            }
        }
    }
    (None, content.to_string())
}

fn parse_json_file(path: &PathBuf) -> Option<serde_json::Value> {
    read_file_content(path).and_then(|content| serde_json::from_str(&content).ok())
}

// ============================================================================
// Discovery Commands
// ============================================================================

#[tauri::command]
pub fn get_home_directory() -> Result<String, String> {
    get_home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "Could not find home directory".to_string())
}

#[tauri::command]
pub fn get_config_directory() -> Result<String, String> {
    get_config_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "Could not find config directory".to_string())
}

#[tauri::command]
pub fn get_global_claude_path() -> Result<String, String> {
    get_home_dir()
        .map(|p| p.join(".claude").to_string_lossy().to_string())
        .ok_or_else(|| "Could not find home directory".to_string())
}

#[tauri::command]
pub fn discover_all(project_paths: Option<Vec<String>>) -> Result<DiscoveryResult, String> {
    let home = get_home_dir().ok_or("Could not find home directory")?;
    let global_claude_path = home.join(".claude");
    
    let mut all_settings = Vec::new();
    let mut all_memory = Vec::new();
    let mut all_agents = Vec::new();
    let mut all_skills = Vec::new();
    let mut all_commands = Vec::new();
    let mut all_hooks = Vec::new();
    let mut all_plugins = Vec::new();
    let mut all_mcp = Vec::new();
    let mut all_symlinks = Vec::new();
    let mut projects = Vec::new();
    
    // Track seen paths to prevent duplicates
    let mut seen_settings_paths = std::collections::HashSet::new();
    let mut seen_memory_paths = std::collections::HashSet::new();
    let mut seen_agent_paths = std::collections::HashSet::new();
    let mut seen_skill_paths = std::collections::HashSet::new();
    let mut seen_command_paths = std::collections::HashSet::new();
    let mut seen_plugin_paths = std::collections::HashSet::new();
    let mut seen_hook_ids = std::collections::HashSet::new();
    let mut seen_mcp_ids = std::collections::HashSet::new();

    // Discover global entities
    for s in discover_settings_internal(&global_claude_path, "global", None)? {
        if seen_settings_paths.insert(s.base.path.clone()) {
            all_settings.push(s);
        }
    }
    for m in discover_memory_internal(&global_claude_path, &home, "global", None)? {
        if seen_memory_paths.insert(m.base.path.clone()) {
            all_memory.push(m);
        }
    }
    for a in discover_agents_internal(&global_claude_path.join("agents"), "global", None)? {
        if seen_agent_paths.insert(a.base.path.clone()) {
            all_agents.push(a);
        }
    }
    for s in discover_skills_internal(&global_claude_path.join("skills"), "global", None)? {
        if seen_skill_paths.insert(s.base.path.clone()) {
            all_skills.push(s);
        }
    }
    for c in discover_commands_internal(&global_claude_path.join("commands"), "global", None)? {
        if seen_command_paths.insert(c.base.path.clone()) {
            all_commands.push(c);
        }
    }
    // Discover local plugins from ~/.claude/plugins/ directory
    for p in discover_plugins_internal(&global_claude_path.join("plugins"), "global", None)? {
        if seen_plugin_paths.insert(p.base.path.clone()) {
            all_plugins.push(p);
        }
    }
    // Discover installed plugins from installed_plugins.json (marketplace plugins)
    for p in discover_installed_plugins(&home)? {
        if seen_plugin_paths.insert(p.base.id.clone()) {
            all_plugins.push(p);
        }
    }
    for h in extract_hooks_internal(&global_claude_path.join("settings.json"), "global")? {
        if seen_hook_ids.insert(h.id.clone()) {
            all_hooks.push(h);
        }
    }
    for m in discover_mcp_from_claude_json(&home)? {
        if seen_mcp_ids.insert(m.id.clone()) {
            all_mcp.push(m);
        }
    }

    // Discover project entities - first scan for projects recursively in given directories
    if let Some(base_paths) = project_paths {
        // Use scan_projects to find all projects recursively
        let found_projects = scan_projects(base_paths)?;
        
        for project_info in found_projects {
            let project_path = PathBuf::from(&project_info.path);
            let claude_dir = project_path.join(".claude");
            let project_path_str = project_info.path.clone();

            // Count entities for this project
            let mut counts = EntityCounts {
                settings: 0,
                memory: 0,
                agents: 0,
                skills: 0,
                commands: 0,
                plugins: 0,
                hooks: 0,
                mcp: 0,
            };

            for s in discover_settings_internal(&claude_dir, "project", Some(&project_path_str))? {
                if seen_settings_paths.insert(s.base.path.clone()) {
                    counts.settings += 1;
                    all_settings.push(s);
                }
            }

            for m in discover_memory_internal(&claude_dir, &project_path, "project", Some(&project_path_str))? {
                if seen_memory_paths.insert(m.base.path.clone()) {
                    counts.memory += 1;
                    all_memory.push(m);
                }
            }

            for a in discover_agents_internal(&claude_dir.join("agents"), "project", Some(&project_path_str))? {
                if seen_agent_paths.insert(a.base.path.clone()) {
                    counts.agents += 1;
                    all_agents.push(a);
                }
            }

            for s in discover_skills_internal(&claude_dir.join("skills"), "project", Some(&project_path_str))? {
                if seen_skill_paths.insert(s.base.path.clone()) {
                    counts.skills += 1;
                    all_skills.push(s);
                }
            }

            for c in discover_commands_internal(&claude_dir.join("commands"), "project", Some(&project_path_str))? {
                if seen_command_paths.insert(c.base.path.clone()) {
                    counts.commands += 1;
                    all_commands.push(c);
                }
            }

            for p in discover_plugins_internal(&claude_dir.join("plugins"), "project", Some(&project_path_str))? {
                if seen_plugin_paths.insert(p.base.path.clone()) {
                    counts.plugins += 1;
                    all_plugins.push(p);
                }
            }

            for h in extract_hooks_internal(&claude_dir.join("settings.json"), "project")? {
                if seen_hook_ids.insert(h.id.clone()) {
                    counts.hooks += 1;
                    all_hooks.push(h);
                }
            }
            for h in extract_hooks_internal(&claude_dir.join("settings.local.json"), "local")? {
                if seen_hook_ids.insert(h.id.clone()) {
                    counts.hooks += 1;
                    all_hooks.push(h);
                }
            }

            // MCP from .mcp.json
            for m in discover_mcp_from_project(&project_path)? {
                if seen_mcp_ids.insert(m.id.clone()) {
                    counts.mcp += 1;
                    all_mcp.push(m);
                }
            }

            projects.push(ProjectInfo {
                path: project_path_str.clone(),
                name: project_info.name.clone(),
                has_claude_dir: claude_dir.exists(),
                has_mcp_json: project_path.join(".mcp.json").exists(),
                has_claude_md: claude_dir.join("CLAUDE.md").exists(),
                has_root_claude_md: project_path.join("CLAUDE.md").exists(),
                entity_counts: counts,
            });
        }
    }

    // Collect symlinks
    for entity in &all_settings {
        if entity.base.is_symlink {
            all_symlinks.push(SymlinkInfo {
                path: entity.base.path.clone(),
                target: entity.base.symlink_target.clone().unwrap_or_default(),
                target_exists: PathBuf::from(&entity.base.symlink_target.clone().unwrap_or_default()).exists(),
                entity_type: Some("settings".to_string()),
                entity_id: Some(entity.base.id.clone()),
            });
        }
    }
    for entity in &all_agents {
        if entity.base.is_symlink {
            all_symlinks.push(SymlinkInfo {
                path: entity.base.path.clone(),
                target: entity.base.symlink_target.clone().unwrap_or_default(),
                target_exists: PathBuf::from(&entity.base.symlink_target.clone().unwrap_or_default()).exists(),
                entity_type: Some("agent".to_string()),
                entity_id: Some(entity.base.id.clone()),
            });
        }
    }
    // ... similar for other entity types

    // Find duplicates
    let duplicates = find_duplicates_internal(&all_agents, &all_skills, &all_commands)?;

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    Ok(DiscoveryResult {
        global_config_path: global_claude_path.to_string_lossy().to_string(),
        projects,
        settings: all_settings,
        memory: all_memory,
        agents: all_agents,
        skills: all_skills,
        commands: all_commands,
        hooks: all_hooks,
        plugins: all_plugins,
        mcp_servers: all_mcp,
        duplicates,
        symlinks: all_symlinks,
        discovered_at: now,
    })
}

// ============================================================================
// Entity-Specific Discovery Functions
// ============================================================================

fn discover_settings_internal(claude_dir: &PathBuf, scope: &str, project_path: Option<&str>) -> Result<Vec<SettingsEntity>, String> {
    let mut settings = Vec::new();
    
    if scope == "global" {
        let global_settings_path = claude_dir.join("settings.json");
        if global_settings_path.exists() {
            let (is_symlink, symlink_target) = is_symlink_with_target(&global_settings_path);
            let content = read_file_content(&global_settings_path);
            let parsed = content.as_ref().and_then(|c| serde_json::from_str(c).ok());
            
            settings.push(SettingsEntity {
                base: BaseEntity {
                    id: generate_id("settings", &global_settings_path.to_string_lossy()),
                    name: "settings.json".to_string(),
                    path: global_settings_path.to_string_lossy().to_string(),
                    scope: scope.to_string(),
                    project_path: project_path.map(String::from),
                    is_symlink,
                    symlink_target,
                    content,
                    last_modified: get_last_modified(&global_settings_path),
                },
                entity_type: "settings".to_string(),
                variant: "global".to_string(),
                parsed,
            });
        }
    } else {
        // Project settings
        let project_settings_path = claude_dir.join("settings.json");
        if project_settings_path.exists() {
            let (is_symlink, symlink_target) = is_symlink_with_target(&project_settings_path);
            let content = read_file_content(&project_settings_path);
            let parsed = content.as_ref().and_then(|c| serde_json::from_str(c).ok());
            
            settings.push(SettingsEntity {
                base: BaseEntity {
                    id: generate_id("settings", &project_settings_path.to_string_lossy()),
                    name: "settings.json".to_string(),
                    path: project_settings_path.to_string_lossy().to_string(),
                    scope: scope.to_string(),
                    project_path: project_path.map(String::from),
                    is_symlink,
                    symlink_target,
                    content,
                    last_modified: get_last_modified(&project_settings_path),
                },
                entity_type: "settings".to_string(),
                variant: "project".to_string(),
                parsed,
            });
        }
        
        // Local settings
        let local_settings_path = claude_dir.join("settings.local.json");
        if local_settings_path.exists() {
            let (is_symlink, symlink_target) = is_symlink_with_target(&local_settings_path);
            let content = read_file_content(&local_settings_path);
            let parsed = content.as_ref().and_then(|c| serde_json::from_str(c).ok());
            
            settings.push(SettingsEntity {
                base: BaseEntity {
                    id: generate_id("settings", &local_settings_path.to_string_lossy()),
                    name: "settings.local.json".to_string(),
                    path: local_settings_path.to_string_lossy().to_string(),
                    scope: scope.to_string(),
                    project_path: project_path.map(String::from),
                    is_symlink,
                    symlink_target,
                    content,
                    last_modified: get_last_modified(&local_settings_path),
                },
                entity_type: "settings".to_string(),
                variant: "local".to_string(),
                parsed,
            });
        }
    }
    
    Ok(settings)
}

fn discover_memory_internal(claude_dir: &PathBuf, base_path: &PathBuf, scope: &str, project_path: Option<&str>) -> Result<Vec<MemoryEntity>, String> {
    let mut memory = Vec::new();
    
    // CLAUDE.md in .claude/ directory
    let dotclaude_md_path = claude_dir.join("CLAUDE.md");
    if dotclaude_md_path.exists() {
        let (is_symlink, symlink_target) = is_symlink_with_target(&dotclaude_md_path);
        let content = read_file_content(&dotclaude_md_path);
        
        memory.push(MemoryEntity {
            base: BaseEntity {
                id: generate_id("memory", &dotclaude_md_path.to_string_lossy()),
                name: "CLAUDE.md".to_string(),
                path: dotclaude_md_path.to_string_lossy().to_string(),
                scope: scope.to_string(),
                project_path: project_path.map(String::from),
                is_symlink,
                symlink_target,
                content,
                last_modified: get_last_modified(&dotclaude_md_path),
            },
            entity_type: "memory".to_string(),
            variant: "dotclaude".to_string(),
        });
    }
    
    // CLAUDE.md at root (only for projects)
    if scope == "project" {
        let root_md_path = base_path.join("CLAUDE.md");
        if root_md_path.exists() {
            let (is_symlink, symlink_target) = is_symlink_with_target(&root_md_path);
            let content = read_file_content(&root_md_path);
            
            memory.push(MemoryEntity {
                base: BaseEntity {
                    id: generate_id("memory", &root_md_path.to_string_lossy()),
                    name: "CLAUDE.md".to_string(),
                    path: root_md_path.to_string_lossy().to_string(),
                    scope: scope.to_string(),
                    project_path: project_path.map(String::from),
                    is_symlink,
                    symlink_target,
                    content,
                    last_modified: get_last_modified(&root_md_path),
                },
                entity_type: "memory".to_string(),
                variant: "root".to_string(),
            });
        }
    }
    
    Ok(memory)
}

fn discover_agents_internal(agents_dir: &PathBuf, scope: &str, project_path: Option<&str>) -> Result<Vec<AgentEntity>, String> {
    let mut agents = Vec::new();
    
    if agents_dir.exists() && agents_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(agents_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "md") {
                    let (is_symlink, symlink_target) = is_symlink_with_target(&path);
                    let content = read_file_content(&path);
                    let (frontmatter, _body) = content.as_ref()
                        .map(|c| parse_frontmatter(c))
                        .unwrap_or((None, String::new()));
                    
                    let name = path.file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    
                    agents.push(AgentEntity {
                        base: BaseEntity {
                            id: generate_id("agent", &path.to_string_lossy()),
                            name: name.clone(),
                            path: path.to_string_lossy().to_string(),
                            scope: scope.to_string(),
                            project_path: project_path.map(String::from),
                            is_symlink,
                            symlink_target,
                            content,
                            last_modified: get_last_modified(&path),
                        },
                        entity_type: "agent".to_string(),
                        frontmatter,
                    });
                }
            }
        }
    }
    
    Ok(agents)
}

fn discover_skills_internal(skills_dir: &PathBuf, scope: &str, project_path: Option<&str>) -> Result<Vec<SkillEntity>, String> {
    let mut skills = Vec::new();
    
    if skills_dir.exists() && skills_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(skills_dir) {
            for entry in entries.flatten() {
                let skill_dir = entry.path();
                if skill_dir.is_dir() {
                    let skill_file = skill_dir.join("SKILL.md");
                    if skill_file.exists() {
                        let (is_symlink, symlink_target) = is_symlink_with_target(&skill_file);
                        let content = read_file_content(&skill_file);
                        let (frontmatter, _body) = content.as_ref()
                            .map(|c| parse_frontmatter(c))
                            .unwrap_or((None, String::new()));
                        
                        let skill_name = skill_dir.file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        
                        // Find supporting files
                        let mut supporting_files = Vec::new();
                        if let Ok(skill_entries) = fs::read_dir(&skill_dir) {
                            for skill_entry in skill_entries.flatten() {
                                let file_name = skill_entry.file_name().to_string_lossy().to_string();
                                if file_name != "SKILL.md" {
                                    supporting_files.push(skill_entry.path().to_string_lossy().to_string());
                                }
                            }
                        }
                        
                        skills.push(SkillEntity {
                            base: BaseEntity {
                                id: generate_id("skill", &skill_file.to_string_lossy()),
                                name: skill_name.clone(),
                                path: skill_file.to_string_lossy().to_string(),
                                scope: scope.to_string(),
                                project_path: project_path.map(String::from),
                                is_symlink,
                                symlink_target,
                                content,
                                last_modified: get_last_modified(&skill_file),
                            },
                            entity_type: "skill".to_string(),
                            skill_dir: skill_dir.to_string_lossy().to_string(),
                            frontmatter,
                            supporting_files,
                        });
                    }
                }
            }
        }
    }
    
    Ok(skills)
}

fn discover_commands_internal(commands_dir: &PathBuf, scope: &str, project_path: Option<&str>) -> Result<Vec<CommandEntity>, String> {
    let mut commands = Vec::new();
    
    fn scan_commands_dir(dir: &PathBuf, scope: &str, project_path: Option<&str>, namespace: Option<&str>, commands: &mut Vec<CommandEntity>) {
        if dir.exists() && dir.is_dir() {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    
                    if path.is_dir() {
                        // Subdirectory becomes namespace
                        let subdir_name = path.file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        scan_commands_dir(&path, scope, project_path, Some(&subdir_name), commands);
                    } else if path.extension().map_or(false, |ext| ext == "md") {
                        let (is_symlink, symlink_target) = is_symlink_with_target(&path);
                        let content = read_file_content(&path);
                        let (frontmatter, _body) = content.as_ref()
                            .map(|c| parse_frontmatter(c))
                            .unwrap_or((None, String::new()));
                        
                        let name = path.file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        
                        commands.push(CommandEntity {
                            base: BaseEntity {
                                id: generate_id("command", &path.to_string_lossy()),
                                name: name.clone(),
                                path: path.to_string_lossy().to_string(),
                                scope: scope.to_string(),
                                project_path: project_path.map(String::from),
                                is_symlink,
                                symlink_target,
                                content,
                                last_modified: get_last_modified(&path),
                            },
                            entity_type: "command".to_string(),
                            namespace: namespace.map(String::from),
                            frontmatter,
                        });
                    }
                }
            }
        }
    }
    
    scan_commands_dir(commands_dir, scope, project_path, None, &mut commands);
    Ok(commands)
}

fn discover_plugins_internal(plugins_dir: &PathBuf, scope: &str, project_path: Option<&str>) -> Result<Vec<PluginEntity>, String> {
    let mut plugins = Vec::new();
    
    if plugins_dir.exists() && plugins_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(plugins_dir) {
            for entry in entries.flatten() {
                let plugin_dir = entry.path();
                if plugin_dir.is_dir() {
                    let manifest_path = plugin_dir.join(".claude-plugin").join("plugin.json");
                    if manifest_path.exists() {
                        let (is_symlink, symlink_target) = is_symlink_with_target(&plugin_dir);
                        let manifest = parse_json_file(&manifest_path);
                        
                        let name = plugin_dir.file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        
                        plugins.push(PluginEntity {
                            base: BaseEntity {
                                id: generate_id("plugin", &plugin_dir.to_string_lossy()),
                                name: name.clone(),
                                path: manifest_path.to_string_lossy().to_string(),
                                scope: scope.to_string(),
                                project_path: project_path.map(String::from),
                                is_symlink,
                                symlink_target,
                                content: read_file_content(&manifest_path),
                                last_modified: get_last_modified(&manifest_path),
                            },
                            entity_type: "plugin".to_string(),
                            plugin_dir: plugin_dir.to_string_lossy().to_string(),
                            manifest,
                            has_commands: plugin_dir.join("commands").exists(),
                            has_agents: plugin_dir.join("agents").exists(),
                            has_skills: plugin_dir.join("skills").exists(),
                            has_hooks: plugin_dir.join("hooks").exists() || plugin_dir.join("hooks.json").exists(),
                            has_mcp: plugin_dir.join(".mcp.json").exists(),
                            has_lsp: plugin_dir.join(".lsp.json").exists(),
                        });
                    }
                }
            }
        }
    }
    
    Ok(plugins)
}

/// Extract enabled plugins from settings.json (registry plugins like @claude-plugins-official)
fn extract_enabled_plugins_from_settings(settings_path: &PathBuf, scope: &str) -> Result<Vec<PluginEntity>, String> {
    // This is now handled by discover_installed_plugins which reads from installed_plugins.json
    Ok(Vec::new())
}

/// Discover installed plugins from ~/.claude/plugins/installed_plugins.json
fn discover_installed_plugins(home: &PathBuf) -> Result<Vec<PluginEntity>, String> {
    let mut plugins = Vec::new();
    
    let installed_plugins_path = home.join(".claude").join("plugins").join("installed_plugins.json");
    
    if installed_plugins_path.exists() {
        if let Some(installed) = parse_json_file(&installed_plugins_path) {
            if let Some(plugins_obj) = installed.get("plugins").and_then(|p| p.as_object()) {
                for (plugin_full_name, installations) in plugins_obj {
                    // plugin_full_name is like "frontend-design@claude-plugins-official"
                    let parts: Vec<&str> = plugin_full_name.split('@').collect();
                    let plugin_name = parts.first().unwrap_or(&plugin_full_name.as_str()).to_string();
                    let marketplace = parts.get(1).map(|s| s.to_string());
                    
                    if let Some(installations_arr) = installations.as_array() {
                        for installation in installations_arr {
                            let scope_str = installation.get("scope")
                                .and_then(|s| s.as_str())
                                .unwrap_or("user");
                            let install_path = installation.get("installPath")
                                .and_then(|p| p.as_str())
                                .unwrap_or("");
                            let version = installation.get("version")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            let project_path = installation.get("projectPath")
                                .and_then(|p| p.as_str())
                                .map(String::from);
                            let installed_at = installation.get("installedAt")
                                .and_then(|t| t.as_str())
                                .unwrap_or("");
                            
                            // Try to read the plugin manifest from the install path
                            let install_path_buf = PathBuf::from(install_path);
                            let manifest_path = install_path_buf.join(".claude-plugin").join("plugin.json");
                            let manifest = if manifest_path.exists() {
                                parse_json_file(&manifest_path)
                            } else {
                                None
                            };
                            
                            // Get description from manifest if available
                            let description = manifest.as_ref()
                                .and_then(|m| m.get("description"))
                                .and_then(|d| d.as_str())
                                .map(String::from);
                            
                            plugins.push(PluginEntity {
                                base: BaseEntity {
                                    id: generate_id("plugin", &format!("{}_{}", plugin_full_name, scope_str)),
                                    name: plugin_name.clone(),
                                    path: install_path.to_string(),
                                    scope: scope_str.to_string(),
                                    project_path: project_path.clone(),
                                    is_symlink: false,
                                    symlink_target: None,
                                    content: description,
                                    last_modified: 0,
                                },
                                entity_type: "plugin".to_string(),
                                plugin_dir: install_path.to_string(),
                                manifest,
                                has_commands: install_path_buf.join("commands").exists(),
                                has_agents: install_path_buf.join("agents").exists(),
                                has_skills: install_path_buf.join("skills").exists(),
                                has_hooks: install_path_buf.join("hooks").exists() || install_path_buf.join("hooks.json").exists(),
                                has_mcp: install_path_buf.join(".mcp.json").exists(),
                                has_lsp: install_path_buf.join(".lsp.json").exists(),
                            });
                        }
                    }
                }
            }
        }
    }
    
    Ok(plugins)
}

fn extract_hooks_internal(settings_path: &PathBuf, source: &str) -> Result<Vec<HookEntity>, String> {
    let mut hooks = Vec::new();
    
    if settings_path.exists() {
        if let Some(settings) = parse_json_file(settings_path) {
            if let Some(hooks_obj) = settings.get("hooks").and_then(|h| h.as_object()) {
                for (event_name, matchers) in hooks_obj {
                    if let Some(matchers_arr) = matchers.as_array() {
                        for (idx, matcher_obj) in matchers_arr.iter().enumerate() {
                            let matcher = matcher_obj.get("matcher")
                                .and_then(|m| m.as_str())
                                .map(String::from);
                            
                            let hook_defs: Vec<HookDefinition> = matcher_obj.get("hooks")
                                .and_then(|h| h.as_array())
                                .map(|arr| {
                                    arr.iter().filter_map(|h| {
                                        serde_json::from_value(h.clone()).ok()
                                    }).collect()
                                })
                                .unwrap_or_default();
                            
                            if !hook_defs.is_empty() {
                                // ID should be based on path + event + index only (not source)
                                // This ensures the same hook from the same file isn't duplicated
                                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                                settings_path.to_string_lossy().hash(&mut hasher);
                                event_name.hash(&mut hasher);
                                idx.hash(&mut hasher);
                                let id = format!("hook_{}_{}_{:x}", 
                                    event_name, idx,
                                    hasher.finish()
                                );
                                
                                hooks.push(HookEntity {
                                    id,
                                    entity_type: "hook".to_string(),
                                    event: event_name.clone(),
                                    matcher,
                                    hooks: hook_defs,
                                    source: source.to_string(),
                                    source_path: settings_path.to_string_lossy().to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    
    Ok(hooks)
}

fn discover_mcp_from_claude_json(home: &PathBuf) -> Result<Vec<McpServerEntity>, String> {
    let mut servers = Vec::new();
    
    let claude_json_path = home.join(".claude.json");
    if claude_json_path.exists() {
        if let Some(config) = parse_json_file(&claude_json_path) {
            if let Some(mcp_servers) = config.get("mcpServers").and_then(|m| m.as_object()) {
                for (name, server_config) in mcp_servers {
                    let transport = server_config.get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or_else(|| {
                            if server_config.get("command").is_some() { "stdio" }
                            else if server_config.get("url").is_some() { "http" }
                            else { "unknown" }
                        });
                    
                    servers.push(McpServerEntity {
                        id: generate_id("mcp", &format!("user_{}", name)),
                        entity_type: "mcp".to_string(),
                        name: name.clone(),
                        scope: "user".to_string(),
                        transport: transport.to_string(),
                        config: serde_json::from_value(server_config.clone()).unwrap_or(McpServerConfig {
                            transport_type: None,
                            command: None,
                            args: None,
                            url: None,
                            env: None,
                            headers: None,
                        }),
                        source_path: claude_json_path.to_string_lossy().to_string(),
                        is_from_plugin: false,
                        plugin_name: None,
                    });
                }
            }
        }
    }
    
    Ok(servers)
}

fn discover_mcp_from_project(project_path: &PathBuf) -> Result<Vec<McpServerEntity>, String> {
    let mut servers = Vec::new();
    
    let mcp_json_path = project_path.join(".mcp.json");
    if mcp_json_path.exists() {
        if let Some(config) = parse_json_file(&mcp_json_path) {
            // Handle both formats: { "mcpServers": {...} } and { "server-name": {...} }
            let mcp_obj = config.get("mcpServers")
                .and_then(|m| m.as_object())
                .or_else(|| config.as_object());
            
            if let Some(mcp_servers) = mcp_obj {
                for (name, server_config) in mcp_servers {
                    if name == "mcpServers" { continue; } // Skip wrapper key
                    
                    let transport = server_config.get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or_else(|| {
                            if server_config.get("command").is_some() { "stdio" }
                            else if server_config.get("url").is_some() { "http" }
                            else { "unknown" }
                        });
                    
                    servers.push(McpServerEntity {
                        id: generate_id("mcp", &format!("project_{}_{}", project_path.to_string_lossy(), name)),
                        entity_type: "mcp".to_string(),
                        name: name.clone(),
                        scope: "project".to_string(),
                        transport: transport.to_string(),
                        config: serde_json::from_value(server_config.clone()).unwrap_or(McpServerConfig {
                            transport_type: None,
                            command: None,
                            args: None,
                            url: None,
                            env: None,
                            headers: None,
                        }),
                        source_path: mcp_json_path.to_string_lossy().to_string(),
                        is_from_plugin: false,
                        plugin_name: None,
                    });
                }
            }
        }
    }
    
    Ok(servers)
}

fn find_duplicates_internal(
    agents: &[AgentEntity],
    skills: &[SkillEntity],
    commands: &[CommandEntity],
) -> Result<Vec<DuplicateGroup>, String> {
    let mut duplicates = Vec::new();
    
    // Find duplicate agents
    let mut agent_map: HashMap<String, Vec<&AgentEntity>> = HashMap::new();
    for agent in agents {
        agent_map.entry(agent.base.name.clone()).or_default().push(agent);
    }
    for (name, entities) in agent_map {
        if entities.len() > 1 {
            duplicates.push(DuplicateGroup {
                name,
                entity_type: "agent".to_string(),
                entities: entities.iter().enumerate().map(|(idx, e)| {
                    DuplicateEntity {
                        id: e.base.id.clone(),
                        path: e.base.path.clone(),
                        scope: e.base.scope.clone(),
                        project_path: e.base.project_path.clone(),
                        precedence: if e.base.scope == "project" { idx as u32 } else { (idx + 100) as u32 },
                    }
                }).collect(),
            });
        }
    }
    
    // Find duplicate skills
    let mut skill_map: HashMap<String, Vec<&SkillEntity>> = HashMap::new();
    for skill in skills {
        skill_map.entry(skill.base.name.clone()).or_default().push(skill);
    }
    for (name, entities) in skill_map {
        if entities.len() > 1 {
            duplicates.push(DuplicateGroup {
                name,
                entity_type: "skill".to_string(),
                entities: entities.iter().enumerate().map(|(idx, e)| {
                    DuplicateEntity {
                        id: e.base.id.clone(),
                        path: e.base.path.clone(),
                        scope: e.base.scope.clone(),
                        project_path: e.base.project_path.clone(),
                        precedence: if e.base.scope == "project" { idx as u32 } else { (idx + 100) as u32 },
                    }
                }).collect(),
            });
        }
    }
    
    // Find duplicate commands
    let mut command_map: HashMap<String, Vec<&CommandEntity>> = HashMap::new();
    for command in commands {
        command_map.entry(command.base.name.clone()).or_default().push(command);
    }
    for (name, entities) in command_map {
        if entities.len() > 1 {
            duplicates.push(DuplicateGroup {
                name,
                entity_type: "command".to_string(),
                entities: entities.iter().enumerate().map(|(idx, e)| {
                    DuplicateEntity {
                        id: e.base.id.clone(),
                        path: e.base.path.clone(),
                        scope: e.base.scope.clone(),
                        project_path: e.base.project_path.clone(),
                        precedence: if e.base.scope == "project" { idx as u32 } else { (idx + 100) as u32 },
                    }
                }).collect(),
            });
        }
    }
    
    Ok(duplicates)
}

// ============================================================================
// Public Tauri Commands
// ============================================================================

#[tauri::command]
pub fn discover_settings() -> Result<Vec<SettingsEntity>, String> {
    let home = get_home_dir().ok_or("Could not find home directory")?;
    let global_claude_path = home.join(".claude");
    discover_settings_internal(&global_claude_path, "global", None)
}

#[tauri::command]
pub fn discover_memory() -> Result<Vec<MemoryEntity>, String> {
    let home = get_home_dir().ok_or("Could not find home directory")?;
    let global_claude_path = home.join(".claude");
    discover_memory_internal(&global_claude_path, &home, "global", None)
}

#[tauri::command]
pub fn discover_agents() -> Result<Vec<AgentEntity>, String> {
    let home = get_home_dir().ok_or("Could not find home directory")?;
    let agents_dir = home.join(".claude").join("agents");
    discover_agents_internal(&agents_dir, "global", None)
}

#[tauri::command]
pub fn discover_skills() -> Result<Vec<SkillEntity>, String> {
    let home = get_home_dir().ok_or("Could not find home directory")?;
    let skills_dir = home.join(".claude").join("skills");
    discover_skills_internal(&skills_dir, "global", None)
}

#[tauri::command]
pub fn discover_commands() -> Result<Vec<CommandEntity>, String> {
    let home = get_home_dir().ok_or("Could not find home directory")?;
    let commands_dir = home.join(".claude").join("commands");
    discover_commands_internal(&commands_dir, "global", None)
}

#[tauri::command]
pub fn discover_plugins() -> Result<Vec<PluginEntity>, String> {
    let home = get_home_dir().ok_or("Could not find home directory")?;
    let plugins_dir = home.join(".claude").join("plugins");
    discover_plugins_internal(&plugins_dir, "global", None)
}

#[tauri::command]
pub fn discover_mcp_servers() -> Result<Vec<McpServerEntity>, String> {
    let home = get_home_dir().ok_or("Could not find home directory")?;
    discover_mcp_from_claude_json(&home)
}

#[tauri::command]
pub fn extract_hooks(settings_path: String) -> Result<Vec<HookEntity>, String> {
    let path = PathBuf::from(&settings_path);
    extract_hooks_internal(&path, "global")
}

#[tauri::command]
pub fn scan_projects(base_paths: Vec<String>) -> Result<Vec<ProjectInfo>, String> {
    let mut projects = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();
    let max_depth = 5u32;
    
    // Directories to skip entirely (won't descend into these)
    let skip_dirs: std::collections::HashSet<&str> = [
        // Build/dependency directories
        "node_modules", "target", "build", "dist", ".git", "vendor", 
        "__pycache__", ".venv", "venv", "env", ".env",
        "Pods", "DerivedData", ".build", "Packages",
        // System/cache directories
        "Library", "Applications", ".Trash", ".cache", ".npm", ".cargo",
        ".rustup", ".local", ".config", "Caches", "Cache",
        // Large directories unlikely to have projects
        "Movies", "Music", "Pictures", "Photos", "Downloads",
        ".docker", ".gradle", ".m2", ".pub-cache",
        // IDE/editor directories
        ".idea", ".vscode", ".vs",
    ].into_iter().collect();
    
    // Use a stack for iterative traversal instead of recursion
    let mut stack: Vec<(PathBuf, u32)> = base_paths
        .iter()
        .map(|p| (PathBuf::from(p), 0u32))
        .collect();
    
    while let Some((path, depth)) = stack.pop() {
        if depth > max_depth {
            continue;
        }
        
        if !path.is_dir() {
            continue;
        }
        
        // Get directory name for filtering
        let dir_name = path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        
        // Skip hidden directories (except at depth 0 for home dir)
        if depth > 0 && dir_name.starts_with('.') && dir_name != ".claude" {
            continue;
        }
        
        // Skip known non-project directories
        if skip_dirs.contains(dir_name.as_ref()) {
            continue;
        }
        
        // Check if this directory is a project (has .claude/ or CLAUDE.md or .mcp.json)
        let claude_dir = path.join(".claude");
        let has_claude = claude_dir.exists() || path.join("CLAUDE.md").exists() || path.join(".mcp.json").exists();
        
        if has_claude {
            let path_str = path.to_string_lossy().into_owned();
            
            // Skip if we've already seen this path (deduplication)
            if seen_paths.contains(&path_str) {
                continue;
            }
            seen_paths.insert(path_str.clone());
            
            let name = path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned());
            
            projects.push(ProjectInfo {
                path: path_str,
                name,
                has_claude_dir: claude_dir.exists(),
                has_mcp_json: path.join(".mcp.json").exists(),
                has_claude_md: claude_dir.join("CLAUDE.md").exists(),
                has_root_claude_md: path.join("CLAUDE.md").exists(),
                entity_counts: EntityCounts {
                    settings: 0,
                    memory: 0,
                    agents: 0,
                    skills: 0,
                    commands: 0,
                    plugins: 0,
                    hooks: 0,
                    mcp: 0,
                },
            });
        }
        
        // Add subdirectories to stack
        if let Ok(entries) = fs::read_dir(&path) {
            for entry in entries.flatten() {
                let subdir = entry.path();
                if subdir.is_dir() {
                    stack.push((subdir, depth + 1));
                }
            }
        }
    }
    
    Ok(projects)
}

#[tauri::command]
pub fn find_duplicates() -> Result<Vec<DuplicateGroup>, String> {
    let home = get_home_dir().ok_or("Could not find home directory")?;
    let claude_dir = home.join(".claude");
    
    let agents = discover_agents_internal(&claude_dir.join("agents"), "global", None)?;
    let skills = discover_skills_internal(&claude_dir.join("skills"), "global", None)?;
    let commands = discover_commands_internal(&claude_dir.join("commands"), "global", None)?;
    
    find_duplicates_internal(&agents, &skills, &commands)
}

#[tauri::command]
pub fn check_symlink(path: String) -> Result<Option<SymlinkInfo>, String> {
    let path_buf = PathBuf::from(&path);
    
    if path_buf.is_symlink() {
        let target = fs::read_link(&path_buf)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let target_exists = PathBuf::from(&target).exists();
        
        Ok(Some(SymlinkInfo {
            path,
            target,
            target_exists,
            entity_type: None,
            entity_id: None,
        }))
    } else {
        Ok(None)
    }
}

// ============================================================================
// File Operations
// ============================================================================

#[tauri::command]
pub fn read_file(path: String) -> Result<String, String> {
    fs::read_to_string(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn write_file(path: String, content: String) -> Result<(), String> {
    if let Some(parent) = PathBuf::from(&path).parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&path, content).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn file_exists(path: String) -> bool {
    PathBuf::from(&path).exists()
}

#[tauri::command]
pub fn delete_file(path: String) -> Result<(), String> {
    let path = PathBuf::from(&path);
    if !path.exists() {
        return Err("File does not exist".to_string());
    }
    fs::remove_file(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_directory(path: String) -> Result<(), String> {
    let path = PathBuf::from(&path);
    if !path.exists() {
        return Err("Directory does not exist".to_string());
    }
    fs::remove_dir_all(&path).map_err(|e| e.to_string())
}

// ============================================================================
// Entity Creation
// ============================================================================

#[tauri::command]
pub fn create_entity(
    entity_type: String,
    name: String,
    scope: String,
    project_path: Option<String>,
    content: Option<String>,
) -> Result<String, String> {
    let home = get_home_dir().ok_or("Could not find home directory")?;
    
    let base_dir = if scope == "global" {
        home.join(".claude")
    } else {
        project_path.as_ref()
            .map(|p| PathBuf::from(p).join(".claude"))
            .ok_or("Project path required for project-scoped entities")?
    };
    
    let (file_path, file_content) = match entity_type.as_str() {
        "agent" => {
            let path = base_dir.join("agents").join(format!("{}.md", name));
            let content = content.unwrap_or_else(|| {
                format!(
                    "---\nname: {}\ndescription: A custom agent\ntools: Read, Grep, Glob\nmodel: sonnet\n---\n\nYou are a specialized agent.\n\nWhen invoked:\n1. Analyze the task\n2. Execute appropriate actions\n3. Report results\n",
                    name
                )
            });
            (path, content)
        }
        "skill" => {
            let skill_dir = base_dir.join("skills").join(&name);
            fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;
            let path = skill_dir.join("SKILL.md");
            let content = content.unwrap_or_else(|| {
                format!(
                    "---\nname: {}\ndescription: A custom skill\n---\n\n# {} Skill\n\n## When to use this skill\n\nUse this skill when...\n\n## Instructions\n\nFollow these steps...\n",
                    name, name
                )
            });
            (path, content)
        }
        "command" => {
            let path = base_dir.join("commands").join(format!("{}.md", name));
            let content = content.unwrap_or_else(|| {
                format!(
                    "---\ndescription: A custom command\n---\n\n# {} Command\n\n$ARGUMENTS\n",
                    name
                )
            });
            (path, content)
        }
        "memory" => {
            let path = if scope == "global" {
                base_dir.join("CLAUDE.md")
            } else {
                project_path.as_ref()
                    .map(|p| PathBuf::from(p).join("CLAUDE.md"))
                    .ok_or("Project path required")?
            };
            let content = content.unwrap_or_else(|| {
                "# Project Memory\n\n## Overview\n\nThis file contains project-specific context and instructions for Claude.\n\n## Guidelines\n\n- ...\n".to_string()
            });
            (path, content)
        }
        _ => return Err(format!("Unknown entity type: {}", entity_type)),
    };
    
    // Create parent directories
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    
    // Write the file
    fs::write(&file_path, &file_content).map_err(|e| e.to_string())?;
    
    Ok(file_path.to_string_lossy().to_string())
}

// ============================================================================
// Legacy Commands (backward compatibility)
// ============================================================================

#[tauri::command]
pub fn discover_configs(project_path: Option<String>) -> Result<DiscoveredConfigs, String> {
    let home = get_home_dir().ok_or("Could not find home directory")?;
    let config_dir = get_config_dir().ok_or("Could not find config directory")?;
    
    let mut claude_code = Vec::new();
    let mut opencode = Vec::new();
    let mut agents_md = Vec::new();
    let mut agents = Vec::new();
    let mut skills = Vec::new();

    // Claude Code global configs
    let claude_dir = home.join(".claude");
    
    let settings_path = claude_dir.join("settings.json");
    let (exists, content) = if settings_path.exists() {
        (true, read_file_content(&settings_path))
    } else {
        (false, None)
    };
    claude_code.push(ConfigFile {
        path: settings_path.to_string_lossy().to_string(),
        name: "settings.json".to_string(),
        tool: "claude-code".to_string(),
        scope: "global".to_string(),
        file_type: "json".to_string(),
        exists,
        content,
    });

    let claude_md_path = claude_dir.join("CLAUDE.md");
    let (exists, content) = if claude_md_path.exists() {
        (true, read_file_content(&claude_md_path))
    } else {
        (false, None)
    };
    claude_code.push(ConfigFile {
        path: claude_md_path.to_string_lossy().to_string(),
        name: "CLAUDE.md".to_string(),
        tool: "claude-code".to_string(),
        scope: "global".to_string(),
        file_type: "markdown".to_string(),
        exists,
        content,
    });

    // Claude Code agents
    let claude_agents_dir = claude_dir.join("agents");
    agents.extend(discover_agents_legacy(&claude_agents_dir, "claude-code", "global"));

    // OpenCode global configs  
    let opencode_dir = config_dir.join("opencode");
    
    let opencode_json_path = opencode_dir.join("opencode.json");
    let (exists, content) = if opencode_json_path.exists() {
        (true, read_file_content(&opencode_json_path))
    } else {
        (false, None)
    };
    opencode.push(ConfigFile {
        path: opencode_json_path.to_string_lossy().to_string(),
        name: "opencode.json".to_string(),
        tool: "opencode".to_string(),
        scope: "global".to_string(),
        file_type: "json".to_string(),
        exists,
        content,
    });

    // OpenCode agents
    let opencode_agents_dir = opencode_dir.join("agent");
    agents.extend(discover_agents_legacy(&opencode_agents_dir, "opencode", "global"));

    // Skills
    let opencode_skills_dir = opencode_dir.join("skill");
    skills.extend(discover_skills_legacy(&opencode_skills_dir, "opencode", "global"));

    let claude_skills_dir = claude_dir.join("skills");
    skills.extend(discover_skills_legacy(&claude_skills_dir, "claude-code", "global"));

    // Project-specific configs
    if let Some(project) = project_path {
        let project_path = PathBuf::from(&project);
        let claude_project_dir = project_path.join(".claude");
        
        let project_settings = claude_project_dir.join("settings.json");
        let (exists, content) = if project_settings.exists() {
            (true, read_file_content(&project_settings))
        } else {
            (false, None)
        };
        claude_code.push(ConfigFile {
            path: project_settings.to_string_lossy().to_string(),
            name: "settings.json".to_string(),
            tool: "claude-code".to_string(),
            scope: "project".to_string(),
            file_type: "json".to_string(),
            exists,
            content,
        });

        let project_settings_local = claude_project_dir.join("settings.local.json");
        let (exists, content) = if project_settings_local.exists() {
            (true, read_file_content(&project_settings_local))
        } else {
            (false, None)
        };
        claude_code.push(ConfigFile {
            path: project_settings_local.to_string_lossy().to_string(),
            name: "settings.local.json".to_string(),
            tool: "claude-code".to_string(),
            scope: "project".to_string(),
            file_type: "json".to_string(),
            exists,
            content,
        });

        let project_claude_md = project_path.join("CLAUDE.md");
        let (exists, content) = if project_claude_md.exists() {
            (true, read_file_content(&project_claude_md))
        } else {
            (false, None)
        };
        claude_code.push(ConfigFile {
            path: project_claude_md.to_string_lossy().to_string(),
            name: "CLAUDE.md".to_string(),
            tool: "claude-code".to_string(),
            scope: "project".to_string(),
            file_type: "markdown".to_string(),
            exists,
            content,
        });

        // Project agents
        let claude_project_agents = claude_project_dir.join("agents");
        agents.extend(discover_agents_legacy(&claude_project_agents, "claude-code", "project"));

        // Project AGENTS.md
        let project_agents_md = project_path.join("AGENTS.md");
        let (exists, content) = if project_agents_md.exists() {
            (true, read_file_content(&project_agents_md))
        } else {
            (false, None)
        };
        agents_md.push(ConfigFile {
            path: project_agents_md.to_string_lossy().to_string(),
            name: "AGENTS.md".to_string(),
            tool: "agents-md".to_string(),
            scope: "project".to_string(),
            file_type: "markdown".to_string(),
            exists,
            content,
        });

        // Project skills
        let claude_project_skills = claude_project_dir.join("skills");
        skills.extend(discover_skills_legacy(&claude_project_skills, "claude-code", "project"));
    }

    Ok(DiscoveredConfigs {
        claude_code,
        opencode,
        agents_md,
        agents,
        skills,
    })
}

fn discover_agents_legacy(dir: &PathBuf, tool: &str, scope: &str) -> Vec<AgentFile> {
    let mut agents = Vec::new();
    
    if dir.exists() && dir.is_dir() {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "md") {
                    if let Some(content) = read_file_content(&path) {
                        let (frontmatter, body) = parse_frontmatter(&content);
                        let name = path.file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        
                        agents.push(AgentFile {
                            path: path.to_string_lossy().to_string(),
                            name,
                            tool: tool.to_string(),
                            scope: scope.to_string(),
                            frontmatter,
                            content: body,
                        });
                    }
                }
            }
        }
    }
    
    agents
}

fn discover_skills_legacy(dir: &PathBuf, tool: &str, scope: &str) -> Vec<AgentFile> {
    let mut skills = Vec::new();
    
    if dir.exists() && dir.is_dir() {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let skill_dir = entry.path();
                if skill_dir.is_dir() {
                    let skill_file = skill_dir.join("SKILL.md");
                    if skill_file.exists() {
                        if let Some(content) = read_file_content(&skill_file) {
                            let (frontmatter, body) = parse_frontmatter(&content);
                            let name = skill_dir.file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default();
                            
                            skills.push(AgentFile {
                                path: skill_file.to_string_lossy().to_string(),
                                name,
                                tool: tool.to_string(),
                                scope: scope.to_string(),
                                frontmatter,
                                content: body,
                            });
                        }
                    }
                }
            }
        }
    }
    
    skills
}

#[tauri::command]
pub fn create_agent(path: String, name: String, description: String, tools: Vec<String>, model: String, prompt: String) -> Result<(), String> {
    let frontmatter = format!(
        "---\nname: {}\ndescription: {}\ntools: {}\nmodel: {}\n---\n\n{}",
        name,
        description,
        tools.join(", "),
        model,
        prompt
    );
    
    write_file(path, frontmatter)
}

#[tauri::command]
pub fn create_skill(base_path: String, name: String, description: String, content: String) -> Result<String, String> {
    let skill_dir = PathBuf::from(&base_path).join(&name);
    fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;
    
    let skill_file = skill_dir.join("SKILL.md");
    let file_content = format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}",
        name,
        description,
        content
    );
    
    fs::write(&skill_file, file_content).map_err(|e| e.to_string())?;
    Ok(skill_file.to_string_lossy().to_string())
}

#[tauri::command]
pub fn delete_skill(path: String) -> Result<(), String> {
    let path = PathBuf::from(&path);
    
    if let Some(skill_dir) = path.parent() {
        if skill_dir.is_dir() {
            fs::remove_dir_all(skill_dir).map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
    
    if path.exists() {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    
    Ok(())
}
