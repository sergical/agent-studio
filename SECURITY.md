# Security Policy

## Supported versions

Only the latest release receives security fixes. Older releases are not
patched.

## Reporting a vulnerability

Report a vulnerability through GitHub private vulnerability reporting:

https://github.com/sergical/agent-studio/security/advisories/new

Do not report a vulnerability through a public issue.

When you report, include:

- A description of the vulnerability and its impact.
- Steps to reproduce it, including your OS and app version.
- Any proof-of-concept code or logs (remove secrets first).

You should get a first answer within 7 days.

## Scope

In scope:

- The desktop app (`apps/desktop`).
- The CLI (`apps/cli`).
- The MCP server (`apps/mcp`).
- The hosted skills.sh proxy (`apps/server`).

Out of scope:

- Content of third-party skills installed through the app. Report a
  malicious skill to its own source, not to this project.
