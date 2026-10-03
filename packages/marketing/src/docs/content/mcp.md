# MCP server

Connect Skill Studio to your agent. Then you can ask the agent to check, fix and park your skills.

The server runs with `npx skill-studio mcp`. It needs Node.js and macOS.

## Add to Claude Code

```sh
claude mcp add --transport stdio skill-studio -- npx -y skill-studio mcp
```

To share the server with a project, add it to `.mcp.json` in the project folder:

```json
{
  "mcpServers": {
    "skill-studio": {
      "command": "npx",
      "args": ["-y", "skill-studio", "mcp"]
    }
  }
}
```

## Add to other agents

### Codex

```sh
codex mcp add skill-studio -- npx -y skill-studio mcp
```

Or add it to `~/.codex/config.toml`:

```toml
[mcp_servers.skill-studio]
command = "npx"
args = ["-y", "skill-studio", "mcp"]
startup_timeout_sec = 30
```

The first start downloads the package, which can take longer than the default 10 seconds. `startup_timeout_sec = 30` gives it more time.

### Cursor

Add it to `.cursor/mcp.json` in your project, or to `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "skill-studio": {
      "command": "npx",
      "args": ["-y", "skill-studio", "mcp"]
    }
  }
}
```

### OpenCode

Add it to `opencode.json` in your project, or to `~/.config/opencode/opencode.json`:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "skill-studio": {
      "type": "local",
      "command": ["npx", "-y", "skill-studio", "mcp"],
      "enabled": true
    }
  }
}
```

## What your agent can do

- List your skills and find problems.
- Find unused skills.
- Find skills that have copies with different content.
- Fix broken skills.
- Park and unpark skills.
- Remove a duplicate copy.
- Turn a skill off for all agents by parking it. Turning it off for one agent only is coming.
- Install skills, find skills that have a newer version, and update them.
- List past changes and restore one.

## Try it

Ask your agent:

```text
Find my unused and broken skills and park the ones I don't need.
```

## Undo

Each change that your agent makes goes into the same history that the app shows. To undo a change, open **Activity** in the app and select **Restore**, or run `npx skill-studio undo`. To undo a park, unpark the skill.
