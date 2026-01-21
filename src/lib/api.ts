// ============================================================================
// Agent Studio - API Layer
// Tauri IPC communication with comprehensive Claude Code entity management
// ============================================================================

import { invoke } from '@tauri-apps/api/core';
import { open as shellOpen } from '@tauri-apps/plugin-shell';
import type {
  DiscoveryResult,
  SettingsEntity,
  MemoryEntity,
  AgentEntity,
  SkillEntity,
  CommandEntity,
  HookEntity,
  PluginEntity,
  McpServerEntity,
  ProjectInfo,
  DuplicateGroup,
  SymlinkInfo,
  EntityType,
  ConfigState,
} from './types';

// ============================================================================
// Discovery API
// ============================================================================

/**
 * Discover all Claude Code entities from global and project locations
 */
export async function discoverAll(projectPaths?: string[]): Promise<DiscoveryResult> {
  return invoke('discover_all', { projectPaths });
}

/**
 * Scan directories for projects with Claude Code configuration
 */
export async function scanProjects(basePaths: string[]): Promise<ProjectInfo[]> {
  return invoke('scan_projects', { basePaths });
}

// ============================================================================
// Entity-Specific Discovery
// ============================================================================

export async function discoverSettings(): Promise<SettingsEntity[]> {
  return invoke('discover_settings');
}

export async function discoverMemory(): Promise<MemoryEntity[]> {
  return invoke('discover_memory');
}

export async function discoverAgents(): Promise<AgentEntity[]> {
  return invoke('discover_agents');
}

export async function discoverSkills(): Promise<SkillEntity[]> {
  return invoke('discover_skills');
}

export async function discoverCommands(): Promise<CommandEntity[]> {
  return invoke('discover_commands');
}

export async function discoverPlugins(): Promise<PluginEntity[]> {
  return invoke('discover_plugins');
}

export async function discoverMcpServers(): Promise<McpServerEntity[]> {
  return invoke('discover_mcp_servers');
}

export async function extractHooks(settingsPath: string): Promise<HookEntity[]> {
  return invoke('extract_hooks', { settingsPath });
}

// ============================================================================
// Analysis API
// ============================================================================

export async function findDuplicates(): Promise<DuplicateGroup[]> {
  return invoke('find_duplicates');
}

export async function checkSymlink(path: string): Promise<SymlinkInfo | null> {
  return invoke('check_symlink', { path });
}

// ============================================================================
// Config State API (AGENTS.md / CLAUDE.md consistency)
// ============================================================================

/**
 * Get the configuration state for a project (AGENTS.md / CLAUDE.md setup)
 */
export async function getProjectConfigState(projectPath: string): Promise<ConfigState> {
  return invoke('get_project_config_state', { projectPath });
}

/**
 * Fix the configuration state for a project
 * - missing_symlink: Creates CLAUDE.md → AGENTS.md symlink
 * - needs_migration: Moves CLAUDE.md content to AGENTS.md, creates symlink
 * - empty: Creates empty AGENTS.md and CLAUDE.md symlink
 * - conflict: Returns error (requires manual resolution)
 */
export async function fixProjectConfig(projectPath: string): Promise<string> {
  return invoke('fix_project_config', { projectPath });
}

// ============================================================================
// File Operations
// ============================================================================

export async function readFile(path: string): Promise<string> {
  return invoke('read_file', { path });
}

export async function writeFile(path: string, content: string): Promise<void> {
  return invoke('write_file', { path, content });
}

export async function fileExists(path: string): Promise<boolean> {
  return invoke('file_exists', { path });
}

export async function deleteFile(path: string): Promise<void> {
  return invoke('delete_file', { path });
}

export async function deleteDirectory(path: string): Promise<void> {
  return invoke('delete_directory', { path });
}

// ============================================================================
// Entity Actions
// ============================================================================

/**
 * Copy an entity to a new location (global or project scope)
 */
export async function copyEntity(
  sourcePath: string,
  entityType: EntityType,
  targetScope: 'global' | 'project',
  targetProjectPath?: string,
  newName?: string,
  tool: 'claude' | 'opencode' = 'claude'
): Promise<string> {
  return invoke('copy_entity', { 
    sourcePath, 
    entityType, 
    targetScope, 
    targetProjectPath, 
    newName,
    tool 
  });
}

/**
 * Create a symlink to an entity in a new location
 */
export async function createEntitySymlink(
  sourcePath: string,
  entityType: EntityType,
  targetScope: 'global' | 'project',
  targetProjectPath?: string,
  tool: 'claude' | 'opencode' = 'claude'
): Promise<string> {
  return invoke('create_entity_symlink', { 
    sourcePath, 
    entityType, 
    targetScope, 
    targetProjectPath,
    tool 
  });
}

/**
 * Rename an entity (move to new name in same directory)
 */
export async function renameEntity(
  sourcePath: string,
  newName: string,
  entityType: EntityType
): Promise<string> {
  return invoke('rename_entity', { sourcePath, newName, entityType });
}

/**
 * Delete an entity (file or skill directory)
 */
