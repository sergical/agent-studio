// ============================================================================
// Agent Studio - Skills API
// Tauri IPC communication for skills.sh integration
// ============================================================================

import { invoke } from '@tauri-apps/api/core';
import type {
  AgentTarget,
  InstallRequest,
  InstallResult,
  InstalledSkill,
  PaginatedSkillsResponse,
  SkillSearchResult,
} from './skillsTypes';

// ============================================================================
// Search API
// ============================================================================

/**
 * Search for skills on skills.sh
 */
export async function searchSkills(
  query: string,
  limit?: number,
  offset?: number
): Promise<PaginatedSkillsResponse> {
  return invoke('search_skills', { query, limit, offset });
}

/**
 * Get popular skills (sorted by install count)
 */
export async function getPopularSkills(
  limit?: number,
  offset?: number
): Promise<PaginatedSkillsResponse> {
  return invoke('get_popular_skills', { limit, offset });
}

/**
 * Get skill details from skills.sh
 */
export async function getSkillDetails(skillId: string): Promise<SkillSearchResult> {
  return invoke('get_skill_details', { skillId });
}

// ============================================================================
// Installed Skills API
// ============================================================================

/**
 * Get all installed skills from the lock file
 */
export async function getInstalledSkills(): Promise<InstalledSkill[]> {
  return invoke('get_installed_skills');
}

/**
 * Check if a skill is installed
 */
export async function isSkillInstalled(skillName: string): Promise<boolean> {
  return invoke('is_skill_installed', { skillName });
}

// ============================================================================
// Agent Targets API
// ============================================================================

/**
 * Get all supported agent targets
 */
export async function getAgentTargets(): Promise<AgentTarget[]> {
  return invoke('get_agent_targets');
}

// ============================================================================
// Installation API
// ============================================================================

/**
 * Install a skill using npx skills CLI
 */
export async function installSkill(request: InstallRequest): Promise<InstallResult> {
  return invoke('install_skill', { request });
}

/**
 * Remove a skill using npx skills CLI
 */
export async function removeSkill(skillName: string, global: boolean): Promise<InstallResult> {
  return invoke('remove_skill', { skillName, global });
}

/**
 * Update a skill using npx skills CLI
 */
export async function updateSkill(skillName: string, global: boolean): Promise<InstallResult> {
  return invoke('update_skill', { skillName, global });
}
