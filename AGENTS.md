# AGENTS.md - Agent Studio

## Project Overview

Agent Studio is a **GUI for skills.sh** - a Tauri 2.x desktop application for discovering, installing, and managing agent skills across 41+ AI coding assistants. It also provides memory file management (AGENTS.md/CLAUDE.md consistency) and health checks.

### Core Features
1. **Skill Discovery** - Search 36,000+ skills from skills.sh
2. **Skill Installation** - Install to global or project scope via `npx skills` CLI
3. **Multi-Agent Support** - Claude Code, Cursor, OpenCode, Cline, Windsurf, and 36+ more
4. **Memory Management** - AGENTS.md/CLAUDE.md consistency and symlink detection
5. **Health Checks** - Duplicate detection, broken symlinks, configuration issues

### Tech Stack
- **Frontend**: React 19 + TypeScript + Tailwind CSS 4.x + Zustand
- **Backend**: Tauri 2.x (Rust)
- **Skills Integration**: skills.sh API + `npx skills` CLI

## Build & Development Commands

```bash
# Install dependencies
npm install

# Development mode (starts Vite + Tauri)
npm run tauri dev

# Build for production
npm run tauri build

# Frontend only (Vite dev server)
npm run dev

# Type check + build frontend
npm run build

# Preview built frontend
npm run preview
```

### Rust Commands

```bash
# Build Rust backend
cd src-tauri && cargo build

# Run Rust tests
cd src-tauri && cargo test

# Run a single Rust test
cd src-tauri && cargo test test_name

# Check Rust code
cd src-tauri && cargo check

# Format Rust code
cd src-tauri && cargo fmt

# Lint Rust code
cd src-tauri && cargo clippy
```

### Frontend Commands

```bash
# Type check only (no emit)
npx tsc --noEmit

# Note: No ESLint/Prettier configured - rely on TypeScript strict mode
```

## Tech Stack

| Layer     | Technology                           |
|-----------|--------------------------------------|
| Framework | Tauri 2.x (macOS desktop app)        |
| Frontend  | React 19.1, TypeScript 5.8           |
| Styling   | Tailwind CSS 4.x                     |
| State     | Zustand 5.x                          |
| Editor    | Monaco Editor (@monaco-editor/react) |
| Animation | Motion (formerly Framer Motion)      |
| Icons     | lucide-react                         |
| Backend   | Rust 2021 Edition                    |

## Project Structure

```
/
├── src/                    # React frontend
│   ├── components/         # React components
│   │   └── ui/            # Reusable UI components
│   ├── hooks/             # Custom React hooks
│   ├── lib/               # Utilities (api.ts, types.ts)
│   ├── store/             # Zustand store (appStore.ts)
│   ├── App.tsx            # Main app component
│   └── main.tsx           # Entry point
├── src-tauri/             # Rust backend
│   ├── src/
│   │   ├── commands/      # Tauri IPC commands (mod.rs)
│   │   ├── lib.rs         # Library entry
│   │   └── main.rs        # Rust entry point
│   └── Cargo.toml         # Rust dependencies
└── package.json           # npm dependencies
```

## Code Style Guidelines

### TypeScript/React

**Imports:** Group in order - React, external libs, internal modules, types
```typescript
import { useEffect, useCallback, useState } from 'react';
import { motion } from 'motion/react';
import { X, Save } from 'lucide-react';
import { useAppStore } from './store/appStore';
import type { EntityType, DisplayableEntity } from './lib/types';
```

**Components:** Use function components with explicit prop interfaces
```typescript
interface PanelProps {
  isOpen: boolean;
  onClose: () => void;
  title: string;
  children: React.ReactNode;
}

export function Panel({ isOpen, onClose, title, children }: PanelProps) {
  // ...
}
```

**Hooks:** Prefix with `use`, return object or array consistently
```typescript
export function useKeyboardNavigation(options: Options) { ... }
```

**Types:** Use `interface` for objects, `type` for unions/primitives
```typescript
export interface BaseEntityFields { id: string; name: string; }
export type EntityType = 'settings' | 'memory' | 'agent';
export type FilterScope = 'all' | 'global' | 'project';
```

**Type Guards:** Create explicit type guards for discriminated unions
```typescript
export function isFlatEntity(entity: DisplayableEntity): entity is FlatEntity {
  return 'path' in entity && 'scope' in entity;
}
```

**File Headers:** Use comment blocks for major files
```typescript
// ============================================================================
// Agent Studio - Module Name
// Brief description of purpose
// ============================================================================
```

### Rust

**Structs:** Use `#[derive(Debug, Serialize, Deserialize, Clone)]`
```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BaseEntity {
    pub id: String,
    pub name: String,
}
```

**Tauri Commands:** Use `#[tauri::command]` attribute
```rust
#[tauri::command]
pub fn discover_all(project_paths: Option<Vec<String>>) -> Result<DiscoveryResult, String> {
    // ...
}
```

**Error Handling:** Return `Result<T, String>` for Tauri commands
```rust
fn get_home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}
```

### Tailwind CSS

- Use CSS variables for theming: `var(--color-bg-primary)`, `var(--color-text-primary)`
- Prefer utility classes over custom CSS
- Use responsive prefixes: `sm:`, `md:`, `lg:`
- Common patterns: `flex items-center gap-2`, `px-4 py-2`, `rounded-md`

### State Management (Zustand)

- Single store in `src/store/appStore.ts`
- Use selectors for performance: `useAppStore((state) => state.activeView)`
- Group related state and actions together
- Invalidate caches by setting `_cachedSections: null`

### Naming Conventions

