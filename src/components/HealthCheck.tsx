// ============================================================================
// HealthCheck - Configuration analysis and issue detection
// ============================================================================

import { useMemo } from 'react';
import { useAppStore } from '../store/appStore';
import type { HealthIssue, HealthIssueSeverity } from '../lib/types';
import { 
  AlertTriangle, 
  AlertCircle, 
  Info, 
  CheckCircle2, 
  Link2Off, 
  Copy, 
  FileWarning,
  Sparkles,
  Bot,
  Terminal,
  FileText,
  ChevronRight
} from 'lucide-react';
import { clsx } from 'clsx';

export function HealthCheck() {
  const duplicates = useAppStore(state => state.duplicates);
  const symlinks = useAppStore(state => state.symlinks);
  const agents = useAppStore(state => state.agents);
  const skills = useAppStore(state => state.skills);
  const commands = useAppStore(state => state.commands);
  const settings = useAppStore(state => state.settings);
  const selectEntity = useAppStore(state => state.selectEntity);
  
  // Analyze and generate health issues
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
    
    // Check agents for missing descriptions
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
    
    // Check for empty CLAUDE.md files
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
  
  // Group issues by severity
  const groupedIssues = useMemo(() => {
    const groups: Record<HealthIssueSeverity, HealthIssue[]> = {
      error: [],
      warning: [],
      info: [],
    };
    
    issues.forEach(issue => {
      groups[issue.severity].push(issue);
    });
    
    return groups;
  }, [issues]);
  
  const totalIssues = issues.length;
  const errorCount = groupedIssues.error.length;
  const warningCount = groupedIssues.warning.length;
  const infoCount = groupedIssues.info.length;
  
  // Get icon for issue category
  const getCategoryIcon = (category: string) => {
    switch (category) {
      case 'Broken Symlinks':
        return <Link2Off className="w-4 h-4" />;
      case 'Duplicates':
        return <Copy className="w-4 h-4" />;
      case 'Security':
        return <AlertTriangle className="w-4 h-4" />;
      case 'Best Practices':
        return <Sparkles className="w-4 h-4" />;
      case 'Empty Files':
        return <FileWarning className="w-4 h-4" />;
      default:
        return <Info className="w-4 h-4" />;
    }
  };
  
  // Get icon for entity type
  const getEntityIcon = (type?: string) => {
    switch (type) {
      case 'agent':
        return <Bot className="w-3.5 h-3.5" />;
      case 'skill':
        return <Sparkles className="w-3.5 h-3.5" />;
      case 'command':
        return <Terminal className="w-3.5 h-3.5" />;
      default:
        return <FileText className="w-3.5 h-3.5" />;
    }
  };
  
  // Navigate to entity
  const handleIssueClick = (issue: HealthIssue) => {
    if (issue.entityId && issue.entityType) {
      // Find and select the entity
      let entity = null;
      switch (issue.entityType) {
        case 'agent':
          entity = agents.find(a => a.id === issue.entityId);
          break;
        case 'skill':
          entity = skills.find(s => s.id === issue.entityId);
          break;
        case 'command':
          entity = commands.find(c => c.id === issue.entityId);
          break;
        case 'settings':
          entity = settings.find(s => s.id === issue.entityId);
          break;
      }
      
      if (entity) {
        selectEntity(entity);
      }
    }
  };
  
  const formatPath = (path?: string) => {
    if (!path) return '';
    return path.replace(/^\/Users\/[^/]+/, '~');
  };
  
  return (
    <div className="health-check">
      <div className="health-check-header">
        <h1 className="health-check-title">Configuration Health</h1>
        <p className="health-check-subtitle">
          Analyzing your Claude Code configuration for potential issues
        </p>
      </div>
      
      {/* Summary Cards */}
      <div className="health-summary">
        <div className={clsx('health-summary-card', totalIssues === 0 && 'success')}>
          {totalIssues === 0 ? (
            <>
              <CheckCircle2 className="w-8 h-8 text-[var(--color-success)]" />
              <div>
                <span className="health-summary-count">All Clear</span>
                <span className="health-summary-label">No issues found</span>
              </div>
            </>
          ) : (
            <>
              <AlertCircle className="w-8 h-8 text-[var(--color-text-tertiary)]" />
              <div>
                <span className="health-summary-count">{totalIssues}</span>
                <span className="health-summary-label">Total Issues</span>
              </div>
            </>
          )}
        </div>
        
        {errorCount > 0 && (
          <div className="health-summary-card error">
            <AlertCircle className="w-6 h-6 text-[var(--color-error)]" />
            <div>
              <span className="health-summary-count">{errorCount}</span>
              <span className="health-summary-label">Errors</span>
            </div>
          </div>
        )}
        
        {warningCount > 0 && (
          <div className="health-summary-card warning">
            <AlertTriangle className="w-6 h-6 text-[var(--color-warning)]" />
            <div>
              <span className="health-summary-count">{warningCount}</span>
              <span className="health-summary-label">Warnings</span>
            </div>
          </div>
        )}
        
        {infoCount > 0 && (
          <div className="health-summary-card info">
            <Info className="w-6 h-6 text-[var(--color-info)]" />
            <div>
              <span className="health-summary-count">{infoCount}</span>
              <span className="health-summary-label">Suggestions</span>
            </div>
          </div>
        )}
      </div>
      
      {/* Issues List */}
      {totalIssues > 0 && (
        <div className="health-issues">
          {/* Errors */}
          {groupedIssues.error.length > 0 && (
            <div className="health-issues-section">
              <h2 className="health-issues-section-title error">
                <AlertCircle className="w-4 h-4" />
                Errors ({groupedIssues.error.length})
              </h2>
              <div className="health-issues-list">
                {groupedIssues.error.map(issue => (
                  <IssueCard 
                    key={issue.id} 
                    issue={issue} 
                    onClick={() => handleIssueClick(issue)}
                    getCategoryIcon={getCategoryIcon}
                    getEntityIcon={getEntityIcon}
                    formatPath={formatPath}
                  />
                ))}
              </div>
            </div>
          )}
          
          {/* Warnings */}
          {groupedIssues.warning.length > 0 && (
            <div className="health-issues-section">
              <h2 className="health-issues-section-title warning">
                <AlertTriangle className="w-4 h-4" />
                Warnings ({groupedIssues.warning.length})
              </h2>
              <div className="health-issues-list">
                {groupedIssues.warning.map(issue => (
                  <IssueCard 
                    key={issue.id} 
                    issue={issue} 
                    onClick={() => handleIssueClick(issue)}
                    getCategoryIcon={getCategoryIcon}
                    getEntityIcon={getEntityIcon}
                    formatPath={formatPath}
                  />
                ))}
              </div>
            </div>
          )}
          
          {/* Info/Suggestions */}
          {groupedIssues.info.length > 0 && (
            <div className="health-issues-section">
              <h2 className="health-issues-section-title info">
                <Info className="w-4 h-4" />
                Suggestions ({groupedIssues.info.length})
              </h2>
              <div className="health-issues-list">
                {groupedIssues.info.map(issue => (
                  <IssueCard 
                    key={issue.id} 
                    issue={issue} 
                    onClick={() => handleIssueClick(issue)}
                    getCategoryIcon={getCategoryIcon}
                    getEntityIcon={getEntityIcon}
                    formatPath={formatPath}
                  />
                ))}
              </div>
            </div>
          )}
        </div>
      )}
      
      {/* Empty State */}
      {totalIssues === 0 && (
        <div className="health-empty">
          <CheckCircle2 className="w-16 h-16 text-[var(--color-success)] opacity-30" />
          <h3>Your configuration looks healthy!</h3>
          <p>No issues, warnings, or suggestions were found in your Claude Code configuration.</p>
        </div>
      )}
    </div>
  );
}

