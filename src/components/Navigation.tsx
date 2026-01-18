// ============================================================================
// Navigation - Sidebar navigation for different views
// ============================================================================

import { useAppStore } from '../store/appStore';
import type { ViewType, EntityType } from '../lib/types';

interface NavItem {
  view: ViewType;
  label: string;
  entityType?: EntityType;
  icon: React.ReactNode;
}

const NAV_ITEMS: NavItem[] = [
  {
    view: 'dashboard',
    label: 'Dashboard',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <rect x="3" y="3" width="7" height="7" rx="1.5" />
        <rect x="14" y="3" width="7" height="7" rx="1.5" />
        <rect x="14" y="14" width="7" height="7" rx="1.5" />
        <rect x="3" y="14" width="7" height="7" rx="1.5" />
      </svg>
    ),
  },
  {
    view: 'settings',
    label: 'Settings',
    entityType: 'settings',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <circle cx="12" cy="12" r="3" />
        <path d="M12 1v4M12 19v4M4.22 4.22l2.83 2.83M16.95 16.95l2.83 2.83M1 12h4M19 12h4M4.22 19.78l2.83-2.83M16.95 7.05l2.83-2.83" />
      </svg>
    ),
  },
  {
    view: 'memory',
    label: 'Memory',
    entityType: 'memory',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z" />
        <polyline points="14,2 14,8 20,8" />
      </svg>
    ),
  },
  {
    view: 'agents',
    label: 'Agents',
    entityType: 'agent',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <circle cx="12" cy="8" r="4" />
        <path d="M20 21a8 8 0 10-16 0" />
      </svg>
    ),
  },
  {
    view: 'skills',
    label: 'Skills',
    entityType: 'skill',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <polygon points="12,2 15.09,8.26 22,9.27 17,14.14 18.18,21.02 12,17.77 5.82,21.02 7,14.14 2,9.27 8.91,8.26" />
      </svg>
    ),
  },
  {
    view: 'commands',
    label: 'Commands',
    entityType: 'command',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <polyline points="4,17 10,11 4,5" />
        <line x1="12" y1="19" x2="20" y2="19" />
      </svg>
    ),
  },
  {
    view: 'hooks',
    label: 'Hooks',
    entityType: 'hook',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <path d="M10 13a5 5 0 007.54.54l3-3a5 5 0 00-7.07-7.07l-1.72 1.71" />
        <path d="M14 11a5 5 0 00-7.54-.54l-3 3a5 5 0 007.07 7.07l1.71-1.71" />
      </svg>
    ),
  },
  {
    view: 'plugins',
    label: 'Plugins',
    entityType: 'plugin',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <rect x="4" y="4" width="16" height="16" rx="2" />
        <path d="M9 1v3M15 1v3M9 20v3M15 20v3" />
      </svg>
    ),
  },
  {
    view: 'mcp',
    label: 'MCP Servers',
    entityType: 'mcp',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <rect x="2" y="2" width="20" height="8" rx="2" />
        <rect x="2" y="14" width="20" height="8" rx="2" />
        <circle cx="6" cy="6" r="1" fill="currentColor" />
        <circle cx="6" cy="18" r="1" fill="currentColor" />
      </svg>
    ),
  },
];

export function Navigation() {
  const activeView = useAppStore(state => state.activeView);
  const setActiveView = useAppStore(state => state.setActiveView);
  
  // Select raw state arrays directly
  const settings = useAppStore(state => state.settings);
  const memory = useAppStore(state => state.memory);
  const agents = useAppStore(state => state.agents);
  const skills = useAppStore(state => state.skills);
  const commands = useAppStore(state => state.commands);
  const hooks = useAppStore(state => state.hooks);
  const plugins = useAppStore(state => state.plugins);
  const mcpServers = useAppStore(state => state.mcpServers);
  
  const counts: Record<string, number> = {
    settings: settings.length,
    memory: memory.length,
    agent: agents.length,
    skill: skills.length,
    command: commands.length,
    hook: hooks.length,
    plugin: plugins.length,
    mcp: mcpServers.length,
  };
  
  return (
    <nav className="nav-sidebar">
      <div className="nav-logo">
        <span className="nav-logo-text">Agent Studio</span>
      </div>
      
      <div className="nav-section">
        <div className="nav-section-title">Overview</div>
        <button
          className={`nav-item ${activeView === 'dashboard' ? 'active' : ''}`}
          onClick={() => setActiveView('dashboard')}
        >
          <span className="nav-item-icon">{NAV_ITEMS[0].icon}</span>
          <span>Dashboard</span>
        </button>
      </div>
      
      <div className="nav-section" style={{ flex: 1 }}>
        <div className="nav-section-title">Entities</div>
        {NAV_ITEMS.slice(1).map((item) => (
          <button
            key={item.view}
            className={`nav-item ${activeView === item.view ? 'active' : ''}`}
            onClick={() => setActiveView(item.view)}
          >
            <span className="nav-item-icon">{item.icon}</span>
            <span>{item.label}</span>
            {item.entityType && counts[item.entityType] > 0 && (
              <span className="nav-item-badge">{counts[item.entityType]}</span>
            )}
          </button>
        ))}
      </div>
      
      {/* Keyboard hints at bottom */}
      <div className="nav-footer">
        <div className="nav-footer-hint">
          <kbd>⌘ .</kbd>
          <span>Shortcuts</span>
        </div>
      </div>
    </nav>
  );
}
