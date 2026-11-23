#!/usr/bin/env bash
# GitHub Actions: the Tenderdash proto sources that tenderdash-proto's build script downloads
# (github.com/dashpay/tenderdash/archive/<commitish>.zip), so a build needs no live download. The
# build script uses $CARGO_TARGET_DIR/tenderdash-<commitish>.zip when that file exists, so this
# puts the archive in rust/target and exports CARGO_TARGET_DIR=rust/target, where cargo builds
# anyway.
#
# The archive comes from $PINNED_CACHE_DIR (restored from the Actions cache by
# .github/actions/pinned-sources) or, when it is not there, from upstream with retries. Either
# way it is checked against the SHA-256 below before it is used; a mismatch stops the job.
#
# The pin is for the tenderdash-proto that rust/Cargo.lock takes: rs-tenderdash-abci at $rev,
# whose proto/build.rs defaults TENDERDASH_COMMITISH to $version.
set -euo pipefail

rev=eebfc46cc64b27022ccbbb3924604dd87a10c340
version=v1.8.0
sha256=39489a7459121cff12dd4487fe0fa42b88bf607bda3a64c778bbc5a27f524c50

cache="${PINNED_CACHE_DIR:?set PINNED_CACHE_DIR}"
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
lock="$root/rust/Cargo.lock"
source=$(awk '$0 == "name = \"tenderdash-proto\"" { found = 1; next } found && /^source = / { gsub(/^source = "|"$/, ""); print; found = 0 }' "$lock")
shown=${source//$'\n'/ and }
if [[ -z "$source" || "$source" == *$'\n'* || "$source" != *"rev=$rev#"* ]]; then
  echo "::error::tenderdash-sources: rust/Cargo.lock takes tenderdash-proto from ${shown:-nowhere}, and the pin in ci/github/tenderdash-sources.sh is for rs-tenderdash-abci $rev. Read DEFAULT_VERSION in proto/build.rs at the new rev, download https://github.com/dashpay/tenderdash/archive/<that version>.zip twice, check both give one SHA-256, and set rev, version and sha256 in ci/github/tenderdash-sources.sh." >&2
  exit 1
fi
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  echo "::error::tenderdash-sources: CARGO_TARGET_DIR is already set ($CARGO_TARGET_DIR); this job's build would not read the archive from rust/target." >&2
  exit 1
fi

zip="$cache/tenderdash-$version.zip"
mkdir -p "$cache"
if [[ -f "$zip" ]]; then
  from="the Actions cache"
else
  from="https://github.com/dashpay/tenderdash/archive/$version.zip"
  curl -fsSL --retry 5 --retry-all-errors --retry-delay 5 --connect-timeout 30 -o "$zip.part" "$from"
  mv "$zip.part" "$zip"
fi
if command -v sha256sum >/dev/null; then actual="$(sha256sum <"$zip")"; else actual="$(shasum -a 256 <"$zip")"; fi
actual="${actual%% *}"
if [[ "$actual" != "$sha256" ]]; then
  if [[ "$from" == "the Actions cache" ]]; then
    hint="The cache entry is corrupt or was tampered with: delete it (gh cache delete '${PINNED_CACHE_KEY:-pinned-sources-…}') and rerun."
  else
    hint="Upstream now serves a different archive for $version: find out why before changing the pin."
  fi
  echo "::error::tenderdash-sources: SHA-256 mismatch for tenderdash-$version.zip from $from: got $actual, pinned $sha256. Nothing was built from it. $hint" >&2
  exit 1
fi
echo "tenderdash-sources: tenderdash-$version.zip from $from, SHA-256 $actual (pinned)"

target="$root/rust/target"
mkdir -p "$target"
cp "$zip" "$target/tenderdash-$version.zip"
if command -v cygpath >/dev/null; then target=$(cygpath -w "$target"); fi
echo "CARGO_TARGET_DIR=$target" >> "${GITHUB_ENV:?}"
