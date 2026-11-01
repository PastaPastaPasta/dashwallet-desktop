#!/usr/bin/env bash
# GitHub Actions (release.yml, `decide` job, read-only): what this run builds and whether it may publish.
# Appends build=true|false and publish=true|false to $GITHUB_OUTPUT.
#
#   ci/github/release-decide.sh
#
# Needs: GITHUB_EVENT_NAME, GITHUB_REF, GITHUB_SHA, GITHUB_REPOSITORY, GITHUB_OUTPUT, and a checkout of
# full history (fetch-depth: 0). DECIDE_BASE is the commit the change is measured from: the push's
# `before` or the pull request's base.
#
# publish   a push to main or a release tag (release-publish.sh classify), in this repository, whose
#           commit is an ancestor of a freshly fetched origin/main. A release tag on a commit outside
#           main therefore never starts the job that holds write permissions. This check runs in the
#           read-only job; release-publish.sh repeats it. It is code of the pushed commit like the
#           workflow itself, so it cannot stand against a tag whose workflow was edited: that needs an
#           environment deployment policy or a tag ruleset (repository settings).
# build     always, except a branch push or pull request that changes only documentation and files no
#           release bundle is built from (docs, *.md, the Swift trees, the unrelated workflows). Main,
#           tags and dispatch always build. An unknown base (a new branch, a forced push) builds.
set -euo pipefail

event=${GITHUB_EVENT_NAME:?}
ref=${GITHUB_REF:?}
sha=${GITHUB_SHA:?}
repo=${GITHUB_REPOSITORY:?}
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

kind=$("$here/release-publish.sh" classify)

publish=false
if [[ "$kind" == skip ]]; then
  echo "decide: $event on $ref publishes nothing"
elif [[ "$repo" != PastaPastaPasta/dashwallet-desktop ]]; then
  echo "decide: $repo is not the release repository; publishing nothing"
else
  [[ "$(git rev-parse --is-shallow-repository)" == false ]] || { echo "::error::decide: shallow checkout; cannot check ancestry" >&2; exit 1; }
  [[ -n "${DECIDE_NO_FETCH:-}" ]] || git fetch --quiet --no-tags origin '+refs/heads/main:refs/remotes/origin/main'
  if git merge-base --is-ancestor "$sha" refs/remotes/origin/main; then
    publish=true
  else
    echo "::warning::decide: $sha is not on origin/main; $ref publishes nothing"
  fi
fi

build=true
base=${DECIDE_BASE:-}
if [[ "$ref" != refs/heads/main && "$ref" != refs/tags/* && ("$event" == push || "$event" == pull_request) ]] &&
  [[ -n "$base" && "$base" != 0000000000000000000000000000000000000000 ]] &&
  git cat-file -e "$base^{commit}" 2>/dev/null; then
  changed=$(git diff --name-only "$base" "$sha")
  other=$(grep -vE '^(docs/|(Sources|Tests|Apps|Vendor)/|Package\.(swift|resolved)$|\.github/workflows/(ci|gate|nightly|tauri-selftest)\.yml$)|\.md$' <<<"$changed" || true)
  if [[ -n "$changed" && -z "$other" ]]; then
    build=false
    echo "decide: only documentation and unrelated files changed; no build"
  fi
fi

echo "decide: kind=$kind publish=$publish build=$build"
{
  echo "publish=$publish"
  echo "build=$build"
} >> "${GITHUB_OUTPUT:?}"
