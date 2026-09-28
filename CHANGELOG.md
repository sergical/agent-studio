# Changelog

All notable changes to Skill Studio are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## Unreleased

- The telemetry switch in `~/.agents/skill-studio.json` is now `telemetry_enabled`; a file saved by an rc build under `error_reporting_enabled` is still read.

## v0.1.0

### Added

- Signed and notarized macOS release workflow, so the app installs without a
  Gatekeeper warning.
- Shared `skill-studio-core` crate: scan, ops, events, and DTOs used by the
  desktop app, the CLI, and the MCP server.
- Crash report scaffolding: a panic hook that sends only the code location
  where Skill Studio crashed. The welcome screen offers it on; Settings'
  "Telemetry" switch lets the user change that choice. Off in the registry
  until a choice is saved.

### Removed

- Eleven orphan IPC commands with no frontend caller.
