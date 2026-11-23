#!/usr/bin/env bash
# GitHub Actions (Release's SBOM job, Linux x86_64): install cargo-cyclonedx 0.5.9 from the
# CycloneDX release tarball into $RUNNER_TEMP/cargo-cyclonedx-bin and put it on the job's PATH.
# The tarball is checked against the SHA-256 below on every use; it equals the digest GitHub lists
# for the release asset, and two downloads gave the same file. A new version needs its URL and
# SHA-256 here.
# With PINNED_CACHE_DIR set, the tarball is read from there when the Actions cache restored it,
# and downloaded into it otherwise.
set -euo pipefail

version=0.5.9
asset=cargo-cyclonedx-x86_64-unknown-linux-gnu
sha256=fb8dbee9f182173e062a64a387b21a0badc6fab8b2abf9294973f012972bf6d8
[[ "$(uname -s)/$(uname -m)" == Linux/x86_64 ]] ||
  { echo "install-cargo-cyclonedx: only Linux x86_64 has a pinned tarball" >&2; exit 1; }

cache="${PINNED_CACHE_DIR:-}"
tarball="${cache:-${RUNNER_TEMP:?}}/$asset-$version.tar.xz"
from="https://github.com/CycloneDX/cyclonedx-rust-cargo/releases/download/cargo-cyclonedx-$version/$asset.tar.xz"
if [[ -n "$cache" && -f "$tarball" ]]; then
  from="the Actions cache"
else
  mkdir -p "$(dirname "$tarball")"
  curl -fsSL --retry 5 --retry-all-errors --retry-delay 5 --connect-timeout 30 -o "$tarball.part" "$from"
  mv "$tarball.part" "$tarball"
fi
actual=$(sha256sum < "$tarball")
if [[ "${actual%% *}" != "$sha256" ]]; then
  hint=
  [[ "$from" != "the Actions cache" ]] || hint=" The cache entry is corrupt or was tampered with: delete it (gh cache delete '${PINNED_CACHE_KEY:-cargo-cyclonedx-…}') and rerun."
  echo "::error::install-cargo-cyclonedx: SHA-256 mismatch for $asset.tar.xz from $from: ${actual%% *}, pinned $sha256.$hint" >&2
  exit 1
fi
echo "install-cargo-cyclonedx: $asset.tar.xz $version from $from, SHA-256 matches the pin"
dest="${RUNNER_TEMP:?}/cargo-cyclonedx-bin"
rm -rf "$dest"
mkdir -p "$dest"
tar -xJf "$tarball" -C "$dest" --strip-components=1 "$asset/cargo-cyclonedx"
[[ -n "$cache" ]] || rm "$tarball"
echo "$dest" >> "${GITHUB_PATH:?}"
"$dest/cargo-cyclonedx" cyclonedx --version
