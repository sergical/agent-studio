#!/usr/bin/env bash
# Fails when the commit a release tag points at is not on the repository's
# default branch.
#
# release.yml triggers on `push: tags`, which never runs the branch
# ruleset's required checks (`test` from rust.yml, `check` from web.yml) and
# never proves the tagged commit was merged. Without this check a `v*` tag
# on an unmerged or never-built commit would go straight to a public
# release, because the release build is the only place a desktop production
# build runs. This asserts the tagged commit is reachable from the default
# branch before the build job starts, so the release path is covered by the
# same "must be on main" rule the merge gate enforces.
#
# Ancestry is answered through GitHub's compare API instead of a local
# `git merge-base --is-ancestor`, so the check needs no full clone of the
# default branch's history: with base = the tagged commit and head = the
# default branch, the commit is on that branch exactly when the compare
# status is `ahead` (the branch is ahead of it) or `identical` (the tag is
# the branch head). `behind` (the commit is not merged) and `diverged` both
# fail, as does any unexpected status or API error.
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <commit-sha> <default-branch>" >&2
  exit 1
fi

sha="$1"
branch="$2"
repo="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY must be set}"

status="$(gh api "repos/${repo}/compare/${sha}...${branch}" --jq '.status')"

case "$status" in
  ahead|identical)
    echo "Commit ${sha} is on ${branch} (compare status: ${status})."
    ;;
  behind)
    echo "Refusing to release: commit ${sha} is not on ${branch} - the branch is behind an unmerged commit." >&2
    exit 1
    ;;
  diverged)
    echo "Refusing to release: commit ${sha} is not on ${branch} - the histories diverged." >&2
    exit 1
    ;;
  *)
    echo "Refusing to release: compare of ${sha}...${branch} returned unexpected status '${status}'." >&2
    exit 1
    ;;
esac
