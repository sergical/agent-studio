// ============================================================================
// CreateEntityDialog - Modal for creating new entities with templates
// ============================================================================

import { useState, useEffect, useRef, useCallback } from 'react';
import { motion, AnimatePresence } from 'motion/react';
import { X, Plus, User, Star, Terminal, FileText, ChevronDown, Wand2 } from 'lucide-react';
import { clsx } from 'clsx';
import { createEntity } from '../../lib/api';
import { useAppStore } from '../../store/appStore';
import type { EntityType } from '../../lib/types';

type CreatableEntityType = 'agent' | 'skill' | 'command' | 'memory';

interface Template {
  id: string;
  name: string;
  description: string;
  content: string;
}

// Templates for each entity type
const TEMPLATES: Record<CreatableEntityType, Template[]> = {
  agent: [
    {
      id: 'basic',
      name: 'Basic Agent',
      description: 'Simple agent with description and default settings',
      content: `---
name: {{name}}
description: A custom agent for specialized tasks
model: sonnet
---

# {{name}}

Instructions for this agent go here. Describe what the agent should do and how it should behave.

## Key Responsibilities
- List the main tasks this agent handles
- Define the scope of work

## Guidelines
- Add specific guidelines for behavior
- Include any constraints or rules
`,
    },
    {
      id: 'code-reviewer',
      name: 'Code Reviewer',
      description: 'Agent specialized for code review tasks',
      content: `---
name: {{name}}
description: Reviews code for quality, security, and best practices
model: sonnet
tools:
  - Read
  - Glob
  - Grep
permissionMode: default
---

# {{name}}

You are a code review specialist. Review code for:
- Code quality and readability
- Security vulnerabilities
- Performance issues
- Best practice violations

## Review Process
1. Understand the context and purpose of the code
2. Check for logical errors and edge cases
3. Identify security concerns
4. Suggest improvements with explanations
5. Provide actionable feedback

## Guidelines
- Be constructive and specific
- Explain the "why" behind suggestions
- Prioritize issues by severity
`,
    },
    {
      id: 'security',
      name: 'Security Focused',
      description: 'Agent with restricted permissions for sensitive work',
      content: `---
name: {{name}}
description: Security-conscious agent with restricted permissions
model: sonnet
permissionMode: default
disallowedTools:
  - Bash
  - Write
---

# {{name}}

This agent operates with restricted permissions for security-sensitive tasks.

## Allowed Operations
- Reading and analyzing code
- Providing recommendations
- Documentation review

## Restrictions
- Cannot execute shell commands
- Cannot modify files directly
- Must request human approval for changes
`,
    },
  ],
  skill: [
    {
      id: 'basic',
      name: 'Basic Skill',
      description: 'Simple skill template with metadata',
      content: `---
name: {{name}}
description: A reusable skill for Claude
user-invocable: true
---

# {{name}}

Instructions for this skill. Describe what knowledge or capability this skill provides.

## When to Use
- List scenarios where this skill applies
- Define trigger conditions

## How It Works
Explain the approach or methodology this skill uses.
`,
    },
    {
      id: 'tool-focused',
      name: 'Tool-Focused Skill',
      description: 'Skill that uses specific tools',
      content: `---
name: {{name}}
description: Skill with specific tool permissions
allowed-tools:
  - Read
  - Glob
  - Grep
user-invocable: true
---

# {{name}}

This skill has access to specific tools for its task.

## Available Tools
- **Read**: Read file contents
- **Glob**: Find files by pattern
- **Grep**: Search file contents

## Usage Instructions
Describe how to use this skill effectively.
`,
    },
    {
      id: 'reference',
      name: 'Reference Guide',
      description: 'Skill with supporting reference files',
      content: `---
name: {{name}}
description: Knowledge skill with reference documentation
user-invocable: true
disable-model-invocation: true
---

# {{name}}

This skill provides reference information.

## Overview
High-level description of the knowledge domain.

## Key Concepts
- Concept 1: Description
- Concept 2: Description

## Best Practices
1. First best practice
2. Second best practice

## Common Patterns
Describe common patterns and their applications.
`,
    },
  ],
  command: [
    {
      id: 'basic',
      name: 'Basic Command',
      description: 'Simple slash command',
      content: `---
description: A quick action command
---

Execute this task:
$ARGUMENTS
`,
    },
    {
      id: 'with-args',
      name: 'Command with Arguments',
      description: 'Command that accepts arguments',
      content: `---
description: Command that processes input arguments
argument-hint: "<input>"
---

Process the following input: $ARGUMENTS

## Instructions
1. Parse the provided arguments
2. Execute the main logic
3. Report results
`,
    },
    {
      id: 'agent-delegate',
      name: 'Agent Delegation',
      description: 'Command that delegates to an agent',
      content: `---
description: Delegates task to a specialized agent
agent: explorer
argument-hint: "<query>"
---

Execute this task using the explorer agent:
$ARGUMENTS
`,
    },
    {
      id: 'code-action',
      name: 'Code Action',
      description: 'Command for code operations',
      content: `---
description: Performs code-related operations
allowed-tools:
  - Read
  - Edit
  - Glob
  - Grep
argument-hint: "<file-or-pattern>"
---

Perform the following code operation:
$ARGUMENTS

## Guidelines
- Analyze before modifying
- Preserve existing style
- Add appropriate comments
`,
    },
  ],
  memory: [
    {
      id: 'basic',
      name: 'Basic Memory',
      description: 'Simple CLAUDE.md with key sections',
      content: `# Project Overview

Brief description of this project.

## Tech Stack
- Language/Framework
- Key dependencies

## Project Structure
Describe the main directories and their purposes.

## Development Guidelines
- Coding conventions
- Testing requirements
- Documentation standards

## Important Notes
Any critical information for Claude to remember.
`,
    },
    {
      id: 'detailed',
      name: 'Detailed Project Guide',
      description: 'Comprehensive project documentation',
      content: `# Project: Project Name

## Overview
Detailed description of what this project does and its main goals.

## Architecture
- Frontend: 
- Backend: 
- Database: 

## Key Directories
- \`src/\` - Source code
- \`tests/\` - Test files
- \`docs/\` - Documentation

## Coding Standards
- Follow existing patterns
- Use TypeScript strict mode
- Write tests for new features

## Common Tasks
- How to add a new feature
- How to run tests
- How to deploy

## Gotchas
- Known issues or quirks
- Things to watch out for

## Contacts
- Lead developer: 
- Documentation: 
`,
    },
  ],
};