| Type          | Convention        | Example                    |
|---------------|-------------------|----------------------------|
| Components    | PascalCase        | `DetailPanel`, `Toast`     |
| Hooks         | camelCase + use   | `useKeyboardNavigation`    |
| Types         | PascalCase        | `EntityType`, `ViewType`   |
| Variables     | camelCase         | `selectedEntity`, `isOpen` |
| Constants     | SCREAMING_SNAKE   | `ENTITY_TEMPLATES`         |
| Files (TS)    | PascalCase.tsx    | `DetailPanel.tsx`          |
| Files (Rust)  | snake_case.rs     | `mod.rs`, `lib.rs`         |

### Error Handling

**Frontend:** Use try-catch with toast notifications
```typescript
try {
  await discoverAll([homeDir]);
} catch (err) {
  addToast({
    type: 'error',
    title: 'Discovery Failed',
    message: err instanceof Error ? err.message : 'Unknown error',
  });
}
```

**Backend:** Return Result types, use `.ok_or()` for Option conversion
```rust
let home = get_home_dir().ok_or("Could not find home directory")?;
```

### Key Files

| Purpose              | File                                |
|----------------------|-------------------------------------|
| Main App             | `src/App.tsx`                       |
| State Store          | `src/store/appStore.ts`             |
| Type Definitions     | `src/lib/types.ts`                  |
| API Layer            | `src/lib/api.ts`                    |
| Tauri Commands       | `src-tauri/src/commands/mod.rs`     |
| Tauri Config         | `src-tauri/tauri.conf.json`         |
| TS Config            | `tsconfig.json`                     |

### TypeScript Strictness

Enabled in `tsconfig.json`:
- `strict: true`
- `noUnusedLocals: true`
- `noUnusedParameters: true`
- `noFallthroughCasesInSwitch: true`

### Testing

No testing framework is currently configured. When adding tests:
- Use Vitest for frontend (compatible with Vite)
- Use `cargo test` for Rust backend

## Skills.sh Integration

Agent Studio integrates with skills.sh for skill discovery and installation.

### API Endpoint
- **Search**: `https://skills.sh/api/search?q=<query>`
- Returns skill metadata including name, install count, top source

### Lock File (`~/.agents/.skill-lock.json`)
Tracks installed skills with their sources and hashes:
```json
{
  "version": 3,
  "skills": {
    "skill-name": {
      "source": "owner/repo",
      "sourceType": "github",
      "sourceUrl": "https://github.com/...",
      "skillFolderHash": "abc123",
      "installedAt": "2024-01-31T...",
      "updatedAt": "2024-01-31T..."
    }
  }
}
```

### CLI Dependency
- Installation: Uses `npx skills add <skill>` for battle-tested install logic
- Removal: Uses `npx skills remove <skill>`
- Updates: Uses `npx skills update <skill>`
- Requires Node.js 18+

### Supported Agents (41 total)

| Agent | Project Path | Global Path |
|-------|--------------|-------------|
| Claude Code | `.claude/skills/` | `~/.claude/skills/` |
| OpenCode | `.opencode/skills/` | `~/.config/opencode/skills/` |
| Cursor | `.cursor/skills/` | `~/.cursor/skills/` |
| Cline | `.cline/skills/` | `~/.cline/skills/` |
| Windsurf | `.windsurf/skills/` | `~/.windsurf/skills/` |
| Roo Code | `.roo-code/skills/` | `~/.roo-code/skills/` |
| Codex | `.codex/skills/` | `~/.codex/skills/` |
| Amp | `.amp/skills/` | `~/.amp/skills/` |
| Zed | `.zed/skills/` | `~/.zed/skills/` |
| Void | `.void/skills/` | `~/.void/skills/` |
| Aider | `.aider/skills/` | `~/.aider/skills/` |
| Pear AI | `.pearai/skills/` | `~/.pearai/skills/` |
| Continue | `.continue/skills/` | `~/.continue/skills/` |

See `src-tauri/src/skills/types.rs` for the complete list of 41 agents.

## Multi-Tool Support (Claude Code & OpenCode)

Agent Studio supports both **Claude Code** and **OpenCode** coding assistants. Each tool has its own configuration paths and file formats.

### Tool Identification

All entities have a `tool` field (`'claude' | 'opencode'`) to identify which tool they belong to:
- **Claude Code**: Orange indicator (#F97316)
- **OpenCode**: Dark Blue indicator (#1E40AF)

### Configuration Paths

| Tool | Scope | Path |
|------|-------|------|
| Claude | Global | `~/.claude/` |
| Claude | Project | `.claude/` |
| OpenCode | Global | `~/.config/opencode/` |
| OpenCode | Project | `.opencode/` |

### Entity Paths

| Entity | Claude | OpenCode |
|--------|--------|----------|
| Settings | `settings.json` | `opencode.json` / `opencode.jsonc` |
| Memory | `CLAUDE.md` | `AGENTS.md` |
| Agents | `agents/*.md` | `agent/*.md` |
| Skills | `skills/*/SKILL.md` | `skill/*/SKILL.md` |
| Commands | `commands/*.md` | `command/*.md` |
| MCP Servers | `mcpServers` in settings | `mcp` key in opencode.json |

### Key Implementation Details

**TypeScript Types** (`src/lib/types.ts`):
```typescript
export type ToolType = 'claude' | 'opencode';
export const TOOL_COLORS: Record<ToolType, string> = {
  claude: '#F97316',
  opencode: '#1E40AF',
};
```

**Filtering**: Store has `filterTool` state with localStorage persistence:
```typescript
filterTool: 'all' | 'claude' | 'opencode'
```

**Entity Creation**: `createEntity` API accepts a `tool` parameter to create entities in the appropriate directory structure.

**Rust Discovery**: `discover_all` discovers entities from both Claude and OpenCode paths, setting the `tool` field appropriately on each entity.

