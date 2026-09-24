#!/usr/bin/env bash
# Fails when a release is missing any artifact a complete two-architecture
# macOS release must carry.
#
# release.yml builds aarch64 and x86_64 in two matrix legs, and both legs
# upload to the same release with tauri-action. The release is created as a
# draft and is published only from a final job that runs this script first, so
# a leg that failed (or any step that failed after an upload) can never leave a
# public release that only carries one architecture. This asserts, from the
# release's own uploaded assets, that both legs landed: two DMGs, two updater
# archives with their signatures, the two dSYM archives, and a latest.json
# that names both darwin-aarch64 and darwin-x86_64. A single-target
# latest.json is exactly the dead end the updater hits when the other
# architecture is missing.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <tag>" >&2
  exit 1
fi

tag="$1"

assets="$(gh release view "$tag" --json assets --jq '.assets[].name')"

count() { printf '%s\n' "$assets" | grep -cE "$1" || true; }

problems=()

if [[ "$(count '\.dmg$')" -ne 2 ]]; then
  problems+=("expected 2 .dmg assets (one per architecture), found $(count '\.dmg$')")
fi
if [[ "$(count '\.app\.tar\.gz$')" -ne 2 ]]; then
  problems+=("expected 2 .app.tar.gz updater archives, found $(count '\.app\.tar\.gz$')")
fi
if [[ "$(count '\.app\.tar\.gz\.sig$')" -ne 2 ]]; then
  problems+=("expected 2 .app.tar.gz.sig signatures, found $(count '\.app\.tar\.gz\.sig$')")
fi
if [[ "$(count '\.dSYM\.zip$')" -ne 2 ]]; then
  problems+=("expected 2 .dSYM.zip debug-symbol archives, found $(count '\.dSYM\.zip$')")
fi

if printf '%s\n' "$assets" | grep -qx 'latest.json'; then
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  gh release download "$tag" --pattern 'latest.json' --dir "$tmp" --clobber
  for platform in darwin-aarch64 darwin-x86_64; do
    if ! jq -e --arg p "$platform" '.platforms[$p].url and .platforms[$p].signature' "$tmp/latest.json" >/dev/null; then
      problems+=("latest.json has no url+signature entry for $platform")
    fi
  done
else
  problems+=("missing latest.json updater manifest")
fi

if [[ ${#problems[@]} -ne 0 ]]; then
  echo "Release $tag is incomplete; refusing to publish it." >&2
  printf '  - %s\n' "${problems[@]}" >&2
  echo "Uploaded assets:" >&2
  printf '  %s\n' "$assets" >&2
  exit 1
fi

echo "Release $tag is complete: 2 DMGs, 2 updater archives with signatures, 2 dSYM archives, and a latest.json covering darwin-aarch64 and darwin-x86_64."
