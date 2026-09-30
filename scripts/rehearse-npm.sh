#!/usr/bin/env bash
# Rehearses the npm release of the CLI on this Mac, without publishing.
#
# Usage: scripts/rehearse-npm.sh <path to a built skill-studio binary>
#
# Copies the three packages under packages/npm to a temp dir, puts the binary
# into the platform package for this Mac's CPU, packs all three, checks that
# the packed binary keeps its exec bit, installs the platform and main
# tarballs into a temp prefix with a temp HOME, and runs
# `skill-studio --help` through the JS launcher. Nothing touches the real
# HOME, ~/.npm or the global npm prefix.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <path to skill-studio binary>" >&2
  exit 1
fi

binary="$1"
if [[ ! -f "$binary" ]]; then
  echo "no file at $binary" >&2
  exit 1
fi

case "$(uname -m)" in
  arm64) platform="cli-darwin-arm64" ;;
  x86_64) platform="cli-darwin-x64" ;;
  *)
    echo "unsupported CPU $(uname -m); the CLI ships for arm64 and x86_64 Macs only" >&2
    exit 1
    ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

cp -R "$repo_root/packages/npm" "$work/src"
mkdir -p "$work/src/$platform/bin"
cp "$binary" "$work/src/$platform/bin/skill-studio"
chmod 755 "$work/src/$platform/bin/skill-studio"

export HOME="$work/home"
export npm_config_cache="$work/npm-cache"
export npm_config_prefix="$work/prefix"
export npm_config_update_notifier=false
mkdir -p "$HOME" "$work/packs"

for package in cli-darwin-arm64 cli-darwin-x64 skill-studio; do
  (cd "$work/packs" && npm pack "$work/src/$package" --silent > /dev/null)
done

platform_tgz="$(ls "$work/packs/skill-studio-$platform-"*.tgz)"
main_tgz="$(ls "$work/packs"/skill-studio-[0-9]*.tgz)"

check_executable() {
  local tgz="$1" path="$2" mode
  mode="$(tar -tvf "$tgz" "package/$path" | awk '{print $1}')"
  if [[ "$mode" != -rwx* ]]; then
    echo "FAIL: $path in $(basename "$tgz") has mode '$mode', expected -rwxr-xr-x" >&2
    exit 1
  fi
  echo "ok: $path in $(basename "$tgz") is $mode"
}
check_executable "$platform_tgz" bin/skill-studio
check_executable "$main_tgz" bin/skill-studio.js

# --omit=optional: the main package pins platform packages that are not on
# npm yet at this version. The platform tarball installs beside it in the
# global prefix, which is where the launcher's require.resolve finds it.
npm install --global --omit=optional --no-audit --no-fund --silent "$platform_tgz" "$main_tgz"

cli="$npm_config_prefix/bin/skill-studio"
if ! "$cli" --help > "$work/help.txt"; then
  echo "FAIL: skill-studio --help exited with an error" >&2
  exit 1
fi
head -n 5 "$work/help.txt"
echo "ok: skill-studio --help ran through the npm launcher"

set +e
"$cli" no-such-command > /dev/null 2>&1
status=$?
set -e
if [[ $status -eq 0 ]]; then
  echo "FAIL: an unknown command exited 0; the launcher is not forwarding the exit code" >&2
  exit 1
fi
echo "ok: an unknown command exits $status through the launcher"

echo "Rehearsal passed for $platform."
