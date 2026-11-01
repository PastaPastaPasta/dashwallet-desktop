#!/usr/bin/env bash
# GitHub Actions (release.yml): copy the one bundle a glob matches to OUT_DIR under its stable
# release name, dash-wallet-<version>-<suffix>, and echo its path. Tauri's own names carry a space
# ("Dash Wallet_0.1.0_x64-setup.exe") and vary by OS.
#
#   ci/github/release-stage.sh OUT_DIR VERSION SUFFIX GLOB
#
# GLOB is relative to rust/target (resolved through `pwd`: on Windows $GITHUB_WORKSPACE is a
# `D:\a\...` path whose backslashes a glob reads as escapes). The bundle must be exactly one file.
set -euo pipefail

out=${1:?usage: $0 OUT_DIR VERSION SUFFIX GLOB}
version=${2:?}
suffix=${3:?}
glob=${4:?}

target=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../rust/target" && pwd)
# No mapfile: macOS runners have bash 3.2.
matches=()
while IFS= read -r line; do matches+=("$line"); done < <(compgen -G "$target/$glob" || true)
if [[ ${#matches[@]} -ne 1 ]]; then
  echo "::error::release-stage: expected one file for $glob, found ${#matches[@]}: ${matches[*]:-none}" >&2
  exit 1
fi
mkdir -p "$out"
dest="$out/dash-wallet-$version-$suffix"
cp "${matches[0]}" "$dest"
echo "release-stage: ${matches[0]} -> $dest ($(wc -c <"$dest") bytes)" >&2
echo "$dest"
