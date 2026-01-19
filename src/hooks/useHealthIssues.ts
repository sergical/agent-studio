// ============================================================================
// useHealthIssues - Shared hook for calculating health issues
// ============================================================================

import { useMemo } from 'react';
import { useAppStore } from '../store/appStore';
import type { HealthIssue, HealthIssueSeverity } from '../lib/types';

export interface HealthSummary {
  issues: HealthIssue[];
  errorCount: number;
  warningCount: number;
  infoCount: number;
  totalCount: number;
  isHealthy: boolean;
}

export function useHealthIssues(): HealthSummary {
  const duplicates = useAppStore(state => state.duplicates);
  const symlinks = useAppStore(state => state.symlinks);
  const agents = useAppStore(state => state.agents);
  const skills = useAppStore(state => state.skills);
  const commands = useAppStore(state => state.commands);
  const settings = useAppStore(state => state.settings);
  
  const issues = useMemo(() => {
    const result: HealthIssue[] = [];
    
    // Check for broken symlinks
    symlinks?.forEach(symlink => {
      if (!symlink.target_exists) {
        result.push({
          id: `broken-symlink-${symlink.path}`,
          severity: 'error',
          category: 'Broken Symlinks',
          title: 'Broken symlink detected',
          description: `Symlink target does not exist`,
          path: symlink.path,
          entityType: symlink.entity_type as any,
          entityId: symlink.entity_id || undefined,
          suggestion: `Remove the symlink or restore the target at: ${symlink.target}`,
        });
      }
    });
    
    // Check for duplicates
    duplicates?.forEach(dup => {
      result.push({
        id: `duplicate-${dup.entity_type}-${dup.name}`,
        severity: 'warning',
        category: 'Duplicates',
        title: `Duplicate ${dup.entity_type}: ${dup.name}`,
        description: `Found ${dup.entities.length} definitions with the same name`,
        entityType: dup.entity_type as any,
        suggestion: `Only one definition will be active. Consider removing duplicates or renaming.`,
      });
    });
    
    // Check agents for issues
    agents?.forEach(agent => {
      if (!agent.frontmatter?.description) {
        result.push({
          id: `agent-no-desc-${agent.id}`,
          severity: 'info',
          category: 'Best Practices',
          title: `Agent "${agent.name}" has no description`,
          description: 'Adding a description helps with agent discoverability',
          path: agent.path,
          entityType: 'agent',
          entityId: agent.id,
          suggestion: 'Add a description field to the YAML frontmatter',
        });
      }
      
      // Check for bypassPermissions mode
      if (agent.frontmatter?.permissionMode === 'bypassPermissions') {
        result.push({
          id: `agent-bypass-${agent.id}`,
          severity: 'warning',
          category: 'Security',
          title: `Agent "${agent.name}" bypasses permissions`,
          description: 'This agent runs with elevated permissions which could be risky',
          path: agent.path,
          entityType: 'agent',
          entityId: agent.id,
          suggestion: 'Consider using "acceptEdits" or "default" permission mode unless bypass is necessary',
        });
      }
    });
    
    // Check skills for missing descriptions
    skills?.forEach(skill => {
      if (!skill.frontmatter?.description) {
        result.push({
          id: `skill-no-desc-${skill.id}`,
          severity: 'info',
          category: 'Best Practices',
          title: `Skill "${skill.name}" has no description`,
          description: 'Adding a description helps Claude understand when to use this skill',
          path: skill.path,
          entityType: 'skill',
          entityId: skill.id,
          suggestion: 'Add a description field to the YAML frontmatter',
        });
      }
    });
    
    // Check commands for missing descriptions
    commands?.forEach(cmd => {
      if (!cmd.frontmatter?.description) {
        result.push({
          id: `cmd-no-desc-${cmd.id}`,
          severity: 'info',
          category: 'Best Practices',
          title: `Command "${cmd.name}" has no description`,
          description: 'Adding a description helps users understand what the command does',
          path: cmd.path,
          entityType: 'command',
          entityId: cmd.id,
          suggestion: 'Add a description field to the YAML frontmatter',
        });
      }
    });
    
    // Check for empty settings files
    settings?.forEach(setting => {
      if (!setting.content || setting.content.trim().length < 10) {
        result.push({
          id: `empty-settings-${setting.id}`,
          severity: 'info',
          category: 'Empty Files',
          title: `Settings file appears empty`,
          description: `${setting.path} has minimal or no content`,
          path: setting.path,
          entityType: 'settings',
          entityId: setting.id,
          suggestion: 'Add configuration or remove the file if not needed',
        });
      }
    });
    
    return result;
  }, [duplicates, symlinks, agents, skills, commands, settings]);
  
  const counts = useMemo(() => {
    const errorCount = issues.filter(i => i.severity === 'error').length;
    const warningCount = issues.filter(i => i.severity === 'warning').length;
    const infoCount = issues.filter(i => i.severity === 'info').length;
    
    return {
      errorCount,
      warningCount,
      infoCount,
      totalCount: issues.length,
      isHealthy: errorCount === 0 && warningCount === 0,
    };
  }, [issues]);
  
  return {
    issues,
    ...counts,
  };
}

/**
 * Get icon for issue severity
 */
export function getSeverityIcon(severity: HealthIssueSeverity) {
  switch (severity) {
    case 'error':
      return 'AlertCircle';
    case 'warning':
      return 'AlertTriangle';
    case 'info':
      return 'Info';
    default:
      return 'Info';
  }
}

/**
 * Get color for issue severity
 */
export function getSeverityColor(severity: HealthIssueSeverity) {
  switch (severity) {
    case 'error':
      return 'var(--color-error)';
    case 'warning':
      return 'var(--color-warning)';
    case 'info':
      return 'var(--color-info)';
    default:
      return 'var(--color-text-tertiary)';
  }
}
