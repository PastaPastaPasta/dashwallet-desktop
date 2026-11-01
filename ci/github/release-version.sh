#!/usr/bin/env bash
# GitHub Actions (release.yml): the version a release run builds. Prints it and appends
# VERSION=<version> to $GITHUB_ENV.
#
#   ci/github/release-version.sh
#
# The version is the tag without its `v` (v0.1.0 -> 0.1.0). A plain vX.Y.Z tag must equal the
# version in tauri.conf.json, which is what the binaries and bundles say; a test tag such as
# v0.0.0-rel1 is only named by its tag.
set -euo pipefail

tag=${GITHUB_REF_NAME:?set GITHUB_REF_NAME}
version=${tag#v}
conf=$(sed -n 's/^  "version": "\(.*\)",$/\1/p' "$(dirname "${BASH_SOURCE[0]}")/../../rust/crates/dw-app/tauri.conf.json")
if [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && "$version" != "$conf" ]]; then
  echo "::error::tag $tag but tauri.conf.json says version $conf" >&2
  exit 1
fi
echo "release version $version (tauri.conf.json $conf)"
echo "VERSION=$version" >> "${GITHUB_ENV:?}"