export async function deleteEntity(
  path: string,
  entityType: EntityType
): Promise<void> {
  return invoke('delete_entity', { path, entityType });
}

/**
 * Duplicate an entity within the same scope (creates a copy with new name)
 */
export async function duplicateEntity(
  sourcePath: string,
  entityType: EntityType
): Promise<string> {
  return invoke('duplicate_entity', { sourcePath, entityType });
}

// ============================================================================
// Entity Creation
// ============================================================================

export async function createEntity(
  entityType: EntityType,
  name: string,
  scope: 'global' | 'project',
  projectPath?: string,
  content?: string,
  tool?: 'claude' | 'opencode'
): Promise<string> {
  return invoke('create_entity', { entityType, name, scope, projectPath, content, tool });
}

// ============================================================================
// Utility
// ============================================================================

export async function getHomeDirectory(): Promise<string> {
  return invoke('get_home_directory');
}

export async function getConfigDirectory(): Promise<string> {
  return invoke('get_config_directory');
}

export async function getGlobalClaudePath(): Promise<string> {
  return invoke('get_global_claude_path');
}

/**
 * Open a path in the system file browser
 */
export async function openInFinder(path: string): Promise<void> {
  // Use macOS 'open' command to open in Finder
  await shellOpen(path);
}

// ============================================================================
// Legacy API (backward compatibility)
// ============================================================================

// Legacy type for backward compatibility
interface LegacyDiscoveredConfigs {
  claude_code: unknown[];
  opencode: unknown[];
  agents_md: unknown[];
  agents: unknown[];
  skills: unknown[];
}

export async function discoverConfigs(projectPath?: string): Promise<LegacyDiscoveredConfigs> {
  return invoke('discover_configs', { projectPath });
}

export async function createAgent(
  path: string,
  name: string,
  description: string,
  tools: string[],
  model: string,
  prompt: string
): Promise<void> {
  return invoke('create_agent', { path, name, description, tools, model, prompt });
}

export async function createSkill(
  basePath: string,
  name: string,
  description: string,
  content: string
): Promise<string> {
  return invoke('create_skill', { basePath, name, description, content });
}

export async function deleteSkill(path: string): Promise<void> {
  return invoke('delete_skill', { path });
}

// ============================================================================
// Frontmatter Utilities
// ============================================================================

/**
 * Parse YAML frontmatter from markdown content
 */
export function parseFrontmatter(content: string): { 
  frontmatter: Record<string, unknown> | null; 
  body: string 
} {
  if (!content.startsWith('---')) {
    return { frontmatter: null, body: content };
  }
  
  const endIndex = content.indexOf('---', 3);
  if (endIndex === -1) {
    return { frontmatter: null, body: content };
  }
  
  const yamlStr = content.slice(3, endIndex).trim();
  const body = content.slice(endIndex + 3).trim();
  
  try {
    const frontmatter: Record<string, unknown> = {};
    const lines = yamlStr.split('\n');
    let currentKey = '';
    let inArray = false;
    const arrayValues: string[] = [];
    
    for (const line of lines) {
      const trimmedLine = line.trim();
      
      // Handle array continuation
      if (inArray) {
        if (trimmedLine.startsWith('- ')) {
          arrayValues.push(trimmedLine.slice(2).trim());
          continue;
        } else {
          // End of array
          frontmatter[currentKey] = arrayValues.slice();
          arrayValues.length = 0;
          inArray = false;
        }
      }
      
      const colonIndex = trimmedLine.indexOf(':');
      if (colonIndex > 0) {
        const key = trimmedLine.slice(0, colonIndex).trim();
        let value: unknown = trimmedLine.slice(colonIndex + 1).trim();
        
        // Check for array start
        if (value === '' && lines[lines.indexOf(line) + 1]?.trim().startsWith('- ')) {
          currentKey = key;
          inArray = true;
          continue;
        }
        
        if (value === '') continue;
        
        // Handle inline arrays
        if (typeof value === 'string' && value.includes(',') && !value.startsWith('"')) {
          value = value.split(',').map(v => v.trim());
        }
        // Handle quoted strings
        else if (typeof value === 'string' && value.startsWith('"') && value.endsWith('"')) {
          value = value.slice(1, -1);
        } else if (typeof value === 'string' && value.startsWith("'") && value.endsWith("'")) {
          value = value.slice(1, -1);
        }
        // Handle booleans
        else if (value === 'true') {
          value = true;
        } else if (value === 'false') {
          value = false;
        }
        // Handle numbers
        else if (typeof value === 'string' && !isNaN(Number(value))) {
          value = Number(value);
        }
        
        frontmatter[key] = value;
      }
    }
    
    // Handle any remaining array
    if (inArray && arrayValues.length > 0) {
      frontmatter[currentKey] = arrayValues;
    }
    
    return { frontmatter, body };
  } catch {
    return { frontmatter: null, body: content };
  }
}

/**
 * Generate YAML frontmatter string from object
 */
