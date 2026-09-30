#!/usr/bin/env bash
# Fails when the built CLI lacks a command that packages/npm/skill-studio/
# README.md tells users to run. The README ships on npmjs.com with the
# binary, so a tag cut from a tree whose CLI predates those commands (for
# example before `mcp` and `usage` landed) would publish instructions that
# fail with "unrecognized subcommand". Each probe asks clap for help, which
# exits 0 only when the command, and any positional it takes, exists.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <path-to-skill-studio-binary>" >&2
  exit 1
fi

bin="$1"
# Help output never reads the home folder, but an empty one keeps a probe
# from ever touching a real ~/.agents on a developer machine.
probe_home="$(mktemp -d)"
trap 'rm -rf "$probe_home"' EXIT

probes=(
  "scan --help"
  "diagnose --help"
  "usage --help"
  "park some-skill --help"
  "unpark some-skill --help"
  "undo --help"
  "mcp --help"
)

status=0
for probe in "${probes[@]}"; do
  read -r -a args <<< "$probe"
  if ! HOME="$probe_home" "$bin" "${args[@]}" > /dev/null 2>&1; then
    echo "::error::skill-studio $probe failed; the npm README documents this command." >&2
    status=1
  fi
done

exit "$status"
