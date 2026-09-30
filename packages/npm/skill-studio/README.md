# skill-studio

Tidy up your agent skills. `skill-studio` finds your skills across Claude Code, Codex, OpenCode, pi, Cursor, Grok Build and the shared `.agents` folder. It finds broken, duplicate and unused skills. You can undo most changes.

It runs on macOS, on Apple silicon and Intel Macs. It needs Node.js 18 or later.

## Run it

```bash
npx skill-studio diagnose
```

Or install it:

```bash
npm install -g skill-studio
skill-studio --help
```

Some commands:

- `skill-studio scan` lists every skill installed for your agents.
- `skill-studio diagnose` finds broken skills and says what is wrong with each.
- `skill-studio usage` shows which skills your agents used, and which they never used.
- `skill-studio park <skill>` turns a skill off for all agents. `skill-studio unpark <skill>` turns it on again.
- `skill-studio undo` reverses your last install, remove, update, split or on/off change. It does not undo park or unpark. It skips them and reverses the change before them. To undo a park, run `skill-studio unpark <skill>`.

## Use it from your agent (MCP)

`skill-studio mcp` starts an MCP server. Add it to your agent:

Claude Code:

```bash
claude mcp add --transport stdio skill-studio -- npx -y skill-studio mcp
```

Codex:

```bash
codex mcp add skill-studio -- npx -y skill-studio mcp
```

The first `npx` download can take more than ten seconds. If Codex stops waiting, set `startup_timeout_sec = 30` under `[mcp_servers.skill-studio]` in `~/.codex/config.toml`.

Cursor, in `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "skill-studio": { "command": "npx", "args": ["-y", "skill-studio", "mcp"] }
  }
}
```

OpenCode, in `opencode.json`:

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

## Privacy

No account. It runs on your Mac. It can send anonymous crash reports, which you can turn off.

## License

MIT
