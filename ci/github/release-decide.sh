#!/usr/bin/env bash
# GitHub Actions (release.yml, `decide` job, read-only): what this run builds and whether it may publish.
# Appends build=, publish=, stable_tag=, signed_stable=, signed_rc= and detached=true|false to $GITHUB_OUTPUT.
#
#   ci/github/release-decide.sh
#
# Needs: GITHUB_EVENT_NAME, GITHUB_REF, GITHUB_SHA, GITHUB_REPOSITORY, GITHUB_OUTPUT, and a checkout of
# full history (fetch-depth: 0). DECIDE_BASE is the commit the change is measured from: the push's
# `before` or the pull request's base.
#
# publish     a push to main or a release tag (release-publish.sh classify), in this repository, whose
#             commit is an ancestor of a freshly fetched origin/main. A release tag on a commit outside
#             main therefore never starts the job that holds write permissions. This check runs in the
#             read-only job; release-publish.sh repeats it.
# stable_tag     publish, on a final stable tag vX.Y.Z (never rc, dry-run, main or dispatch).
# signed_stable  stable_tag, unless DW_ALLOW_UNSIGNED_STABLE=1 is set (and DW_SIGNING_REQUIRED != 1).
#                Only signed_stable=true embeds the stable updater feed URL, packs unsigned-macos /
#                unsigned.SHA256SUMS, and waits for detached signatures; under DW_ALLOW_UNSIGNED_STABLE=1
#                the build ships without an updater endpoint so it never trusts an unrotated key.
# signed_rc      publish, on a release candidate tag vX.Y.Z-rc.N, when the repository variable DW_SIGN_RC=1
#                opts candidates in to detached signing (docs/release/RC-SIGNING.md). Packs the same
#                unsigned signing inputs and waits for release-publish-signed.yml like a stable tag, but
#                never embeds the updater feed URL. Unset, a candidate stays keyless and publishes at once.
# detached       signed_stable or signed_rc: the run packs signing inputs and waits for detached signatures.
# build       always, except a branch push or pull request that changes only documentation and files no
#             release bundle is built from (docs, *.md, the Swift trees, the unrelated workflows). Main,
#             tags and dispatch always build.
set -euo pipefail

event=${GITHUB_EVENT_NAME:?}
ref=${GITHUB_REF:?}
sha=${GITHUB_SHA:?}
repo=${GITHUB_REPOSITORY:?}
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

kind=$("$here/release-publish.sh" classify)
[[ -n "${DECIDE_NO_FETCH:-}" ]] || git fetch --quiet --no-tags origin '+refs/heads/main:refs/remotes/origin/main'

publish=false
if [[ "$kind" == skip ]]; then
  echo "decide: $event on $ref publishes nothing"
elif [[ "$repo" != PastaPastaPasta/dashwallet-desktop ]]; then
  echo "decide: $repo is not the release repository; publishing nothing"
else
  [[ "$(git rev-parse --is-shallow-repository)" == false ]] || { echo "::error::decide: shallow checkout; cannot check ancestry" >&2; exit 1; }
  if git merge-base --is-ancestor "$sha" refs/remotes/origin/main; then
    publish=true
  else
    echo "::warning::decide: $sha is not on origin/main; $ref publishes nothing"
  fi
fi

build=true
base=
if [[ "$event" == push ]]; then
  base=$(git merge-base refs/remotes/origin/main "$sha" 2>/dev/null || true)
elif [[ "$event" == pull_request ]]; then
  base=${DECIDE_BASE:-}
fi
if [[ "$ref" != refs/heads/main && "$ref" != refs/tags/* ]] && [[ -n "$base" ]] && git cat-file -e "$base^{commit}" 2>/dev/null; then
  changed=$(git diff --no-renames --name-only "$base" "$sha")
  other=$(grep -vE '^(docs/|(Sources|Tests|Apps|Vendor)/|Package\.(swift|resolved)$|\.github/workflows/(ci|gate|nightly|tauri-selftest)\.yml$)|\.md$' <<<"$changed" || true)
  if [[ -n "$changed" && -z "$other" ]]; then
    build=false
    echo "decide: only documentation and unrelated files changed; no build"
  fi
fi

stable_tag=false
if [[ "$publish" == true && "$event" == push && "$ref" =~ ^refs/tags/v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  stable_tag=true
fi

signed_stable=false
if [[ "$stable_tag" == true ]] && [[ "${DW_ALLOW_UNSIGNED_STABLE:-}" != 1 || "${DW_SIGNING_REQUIRED:-}" == 1 ]]; then
  signed_stable=true
fi

signed_rc=false
if [[ "$publish" == true && "$event" == push && "$ref" =~ ^refs/tags/v[0-9]+\.[0-9]+\.[0-9]+-rc\.[0-9]+$ && "${DW_SIGN_RC:-}" == 1 ]]; then
  signed_rc=true
fi
detached=false
if [[ "$signed_stable" == true || "$signed_rc" == true ]]; then
  detached=true
fi

echo "decide: kind=$kind publish=$publish build=$build stable_tag=$stable_tag signed_stable=$signed_stable signed_rc=$signed_rc detached=$detached"
{
  echo "publish=$publish"
  echo "build=$build"
  echo "stable_tag=$stable_tag"
  echo "signed_stable=$signed_stable"
  echo "signed_rc=$signed_rc"
  echo "detached=$detached"
} >> "${GITHUB_OUTPUT:?}"
