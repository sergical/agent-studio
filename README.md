# Agent Studio

A macOS desktop application for managing coding assistant configuration files in one place. Supports Claude Code, OpenCode, and AGENTS.md configurations.

## Features

- **Settings Editor**: Visual and code-based editor for JSON configuration files
  - Claude Code: `~/.claude/settings.json`, `.claude/settings.json`
  - OpenCode: `~/.config/opencode/opencode.json`, `opencode.json`
  
- **Agent Manager**: Create and edit custom subagents
  - Claude Code agents: `~/.claude/agents/*.md`
  - OpenCode agents: `~/.config/opencode/agent/*.md`
  - Form-based editor with YAML frontmatter support
  
- **Skills Discovery**: Browse and manage reusable skills
  - Claude Code skills: `~/.claude/skills/*/SKILL.md`
  - OpenCode skills: `~/.config/opencode/skill/*/SKILL.md`
  
- **Template Library**: Pre-built templates for common agent types
  - Code Reviewer
  - Debugger
  - Test Writer
  - Security Auditor
  - Documentation Writer

## Screenshots

The app features a dark theme with a sidebar navigation and Monaco editor integration.

## Development

### Prerequisites

- [Node.js](https://nodejs.org/) (v18+)
- [Rust](https://rustup.rs/)
- [Tauri CLI](https://tauri.app/start/prerequisites/)

### Setup

```bash
# Install dependencies
npm install

# Run in development mode
npm run tauri dev

# Build for production
npm run tauri build
```

### Tech Stack

- **Frontend**: React, TypeScript, Tailwind CSS
- **Editor**: Monaco Editor
- **Backend**: Rust, Tauri 2.x
- **State Management**: Zustand

## Configuration Files Supported

### Claude Code

| File | Location | Description |
|------|----------|-------------|
| `settings.json` | `~/.claude/` | Global settings |
| `settings.json` | `.claude/` | Project settings |
| `settings.local.json` | `.claude/` | Local project settings |
| `CLAUDE.md` | `~/.claude/` or project root | Memory/instructions |
| `agents/*.md` | `~/.claude/agents/` | Custom subagents |
| `skills/*/SKILL.md` | `~/.claude/skills/` | Custom skills |

### OpenCode

| File | Location | Description |
|------|----------|-------------|
| `opencode.json` | `~/.config/opencode/` | Global config |
| `opencode.json` | Project root | Project config |
| `AGENTS.md` | `~/.config/opencode/` or project root | Rules/instructions |
| `agent/*.md` | `~/.config/opencode/agent/` | Custom agents |
| `skill/*/SKILL.md` | `~/.config/opencode/skill/` | Custom skills |

### AGENTS.md

Universal agent instructions file supported by multiple AI coding tools.

## License

MIT