// Issue Card Component
function IssueCard({ 
  issue, 
  onClick,
  getCategoryIcon,
  getEntityIcon,
  formatPath,
}: { 
  issue: HealthIssue;
  onClick: () => void;
  getCategoryIcon: (category: string) => React.ReactNode;
  getEntityIcon: (type?: string) => React.ReactNode;
  formatPath: (path?: string) => string;
}) {
  const severityColors = {
    error: 'var(--color-error)',
    warning: 'var(--color-warning)',
    info: 'var(--color-info)',
  };
  
  return (
    <button
      className={clsx('health-issue-card', issue.severity)}
      onClick={onClick}
      disabled={!issue.entityId}
    >
      <div className="health-issue-icon" style={{ color: severityColors[issue.severity] }}>
        {getCategoryIcon(issue.category)}
      </div>
      <div className="health-issue-content">
        <div className="health-issue-header">
          <span className="health-issue-title">{issue.title}</span>
          <span className="health-issue-category">{issue.category}</span>
        </div>
        <p className="health-issue-description">{issue.description}</p>
        {issue.path && (
          <div className="health-issue-path">
            {issue.entityType && getEntityIcon(issue.entityType)}
            <code>{formatPath(issue.path)}</code>
          </div>
        )}
        {issue.suggestion && (
          <p className="health-issue-suggestion">{issue.suggestion}</p>
        )}
      </div>
      {issue.entityId && (
        <ChevronRight className="w-4 h-4 text-[var(--color-text-quaternary)]" />
      )}
    </button>
  );
}
