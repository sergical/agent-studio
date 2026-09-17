#!/usr/bin/env bash
# Fails when the version in apps/desktop/src-tauri/tauri.conf.json or
# apps/desktop/package.json does not equal the given tag with its leading
# "v" stripped. The release workflow runs this before the build so a
# forgotten version bump never reaches a tagged release.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <tag>" >&2
  exit 1
fi

tag="$1"
expected="${tag#v}"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tauri_conf="$repo_root/apps/desktop/src-tauri/tauri.conf.json"
package_json="$repo_root/apps/desktop/package.json"

tauri_version="$(node -pe "require('$tauri_conf').version")"
package_version="$(node -pe "require('$package_json').version")"

status=0

if [[ "$tauri_version" != "$expected" ]]; then
  echo "tauri.conf.json version '$tauri_version' does not match tag '$tag' (expected '$expected')" >&2
  status=1
fi

if [[ "$package_version" != "$expected" ]]; then
  echo "package.json version '$package_version' does not match tag '$tag' (expected '$expected')" >&2
  status=1
fi

exit "$status"
