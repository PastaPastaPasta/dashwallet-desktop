#!/usr/bin/env bash
# GitHub Actions (release.yml): the version a run names its files with. Prints it and appends
# VERSION=<version> to $GITHUB_ENV.
#
#   ci/github/release-version.sh
#
# The app's version is the one in tauri.conf.json (the version source; Cargo.toml's workspace
# version is the same, 0.1.0 everywhere). By trigger:
#   tag vX.Y.Z      X.Y.Z, and it must equal tauri.conf.json: the binaries say what the tag says;
#   other tag       the tag without its `v` (a dry run such as v0.0.0-rel1);
#   anything else   <version>-dev.<run number>.g<short sha>, e.g. 0.1.0-dev.57.g1a2b3c4.
# File names carry no `+`: the immutable per-run release tag does (ci/github/release-publish.sh), the
# files do not.
set -euo pipefail

conf=$(sed -n 's/^  "version": "\(.*\)",$/\1/p' "$(dirname "${BASH_SOURCE[0]}")/../../rust/crates/dw-app/tauri.conf.json")
[[ -n "$conf" ]] || { echo "::error::no version in tauri.conf.json" >&2; exit 1; }

if [[ "${GITHUB_REF_TYPE:-}" == tag ]]; then
  version=${GITHUB_REF_NAME#v}
  if [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && "$version" != "$conf" ]]; then
    echo "::error::tag $GITHUB_REF_NAME but tauri.conf.json says version $conf" >&2
    exit 1
  fi
else
  sha=${GITHUB_SHA:?set GITHUB_SHA}
  version="$conf-dev.${GITHUB_RUN_NUMBER:?set GITHUB_RUN_NUMBER}.g${sha:0:7}"
fi
echo "release version $version (tauri.conf.json $conf)"
echo "VERSION=$version" >> "${GITHUB_ENV:?}"