export function generateFrontmatter(data: Record<string, unknown>): string {
  const lines: string[] = ['---'];
  
  for (const [key, value] of Object.entries(data)) {
    if (value === undefined || value === null) continue;
    
    if (typeof value === 'string') {
      // Quote strings that contain special characters
      if (value.includes(':') || value.includes('#') || value.includes('\n')) {
        lines.push(`${key}: "${value.replace(/"/g, '\\"')}"`);
      } else {
        lines.push(`${key}: ${value}`);
      }
    } else if (Array.isArray(value)) {
      if (value.length === 0) continue;
      // Use inline format for short arrays
      if (value.every(v => typeof v === 'string' && v.length < 20) && value.length <= 5) {
        lines.push(`${key}: ${value.join(', ')}`);
      } else {
        lines.push(`${key}:`);
        for (const item of value) {
          lines.push(`  - ${item}`);
        }
      }
    } else if (typeof value === 'boolean' || typeof value === 'number') {
      lines.push(`${key}: ${value}`);
    } else if (typeof value === 'object') {
      lines.push(`${key}: ${JSON.stringify(value)}`);
    }
  }
  
  lines.push('---');
  return lines.join('\n');
}

/**
 * Create full markdown content with frontmatter
 */
export function createMarkdownContent(frontmatter: Record<string, unknown>, body: string): string {
  return `${generateFrontmatter(frontmatter)}\n\n${body}`;
}

// ============================================================================
// Path Utilities
// ============================================================================

/**
 * Get default paths for Claude Code entities
 */
export async function getDefaultPaths() {
  const home = await getHomeDirectory();
  
  return {
    global: {
      root: `${home}/.claude`,
      settings: `${home}/.claude/settings.json`,
      memory: `${home}/.claude/CLAUDE.md`,
      agents: `${home}/.claude/agents`,
      skills: `${home}/.claude/skills`,
      commands: `${home}/.claude/commands`,
      plugins: `${home}/.claude/plugins`,
    },
    userConfig: `${home}/.claude.json`,
    project: (projectPath: string) => ({
      root: `${projectPath}/.claude`,
      settings: `${projectPath}/.claude/settings.json`,
      settingsLocal: `${projectPath}/.claude/settings.local.json`,
      memoryRoot: `${projectPath}/CLAUDE.md`,
      memoryDotClaude: `${projectPath}/.claude/CLAUDE.md`,
      agents: `${projectPath}/.claude/agents`,
      skills: `${projectPath}/.claude/skills`,
      commands: `${projectPath}/.claude/commands`,
      plugins: `${projectPath}/.claude/plugins`,
      mcpJson: `${projectPath}/.mcp.json`,
    }),
  };
}

// ============================================================================
// Entity Templates
// ============================================================================

export const ENTITY_TEMPLATES = {
  agent: {
    default: `---
name: {{name}}
description: A custom agent
tools: Read, Grep, Glob
model: sonnet
---

You are a specialized agent.

When invoked:
1. Analyze the task at hand
2. Use appropriate tools to gather information
3. Execute the required actions
4. Report your findings

Focus on:
- Being thorough and accurate
- Explaining your reasoning
- Asking clarifying questions when needed
`,
    codeReviewer: `---
name: code-reviewer
description: Reviews code for quality, security, and best practices
tools: Read, Grep, Glob
model: sonnet
---

You are a code reviewer. When invoked, analyze code and provide specific, actionable feedback.

Review checklist:
- Code organization and structure
- Error handling
- Security concerns
- Test coverage
- Performance considerations

Provide feedback organized by priority:
- Critical issues (must fix)
- Warnings (should fix)
- Suggestions (consider improving)
`,
    debugger: `---
name: debugger
description: Debugging specialist for errors and unexpected behavior
tools: Read, Edit, Bash, Grep, Glob
model: sonnet
---

You are an expert debugger specializing in root cause analysis.

When invoked:
1. Capture error message and stack trace
2. Identify reproduction steps
3. Isolate the failure location
4. Implement minimal fix
5. Verify solution works

Focus on fixing the underlying issue, not the symptoms.
`,
  },
  skill: {
    default: `---
name: {{name}}
description: A custom skill
---

# {{name}} Skill

## When to use this skill

Use this skill when...

## Instructions

Follow these steps to accomplish the task:

1. First step
2. Second step
3. Third step

## Best practices

- Be thorough
- Document your work
- Ask for clarification when needed
`,
  },
  command: {
    default: `---
description: A custom command
---

# {{name}}

Execute the following task:

$ARGUMENTS
`,
    withTools: `---
description: A command with tool access
allowed-tools: Read, Edit, Bash
---

# {{name}}

$ARGUMENTS

Use the available tools to complete this task.
`,
  },
  memory: {
    default: `# Project Memory

## Overview

This file contains project-specific context and instructions for Claude.

## Project Structure

- Describe the main directories and their purposes
- Note any important files

## Guidelines

- Coding standards to follow
- Testing requirements
- Documentation expectations

## Common Tasks

- How to run the project
- How to run tests
- How to deploy
`,
  },
};
