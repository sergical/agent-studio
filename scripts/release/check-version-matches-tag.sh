#!/usr/bin/env bash
# Fails when the version in apps/desktop/src-tauri/tauri.conf.json,
# apps/desktop/package.json, or one of the npm CLI packages under
# packages/npm (including the main package's optionalDependencies pins on the
# platform packages) does not equal the given tag's base version (leading "v"
# stripped, and any "-<suffix>" pre-release suffix stripped).
# The release workflow runs this before the build so a forgotten version
# bump never reaches a tagged release. A pre-release tag like
# "v0.1.0-rc.1" is expected to match a committed "0.1.0" - the full
# "0.1.0-rc.1" only ever appears as the built app's own version, stamped in
# separately via tauri.release.conf.json (see release.yml).
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <tag>" >&2
  exit 1
fi

tag="$1"
full="${tag#v}"
expected="${full%%-*}"

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

npm_dir="$repo_root/packages/npm"
for package in skill-studio cli-darwin-arm64 cli-darwin-x64; do
  npm_version="$(node -pe "require('$npm_dir/$package/package.json').version")"
  if [[ "$npm_version" != "$expected" ]]; then
    echo "packages/npm/$package/package.json version '$npm_version' does not match tag '$tag' (expected '$expected')" >&2
    status=1
  fi
done

# The launcher can only find a platform binary the main package pins, so a
# stale pin would publish a main package that installs last release's binary.
for platform in cli-darwin-arm64 cli-darwin-x64; do
  pin="$(node -pe "require('$npm_dir/skill-studio/package.json').optionalDependencies?.['@skill-studio/$platform'] ?? ''")"
  if [[ "$pin" != "$expected" ]]; then
    echo "packages/npm/skill-studio/package.json pins @skill-studio/$platform at '$pin', expected '$expected'" >&2
    status=1
  fi
done

exit "$status"
