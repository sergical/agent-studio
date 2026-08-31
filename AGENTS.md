# AGENTS.md - Skill Studio

## Project Overview

Skill Studio is a Tauri 2.x desktop application to manage, sync, and test agent skills across Claude Code, Codex, OpenCode, and pi, with skills.sh discovery built in.

### Core Features

1. **Skill Discovery** - Search 36,000+ skills from skills.sh
2. **Skill Installation** - Install/remove/update via `npx skills` CLI to global or project scope
3. **First-Class Agents** - Claude Code, Codex, OpenCode, pi, plus a shared `.agents/skills` root
4. **Provenance** - Every installed skill is classified as `skills-sh`, `plugin`, `dotagents`, or `manual` (precedence: dotagents > plugin > skills-sh > manual)
5. **Native Plugin Enumeration** - Discovers skills shipped inside Claude Code (`~/.claude/plugins/cache`) and Codex (`~/.codex/plugins/cache`) plugin caches, per the agent-plugins.org manifest convention
6. **Spec Validation** - Flags agentskills.io SKILL.md spec violations (`spec_violations`) and detects the getsentry/skillet spec pattern (`has_spec`)

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
cd apps/desktop/src-tauri && cargo build

# Run Rust tests
cd apps/desktop/src-tauri && cargo test

# Run a single Rust test
cd apps/desktop/src-tauri && cargo test test_name

# Check Rust code
cd apps/desktop/src-tauri && cargo check

# Format Rust code
cd apps/desktop/src-tauri && cargo fmt

# Lint Rust code
cd apps/desktop/src-tauri && cargo clippy
```

### Frontend Commands

```bash
# Type check only (no emit)
npm run typecheck

# Lint (oxlint)
npm run lint
npm run lint:fix

# Format (oxfmt)
npm run format
npm run format:check

# Full gate: typecheck + lint + format:check + cargo fmt --check + clippy -D warnings + cargo test
npm run check
```

## Tech Stack

| Layer       | Technology                                                      |
| ----------- | --------------------------------------------------------------- |
| Framework   | Tauri 2.x (macOS desktop app)                                   |
| Frontend    | React 19.1, TypeScript 5.8                                      |
| Styling     | Tailwind CSS 4.x                                                |
| State       | Zustand 5.x                                                     |
| Linting     | oxlint (JS/TS), clippy (Rust)                                   |
| Formatting  | oxfmt (JS/TS), rustfmt (Rust)                                   |
| Lint plugin | anti-slop (local oxlint JS plugin in `tools/oxlint/anti-slop/`) |
| Icons       | lucide-react                                                    |
| Backend     | Rust 2021 Edition                                               |

## Project Structure

```
/
├── apps/
│   └── desktop/                  # The Tauri app (npm package "skill-studio")
│       ├── src/                  # React frontend
│       │   ├── components/
│       │   │   ├── SkillStore/   # SkillStore, SkillBrowser, SkillDetailPanel,
│       │   │   │                 # SkillDetailHeader, SkillContent, InstallControls,
│       │   │   │                 # AgentTargetSelector, SkillSearchBar, InstallProgressModal
│       │   │   └── ui/           # Toast, ToastContainer
│       │   ├── lib/
│       │   │   ├── skill-types.ts        # Type definitions
│       │   │   ├── skill-api.ts          # Tauri IPC wrappers
│       │   │   └── github-skill-source.ts # GitHub SKILL.md fetch
│       │   ├── store/
│       │   │   └── appStore.ts   # Zustand store
│       │   ├── App.tsx           # Main app component
│       │   └── main.tsx          # Entry point
│       └── src-tauri/            # Rust backend
│           ├── src/
│           │   ├── skills/
│           │   │   ├── mod.rs
│           │   │   ├── agents.rs         # AgentId, agent paths
│           │   │   ├── api.rs            # skills.sh HTTP client
│           │   │   ├── commands.rs       # Tauri IPC commands
│           │   │   ├── frontmatter.rs    # SKILL.md frontmatter parsing/validation
│           │   │   ├── lock_file.rs      # ~/.agents/.skill-lock.json
│           │   │   ├── plugins.rs        # Native plugin cache enumeration
│           │   │   ├── provenance.rs     # Source-kind classification
│           │   │   ├── scan.rs           # Installed-skill directory scanner
│           │   │   └── skill_dto.rs      # Serde DTOs sent to the frontend
│           │   ├── lib.rs                # Library entry
│           │   └── main.rs               # Rust entry point
│           └── Cargo.toml                # Rust dependencies
├── packages/
│   └── ui/                       # Shared UI package placeholder (@skill-studio/ui)
├── tools/
│   └── oxlint/anti-slop/         # Local oxlint JS plugin
└── package.json                  # npm workspaces root
```

## Code Style Guidelines

### TypeScript/React

**Imports:** Group in order - React, external libs, internal modules, types

```typescript
import { useEffect, useCallback, useState } from "react";
import { motion } from "motion/react";
import { X, Save } from "lucide-react";
import { useAppStore } from "./store/appStore";
import type { AgentId, InstalledSkill } from "./lib/skill-types";
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
export interface BaseEntityFields {
  id: string;
  name: string;
}
export type EntityType = "settings" | "memory" | "agent";
export type FilterScope = "all" | "global" | "project";
```

**Type Guards:** Create explicit type guards for discriminated unions

```typescript
export function isFlatEntity(entity: DisplayableEntity): entity is FlatEntity {
  return "path" in entity && "scope" in entity;
}
```

**File Headers:** Use comment blocks for major files

```typescript
// ============================================================================
// Skill Studio - Module Name
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