interface CreateEntityDialogProps {
  isOpen: boolean;
  onClose: () => void;
  initialEntityType?: CreatableEntityType;
  initialScope?: 'global' | 'project';
  projectPath?: string;
}

const ENTITY_OPTIONS: { type: CreatableEntityType; label: string; icon: React.ReactNode; description: string }[] = [
  {
    type: 'agent',
    label: 'Agent',
    icon: <User className="w-4 h-4" />,
    description: 'Custom AI agent with specific tools and behavior',
  },
  {
    type: 'skill',
    label: 'Skill',
    icon: <Star className="w-4 h-4" />,
    description: 'Reusable knowledge or capability for Claude',
  },
  {
    type: 'command',
    label: 'Command',
    icon: <Terminal className="w-4 h-4" />,
    description: 'Slash command for quick task execution',
  },
  {
    type: 'memory',
    label: 'Memory',
    icon: <FileText className="w-4 h-4" />,
    description: 'CLAUDE.md file with project context',
  },
];

export function CreateEntityDialog({
  isOpen,
  onClose,
  initialEntityType,
  initialScope = 'global',
  projectPath,
}: CreateEntityDialogProps) {
  const [entityType, setEntityType] = useState<CreatableEntityType>(initialEntityType || 'agent');
  const [name, setName] = useState('');
  const [scope, setScope] = useState<'global' | 'project'>(initialScope);
  const [selectedProject, setSelectedProject] = useState<string | null>(projectPath || null);
  const [selectedTemplate, setSelectedTemplate] = useState<string>('basic');
  const [isCreating, setIsCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showTypeDropdown, setShowTypeDropdown] = useState(false);
  const [showTemplateDropdown, setShowTemplateDropdown] = useState(false);
  
  const nameInputRef = useRef<HTMLInputElement>(null);
  const projects = useAppStore(state => state.projects);
  const refreshDiscovery = useAppStore(state => state.refreshDiscovery);
  const addToast = useAppStore(state => state.addToast);
  const setActiveView = useAppStore(state => state.setActiveView);
  
  // Get available templates for current entity type
  const availableTemplates = TEMPLATES[entityType] || [];
  const currentTemplate = availableTemplates.find(t => t.id === selectedTemplate) || availableTemplates[0];
  
  // Reset form when dialog opens
  useEffect(() => {
    if (isOpen) {
      setName('');
      setError(null);
      setEntityType(initialEntityType || 'agent');
      setScope(projectPath ? 'project' : initialScope);
      setSelectedProject(projectPath || null);
      setSelectedTemplate('basic');
      // Focus name input after a short delay for animation
      setTimeout(() => nameInputRef.current?.focus(), 100);
    }
  }, [isOpen, initialEntityType, initialScope, projectPath]);
  
  // Reset template when entity type changes
  useEffect(() => {
    setSelectedTemplate('basic');
  }, [entityType]);
  
  // Keyboard handling
  const handleKeyDown = useCallback((e: KeyboardEvent) => {
    if (!isOpen) return;
    
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    }
  }, [isOpen, onClose]);
  
  useEffect(() => {
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);
  
  // Prevent body scroll when open
  useEffect(() => {
    if (isOpen) {
      document.body.style.overflow = 'hidden';
      return () => { document.body.style.overflow = ''; };
    }
  }, [isOpen]);
  
  const handleCreate = async () => {
    const trimmedName = name.trim();
    
    // Memory doesn't need a name
    if (entityType !== 'memory' && !trimmedName) {
      setError('Name is required');
      return;
    }
    
    // Validate name format (alphanumeric, hyphens, underscores)
    if (entityType !== 'memory' && !/^[a-zA-Z0-9_-]+$/.test(trimmedName)) {
      setError('Name can only contain letters, numbers, hyphens, and underscores');
      return;
    }
    
    if (scope === 'project' && !selectedProject) {
      setError('Please select a project');
      return;
    }
    
    setIsCreating(true);
    setError(null);
    
    try {
      // Generate content from template
      const templateContent = currentTemplate?.content || '';
      const content = templateContent.replace(/\{\{name\}\}/g, trimmedName || 'unnamed');
      
      await createEntity(
        entityType as EntityType,
        trimmedName || 'CLAUDE',
        scope,
        scope === 'project' ? selectedProject || undefined : undefined,
        content
      );
      
      addToast({
        type: 'success',
        title: 'Created',
        message: `${entityType.charAt(0).toUpperCase() + entityType.slice(1)} "${trimmedName}" created successfully`,
      });
      
      // Refresh discovery to pick up the new entity
      await refreshDiscovery();
      
      // Navigate to the appropriate view
      const viewMap: Record<CreatableEntityType, string> = {
        agent: 'agents',
        skill: 'skills',
        command: 'commands',
        memory: 'memory',
      };
      setActiveView(viewMap[entityType] as any);
      
      onClose();
    } catch (err) {
      console.error('Failed to create entity:', err);
      setError(err instanceof Error ? err.message : 'Failed to create entity');
    } finally {
      setIsCreating(false);
    }
  };
  
  const selectedOption = ENTITY_OPTIONS.find(opt => opt.type === entityType)!;
  
  return (
    <AnimatePresence>
      {isOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center">
          {/* Backdrop */}
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.15 }}
            className="absolute inset-0 bg-black/60 backdrop-blur-sm"
            onClick={onClose}
          />
          
          {/* Dialog */}
          <motion.div
            initial={{ opacity: 0, scale: 0.95, y: 10 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.95, y: 10 }}
            transition={{ duration: 0.15, ease: [0.25, 0.46, 0.45, 0.94] }}
            className="relative w-full max-w-md mx-4 bg-[var(--color-bg-primary)] rounded-xl border border-[var(--color-border)] shadow-2xl"
            role="dialog"
            aria-modal="true"
            aria-labelledby="create-dialog-title"
          >
            {/* Header */}
            <div className="flex items-center justify-between px-6 py-4 border-b border-[var(--color-border)]">
              <h2 id="create-dialog-title" className="text-lg font-semibold text-[var(--color-text-primary)]">
                Create New Entity
              </h2>
              <button
                onClick={onClose}
                className="p-1.5 text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] rounded-md transition-colors"
                aria-label="Close"
              >
                <X className="w-4 h-4" />
              </button>
            </div>
            
            {/* Content */}
            <div className="p-6 space-y-5">
              {/* Entity Type Selector */}
              <div>
                <label className="block text-xs font-medium text-[var(--color-text-secondary)] mb-2">
                  Type
                </label>
                <div className="relative">
                  <button
                    type="button"
                    onClick={() => setShowTypeDropdown(!showTypeDropdown)}
                    className="w-full flex items-center justify-between gap-3 px-3 py-2.5 bg-[var(--color-bg-secondary)] border border-[var(--color-border)] rounded-lg text-left hover:border-[var(--color-border-focus)] transition-colors"
                  >
                    <div className="flex items-center gap-3">
                      <div className="flex items-center justify-center w-8 h-8 rounded-lg bg-[var(--color-accent-soft)] text-[var(--color-accent)]">
                        {selectedOption.icon}
                      </div>
                      <div>
                        <div className="text-sm font-medium text-[var(--color-text-primary)]">
                          {selectedOption.label}
                        </div>
                        <div className="text-xs text-[var(--color-text-tertiary)]">
                          {selectedOption.description}
                        </div>
                      </div>
                    </div>
                    <ChevronDown className={clsx(
                      'w-4 h-4 text-[var(--color-text-tertiary)] transition-transform',
                      showTypeDropdown && 'rotate-180'
                    )} />
                  </button>
                  
                  {showTypeDropdown && (
                    <motion.div
                      initial={{ opacity: 0, y: -4 }}
                      animate={{ opacity: 1, y: 0 }}
                      className="absolute top-full left-0 right-0 mt-1 bg-[var(--color-bg-elevated)] border border-[var(--color-border)] rounded-lg shadow-lg z-10 overflow-hidden"
                    >
                      {ENTITY_OPTIONS.map(option => (
                        <button
                          key={option.type}
                          type="button"
                          onClick={() => {
                            setEntityType(option.type);
                            setShowTypeDropdown(false);
                          }}
                          className={clsx(
                            'w-full flex items-center gap-3 px-3 py-2.5 text-left hover:bg-[var(--color-bg-hover)] transition-colors',
                            option.type === entityType && 'bg-[var(--color-accent-softer)]'
                          )}
                        >
                          <div className={clsx(
                            'flex items-center justify-center w-8 h-8 rounded-lg',
                            option.type === entityType
                              ? 'bg-[var(--color-accent-soft)] text-[var(--color-accent)]'
                              : 'bg-[var(--color-bg-tertiary)] text-[var(--color-text-tertiary)]'
                          )}>
                            {option.icon}
                          </div>
                          <div>
                            <div className="text-sm font-medium text-[var(--color-text-primary)]">
                              {option.label}
                            </div>
                            <div className="text-xs text-[var(--color-text-tertiary)]">
                              {option.description}
                            </div>
                          </div>
                        </button>
                      ))}
                    </motion.div>
                  )}
                </div>
              </div>
              
              {/* Template Selector */}
              {availableTemplates.length > 1 && (
                <div>
                  <label className="block text-xs font-medium text-[var(--color-text-secondary)] mb-2">
                    Template
                  </label>
                  <div className="relative">
                    <button
                      type="button"
                      onClick={() => setShowTemplateDropdown(!showTemplateDropdown)}
                      className="w-full flex items-center justify-between gap-3 px-3 py-2.5 bg-[var(--color-bg-secondary)] border border-[var(--color-border)] rounded-lg text-left hover:border-[var(--color-border-focus)] transition-colors"
                    >
                      <div className="flex items-center gap-3">
                        <div className="flex items-center justify-center w-7 h-7 rounded-md bg-[var(--color-success-soft)] text-[var(--color-success)]">
                          <Wand2 className="w-3.5 h-3.5" />
                        </div>
                        <div>
                          <div className="text-sm font-medium text-[var(--color-text-primary)]">
                            {currentTemplate?.name || 'Basic'}
                          </div>
                          <div className="text-xs text-[var(--color-text-tertiary)]">
                            {currentTemplate?.description || 'Simple template'}
                          </div>
                        </div>
                      </div>
                      <ChevronDown className={clsx(
                        'w-4 h-4 text-[var(--color-text-tertiary)] transition-transform',
                        showTemplateDropdown && 'rotate-180'
                      )} />
                    </button>
                    
                    {showTemplateDropdown && (
                      <motion.div
                        initial={{ opacity: 0, y: -4 }}
                        animate={{ opacity: 1, y: 0 }}
                        className="absolute top-full left-0 right-0 mt-1 bg-[var(--color-bg-elevated)] border border-[var(--color-border)] rounded-lg shadow-lg z-10 overflow-hidden max-h-64 overflow-y-auto"
                      >
                        {availableTemplates.map(template => (
                          <button
                            key={template.id}
                            type="button"
                            onClick={() => {
                              setSelectedTemplate(template.id);
                              setShowTemplateDropdown(false);
                            }}
                            className={clsx(
                              'w-full flex items-center gap-3 px-3 py-2.5 text-left hover:bg-[var(--color-bg-hover)] transition-colors',
                              template.id === selectedTemplate && 'bg-[var(--color-success-softer)]'
                            )}
                          >
                            <div className={clsx(
                              'flex items-center justify-center w-7 h-7 rounded-md',
                              template.id === selectedTemplate
                                ? 'bg-[var(--color-success-soft)] text-[var(--color-success)]'
                                : 'bg-[var(--color-bg-tertiary)] text-[var(--color-text-tertiary)]'
                            )}>
                              <Wand2 className="w-3.5 h-3.5" />
                            </div>
                            <div>
                              <div className="text-sm font-medium text-[var(--color-text-primary)]">
                                {template.name}
                              </div>
                              <div className="text-xs text-[var(--color-text-tertiary)]">
                                {template.description}
                              </div>
                            </div>
                          </button>
                        ))}
                      </motion.div>
                    )}
                  </div>
                </div>
              )}
              
              {/* Name Input */}
              {entityType !== 'memory' && (
                <div>
                  <label htmlFor="entity-name" className="block text-xs font-medium text-[var(--color-text-secondary)] mb-2">
                    Name
                  </label>
                  <input
                    ref={nameInputRef}
                    id="entity-name"
                    type="text"
                    value={name}
                    onChange={e => setName(e.target.value)}
                    onKeyDown={e => {
                      if (e.key === 'Enter' && !isCreating) {
                        handleCreate();
                      }
                    }}
                    placeholder={`my-${entityType}`}
                    className="w-full px-3 py-2.5 bg-[var(--color-bg-secondary)] border border-[var(--color-border)] rounded-lg text-sm text-[var(--color-text-primary)] placeholder:text-[var(--color-text-quaternary)] focus:outline-none focus:border-[var(--color-accent)] focus:ring-2 focus:ring-[var(--color-accent-softer)] transition-all"
                  />
                </div>
              )}
              
              {/* Scope Selector */}
              <div>
                <label className="block text-xs font-medium text-[var(--color-text-secondary)] mb-2">
                  Scope
                </label>
                <div className="flex gap-2">
                  <button
                    type="button"
                    onClick={() => setScope('global')}
                    className={clsx(
                      'flex-1 px-3 py-2 text-sm font-medium rounded-lg border transition-colors',
                      scope === 'global'
                        ? 'bg-[var(--color-info-soft)] border-[var(--color-info)] text-[var(--color-info)]'
                        : 'bg-[var(--color-bg-secondary)] border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-border-focus)]'
                    )}
                  >
                    Global
                  </button>
                  <button
                    type="button"
                    onClick={() => setScope('project')}
                    className={clsx(
                      'flex-1 px-3 py-2 text-sm font-medium rounded-lg border transition-colors',
                      scope === 'project'
                        ? 'bg-[var(--color-success-soft)] border-[var(--color-success)] text-[var(--color-success)]'
                        : 'bg-[var(--color-bg-secondary)] border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-border-focus)]'
                    )}
                  >
                    Project
                  </button>
                </div>
              </div>
              
              {/* Project Selector (when scope is project) */}
              {scope === 'project' && (
                <div>
                  <label htmlFor="project-select" className="block text-xs font-medium text-[var(--color-text-secondary)] mb-2">
                    Project
                  </label>
                  <select
                    id="project-select"
                    value={selectedProject || ''}
                    onChange={e => setSelectedProject(e.target.value || null)}
                    className="w-full px-3 py-2.5 bg-[var(--color-bg-secondary)] border border-[var(--color-border)] rounded-lg text-sm text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] focus:ring-2 focus:ring-[var(--color-accent-softer)] transition-all"
                  >
                    <option value="">Select a project...</option>
                    {projects.map(project => (
                      <option key={project.path} value={project.path}>
                        {project.name}
                      </option>
                    ))}
                  </select>
                </div>
              )}
              
              {/* Error Message */}
              {error && (
                <div className="px-3 py-2 text-sm text-[var(--color-error)] bg-[var(--color-error-soft)] rounded-lg">
                  {error}
                </div>
              )}
            </div>
            
            {/* Footer */}
            <div className="flex justify-end gap-3 px-6 py-4 border-t border-[var(--color-border)]">
              <button
                type="button"
                onClick={onClose}
                disabled={isCreating}
                className="px-4 py-2 text-sm font-medium text-[var(--color-text-secondary)] bg-[var(--color-bg-secondary)] hover:bg-[var(--color-bg-hover)] rounded-lg transition-colors disabled:opacity-50"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={handleCreate}
                disabled={isCreating || (entityType !== 'memory' && !name.trim())}
                className="flex items-center gap-2 px-4 py-2 text-sm font-medium text-white bg-[var(--color-accent)] hover:bg-[var(--color-accent-hover)] rounded-lg transition-colors disabled:opacity-50"
              >
                {isCreating ? (
                  <>
                    <svg className="animate-spin w-4 h-4" viewBox="0 0 24 24" fill="none">
                      <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                      <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                    </svg>
                    Creating...
                  </>
                ) : (
                  <>
                    <Plus className="w-4 h-4" />
                    Create
                  </>
                )}
              </button>
            </div>
          </motion.div>
        </div>
      )}
    </AnimatePresence>
  );
}
