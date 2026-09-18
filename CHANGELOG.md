# Changelog

All notable changes to Skill Studio are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## Unreleased

## v0.1.0

### Added

- Signed and notarized macOS release workflow, so the app installs without a
  Gatekeeper warning.
- Shared `skill-studio-core` crate: scan, ops, events, and DTOs used by the
  desktop app, the CLI, and the MCP server.
- Opt-in error reporting scaffolding: a Settings switch and a panic hook,
  off until the user turns it on.

### Removed

- Eleven orphan IPC commands with no frontend caller.