- Single store in `apps/desktop/src/store/appStore.ts`
- Use selectors for performance: `useAppStore((state) => state.activeView)`
- Group related state and actions together
- Invalidate caches by setting `_cachedSections: null`

### Naming Conventions

| Type         | Convention      | Example                    |
| ------------ | --------------- | -------------------------- |
| Components   | PascalCase      | `DetailPanel`, `Toast`     |
| Hooks        | camelCase + use | `useKeyboardNavigation`    |
| Types        | PascalCase      | `EntityType`, `ViewType`   |
| Variables    | camelCase       | `selectedEntity`, `isOpen` |
| Constants    | SCREAMING_SNAKE | `ENTITY_TEMPLATES`         |
| Files (TS)   | PascalCase.tsx  | `DetailPanel.tsx`          |
| Files (Rust) | snake_case.rs   | `mod.rs`, `lib.rs`         |

### Naming files

No bare-role filenames (`types.ts`, `api.ts`, `utils.ts`); prefix the domain (`skill-types.ts`, `skill-api.ts`). Use `index.ts` only as a thin re-export.

### Error Handling

**Frontend:** Use try-catch with toast notifications

```typescript
try {
  await discoverAll([homeDir]);
} catch (err) {
  addToast({
    type: "error",
    title: "Discovery Failed",
    message: err instanceof Error ? err.message : "Unknown error",
  });
}
```

**Backend:** Return Result types, use `.ok_or()` for Option conversion

```rust
let home = get_home_dir().ok_or("Could not find home directory")?;
```

### Key Files

| Purpose               | File                                              |
| --------------------- | ------------------------------------------------- |
| Main App              | `apps/desktop/src/App.tsx`                        |
| State Store           | `apps/desktop/src/store/appStore.ts`              |
| Skill types           | `apps/desktop/src/lib/skill-types.ts`             |
| Tauri IPC wrappers    | `apps/desktop/src/lib/skill-api.ts`               |
| GitHub SKILL.md fetch | `apps/desktop/src/lib/github-skill-source.ts`     |
| Tauri commands        | `apps/desktop/src-tauri/src/skills/commands.rs`   |
| Scanner               | `apps/desktop/src-tauri/src/skills/scan.rs`       |
| Provenance            | `apps/desktop/src-tauri/src/skills/provenance.rs` |
| Agent paths           | `apps/desktop/src-tauri/src/skills/agents.rs`     |
| Lint config           | `.oxlintrc.json`                                  |
| Format config         | `.oxfmtrc.json`                                   |
| Tauri Config          | `apps/desktop/src-tauri/tauri.conf.json`          |
| TS Config             | `apps/desktop/tsconfig.json`                      |

### TypeScript Strictness

Enabled in `apps/desktop/tsconfig.json`:

- `strict: true`
- `noUnusedLocals: true`
- `noUnusedParameters: true`
- `noFallthroughCasesInSwitch: true`

### Testing

- Rust: `cargo test` in `apps/desktop/src-tauri`, tests live in colocated `#[cfg(test)]` modules
- Frontend: no test runner is configured yet; use Vitest (compatible with Vite) when adding tests

## Reference docs

- `docs/agent-skill-conventions.md` — agentskills.io spec rules, per-agent discovery paths, invocation control (explicit vs model-invocable), native disable mechanisms, and the local data sources Skill Studio reads. Check it before researching agent behavior again.

## Skills.sh Integration

Skill Studio integrates with skills.sh for skill discovery and installation.

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
- Requires Node.js ^22.18 or >=24.11 (the `npx skills` CLI itself needs only 18+, but the build toolchain needs the newer range)

### First-Class Agents

| Agent       | Project Path                        | Global Path                                  |
| ----------- | ----------------------------------- | -------------------------------------------- |
| Claude Code | `.claude/skills/`                   | `~/.claude/skills/`                          |
| Codex       | `.codex/skills/`                    | `~/.codex/skills/`                           |
| OpenCode    | `.opencode/skills/` (also `skill/`) | `~/.config/opencode/skills/` (also `skill/`) |
| pi          | `.pi/skills/`                       | `~/.pi/agent/skills/`                        |
| shared      | `.agents/skills/`                   | `~/.agents/skills/`                          |

`npx skills` can still target the full agent list; see `apps/desktop/src-tauri/src/skills/agents.rs` for `AgentId`.
